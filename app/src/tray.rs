//! Icona nella system tray e relativo menu contestuale.

use std::sync::OnceLock;

use windows::Win32::Foundation::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

/// Messaggio che Windows ci invia quando l'utente interagisce con l'icona.
pub const WM_TRAY: u32 = WM_APP + 1;

const ICON_ID: u32 = 1;
const CMD_SETTINGS: usize = 1;
const CMD_EXIT: usize = 2;

/// Cosa ha chiesto l'utente tramite l'icona.
pub enum Command {
    Settings,
    Exit,
}

pub fn add(hwnd: HWND) -> Result<()> {
    let mut data = base_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = match crate::app_icon(SM_CXSMICON) {
        Some(icon) => icon,
        None => unsafe { LoadIconW(None, IDI_APPLICATION)? },
    };
    copy_wide(&mut data.szTip, "Wheelor");

    unsafe {
        // Se un tentativo precedente era andato a buon fine pur segnalando un errore (succede
        // all'accesso a Windows, quando Explorer è lento), l'icona esiste già: basta aggiornarla.
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            Shell_NotifyIconW(NIM_MODIFY, &data).ok()?;
        }
        // Versione 4: comportamento moderno degli eventi (WM_CONTEXTMENU, coordinate in wparam).
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &data).ok()?;
    }
    Ok(())
}

pub fn remove(hwnd: HWND) {
    let data = base_data(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// Mostra una notifica di Windows associata all'icona, senza suono.
pub fn notify(hwnd: HWND, title: &str, text: &str) {
    let mut data = base_data(hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO | NIIF_NOSOUND;
    copy_wide(&mut data.szInfoTitle, title);
    copy_wide(&mut data.szInfo, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

pub fn on_message(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> Option<Command> {
    // Con NOTIFYICON_VERSION_4: evento nei 16 bit bassi di lparam,
    // coordinate del punto di ancoraggio in wparam (x bassa, y alta).
    match (lparam.0 & 0xFFFF) as u32 {
        WM_CONTEXTMENU => {
            let x = (wparam.0 & 0xFFFF) as i16 as i32;
            let y = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
            show_menu(hwnd, x, y).ok().flatten()
        }
        // Clic sinistro sull'icona: apre direttamente le impostazioni.
        NIN_SELECT => Some(Command::Settings),
        _ => None,
    }
}

/// Explorer trasmette questo messaggio quando riparte: chi ha un'icona deve riaggiungerla.
pub fn taskbar_created() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

fn show_menu(hwnd: HWND, x: i32, y: i32) -> Result<Option<Command>> {
    unsafe {
        let menu = CreatePopupMenu()?;
        AppendMenuW(menu, MF_STRING, CMD_SETTINGS, w!("Impostazioni…"))?;
        AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
        AppendMenuW(menu, MF_STRING, CMD_EXIT, w!("Esci"))?;
        // La voce predefinita appare in grassetto.
        SetMenuDefaultItem(menu, CMD_SETTINGS as u32, 0)?;

        // Senza questo il menu non si chiude cliccando altrove (stranezza storica di Windows).
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            x,
            y,
            None,
            hwnd,
            None,
        );
        DestroyMenu(menu)?;

        Ok(match cmd.0 as usize {
            CMD_SETTINGS => Some(Command::Settings),
            CMD_EXIT => Some(Command::Exit),
            _ => None,
        })
    }
}

fn base_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    }
}

/// Copia una stringa Rust (UTF-8) in un buffer UTF-16 a dimensione fissa di Windows,
/// troncandola se serve e lasciando sempre lo 0 finale.
fn copy_wide(dst: &mut [u16], s: &str) {
    let max = dst.len() - 1;
    for (i, unit) in s.encode_utf16().take(max).enumerate() {
        dst[i] = unit;
    }
}
