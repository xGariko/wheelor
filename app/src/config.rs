//! Configurazione: file TOML in %APPDATA%\Wheelor\config.toml.
//!
//! `serde` genera da solo il codice per leggere e scrivere queste struct:
//! basta annotarle con `#[derive(Serialize, Deserialize)]`.
//!
//! Il modello ha due parti:
//! - la **libreria** (`[[action]]`): tutte le azioni salvate, senza limite di numero;
//! - le **ruote** (`[[wheel]]`): ognuna mostra fino a 8 elementi, che sono azioni della
//!   libreria o altre ruote (sottomenu). La ruota "main" è quella principale; una ruota con
//!   `apps` è il profilo di quelle app e si apre al posto della principale.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Elementi al massimo in una ruota.
pub const MAX_SLOTS: usize = 8;
/// Id della ruota principale: c'è sempre.
pub const MAIN_WHEEL: &str = "main";
/// Negli `slots` un sottomenu si scrive "wheel:<id>", un'azione semplicemente "<id>".
const WHEEL_PREFIX: &str = "wheel:";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Combinazione che apre la ruota, es. "Ctrl+Alt+W".
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(rename = "action", default)]
    pub actions: Vec<Item>,
    #[serde(rename = "wheel", default)]
    pub wheels: Vec<Wheel>,
}

/// Un'azione della libreria.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Identificativo stabile, usato dalle ruote per riferirsi all'azione.
    pub id: String,
    pub label: String,
    /// Codice esadecimale del glifo Segoe Fluent Icons, es. "E756".
    #[serde(default)]
    pub icon: String,
    /// `flatten`: i campi dell'azione (type, command, …) stanno allo stesso livello di id e label.
    #[serde(flatten)]
    pub action: Action,
}

/// Cosa fa un'azione. Nel file diventa `type = "command"`, `type = "open"`, …
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Comando da terminale.
    Command {
        command: String,
        #[serde(default)]
        shell: Shell,
        #[serde(default)]
        window: WindowMode,
        /// Cartella di lavoro (vuota = cartella utente). Accetta %VARIABILI% e ~.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        cwd: String,
        #[serde(default, skip_serializing_if = "is_false")]
        admin: bool,
    },
    /// Apre un'app, un file, una cartella o un URL.
    Open {
        target: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        args: String,
    },
    /// Invia una combinazione di tasti, es. "Ctrl+Shift+S".
    Keys { keys: String },
    /// Scrive un testo nell'app in primo piano.
    Text { text: String },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    /// Windows PowerShell 5.1, sempre presente.
    #[default]
    Powershell,
    /// PowerShell 7+, se installato.
    Pwsh,
    /// Prompt dei comandi.
    Cmd,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    /// In background, senza finestra. Se fallisce compare una notifica con l'errore.
    #[default]
    Hidden,
    /// In una finestra di terminale che si chiude alla fine.
    Visible,
    /// In una finestra di terminale che resta aperta.
    KeepOpen,
}

/// Una ruota: la principale, un sottomenu o il profilo di alcune app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wheel {
    pub id: String,
    pub name: String,
    /// Icona mostrata quando la ruota è un sottomenu.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    /// Eseguibili per cui questa ruota sostituisce la principale, es. ["code.exe"].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
    #[serde(default)]
    pub slots: Vec<Slot>,
}

/// Un elemento di una ruota. Nel file è una stringa: "terminale" o "wheel:media".
/// `from`/`into`: serde lo converte da e verso `String` con le implementazioni qui sotto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Slot {
    Action(String),
    Wheel(String),
}

impl From<String> for Slot {
    fn from(text: String) -> Self {
        match text.strip_prefix(WHEEL_PREFIX) {
            Some(id) => Slot::Wheel(id.to_string()),
            None => Slot::Action(text),
        }
    }
}

impl From<Slot> for String {
    fn from(slot: Slot) -> Self {
        match slot {
            Slot::Action(id) => id,
            Slot::Wheel(id) => format!("{WHEEL_PREFIX}{id}"),
        }
    }
}

fn default_hotkey() -> String {
    "Ctrl+Alt+W".into()
}

fn is_false(value: &bool) -> bool {
    !value
}

impl Wheel {
    pub fn new(id: &str, name: &str) -> Self {
        Self { id: id.into(), name: name.into(), icon: String::new(), apps: Vec::new(), slots: Vec::new() }
    }

    pub fn is_main(&self) -> bool {
        self.id == MAIN_WHEEL
    }

    /// Codice dell'icona da mostrare: quella scelta o, se manca, una generica (casa per la principale).
    pub fn icon_or_default(&self) -> &str {
        match self.icon.as_str() {
            "" if self.is_main() => "E80F",
            "" => "E8A9",
            code => code,
        }
    }

    /// Questa ruota è il profilo dell'app `path` (percorso completo dell'eseguibile)?
    pub fn matches_app(&self, path: &str) -> bool {
        self.apps.iter().any(|pattern| app_matches(pattern, path))
    }
}

/// Confronta un'app scritta dall'utente con il percorso dell'eseguibile, ignorando maiuscole:
/// "code.exe" e "code" valgono per qualunque Code.exe; un percorso completo solo per quello.
fn app_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim().to_lowercase();
    let path = path.to_lowercase();
    if pattern.is_empty() {
        return false;
    }
    if pattern.contains(['\\', '/']) {
        return path == pattern.replace('/', "\\");
    }
    let file = path.rsplit('\\').next().unwrap_or(&path);
    file == pattern || file.strip_suffix(".exe") == Some(&pattern)
}

impl Default for Config {
    fn default() -> Self {
        let item = |id: &str, label: &str, icon: &str, action| Item { id: id.into(), label: label.into(), icon: icon.into(), action };
        let open = |target: &str| Action::Open { target: target.into(), args: String::new() };
        let keys = |keys: &str| Action::Keys { keys: keys.into() };
        let actions = vec![
            item("terminale", "Terminale", "E756", open("wt.exe")),
            item("browser", "Browser", "E774", open("https://www.google.com")),
            item("esplora-file", "Esplora file", "E8B7", open("explorer.exe")),
            item("screenshot", "Screenshot", "E722", keys("Win+Shift+S")),
            item("copia", "Copia", "E8C8", keys("Ctrl+C")),
            item("incolla", "Incolla", "E77F", keys("Ctrl+V")),
            item(
                "blocca-pc",
                "Blocca PC",
                "E72E",
                Action::Command {
                    command: "rundll32.exe user32.dll,LockWorkStation".into(),
                    shell: Shell::Cmd,
                    window: WindowMode::Hidden,
                    cwd: String::new(),
                    admin: false,
                },
            ),
            item("play-pausa", "Play/Pausa", "E768", keys("MediaPlayPause")),
            item("brano-successivo", "Successivo", "E893", keys("MediaNext")),
            item("brano-precedente", "Precedente", "E892", keys("MediaPrev")),
            item("volume-su", "Volume +", "E995", keys("VolumeUp")),
            item("volume-giu", "Volume −", "E993", keys("VolumeDown")),
            item("muto", "Muto", "E74F", keys("VolumeMute")),
        ];

        let action = |id: &str| Slot::Action(id.into());
        let mut main = Wheel::new(MAIN_WHEEL, "Principale");
        main.slots = ["terminale", "browser", "esplora-file", "screenshot", "copia", "incolla"].map(action).to_vec();
        main.slots.push(Slot::Wheel("media".into()));
        main.slots.push(action("blocca-pc"));

        let mut media = Wheel::new("media", "Media");
        media.icon = "E8D6".into();
        media.slots =
            ["play-pausa", "brano-successivo", "volume-su", "muto", "volume-giu", "brano-precedente"].map(action).to_vec();

        Self { hotkey: default_hotkey(), actions, wheels: vec![main, media] }
    }
}

impl Config {
    pub fn action(&self, id: &str) -> Option<&Item> {
        self.actions.iter().find(|a| a.id == id)
    }

    pub fn wheel(&self, id: &str) -> Option<&Wheel> {
        self.wheels.iter().find(|w| w.id == id)
    }

    /// La ruota da aprire quando in primo piano c'è l'app `path`: il primo profilo
    /// che la contiene, altrimenti la principale.
    pub fn wheel_for_app(&self, path: &str) -> &Wheel {
        self.wheels
            .iter()
            .find(|w| !w.is_main() && w.matches_app(path))
            .or_else(|| self.wheel(MAIN_WHEEL))
            .expect("la ruota principale c'è sempre (vedi `normalize`)")
    }

    /// Garantisce che la ruota principale esista e sia la prima.
    fn normalize(&mut self) {
        match self.wheels.iter().position(Wheel::is_main) {
            Some(0) => {}
            Some(index) => {
                let main = self.wheels.remove(index);
                self.wheels.insert(0, main);
            }
            None => self.wheels.insert(0, Wheel::new(MAIN_WHEEL, "Principale")),
        }
    }

    /// Controlla che la configurazione sia coerente. Il messaggio d'errore dice cosa correggere.
    pub fn check(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        for item in &self.actions {
            if item.id.trim().is_empty() {
                return Err(format!("L'azione \"{}\" non ha un id", item.label));
            }
            // `insert` restituisce false se l'elemento c'era già.
            if !ids.insert(item.id.as_str()) {
                return Err(format!("Ci sono due azioni con id \"{}\"", item.id));
            }
        }

        let mut wheel_ids = HashSet::new();
        for wheel in &self.wheels {
            if wheel.id.trim().is_empty() {
                return Err(format!("La ruota \"{}\" non ha un id", wheel.name));
            }
            if !wheel_ids.insert(wheel.id.as_str()) {
                return Err(format!("Ci sono due ruote con id \"{}\"", wheel.id));
            }
        }

        for wheel in &self.wheels {
            if wheel.slots.len() > MAX_SLOTS {
                return Err(format!("La ruota \"{}\" ha più di {MAX_SLOTS} elementi", wheel.name));
            }
            for slot in &wheel.slots {
                match slot {
                    Slot::Action(id) if !ids.contains(id.as_str()) => {
                        return Err(format!("La ruota \"{}\" usa l'azione \"{id}\", che non esiste", wheel.name));
                    }
                    Slot::Wheel(id) if !wheel_ids.contains(id.as_str()) => {
                        return Err(format!("La ruota \"{}\" usa la ruota \"{id}\", che non esiste", wheel.name));
                    }
                    _ => {}
                }
            }
        }

        if let Some(wheel) = self.wheels.iter().find(|w| self.reaches(&w.id, &w.id)) {
            return Err(format!("La ruota \"{}\" contiene sé stessa tramite i suoi sottomenu", wheel.name));
        }
        Ok(())
    }

    /// Dalla ruota `from`, aprendo sottomenu, si arriva alla ruota `to`?
    /// Serve a impedire i cicli (A contiene B che contiene A).
    pub fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen = HashSet::new();
        let mut pending: Vec<&str> = self.submenus(from).collect();
        // Visita in profondità con una pila esplicita: niente ricorsione, e `seen` evita
        // di girare all'infinito proprio quando un ciclo c'è.
        while let Some(id) = pending.pop() {
            if id == to {
                return true;
            }
            if seen.insert(id) {
                pending.extend(self.submenus(id));
            }
        }
        false
    }

    /// Id dei sottomenu della ruota `id`. `impl Iterator`: restituiamo un iteratore
    /// "pigro" senza dover creare un Vec.
    fn submenus<'a>(&'a self, id: &str) -> impl Iterator<Item = &'a str> {
        self.wheel(id).into_iter().flat_map(|w| &w.slots).filter_map(|slot| match slot {
            Slot::Wheel(id) => Some(id.as_str()),
            Slot::Action(_) => None,
        })
    }
}

/// Un id leggibile ricavato da `label` ("Esplora file" → "esplora-file"),
/// con un numero in fondo se `taken` dice che è già usato.
pub fn unique_id(label: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut base = String::new();
    for c in label.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            base.push(c);
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    let base = base.trim_end_matches('-');
    let base = if base.is_empty() { "azione" } else { base };
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).unwrap()
}

pub fn path() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").unwrap_or_default();
    PathBuf::from(appdata).join("Wheelor").join("config.toml")
}

/// Ultima modifica del file, per capire se va ricaricato.
pub fn modified() -> Option<SystemTime> {
    fs::metadata(path()).and_then(|m| m.modified()).ok()
}

/// Legge la configurazione. Al primo avvio crea il file con le azioni di esempio;
/// un file nel vecchio formato viene convertito (e l'originale conservato a parte).
pub fn load() -> Result<Config, String> {
    let path = path();
    if !path.exists() {
        let config = Config::default();
        save(&config)?;
        return Ok(config);
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("Impossibile leggere {}: {e}", path.display()))?;
    let error = |e: toml::de::Error| format!("Errore in {}:\n{e}", path.display());

    let (config, migrated) = parse(&text).map_err(error)?;
    config.check().map_err(|e| format!("Errore in {}:\n{e}", path.display()))?;
    if migrated {
        let backup = path.with_file_name("config.v1.toml");
        fs::copy(&path, &backup).map_err(|e| format!("Impossibile copiare {}: {e}", backup.display()))?;
        save(&config)?;
    }
    Ok(config)
}

/// Interpreta il testo del file. Il booleano dice se era nel vecchio formato.
fn parse(text: &str) -> Result<(Config, bool), toml::de::Error> {
    let mut config: Config = toml::from_str(text)?;
    let mut migrated = false;
    if config.actions.is_empty() && config.wheels.is_empty() {
        let legacy: Legacy = toml::from_str(text)?;
        if !legacy.slices.is_empty() {
            config = migrate(config.hotkey, legacy.slices);
            migrated = true;
        }
    }
    config.normalize();
    Ok((config, migrated))
}

/// Il formato della prima versione: un elenco di spicchi, ognuno con la sua azione.
#[derive(Deserialize)]
struct Legacy {
    #[serde(rename = "slice", default)]
    slices: Vec<LegacySlice>,
}

#[derive(Deserialize)]
struct LegacySlice {
    label: String,
    #[serde(default)]
    icon: String,
    action: Action,
}

/// Ogni vecchio spicchio diventa un'azione della libreria; i primi 8 formano la ruota principale.
fn migrate(hotkey: String, slices: Vec<LegacySlice>) -> Config {
    let mut actions: Vec<Item> = Vec::new();
    for slice in slices {
        let id = unique_id(&slice.label, |id| actions.iter().any(|a| a.id == id));
        actions.push(Item { id, label: slice.label, icon: slice.icon, action: slice.action });
    }
    let mut main = Wheel::new(MAIN_WHEEL, "Principale");
    main.slots = actions.iter().take(MAX_SLOTS).map(|a| Slot::Action(a.id.clone())).collect();
    Config { hotkey, actions, wheels: vec![main] }
}

/// Salva in modo atomico: scrive un file temporaneo e poi lo sostituisce all'originale,
/// così chi legge non trova mai un file scritto a metà.
pub fn save(config: &Config) -> Result<(), String> {
    let path = path();
    let text = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Impossibile creare {}: {e}", dir.display()))?;
    }
    let temp = path.with_extension("toml.tmp");
    fs::write(&temp, text).map_err(|e| format!("Impossibile scrivere {}: {e}", temp.display()))?;
    fs::rename(&temp, &path).map_err(|e| format!("Impossibile salvare {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_survives_a_round_trip() {
        let config = Config::default();
        config.check().unwrap();
        let text = toml::to_string_pretty(&config).unwrap();
        let (back, migrated) = parse(&text).unwrap();
        assert!(!migrated);
        assert_eq!(back, config);
    }

    #[test]
    fn actions_are_written_flat() {
        let text = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(text.contains("[[action]]\nid = \"terminale\"\nlabel = \"Terminale\"\nicon = \"E756\"\ntype = \"open\""));
        assert!(text.contains("\"wheel:media\""));
    }

    #[test]
    fn missing_fields_get_defaults() {
        let text = r#"
            [[action]]
            id = "ciao"
            label = "Ciao"
            type = "command"
            command = "echo ciao"
        "#;
        let (config, _) = parse(text).unwrap();
        assert_eq!(config.hotkey, "Ctrl+Alt+W");
        assert_eq!(
            config.actions[0].action,
            Action::Command {
                command: "echo ciao".into(),
                shell: Shell::Powershell,
                window: WindowMode::Hidden,
                cwd: String::new(),
                admin: false,
            }
        );
        // La ruota principale viene creata anche se il file non la contiene.
        assert!(config.wheels[0].is_main());
    }

    #[test]
    fn old_format_is_migrated() {
        let mut text = String::from("hotkey = \"Ctrl+Alt+Q\"\n");
        for i in 1..=10 {
            let label = if i <= 2 { "Uguale".to_string() } else { format!("Azione {i}") };
            text += &format!("[[slice]]\nlabel = \"{label}\"\n[slice.action]\ntype = \"keys\"\nkeys = \"F{i}\"\n");
        }
        let (config, migrated) = parse(&text).unwrap();
        assert!(migrated);
        assert_eq!(config.hotkey, "Ctrl+Alt+Q");
        assert_eq!(config.actions.len(), 10);
        assert_eq!(config.actions[0].id, "uguale");
        assert_eq!(config.actions[1].id, "uguale-2");
        assert_eq!(config.actions[9].id, "azione-10");
        assert_eq!(config.wheels.len(), 1);
        assert_eq!(config.wheels[0].slots.len(), MAX_SLOTS);
        assert_eq!(config.wheels[0].slots[1], Slot::Action("uguale-2".into()));
        config.check().unwrap();
    }

    #[test]
    fn check_finds_problems() {
        let mut config = Config::default();
        config.wheels[0].slots.push(Slot::Action("copia".into()));
        assert!(config.check().unwrap_err().contains("più di 8"));

        let mut config = Config::default();
        config.wheels[1].slots[0] = Slot::Action("non-esiste".into());
        assert!(config.check().unwrap_err().contains("non esiste"));

        // Media contiene la principale, che contiene Media: ciclo.
        let mut config = Config::default();
        config.wheels[1].slots[0] = Slot::Wheel(MAIN_WHEEL.into());
        assert!(config.check().unwrap_err().contains("sé stessa"));
        assert!(config.reaches("media", "media"));
    }

    #[test]
    fn profiles_follow_the_foreground_app() {
        let mut config = Config::default();
        let mut code = Wheel::new("vscode", "VS Code");
        code.apps = vec!["code.exe".into()];
        config.wheels.push(code);
        let mut notepad = Wheel::new("blocco", "Blocco note");
        notepad.apps = vec!["notepad".into(), "C:/Tools/Editor.exe".into()];
        config.wheels.push(notepad);

        let open = |path: &str| config.wheel_for_app(path).id.clone();
        assert_eq!(open(r"C:\Users\Io\AppData\Local\Programs\Microsoft VS Code\Code.exe"), "vscode");
        assert_eq!(open(r"C:\Windows\System32\NOTEPAD.EXE"), "blocco");
        assert_eq!(open(r"C:\Tools\editor.exe"), "blocco");
        assert_eq!(open(r"D:\Altro\Editor.exe"), MAIN_WHEEL);
        assert_eq!(open(""), MAIN_WHEEL);
    }

    #[test]
    fn unique_ids_are_readable() {
        assert_eq!(unique_id("Esplora file", |_| false), "esplora-file");
        assert_eq!(unique_id("  Play/Pausa! ", |_| false), "play-pausa");
        assert_eq!(unique_id("Proprietà", |_| false), "proprietà");
        assert_eq!(unique_id("???", |_| false), "azione");
        assert_eq!(unique_id("Copia", |id| id == "copia" || id == "copia-2"), "copia-3");
    }
}
