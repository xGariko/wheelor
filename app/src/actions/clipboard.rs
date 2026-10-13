//! Gli appunti di Windows: leggere e scrivere testo, copiare la selezione dell'app in primo
//! piano e mettere da parte (poi rimettere) quello che c'era prima.

use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_C, VK_CONTROL, VK_V};
use windows::core::w;

use crate::keys::{self, Combo};
use crate::tr;

/// Quanto aspettare che l'app in primo piano risponda a Ctrl+C.
const COPY_TIMEOUT: Duration = Duration::from_millis(800);

/// Il contenuto degli appunti messo da parte, formato per formato.
pub struct Saved(Vec<(u32, Vec<u8>)>);

/// Apre gli appunti: un'altra app potrebbe tenerli aperti per un attimo, quindi riprova un po'.
struct Open;

impl Open {
    fn new() -> Result<Self, String> {
        for _ in 0..20 {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(Open);
            }
            thread::sleep(Duration::from_millis(15));
        }
        Err(tr!("Gli appunti sono occupati da un'altra app", "The clipboard is busy in another app").into())
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

/// Numero che cambia a ogni modifica degli appunti.
pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Il testo negli appunti (vuoto se non c'è testo).
pub fn text() -> String {
    let Ok(_open) = Open::new() else { return String::new() };
    unsafe {
        let Ok(handle) = GetClipboardData(CF_UNICODETEXT.0 as u32) else { return String::new() };
        let memory = HGLOBAL(handle.0);
        let pointer = GlobalLock(memory) as *const u16;
        if pointer.is_null() {
            return String::new();
        }
        // Il testo finisce con uno 0, ma non oltre la memoria allocata.
        let max = GlobalSize(memory) / 2;
        let len = (0..max).find(|&i| *pointer.add(i) == 0).unwrap_or(max);
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, len));
        let _ = GlobalUnlock(memory);
        text
    }
}

/// Mette `text` negli appunti, al posto di tutto ciò che c'era.
pub fn set_text(text: &str) -> Result<(), String> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    let bytes = unsafe { std::slice::from_raw_parts(units.as_ptr() as *const u8, units.len() * 2) };
    let _open = Open::new()?;
    unsafe {
        EmptyClipboard().map_err(|e| e.message())?;
    }
    put(CF_UNICODETEXT.0 as u32, bytes)
}

/// Copia `data` in memoria globale e la consegna agli appunti (già aperti), che ne diventano i proprietari.
fn put(format: u32, data: &[u8]) -> Result<(), String> {
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1)).map_err(|e| e.message())?;
        let pointer = GlobalLock(memory) as *mut u8;
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            return Err(tr!("Memoria esaurita", "Out of memory").into());
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), pointer, data.len());
        let _ = GlobalUnlock(memory);
        if let Err(e) = SetClipboardData(format, Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(e.message());
        }
        Ok(())
    }
}

/// Mette da parte tutto quello che c'è negli appunti. I formati che non sono semplice memoria
/// (bitmap e metafile come oggetti GDI) si saltano: Windows li ricava dagli altri (CF_DIB…).
pub fn save() -> Saved {
    const SKIP: [CLIPBOARD_FORMAT; 8] = [
        CF_BITMAP,
        CF_METAFILEPICT,
        CF_PALETTE,
        CF_ENHMETAFILE,
        CF_OWNERDISPLAY,
        CF_DSPBITMAP,
        CF_DSPMETAFILEPICT,
        CF_DSPENHMETAFILE,
    ];
    let mut saved = Vec::new();
    let Ok(_open) = Open::new() else { return Saved(saved) };
    unsafe {
        let mut format = EnumClipboardFormats(0);
        while format != 0 {
            if !SKIP.iter().any(|skip| u32::from(skip.0) == format)
                && let Ok(handle) = GetClipboardData(format)
            {
                let memory = HGLOBAL(handle.0);
                let size = GlobalSize(memory);
                let pointer = GlobalLock(memory) as *const u8;
                if !pointer.is_null() {
                    saved.push((format, std::slice::from_raw_parts(pointer, size).to_vec()));
                    let _ = GlobalUnlock(memory);
                }
            }
            format = EnumClipboardFormats(format);
        }
    }
    Saved(saved)
}

/// Rimette negli appunti quello che `save` aveva messo da parte. Il formato
/// "ExcludeClipboardContentFromMonitorProcessing" chiede alla cronologia degli appunti (Win+V)
/// di non registrarlo un'altra volta.
pub fn restore(saved: Saved) {
    let Ok(_open) = Open::new() else { return };
    unsafe {
        let _ = EmptyClipboard();
        for (format, data) in &saved.0 {
            let _ = put(*format, data);
        }
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        if exclude != 0 {
            let _ = put(exclude, &[0]);
        }
    }
}

/// Copia la selezione dell'app in primo piano (Ctrl+C) e aspetta che arrivi negli appunti.
/// Restituisce false se l'app non ha copiato niente (per esempio perché non c'era selezione).
pub fn copy_selection(cancelled: &dyn Fn() -> bool) -> bool {
    keys::wait_for_modifiers_release();
    let before = sequence();
    keys::send(&Combo { modifiers: vec![VK_CONTROL], key: VK_C });
    let start = Instant::now();
    while start.elapsed() < COPY_TIMEOUT && !cancelled() {
        if sequence() != before {
            // L'app potrebbe star ancora scrivendo gli altri formati: `Open` aspetta che finisca.
            thread::sleep(Duration::from_millis(30));
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Incolla (Ctrl+V) nell'app in primo piano.
pub fn paste() {
    keys::wait_for_modifiers_release();
    keys::send(&Combo { modifiers: vec![VK_CONTROL], key: VK_V });
}
