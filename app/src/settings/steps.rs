//! I passi delle azioni nella finestra: i tipi tra cui scegliere, i campi dell'editor del passo
//! aperto, i riassunti nell'elenco e gli esempi pronti della finestra "Nuova azione".

use slint::{ModelRc, SharedString, VecModel};

use super::{Choice, SettingsWindow, StepRow};
use crate::config::{ClipboardOp, Config, DEFAULT_TIMEOUT, Item, Shell, Step, SystemOp, WindowMode, WindowOp};
use crate::{icons, keys, tr, trf};

/// Icona e spiegazione di ogni tipo di passo, nell'ordine di `ed-kind` (vedi `kind`).
const KINDS: [&str; 10] = ["E8A7", "E765", "E8D2", "E756", "E916", "E78B", "E7C4", "E7E8", "E77F", "E8EE"];

/// I tipi di passo proposti per una nuova azione: Apri, Tasti, Testo, Comando, Sistema, Finestra.
const STARTERS: [i32; 6] = [0, 1, 2, 3, 7, 6];

/// Il posto del tipo di passo nell'elenco dei tipi (`ed-kind`).
pub fn kind(step: &Step) -> i32 {
    match step {
        Step::Open { .. } => 0,
        Step::Keys { .. } => 1,
        Step::Text { .. } => 2,
        Step::Command { .. } => 3,
        Step::Wait { .. } => 4,
        Step::WaitWindow { .. } => 5,
        Step::Window { .. } => 6,
        Step::System { .. } => 7,
        Step::Clipboard { .. } => 8,
        Step::Run { .. } => 9,
    }
}

/// Un passo nuovo del tipo `kind`, con i valori di partenza.
pub fn new_step(kind: i32) -> Step {
    match kind {
        1 => Step::Keys { keys: String::new() },
        2 => Step::Text { text: String::new() },
        3 => Step::Command {
            command: String::new(),
            shell: Shell::default(),
            window: WindowMode::default(),
            cwd: String::new(),
            admin: false,
            wait: false,
        },
        4 => Step::Wait { seconds: 1.0 },
        5 => Step::WaitWindow { app: String::new(), title: String::new(), timeout: DEFAULT_TIMEOUT },
        6 => Step::Window { op: WindowOp::default() },
        7 => Step::System { op: SystemOp::default() },
        8 => Step::Clipboard { op: ClipboardOp::default(), text: String::new() },
        9 => Step::Run { action: String::new() },
        _ => Step::Open { target: String::new(), args: String::new() },
    }
}

fn choice(glyph: &str, label: &str, hint: &str) -> Choice {
    Choice { glyph: icons::glyph(glyph).into(), label: label.into(), hint: hint.into() }
}

fn model(choices: Vec<Choice>) -> ModelRc<Choice> {
    ModelRc::new(VecModel::from(choices))
}

/// I tipi di passo con la loro spiegazione.
fn kinds() -> Vec<Choice> {
    let hints = [
        tr!("Un'app, un file, una cartella o un sito", "An app, a file, a folder or a website"),
        tr!("Una combinazione, es. Ctrl+Shift+S", "A combination, e.g. Ctrl+Shift+S"),
        tr!("Scrive un testo dove c'è il cursore", "Types text where the cursor is"),
        tr!("PowerShell o Prompt dei comandi", "PowerShell or Command Prompt"),
        tr!("Una pausa prima del passo dopo", "A pause before the next step"),
        tr!("Finché un'app non è aperta e davanti", "Until an app is open and in front"),
        tr!("Ingrandisci, affianca, chiudi…", "Maximize, snap, close…"),
        tr!("Blocca, sospendi, volume, musica", "Lock, sleep, volume, music"),
        tr!("Copia la selezione, incolla, metti un testo", "Copy the selection, paste, put text"),
        tr!("Tutti i passi di un'altra azione", "All the steps of another action"),
    ];
    (0..10).map(|kind| choice(KINDS[kind as usize], new_step(kind).name(), hints[kind as usize])).collect()
}

fn window_op_label(op: WindowOp) -> &'static str {
    match op {
        WindowOp::Focus => tr!("Torna alla finestra di partenza", "Back to the starting window"),
        WindowOp::Minimize => tr!("Riduci a icona", "Minimize"),
        WindowOp::Maximize => tr!("Ingrandisci", "Maximize"),
        WindowOp::Restore => tr!("Ripristina", "Restore"),
        WindowOp::LeftHalf => tr!("Metà sinistra", "Left half"),
        WindowOp::RightHalf => tr!("Metà destra", "Right half"),
        WindowOp::AlwaysOnTop => tr!("Sempre in primo piano", "Always on top"),
        WindowOp::Close => tr!("Chiudi", "Close"),
    }
}

fn window_op_glyph(op: WindowOp) -> &'static str {
    match op {
        WindowOp::Focus => "E8F9",
        WindowOp::Minimize => "E921",
        WindowOp::Maximize => "E922",
        WindowOp::Restore => "E923",
        WindowOp::LeftHalf => "E90C",
        WindowOp::RightHalf => "E90D",
        WindowOp::AlwaysOnTop => "E718",
        WindowOp::Close => "E8BB",
    }
}

fn system_op_label(op: SystemOp) -> &'static str {
    match op {
        SystemOp::Lock => tr!("Blocca il PC", "Lock the PC"),
        SystemOp::Sleep => tr!("Sospendi", "Sleep"),
        SystemOp::ScreenOff => tr!("Spegni lo schermo", "Turn off the screen"),
        SystemOp::VolumeUp => tr!("Alza il volume", "Volume up"),
        SystemOp::VolumeDown => tr!("Abbassa il volume", "Volume down"),
        SystemOp::Mute => tr!("Audio sì/no", "Mute on/off"),
        SystemOp::PlayPause => tr!("Play/Pausa", "Play/Pause"),
        SystemOp::NextTrack => tr!("Brano successivo", "Next track"),
        SystemOp::PreviousTrack => tr!("Brano precedente", "Previous track"),
    }
}

fn system_op_glyph(op: SystemOp) -> &'static str {
    match op {
        SystemOp::Lock => "E72E",
        SystemOp::Sleep => "E708",
        SystemOp::ScreenOff => "E7F4",
        SystemOp::VolumeUp => "E995",
        SystemOp::VolumeDown => "E993",
        SystemOp::Mute => "E74F",
        SystemOp::PlayPause => "E768",
        SystemOp::NextTrack => "E893",
        SystemOp::PreviousTrack => "E892",
    }
}

fn clipboard_op_label(op: ClipboardOp) -> &'static str {
    match op {
        ClipboardOp::Copy => tr!("Copia la selezione", "Copy the selection"),
        ClipboardOp::Paste => tr!("Incolla", "Paste"),
        ClipboardOp::Set => tr!("Metti un testo", "Put some text"),
    }
}

/// Riempie gli elenchi di scelte, nella lingua attuale.
pub fn set_choices(ui: &SettingsWindow) {
    let kinds = kinds();
    ui.set_starters(model(STARTERS.iter().map(|&k| kinds[k as usize].clone()).collect()));
    ui.set_step_kinds(model(kinds));
    ui.set_window_ops(model(WindowOp::ALL.iter().map(|&op| choice(window_op_glyph(op), window_op_label(op), "")).collect()));
    ui.set_system_ops(model(SystemOp::ALL.iter().map(|&op| choice(system_op_glyph(op), system_op_label(op), "")).collect()));
    let clipboard_glyphs = ["E8C8", "E77F", "F0E3"];
    ui.set_clipboard_ops(model(
        ClipboardOp::ALL.iter().zip(clipboard_glyphs).map(|(&op, glyph)| choice(glyph, clipboard_op_label(op), "")).collect(),
    ));
    ui.set_examples(model(EXAMPLES.iter().map(|example| choice(example.glyph, (example.label)(), (example.hint)())).collect()));
}

/// Il tipo di passo di `starter` (la posizione nella finestra "Nuova azione").
pub fn starter_kind(starter: i32) -> i32 {
    usize::try_from(starter).ok().and_then(|i| STARTERS.get(i)).copied().unwrap_or(0)
}

/// L'icona proposta per un'azione nuova che inizia con un passo di tipo `kind`.
pub fn kind_icon(kind: i32) -> &'static str {
    usize::try_from(kind).ok().and_then(|i| KINDS.get(i)).copied().unwrap_or("E710")
}

/// Il passo come riga dell'elenco: tipo, icona e cosa fa.
pub fn row(config: &Config, step: &Step) -> StepRow {
    StepRow {
        glyph: icons::glyph(KINDS[kind(step) as usize]).into(),
        title: step.name().into(),
        summary: summary(config, step).into(),
    }
}

/// Cosa fa il passo, in breve (senza il tipo).
pub fn summary(config: &Config, step: &Step) -> String {
    let first_line = |text: &str| text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    match step {
        Step::Open { target, args } if args.trim().is_empty() => target.trim().to_string(),
        Step::Open { target, args } => format!("{} {}", target.trim(), args.trim()),
        Step::Keys { keys } => keys::caps(keys).join("+"),
        Step::Text { text } => first_line(text),
        Step::Command { command, .. } => first_line(command),
        Step::Wait { seconds } => seconds_text(*seconds),
        Step::WaitWindow { app, title, timeout } => {
            let what = if app.trim().is_empty() { title.trim() } else { app.trim() };
            trf!("{what} · al massimo {}", "{what} · at most {}", seconds_text(*timeout))
        }
        Step::Window { op } => window_op_label(*op).into(),
        Step::System { op } => system_op_label(*op).into(),
        Step::Clipboard { op: ClipboardOp::Set, text } => format!("{} · {}", clipboard_op_label(ClipboardOp::Set), first_line(text)),
        Step::Clipboard { op, .. } => clipboard_op_label(*op).into(),
        Step::Run { action } => match config.action(action) {
            Some(item) => item.label.clone(),
            None => tr!("Nessuna azione scelta", "No action chosen").into(),
        },
    }
}

/// "1,5 s" in italiano, "1.5 s" in inglese; senza decimali se è un numero intero.
fn seconds_text(seconds: f32) -> String {
    let rounded = (seconds * 10.0).round() / 10.0;
    let text = if rounded.fract() == 0.0 { format!("{rounded:.0} s") } else { format!("{rounded:.1} s") };
    if crate::i18n::english() { text } else { text.replace('.', ",") }
}

/// Copia il passo nei campi dell'editor. Ogni tipo ha i suoi campi: quelli degli altri tipi
/// vengono svuotati, così cambiando tipo si parte da campi puliti.
pub fn fill_editor(ui: &SettingsWindow, config: &Config, step: &Step) {
    let empty = SharedString::new;
    let (mut command, mut shell, mut window, mut cwd, mut admin, mut wait) = (empty(), 0, 0, empty(), false, false);
    let (mut target, mut args, mut keys, mut text) = (empty(), empty(), empty(), empty());
    let (mut seconds, mut app, mut title, mut timeout, mut op, mut run) = (1.0, empty(), empty(), DEFAULT_TIMEOUT, 0, empty());
    match step {
        Step::Command { command: c, shell: s, window: w, cwd: d, admin: a, wait: t } => {
            command = c.into();
            shell = match s {
                Shell::Powershell => 0,
                Shell::Pwsh => 1,
                Shell::Cmd => 2,
            };
            window = match w {
                WindowMode::Hidden => 0,
                WindowMode::Visible => 1,
                WindowMode::KeepOpen => 2,
            };
            cwd = d.into();
            admin = *a;
            wait = *t;
        }
        Step::Open { target: t, args: a } => {
            target = t.into();
            args = a.into();
        }
        Step::Keys { keys: k } => keys = k.into(),
        Step::Text { text: t } => text = t.into(),
        Step::Wait { seconds: s } => seconds = *s,
        Step::WaitWindow { app: a, title: t, timeout: m } => {
            app = a.into();
            title = t.into();
            timeout = *m;
        }
        Step::Window { op: o } => op = position(&WindowOp::ALL, *o),
        Step::System { op: o } => op = position(&SystemOp::ALL, *o),
        Step::Clipboard { op: o, text: t } => {
            op = position(&ClipboardOp::ALL, *o);
            text = t.into();
        }
        Step::Run { action } => run = action.into(),
    }
    ui.set_ed_kind(kind(step));
    ui.set_ed_command(command);
    ui.set_ed_shell(shell);
    ui.set_ed_window(window);
    ui.set_ed_cwd(cwd);
    ui.set_ed_admin(admin);
    ui.set_ed_wait(wait);
    ui.set_ed_target(target);
    ui.set_ed_args(args);
    ui.set_ed_keys_caps(super::rows::caps_model(&keys));
    ui.set_ed_keys(keys);
    ui.set_ed_text(text);
    ui.set_ed_seconds(seconds);
    ui.set_ed_app(app);
    ui.set_ed_title(title);
    ui.set_ed_timeout(timeout);
    ui.set_ed_op(op);
    set_run(ui, config, &run);
}

/// Il posto di `value` in `all` (le operazioni, nell'ordine in cui le mostra l'editor).
fn position<T: PartialEq>(all: &[T], value: T) -> i32 {
    all.iter().position(|v| *v == value).unwrap_or(0) as i32
}

/// L'azione eseguita dal passo "Esegui azione" aperto nell'editor.
pub fn set_run(ui: &SettingsWindow, config: &Config, id: &str) {
    let item = config.action(id);
    ui.set_ed_run(id.into());
    ui.set_ed_run_label(item.map_or(SharedString::new(), |item| item.label.as_str().into()));
    ui.set_ed_run_glyph(item.map_or(SharedString::new(), |item| icons::glyph(&item.icon).into()));
}

/// Legge il passo dai campi dell'editor; conta solo il tipo scelto.
pub fn read_editor(ui: &SettingsWindow) -> Step {
    let s = |text: SharedString| text.to_string();
    let op = ui.get_ed_op().max(0) as usize;
    match ui.get_ed_kind() {
        1 => Step::Keys { keys: s(ui.get_ed_keys()) },
        2 => Step::Text { text: s(ui.get_ed_text()) },
        3 => Step::Command {
            command: s(ui.get_ed_command()),
            shell: match ui.get_ed_shell() {
                1 => Shell::Pwsh,
                2 => Shell::Cmd,
                _ => Shell::Powershell,
            },
            window: match ui.get_ed_window() {
                1 => WindowMode::Visible,
                2 => WindowMode::KeepOpen,
                _ => WindowMode::Hidden,
            },
            cwd: s(ui.get_ed_cwd()),
            admin: ui.get_ed_admin(),
            wait: ui.get_ed_wait() && ui.get_ed_window() != 0,
        },
        // Un decimale basta: niente 0.30000001 nel file.
        4 => Step::Wait { seconds: (ui.get_ed_seconds() * 10.0).round() / 10.0 },
        5 => Step::WaitWindow { app: s(ui.get_ed_app()), title: s(ui.get_ed_title()), timeout: ui.get_ed_timeout().round() },
        6 => Step::Window { op: WindowOp::ALL.get(op).copied().unwrap_or_default() },
        7 => Step::System { op: SystemOp::ALL.get(op).copied().unwrap_or_default() },
        8 => {
            let op = ClipboardOp::ALL.get(op).copied().unwrap_or_default();
            let text = if op == ClipboardOp::Set { s(ui.get_ed_text()) } else { String::new() };
            Step::Clipboard { op, text }
        }
        9 => Step::Run { action: s(ui.get_ed_run()) },
        _ => Step::Open { target: s(ui.get_ed_target()), args: s(ui.get_ed_args()) },
    }
}

/// Un esempio della finestra "Nuova azione".
struct Example {
    /// L'id dell'azione creata (in inglese, come tutto il file), se non è già usato.
    id: &'static str,
    glyph: &'static str,
    label: fn() -> &'static str,
    hint: fn() -> &'static str,
    steps: fn() -> Vec<Step>,
}

fn open(target: &str) -> Step {
    Step::Open { target: target.into(), args: String::new() }
}

fn wait_for(app: &str) -> Step {
    Step::WaitWindow { app: app.into(), title: String::new(), timeout: DEFAULT_TIMEOUT }
}

const EXAMPLES: [Example; 8] = [
    Example {
        id: "search-selection",
        glyph: "E721",
        label: || tr!("Cerca la selezione sul web", "Search the selection on the web"),
        hint: || tr!("Apri, con {selection} nell'indirizzo", "Open, with {selection} in the address"),
        steps: || vec![open("https://www.google.com/search?q={selection}")],
    },
    Example {
        id: "translate-selection",
        glyph: "E909",
        label: || tr!("Traduci la selezione", "Translate the selection"),
        hint: || tr!("Google Traduttore, verso l'italiano", "Google Translate, into English"),
        steps: || {
            let language = if crate::i18n::english() { "en" } else { "it" };
            vec![open(&format!("https://translate.google.com/?sl=auto&tl={language}&text={{selection}}&op=translate"))]
        },
    },
    Example {
        id: "youtube-search",
        glyph: "E714",
        label: || tr!("Cerca su YouTube", "Search on YouTube"),
        hint: || tr!("Chiede cosa cercare con {ask:…}", "Asks what to search with {ask:…}"),
        steps: || {
            let question = tr!("Cosa cerchi su YouTube?", "What are you looking for on YouTube?");
            vec![open(&format!("https://www.youtube.com/results?search_query={{ask:{question}}}"))]
        },
    },
    Example {
        id: "paste-plain",
        glyph: "E77F",
        label: || tr!("Incolla come testo semplice", "Paste as plain text"),
        hint: || tr!("Appunti: senza formattazione, poi incolla", "Clipboard: no formatting, then paste"),
        steps: || {
            vec![Step::Clipboard { op: ClipboardOp::Set, text: "{clipboard}".into() }, Step::Clipboard {
                op: ClipboardOp::Paste,
                text: String::new(),
            }]
        },
    },
    Example {
        id: "email-selection",
        glyph: "E715",
        label: || tr!("Email con la selezione", "Email the selection"),
        hint: || tr!("Una nuova email con il testo selezionato", "A new email with the selected text"),
        steps: || vec![open("mailto:?body={selection}")],
    },
    Example {
        id: "today",
        glyph: "E787",
        label: || tr!("Scrivi la data di oggi", "Type today's date"),
        hint: || tr!("Testo con {date}", "Text with {date}"),
        steps: || vec![Step::Text { text: "{date}".into() }],
    },
    Example {
        id: "side-by-side",
        glyph: "E90C",
        label: || tr!("Due app affiancate", "Two apps side by side"),
        hint: || tr!("Apri, aspetta e sistema due finestre", "Open, wait and place two windows"),
        steps: || {
            vec![
                open("explorer.exe"),
                wait_for("explorer.exe"),
                Step::Window { op: WindowOp::LeftHalf },
                open("notepad.exe"),
                wait_for("notepad.exe"),
                Step::Window { op: WindowOp::RightHalf },
            ]
        },
    },
    Example {
        id: "break",
        glyph: "E708",
        label: || tr!("Pausa", "Break"),
        hint: || tr!("Ferma la musica e blocca il PC", "Pause the music and lock the PC"),
        steps: || vec![Step::System { op: SystemOp::PlayPause }, Step::Wait { seconds: 0.5 }, Step::System { op: SystemOp::Lock }],
    },
];

/// L'azione dell'esempio `index`, pronta da aggiungere alla libreria, con l'id proposto.
pub fn example(index: i32) -> Option<(Item, &'static str)> {
    let example = EXAMPLES.get(usize::try_from(index).ok()?)?;
    Some((Item::new((example.label)(), example.glyph, (example.steps)()), example.id))
}
