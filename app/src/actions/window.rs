//! Le finestre delle altre app: trovarle, portarle in primo piano, ridimensionarle.

use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

use super::process_path;
use crate::config::{WindowOp, app_matches};
use crate::{tr, trf};

/// La finestra in primo piano, se è una finestra vera e non il desktop o la barra delle applicazioni.
pub fn foreground() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    let shell = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&class.as_str());
    (!shell).then_some(hwnd)
}

/// Porta `hwnd` in primo piano (ripristinandola se è ridotta a icona).
///
/// Windows lascia cambiare la finestra in primo piano solo a chi l'ha già, o a chi ha ricevuto
/// l'ultimo input: agganciandoci per un attimo all'input della finestra attuale ne prendiamo i
/// diritti (lo stesso trucco usato dalle utility di automazione).
pub fn bring_to_front(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd {
            return true;
        }
        // Un thread ha una coda di messaggi solo dopo averne chiesto uno: serve ad AttachThreadInput.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        let current = GetCurrentThreadId();
        let owner = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let attached = owner != 0 && owner != current && AttachThreadInput(current, owner, true).as_bool();
        // Un tasto fittizio (VK 0xE8, non fa nulla) conta come input ricevuto.
        let mask = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), dwFlags: flags, ..Default::default() } },
        };
        SendInput(&[mask(KEYBD_EVENT_FLAGS(0)), mask(KEYEVENTF_KEYUP)], size_of::<INPUT>() as i32);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(current, owner, false);
        }
        GetForegroundWindow() == hwnd
    }
}

/// La finestra `hwnd` è dell'app `app` e il suo titolo contiene `title`? (Vuoti = qualunque.)
fn matches(hwnd: HWND, app: &str, title: &str) -> bool {
    if !app.trim().is_empty() && !process_path(hwnd).is_some_and(|path| app_matches(app, &path)) {
        return false;
    }
    title.trim().is_empty() || window_title(hwnd).to_lowercase().contains(&title.trim().to_lowercase())
}

fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..len])
}

/// Una finestra visibile (non nascosta da Windows) che corrisponde ad app e titolo.
fn find(app: &str, title: &str) -> Option<HWND> {
    struct Search<'a> {
        app: &'a str,
        title: &'a str,
        found: Option<HWND>,
    }
    extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut cloaked = 0u32;
        unsafe {
            let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32);
            let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid());
            if IsWindowVisible(hwnd).as_bool() && cloaked == 0 && !owned && matches(hwnd, search.app, search.title) {
                search.found = Some(hwnd);
                return FALSE; // trovata: basta così
            }
        }
        TRUE
    }
    let mut search = Search { app, title, found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// Aspetta che una finestra dell'app `app` (e con `title` nel titolo) sia in primo piano.
/// Se c'è già ma sta dietro, la porta davanti.
pub fn wait_for(app: &str, title: &str, timeout: f32, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if app.trim().is_empty() && title.trim().is_empty() {
        return Err(tr!("Scrivi l'app o il titolo della finestra da aspettare", "Enter the app or the window title to wait for").into());
    }
    let start = Instant::now();
    let limit = Duration::from_secs_f32(timeout.max(0.5));
    loop {
        if foreground().is_some_and(|hwnd| matches(hwnd, app, title)) {
            return Ok(());
        }
        if let Some(hwnd) = find(app, title)
            && bring_to_front(hwnd)
        {
            return Ok(());
        }
        if cancelled() {
            return Ok(()); // chi esegue se ne accorge da solo
        }
        if start.elapsed() >= limit {
            let what = if app.trim().is_empty() { title.trim() } else { app.trim() };
            return Err(trf!(
                "«{what}» non è comparsa entro {} secondi",
                "“{what}” didn't show up within {} seconds",
                limit.as_secs_f32().round()
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Esegue `op`: su `original` (la finestra in primo piano quando si è aperta la ruota) per
/// `Focus`, sulla finestra in primo piano adesso per tutto il resto.
pub fn apply(op: WindowOp, original: HWND) -> Result<(), String> {
    if op == WindowOp::Focus {
        if original.is_invalid() || !unsafe { IsWindow(Some(original)) }.as_bool() {
            return Err(tr!("La finestra di partenza non c'è più", "The starting window is gone").into());
        }
        bring_to_front(original);
        return Ok(());
    }
    let Some(hwnd) = foreground() else {
        return Err(tr!("Non c'è una finestra in primo piano", "There's no window in the foreground").into());
    };
    unsafe {
        match op {
            WindowOp::Focus => {}
            WindowOp::Minimize => {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            }
            WindowOp::Maximize => {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            }
            WindowOp::Restore => {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            WindowOp::LeftHalf => snap(hwnd, true),
            WindowOp::RightHalf => snap(hwnd, false),
            WindowOp::AlwaysOnTop => {
                let topmost = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0;
                let after = if topmost { HWND_NOTOPMOST } else { HWND_TOPMOST };
                let _ = SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            WindowOp::Close => {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
    Ok(())
}

/// Sistema la finestra sulla metà sinistra o destra dello schermo su cui si trova.
fn snap(hwnd: HWND, left: bool) {
    unsafe {
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return;
        }
        if IsZoomed(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        // Su Windows 10 e 11 il rettangolo della finestra comprende un bordo invisibile (l'ombra):
        // lo si ricava confrontandolo con quello disegnato, e se ne tiene conto.
        let mut outer = RECT::default();
        let mut frame = RECT::default();
        let _ = GetWindowRect(hwnd, &mut outer);
        let border = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut RECT as *mut _, size_of::<RECT>() as u32);
        let (l, t, r, b) = match border {
            Ok(()) => (frame.left - outer.left, frame.top - outer.top, outer.right - frame.right, outer.bottom - frame.bottom),
            Err(_) => (0, 0, 0, 0),
        };
        let work = info.rcWork;
        let half = (work.right - work.left) / 2;
        let (x, width) = if left { (work.left, half) } else { (work.left + half, work.right - work.left - half) };
        let _ = SetWindowPos(
            hwnd,
            None,
            x - l,
            work.top - t,
            width + l + r,
            work.bottom - work.top + t + b,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}
