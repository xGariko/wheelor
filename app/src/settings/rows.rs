//! Conversioni tra la configurazione e quello che mostra la finestra: le righe degli elenchi
//! e i campi dell'editor delle azioni.

use slint::SharedString;

use super::{ActionRow, PickItem, SettingsWindow, SlotRow, WheelRow};
use crate::config::{Action, Config, Item, Shell, Slot, Wheel, WindowMode};
use crate::icons;

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
        return "Ruota principale".into();
    }
    if !wheel.apps.is_empty() {
        return format!("Profilo · {}", wheel.apps.join(", "));
    }
    let parents = parents(config, wheel);
    if parents.is_empty() { "Non usata".into() } else { format!("Sottomenu di {}", parents.join(", ")) }
}

/// Nomi delle ruote che contengono `wheel` come sottomenu.
fn parents<'a>(config: &'a Config, wheel: &Wheel) -> Vec<&'a str> {
    let slot = Slot::Wheel(wheel.id.clone());
    config.wheels.iter().filter(|w| w.slots.contains(&slot)).map(|w| w.name.as_str()).collect()
}

/// Suggerimento sotto i campi della ruota: spiega quando si apre.
pub fn wheel_hint(config: &Config, wheel: &Wheel) -> String {
    if wheel.is_main() {
        return "Si apre con la scorciatoia, quando l'app in primo piano non ha un suo profilo.".into();
    }
    let parents = parents(config, wheel);
    match (wheel.apps.is_empty(), parents.is_empty()) {
        (true, true) => "Questa ruota non si apre ancora: aggiungila come sottomenu di un'altra ruota \
                         oppure scrivi qui sopra le app per cui deve aprirsi."
            .into(),
        (true, false) => format!("Si apre come sottomenu di: {}.", parents.join(", ")),
        (false, _) => "Con una di queste app in primo piano, la scorciatoia apre questa ruota.".into(),
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
                submenu: false,
            }),
            Slot::Wheel(id) => config.wheel(id).map(|sub| SlotRow {
                label: sub.name.as_str().into(),
                glyph: wheel_glyph(sub),
                submenu: true,
            }),
        })
        .collect()
}

/// Dove è usata un'azione, da mostrare sotto l'etichetta.
pub fn usage(config: &Config, id: &str) -> String {
    let slot = Slot::Action(id.to_string());
    let names: Vec<&str> = config.wheels.iter().filter(|w| w.slots.contains(&slot)).map(|w| w.name.as_str()).collect();
    if names.is_empty() {
        "Non è in nessuna ruota: aggiungila dalla pagina di una ruota.".into()
    } else {
        format!("Nelle ruote: {}", names.join(", "))
    }
}

/// Riassunto di una riga di cosa fa l'azione.
fn summary(action: &Action) -> String {
    let first_line = |text: &str| text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    match action {
        Action::Command { command, .. } => format!("Comando · {}", first_line(command)),
        Action::Open { target, .. } => format!("Apri · {target}"),
        Action::Keys { keys } => format!("Tasti · {keys}"),
        Action::Text { text } => format!("Testo · {}", first_line(text)),
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
