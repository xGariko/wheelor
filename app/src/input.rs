//! Cattura di mouse e tastiera mentre la ruota è aperta, tramite hook di basso livello
//! (WH_MOUSE_LL / WH_KEYBOARD_LL) installati solo per quel momento.
//!
//! Windows rimuove in silenzio gli hook troppo lenti: le callback si limitano a decidere
//! se "mangiare" l'evento e a inviare un messaggio alla finestra principale.

use std::cell::Cell;

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::Result;

use crate::hotkey;

// Messaggi inviati alla finestra principale mentre la ruota è aperta.
/// Il mouse si è mosso: wparam = x, lparam = y (pixel fisici).
pub const WM_WHEEL_MOVE: u32 = WM_APP + 2;
/// Clic sinistro.
pub const WM_WHEEL_CLICK: u32 = WM_APP + 3;
/// Esc: chiudere tutto senza fare nulla.
pub const WM_WHEEL_CANCEL: u32 = WM_APP + 4;
/// Tasto della hotkey rilasciato.
pub const WM_WHEEL_KEY_UP: u32 = WM_APP + 5;
/// Tasto della hotkey premuto di nuovo (non autorepeat).
pub const WM_WHEEL_KEY_DOWN: u32 = WM_APP + 6;
/// Tasto numerico 1-9: wparam = indice dello spicchio (0-8).
pub const WM_WHEEL_PICK: u32 = WM_APP + 7;
/// Backspace o clic con un tasto diverso dal sinistro: tornare alla ruota precedente.
pub const WM_WHEEL_BACK: u32 = WM_APP + 8;

/// Tasto virtuale non assegnato, usato per "mascherare" Alt (vedi `send_mask_key`).
const VK_MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

thread_local! {
    // Gli hook LL vengono chiamati sul thread che li ha installati, quindi bastano variabili per-thread.
    static TARGET: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    static MOUSE_HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
    static KEYBOARD_HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
    // Il tasto della hotkey è giù? Serve a distinguere una nuova pressione dall'autorepeat.
    static KEY_HELD: Cell<bool> = const { Cell::new(false) };
}

/// Inizia a catturare l'input; i messaggi andranno a `target`.
pub fn start(target: HWND) -> Result<()> {
    // Per sicurezza: hook rimasti installati mangerebbero il tasto della hotkey per sempre.
    stop();
    TARGET.set(target);
    // La ruota si apre proprio mentre il tasto della hotkey è premuto.
    KEY_HELD.set(true);
    send_mask_key();
    unsafe {
        let instance = Some(GetModuleHandleW(None)?.into());
        MOUSE_HOOK.set(Some(SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), instance, 0)?));
        KEYBOARD_HOOK.set(Some(SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), instance, 0)?));
    }
    Ok(())
}

pub fn stop() {
    unsafe {
        if let Some(hook) = MOUSE_HOOK.take() {
            let _ = UnhookWindowsHookEx(hook);
        }
        if let Some(hook) = KEYBOARD_HOOK.take() {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
}

/// Se Alt viene premuto e rilasciato senza altri tasti "visibili" in mezzo (la W l'ha presa
/// la hotkey), molte app attivano la barra dei menu. Un tasto fittizio in mezzo lo evita.
fn send_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VK_MASK, dwFlags: flags, ..Default::default() },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

fn post(msg: u32, wparam: usize, lparam: isize) {
    unsafe {
        let _ = PostMessageW(Some(TARGET.get()), msg, WPARAM(wparam), LPARAM(lparam));
    }
}

extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // Per WH_MOUSE_LL, lparam punta a una struttura con posizione e dettagli dell'evento.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        match wparam.0 as u32 {
            // Il movimento non lo mangiamo: il cursore deve muoversi.
            WM_MOUSEMOVE => post(WM_WHEEL_MOVE, info.pt.x as usize, info.pt.y as isize),
            // Pressioni: le mangiamo, così non arrivano all'app sotto la ruota.
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => return LRESULT(1),
            WM_LBUTTONUP => {
                post(WM_WHEEL_CLICK, 0, 0);
                return LRESULT(1);
            }
            WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
                post(WM_WHEEL_BACK, 0, 0);
                return LRESULT(1);
            }
            _ => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let vk = VIRTUAL_KEY(key.vkCode as u16);
        // Con Alt premuto i tasti arrivano come WM_SYSKEY*, quindi controlliamo entrambi.
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        if handle_key(vk, down) {
            return LRESULT(1); // tasto nostro: l'app sotto non lo vede
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Reagisce a un tasto e dice se va "mangiato". Ctrl, Alt e gli altri tasti passano
/// sempre: il sistema deve sapere quando vengono rilasciati.
fn handle_key(vk: VIRTUAL_KEY, down: bool) -> bool {
    if vk == VK_ESCAPE {
        if down {
            post(WM_WHEEL_CANCEL, 0, 0);
        }
        return true;
    }
    if vk == VK_BACK {
        if down {
            post(WM_WHEEL_BACK, 0, 0);
        }
        return true;
    }
    if vk == hotkey::key() {
        if down && !KEY_HELD.get() {
            post(WM_WHEEL_KEY_DOWN, 0, 0);
        }
        if !down {
            post(WM_WHEEL_KEY_UP, 0, 0);
        }
        KEY_HELD.set(down);
        return true;
    }
    if let Some(index) = digit_index(vk) {
        if down {
            post(WM_WHEEL_PICK, index, 0);
        }
        return true;
    }
    false
}

/// Tasti 1-9 (riga dei numeri o tastierino) → indice 0-8.
fn digit_index(vk: VIRTUAL_KEY) -> Option<usize> {
    match vk.0 {
        0x31..=0x39 => Some((vk.0 - 0x31) as usize),
        v if (VK_NUMPAD1.0..=VK_NUMPAD9.0).contains(&v) => Some((v - VK_NUMPAD1.0) as usize),
        _ => None,
    }
}
