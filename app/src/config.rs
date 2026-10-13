//! Configurazione: file TOML in %APPDATA%\Wheelor\config.toml.
//!
//! `serde` genera da solo il codice per leggere e scrivere queste struct:
//! basta annotarle con `#[derive(Serialize, Deserialize)]`.
//!
//! Il modello ha due parti:
//! - la **libreria** (`[[action]]`): tutte le azioni salvate, senza limite di numero. Ogni azione
//!   è una sequenza di passi (aprire, premere tasti, aspettare…) eseguiti uno dopo l'altro;
//! - le **ruote** (`[[wheel]]`): ognuna mostra fino a 8 elementi, che sono azioni della
//!   libreria o altre ruote (sottomenu). La ruota "main" è quella principale; una ruota con
//!   `apps` è il profilo di quelle app e si apre al posto della principale.
//!
//! Chiavi e valori del file sono in inglese, qualunque sia la lingua dell'interfaccia.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::i18n::Language;
use crate::{tr, trf};

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
    /// Lingua dell'interfaccia; senza, quella di Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    /// Dimensione della ruota sullo schermo. Prima delle tabelle: in TOML i valori semplici
    /// vanno scritti prima di `[[action]]` e `[[wheel]]`.
    #[serde(default, skip_serializing_if = "is_default")]
    pub size: WheelSize,
    /// Opacità della ruota in percentuale, da MIN_OPACITY a 100.
    #[serde(default = "full_opacity", skip_serializing_if = "is_full_opacity")]
    pub opacity: u8,
    /// Colore principale di ruota e impostazioni.
    #[serde(default, skip_serializing_if = "is_default")]
    pub accent: Accent,
    /// true: conta solo la direzione del mouse, anche fuori dalla ruota, e un clic ovunque
    /// sceglie lo spicchio puntato. false: fuori dalla ruota non si punta nulla e un clic la chiude.
    #[serde(default, skip_serializing_if = "is_false")]
    pub aim_anywhere: bool,
    /// Controlla da solo se c'è una nuova versione (su wheelor.app).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub check_updates: bool,
    #[serde(rename = "action", default)]
    pub actions: Vec<Item>,
    #[serde(rename = "wheel", default)]
    pub wheels: Vec<Wheel>,
}

/// Un'azione della libreria: una sequenza di passi.
///
/// Nel file un'azione con un solo passo si scrive come sempre, con i campi del passo (type,
/// command, …) allo stesso livello di id e label; con più passi, ognuno è un `[[action.step]]`:
///
/// ```toml
/// [[action]]
/// id = "meeting"
/// label = "Inizio riunione"
///
/// [[action.step]]
/// type = "open"
/// target = "https://teams.microsoft.com/…"
///
/// [[action.step]]
/// type = "wait_window"
/// app = "ms-teams.exe"
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Identificativo stabile, usato dalle ruote per riferirsi all'azione.
    pub id: String,
    pub label: String,
    /// Codice esadecimale del glifo Segoe Fluent Icons, es. "E756".
    pub icon: String,
    /// Combinazione che esegue l'azione senza aprire la ruota (vuota = nessuna).
    pub hotkey: String,
    /// I passi, eseguiti uno dopo l'altro: sempre almeno uno.
    pub steps: Vec<Step>,
}

impl Item {
    pub fn new(label: &str, icon: &str, steps: Vec<Step>) -> Self {
        Self { id: String::new(), label: label.into(), icon: icon.into(), hotkey: String::new(), steps }
    }

    /// Le azioni della libreria eseguite dai passi "Esegui azione".
    pub fn runs(&self) -> impl Iterator<Item = &str> {
        self.steps.iter().filter_map(|step| match step {
            Step::Run { action } => Some(action.as_str()),
            _ => None,
        })
    }
}

impl Serialize for Item {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Due forme: un passo solo, "appiattito" accanto a id e label (come nelle prime versioni),
        // oppure l'elenco dei passi in `[[action.step]]`.
        #[derive(Serialize)]
        struct Single<'a> {
            id: &'a str,
            label: &'a str,
            icon: &'a str,
            #[serde(skip_serializing_if = "str::is_empty")]
            hotkey: &'a str,
            #[serde(flatten)]
            step: &'a Step,
        }
        #[derive(Serialize)]
        struct Steps<'a> {
            id: &'a str,
            label: &'a str,
            icon: &'a str,
            #[serde(skip_serializing_if = "str::is_empty")]
            hotkey: &'a str,
            #[serde(rename = "step")]
            steps: &'a [Step],
        }
        let (id, label, icon, hotkey) = (self.id.as_str(), self.label.as_str(), self.icon.as_str(), self.hotkey.as_str());
        match self.steps.as_slice() {
            [step] => Single { id, label, icon, hotkey, step }.serialize(serializer),
            steps => Steps { id, label, icon, hotkey, steps }.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Item {
    /// Legge prima tutta la tabella, poi decide quale delle due forme è: così gli errori di un
    /// passo arrivano con il loro messaggio preciso ("missing field `target`", …).
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut table = toml::Table::deserialize(deserializer)?;
        let mut text = |key: &str, required: bool| match table.remove(key) {
            Some(toml::Value::String(value)) => Ok(value),
            Some(_) => Err(D::Error::custom(format!("`{key}` must be a string"))),
            None if required => Err(D::Error::custom(format!("missing field `{key}`"))),
            None => Ok(String::new()),
        };
        let id = text("id", true)?;
        let label = text("label", true)?;
        let icon = text("icon", false)?;
        let hotkey = text("hotkey", false)?;
        let error = |e: toml::de::Error| D::Error::custom(format!("action `{id}`: {e}"));
        if table.contains_key("step") && table.contains_key("type") {
            return Err(D::Error::custom(format!("action `{id}`: use either `type` or `[[action.step]]`, not both")));
        }
        let steps = match table.remove("step") {
            Some(steps) => Vec::<Step>::deserialize(steps).map_err(error)?,
            None => vec![Step::deserialize(toml::Value::Table(table)).map_err(error)?],
        };
        Ok(Item { id, label, icon, hotkey, steps })
    }
}

/// Cosa fa un passo di un'azione. Nel file diventa `type = "command"`, `type = "open"`, …
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
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
        /// Con la finestra visibile: il passo successivo aspetta che il comando finisca.
        /// Nascosto, il comando si aspetta sempre (serve a leggerne il risultato e gli errori).
        #[serde(default, skip_serializing_if = "is_false")]
        wait: bool,
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
    /// Aspetta un po' prima del passo successivo.
    Wait { seconds: f32 },
    /// Aspetta che un'app (o una finestra con quel titolo) sia in primo piano; se è già aperta
    /// ma dietro ad altre, la porta davanti.
    WaitWindow {
        /// Eseguibile, come per le app collegate: "code.exe", "code" o il percorso completo.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        app: String,
        /// Parte del titolo della finestra (maiuscole e minuscole non contano).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        title: String,
        /// Secondi dopo i quali l'azione si ferma con un errore.
        #[serde(default = "default_timeout", skip_serializing_if = "is_default_timeout")]
        timeout: f32,
    },
    /// Fa qualcosa alla finestra in primo piano.
    Window {
        #[serde(rename = "do")]
        op: WindowOp,
    },
    /// Funzioni di Windows: blocco, volume, musica…
    System {
        #[serde(rename = "do")]
        op: SystemOp,
    },
    /// Appunti: copia la selezione, incolla, o mette un testo negli appunti.
    Clipboard {
        #[serde(rename = "do")]
        op: ClipboardOp,
        /// Il testo da mettere negli appunti (solo per `set`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        text: String,
    },
    /// Esegue un'altra azione della libreria, con tutti i suoi passi.
    Run { action: String },
}

impl Step {
    /// Il tipo di passo, come appare nelle impostazioni.
    pub fn name(&self) -> &'static str {
        match self {
            Step::Open { .. } => tr!("Apri", "Open"),
            Step::Keys { .. } => tr!("Tasti", "Keys"),
            Step::Text { .. } => tr!("Testo", "Text"),
            Step::Command { .. } => tr!("Comando", "Command"),
            Step::Wait { .. } => tr!("Attendi", "Wait"),
            Step::WaitWindow { .. } => tr!("Aspetta la finestra", "Wait for window"),
            Step::Window { .. } => tr!("Finestra", "Window"),
            Step::System { .. } => tr!("Sistema", "System"),
            Step::Clipboard { .. } => tr!("Appunti", "Clipboard"),
            Step::Run { .. } => tr!("Esegui azione", "Run action"),
        }
    }

    /// I campi di testo del passo, dove possono esserci segnaposto.
    pub fn texts(&self) -> Vec<&str> {
        match self {
            Step::Command { command, cwd, .. } => vec![command, cwd],
            Step::Open { target, args } => vec![target, args],
            Step::Text { text } | Step::Clipboard { text, .. } => vec![text],
            Step::WaitWindow { app, title, .. } => vec![app, title],
            _ => Vec::new(),
        }
    }
}

/// Cosa fare alla finestra. Tutte agiscono su quella in primo piano, tranne `focus`, che torna a
/// quella che era in primo piano quando si è aperta la ruota.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowOp {
    #[default]
    Focus,
    Minimize,
    Maximize,
    Restore,
    LeftHalf,
    RightHalf,
    AlwaysOnTop,
    Close,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemOp {
    #[default]
    Lock,
    Sleep,
    ScreenOff,
    VolumeUp,
    VolumeDown,
    Mute,
    PlayPause,
    NextTrack,
    PreviousTrack,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardOp {
    /// Copia il testo selezionato nell'app in primo piano (Ctrl+C, aspettando che arrivi).
    #[default]
    Copy,
    /// Incolla (Ctrl+V).
    Paste,
    /// Mette negli appunti `text`, dopo averne sostituito i segnaposto.
    Set,
}

impl WindowOp {
    pub const ALL: [WindowOp; 8] = [
        WindowOp::Focus,
        WindowOp::Minimize,
        WindowOp::Maximize,
        WindowOp::Restore,
        WindowOp::LeftHalf,
        WindowOp::RightHalf,
        WindowOp::AlwaysOnTop,
        WindowOp::Close,
    ];
}

impl SystemOp {
    pub const ALL: [SystemOp; 9] = [
        SystemOp::Lock,
        SystemOp::Sleep,
        SystemOp::ScreenOff,
        SystemOp::VolumeUp,
        SystemOp::VolumeDown,
        SystemOp::Mute,
        SystemOp::PlayPause,
        SystemOp::NextTrack,
        SystemOp::PreviousTrack,
    ];
}

impl ClipboardOp {
    pub const ALL: [ClipboardOp; 3] = [ClipboardOp::Copy, ClipboardOp::Paste, ClipboardOp::Set];
}

/// Quanto aspetta al massimo "Aspetta la finestra", se non è scritto (secondi).
pub const DEFAULT_TIMEOUT: f32 = 10.0;

fn default_timeout() -> f32 {
    DEFAULT_TIMEOUT
}

fn is_default_timeout(value: &f32) -> bool {
    *value == DEFAULT_TIMEOUT
}

/// Dimensione della ruota: la misura normale, ingrandita del 15% o del 30%.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WheelSize {
    #[default]
    Normal,
    Large,
    Huge,
}

impl WheelSize {
    /// Fattore di ingrandimento rispetto alla misura normale.
    pub fn zoom(self) -> f32 {
        match self {
            WheelSize::Normal => 1.0,
            WheelSize::Large => 1.15,
            WheelSize::Huge => 1.3,
        }
    }
}

/// Opacità minima della ruota (%): più trasparente non si leggerebbe più.
pub const MIN_OPACITY: u8 = 40;

/// Colore principale. Tutti accesi come l'acqua del sito, con luminosità e saturazione percepite
/// simili (calcolate nello spazio OKLCH), così stanno bene sul tema scuro e con il testo scuro sopra.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Aqua,
    Blue,
    Violet,
    Pink,
    Coral,
    Amber,
    Lime,
    Mint,
}

/// Le tinte che derivano da un colore principale (esadecimali, come nei CSS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub accent: u32,
    /// Un po' più chiaro: pulsanti sotto il mouse, bordi accesi.
    pub hover: u32,
    /// Un po' più scuro: pulsanti premuti.
    pub press: u32,
    /// Testo e icone sopra il colore principale: quasi nero, della stessa tinta.
    pub ink: u32,
    /// Gradiente della stella del logo, dal centro al bordo.
    pub star_light: u32,
    pub star_dark: u32,
}

impl Accent {
    /// Nell'ordine del cerchio dei colori, partendo dall'acqua del logo.
    pub const ALL: [Accent; 8] =
        [Accent::Aqua, Accent::Blue, Accent::Violet, Accent::Pink, Accent::Coral, Accent::Amber, Accent::Lime, Accent::Mint];

    pub fn name(self) -> &'static str {
        match self {
            Accent::Aqua => tr!("Acqua", "Aqua"),
            Accent::Blue => tr!("Blu", "Blue"),
            Accent::Violet => tr!("Viola", "Violet"),
            Accent::Pink => tr!("Rosa", "Pink"),
            Accent::Coral => tr!("Corallo", "Coral"),
            Accent::Amber => tr!("Ambra", "Amber"),
            Accent::Lime => tr!("Lime", "Lime"),
            Accent::Mint => tr!("Menta", "Mint"),
        }
    }

    pub fn palette(self) -> Palette {
        let p = |accent, hover, press, ink, star_light, star_dark| Palette { accent, hover, press, ink, star_light, star_dark };
        match self {
            Accent::Aqua => p(0x6FE0E6, 0x8FE9EE, 0x5CCBD1, 0x06191B, 0x83EAF0, 0x49C8CE),
            Accent::Blue => p(0x65C1FE, 0x82CCFF, 0x53ADE8, 0x081722, 0x81CBFF, 0x42A9E9),
            Accent::Violet => p(0xBB95FC, 0xC4A4FD, 0xA782E5, 0x181224, 0xC4A3FF, 0xA57AE7),
            Accent::Pink => p(0xFF7DB5, 0xFF94BF, 0xE86BA1, 0x240D16, 0xFF93BF, 0xE9609D),
            Accent::Coral => p(0xFF8E7C, 0xFFA192, 0xE87B6B, 0x230F0C, 0xFFA191, 0xE87261),
            Accent::Amber => p(0xFCB739, 0xFFC76A, 0xE6A41F, 0x1F1302, 0xFFC667, 0xE29F00),
            Accent::Lime => p(0xADE74E, 0xBEF176, 0x9AD239, 0x0F1902, 0xBAF266, 0x92CF08),
            Accent::Mint => p(0x51EEAD, 0x7CF5BE, 0x3AD89A, 0x031B10, 0x6BF7BA, 0x03D593),
        }
    }
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

/// Nome della ruota principale quando la crea Wheelor.
fn main_name() -> &'static str {
    tr!("Principale", "Main")
}

fn default_hotkey() -> String {
    "Ctrl+Alt+W".into()
}

fn full_opacity() -> u8 {
    100
}

fn is_full_opacity(value: &u8) -> bool {
    *value == 100
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Il valore è quello predefinito: inutile scriverlo nel file.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
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
pub fn app_matches(pattern: &str, path: &str) -> bool {
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
    /// La configurazione di esempio del primo avvio, con i nomi nella lingua attuale.
    fn default() -> Self {
        // Gli id sono in inglese, come tutto il file; i nomi nella lingua dell'interfaccia.
        let item = |id: &str, label: &str, icon: &str, steps: Vec<Step>| Item { id: id.into(), ..Item::new(label, icon, steps) };
        let open = |target: &str| Step::Open { target: target.into(), args: String::new() };
        let keys = |keys: &str| Step::Keys { keys: keys.into() };
        let system = |op| Step::System { op };
        let actions = vec![
            item("terminal", tr!("Terminale", "Terminal"), "E756", vec![open("wt.exe")]),
            item("browser", "Browser", "E774", vec![open("https://www.google.com")]),
            item("file-explorer", tr!("Esplora file", "File Explorer"), "E8B7", vec![open("explorer.exe")]),
            item("screenshot", "Screenshot", "E722", vec![keys("Win+Shift+S")]),
            item("copy", tr!("Copia", "Copy"), "E8C8", vec![keys("Ctrl+C")]),
            item("paste", tr!("Incolla", "Paste"), "E77F", vec![keys("Ctrl+V")]),
            item("lock-pc", tr!("Blocca PC", "Lock PC"), "E72E", vec![system(SystemOp::Lock)]),
            item("play-pause", tr!("Play/Pausa", "Play/Pause"), "E768", vec![system(SystemOp::PlayPause)]),
            item("next-track", tr!("Successivo", "Next"), "E893", vec![system(SystemOp::NextTrack)]),
            item("previous-track", tr!("Precedente", "Previous"), "E892", vec![system(SystemOp::PreviousTrack)]),
            item("volume-up", "Volume +", "E995", vec![system(SystemOp::VolumeUp)]),
            item("volume-down", "Volume −", "E993", vec![system(SystemOp::VolumeDown)]),
            item("mute", tr!("Muto", "Mute"), "E74F", vec![system(SystemOp::Mute)]),
            // Due esempi di ciò che si può fare con i segnaposto e i passi.
            item(
                "search-selection",
                tr!("Cerca la selezione", "Search the selection"),
                "E721",
                vec![open("https://www.google.com/search?q={selection}")],
            ),
            item(
                "paste-plain",
                tr!("Incolla come testo semplice", "Paste as plain text"),
                "E8E9",
                vec![
                    Step::Clipboard { op: ClipboardOp::Set, text: "{clipboard}".into() },
                    Step::Clipboard { op: ClipboardOp::Paste, text: String::new() },
                ],
            ),
        ];

        let action = |id: &str| Slot::Action(id.into());
        let mut main = Wheel::new(MAIN_WHEEL, main_name());
        main.slots = ["terminal", "browser", "file-explorer", "screenshot", "copy", "paste"].map(action).to_vec();
        main.slots.push(Slot::Wheel("media".into()));
        main.slots.push(action("lock-pc"));

        let mut media = Wheel::new("media", "Media");
        media.icon = "E8D6".into();
        media.slots =
            ["play-pause", "next-track", "volume-up", "mute", "volume-down", "previous-track"].map(action).to_vec();

        Self { actions, wheels: vec![main, media], ..Self::empty() }
    }
}

impl Config {
    /// Preferenze predefinite, senza azioni né ruote.
    fn empty() -> Self {
        Self {
            hotkey: default_hotkey(),
            language: None,
            size: WheelSize::default(),
            opacity: full_opacity(),
            accent: Accent::default(),
            aim_anywhere: false,
            check_updates: true,
            actions: Vec::new(),
            wheels: Vec::new(),
        }
    }

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

    /// Garantisce che la ruota principale esista e sia la prima, e che l'opacità sia sensata.
    fn normalize(&mut self) {
        self.opacity = self.opacity.clamp(MIN_OPACITY, 100);
        match self.wheels.iter().position(Wheel::is_main) {
            Some(0) => {}
            Some(index) => {
                let main = self.wheels.remove(index);
                self.wheels.insert(0, main);
            }
            None => self.wheels.insert(0, Wheel::new(MAIN_WHEEL, main_name())),
        }
    }

    /// Controlla che la configurazione sia coerente. Il messaggio d'errore dice cosa correggere.
    pub fn check(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        for item in &self.actions {
            if item.id.trim().is_empty() {
                return Err(trf!("L'azione \"{}\" non ha un id", "The action \"{}\" has no id", item.label));
            }
            // `insert` restituisce false se l'elemento c'era già.
            if !ids.insert(item.id.as_str()) {
                return Err(trf!("Ci sono due azioni con id \"{}\"", "Two actions have the id \"{}\"", item.id));
            }
        }

        let mut wheel_ids = HashSet::new();
        for wheel in &self.wheels {
            if wheel.id.trim().is_empty() {
                return Err(trf!("La ruota \"{}\" non ha un id", "The wheel \"{}\" has no id", wheel.name));
            }
            if !wheel_ids.insert(wheel.id.as_str()) {
                return Err(trf!("Ci sono due ruote con id \"{}\"", "Two wheels have the id \"{}\"", wheel.id));
            }
        }

        for wheel in &self.wheels {
            if wheel.slots.len() > MAX_SLOTS {
                return Err(trf!(
                    "La ruota \"{}\" ha più di {MAX_SLOTS} elementi",
                    "The wheel \"{}\" has more than {MAX_SLOTS} items",
                    wheel.name
                ));
            }
            for slot in &wheel.slots {
                match slot {
                    Slot::Action(id) if !ids.contains(id.as_str()) => {
                        return Err(trf!(
                            "La ruota \"{}\" usa l'azione \"{id}\", che non esiste",
                            "The wheel \"{}\" uses the action \"{id}\", which doesn't exist",
                            wheel.name
                        ));
                    }
                    Slot::Wheel(id) if !wheel_ids.contains(id.as_str()) => {
                        return Err(trf!(
                            "La ruota \"{}\" usa la ruota \"{id}\", che non esiste",
                            "The wheel \"{}\" uses the wheel \"{id}\", which doesn't exist",
                            wheel.name
                        ));
                    }
                    _ => {}
                }
            }
        }

        if let Some(wheel) = self.wheels.iter().find(|w| self.reaches(&w.id, &w.id)) {
            return Err(trf!(
                "La ruota \"{}\" contiene sé stessa tramite i suoi sottomenu",
                "The wheel \"{}\" contains itself through its submenus",
                wheel.name
            ));
        }

        for item in &self.actions {
            if item.steps.is_empty() {
                return Err(trf!("L'azione \"{}\" non ha passi", "The action \"{}\" has no steps", item.label));
            }
            if let Some(id) = item.runs().find(|id| !ids.contains(id)) {
                return Err(trf!(
                    "L'azione \"{}\" esegue l'azione \"{id}\", che non esiste",
                    "The action \"{}\" runs the action \"{id}\", which doesn't exist",
                    item.label
                ));
            }
        }
        if let Some(item) = self.actions.iter().find(|a| self.runs_into(&a.id, &a.id)) {
            return Err(trf!(
                "L'azione \"{}\" esegue sé stessa tramite i suoi passi",
                "The action \"{}\" runs itself through its steps",
                item.label
            ));
        }
        Ok(())
    }

    /// L'azione `from`, con i suoi passi "Esegui azione" (anche indiretti), esegue l'azione `to`?
    /// Come `reaches` per le ruote: impedisce i cicli.
    pub fn runs_into(&self, from: &str, to: &str) -> bool {
        let runs = |id: &str| self.action(id).into_iter().flat_map(Item::runs);
        let mut seen = HashSet::new();
        let mut pending: Vec<&str> = runs(from).collect();
        while let Some(id) = pending.pop() {
            if id == to {
                return true;
            }
            if seen.insert(id) {
                pending.extend(runs(id));
            }
        }
        false
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
    let base = if base.is_empty() { "action" } else { base };
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
    let text = fs::read_to_string(&path)
        .map_err(|e| trf!("Impossibile leggere {}: {e}", "Can't read {}: {e}", path.display()))?;
    let error = |e: toml::de::Error| trf!("Errore in {}:\n{e}", "Error in {}:\n{e}", path.display());

    let (config, migrated) = parse(&text).map_err(error)?;
    config.check().map_err(|e| trf!("Errore in {}:\n{e}", "Error in {}:\n{e}", path.display()))?;
    if migrated {
        let backup = path.with_file_name("config.v1.toml");
        fs::copy(&path, &backup)
            .map_err(|e| trf!("Impossibile copiare {}: {e}", "Can't copy {}: {e}", backup.display()))?;
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
            config = migrate(config, legacy.slices);
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
    action: Step,
}

/// Ogni vecchio spicchio diventa un'azione della libreria; i primi 8 formano la ruota principale.
/// Le preferenze (scorciatoia, …) restano quelle lette da `config`.
fn migrate(config: Config, slices: Vec<LegacySlice>) -> Config {
    let mut actions: Vec<Item> = Vec::new();
    for slice in slices {
        let id = unique_id(&slice.label, |id| actions.iter().any(|a| a.id == id));
        actions.push(Item { id, ..Item::new(&slice.label, &slice.icon, vec![slice.action]) });
    }
    let mut main = Wheel::new(MAIN_WHEEL, main_name());
    main.slots = actions.iter().take(MAX_SLOTS).map(|a| Slot::Action(a.id.clone())).collect();
    Config { actions, wheels: vec![main], ..config }
}

/// Salva in modo atomico: scrive un file temporaneo e poi lo sostituisce all'originale,
/// così chi legge non trova mai un file scritto a metà.
pub fn save(config: &Config) -> Result<(), String> {
    let path = path();
    let text = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| trf!("Impossibile creare {}: {e}", "Can't create {}: {e}", dir.display()))?;
    }
    let temp = path.with_extension("toml.tmp");
    fs::write(&temp, text).map_err(|e| trf!("Impossibile scrivere {}: {e}", "Can't write {}: {e}", temp.display()))?;
    fs::rename(&temp, &path).map_err(|e| trf!("Impossibile salvare {}: {e}", "Can't save {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_size_survives_a_round_trip() {
        // Predefinita: non compare nel file. Diversa: compare e si rilegge uguale.
        let config = Config::default();
        assert!(!toml::to_string_pretty(&config).unwrap().contains("size"));
        let config = Config { size: WheelSize::Large, ..Config::default() };
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("size = \"large\""));
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }

    #[test]
    fn preferences_survive_a_round_trip() {
        // Predefinite: non compaiono nel file.
        let text = toml::to_string_pretty(&Config::default()).unwrap();
        for key in ["opacity", "accent", "aim_anywhere", "check_updates"] {
            assert!(!text.contains(key), "{key} non dovrebbe essere nel file");
        }
        let config =
            Config { opacity: 70, accent: Accent::Lime, aim_anywhere: true, check_updates: false, ..Config::default() };
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("opacity = 70\naccent = \"lime\"\naim_anywhere = true\ncheck_updates = false"));
        assert_eq!(parse(&text).unwrap().0, config);
    }

    #[test]
    fn opacity_is_kept_in_range() {
        let (config, _) = parse("opacity = 5").unwrap();
        assert_eq!(config.opacity, MIN_OPACITY);
        let (config, _) = parse("opacity = 250").unwrap();
        assert_eq!(config.opacity, 100);
    }

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
        assert!(text.contains("[[action]]\nid = \"terminal\"\nlabel = \"Terminale\"\nicon = \"E756\"\ntype = \"open\""));
        assert!(text.contains("\"wheel:media\""));
    }

    #[test]
    fn several_steps_are_written_as_tables() {
        let mut item = Item::new("Riunione", "E8F2", vec![
            Step::Open { target: "ms-teams:".into(), args: String::new() },
            Step::Wait { seconds: 1.5 },
            Step::WaitWindow { app: "ms-teams.exe".into(), title: String::new(), timeout: DEFAULT_TIMEOUT },
            Step::Window { op: WindowOp::LeftHalf },
            Step::Keys { keys: "Ctrl+Shift+M".into() },
        ]);
        item.id = "meeting".into();
        item.hotkey = "Ctrl+Alt+M".into();
        let config = Config { actions: vec![item], ..Config::empty() };
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("[[action]]\nid = \"meeting\"\nlabel = \"Riunione\"\nicon = \"E8F2\"\nhotkey = \"Ctrl+Alt+M\"\n"));
        assert!(text.contains("[[action.step]]\ntype = \"wait\"\nseconds = 1.5\n"));
        assert!(text.contains("[[action.step]]\ntype = \"wait_window\"\napp = \"ms-teams.exe\"\n\n"));
        assert!(text.contains("[[action.step]]\ntype = \"window\"\ndo = \"left_half\"\n"));
        assert_eq!(parse(&text).unwrap().0.actions, config.actions);
    }

    #[test]
    fn missing_fields_get_defaults() {
        let text = r#"
            [[action]]
            id = "ciao"
            label = "Ciao"
            type = "command"
            command = "echo ciao"

            [[action]]
            id = "pausa"
            label = "Pausa"
            [[action.step]]
            type = "wait"
            seconds = 2
            [[action.step]]
            type = "wait_window"
            title = "Blocco note"
        "#;
        let (config, _) = parse(text).unwrap();
        assert_eq!(config.hotkey, "Ctrl+Alt+W");
        assert_eq!(config.actions[0].steps, vec![Step::Command {
            command: "echo ciao".into(),
            shell: Shell::Powershell,
            window: WindowMode::Hidden,
            cwd: String::new(),
            admin: false,
            wait: false,
        }]);
        assert_eq!(config.actions[0].icon, "");
        assert_eq!(config.actions[1].steps, vec![
            Step::Wait { seconds: 2.0 },
            Step::WaitWindow { app: String::new(), title: "Blocco note".into(), timeout: DEFAULT_TIMEOUT },
        ]);
        // La ruota principale viene creata anche se il file non la contiene.
        assert!(config.wheels[0].is_main());
    }

    #[test]
    fn step_errors_are_precise() {
        let error = parse("[[action]]\nid = \"x\"\nlabel = \"X\"\ntype = \"open\"\n").unwrap_err().to_string();
        assert!(error.contains("action `x`") && error.contains("target"), "{error}");
        let error = parse("[[action]]\nid = \"x\"\nlabel = \"X\"\ntype = \"fly\"\n").unwrap_err().to_string();
        assert!(error.contains("fly"), "{error}");
        let error = parse("[[action]]\nlabel = \"X\"\ntype = \"keys\"\nkeys = \"A\"\n").unwrap_err().to_string();
        assert!(error.contains("id"), "{error}");
        let text = "[[action]]\nid = \"x\"\nlabel = \"X\"\ntype = \"keys\"\nkeys = \"A\"\n[[action.step]]\ntype = \"keys\"\nkeys = \"B\"\n";
        assert!(parse(text).unwrap_err().to_string().contains("not both"));
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
        assert_eq!(config.actions[0].steps, vec![Step::Keys { keys: "F1".into() }]);
        config.check().unwrap();
    }

    #[test]
    fn check_finds_problems() {
        let mut config = Config::default();
        config.wheels[0].slots.push(Slot::Action("copy".into()));
        assert!(config.check().unwrap_err().contains("più di 8"));

        let mut config = Config::default();
        config.wheels[1].slots[0] = Slot::Action("non-esiste".into());
        assert!(config.check().unwrap_err().contains("non esiste"));

        // Media contiene la principale, che contiene Media: ciclo.
        let mut config = Config::default();
        config.wheels[1].slots[0] = Slot::Wheel(MAIN_WHEEL.into());
        assert!(config.check().unwrap_err().contains("sé stessa"));
        assert!(config.reaches("media", "media"));

        let mut config = Config::default();
        config.actions[0].steps.clear();
        assert!(config.check().unwrap_err().contains("non ha passi"));

        let mut config = Config::default();
        config.actions[0].steps.push(Step::Run { action: "non-esiste".into() });
        assert!(config.check().unwrap_err().contains("non esiste"));

        // Copia esegue Incolla, che esegue Copia: ciclo.
        let mut config = Config::default();
        config.actions[4].steps.push(Step::Run { action: "paste".into() });
        config.check().unwrap();
        config.actions[5].steps.push(Step::Run { action: "copy".into() });
        assert!(config.check().unwrap_err().contains("sé stessa"));
        assert!(config.runs_into("paste", "paste"));
        assert!(!config.runs_into("terminal", "terminal"));
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
        assert_eq!(unique_id("???", |_| false), "action");
        assert_eq!(unique_id("Copia", |id| id == "copia" || id == "copia-2"), "copia-3");
    }
}
