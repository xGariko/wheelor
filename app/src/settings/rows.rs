//! Conversioni tra la configurazione e quello che mostra la finestra: le righe degli elenchi
//! e i campi dell'editor delle azioni.

use slint::{ModelRc, SharedString, VecModel};

use super::{ActionRow, PickItem, SettingsWindow, SlotRow, WheelRow};
use crate::config::{Action, Config, Item, Shell, Slot, Wheel, WindowMode};
use crate::{icons, keys, tr, trf};

/// I tasti di una combinazione ("Ctrl+Alt+W"), come vanno disegnati.
pub fn caps_model(text: &str) -> ModelRc<SharedString> {
    let caps: Vec<SharedString> = keys::caps(text).into_iter().map(SharedString::from).collect();
    ModelRc::new(VecModel::from(caps))
}

/// Icona di una ruota; se non ne ha una, un'icona generica.
pub fn wheel_glyph(wheel: &Wheel) -> SharedString {
    icons::glyph(wheel.icon_or_default()).into()
}

pub fn action_row(item: &Item) -> ActionRow {
    ActionRow {
        id: item.id.as_str().into(),
        label: item.label.as_str().into(),
        glyph: icons::glyph(&item.icon).into(),
        detail: summary(&item.action).into(),
    }
}

/// Le azioni come voci della finestra di scelta.
pub fn action_item(item: &Item) -> PickItem {
    let row = action_row(item);
    PickItem { id: row.id, glyph: row.glyph, label: row.label, detail: row.detail }
}

pub fn wheel_row(config: &Config, wheel: &Wheel) -> WheelRow {
    WheelRow {
        id: wheel.id.as_str().into(),
        name: wheel.name.as_str().into(),
        glyph: wheel_glyph(wheel),
        detail: wheel_role(config, wheel).into(),
    }
}

pub fn wheel_item(config: &Config, wheel: &Wheel) -> PickItem {
    let row = wheel_row(config, wheel);
    PickItem { id: row.id, glyph: row.glyph, label: row.name, detail: row.detail }
}

/// A cosa serve la ruota, in breve.
fn wheel_role(config: &Config, wheel: &Wheel) -> String {
    if wheel.is_main() {
        return tr!("Ruota principale", "Main wheel").into();
    }
    if !wheel.apps.is_empty() {
        return trf!("Profilo · {}", "Profile · {}", wheel.apps.join(", "));
    }
    let parents = parents(config, wheel).join(", ");
    if parents.is_empty() { tr!("Non usata", "Not used").into() } else { trf!("Sottomenu di {parents}", "Submenu of {parents}") }
}

/// Nomi delle ruote che contengono `wheel` come sottomenu.
pub fn parents<'a>(config: &'a Config, wheel: &Wheel) -> Vec<&'a str> {
    containing(config, &Slot::Wheel(wheel.id.clone()))
}

/// Nomi delle ruote che contengono `slot`.
pub fn containing<'a>(config: &'a Config, slot: &Slot) -> Vec<&'a str> {
    config.wheels.iter().filter(|w| w.slots.contains(slot)).map(|w| w.name.as_str()).collect()
}

/// Suggerimento sotto i campi della ruota: spiega quando si apre.
pub fn wheel_hint(config: &Config, wheel: &Wheel) -> String {
    if wheel.is_main() {
        return tr!(
            "Si apre con la scorciatoia, quando l'app in primo piano non ha un suo profilo.",
            "Opens with the shortcut, when the app in the foreground has no profile of its own."
        )
        .into();
    }
    let parents = parents(config, wheel).join(", ");
    match (wheel.apps.is_empty(), parents.is_empty()) {
        (true, true) => tr!(
            "Questa ruota non si apre ancora: aggiungila come sottomenu di un'altra ruota oppure scrivi qui sotto le app per cui deve aprirsi.",
            "This wheel doesn't open yet: add it as a submenu of another wheel, or write below the apps it should open for."
        )
        .into(),
        (true, false) => trf!("Si apre come sottomenu di: {parents}.", "Opens as a submenu of: {parents}."),
        (false, _) => tr!(
            "Con una di queste app in primo piano, la scorciatoia apre questa ruota.",
            "With one of these apps in the foreground, the shortcut opens this wheel."
        )
        .into(),
    }
}

pub fn slot_rows(config: &Config, wheel: &Wheel) -> Vec<SlotRow> {
    wheel
        .slots
        .iter()
        .filter_map(|slot| match slot {
            Slot::Action(id) => config.action(id).map(|item| SlotRow {
                label: item.label.as_str().into(),
                glyph: icons::glyph(&item.icon).into(),
                initial: initial(&item.label),
                submenu: false,
            }),
            Slot::Wheel(id) => config.wheel(id).map(|sub| SlotRow {
                label: sub.name.as_str().into(),
                glyph: wheel_glyph(sub),
                initial: initial(&sub.name),
                submenu: true,
            }),
        })
        .collect()
}

/// Prima lettera, maiuscola: sulla ruota prende il posto dell'icona mancante.
fn initial(label: &str) -> SharedString {
    label.chars().take(1).flat_map(char::to_uppercase).collect::<String>().into()
}

/// Dove è usata un'azione, da mostrare sotto l'etichetta.
pub fn usage(config: &Config, id: &str) -> String {
    let names = containing(config, &Slot::Action(id.to_string())).join(", ");
    if names.is_empty() {
        tr!("Non è in nessuna ruota: aggiungila dalla pagina di una ruota.", "Not in any wheel: add it from a wheel's page.").into()
    } else {
        trf!("Nelle ruote: {names}", "In the wheels: {names}")
    }
}

/// Riassunto di una riga di cosa fa l'azione.
fn summary(action: &Action) -> String {
    let first_line = |text: &str| text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    match action {
        Action::Command { command, .. } => trf!("Comando · {}", "Command · {}", first_line(command)),
        Action::Open { target, .. } => trf!("Apri · {target}", "Open · {target}"),
        Action::Keys { keys } => trf!("Tasti · {keys}", "Keys · {keys}"),
        Action::Text { text } => trf!("Testo · {}", "Text · {}", first_line(text)),
    }
}

/// Copia l'azione nei campi dell'editor. Ogni tipo ha i suoi campi: quelli degli altri tipi
/// vengono svuotati, così cambiando tipo si parte da campi puliti.
pub fn fill_editor(ui: &SettingsWindow, item: &Item) {
    ui.set_ed_label(item.label.as_str().into());
    ui.set_ed_icon(item.icon.as_str().into());
    let empty = SharedString::new;
    let (mut command, mut shell, mut window, mut cwd, mut admin) = (empty(), 0, 0, empty(), false);
    let (mut target, mut args, mut keys, mut text) = (empty(), empty(), empty(), empty());
    let kind = match &item.action {
        Action::Command { command: c, shell: s, window: w, cwd: d, admin: a } => {
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
            0
        }
        Action::Open { target: t, args: a } => {
            target = t.into();
            args = a.into();
            1
        }
        Action::Keys { keys: k } => {
            keys = k.into();
            2
        }
        Action::Text { text: t } => {
            text = t.into();
            3
        }
    };
    ui.set_ed_kind(kind);
    ui.set_ed_command(command);
    ui.set_ed_shell(shell);
    ui.set_ed_window(window);
    ui.set_ed_cwd(cwd);
    ui.set_ed_admin(admin);
    ui.set_ed_target(target);
    ui.set_ed_args(args);
    ui.set_ed_keys_caps(caps_model(&keys));
    ui.set_ed_keys(keys);
    ui.set_ed_text(text);
}

/// Legge l'azione dai campi dell'editor; conta solo il tipo scelto.
pub fn read_editor(ui: &SettingsWindow, id: &str) -> Item {
    let s = |text: SharedString| text.to_string();
    let action = match ui.get_ed_kind() {
        0 => Action::Command {
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
        },
        1 => Action::Open { target: s(ui.get_ed_target()), args: s(ui.get_ed_args()) },
        2 => Action::Keys { keys: s(ui.get_ed_keys()) },
        _ => Action::Text { text: s(ui.get_ed_text()) },
    };
    Item { id: id.to_string(), label: s(ui.get_ed_label()), icon: s(ui.get_ed_icon()), action }
}
