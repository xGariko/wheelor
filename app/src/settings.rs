//! Finestra delle impostazioni, in un processo separato (`wheelor.exe --settings`).
//! L'aspetto è descritto in ui/settings.slint; qui la colleghiamo ai dati.
//!
//! La configurazione in lavorazione vive in `App::config`: ogni modifica fatta nella finestra
//! la aggiorna subito, e "Salva" la scrive su disco. Le modifiche alla struttura (elementi delle
//! ruote, passi, azioni e ruote create o eliminate) si possono annullare (`App::change`).
//!
//! Qui c'è anche la piccola finestra dei segnaposto {ask:…} (`ask`, `wheelor.exe --ask`).

// Genera da ui/settings.slint i tipi Rust SettingsWindow, WheelRow, ActionRow, …
slint::include_modules!();

mod capture;
mod rows;
mod steps;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CLOAKED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

use crate::actions::{self, Context, Job};
use crate::config::{self, Accent, Config, Item, MAIN_WHEEL, MAX_SLOTS, MIN_OPACITY, Slot, Step, Wheel, WheelSize};
use crate::keys::Combo;
use crate::update::{self, Release};
use crate::i18n::{self, Language};
use crate::{MAIN_CLASS, WM_RELOAD_CONFIG, autostart, hotkey, icons, keys, package, tr, trf};
use capture::Event;
use rows::caps_model;

/// Il titolo della finestra nelle due lingue (deve coincidere con `title` in ui/settings.slint e la
/// sua traduzione): serve a ritrovarla se è già aperta, e lo usa anche l'installer.
const TITLES: [PCWSTR; 2] = [w!("Wheelor – Impostazioni"), w!("Wheelor – Settings")];

// Cosa sta scegliendo la finestra di scelta (proprietà `picker` in Slint).
const PICK_APP: i32 = 3;
const PICK_WAIT_APP: i32 = 4;
const PICK_RUN: i32 = 5;

// Per chi è aperto il selettore delle icone (proprietà `picking-icon`).
const ICON_FOR_ACTION: i32 = 1;
const ICON_FOR_WHEEL: i32 = 2;

// Cosa chiede di eliminare la finestra di conferma (proprietà `deleting`).
const DELETE_WHEEL: i32 = 1;
const DELETE_ACTION: i32 = 2;

// Quale campo sta registrando una combinazione di tasti (proprietà `listening`).
const LISTEN_HOTKEY: i32 = 1;
const LISTEN_KEYS: i32 = 2;
const LISTEN_ACTION_HOTKEY: i32 = 3;

// Da dove si crea una nuova azione (proprietà `creating`): dalla ruota si aggiunge anche lì, in
// fondo o al posto dell'elemento scelto.
const CREATE_FOR_WHEEL: i32 = 2;
const CREATE_REPLACING: i32 = 3;

/// La finestra ha una misura fissa: la più grande che sta nello schermo fino a questa (in pixel
/// logici), ma non meno della minima (vedi `fit_window`).
const WINDOW_SIZE: (f32, f32) = (1120.0, 780.0);
const WINDOW_MIN_SIZE: (f32, f32) = (880.0, 600.0);

// A che punto è l'aggiornamento (proprietà `update-state`).
const UPDATE_IDLE: i32 = 0;
const UPDATE_CHECKING: i32 = 1;
const UPDATE_CONFIRM: i32 = 2;
const UPDATE_DOWNLOADING: i32 = 3;
const UPDATE_INSTALLING: i32 = 4;

/// Quante modifiche si possono annullare, al massimo.
const UNDO_LIMIT: usize = 50;
/// Secondi prima che parta la prova di un'azione che lavora sull'app in primo piano: il tempo
/// di passare a quell'app.
const TEST_DELAY: u32 = 3;

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
    palette: Rc<VecModel<PaletteItem>>,
    steps: Rc<VecModel<StepRow>>,
    picker: Rc<VecModel<PickItem>>,
    /// Tutte le voci della finestra di scelta, prima del filtro di ricerca.
    picker_all: RefCell<Vec<PickItem>>,
    /// La configurazione com'era prima delle ultime modifiche alla struttura, con la loro
    /// descrizione: "Annulla" torna alla più recente.
    history: RefCell<Vec<(Config, String)>>,
    /// Le scorciatoie delle azioni come sono nel file: Wheelor in background le ha già registrate,
    /// quindi risultano occupate ma vanno bene.
    saved_hotkeys: RefCell<Vec<String>>,
}

/// Dove va un elemento nuovo nella ruota: in fondo, o al posto di quello in una posizione.
#[derive(Clone, Copy)]
enum Place {
    End,
    Replace(usize),
}

impl Place {
    /// Mette `slot` nella ruota e restituisce la sua posizione; None se va in fondo e non c'è posto.
    fn put(self, wheel: &mut Wheel, slot: Slot) -> Option<usize> {
        match self {
            Place::Replace(index) if index < wheel.slots.len() => {
                wheel.slots[index] = slot;
                Some(index)
            }
            _ if wheel.slots.len() < MAX_SLOTS => {
                wheel.slots.push(slot);
                Some(wheel.slots.len() - 1)
            }
            _ => None,
        }
    }
}

fn show() -> Result<(), slint::PlatformError> {
    // Skia disegnato sulla CPU, come la ruota: niente driver della GPU per una finestra che si apre ogni tanto.
    slint::BackendSelector::new().backend_name("winit".into()).renderer_name("skia-software".into()).select()?;
    // Se il file ha errori partiamo dai valori predefiniti, avvisando che il salvataggio lo sostituirà.
    i18n::set(None);
    let (config, load_error) = match config::load() {
        Ok(config) => (config, None),
        Err(e) => (Config::default(), Some(e)),
    };
    i18n::set(config.language);
    // La versione dello Store la aggiorna lo Store (vedi package::can_self_update).
    let check = config.check_updates && package::can_self_update();
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
    if check || (ask && package::can_self_update()) {
        app.check_for_updates(ask);
    }
    fit_window(&ui);
    set_icon_when_ready(ui.as_weak(), 50);
    ui.run()
}

/// La finestra non si ridimensiona: la sua misura è la più grande che sta nello schermo con il
/// cursore (da lì la si è aperta), fino a `WINDOW_SIZE` e non meno di `WINDOW_MIN_SIZE`. In quello
/// schermo la mettiamo al centro.
fn fit_window(ui: &SettingsWindow) {
    const MARGIN: f32 = 24.0;
    let (mut width, mut height) = WINDOW_SIZE;
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let (mut dpi, mut _y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let scale = dpi as f32 / 96.0;
            let work = info.rcWork;
            let (work_width, work_height) = ((work.right - work.left) as f32, (work.bottom - work.top) as f32);
            width = width.min(work_width / scale - 2.0 * MARGIN).max(WINDOW_MIN_SIZE.0);
            height = height.min(work_height / scale - 2.0 * MARGIN).max(WINDOW_MIN_SIZE.1);
            let x = work.left as f32 + ((work_width - width * scale) / 2.0).max(0.0);
            let y = work.top as f32 + ((work_height - height * scale) / 2.0).max(0.0);
            ui.window().set_position(slint::PhysicalPosition::new(x as i32, y as i32));
        }
    }
    ui.set_fixed_width(width);
    ui.set_fixed_height(height);
}

/// Crea la finestra e la riempie con `config`.
fn build(config: Config) -> Result<(SettingsWindow, Rc<App>), slint::PlatformError> {
    let ui = SettingsWindow::new()?;
    let saved_hotkeys = config.actions.iter().map(|a| a.hotkey.clone()).filter(|h| !h.is_empty()).collect();
    let app = Rc::new(App {
        ui: ui.as_weak(),
        config: RefCell::new(config),
        wheels: Rc::new(VecModel::default()),
        slots: Rc::new(VecModel::default()),
        actions: Rc::new(VecModel::default()),
        palette: Rc::new(VecModel::default()),
        steps: Rc::new(VecModel::default()),
        picker: Rc::new(VecModel::default()),
        picker_all: RefCell::new(Vec::new()),
        history: RefCell::new(Vec::new()),
        saved_hotkeys: RefCell::new(saved_hotkeys),
    });
    ui.set_wheels(ModelRc::from(app.wheels.clone()));
    ui.set_slots(ModelRc::from(app.slots.clone()));
    ui.set_actions(ModelRc::from(app.actions.clone()));
    ui.set_palette(ModelRc::from(app.palette.clone()));
    ui.set_steps(ModelRc::from(app.steps.clone()));
    ui.set_picker_items(ModelRc::from(app.picker.clone()));
    ui.global::<Fonts>().set_icons(icons::font_family().into());
    ui.set_max_slots(MAX_SLOTS as i32);
    ui.set_config_path(package::visible_path(config::path()).display().to_string().into());
    // Niente aggiornamenti dall'app nella versione dello Store, e nemmeno nella copia dell'installer
    // quando è installata anche quella dello Store.
    ui.set_store(!package::can_self_update());
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    steps::set_choices(&ui);
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
        apply_accent(&ui.global::<Theme>(), config.accent);
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

    // Ruote ed elementi.
    on!(on_select_wheel, |app, id| app.select_wheel(&id));
    on!(on_new_wheel, |app| app.new_wheel());
    on!(on_wheel_edited, |app| app.wheel_edited());
    on!(on_select_slot, |app, index| app.ui().set_current_slot(index));
    on!(on_add_slot, |app, id, submenu, index| app.add_slot(&id, submenu, index));
    on!(on_replace_slot, |app, id, submenu, index| app.replace_slot(&id, submenu, index));
    on!(on_move_slot, |app, from, to| app.move_slot(from, to));
    on!(on_remove_slot, |app, index| app.remove_slot(index));
    on!(on_edit_slot, |app, index| app.edit_slot(index));
    on!(on_palette_changed, |app| app.refresh_palette());
    on!(on_new_submenu, |app, replace| app.new_submenu(replace));
    on!(on_open_picker, |app, mode| app.open_picker(mode));
    on!(on_picker_search_changed, |app, text| app.filter_picker(&text));
    on!(on_picked, |app, id| app.picked(&id));

    // Libreria e passi.
    on!(on_select_action, |app, id| {
        app.ui().set_return_wheel(SharedString::new());
        app.select_action(&id);
    });
    on!(on_new_action, |app, kind, example| app.new_action(kind, example));
    on!(on_duplicate_action, |app| app.duplicate_action());
    on!(on_action_edited, |app| app.action_edited());
    on!(on_search_actions, |app, _text| app.refresh_actions());
    on!(on_return_to_wheel, |app| app.return_to_wheel());
    on!(on_select_step, |app, index| app.select_step(index));
    on!(on_add_step, |app, kind| app.add_step(kind));
    on!(on_step_kind_changed, |app| app.step_kind_changed());
    on!(on_move_step, |app, from, to| app.move_step(from, to));
    on!(on_duplicate_step, |app, index| app.duplicate_step(index));
    on!(on_remove_step, |app, index| app.remove_step(index));
    on!(on_clear_action_hotkey, |app| app.clear_action_hotkey());
    on!(on_browse, |app| app.browse());
    on!(on_test, |app| app.test());

    // Eliminazioni: prima la conferma, poi il lavoro vero. Annulla torna indietro.
    on!(on_ask_delete, |app, what| app.ask_delete(what));
    on!(on_confirm_delete, |app| app.confirm_delete());
    on!(on_undo, |app| app.undo());

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
    // La barra del titolo è disegnata dalla finestra: questi sono i suoi due pulsanti.
    on!(on_minimize_window, |app| app.ui().window().set_minimized(true));
    on!(on_close_window, |app| {
        if close_allowed(&app.ui()) {
            let _ = app.ui().hide();
        }
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
    ui.on_open_wheelor_site(|| open_url(tr!("https://wheelor.app/it/", "https://wheelor.app")));
    ui.on_open_author_site(|| open_url("https://gabrieleiovino.it"));

    // Alt+F4 e la chiusura dalla taskbar passano da qui.
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || match close_allowed(&weak.unwrap()) {
        true => CloseRequestResponse::HideWindow,
        false => CloseRequestResponse::KeepWindowShown,
    });
}

/// Chiudendo con modifiche non salvate, chiediamo conferma invece di chiudere.
fn close_allowed(ui: &SettingsWindow) -> bool {
    if ui.get_dirty() {
        ui.set_confirm_close(true);
        return false;
    }
    true
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

    // ── Annulla ──────────────────────────────────────────────────────────────

    /// Applica una modifica alla struttura che si potrà annullare; `f` restituisce false se non
    /// ha cambiato niente. `what` la descrive, nella barra in basso accanto ad "Annulla".
    fn change(&self, what: String, f: impl FnOnce(&mut Config) -> bool) -> bool {
        let before = self.config.borrow().clone();
        if !f(&mut self.config.borrow_mut()) {
            return false;
        }
        {
            let mut history = self.history.borrow_mut();
            history.push((before, what.clone()));
            if history.len() > UNDO_LIMIT {
                history.remove(0);
            }
        }
        self.mark_dirty();
        let ui = self.ui();
        set_status(&ui, &what, false);
        ui.set_undo_text(what.into());
        true
    }

    /// Le modifiche fatte scrivendo nei campi non si annullano da qui (i campi hanno il loro
    /// Ctrl+Z): dopo una di queste, tornare a una copia precedente le cancellerebbe, quindi la
    /// cronologia si svuota.
    fn forget_history(&self) {
        if !self.history.borrow().is_empty() {
            self.history.borrow_mut().clear();
            self.ui().set_undo_text(SharedString::new());
        }
    }

    fn undo(&self) {
        let Some((config, what)) = self.history.borrow_mut().pop() else { return };
        self.config.replace(config);
        let ui = self.ui();
        let previous = self.history.borrow().last().map(|(_, what)| what.clone()).unwrap_or_default();
        ui.set_undo_text(previous.into());
        self.refresh_all();
        ui.set_dirty(true);
        set_status(&ui, &trf!("Annullato: {what}", "Undone: {what}"), false);
    }

    /// Ridisegna tutto dopo che la configurazione è stata sostituita (Annulla), restando dove si era.
    fn refresh_all(&self) {
        let ui = self.ui();
        self.refresh_wheels();
        let wheel = self.current_wheel();
        if self.config.borrow().wheel(&wheel).is_some() {
            self.show_wheel(&wheel);
        } else {
            self.show_wheel(MAIN_WHEEL);
        }
        let slots = self.config.borrow().wheel(&self.current_wheel()).map_or(0, |w| w.slots.len()) as i32;
        if ui.get_current_slot() >= slots {
            ui.set_current_slot(-1);
        }
        self.refresh_slots();
        self.refresh_palette();
        self.refresh_actions();
        let action = ui.get_current_action().to_string();
        let exists = self.config.borrow().action(&action).is_some();
        let first = self.config.borrow().actions.first().map(|a| a.id.clone());
        match (exists, first) {
            (true, _) => self.load_action(&action, ui.get_current_step()),
            (false, Some(first)) => self.select_action(&first),
            (false, None) => ui.set_current_action(SharedString::new()),
        }
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

    /// Le voci della finestra «Aggiungi» della ruota: azioni o ruote, secondo la scheda scelta, con la ricerca.
    fn refresh_palette(&self) {
        let ui = self.ui();
        let config = self.config.borrow();
        let rows = config
            .wheel(&self.current_wheel())
            .map(|w| rows::palette(&config, w, ui.get_palette_tab() == 1, &ui.get_palette_search()))
            .unwrap_or_default();
        sync(&self.palette, rows);
    }

    fn select_wheel(&self, id: &str) {
        if self.config.borrow().wheel(id).is_none() {
            return;
        }
        let ui = self.ui();
        self.show_wheel(id);
        ui.set_current_slot(-1);
        ui.set_picking_icon(0);
        ui.set_page(0);
        self.refresh_slots();
        self.refresh_palette();
    }

    /// Mette nei campi della pagina la ruota `id`.
    fn show_wheel(&self, id: &str) {
        let ui = self.ui();
        {
            let config = self.config.borrow();
            let Some(wheel) = config.wheel(id) else { return };
            ui.set_current_wheel(id.into());
            ui.set_wh_name(wheel.name.as_str().into());
            ui.set_wh_icon(wheel.icon.as_str().into());
            ui.set_wh_apps(wheel.apps.join(", ").into());
            ui.set_wh_main(wheel.is_main());
        }
        self.refresh_wheel_hint();
    }

    /// La riga sotto il nome della ruota selezionata: quando si apre.
    fn refresh_wheel_hint(&self) {
        let ui = self.ui();
        let config = self.config.borrow();
        if let Some(wheel) = config.wheel(&self.current_wheel()) {
            ui.set_wh_hint(rows::wheel_hint(&config, wheel, &ui.get_hotkey()).into());
        }
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
        self.forget_history();
        self.refresh_wheel_hint();
        self.refresh_wheels();
        self.refresh_slots();
        self.refresh_palette();
        self.mark_dirty();
    }

    /// Aggiunge una ruota nuova (id in inglese, come tutto il file) e restituisce il suo id.
    fn add_wheel(config: &mut Config) -> String {
        let id = config::unique_id("wheel", |id| config.wheel(id).is_some());
        config.wheels.push(Wheel::new(&id, tr!("Nuova ruota", "New wheel")));
        id
    }

    fn new_wheel(&self) {
        let mut id = String::new();
        self.change(tr!("Creata una nuova ruota", "Created a new wheel").into(), |config| {
            id = Self::add_wheel(config);
            true
        });
        self.refresh_wheels();
        self.select_wheel(&id);
    }

    /// "Nuovo sottomenu": una ruota nuova, aggiunta come elemento della ruota selezionata (in
    /// fondo, o con `replace` al posto dell'elemento scelto).
    fn new_submenu(&self, replace: bool) {
        let wheel = self.current_wheel();
        let place = self.place(replace);
        let mut placed = None;
        self.change(tr!("Creato un nuovo sottomenu", "Created a new submenu").into(), |config| {
            let id = Self::add_wheel(config);
            if let Some(wheel) = config.wheels.iter_mut().find(|w| w.id == wheel) {
                placed = place.put(wheel, Slot::Wheel(id));
            }
            true
        });
        self.refresh_wheels();
        self.refresh_slots();
        self.refresh_palette();
        match placed {
            Some(index) => self.ui().set_current_slot(index as i32),
            None => set_status(
                &self.ui(),
                tr!("Creata la ruota, ma questa è piena: scegli una pallina e sostituiscila.", "Wheel created, but this one is full: choose a ball and replace it."),
                true,
            ),
        }
    }

    /// Dove va un elemento nuovo nella ruota selezionata: in fondo, o con `replace` al posto di
    /// quello scelto.
    fn place(&self, replace: bool) -> Place {
        match usize::try_from(self.ui().get_current_slot()) {
            Ok(index) if replace => Place::Replace(index),
            _ => Place::End,
        }
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
                    let wheels = rows::containing(&config, &Slot::Action(item.id.clone())).join(", ");
                    let mut text = if wheels.is_empty() {
                        tr!("Non è in nessuna ruota.", "It isn't in any wheel.").to_string()
                    } else {
                        trf!("Viene tolta anche dalle ruote: {wheels}.", "It's also removed from the wheels: {wheels}.")
                    };
                    let runners = rows::runners(&config, &item.id).join(", ");
                    if !runners.is_empty() {
                        text += &trf!(
                            " Spariscono i passi che la eseguono in: {runners}.",
                            " The steps that run it disappear from: {runners}."
                        );
                    }
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
        let name = self.config.borrow().wheel(&id).map(|w| w.name.clone()).unwrap_or_default();
        self.change(trf!("Eliminata la ruota «{name}»", "Deleted the wheel “{name}”"), |config| {
            config.wheels.retain(|w| w.id != id);
            // Toglie la ruota anche da dove era usata come sottomenu.
            let slot = Slot::Wheel(id.clone());
            for wheel in &mut config.wheels {
                wheel.slots.retain(|s| *s != slot);
            }
            true
        });
        self.refresh_wheels();
        self.select_wheel(MAIN_WHEEL);
    }

    /// Il nome di un elemento (azione o ruota), per i messaggi.
    fn slot_label(&self, slot: &Slot) -> String {
        let config = self.config.borrow();
        match slot {
            Slot::Action(id) => config.action(id).map(|a| a.label.clone()),
            Slot::Wheel(id) => config.wheel(id).map(|w| w.name.clone()),
        }
        .unwrap_or_default()
    }

    /// Dopo una modifica agli elementi della ruota: la ruota, i ruoli delle ruote, la finestra «Aggiungi».
    fn slots_changed(&self, selected: i32) {
        self.ui().set_current_slot(selected);
        self.refresh_slots();
        self.refresh_wheels(); // il ruolo delle ruote ("Sottomenu di …") può cambiare
        self.refresh_palette();
        self.refresh_wheel_hint();
    }

    /// Un sottomenu che porterebbe di nuovo a questa ruota creerebbe un ciclo.
    fn makes_cycle(&self, slot: &Slot) -> bool {
        let current = self.current_wheel();
        match slot {
            Slot::Wheel(id) => *id == current || self.config.borrow().reaches(id, &current),
            Slot::Action(_) => false,
        }
    }

    /// Aggiunge un'azione o un sottomenu alla ruota in posizione `index` (-1 = in fondo).
    fn add_slot(&self, id: &str, submenu: bool, index: i32) {
        let slot = if submenu { Slot::Wheel(id.into()) } else { Slot::Action(id.into()) };
        if self.makes_cycle(&slot) {
            return;
        }
        let full = self.with_current_wheel(|w| w.slots.len() >= MAX_SLOTS).unwrap_or(true);
        if full {
            let text = tr!(
                "La ruota è piena: scegli una pallina e sostituiscila.",
                "The wheel is full: choose a ball and replace it."
            );
            set_status(&self.ui(), text, true);
            return;
        }
        let label = self.slot_label(&slot);
        let wheel = self.current_wheel();
        let mut placed = 0;
        let done = self.change(trf!("Aggiunta «{label}» alla ruota", "Added “{label}” to the wheel"), |config| {
            let Some(wheel) = config.wheels.iter_mut().find(|w| w.id == wheel) else { return false };
            placed = usize::try_from(index).map_or(wheel.slots.len(), |i| i.min(wheel.slots.len()));
            wheel.slots.insert(placed, slot);
            true
        });
        if done {
            self.slots_changed(placed as i32);
        }
    }

    /// Ruota piena: l'elemento trascinato prende il posto di quello in posizione `index`.
    fn replace_slot(&self, id: &str, submenu: bool, index: i32) {
        let slot = if submenu { Slot::Wheel(id.into()) } else { Slot::Action(id.into()) };
        let Ok(index) = usize::try_from(index) else { return };
        if self.makes_cycle(&slot) {
            return;
        }
        let Some(old) = self.with_current_wheel(|w| w.slots.get(index).cloned()).flatten() else { return };
        let (old_label, new_label) = (self.slot_label(&old), self.slot_label(&slot));
        let wheel = self.current_wheel();
        let what = trf!("Sostituita «{old_label}» con «{new_label}»", "Replaced “{old_label}” with “{new_label}”");
        let done = self.change(what, |config| {
            let Some(wheel) = config.wheels.iter_mut().find(|w| w.id == wheel) else { return false };
            wheel.slots[index] = slot;
            true
        });
        if done {
            self.slots_changed(index as i32);
        }
    }

    fn move_slot(&self, from: i32, to: i32) {
        let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else { return };
        let Some(slot) = self.with_current_wheel(|w| w.slots.get(from).cloned()).flatten() else { return };
        let label = self.slot_label(&slot);
        let wheel = self.current_wheel();
        let done = self.change(trf!("Spostata «{label}»", "Moved “{label}”"), |config| {
            let Some(wheel) = config.wheels.iter_mut().find(|w| w.id == wheel) else { return false };
            if from == to || to >= wheel.slots.len() {
                return false;
            }
            let slot = wheel.slots.remove(from);
            wheel.slots.insert(to, slot);
            true
        });
        if done {
            self.slots_changed(to as i32);
        }
    }

    fn remove_slot(&self, index: i32) {
        let Ok(index) = usize::try_from(index) else { return };
        let Some(slot) = self.with_current_wheel(|w| w.slots.get(index).cloned()).flatten() else { return };
        let label = self.slot_label(&slot);
        let wheel = self.current_wheel();
        let done = self.change(trf!("Tolta «{label}» dalla ruota", "Removed “{label}” from the wheel"), |config| {
            let Some(wheel) = config.wheels.iter_mut().find(|w| w.id == wheel) else { return false };
            wheel.slots.remove(index);
            true
        });
        if done {
            self.slots_changed(-1);
        }
    }

    /// "Modifica": apre l'azione nella libreria (con il pulsante per tornare qui), o la ruota del sottomenu.
    fn edit_slot(&self, index: i32) {
        let ui = self.ui();
        let Ok(index) = usize::try_from(index) else { return };
        let slot = self.with_current_wheel(|wheel| wheel.slots.get(index).cloned()).flatten();
        match slot {
            Some(Slot::Action(id)) => {
                ui.set_return_wheel(ui.get_current_wheel());
                ui.set_return_wheel_name(ui.get_wh_name());
                ui.set_action_search(SharedString::new());
                self.refresh_actions();
                self.select_action(&id);
                ui.set_page(1);
            }
            Some(Slot::Wheel(id)) => self.select_wheel(&id),
            None => {}
        }
    }

    fn return_to_wheel(&self) {
        let ui = self.ui();
        let id = ui.get_return_wheel().to_string();
        ui.set_return_wheel(SharedString::new());
        self.select_wheel(&id);
    }

    // ── Finestra di scelta ───────────────────────────────────────────────────

    fn open_picker(&self, mode: i32) {
        let (title, items) = {
            let config = self.config.borrow();
            match mode {
                PICK_RUN => {
                    // Niente cicli: non l'azione stessa, né un'azione che torna a questa.
                    let current = self.ui().get_current_action().to_string();
                    let items = config
                        .actions
                        .iter()
                        .filter(|a| a.id != current && !config.runs_into(&a.id, &current))
                        .map(|a| rows::action_item(&config, a))
                        .collect();
                    (tr!("Scegli l'azione da eseguire", "Choose the action to run"), items)
                }
                PICK_WAIT_APP => (tr!("Scegli l'app da aspettare", "Choose the app to wait for"), running_apps()),
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
        match mode {
            PICK_APP => {
                let apps = ui.get_wh_apps();
                let already = apps.split([',', ';']).any(|a| a.trim().eq_ignore_ascii_case(id));
                if !already {
                    let base = apps.trim().trim_end_matches([',', ';']).trim_end();
                    let text = if base.is_empty() { id.to_string() } else { format!("{base}, {id}") };
                    ui.set_wh_apps(text.into());
                    self.wheel_edited();
                }
            }
            PICK_WAIT_APP => {
                ui.set_ed_app(id.into());
                self.action_edited();
            }
            PICK_RUN => {
                steps::set_run(&ui, &self.config.borrow(), id);
                self.action_edited();
            }
            _ => {}
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
            .map(|item| rows::action_row(&config, item))
            .filter(|row| {
                query.is_empty() || row.label.to_lowercase().contains(&query) || row.detail.to_lowercase().contains(&query)
            })
            .collect();
        sync(&self.actions, rows);
        self.refresh_action_index();
    }

    /// La riga dell'azione selezionata nell'elenco (filtrato), perché resti in vista.
    fn refresh_action_index(&self) {
        let ui = self.ui();
        let current = ui.get_current_action();
        ui.set_current_action_index(self.actions.iter().position(|row| row.id == current).map_or(-1, |i| i as i32));
    }

    fn select_action(&self, id: &str) {
        self.ui().set_picking_icon(0);
        self.load_action(id, 0);
    }

    /// Mette nell'editor l'azione `id`, con aperto il passo `step`.
    fn load_action(&self, id: &str, step: i32) {
        let ui = self.ui();
        {
            let config = self.config.borrow();
            let Some(item) = config.action(id) else { return };
            ui.set_current_action(id.into());
            ui.set_ed_label(item.label.as_str().into());
            ui.set_ed_icon(item.icon.as_str().into());
            ui.set_ed_hotkey(item.hotkey.as_str().into());
            ui.set_ed_hotkey_caps(caps_model(&item.hotkey));
            ui.set_ed_hotkey_note(SharedString::new());
            ui.set_ed_usage(rows::usage(&config, id).into());
        }
        self.refresh_action_index();
        self.refresh_steps();
        self.show_step(step);
    }

    /// Le righe dei passi dell'azione selezionata.
    fn refresh_steps(&self) {
        let config = self.config.borrow();
        let rows = config
            .action(&self.ui().get_current_action())
            .map(|item| item.steps.iter().map(|step| steps::row(&config, step)).collect())
            .unwrap_or_default();
        sync(&self.steps, rows);
    }

    /// Apre il passo `index` (o l'ultimo, se non c'è) e ne mette i campi nell'editor.
    fn show_step(&self, index: i32) {
        let ui = self.ui();
        if ui.get_listening() == LISTEN_KEYS {
            self.stop_listening();
        }
        let config = self.config.borrow();
        let Some(item) = config.action(&ui.get_current_action()) else { return };
        let index = index.clamp(0, item.steps.len().saturating_sub(1) as i32);
        ui.set_current_step(index);
        if let Some(step) = item.steps.get(index as usize) {
            steps::fill_editor(&ui, &config, step);
        }
    }

    fn select_step(&self, index: i32) {
        self.show_step(index);
    }

    /// Il nome dell'azione selezionata e l'elenco dei suoi passi sono cambiati: tutto ciò che li mostra.
    fn action_shown_changed(&self) {
        self.refresh_steps();
        self.refresh_actions();
        self.refresh_slots();
        self.refresh_palette();
        let ui = self.ui();
        ui.set_ed_usage(rows::usage(&self.config.borrow(), &ui.get_current_action()).into());
    }

    fn action_edited(&self) {
        let ui = self.ui();
        ui.set_ed_keys_caps(caps_model(&ui.get_ed_keys()));
        let id = ui.get_current_action().to_string();
        let index = ui.get_current_step().max(0) as usize;
        let step = steps::read_editor(&ui);
        {
            let mut config = self.config.borrow_mut();
            let Some(item) = config.actions.iter_mut().find(|a| a.id == id) else { return };
            item.label = ui.get_ed_label().to_string();
            item.icon = ui.get_ed_icon().to_string();
            item.hotkey = ui.get_ed_hotkey().to_string();
            if let Some(existing) = item.steps.get_mut(index) {
                *existing = step;
            }
        }
        self.forget_history();
        self.action_shown_changed();
        self.mark_dirty();
    }

    /// Modifica i passi dell'azione selezionata (annullabile) e poi apre il passo `select`.
    fn change_steps(&self, what: String, select: i32, f: impl FnOnce(&mut Vec<Step>) -> bool) {
        let id = self.ui().get_current_action().to_string();
        let done = self.change(what, |config| config.actions.iter_mut().find(|a| a.id == id).is_some_and(|item| f(&mut item.steps)));
        if done {
            self.action_shown_changed();
            self.show_step(select);
        }
    }

    fn add_step(&self, kind: i32) {
        let step = steps::new_step(kind);
        let at = self.ui().get_current_step() + 1;
        let name = step.name();
        self.change_steps(trf!("Aggiunto il passo «{name}»", "Added the step “{name}”"), at, |steps| {
            steps.insert((at.max(0) as usize).min(steps.len()), step);
            true
        });
    }

    /// Il passo aperto diventa di un altro tipo (`ed-kind`, appena scelto): si riparte da campi puliti.
    fn step_kind_changed(&self) {
        let ui = self.ui();
        let step = steps::new_step(ui.get_ed_kind());
        let index = ui.get_current_step();
        let name = step.name();
        self.change_steps(trf!("Il passo è diventato «{name}»", "The step became “{name}”"), index, |steps| {
            match steps.get_mut(index.max(0) as usize) {
                Some(existing) => *existing = step,
                None => return false,
            }
            true
        });
    }

    /// Sposta il passo `from` nel punto `gap` (0 = prima del primo, `len` = dopo l'ultimo).
    fn move_step(&self, from: i32, gap: i32) {
        let to = if gap > from { gap - 1 } else { gap };
        let count = self.steps.row_count() as i32;
        if from == to || from < 0 || to < 0 || from >= count || to >= count {
            return;
        }
        let name = self.steps.row_data(from as usize).map(|row| row.title).unwrap_or_default();
        self.change_steps(trf!("Spostato il passo «{name}»", "Moved the step “{name}”"), to, |steps| {
            let step = steps.remove(from as usize);
            steps.insert(to as usize, step);
            true
        });
    }

    fn duplicate_step(&self, index: i32) {
        let name = self.steps.row_data(index.max(0) as usize).map(|row| row.title).unwrap_or_default();
        self.change_steps(trf!("Duplicato il passo «{name}»", "Duplicated the step “{name}”"), index + 1, |steps| {
            let Some(step) = steps.get(index.max(0) as usize).cloned() else { return false };
            steps.insert(index as usize + 1, step);
            true
        });
    }

    fn remove_step(&self, index: i32) {
        let name = self.steps.row_data(index.max(0) as usize).map(|row| row.title).unwrap_or_default();
        self.change_steps(trf!("Eliminato il passo «{name}»", "Deleted the step “{name}”"), index.max(1) - 1, |steps| {
            // L'ultimo passo resta: un'azione fa sempre almeno una cosa.
            if steps.len() <= 1 || index < 0 || index as usize >= steps.len() {
                return false;
            }
            steps.remove(index as usize);
            true
        });
    }

    /// Aggiunge `item` alla libreria (con l'id `base`, o uno simile se è già usato) e, con
    /// `wheel`, anche a quella ruota, nel posto indicato. Restituisce l'id scelto.
    fn add_action(&self, mut item: Item, base: &str, wheel: Option<(&str, Place)>, what: String) -> String {
        let mut id = String::new();
        self.change(what, |config| {
            item.id = config::unique_id(base, |id| config.action(id).is_some());
            id = item.id.clone();
            config.actions.push(item);
            if let Some((wheel, place)) = wheel
                && let Some(wheel) = config.wheels.iter_mut().find(|w| w.id == wheel)
            {
                place.put(wheel, Slot::Action(id.clone()));
            }
            true
        });
        id
    }

    /// Apre `id` nell'editor della libreria, anche se la ricerca in corso non lo mostrerebbe.
    fn open_action(&self, id: &str) {
        let ui = self.ui();
        ui.set_action_search(SharedString::new());
        self.refresh_actions();
        self.refresh_wheels();
        self.refresh_slots();
        self.refresh_palette();
        self.select_action(id);
        ui.set_page(1);
    }

    /// "Nuova azione": con un passo del tipo `starter` (vedi `steps::starter_kind`) o da un esempio.
    fn new_action(&self, starter: i32, example: i32) {
        let ui = self.ui();
        let place = match ui.get_creating() {
            CREATE_FOR_WHEEL => Some(Place::End),
            CREATE_REPLACING => Some(self.place(true)),
            _ => None,
        };
        ui.set_creating(0);
        let (item, base) = match steps::example(example) {
            Some((item, id)) => (item, id),
            None => {
                let kind = steps::starter_kind(starter);
                (Item::new(tr!("Nuova azione", "New action"), steps::kind_icon(kind), vec![steps::new_step(kind)]), "action")
            }
        };
        let label = item.label.clone();
        let wheel = self.current_wheel();
        let what = trf!("Creata l'azione «{label}»", "Created the action “{label}”");
        let id = self.add_action(item, base, place.map(|place| (wheel.as_str(), place)), what);
        if place.is_some() {
            ui.set_return_wheel(ui.get_current_wheel());
            ui.set_return_wheel_name(ui.get_wh_name());
        } else {
            ui.set_return_wheel(SharedString::new());
        }
        self.open_action(&id);
    }

    fn duplicate_action(&self) {
        let id = self.ui().get_current_action().to_string();
        let Some(mut copy) = self.config.borrow().action(&id).cloned() else { return };
        copy.label = trf!("{} (copia)", "{} (copy)", copy.label);
        // Due azioni con la stessa scorciatoia non funzionerebbero: la copia non ce l'ha.
        copy.hotkey.clear();
        let label = copy.label.clone();
        let new = self.add_action(copy, &format!("{id}-copy"), None, trf!("Creata «{label}»", "Created “{label}”"));
        self.open_action(&new);
    }

    fn delete_action(&self) {
        let ui = self.ui();
        let id = ui.get_current_action().to_string();
        let label = self.config.borrow().action(&id).map(|a| a.label.clone()).unwrap_or_default();
        // Posizione nell'elenco visibile (che può essere filtrato dalla ricerca).
        let shown = self.actions.iter().position(|row| row.id == id.as_str()).unwrap_or(0);
        let done = self.change(trf!("Eliminata l'azione «{label}»", "Deleted the action “{label}”"), |config| {
            let Some(index) = config.actions.iter().position(|a| a.id == id) else { return false };
            config.actions.remove(index);
            // Toglie l'azione anche da tutte le ruote che la usavano e dai passi che la eseguivano.
            let slot = Slot::Action(id.clone());
            for wheel in &mut config.wheels {
                wheel.slots.retain(|s| *s != slot);
            }
            for item in &mut config.actions {
                item.steps.retain(|step| !matches!(step, Step::Run { action } if *action == id));
                if item.steps.is_empty() {
                    item.steps.push(steps::new_step(0));
                }
            }
            true
        });
        if !done {
            return;
        }
        self.refresh_actions();
        self.refresh_wheels();
        self.slots_changed(-1);
        // Seleziona la riga che ha preso il suo posto, o l'ultima se era in fondo.
        let count = self.actions.row_count();
        match self.actions.row_data(shown.min(count.saturating_sub(1))) {
            Some(row) => self.select_action(&row.id),
            None => ui.set_current_action(SharedString::new()),
        }
    }

    fn browse(&self) {
        if let Some(path) = pick_file() {
            self.ui().set_ed_target(path.into());
            self.action_edited();
        }
    }

    /// "Prova azione". Se lavora sull'app in primo piano (tasti, testo, selezione…) parte dopo
    /// qualche secondo: il tempo di passare a quell'app, altrimenti agirebbe su questa finestra.
    fn test(self: &Rc<Self>) {
        let ui = self.ui();
        let job = {
            let config = self.config.borrow();
            let Some(item) = config.action(&ui.get_current_action()) else { return };
            Job::new(item, &config.actions, Context::default())
        };
        if job.uses_foreground() {
            self.count_down(job, TEST_DELAY);
        } else {
            self.start_test(job);
        }
    }

    fn count_down(self: &Rc<Self>, job: Job, seconds: u32) {
        if seconds == 0 {
            self.start_test(job);
            return;
        }
        let text = trf!(
            "La prova parte tra {seconds}…: passa all'app su cui provarla.",
            "The test starts in {seconds}…: switch to the app to try it on."
        );
        set_status(&self.ui(), &text, false);
        let app = Rc::downgrade(self);
        slint::Timer::single_shot(Duration::from_secs(1), move || {
            if let Some(app) = app.upgrade() {
                app.count_down(job, seconds - 1);
            }
        });
    }

    fn start_test(&self, mut job: Job) {
        let ui = self.ui();
        // L'app in primo piano adesso: quella su cui si prova (o questa finestra).
        job.context = Context::capture();
        set_status(&ui, tr!("Azione avviata…", "Action started…"), false);
        // L'eventuale errore arriva da un altro thread: upgrade_in_event_loop riporta
        // l'aggiornamento sul thread dell'interfaccia, l'unico che può toccarla.
        let weak = self.ui.clone();
        actions::run(job, move |failure| {
            let text = format!("{} — {}", failure.title, failure.message);
            let _ = weak.upgrade_in_event_loop(move |ui| set_status(&ui, &text, true));
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

    /// Un campo "tasti" inizia ad ascoltare: la combinazione premuta, 2 secondi dopo che è
    /// stata rilasciata, va lì.
    fn listen(self: &Rc<Self>, target: i32) {
        let ui = self.ui();
        // Debole: lo stato della registrazione non deve tenere in vita `App`.
        let app = Rc::downgrade(self);
        let started = capture::start(move |event| {
            if let Some(app) = app.upgrade() {
                app.captured(target, event);
            }
        });
        match started {
            Ok(()) => {
                ui.set_pressed_caps(caps_model(""));
                ui.set_confirming(false);
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
            Event::Changed { modifiers, key, confirming } => {
                let text = match key {
                    Some(key) => Combo { modifiers, key }.text(),
                    None => keys::modifier_names(&modifiers).join("+"),
                };
                ui.set_pressed_caps(caps_model(&text));
                ui.set_confirming(confirming);
            }
            Event::Cancelled => ui.set_listening(0),
            Event::Done(combo) => {
                ui.set_listening(0);
                match target {
                    LISTEN_HOTKEY => self.hotkey_chosen(&combo),
                    LISTEN_KEYS => {
                        ui.set_ed_keys(combo.text().into());
                        self.action_edited();
                    }
                    LISTEN_ACTION_HOTKEY => self.action_hotkey_chosen(&combo),
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
        let shown = keys::caps(&text).join("+");
        if let Some(item) = self.config.borrow().actions.iter().find(|a| keys::parse(&a.hotkey).ok().as_ref() == Some(combo)) {
            let text = trf!("{shown} è già la scorciatoia di «{}».", "{shown} is already the shortcut of “{}”.", item.label);
            set_hotkey_note(&ui, &text, true);
            return;
        }
        // Quella salvata è registrata da Wheelor stesso: risulterebbe occupata, ma va bene.
        let saved = keys::parse(&self.config.borrow().hotkey).ok();
        if saved.as_ref() != Some(combo) && !hotkey::is_free(combo) {
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
            self.refresh_wheel_hint();
            self.mark_dirty();
            set_hotkey_note(&ui, tr!("È libera: premi Salva per usarla.", "It's free: press Save to use it."), false);
        } else {
            set_hotkey_note(&ui, "", false);
        }
    }

    /// La scorciatoia dell'azione selezionata: come quella della ruota, deve funzionare davvero.
    fn action_hotkey_chosen(&self, combo: &Combo) {
        let ui = self.ui();
        let note = |text: &str, error: bool| {
            ui.set_ed_hotkey_note(text.into());
            ui.set_ed_hotkey_note_error(error);
        };
        if let Some(problem) = keys::hotkey_problem(combo) {
            note(&problem, true);
            return;
        }
        let text = combo.text();
        let shown = keys::caps(&text).join("+");
        if keys::parse(&ui.get_hotkey()).ok().as_ref() == Some(combo) {
            note(&trf!("{shown} apre la ruota: scegline un'altra.", "{shown} opens the wheel: choose another one."), true);
            return;
        }
        let id = ui.get_current_action();
        let other = self
            .config
            .borrow()
            .actions
            .iter()
            .find(|a| a.id != id.as_str() && keys::parse(&a.hotkey).ok().as_ref() == Some(combo))
            .map(|a| a.label.clone());
        if let Some(other) = other {
            note(&trf!("{shown} è già la scorciatoia di «{other}».", "{shown} is already the shortcut of “{other}”."), true);
            return;
        }
        // Quelle salvate le ha registrate Wheelor stesso: risulterebbero occupate, ma vanno bene.
        let saved = self.saved_hotkeys.borrow().iter().any(|h| keys::parse(h).ok().as_ref() == Some(combo));
        if !saved && !hotkey::is_free(combo) {
            let text = trf!(
                "{shown} è già usata da Windows o da un'altra app: prova un'altra combinazione.",
                "{shown} is already used by Windows or another app: try another combination."
            );
            note(&text, true);
            return;
        }
        ui.set_ed_hotkey(text.as_str().into());
        ui.set_ed_hotkey_caps(caps_model(&text));
        self.action_edited();
        note(tr!("È libera: funziona dopo il salvataggio.", "It's free: it works once you save."), false);
    }

    fn clear_action_hotkey(&self) {
        let ui = self.ui();
        ui.set_ed_hotkey(SharedString::new());
        ui.set_ed_hotkey_caps(caps_model(""));
        ui.set_ed_hotkey_note(SharedString::new());
        self.action_edited();
    }

    // ── Aspetto ──────────────────────────────────────────────────────────────

    /// Il colore si vede subito in tutta la finestra; sulla ruota, dopo il salvataggio.
    fn accent_picked(&self, index: i32) {
        if let Some(&accent) = usize::try_from(index).ok().and_then(|i| Accent::ALL.get(i)) {
            apply_accent(&self.ui().global::<Theme>(), accent);
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
        self.refresh_palette();
        self.refresh_actions();
        self.refresh_steps();
        steps::set_choices(&ui);
        set_accents(&ui);
        ui.set_hotkey_caps(caps_model(&ui.get_hotkey()));
        ui.set_ed_keys_caps(caps_model(&ui.get_ed_keys()));
        ui.set_ed_hotkey_caps(caps_model(&ui.get_ed_hotkey()));
        set_hotkey_note(&ui, "", false);
        ui.set_ed_hotkey_note(SharedString::new());
        self.refresh_wheel_hint();
        ui.set_ed_usage(rows::usage(&self.config.borrow(), &ui.get_current_action()).into());
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
        let wheel_combo = keys::parse(&hotkey).map_err(|e| trf!("Scorciatoia non valida: {e}", "Invalid shortcut: {e}"))?;

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
        let mut hotkeys: Vec<Combo> = vec![wheel_combo];
        for item in &config.actions {
            let label = &item.label;
            for step in &item.steps {
                match step {
                    Step::Keys { keys } => {
                        keys::parse(keys).map_err(|e| format!("{label}: {e}"))?;
                    }
                    Step::Run { action } if action.is_empty() => {
                        return Err(trf!("{label}: scegli l'azione da eseguire", "{label}: choose the action to run"));
                    }
                    _ => {}
                }
            }
            if !item.hotkey.is_empty() {
                let combo = keys::parse(&item.hotkey).map_err(|e| format!("{label}: {e}"))?;
                if hotkeys.contains(&combo) {
                    let shown = keys::caps(&item.hotkey).join("+");
                    return Err(trf!("{label}: {shown} è già usata", "{label}: {shown} is already in use"));
                }
                hotkeys.push(combo);
            }
        }
        if let Some(wheel) = config.wheels.iter().find(|w| w.name.is_empty()) {
            return Err(trf!("La ruota \"{}\" non ha un nome", "The wheel \"{}\" has no name", wheel.id));
        }
        config.check()?;
        config::save(&config)?;
        self.saved_hotkeys.replace(config.actions.iter().map(|a| a.hotkey.clone()).filter(|h| !h.is_empty()).collect());
        drop(config);

        if ui.get_autostart() != autostart::is_enabled() {
            autostart::set(ui.get_autostart())?;
        }
        notify_background();
        Ok(())
    }
}

/// `wheelor.exe --ask <domanda>`: la piccola finestra dei segnaposto {ask:…}. Compare vicino al
/// cursore, sopra a tutto; la risposta va su stdout. Restituisce il codice d'uscita del processo:
/// 0 con una risposta, 1 se è stata annullata (e allora l'azione si ferma).
pub fn ask(question: &str) -> i32 {
    let selected = slint::BackendSelector::new().backend_name("winit".into()).renderer_name("skia-software".into()).select();
    if selected.is_err() {
        return 1;
    }
    let config = config::load().ok();
    i18n::set(config.as_ref().and_then(|c| c.language));
    let Ok(ui) = AskWindow::new() else { return 1 };
    i18n::apply_to_slint();
    ui.global::<Fonts>().set_icons(icons::font_family().into());
    apply_accent(&ui.global::<Theme>(), config.map(|c| c.accent).unwrap_or_default());
    ui.set_question(question.into());

    let answer: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        let answer = answer.clone();
        let weak = ui.as_weak();
        ui.on_accepted(move || {
            if let Some(ui) = weak.upgrade() {
                answer.replace(Some(ui.get_answer().to_string()));
                let _ = ui.hide();
            }
        });
    }
    let weak = ui.as_weak();
    ui.on_cancelled(move || {
        if let Some(ui) = weak.upgrade() {
            let _ = ui.hide();
        }
    });

    // Vicino al cursore, dentro lo schermo su cui si trova.
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_ok() {
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let (mut dpi, mut _y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                let scale = dpi as f32 / 96.0;
                let (width, height) = (440.0 * scale, 200.0 * scale);
                let work = info.rcWork;
                let x = (cursor.x as f32 - width / 2.0).clamp(work.left as f32 + 8.0, (work.right as f32 - width - 8.0).max(work.left as f32));
                let y = (cursor.y as f32 - 60.0 * scale).clamp(work.top as f32 + 8.0, (work.bottom as f32 - height - 8.0).max(work.top as f32));
                ui.window().set_position(slint::PhysicalPosition::new(x as i32, y as i32));
            }
        }
    }
    bring_to_front_when_ready(ui.as_weak(), 50);
    if ui.run().is_err() {
        return 1;
    }
    match answer.take() {
        Some(text) => {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
            0
        }
        None => 1,
    }
}

/// La finestra di `ask` si apre da un processo che non ha il primo piano: appena esiste, la
/// portiamo davanti (riprovando ogni 20 ms, al massimo `attempts` volte).
fn bring_to_front_when_ready(ui: slint::Weak<AskWindow>, attempts: u32) {
    let done = ui.upgrade().and_then(|window| native_window(window.window())).is_some_and(actions::window::bring_to_front);
    if !done && attempts > 0 {
        slint::Timer::single_shot(Duration::from_millis(20), move || bring_to_front_when_ready(ui, attempts - 1));
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
fn native_window(window: &slint::Window) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let window = window.window_handle();
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Mette l'icona di Wheelor sulla taskbar e, visto che la finestra non ha la cornice di Windows,
/// le chiede gli angoli arrotondati e il bordo sottile delle altre finestre di Windows 11 (Windows
/// 10 ignora la richiesta). Restituisce false se la finestra nativa non esiste ancora.
fn set_window_icon(ui: &SettingsWindow) -> bool {
    let Some(hwnd) = native_window(ui.window()) else { return false };
    for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
        if let Some(icon) = crate::app_icon(metric) {
            unsafe {
                SendMessageW(hwnd, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(icon.0 as isize)));
            }
        }
    }
    let corners = DWMWCP_ROUND;
    let border: u32 = 0x0035_3524; // 0x00BBGGRR: `Theme.line-strong` sullo sfondo
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corners as *const _ as *const _, size_of_val(&corners) as u32);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border as *const _ as *const _, size_of_val(&border) as u32);
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
fn apply_accent(theme: &Theme, accent: Accent) {
    let palette = accent.palette();
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

/// Il testo sotto "Aggiornamenti", se c'è una versione da installare.
fn update_message() -> String {
    match RELEASE.with_borrow(|r| r.is_some()) {
        true => {
            let current = update::current();
            trf!(
                "Ora hai la {current}. Si installa in pochi secondi, ruote e azioni restano.",
                "You have {current}. It installs in a few seconds, your wheels and actions stay."
            )
        }
        false => String::new(),
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
        // La finestra vera ha una misura fissa (vedi `fit_window`): qui si cambiano insieme.
        let resize = |ui: &SettingsWindow, width: u32, height: u32| {
            window.set_size(slint::PhysicalSize::new(width, height));
            ui.set_fixed_width(width as f32);
            ui.set_fixed_height(height as f32);
        };
        if store_shots {
            resize(&ui, 1366, 768);
        } else {
            resize(&ui, 1120, 780);
        }
        ui.show().unwrap();

        // La finestra di `ask`, finché non è stata disegnata.
        let ask_window: RefCell<Option<AskWindow>> = RefCell::new(None);
        // Il mouse simulato: `drag` preme sul primo punto e passa per gli altri, `release` rilascia.
        // Tra un evento e l'altro lascia scorrere i timer (i ritardi delle aree che scorrono).
        let pump = || {
            for _ in 0..3 {
                std::thread::sleep(std::time::Duration::from_millis(40));
                slint::platform::update_timers_and_animations();
            }
        };
        let pointer = |event| {
            window.dispatch_event(event);
            pump();
        };
        let drag = |points: &[(f32, f32)]| {
            use slint::platform::{PointerEventButton, WindowEvent};
            let (x, y) = points[0];
            pointer(WindowEvent::PointerMoved { position: slint::LogicalPosition::new(x, y) });
            pointer(WindowEvent::PointerPressed { position: slint::LogicalPosition::new(x, y), button: PointerEventButton::Left });
            for &(x, y) in &points[1..] {
                pointer(WindowEvent::PointerMoved { position: slint::LogicalPosition::new(x, y) });
            }
        };
        let release = |x: f32, y: f32| {
            use slint::platform::{PointerEventButton, WindowEvent};
            pointer(WindowEvent::PointerReleased { position: slint::LogicalPosition::new(x, y), button: PointerEventButton::Left });
        };
        let click = |x: f32, y: f32, button| {
            use slint::platform::WindowEvent;
            let position = slint::LogicalPosition::new(x, y);
            pointer(WindowEvent::PointerMoved { position });
            pointer(WindowEvent::PointerPressed { position, button });
            pointer(WindowEvent::PointerReleased { position, button });
        };
        use slint::platform::PointerEventButton::{Left, Right};

        let available = |ui: &SettingsWindow| {
            ui.set_update_version("0.2.0".into());
            ui.set_update_size("6,3 MB".into());
            ui.set_update_message("Ora hai la 0.1.0-beta.1. Si installa in pochi secondi, ruote e azioni restano.".into());
        };
        type Shot<'a> = (&'static str, Box<dyn Fn(&SettingsWindow) + 'a>);
        // Nella finestra alta 1100: il mozzo, due palline e il «+» della ruota «Media» (6 elementi e il «+»).
        const HUB: (f32, f32) = (496.0, 396.0);
        const BALL_1: (f32, f32) = (591.0, 320.0);
        const BALL_3: (f32, f32) = (549.0, 505.0);
        const PLUS: (f32, f32) = (401.0, 320.0);
        let shots: Vec<Shot> = vec![
            ("generale", Box::new(|ui| ui.set_page(2))),
            (
                "generale-ascolto",
                Box::new(|ui| {
                    ui.set_accent(3);
                    apply_accent(&ui.global::<Theme>(), Accent::Pink);
                    ui.set_wheel_opacity(70.0);
                    ui.set_listening(LISTEN_HOTKEY);
                    ui.set_pressed_caps(caps_model("Ctrl+Alt+W"));
                    ui.set_confirming(true);
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
                    apply_accent(&ui.global::<Theme>(), Accent::Lime);
                    available(ui);
                    ui.set_page(0);
                    ui.set_current_slot(1);
                }),
            ),
            (
                "ruote-media",
                Box::new(|ui| {
                    app.select_wheel("media");
                    ui.set_picking_icon(ICON_FOR_WHEEL);
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
                    apply_accent(&ui.global::<Theme>(), Accent::Aqua);
                    app.select_action("screenshot");
                    ui.set_page(1);
                }),
            ),
            // La finestra alla misura minima: niente deve uscire dai bordi.
            ("tasti-azione-stretta", Box::new(|ui| resize(ui, 880, 600))),
            ("generale-stretta", Box::new(|ui| ui.set_page(2))),
            ("ruote-media-stretta", Box::new(|_| app.select_wheel("media"))),
            (
                "libreria-nuova-stretta",
                Box::new(|ui| {
                    app.new_action(0, -1);
                    ui.set_page(1);
                }),
            ),
            // Alte, per vedere tutta la pagina senza scorrere: niente deve sovrapporsi.
            ("libreria-nuova", Box::new(|ui| resize(ui, 1120, 1100))),
            (
                "ruote-media-intera",
                Box::new(|ui| {
                    app.select_wheel("media");
                    ui.set_page(0);
                }),
            ),
            // L'editor della ruota con il mouse simulato: una pallina scelta, il suo menu, una
            // pallina trascinata fuori dal disco (disegnata prima di rilasciare), la finestra «Aggiungi».
            ("ruote-scelta", Box::new(|_| click(BALL_1.0, BALL_1.1, Left))),
            ("ruote-menu", Box::new(|_| click(BALL_1.0, BALL_1.1, Right))),
            (
                "ruote-trascina-fuori",
                Box::new(|_| {
                    click(HUB.0, HUB.1, Left); // chiude il menu
                    click(HUB.0, HUB.1, Left);
                    drag(&[BALL_3, (BALL_3.0, BALL_3.1 + 30.0), (BALL_3.0 + 10.0, BALL_3.1 + 90.0), (BALL_3.0 + 20.0, BALL_3.1 + 130.0)]);
                }),
            ),
            (
                "ruote-aggiungi",
                Box::new(|ui| {
                    release(BALL_3.0 + 20.0, BALL_3.1 + 130.0);
                    click(PLUS.0, PLUS.1, Left);
                    let _ = ui;
                }),
            ),
            (
                "ruote-sostituisci",
                Box::new(|ui| {
                    ui.set_adding(0);
                    ui.set_current_slot(0);
                    ui.invoke_open_adding(2, 1);
                }),
            ),
            (
                "passi-trascina",
                Box::new(|ui| {
                    ui.set_adding(0);
                    app.select_action("paste-plain");
                    ui.set_page(1);
                    pump();
                    drag(&[(760.0, 180.0), (760.0, 200.0), (760.0, 260.0), (760.0, 296.0)]);
                }),
            ),
            ("passi-spostati", Box::new(|_| release(760.0, 296.0))),
            ("aggiungi-passo", Box::new(|ui| ui.set_picking_kind(1))),
            (
                "nuova-azione",
                Box::new(|ui| {
                    ui.set_picking_kind(0);
                    ui.set_creating(1);
                }),
            ),
            (
                "esempio-due-app",
                Box::new(|ui| {
                    ui.set_creating(1);
                    app.new_action(-1, 6);
                    app.select_step(2);
                }),
            ),
            ("esempio-due-app-stretta", Box::new(|ui| resize(ui, 880, 600))),
            // Per ultima, la piccola finestra dei segnaposto {ask:…} (al posto delle impostazioni).
            (
                "domanda",
                Box::new(|ui| {
                    let _ = ui.hide();
                    let ask = AskWindow::new().unwrap();
                    ask.set_question("Cosa cerchi su YouTube?".into());
                    ask.set_answer("tutorial slint".into());
                    window.set_size(slint::PhysicalSize::new(440, 200));
                    ask.show().unwrap();
                    ask_window.replace(Some(ask));
                }),
            ),
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
                    "store-nuova-azione",
                    Box::new(|ui| {
                        app.select_action("paste-plain");
                        ui.set_page(1);
                        ui.set_creating(1);
                    }),
                ),
                // Per ultima (lascia una modifica da salvare): un'azione a più passi, l'esempio
                // «Due app affiancate», con il terzo passo aperto.
                (
                    "store-passi",
                    Box::new(|_| {
                        app.new_action(-1, 6);
                        app.select_step(2);
                    }),
                ),
            ]
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        for (name, setup) in shots {
            setup(&ui);
            // Lascia finire le animazioni prima di disegnare. Un primo disegno a metà, come i
            // fotogrammi della finestra vera: qualcosa (gli elenchi) si aggiorna solo disegnando.
            let size = window.size();
            let (width, height) = (size.width, size.height);
            let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
            for frame in 0..4 {
                std::thread::sleep(std::time::Duration::from_millis(120));
                slint::platform::update_timers_and_animations();
                if frame == 1 {
                    window.request_redraw();
                    window.draw_if_needed(|renderer| {
                        renderer.render(&mut buffer, width as usize);
                    });
                }
            }
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
