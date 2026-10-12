//! Errori imprevisti (panic): invece di sparire in silenzio, Wheelor li annota in crash.log
//! (accanto alla configurazione) e avvisa l'utente prima di chiudersi.

use std::fs::OpenOptions;
use std::io::Write;

use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};

/// Da chiamare per prima cosa in `main`: vale per tutti i thread del processo.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let message = info.payload_as_str().unwrap_or("errore sconosciuto");
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("senza nome").to_string();
        let process = if std::env::args().any(|a| a == "--settings") { "impostazioni" } else { "ruota" };
        let entry = format!(
            "[{}] Wheelor {} ({process}, thread {thread}): {message} — {location}\n",
            timestamp(),
            env!("CARGO_PKG_VERSION"),
        );

        let path = crate::config::path().with_file_name("crash.log");
        let saved = OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(entry.as_bytes()));
        let detail = match saved {
            Ok(()) => format!("I dettagli sono in {}", path.display()),
            Err(_) => entry,
        };
        let text = HSTRING::from(format!("Wheelor si è chiuso per un errore imprevisto.\n\n{detail}"));
        unsafe {
            MessageBoxW(None, &text, w!("Wheelor"), MB_OK | MB_ICONERROR | MB_SETFOREGROUND);
        }
    }));
}

/// Data e ora locali, es. "2026-10-12 11:32:56".
fn timestamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}
