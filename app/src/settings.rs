//! Finestra delle impostazioni, in un processo separato (`wheelor.exe --settings`).
//! L'aspetto è descritto in ui/settings.slint; qui la colleghiamo ai dati.
//!
//! La configurazione in lavorazione vive in `App::config`: ogni modifica fatta nella finestra
//! la aggiorna subito, e "Salva" la scrive su disco.

// Genera da ui/settings.slint i tipi Rust SettingsWindow, WheelRow, ActionRow, …
slint::include_modules!();

mod capture;
mod rows;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

use crate::actions::{self, Context};
use crate::config::{self, Accent, Action, Config, Item, MAIN_WHEEL, MAX_SLOTS, MIN_OPACITY, Slot, Wheel, WheelSize};
use crate::keys::Combo;
use crate::update::{self, Release};
use crate::i18n::{self, Language};
use crate::{MAIN_CLASS, WM_RELOAD_CONFIG, autostart, hotkey, icons, keys, overlay, package, tr, trf};
use capture::Event;
use rows::caps_model;

/// Il titolo della finestra nelle due lingue (deve coincidere con `title` in ui/settings.slint e la
/// sua traduzione): serve a ritrovarla se è già aperta, e lo usa anche l'installer.
const TITLES: [PCWSTR; 2] = [w!("Wheelor – Impostazioni"), w!("Wheelor – Settings")];

// Cosa sta scegliendo la finestra di scelta (proprietà `picker` in Slint).
const PICK_ACTION: i32 = 1;
const PICK_SUBMENU: i32 = 2;
const PICK_APP: i32 = 3;

// Per chi è aperto il selettore delle icone (proprietà `picking-icon`).
const ICON_FOR_ACTION: i32 = 1;
const ICON_FOR_WHEEL: i32 = 2;

// Cosa chiede di eliminare la finestra di conferma (proprietà `deleting`).
const DELETE_WHEEL: i32 = 1;
const DELETE_ACTION: i32 = 2;

// Quale campo sta registrando una combinazione di tasti (proprietà `listening`).
const LISTEN_HOTKEY: i32 = 1;
const LISTEN_KEYS: i32 = 2;

// A che punto è l'aggiornamento (proprietà `update-state`).
const UPDATE_IDLE: i32 = 0;
const UPDATE_CHECKING: i32 = 1;
const UPDATE_CONFIRM: i32 = 2;
const UPDATE_DOWNLOADING: i32 = 3;
const UPDATE_INSTALLING: i32 = 4;

thread_local! {
    /// La nuova versione trovata, da installare se l'utente lo chiede. Sta qui e non in `App`
    /// perché la imposta una closure che arriva da un altro thread e riceve solo la finestra.
    static RELEASE: RefCell<Option<Release>> = const { RefCell::new(None) };
}

pub fn run() {
    // Una sola finestra delle impostazioni: se è già aperta, la portiamo in primo piano.
    let _mutex = unsafe { CreateMutexW(None, false, w!("Local\\Wheelor.Settings")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            if let Some(hwnd) = TITLES.iter().find_map(|&title| FindWindowW(PCWSTR::null(), title).ok()) {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
        }
        return;
    }

    if let Err(e) = show() {
        let text = windows::core::HSTRING::from(trf!("Impossibile aprire le impostazioni:\n{e}", "Can't open the settings:\n{e}"));
        unsafe {
            MessageBoxW(None, &text, w!("Wheelor"), MB_OK | MB_ICONERROR);
        }
    }
}

/// Lo stato della finestra, condiviso da tutte le callback tramite `Rc<App>`.
///
/// I modelli (`VecModel`) sono liste osservabili: quando cambiano, Slint aggiorna gli elenchi.
/// `ui` è un riferimento debole: la finestra possiede le callback, le callback possiedono
/// `App`, e se `App` possedesse la finestra si formerebbe un ciclo mai liberato.
struct App {
    ui: slint::Weak<SettingsWindow>,
    config: RefCell<Config>,
    wheels: Rc<VecModel<WheelRow>>,
    slots: Rc<VecModel<SlotRow>>,
    actions: Rc<VecModel<ActionRow>>,
    picker: Rc<VecModel<PickItem>>,
    /// Tutte le voci della finestra di scelta, prima del filtro di ricerca.
    picker_all: RefCell<Vec<PickItem>>,
}

fn show() -> Result<(), slint::PlatformError> {
    // Se il file ha errori partiamo dai valori predefiniti, avvisando che il salvataggio lo sostituirà.
    i18n::set(None);
    let (config, load_error) = match config::load() {
        Ok(config) => (config, None),
        Err(e) => (Config::default(), Some(e)),
    };
    i18n::set(config.language);
    // La versione dello Store la aggiorna lo Store.
    let check = config.check_updates && !package::is_store();
    let (ui, app) = build(config)?;
    // Dopo `build`: la scelta della lingua per Slint vale solo quando esiste già una finestra.
    i18n::apply_to_slint();
    if let Some(error) = load_error {
        let text = trf!("{error} — salvando, il file verrà sostituito.", "{error} — saving will replace the file.");
        set_status(&ui, &text, true);
    }
    connect(&ui, &app);
    // Aperta dalla notifica di aggiornamento (o dal menu della tray): si propone subito di installarlo.
    let ask = std::env::args().any(|a| a == "--update");
    if ask {
        ui.set_page(3);
    }
    if check || (ask && !package::is_store()) {
        app.check_for_updates(ask);
    }
    set_icon_when_ready(ui.as_weak(), 50);
    ui.run()
}

/// Crea la finestra e la riempie con `config`.
fn build(config: Config) -> Result<(SettingsWindow, Rc<App>), slint::PlatformError> {
    let ui = SettingsWindow::new()?;
    let app = Rc::new(App {
        ui: ui.as_weak(),
        config: RefCell::new(config),
        wheels: Rc::new(VecModel::default()),
        slots: Rc::new(VecModel::default()),
        actions: Rc::new(VecModel::default()),
        picker: Rc::new(VecModel::default()),
        picker_all: RefCell::new(Vec::new()),
    });
    ui.set_wheels(ModelRc::from(app.wheels.clone()));
    ui.set_slots(ModelRc::from(app.slots.clone()));
    ui.set_actions(ModelRc::from(app.actions.clone()));
    ui.set_picker_items(ModelRc::from(app.picker.clone()));
    ui.global::<Fonts>().set_icons(icons::font_family().into());
    ui.set_max_slots(MAX_SLOTS as i32);
    ui.set_config_path(package::visible_path(config::path()).display().to_string().into());
    ui.set_store(package::is_store());
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    {
        let config = app.config.borrow();
        ui.set_hotkey(config.hotkey.as_str().into());
        ui.set_hotkey_caps(caps_model(&config.hotkey));
        if let Err(e) = keys::parse(&config.hotkey) {
            set_hotkey_note(&ui, &trf!("{e}: scegline un'altra.", "{e}: choose another one."), true);
        }
        ui.set_wheel_size(match config.size {
            WheelSize::Normal => 0,
            WheelSize::Large => 1,
            WheelSize::Huge => 2,
        });
        ui.set_wheel_opacity(f32::from(config.opacity));
        set_accents(&ui);
        ui.set_accent(Accent::ALL.iter().position(|a| *a == config.accent).unwrap_or(0) as i32);
        apply_accent(&ui, config.accent);
        ui.set_aim_anywhere(config.aim_anywhere);
        ui.set_language(match config.language {
            None => 0,
            Some(Language::Italian) => 1,
            Some(Language::English) => 2,
        });
        ui.set_check_updates(config.check_updates);
    }
    ui.set_autostart(autostart::is_enabled());
    ui.set_update_message(update_message().into());
    set_icons(&ui, "");

    app.refresh_wheels();
    app.refresh_actions();
    app.select_wheel(MAIN_WHEEL);
    let first = app.config.borrow().actions.first().map(|a| a.id.clone());
    if let Some(id) = first {
        app.select_action(&id);
    }
    Ok((ui, app))
}

/// Collega ogni callback dell'interfaccia al metodo di `App` che la gestisce.
/// Ogni closure riceve una copia di `Rc<App>` (conteggio dei riferimenti, nessuna copia dei dati).
fn connect(ui: &SettingsWindow, app: &Rc<App>) {
    // Piccola macro locale: `on!(on_x, |app, a, b| …)` diventa
    // `{ let app = app.clone(); ui.on_x(move |a, b| …) }`, così non lo ripetiamo ovunque.
    // Il primo nome è la copia di `Rc<App>` che la closure si porta dietro (`move`).
    macro_rules! on {
        ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {{
            let $app = app.clone();
            ui.$setter(move |$($arg),*| $body);
        }};
    }

    ui.on_glyph(|code| icons::glyph(&code).into());
    ui.on_open_config_folder(open_config_folder);

    // Ruote.
    on!(on_select_wheel, |app, id| app.select_wheel(&id));
    on!(on_new_wheel, |app| app.new_wheel());
    on!(on_wheel_edited, |app| app.wheel_edited());
    on!(on_preview_clicked, |app, x, y| app.preview_clicked(x, y));
    on!(on_select_slot, |app, index| app.ui().set_current_slot(index));
    on!(on_move_slot, |app, delta| app.move_slot(delta));
    on!(on_remove_slot, |app| app.remove_slot());
    on!(on_open_slot, |app| app.open_slot());
    on!(on_open_picker, |app, mode| app.open_picker(mode));
    on!(on_picker_search_changed, |app, text| app.filter_picker(&text));
    on!(on_picked, |app, id| app.picked(&id));

    // Libreria.
    on!(on_select_action, |app, id| app.select_action(&id));
    on!(on_new_action, |app| app.new_action());
    on!(on_duplicate_action, |app| app.duplicate_action());
    on!(on_action_edited, |app| app.action_edited());
    on!(on_search_actions, |app, _text| app.refresh_actions());
    on!(on_browse, |app| app.browse());
    on!(on_test, |app| app.test());

    // Eliminazioni: prima la conferma, poi il lavoro vero.
    on!(on_ask_delete, |app, what| app.ask_delete(what));
    on!(on_confirm_delete, |app| app.confirm_delete());

    // Icone, pagina Generale e salvataggio.
    on!(on_search_icons, |app, query| set_icons(&app.ui(), &query));
    on!(on_pick_icon, |app, code| app.pick_icon(code));
    on!(on_general_edited, |app| app.mark_dirty());
    on!(on_accent_picked, |app, index| app.accent_picked(index));
    on!(on_language_picked, |app, index| app.language_picked(index));
    on!(on_save, |app| app.save());
    on!(on_close_without_saving, |app| {
        let _ = app.ui().hide();
    });

    // Combinazioni di tasti: si registrano premendole.
    on!(on_listen, |app, target| app.listen(target));
    on!(on_stop_listening, |app| app.stop_listening());

    // Aggiornamenti.
    on!(on_check_for_updates, |app| app.check_for_updates(false));
    on!(on_ask_update, |app| app.ask_update());
    on!(on_cancel_update, |app| app.ui().set_update_state(UPDATE_IDLE));
    on!(on_confirm_update, |app| app.confirm_update());
    ui.on_open_release_notes(|| {
        let url = RELEASE.with_borrow(|r| r.as_ref().map(Release::notes_url));
        open_url(&url.unwrap_or_else(|| "https://wheelor.app".into()));
    });

    // Chiudendo con modifiche non salvate, chiediamo conferma invece di chiudere.
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        let ui = weak.unwrap();
        if ui.get_dirty() {
            ui.set_confirm_close(true);
            CloseRequestResponse::KeepWindowShown
        } else {
            CloseRequestResponse::HideWindow
        }
    });
}

impl App {
    fn ui(&self) -> SettingsWindow {
        self.ui.unwrap()
    }

    fn mark_dirty(&self) {
        let ui = self.ui();
        ui.set_dirty(true);
        set_status(&ui, "", false);
    }

    // ── Ruote ────────────────────────────────────────────────────────────────

    fn current_wheel(&self) -> String {
        self.ui().get_current_wheel().to_string()
    }

    /// Esegue `f` sulla ruota selezionata (se esiste) e restituisce il suo risultato.
    fn with_current_wheel<R>(&self, f: impl FnOnce(&mut Wheel) -> R) -> Option<R> {
        let id = self.current_wheel();
        let mut config = self.config.borrow_mut();
        config.wheels.iter_mut().find(|w| w.id == id).map(f)
    }

    fn refresh_wheels(&self) {
        let config = self.config.borrow();
        sync(&self.wheels, config.wheels.iter().map(|w| rows::wheel_row(&config, w)).collect());
        self.ui().set_action_count(config.actions.len() as i32);
    }

    fn refresh_slots(&self) {
        let config = self.config.borrow();
        let rows = config.wheel(&self.current_wheel()).map(|w| rows::slot_rows(&config, w)).unwrap_or_default();
        sync(&self.slots, rows);
    }

    fn select_wheel(&self, id: &str) {
        let ui = self.ui();
        {
            let config = self.config.borrow();
            let Some(wheel) = config.wheel(id) else { return };
            ui.set_current_wheel(id.into());
            ui.set_wh_name(wheel.name.as_str().into());
            ui.set_wh_icon(wheel.icon.as_str().into());
            ui.set_wh_apps(wheel.apps.join(", ").into());
            ui.set_wh_main(wheel.is_main());
            ui.set_wh_hint(rows::wheel_hint(&config, wheel).into());
        }
        ui.set_current_slot(-1);
        ui.set_picking_icon(0);
        ui.set_page(0);
        self.refresh_slots();
    }

    fn wheel_edited(&self) {
        let ui = self.ui();
        let apps: Vec<String> =
            ui.get_wh_apps().split([',', ';']).map(str::trim).filter(|a| !a.is_empty()).map(String::from).collect();
        self.with_current_wheel(|wheel| {
            wheel.name = ui.get_wh_name().trim().to_string();
            wheel.icon = ui.get_wh_icon().to_string();
            wheel.apps = apps;
        });
        let config = self.config.borrow();
        if let Some(wheel) = config.wheel(&self.current_wheel()) {
            ui.set_wh_hint(rows::wheel_hint(&config, wheel).into());
        }
        drop(config); // rilasciamo il prestito prima che refresh_wheels ne chieda un altro
        self.refresh_wheels();
        self.mark_dirty();
    }

    fn new_wheel(&self) {
        let id = {
            let mut config = self.config.borrow_mut();
            let name = tr!("Nuova ruota", "New wheel");
            let id = config::unique_id(name, |id| config.wheel(id).is_some());
            config.wheels.push(Wheel::new(&id, name));
            id
        };
        self.refresh_wheels();
        self.select_wheel(&id);
        self.mark_dirty();
    }

    /// Apre la finestra di conferma, spiegando cosa sparirà insieme a ciò che si elimina.
    fn ask_delete(&self, what: i32) {
        let ui = self.ui();
        let (title, text) = {
            let config = self.config.borrow();
            match what {
                DELETE_WHEEL => {
                    let Some(wheel) = config.wheel(&self.current_wheel()) else { return };
                    if wheel.is_main() {
                        return;
                    }
                    let parents = rows::parents(&config, wheel).join(", ");
                    let mut text = match parents.is_empty() {
                        true => String::new(),
                        false => trf!("Sparisce anche da: {parents}. ", "It also disappears from: {parents}. "),
                    };
                    text += match wheel.slots.is_empty() {
                        true => tr!("Non contiene elementi.", "It has no items."),
                        false => tr!("Le azioni che contiene restano nella libreria.", "Its actions stay in the library."),
                    };
                    (trf!("Eliminare la ruota «{}»?", "Delete the wheel “{}”?", wheel.name), text)
                }
                DELETE_ACTION => {
                    let Some(item) = config.action(&ui.get_current_action()) else { return };
                    let wheels = rows::containing(&config, &Slot::Action(item.id.clone()));
                    let wheels = wheels.join(", ");
                    let text = if wheels.is_empty() {
                        tr!("Non è in nessuna ruota.", "It isn't in any wheel.").to_string()
                    } else {
                        trf!("Viene tolta anche dalle ruote: {wheels}.", "It's also removed from the wheels: {wheels}.")
                    };
                    (trf!("Eliminare l'azione «{}»?", "Delete the action “{}”?", item.label), text)
                }
                _ => return,
            }
        };
        ui.set_delete_title(title.into());
        ui.set_delete_text(text.into());
        ui.set_deleting(what);
    }

    fn confirm_delete(&self) {
        let ui = self.ui();
        let what = ui.get_deleting();
        ui.set_deleting(0);
        match what {
            DELETE_WHEEL => self.delete_wheel(),
            DELETE_ACTION => self.delete_action(),
            _ => {}
        }
    }

    fn delete_wheel(&self) {
        let id = self.current_wheel();
        if id == MAIN_WHEEL {
            return;
        }
        {
            let mut config = self.config.borrow_mut();
            config.wheels.retain(|w| w.id != id);
            // Toglie la ruota anche da dove era usata come sottomenu.
            let slot = Slot::Wheel(id);
            for wheel in &mut config.wheels {
                wheel.slots.retain(|s| *s != slot);
            }
        }
        self.refresh_wheels();
        self.select_wheel(MAIN_WHEEL);
        self.mark_dirty();
    }

    /// Clic sull'anteprima: lo stesso calcolo della ruota vera (zona morta nel mozzo).
    fn preview_clicked(&self, x: f32, y: f32) {
        let count = self.slots.row_count();
        let index = if count == 0 || x.hypot(y) < overlay::DEAD_ZONE { -1 } else { overlay::slice_at(y.atan2(x), count) as i32 };
        self.ui().set_current_slot(index);
    }

    fn move_slot(&self, delta: i32) {
        let ui = self.ui();
        let from = ui.get_current_slot();
        let to = from + delta;
        let moved = self.with_current_wheel(|wheel| {
            let range = 0..wheel.slots.len() as i32;
            if !range.contains(&from) || !range.contains(&to) {
                return false;
            }
            wheel.slots.swap(from as usize, to as usize);
            true
        });
        if moved == Some(true) {
            ui.set_current_slot(to);
            self.refresh_slots();
            self.mark_dirty();
        }
    }

    fn remove_slot(&self) {
        let ui = self.ui();
        let Ok(index) = usize::try_from(ui.get_current_slot()) else { return };
        let remaining = self.with_current_wheel(|wheel| {
            if index < wheel.slots.len() {
                wheel.slots.remove(index);
            }
            wheel.slots.len()
        });
        let remaining = remaining.unwrap_or(0);
        ui.set_current_slot(if remaining == 0 { -1 } else { index.min(remaining - 1) as i32 });
        self.refresh_slots();
        self.refresh_wheels(); // il ruolo delle ruote ("Sottomenu di …") può cambiare
        self.mark_dirty();
    }

    /// "Modifica": apre l'azione nella libreria, o la ruota del sottomenu.
    fn open_slot(&self) {
        let ui = self.ui();
        let Ok(index) = usize::try_from(ui.get_current_slot()) else { return };
        let slot = self.with_current_wheel(|wheel| wheel.slots.get(index).cloned()).flatten();
        match slot {
            Some(Slot::Action(id)) => {
                ui.set_action_search(SharedString::new());
                self.refresh_actions();
                self.select_action(&id);
                ui.set_page(1);
            }
            Some(Slot::Wheel(id)) => self.select_wheel(&id),
            None => {}
        }
    }

    // ── Finestra di scelta ───────────────────────────────────────────────────

    fn open_picker(&self, mode: i32) {
        let current = self.current_wheel();
        let (title, items) = {
            let config = self.config.borrow();
            match mode {
                PICK_ACTION => (
                    tr!("Aggiungi un'azione alla ruota", "Add an action to the wheel"),
                    config.actions.iter().map(rows::action_item).collect(),
                ),
                PICK_SUBMENU => (
                    tr!("Aggiungi un sottomenu", "Add a submenu"),
                    // Niente cicli: non la ruota stessa, né una ruota da cui si torna a questa.
                    config
                        .wheels
                        .iter()
                        .filter(|w| w.id != current && !config.reaches(&w.id, &current))
                        .map(|w| rows::wheel_item(&config, w))
                        .collect(),
                ),
                _ => (tr!("Scegli un'app aperta", "Choose an open app"), running_apps()),
            }
        };
        let ui = self.ui();
        ui.set_picker_title(title.into());
        ui.set_picker_search(SharedString::new());
        self.picker.set_vec(items.clone());
        self.picker_all.replace(items);
        ui.set_picker(mode);
    }

    fn filter_picker(&self, query: &str) {
        let query = query.trim().to_lowercase();
        let matches = |item: &&PickItem| {
            query.is_empty()
                || item.label.to_lowercase().contains(&query)
                || item.detail.to_lowercase().contains(&query)
        };
        self.picker.set_vec(self.picker_all.borrow().iter().filter(matches).cloned().collect::<Vec<_>>());
    }

    fn picked(&self, id: &str) {
        let ui = self.ui();
        let mode = ui.get_picker();
        ui.set_picker(0);
        if mode == PICK_APP {
            let apps = ui.get_wh_apps();
            let already = apps.split([',', ';']).any(|a| a.trim().eq_ignore_ascii_case(id));
            if !already {
                let base = apps.trim().trim_end_matches([',', ';']).trim_end();
                let text = if base.is_empty() { id.to_string() } else { format!("{base}, {id}") };
                ui.set_wh_apps(text.into());
                self.wheel_edited();
            }
            return;
        }
        let slot = if mode == PICK_SUBMENU { Slot::Wheel(id.into()) } else { Slot::Action(id.into()) };
        let added = self.with_current_wheel(|wheel| {
            if wheel.slots.len() >= MAX_SLOTS {
                return None;
            }
            wheel.slots.push(slot);
            Some(wheel.slots.len() - 1)
        });
        if let Some(Some(index)) = added {
            ui.set_current_slot(index as i32);
            self.refresh_slots();
            self.refresh_wheels();
            self.mark_dirty();
        }
    }

    // ── Libreria ─────────────────────────────────────────────────────────────

    /// Ricostruisce l'elenco della libreria applicando la ricerca.
    fn refresh_actions(&self) {
        let query = self.ui().get_action_search().trim().to_lowercase();
        let config = self.config.borrow();
        let rows = config
            .actions
            .iter()
            .map(rows::action_row)
            .filter(|row| {
                query.is_empty() || row.label.to_lowercase().contains(&query) || row.detail.to_lowercase().contains(&query)
            })
            .collect();
        sync(&self.actions, rows);
    }

    fn select_action(&self, id: &str) {
        let ui = self.ui();
        let config = self.config.borrow();
        let Some(item) = config.action(id) else { return };
        ui.set_current_action(id.into());
        ui.set_picking_icon(0);
        rows::fill_editor(&ui, item);
        ui.set_ed_usage(rows::usage(&config, id).into());
    }

    fn action_edited(&self) {
        let ui = self.ui();
        ui.set_ed_keys_caps(caps_model(&ui.get_ed_keys()));
        let id = ui.get_current_action().to_string();
        let item = rows::read_editor(&ui, &id);
        {
            let mut config = self.config.borrow_mut();
            let Some(existing) = config.actions.iter_mut().find(|a| a.id == id) else { return };
            *existing = item;
        }
        self.refresh_actions();
        self.refresh_slots();
        self.mark_dirty();
    }

    /// Aggiunge `item` alla libreria (con un id nuovo) subito dopo `after`, e lo seleziona.
    fn insert_action(&self, mut item: Item, after: Option<&str>) {
        {
            let mut config = self.config.borrow_mut();
            item.id = config::unique_id(&item.label, |id| config.action(id).is_some());
            let position = after
                .and_then(|after| config.actions.iter().position(|a| a.id == after))
                .map_or(config.actions.len(), |i| i + 1);
            config.actions.insert(position, item.clone());
        }
        // La nuova azione potrebbe non corrispondere alla ricerca in corso: la azzeriamo.
        let ui = self.ui();
        ui.set_action_search(SharedString::new());
        self.refresh_actions();
        self.refresh_wheels();
        self.select_action(&item.id);
        ui.set_page(1);
        self.mark_dirty();
    }

    fn new_action(&self) {
        let item = Item {
            id: String::new(),
            label: tr!("Nuova azione", "New action").into(),
            icon: "E710".into(),
            action: Action::Command {
                command: String::new(),
                shell: Default::default(),
                window: Default::default(),
                cwd: String::new(),
                admin: false,
            },
        };
        self.insert_action(item, None);
    }

    fn duplicate_action(&self) {
        let id = self.ui().get_current_action().to_string();
        let Some(mut copy) = self.config.borrow().action(&id).cloned() else { return };
        copy.label = trf!("{} (copia)", "{} (copy)", copy.label);
        self.insert_action(copy, Some(&id));
    }

    fn delete_action(&self) {
        let ui = self.ui();
        let id = ui.get_current_action().to_string();
        // Posizione nell'elenco visibile (che può essere filtrato dalla ricerca).
        let shown = self.actions.iter().position(|row| row.id == id.as_str()).unwrap_or(0);
        {
            let mut config = self.config.borrow_mut();
            let Some(index) = config.actions.iter().position(|a| a.id == id) else { return };
            config.actions.remove(index);
            // Toglie l'azione anche da tutte le ruote che la usavano.
            let slot = Slot::Action(id);
            for wheel in &mut config.wheels {
                wheel.slots.retain(|s| *s != slot);
            }
        }
        self.refresh_actions();
        self.refresh_wheels();
        self.refresh_slots();
        ui.set_current_slot(-1);
        // Seleziona la riga che ha preso il suo posto, o l'ultima se era in fondo.
        let count = self.actions.row_count();
        match self.actions.row_data(shown.min(count.saturating_sub(1))) {
            Some(row) => self.select_action(&row.id),
            None => ui.set_current_action(SharedString::new()),
        }
        self.mark_dirty();
    }

    fn browse(&self) {
        if let Some(path) = pick_file() {
            self.ui().set_ed_target(path.into());
            self.action_edited();
        }
    }

    fn test(&self) {
        let ui = self.ui();
        let item = rows::read_editor(&ui, "");
        set_status(&ui, tr!("Azione avviata…", "Action started…"), false);
        // L'eventuale errore arriva da un altro thread: upgrade_in_event_loop riporta
        // l'aggiornamento sul thread dell'interfaccia, l'unico che può toccarla.
        let weak = self.ui.clone();
        actions::run(item.action, Context::default(), move |error| {
            let _ = weak.upgrade_in_event_loop(move |ui| set_status(&ui, &error, true));
        });
    }

    fn pick_icon(&self, code: SharedString) {
        let ui = self.ui();
        let target = ui.get_picking_icon();
        ui.set_picking_icon(0);
        match target {
            ICON_FOR_ACTION => {
                ui.set_ed_icon(code);
                self.action_edited();
            }
            ICON_FOR_WHEEL => {
                ui.set_wh_icon(code);
                self.wheel_edited();
            }
            _ => {}
        }
    }

    // ── Combinazioni di tasti ────────────────────────────────────────────────

    /// Un campo "tasti" inizia ad ascoltare: la prossima combinazione premuta va lì.
    fn listen(self: &Rc<Self>, target: i32) {
        let ui = self.ui();
        let Some(window) = window_handle(&ui) else { return };
        // Debole: lo stato della registrazione non deve tenere in vita `App`.
        let app = Rc::downgrade(self);
        let started = capture::start(window, move |event| {
            if let Some(app) = app.upgrade() {
                app.captured(target, event);
            }
        });
        match started {
            Ok(()) => {
                ui.set_held_caps(caps_model(""));
                ui.set_listening(target);
            }
            Err(e) => set_status(&ui, &e, true),
        }
    }

    fn stop_listening(&self) {
        capture::stop();
        self.ui().set_listening(0);
    }

    fn captured(&self, target: i32, event: Event) {
        let ui = self.ui();
        match event {
            Event::Held(modifiers) => ui.set_held_caps(ModelRc::new(VecModel::from(
                keys::modifier_names(&modifiers).into_iter().map(SharedString::from).collect::<Vec<_>>(),
            ))),
            Event::Cancelled => ui.set_listening(0),
            Event::Done(combo) => {
                ui.set_listening(0);
                match target {
                    LISTEN_HOTKEY => self.hotkey_chosen(&combo),
                    LISTEN_KEYS => {
                        ui.set_ed_keys(combo.text().into());
                        self.action_edited();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Una nuova scorciatoia per la ruota: si usa solo se funziona davvero.
    fn hotkey_chosen(&self, combo: &Combo) {
        let ui = self.ui();
        let text = combo.text();
        if let Some(problem) = keys::hotkey_problem(combo) {
            set_hotkey_note(&ui, &problem, true);
            return;
        }
        // Quella salvata è registrata da Wheelor stesso: risulterebbe occupata, ma va bene.
        let saved = keys::parse(&self.config.borrow().hotkey).ok();
        if saved.as_ref() != Some(combo) && !hotkey::is_free(combo) {
            let shown = keys::caps(&text).join("+");
            let text = trf!(
                "{shown} è già usata da Windows o da un'altra app: prova un'altra combinazione.",
                "{shown} is already used by Windows or another app: try another combination."
            );
            set_hotkey_note(&ui, &text, true);
            return;
        }
        ui.set_hotkey_caps(caps_model(&text));
        if text != ui.get_hotkey().as_str() {
            ui.set_hotkey(text.into());
            self.mark_dirty();
            set_hotkey_note(&ui, tr!("È libera: premi Salva per usarla.", "It's free: press Save to use it."), false);
        } else {
            set_hotkey_note(&ui, "", false);
        }
    }

    // ── Aspetto ──────────────────────────────────────────────────────────────

    /// Il colore si vede subito in tutta la finestra; sulla ruota, dopo il salvataggio.
    fn accent_picked(&self, index: i32) {
        if let Some(&accent) = usize::try_from(index).ok().and_then(|i| Accent::ALL.get(i)) {
            apply_accent(&self.ui(), accent);
            self.mark_dirty();
        }
    }

    /// Cambia la lingua subito, in tutta la finestra (la ruota e la tray dopo il salvataggio).
    fn language_picked(&self, index: i32) {
        i18n::set(language_choice(index));
        i18n::apply_to_slint();
        self.refresh_texts();
        self.mark_dirty();
    }

    /// Rifà i testi preparati in Rust, dopo un cambio di lingua.
    fn refresh_texts(&self) {
        let ui = self.ui();
        self.refresh_wheels();
        self.refresh_slots();
        self.refresh_actions();
        set_accents(&ui);
        ui.set_hotkey_caps(caps_model(&ui.get_hotkey()));
        ui.set_ed_keys_caps(caps_model(&ui.get_ed_keys()));
        set_hotkey_note(&ui, "", false);
        {
            let config = self.config.borrow();
            if let Some(wheel) = config.wheel(&self.current_wheel()) {
                ui.set_wh_hint(rows::wheel_hint(&config, wheel).into());
            }
            ui.set_ed_usage(rows::usage(&config, &ui.get_current_action()).into());
        }
        ui.set_update_error(false);
        ui.set_update_message(update_message().into());
        if let Some(release) = RELEASE.with_borrow(Clone::clone) {
            ui.set_update_size(release.size_text().into());
        }
    }

    // ── Aggiornamenti ────────────────────────────────────────────────────────

    /// Cerca una nuova versione su un altro thread. Con `ask`, se c'è propone subito di installarla.
    fn check_for_updates(&self, ask: bool) {
        let ui = self.ui();
        ui.set_update_state(UPDATE_CHECKING);
        ui.set_update_error(false);
        ui.set_update_message(tr!("Controllo su wheelor.app…", "Checking wheelor.app…").into());
        let weak = self.ui.clone();
        std::thread::spawn(move || {
            let result = update::check();
            let _ = weak.upgrade_in_event_loop(move |ui| show_update(&ui, result, ask));
        });
    }

    fn ask_update(&self) {
        if RELEASE.with_borrow(Option::is_some) {
            self.ui().set_update_state(UPDATE_CONFIRM);
        }
    }

    /// Salva le modifiche in corso, scarica l'installer e lo avvia; poi la finestra si chiude
    /// (l'installer chiude e riavvia Wheelor in background).
    fn confirm_update(&self) {
        let ui = self.ui();
        let Some(release) = RELEASE.with_borrow(Clone::clone) else {
            ui.set_update_state(UPDATE_IDLE);
            return;
        };
        if ui.get_dirty() {
            if let Err(e) = self.write() {
                ui.set_update_state(UPDATE_IDLE);
                let text = trf!("Prima di aggiornare vanno salvate le modifiche: {e}", "Your changes must be saved before updating: {e}");
                set_status(&ui, &text, true);
                return;
            }
            ui.set_dirty(false);
        }
        ui.set_update_progress(0.0);
        ui.set_update_state(UPDATE_DOWNLOADING);

        let weak = self.ui.clone();
        std::thread::spawn(move || {
            // L'avanzamento arriva all'interfaccia solo quando cambia la percentuale.
            let mut shown = None;
            let progress = |done: u64, total: u64| {
                let percent = (done * 100).checked_div(total).unwrap_or(0);
                if shown != Some(percent) {
                    shown = Some(percent);
                    let _ = weak.upgrade_in_event_loop(move |ui| ui.set_update_progress(percent as f32 / 100.0));
                }
            };
            let result = update::download(&release, progress).and_then(|setup| update::install(&setup));
            let _ = weak.upgrade_in_event_loop(move |ui| match result {
                Ok(()) => {
                    ui.set_update_state(UPDATE_INSTALLING);
                    // Un attimo per leggere il messaggio, poi si lascia il posto all'installer.
                    slint::Timer::single_shot(Duration::from_millis(800), || {
                        let _ = slint::quit_event_loop();
                    });
                }
                Err(e) => {
                    ui.set_update_state(UPDATE_IDLE);
                    ui.set_update_error(true);
                    ui.set_update_message(trf!("Aggiornamento non riuscito: {e}.", "Update failed: {e}.").into());
                    set_status(&ui, &trf!("Aggiornamento non riuscito: {e}", "Update failed: {e}"), true);
                }
            });
        });
    }

    // ── Salvataggio ──────────────────────────────────────────────────────────

    fn save(&self) {
        let ui = self.ui();
        match self.write() {
            Ok(()) => {
                ui.set_dirty(false);
                set_status(&ui, tr!("Salvato ✓", "Saved ✓"), false);
            }
            Err(e) => set_status(&ui, &e, true),
        }
    }

    fn write(&self) -> Result<(), String> {
        let ui = self.ui();
        let hotkey = ui.get_hotkey().trim().to_string();
        keys::parse(&hotkey).map_err(|e| trf!("Scorciatoia non valida: {e}", "Invalid shortcut: {e}"))?;

        let mut config = self.config.borrow_mut();
        config.hotkey = hotkey;
        config.size = match ui.get_wheel_size() {
            1 => WheelSize::Large,
            2 => WheelSize::Huge,
            _ => WheelSize::Normal,
        };
        config.opacity = ui.get_wheel_opacity().round().clamp(f32::from(MIN_OPACITY), 100.0) as u8;
        config.accent = usize::try_from(ui.get_accent()).ok().and_then(|i| Accent::ALL.get(i)).copied().unwrap_or_default();
        config.aim_anywhere = ui.get_aim_anywhere();
        config.check_updates = ui.get_check_updates();
        config.language = language_choice(ui.get_language());
        for item in &config.actions {
            if let Action::Keys { keys } = &item.action {
                keys::parse(keys).map_err(|e| format!("{}: {e}", item.label))?;
            }
        }
        if let Some(wheel) = config.wheels.iter().find(|w| w.name.is_empty()) {
            return Err(trf!("La ruota \"{}\" non ha un nome", "The wheel \"{}\" has no name", wheel.id));
        }
        config.check()?;
        config::save(&config)?;
        drop(config);

        if ui.get_autostart() != autostart::is_enabled() {
            autostart::set(ui.get_autostart())?;
        }
        notify_background();
        Ok(())
    }
}

/// Aggiorna un modello con nuove righe. Se il numero non cambia, sostituisce solo le righe
/// diverse: così gli elenchi non perdono la posizione di scorrimento mentre si scrive.
fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
        return;
    }
    for (index, row) in rows.into_iter().enumerate() {
        if model.row_data(index).as_ref() != Some(&row) {
            model.set_row_data(index, row);
        }
    }
}

/// Le app con una finestra visibile, per scegliere a quali associare un profilo.
fn running_apps() -> Vec<PickItem> {
    // EnumWindows chiama `collect` per ogni finestra; lparam porta il puntatore al nostro Vec.
    extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = unsafe { &mut *(lparam.0 as *mut Vec<(String, String)>) };
        unsafe {
            let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
            let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid());
            // "Cloaked": finestre tecnicamente visibili ma nascoste da Windows (app sospese, altri desktop…).
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32);
            if IsWindowVisible(hwnd).as_bool() && !tool && !owned && cloaked == 0 {
                let mut title = [0u16; 256];
                let len = GetWindowTextW(hwnd, &mut title).max(0) as usize;
                if len > 0
                    && let Some(path) = actions::process_path(hwnd)
                {
                    let file = path.rsplit('\\').next().unwrap_or(&path).to_string();
                    found.push((file, String::from_utf16_lossy(&title[..len])));
                }
            }
        }
        TRUE // continua con la finestra successiva
    }

    let mut found: Vec<(String, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut _ as isize));
    }
    // Una voce per app (la prima finestra trovata), in ordine alfabetico. Esclusi Wheelor e i
    // processi di sistema che ospitano finestre di altri (non distinguono un'app dall'altra).
    const SKIP: [&str; 3] = ["wheelor.exe", "applicationframehost.exe", "textinputhost.exe"];
    found.sort_by_key(|(file, _)| file.to_lowercase());
    found.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    found
        .into_iter()
        .filter(|(file, _)| !SKIP.contains(&file.to_lowercase().as_str()))
        .map(|(file, title)| PickItem {
            id: file.as_str().into(),
            glyph: icons::glyph("E737").into(),
            label: file.into(),
            detail: title.into(),
        })
        .collect()
}

fn open_config_folder() {
    if let Some(dir) = package::visible_path(config::path()).parent() {
        let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
    }
}

/// La finestra nativa viene creata solo quando parte il ciclo degli eventi:
/// riproviamo ogni 20 ms finché non esiste (al massimo `attempts` volte).
fn set_icon_when_ready(ui: slint::Weak<SettingsWindow>, attempts: u32) {
    let done = ui.upgrade().is_some_and(|window| set_window_icon(&window));
    if !done && attempts > 0 {
        slint::Timer::single_shot(std::time::Duration::from_millis(20), move || {
            set_icon_when_ready(ui, attempts - 1)
        });
    }
}

/// La finestra nativa (HWND) dietro alla finestra di Slint, se esiste già.
fn window_handle(ui: &SettingsWindow) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let window = ui.window().window_handle();
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Mette l'icona di Wheelor sulla barra del titolo e sulla taskbar.
/// Restituisce false se la finestra nativa non esiste ancora.
fn set_window_icon(ui: &SettingsWindow) -> bool {
    let Some(hwnd) = window_handle(ui) else { return false };
    for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
        if let Some(icon) = crate::app_icon(metric) {
            unsafe {
                SendMessageW(hwnd, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(icon.0 as isize)));
            }
        }
    }
    true
}

fn set_status(ui: &SettingsWindow, text: &str, error: bool) {
    ui.set_status(text.into());
    ui.set_status_error(error);
}

fn set_hotkey_note(ui: &SettingsWindow, text: &str, error: bool) {
    ui.set_hotkey_note(text.into());
    ui.set_hotkey_note_error(error);
}

/// Colora la finestra con l'accento scelto (la ruota lo usa dopo il salvataggio).
fn apply_accent(ui: &SettingsWindow, accent: Accent) {
    let palette = accent.palette();
    let theme = ui.global::<Theme>();
    theme.set_accent(color(palette.accent));
    theme.set_accent_hover(color(palette.hover));
    theme.set_accent_press(color(palette.press));
    theme.set_on_accent(color(palette.ink));
    theme.set_star_light(color(palette.star_light));
    theme.set_star_dark(color(palette.star_dark));
}

/// La lingua scelta con la scelta a segmenti (0 automatica, 1 italiano, 2 inglese).
fn language_choice(index: i32) -> Option<Language> {
    match index {
        1 => Some(Language::Italian),
        2 => Some(Language::English),
        _ => None,
    }
}

/// I colori di accento con il loro nome, nella lingua attuale.
fn set_accents(ui: &SettingsWindow) {
    let accents: Vec<AccentChoice> =
        Accent::ALL.iter().map(|a| AccentChoice { name: a.name().into(), color: color(a.palette().accent) }).collect();
    ui.set_accents(ModelRc::new(VecModel::from(accents)));
}

/// Il testo sotto "Aggiornamenti": cosa c'è da installare, o come cercarlo.
fn update_message() -> String {
    match RELEASE.with_borrow(|r| r.is_some()) {
        true => {
            let current = update::current();
            trf!(
                "Ora hai la {current}. Si installa in pochi secondi, e ruote e azioni restano.",
                "You have {current}. It installs in a few seconds, and your wheels and actions stay."
            )
        }
        false => tr!(
            "Clic su «Controlla ora» per cercare una nuova versione.",
            "Click “Check now” to look for a new version."
        )
        .into(),
    }
}

fn color(hex: u32) -> slint::Color {
    slint::Color::from_rgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Mostra il risultato del controllo degli aggiornamenti.
fn show_update(ui: &SettingsWindow, result: Result<Option<Release>, String>, ask: bool) {
    ui.set_update_state(UPDATE_IDLE);
    match result {
        Ok(Some(release)) => {
            ui.set_update_version(release.version.as_str().into());
            ui.set_update_size(release.size_text().into());
            RELEASE.set(Some(release));
            ui.set_update_message(update_message().into());
            if ask {
                ui.set_update_state(UPDATE_CONFIRM);
            }
        }
        Ok(None) => {
            ui.set_update_version(SharedString::new());
            ui.set_update_message(tr!("Hai già l'ultima versione.", "You have the latest version.").into());
            RELEASE.set(None);
        }
        Err(e) => {
            ui.set_update_error(true);
            ui.set_update_message(trf!("Controllo non riuscito: {e}.", "Check failed: {e}.").into());
        }
    }
}

/// Apre un indirizzo web nel browser predefinito.
fn open_url(url: &str) {
    let url = windows::core::HSTRING::from(url);
    unsafe {
        ShellExecuteW(None, w!("open"), &url, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

fn set_icons(ui: &SettingsWindow, query: &str) {
    let found: Vec<IconData> = icons::search(query)
        .into_iter()
        .map(|icon| IconData { code: icon.code.into(), glyph: icons::glyph(icon.code).into(), name: icon.name.into() })
        .collect();
    ui.set_icons(ModelRc::new(VecModel::from(found)));
}

/// Avvisa Wheelor in background di ricaricare la configurazione (o lo avvia, se non è attivo).
fn notify_background() {
    unsafe {
        match FindWindowW(MAIN_CLASS, PCWSTR::null()) {
            Ok(hwnd) => {
                let _ = PostMessageW(Some(hwnd), WM_RELOAD_CONFIG, WPARAM(0), LPARAM(0));
            }
            Err(_) => {
                let _ = package::launch_self(&[]);
            }
        }
    }
}

/// Finestra standard di Windows per scegliere un file.
fn pick_file() -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog.SetTitle(w!("Scegli un'app o un file")).ok()?;
        dialog.Show(None).ok()?; // errore anche se l'utente annulla
        let path = dialog.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quanto costa disegnare la finestra: la ridisegna tutta più volte, come durante lo scorrimento,
    /// per ogni pagina, e stampa i millisecondi per fotogramma.
    /// `cargo test render_speed -- --ignored --nocapture` (aggiungere `--release` per la versione ottimizzata).
    #[test]
    #[ignore = "misura i tempi, non controlla nulla"]
    fn render_speed() {
        use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
        use slint::platform::{Platform, WindowAdapter, WindowEvent};

        // Una piattaforma finta: la finestra disegna in memoria invece che sullo schermo.
        struct Offscreen(Rc<MinimalSoftwareWindow>);
        impl Platform for Offscreen {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }

        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Offscreen(window.clone()))).unwrap();
        let (ui, _app) = build(Config::default()).unwrap();
        // 1120×780 al 125%, come la finestra vera su uno schermo comune.
        let (width, height) = (1400, 975);
        window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
        window.set_size(slint::PhysicalSize::new(width, height));
        ui.show().unwrap();
        let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];

        for (page, name) in [(0, "Ruote"), (1, "Libreria"), (2, "Generale")] {
            ui.set_page(page);
            ui.set_current_slot(if page == 0 { 1 } else { -1 });
            // Lascia finire le animazioni, poi misura solo il disegno.
            std::thread::sleep(std::time::Duration::from_millis(400));
            slint::platform::update_timers_and_animations();
            let frames = 20;
            let start = std::time::Instant::now();
            for _ in 0..frames {
                window.request_redraw();
                window.draw_if_needed(|renderer| {
                    renderer.render(&mut buffer, width as usize);
                });
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            println!("{name:>9}: {ms:.1} ms per fotogramma");
        }
    }

    /// Provini della finestra in vari stati, in target/settings-*.bmp, senza aprirla sullo schermo.
    /// `cargo test settings_snapshots -- --ignored --nocapture`
    #[test]
    #[ignore = "scrive immagini in target/"]
    fn settings_snapshots() {
        use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
        use slint::platform::{Platform, WindowAdapter};

        struct Offscreen(Rc<MinimalSoftwareWindow>);
        impl Platform for Offscreen {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }

        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Offscreen(window.clone()))).unwrap();
        // Con WHEELOR_LANG=en: in inglese (anche la configurazione di esempio).
        let english = std::env::var("WHEELOR_LANG").is_ok_and(|l| l == "en");
        i18n::set(Some(if english { Language::English } else { Language::Italian }));
        let (ui, app) = build(Config::default()).unwrap();
        connect(&ui, &app);
        i18n::apply_to_slint();
        ui.set_language(if english { 2 } else { 1 });
        // Con WHEELOR_STORE_SHOTS=1: solo le immagini per la scheda del Microsoft Store, a 1366×768.
        let store_shots = std::env::var_os("WHEELOR_STORE_SHOTS").is_some();
        window.set_size(if store_shots { slint::PhysicalSize::new(1366, 768) } else { slint::PhysicalSize::new(1120, 780) });
        ui.show().unwrap();

        let available = |ui: &SettingsWindow| {
            ui.set_update_version("0.2.0".into());
            ui.set_update_size("6,3 MB".into());
            ui.set_update_message("Ora hai la 0.1.0-beta.1. Si installa in pochi secondi, e ruote e azioni restano.".into());
        };
        type Shot<'a> = (&'static str, Box<dyn Fn(&SettingsWindow) + 'a>);
        let shots: Vec<Shot> = vec![
            ("generale", Box::new(|ui| ui.set_page(2))),
            (
                "generale-ascolto",
                Box::new(|ui| {
                    ui.set_accent(3);
                    apply_accent(ui, Accent::Pink);
                    ui.set_wheel_opacity(70.0);
                    ui.set_listening(LISTEN_HOTKEY);
                    ui.set_held_caps(caps_model("Ctrl+Alt"));
                }),
            ),
            (
                "generale-errore",
                Box::new(|ui| {
                    ui.set_listening(0);
                    set_hotkey_note(ui, &keys::hotkey_problem(&keys::parse("Shift+A").unwrap()).unwrap(), true);
                    ui.set_aim_anywhere(true);
                }),
            ),
            (
                "ruote-aggiornamento",
                Box::new(|ui| {
                    ui.set_accent(6);
                    apply_accent(ui, Accent::Lime);
                    available(ui);
                    ui.set_page(0);
                    ui.set_current_slot(1);
                }),
            ),
            ("informazioni", Box::new(|ui| ui.set_page(3))),
            (
                "informazioni-store",
                Box::new(|ui| {
                    ui.set_store(true);
                    ui.set_update_version(SharedString::new());
                }),
            ),
            (
                "generale-store",
                Box::new(|ui| {
                    ui.set_page(2);
                    ui.set_store(true);
                }),
            ),
            (
                "conferma",
                Box::new(|ui| {
                    ui.set_store(false);
                    available(ui);
                    ui.set_page(3);
                    ui.set_update_state(UPDATE_CONFIRM);
                }),
            ),
            (
                "download",
                Box::new(|ui| {
                    ui.set_update_state(UPDATE_DOWNLOADING);
                    ui.set_update_progress(0.42);
                }),
            ),
            (
                "tasti-azione",
                Box::new(|ui| {
                    ui.set_update_state(UPDATE_IDLE);
                    ui.set_accent(0);
                    apply_accent(ui, Accent::Aqua);
                    app.select_action("screenshot");
                    ui.set_page(1);
                }),
            ),
            // La finestra alla misura minima: niente deve uscire dai bordi.
            ("tasti-azione-stretta", Box::new(|_| window.set_size(slint::PhysicalSize::new(880, 600)))),
            ("generale-stretta", Box::new(|ui| ui.set_page(2))),
        ];

        let shots: Vec<Shot> = if !store_shots {
            shots
        } else {
            vec![
                (
                    "store-ruote",
                    Box::new(|ui| {
                        ui.global::<Fonts>().set_icons(icons::font_family().into());
                        ui.set_page(0);
                        ui.set_current_slot(1);
                    }),
                ),
                ("store-generale", Box::new(|ui| ui.set_page(2))),
                (
                    "store-libreria",
                    Box::new(|ui| {
                        app.select_action("screenshot");
                        ui.set_page(1);
                    }),
                ),
            ]
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        for (name, setup) in shots {
            setup(&ui);
            // Lascia finire le animazioni prima di disegnare.
            for _ in 0..4 {
                std::thread::sleep(std::time::Duration::from_millis(120));
                slint::platform::update_timers_and_animations();
            }
            let size = window.size();
            let (width, height) = (size.width, size.height);
            let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
            window.request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut buffer, width as usize);
            });
            // BMP a 32 bit, dall'alto verso il basso.
            let mut data = Vec::new();
            let size = 54 + buffer.len() as u32 * 4;
            data.extend_from_slice(b"BM");
            data.extend_from_slice(&size.to_le_bytes());
            data.extend_from_slice(&[0; 4]);
            data.extend_from_slice(&54u32.to_le_bytes());
            data.extend_from_slice(&40u32.to_le_bytes());
            data.extend_from_slice(&(width as i32).to_le_bytes());
            data.extend_from_slice(&(-(height as i32)).to_le_bytes());
            data.extend_from_slice(&1u16.to_le_bytes());
            data.extend_from_slice(&32u16.to_le_bytes());
            data.extend_from_slice(&[0; 24]);
            for pixel in &buffer {
                data.extend_from_slice(&[pixel.blue, pixel.green, pixel.red, 255]);
            }
            let language = if english { "en" } else { "it" };
            let path = dir.join(format!("settings-{name}-{language}.bmp"));
            std::fs::write(&path, data).unwrap();
            println!("{}", path.display());
        }
    }
}
