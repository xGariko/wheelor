//! Scorciatoia globale che apre la ruota (configurabile, es. "Ctrl+Alt+W").

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::keys;

/// Identificativo della hotkey della ruota (in futuro potranno essercene altre).
pub const ID_WHEEL: i32 = 1;

thread_local! {
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
    unregister(hwnd);

    // MOD_NOREPEAT: tenendo premuto non arrivano messaggi ripetuti.
    let modifiers = combo.hotkey_modifiers() | MOD_NOREPEAT;
    if let Err(e) = unsafe { RegisterHotKey(Some(hwnd), ID_WHEEL, modifiers, combo.key.0 as u32) } {
        if !previous.is_empty() {
            let _ = register(hwnd, &previous);
        }
        return Err(format!("{text} è già usata da un'altra app ({})", e.message()));
    }
    CURRENT.set(text.to_string());
    KEY.set(combo.key);
    Ok(())
}

pub fn unregister(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), ID_WHEEL);
    }
}
