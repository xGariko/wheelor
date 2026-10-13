//! Scorciatoie globali: quella che apre la ruota (configurabile, es. "Ctrl+Alt+W") e quelle
//! delle azioni che ne hanno una (le eseguono senza aprire la ruota).

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::config::Item;
use crate::{keys, tr, trf};

/// Identificativo della hotkey della ruota.
pub const ID_WHEEL: i32 = 1;
/// Le scorciatoie delle azioni hanno gli id da qui in su, nell'ordine di `ACTIONS`.
pub const FIRST_ACTION: i32 = 100;

thread_local! {
    /// Id delle azioni con una scorciatoia registrata: la prima ha FIRST_ACTION, poi a salire.
    static ACTIONS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Testo della combinazione registrata, per non ri-registrarla se non cambia.
    static CURRENT: RefCell<String> = const { RefCell::new(String::new()) };
    /// Tasto principale (senza modificatori): serve a `input` per capire quando viene rilasciato.
    static KEY: Cell<VIRTUAL_KEY> = const { Cell::new(VK_W) };
}

pub fn key() -> VIRTUAL_KEY {
    KEY.get()
}

/// La combinazione registrata, così come è scritta nella configurazione.
pub fn current() -> String {
    CURRENT.with_borrow(Clone::clone)
}

/// Registra la combinazione. Se non è valida o è già presa da un'altra app,
/// resta attiva quella precedente.
pub fn register(hwnd: HWND, text: &str) -> Result<(), String> {
    if CURRENT.with_borrow(|current| current == text) {
        return Ok(());
    }
    let combo = keys::parse(text)?;
    let previous = CURRENT.take();
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), ID_WHEEL);
    }

    // MOD_NOREPEAT: tenendo premuto non arrivano messaggi ripetuti.
    let modifiers = combo.hotkey_modifiers() | MOD_NOREPEAT;
    if let Err(e) = unsafe { RegisterHotKey(Some(hwnd), ID_WHEEL, modifiers, combo.key.0 as u32) } {
        if !previous.is_empty() {
            let _ = register(hwnd, &previous);
        }
        return Err(trf!("{text} è già usata da un'altra app ({})", "{text} is already used by another app ({})", e.message()));
    }
    CURRENT.set(text.to_string());
    KEY.set(combo.key);
    Ok(())
}

/// La combinazione è libera? La registra per un attimo, senza finestra, e la libera subito.
/// Se è già presa (da un'altra app o da Windows) la registrazione fallisce.
pub fn is_free(combo: &keys::Combo) -> bool {
    const PROBE: i32 = 0xBFFF;
    unsafe {
        let modifiers = combo.hotkey_modifiers() | MOD_NOREPEAT;
        let free = RegisterHotKey(None, PROBE, modifiers, combo.key.0 as u32).is_ok();
        if free {
            let _ = UnregisterHotKey(None, PROBE);
        }
        free
    }
}

/// Toglie tutte le scorciatoie: quella della ruota e quelle delle azioni.
pub fn unregister(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), ID_WHEEL);
    }
    unregister_actions(hwnd);
}

/// Registra le scorciatoie delle azioni (togliendo prima quelle registrate finora).
/// Restituisce i problemi, uno per azione: le altre funzionano comunque.
pub fn register_actions(hwnd: HWND, actions: &[Item], wheel: &str) -> Vec<String> {
    unregister_actions(hwnd);
    let wheel = keys::parse(wheel).ok();
    let mut registered: Vec<String> = Vec::new();
    let mut problems = Vec::new();
    for item in actions.iter().filter(|a| !a.hotkey.trim().is_empty()) {
        let label = &item.label;
        let combo = match keys::parse(&item.hotkey) {
            Ok(combo) => combo,
            Err(e) => {
                problems.push(format!("{label}: {e}"));
                continue;
            }
        };
        if wheel.as_ref() == Some(&combo) {
            problems.push(trf!("{label}: è la scorciatoia della ruota", "{label}: it's the wheel's shortcut"));
            continue;
        }
        let id = FIRST_ACTION + registered.len() as i32;
        let modifiers = combo.hotkey_modifiers() | MOD_NOREPEAT;
        if unsafe { RegisterHotKey(Some(hwnd), id, modifiers, combo.key.0 as u32) }.is_err() {
            let shown = keys::caps(&item.hotkey).join("+");
            problems.push(trf!("{label}: {shown} è già usata", "{label}: {shown} is already in use"));
            continue;
        }
        registered.push(item.id.clone());
    }
    ACTIONS.set(registered);
    problems
}

/// L'azione della scorciatoia con questo id (da WM_HOTKEY).
pub fn action_for(id: i32) -> Option<String> {
    let index = usize::try_from(id - FIRST_ACTION).ok()?;
    ACTIONS.with_borrow(|actions| actions.get(index).cloned())
}

fn unregister_actions(hwnd: HWND) {
    let count = ACTIONS.take().len() as i32;
    for id in FIRST_ACTION..FIRST_ACTION + count {
        unsafe {
            let _ = UnregisterHotKey(Some(hwnd), id);
        }
    }
}

/// Il titolo della notifica quando qualche scorciatoia delle azioni non funziona.
pub fn problems_title() -> &'static str {
    tr!("Alcune scorciatoie non funzionano", "Some shortcuts don't work")
}
