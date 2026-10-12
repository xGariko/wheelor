//! Combinazioni di tasti: lettura da testo ("Ctrl+Shift+S") e invio simulato con SendInput.

use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::*;

/// Una combinazione: zero o più modificatori più un tasto principale.
#[derive(Debug, Clone, PartialEq)]
pub struct Combo {
    pub modifiers: Vec<VIRTUAL_KEY>,
    pub key: VIRTUAL_KEY,
}

impl Combo {
    /// Modificatori nel formato richiesto da RegisterHotKey.
    pub fn hotkey_modifiers(&self) -> HOT_KEY_MODIFIERS {
        let mut flags = HOT_KEY_MODIFIERS(0);
        for &m in &self.modifiers {
            flags |= match m {
                VK_CONTROL => MOD_CONTROL,
                VK_MENU => MOD_ALT,
                VK_SHIFT => MOD_SHIFT,
                _ => MOD_WIN,
            };
        }
        flags
    }
}

/// Legge una combinazione scritta come "Ctrl+Alt+W", "Win+Shift+S", "F5", "MediaPlayPause".
/// Maiuscole/minuscole e spazi non contano.
pub fn parse(text: &str) -> Result<Combo, String> {
    let mut modifiers = Vec::new();
    let mut key = None;

    for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        let name = part.to_ascii_lowercase();
        let modifier = match name.as_str() {
            "ctrl" | "control" => Some(VK_CONTROL),
            "alt" => Some(VK_MENU),
            "shift" | "maiusc" => Some(VK_SHIFT),
            "win" | "windows" => Some(VK_LWIN),
            _ => None,
        };
        match modifier {
            Some(m) => modifiers.push(m),
            None if key.is_some() => return Err(format!("\"{text}\": c'è più di un tasto principale")),
            None => key = Some(key_from_name(&name).ok_or_else(|| format!("Tasto sconosciuto: \"{part}\""))?),
        }
    }

    let key = key.ok_or_else(|| format!("\"{text}\": manca il tasto principale"))?;
    Ok(Combo { modifiers, key })
}

fn key_from_name(name: &str) -> Option<VIRTUAL_KEY> {
    // Una sola lettera o cifra: il codice virtuale coincide con il carattere ASCII maiuscolo.
    if let [c] = name.as_bytes() {
        if c.is_ascii_alphanumeric() {
            return Some(VIRTUAL_KEY(c.to_ascii_uppercase() as u16));
        }
    }
    // F1..F24
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        if (1..=24).contains(&n) {
            return Some(VIRTUAL_KEY(VK_F1.0 + n - 1));
        }
    }
    // Tastierino numerico: num0..num9
    if let Some(n) = name.strip_prefix("num").and_then(|n| n.parse::<u16>().ok()) {
        if n <= 9 {
            return Some(VIRTUAL_KEY(VK_NUMPAD0.0 + n));
        }
    }
    let key = match name {
        "space" | "spazio" => VK_SPACE,
        "enter" | "invio" => VK_RETURN,
        "tab" => VK_TAB,
        "esc" | "escape" => VK_ESCAPE,
        "backspace" => VK_BACK,
        "delete" | "del" | "canc" => VK_DELETE,
        "insert" | "ins" => VK_INSERT,
        "home" => VK_HOME,
        "end" | "fine" => VK_END,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "up" | "su" => VK_UP,
        "down" | "giu" | "giù" => VK_DOWN,
        "left" | "sinistra" => VK_LEFT,
        "right" | "destra" => VK_RIGHT,
        "printscreen" | "stamp" => VK_SNAPSHOT,
        "plus" => VK_OEM_PLUS,
        "minus" => VK_OEM_MINUS,
        "comma" => VK_OEM_COMMA,
        "period" => VK_OEM_PERIOD,
        "mediaplaypause" | "playpause" => VK_MEDIA_PLAY_PAUSE,
        "medianext" | "next" => VK_MEDIA_NEXT_TRACK,
        "mediaprev" | "prev" => VK_MEDIA_PREV_TRACK,
        "mediastop" => VK_MEDIA_STOP,
        "volumeup" => VK_VOLUME_UP,
        "volumedown" => VK_VOLUME_DOWN,
        "volumemute" | "mute" => VK_VOLUME_MUTE,
        _ => return None,
    };
    Some(key)
}

/// Preme e rilascia la combinazione.
pub fn send(combo: &Combo) {
    let mut inputs = Vec::new();
    for &m in &combo.modifiers {
        inputs.push(key_input(m, false));
    }
    inputs.push(key_input(combo.key, false));
    inputs.push(key_input(combo.key, true));
    for &m in combo.modifiers.iter().rev() {
        inputs.push(key_input(m, true));
    }
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

/// Scrive un testo carattere per carattere (funziona con qualsiasi layout di tastiera).
pub fn type_text(text: &str) {
    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        match unit {
            0x0D => {} // '\r': ignorato, l'a capo lo fa '\n'
            0x0A => {
                inputs.push(key_input(VK_RETURN, false));
                inputs.push(key_input(VK_RETURN, true));
            }
            _ => {
                inputs.push(unicode_input(unit, false));
                inputs.push(unicode_input(unit, true));
            }
        }
    }
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

/// Aspetta (al massimo 3 secondi) che l'utente rilasci Ctrl, Alt, Shift e Win.
/// Altrimenti, ad esempio, un Ctrl+C inviato mentre Alt è ancora premuto diventerebbe Ctrl+Alt+C.
pub fn wait_for_modifiers_release() {
    let held = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) } < 0;
    let start = Instant::now();
    while [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN].into_iter().any(held) {
        if start.elapsed() > Duration::from_secs(3) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    keyboard_input(KEYBDINPUT { wVk: vk, dwFlags: flags, ..Default::default() })
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    let flags = if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE };
    keyboard_input(KEYBDINPUT { wScan: unit, dwFlags: flags, ..Default::default() })
}

fn keyboard_input(ki: KEYBDINPUT) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
}

/// Tasti "estesi": senza questo flag alcune app li confondono con quelli del tastierino.
fn is_extended(vk: VIRTUAL_KEY) -> bool {
    matches!(
        vk,
        VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PRIOR | VK_NEXT
            | VK_INSERT | VK_DELETE | VK_LWIN | VK_RWIN | VK_SNAPSHOT
            | VK_MEDIA_PLAY_PAUSE | VK_MEDIA_NEXT_TRACK | VK_MEDIA_PREV_TRACK | VK_MEDIA_STOP
            | VK_VOLUME_UP | VK_VOLUME_DOWN | VK_VOLUME_MUTE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_combos() {
        assert_eq!(parse("Ctrl+Alt+W"), Ok(Combo { modifiers: vec![VK_CONTROL, VK_MENU], key: VIRTUAL_KEY(b'W' as u16) }));
        assert_eq!(parse(" win + shift + s "), Ok(Combo { modifiers: vec![VK_LWIN, VK_SHIFT], key: VIRTUAL_KEY(b'S' as u16) }));
        assert_eq!(parse("F12").map(|c| c.key), Ok(VK_F12));
        assert_eq!(parse("MediaPlayPause").map(|c| c.key), Ok(VK_MEDIA_PLAY_PAUSE));
        assert_eq!(parse("Ctrl+5").map(|c| c.key), Ok(VIRTUAL_KEY(b'5' as u16)));
    }

    #[test]
    fn rejects_invalid_combos() {
        assert!(parse("Ctrl+Alt").is_err()); // solo modificatori
        assert!(parse("Ctrl+A+B").is_err()); // due tasti principali
        assert!(parse("Ctrl+Pippo").is_err()); // tasto inesistente
        assert!(parse("").is_err());
    }
}
