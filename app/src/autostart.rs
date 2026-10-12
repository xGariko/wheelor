//! Avvio automatico con Windows: un valore nella chiave Run del registro dell'utente
//! (non servono permessi di amministratore).

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::*;
use windows::core::{HSTRING, w};

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: windows::core::PCWSTR = w!("Wheelor");

pub fn is_enabled() -> bool {
    let result = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE, RRF_RT_REG_SZ, None, None, None) };
    result == ERROR_SUCCESS
}

pub fn set(enabled: bool) -> Result<(), String> {
    let result = if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // Tra virgolette, nel caso il percorso contenga spazi; --background: niente notifica di
        // benvenuto all'accesso. Il valore include lo 0 finale.
        let command = HSTRING::from(format!("\"{}\" --background", exe.display()));
        let bytes = (command.len() + 1) * size_of::<u16>();
        unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE, REG_SZ.0, Some(command.as_ptr().cast()), bytes as u32)
        }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE) } {
            ERROR_FILE_NOT_FOUND => ERROR_SUCCESS, // già disattivato
            other => other,
        }
    };
    result.ok().map_err(|e| format!("Impossibile modificare l'avvio automatico: {}", e.message()))
}
