//! Logica della ruota in modalità ibrida:
//! - pressione rapida della hotkey: la ruota resta aperta e si sceglie con un clic;
//! - hotkey tenuta premuta: si punta uno spicchio e al rilascio parte l'azione.
//!
//! Uno spicchio può aprire un'altra ruota (sottomenu). Le ruote aperte formano una pila:
//! si scende scegliendo un sottomenu e si risale con il mozzo, il tasto destro o Backspace.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, KillTimer, SetTimer};

use crate::actions::{self, Context, Job};
use crate::config::{Config, Item, Slot};
use crate::overlay::{self, Entrance, Hub, SliceView, Style};
use crate::{icons, input, tr, tray, trf};

/// Id del timer che apre un sottomenu quando ci si ferma sopra tenendo premuta la hotkey.
pub const TIMER_DWELL: usize = 1;
/// Quanto bisogna restare fermi su un sottomenu perché si apra (millisecondi).
const DWELL_MS: u32 = 350;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Closed,
    /// Hotkey ancora premuta: al rilascio si esegue lo spicchio puntato.
    Holding,
    /// Hotkey già rilasciata: si sceglie con un clic.
    Clicking,
}

/// Cosa succede scegliendo uno spicchio della ruota visibile.
#[derive(Clone)]
enum Target {
    Run(Item),
    /// Apre la ruota con questo id.
    Submenu(String),
}

thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Closed) };
    static CONFIG: RefCell<Option<Config>> = const { RefCell::new(None) };
    /// Finestra che era in primo piano quando si è aperta la ruota.
    static CONTEXT: RefCell<Context> = RefCell::new(Context::default());
    /// Id delle ruote aperte: la prima è quella di partenza, l'ultima quella visibile.
    static STACK: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Cosa fa ogni spicchio della ruota visibile, nello stesso ordine.
    static TARGETS: RefCell<Vec<Target>> = const { RefCell::new(Vec::new()) };
    /// Sottomenu su cui il cursore si è fermato (timer in corso).
    static DWELL: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Imposta la configurazione da usare alla prossima apertura.
pub fn set_config(config: Config) {
    overlay::set_style(Style {
        zoom: config.size.zoom(),
        opacity: f32::from(config.opacity) / 100.0,
        palette: config.accent.palette(),
        aim_anywhere: config.aim_anywhere,
    });
    CONFIG.set(Some(config));
}

pub fn is_open() -> bool {
    MODE.get() != Mode::Closed
}

pub fn open(hwnd: HWND) -> Result<(), String> {
    let mut context = Context::capture();
    // Il profilo dell'app in primo piano, se c'è, altrimenti la ruota principale.
    let root = CONFIG.with_borrow(|config| {
        let wheel = config.as_ref()?.wheel_for_app(&context.app);
        Some((wheel.id.clone(), wheel.name.clone(), wheel.slots.is_empty()))
    });
    let Some((id, name, empty)) = root else { return Err(tr!("Configurazione non caricata", "Configuration not loaded").into()) };
    if empty {
        return Err(trf!(
            "La ruota \"{name}\" è vuota: aggiungi qualche azione dalle impostazioni.",
            "The wheel \"{name}\" is empty: add some actions in the settings."
        ));
    }

    context.profile = name;
    CONTEXT.set(context);
    STACK.set(vec![id]);
    show_top(Entrance::Open)?;
    input::start(hwnd).map_err(|e| e.message())?;
    MODE.set(Mode::Holding);
    Ok(())
}

/// Chiude la ruota senza fare nulla: sfuma in un attimo.
pub fn close() {
    stop();
    overlay::dismiss();
}

/// Smette di catturare l'input e dimentica le ruote aperte (la ruota resta da nascondere).
fn stop() {
    input::stop();
    MODE.set(Mode::Closed);
    STACK.take();
    DWELL.set(None);
}

/// Gestisce i messaggi che arrivano da `input` mentre la ruota è aperta.
pub fn on_input(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) {
    // Possono arrivare messaggi rimasti in coda dopo la chiusura: li ignoriamo.
    if !is_open() {
        return;
    }
    match msg {
        input::WM_WHEEL_MOVE => {
            overlay::hover_at(POINT { x: wparam.0 as i32, y: lparam.0 as i32 });
            if MODE.get() == Mode::Holding {
                update_dwell(hwnd);
            }
        }
        input::WM_WHEEL_CLICK => match overlay::hovered() {
            Some(index) => choose(hwnd, index),
            // Fuori dalla ruota (se non si punta da tutto lo schermo): chiude tutto.
            None if overlay::outside() => close(),
            None => back(hwnd), // clic sul mozzo
        },
        // Un clic fuori dalla ruota, con qualunque tasto del mouse, la chiude; Backspace torna indietro.
        input::WM_WHEEL_BACK if wparam.0 == 1 && overlay::outside() => close(),
        input::WM_WHEEL_BACK => back(hwnd),
        // Spazio: come il clic, ma nel mozzo non fa nulla (niente "indietro" per sbaglio).
        input::WM_WHEEL_CONFIRM => {
            if let Some(index) = overlay::hovered() {
                choose(hwnd, index);
            }
        }
        input::WM_WHEEL_CANCEL => close(),
        input::WM_WHEEL_KEY_UP if MODE.get() == Mode::Holding => {
            // Da qui in poi si sceglie con il clic, anche se il rilascio apre un sottomenu.
            MODE.set(Mode::Clicking);
            if let Some(index) = overlay::hovered() {
                choose(hwnd, index);
            }
        }
        input::WM_WHEEL_KEY_DOWN if MODE.get() == Mode::Clicking => close(),
        input::WM_WHEEL_PICK => choose(hwnd, wparam.0),
        _ => {}
    }
}

/// Il timer della pausa è scaduto: se il cursore è ancora sullo stesso sottomenu, lo apre.
pub fn on_timer(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_DWELL);
    }
    let Some(index) = DWELL.take() else { return };
    if MODE.get() == Mode::Holding && overlay::hovered() == Some(index) {
        choose(hwnd, index);
    }
}

fn choose(hwnd: HWND, index: usize) {
    let Some(target) = TARGETS.with_borrow(|targets| targets.get(index).cloned()) else { return };
    match target {
        Target::Submenu(id) => {
            STACK.with_borrow_mut(|stack| stack.push(id));
            refresh(hwnd, Entrance::Open);
        }
        Target::Run(item) => execute(hwnd, &item, CONTEXT.take()),
    }
}

/// Torna alla ruota precedente; dalla prima, chiude.
fn back(hwnd: HWND) {
    let popped = STACK.with_borrow_mut(|stack| stack.len() > 1 && stack.pop().is_some());
    if popped {
        refresh(hwnd, Entrance::Back);
    } else {
        close();
    }
}

/// Mostra la ruota in cima alla pila, centrata dove si trova ora il cursore.
fn refresh(hwnd: HWND, entrance: Entrance) {
    if let Err(e) = show_top(entrance) {
        close();
        tray::notify(hwnd, tr!("Impossibile aprire la ruota", "Can't open the wheel"), &e);
    }
}

fn show_top(entrance: Entrance) -> Result<(), String> {
    let depth = STACK.with_borrow(Vec::len);
    let Some(id) = STACK.with_borrow(|stack| stack.last().cloned()) else { return Ok(()) };

    let (views, targets, hub) = CONFIG.with_borrow(|config| {
        let config = config.as_ref().ok_or(tr!("Configurazione non caricata", "Configuration not loaded"))?;
        let wheel =
            config.wheel(&id).ok_or_else(|| trf!("La ruota \"{id}\" non esiste", "The wheel \"{id}\" doesn't exist"))?;
        let mut views = Vec::new();
        let mut targets = Vec::new();
        for slot in &wheel.slots {
            // I riferimenti sono già stati controllati da `Config::check`; nel dubbio si salta.
            match slot {
                Slot::Action(id) => {
                    let Some(item) = config.action(id) else { continue };
                    views.push(SliceView { icon: icons::glyph(&item.icon), label: item.label.clone(), submenu: false });
                    targets.push(Target::Run(item.clone()));
                }
                Slot::Wheel(id) => {
                    let Some(sub) = config.wheel(id) else { continue };
                    let icon = icons::glyph(sub.icon_or_default());
                    views.push(SliceView { icon, label: sub.name.clone(), submenu: true });
                    targets.push(Target::Submenu(id.clone()));
                }
            }
        }
        let hub = if depth > 1 {
            Hub::Back(wheel.name.clone())
        } else if wheel.is_main() {
            Hub::Logo
        } else {
            Hub::Title(wheel.name.clone())
        };
        Ok::<_, String>((views, targets, hub))
    })?;

    overlay::set_wheel(views, hub);
    TARGETS.set(targets);
    DWELL.set(None);

    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.map_err(|e| e.message())?;
    overlay::show(cursor, entrance).map_err(|e| e.message())
}

/// Tenendo premuta la hotkey, fermarsi su un sottomenu lo apre dopo DWELL_MS:
/// così si arriva a qualunque azione con un solo gesto.
fn update_dwell(hwnd: HWND) {
    let is_submenu = |index: usize| TARGETS.with_borrow(|t| matches!(t.get(index), Some(Target::Submenu(_))));
    let hovered = overlay::hovered().filter(|&index| is_submenu(index));
    let previous = DWELL.replace(hovered);
    unsafe {
        if hovered.is_some() {
            // A ogni movimento il timer riparte da zero (stesso id = sostituisce il precedente):
            // conta solo una pausa vera, non l'attraversare lo spicchio.
            SetTimer(Some(hwnd), TIMER_DWELL, DWELL_MS, None);
        } else if previous.is_some() {
            let _ = KillTimer(Some(hwnd), TIMER_DWELL);
        }
    }
}

fn execute(hwnd: HWND, item: &Item, context: Context) {
    // Niente dissolvenza: l'azione non deve trovarsi la ruota davanti (per esempio uno screenshot).
    stop();
    overlay::hide();
    let library = CONFIG.with_borrow(|config| config.as_ref().map(|c| c.actions.clone()).unwrap_or_default());
    // Un HWND contiene un puntatore e Rust non lo lascia passare tra thread:
    // ne passiamo il valore numerico e lo ricostruiamo dall'altra parte.
    let target = hwnd.0 as usize;
    actions::run(Job::new(item, &library, context), move |failure| {
        tray::notify(HWND(target as *mut _), &failure.title, &failure.message);
    });
}

/// Esegue l'azione `id` della libreria senza passare dalla ruota: la sua scorciatoia o
/// `wheelor.exe --run <id>`. Se la ruota è aperta, si chiude.
pub fn run_action(hwnd: HWND, id: &str) {
    if is_open() {
        close();
    }
    let found = CONFIG.with_borrow(|config| {
        let config = config.as_ref()?;
        let item = config.action(id)?.clone();
        let mut context = Context::capture();
        context.profile = config.wheel_for_app(&context.app).name.clone();
        Some((item, context))
    });
    match found {
        Some((item, context)) => execute(hwnd, &item, context),
        None => tray::notify(
            hwnd,
            tr!("Azione non trovata", "Action not found"),
            &trf!("Nella libreria non c'è un'azione con id \"{id}\".", "There's no action with the id \"{id}\" in the library."),
        ),
    }
}
