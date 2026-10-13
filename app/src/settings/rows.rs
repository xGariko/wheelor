//! Conversioni tra la configurazione e quello che mostra la finestra: le righe degli elenchi.
//! I campi dell'editor dei passi stanno in `steps`.

use slint::{ModelRc, SharedString, VecModel};

use super::{ActionRow, PaletteItem, PickItem, SlotRow, WheelRow, steps};
use crate::config::{Config, Item, Slot, Wheel};
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

pub fn action_row(config: &Config, item: &Item) -> ActionRow {
    ActionRow {
        id: item.id.as_str().into(),
        label: item.label.as_str().into(),
        glyph: icons::glyph(&item.icon).into(),
        detail: summary(config, item).into(),
    }
}

/// Le azioni come voci della finestra di scelta.
pub fn action_item(config: &Config, item: &Item) -> PickItem {
    let row = action_row(config, item);
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

/// A cosa serve la ruota, in breve.
fn wheel_role(config: &Config, wheel: &Wheel) -> String {
    if wheel.is_main() {
        return tr!("Ruota principale", "Main wheel").into();
    }
    if !wheel.apps.is_empty() {
        return trf!("Per {}", "For {}", wheel.apps.join(", "));
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

/// Nomi delle azioni che eseguono l'azione `id` con un loro passo.
pub fn runners<'a>(config: &'a Config, id: &str) -> Vec<&'a str> {
    config.actions.iter().filter(|a| a.runs().any(|run| run == id)).map(|a| a.label.as_str()).collect()
}

/// La riga sotto il nome della ruota: quando si apre. `hotkey` è la scorciatoia scelta
/// ("Ctrl+Alt+W"), anche se non ancora salvata.
pub fn wheel_hint(config: &Config, wheel: &Wheel, hotkey: &str) -> String {
    let hotkey = match keys::caps(hotkey).join("+") {
        caps if caps.is_empty() => tr!("la scorciatoia", "the shortcut").to_string(),
        caps => caps,
    };
    if wheel.is_main() {
        return trf!("Si apre con {hotkey}.", "Opens with {hotkey}.");
    }
    let parents = parents(config, wheel);
    let names = parents.join(", ");
    match (wheel.apps.is_empty(), parents.len()) {
        (true, 0) => tr!(
            "Non si apre ancora: aggiungila a un'altra ruota, o collegala a qualche app qui sotto.",
            "It doesn't open yet: add it to another wheel, or link it to some apps below."
        )
        .into(),
        (true, 1) => trf!("Si apre dalla ruota {names}.", "Opens from the {names} wheel."),
        (true, _) => trf!("Si apre dalle ruote {names}.", "Opens from the wheels {names}."),
        (false, _) => trf!(
            "Si apre con {hotkey} quando usi una delle app collegate.",
            "Opens with {hotkey} when you're using one of the linked apps."
        ),
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
                detail: summary(config, item).into(),
            }),
            Slot::Wheel(id) => config.wheel(id).map(|sub| SlotRow {
                label: sub.name.as_str().into(),
                glyph: wheel_glyph(sub),
                initial: initial(&sub.name),
                submenu: true,
                detail: submenu_detail(sub).into(),
            }),
        })
        .collect()
}

fn submenu_detail(wheel: &Wheel) -> String {
    match wheel.slots.len() {
        1 => tr!("Sottomenu · 1 elemento", "Submenu · 1 item").into(),
        n => trf!("Sottomenu · {n} elementi", "Submenu · {n} items"),
    }
}

/// Le voci della finestra «Aggiungi» della ruota `wheel`: le azioni (`submenus` falso) o le ruote che si possono
/// aggiungere come sottomenu senza creare cicli, filtrate dalla ricerca.
pub fn palette(config: &Config, wheel: &Wheel, submenus: bool, query: &str) -> Vec<PaletteItem> {
    let query = query.trim().to_lowercase();
    let matches = |item: &PaletteItem| {
        query.is_empty() || item.label.to_lowercase().contains(&query) || item.detail.to_lowercase().contains(&query)
    };
    let items: Vec<PaletteItem> = if submenus {
        config
            .wheels
            .iter()
            .filter(|w| w.id != wheel.id && !config.reaches(&w.id, &wheel.id))
            .map(|sub| PaletteItem {
                id: sub.id.as_str().into(),
                glyph: wheel_glyph(sub),
                initial: initial(&sub.name),
                label: sub.name.as_str().into(),
                detail: submenu_detail(sub).into(),
                submenu: true,
                used: wheel.slots.contains(&Slot::Wheel(sub.id.clone())),
            })
            .collect()
    } else {
        config
            .actions
            .iter()
            .map(|item| PaletteItem {
                id: item.id.as_str().into(),
                glyph: icons::glyph(&item.icon).into(),
                initial: initial(&item.label),
                label: item.label.as_str().into(),
                detail: summary(config, item).into(),
                submenu: false,
                used: wheel.slots.contains(&Slot::Action(item.id.clone())),
            })
            .collect()
    };
    items.into_iter().filter(matches).collect()
}

/// Prima lettera, maiuscola: sulla ruota prende il posto dell'icona mancante.
fn initial(label: &str) -> SharedString {
    label.chars().take(1).flat_map(char::to_uppercase).collect::<String>().into()
}

/// Dove è usata un'azione, da mostrare sotto il nome.
pub fn usage(config: &Config, id: &str) -> String {
    let wheels = containing(config, &Slot::Action(id.to_string())).join(", ");
    let runners = runners(config, id).join(", ");
    match (wheels.is_empty(), runners.is_empty()) {
        (true, true) => tr!("Non è ancora in nessuna ruota.", "Not in any wheel yet.").into(),
        (false, true) => trf!("Nelle ruote: {wheels}", "In the wheels: {wheels}"),
        (true, false) => trf!("Eseguita da: {runners}", "Run by: {runners}"),
        (false, false) => trf!("Nelle ruote: {wheels} · eseguita da: {runners}", "In the wheels: {wheels} · run by: {runners}"),
    }
}

/// Riassunto di una riga di cosa fa l'azione: il suo unico passo, o i tipi dei passi.
pub fn summary(config: &Config, item: &Item) -> String {
    match item.steps.as_slice() {
        [step] => match steps::summary(config, step) {
            text if text.is_empty() => step.name().to_string(),
            text => format!("{} · {text}", step.name()),
        },
        all => {
            let names: Vec<&str> = all.iter().map(|step| step.name()).collect();
            trf!("{} passi · {}", "{} steps · {}", all.len(), names.join(", "))
        }
    }
}
