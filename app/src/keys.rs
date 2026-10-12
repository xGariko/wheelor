//! Combinazioni di tasti: lettura da testo ("Ctrl+Shift+S") e invio simulato con SendInput.

use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::trf;

/// Una combinazione: zero o più modificatori più un tasto principale.
#[derive(Debug, Clone, PartialEq)]
pub struct Combo {
    pub modifiers: Vec<VIRTUAL_KEY>,
    pub key: VIRTUAL_KEY,
}

/// I modificatori, nell'ordine in cui si scrivono, con il loro nome.
const MODIFIERS: [(VIRTUAL_KEY, &str); 4] = [(VK_CONTROL, "Ctrl"), (VK_MENU, "Alt"), (VK_SHIFT, "Shift"), (VK_LWIN, "Win")];

/// Nome di ogni tasto nella configurazione (il primo è quello usato scrivendo) e come appare
/// sui tasti disegnati nelle impostazioni, in italiano e in inglese. Lettere, cifre, F1–F24 e Num0–Num9 sono a parte.
const NAMED: [(VIRTUAL_KEY, &[&str], [&str; 2]); 37] = [
    (VK_SPACE, &["Space", "spazio"], ["Spazio", "Space"]),
    (VK_RETURN, &["Enter", "invio"], ["Invio", "Enter"]),
    (VK_TAB, &["Tab"], ["Tab", "Tab"]),
    (VK_ESCAPE, &["Esc", "escape"], ["Esc", "Esc"]),
    (VK_BACK, &["Backspace"], ["Backspace", "Backspace"]),
    (VK_DELETE, &["Delete", "del", "canc"], ["Canc", "Del"]),
    (VK_INSERT, &["Insert", "ins"], ["Ins", "Ins"]),
    (VK_HOME, &["Home"], ["Home", "Home"]),
    (VK_END, &["End", "fine"], ["Fine", "End"]),
    (VK_PRIOR, &["PageUp", "pgup"], ["Pag ↑", "PgUp"]),
    (VK_NEXT, &["PageDown", "pgdn"], ["Pag ↓", "PgDn"]),
    (VK_UP, &["Up", "su"], ["↑", "↑"]),
    (VK_DOWN, &["Down", "giu", "giù"], ["↓", "↓"]),
    (VK_LEFT, &["Left", "sinistra"], ["←", "←"]),
    (VK_RIGHT, &["Right", "destra"], ["→", "→"]),
    (VK_SNAPSHOT, &["PrintScreen", "stamp"], ["Stamp", "PrtSc"]),
    (VK_PAUSE, &["Pause", "pausa"], ["Pausa", "Pause"]),
    (VK_SCROLL, &["ScrollLock"], ["Bloc Scorr", "Scroll Lock"]),
    (VK_APPS, &["Menu", "apps"], ["Menu", "Menu"]),
    (VK_OEM_PLUS, &["Plus"], ["+", "+"]),
    (VK_OEM_MINUS, &["Minus"], ["-", "-"]),
    (VK_OEM_COMMA, &["Comma"], [",", ","]),
    (VK_OEM_PERIOD, &["Period"], [".", "."]),
    (VK_MULTIPLY, &["NumMultiply"], ["Num *", "Num *"]),
    (VK_ADD, &["NumAdd"], ["Num +", "Num +"]),
    (VK_SUBTRACT, &["NumSubtract"], ["Num -", "Num -"]),
    (VK_DECIMAL, &["NumDecimal"], ["Num .", "Num ."]),
    (VK_DIVIDE, &["NumDivide"], ["Num /", "Num /"]),
    (VK_MEDIA_PLAY_PAUSE, &["MediaPlayPause", "playpause"], ["Play/Pausa", "Play/Pause"]),
    (VK_MEDIA_NEXT_TRACK, &["MediaNext", "next"], ["Brano successivo", "Next track"]),
    (VK_MEDIA_PREV_TRACK, &["MediaPrev", "prev"], ["Brano precedente", "Previous track"]),
    (VK_MEDIA_STOP, &["MediaStop"], ["Stop", "Stop"]),
    (VK_VOLUME_UP, &["VolumeUp"], ["Volume +", "Volume +"]),
    (VK_VOLUME_DOWN, &["VolumeDown"], ["Volume −", "Volume −"]),
    (VK_VOLUME_MUTE, &["VolumeMute", "mute"], ["Muto", "Mute"]),
    (VK_CAPITAL, &["CapsLock"], ["Bloc Maiusc", "Caps Lock"]),
    (VK_NUMLOCK, &["NumLock"], ["Bloc Num", "Num Lock"]),
];

impl Combo {
    /// La combinazione come si scrive nella configurazione, con i modificatori sempre nello
    /// stesso ordine: "Ctrl+Alt+W".
    pub fn text(&self) -> String {
        let mut parts = modifier_names(&self.modifiers);
        parts.push(key_name(self.key));
        parts.join("+")
    }

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
        let name = part.to_lowercase();
        match modifier_from_name(&name) {
            Some(m) if !modifiers.contains(&m) => modifiers.push(m),
            Some(_) => {}
            None if key.is_some() => {
                return Err(trf!("\"{text}\": c'è più di un tasto principale", "\"{text}\": more than one main key"));
            }
            None => {
                let unknown = || trf!("Tasto sconosciuto: \"{part}\"", "Unknown key: \"{part}\"");
                key = Some(key_from_name(&name).ok_or_else(unknown)?);
            }
        }
    }

    let key = key.ok_or_else(|| trf!("\"{text}\": manca il tasto principale", "\"{text}\": the main key is missing"))?;
    // Sempre nello stesso ordine: "Alt+Ctrl+W" e "Ctrl+Alt+W" sono la stessa combinazione.
    modifiers.sort_by_key(|m| MODIFIERS.iter().position(|(vk, _)| vk == m));
    Ok(Combo { modifiers, key })
}

/// `name` è già in minuscolo.
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
    if let Some((vk, _, _)) = NAMED.iter().find(|(_, names, _)| names.iter().any(|n| n.eq_ignore_ascii_case(name))) {
        return Some(*vk);
    }
    // Un altro carattere (es. "ò", "'", "<"): il tasto che lo scrive senza Shift nel layout in uso.
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && let Ok(unit) = u16::try_from(u32::from(c))
    {
        let scan = unsafe { VkKeyScanW(unit) };
        // Byte basso: il tasto; byte alto: i modificatori necessari (devono essere nessuno).
        if scan != -1 && scan as u16 >> 8 == 0 {
            return Some(VIRTUAL_KEY(scan as u16 & 0xFF));
        }
    }
    // Qualunque altro tasto, con il suo codice: "vk5B".
    name.strip_prefix("vk").and_then(|hex| u16::from_str_radix(hex, 16).ok()).filter(|&vk| vk > 0 && vk < 0xFF).map(VIRTUAL_KEY)
}

/// Il nome del tasto come lo legge `parse`.
fn key_name(vk: VIRTUAL_KEY) -> String {
    match vk.0 {
        0x30..=0x39 | 0x41..=0x5A => return char::from(vk.0 as u8).to_string(),
        v if (VK_F1.0..=VK_F24.0).contains(&v) => return format!("F{}", v - VK_F1.0 + 1),
        v if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&v) => return format!("Num{}", v - VK_NUMPAD0.0),
        _ => {}
    }
    if let Some((_, names, _)) = NAMED.iter().find(|(key, _, _)| *key == vk) {
        return names[0].to_string();
    }
    // Tasti che scrivono un carattere diverso secondo il layout (ò, à, ', <…).
    let char = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_CHAR) } & 0xFFFF;
    match char::from_u32(char).filter(|c| !c.is_control() && !c.is_whitespace() && *c != '+') {
        Some(c) => c.to_lowercase().collect(),
        None => format!("VK{:02X}", vk.0),
    }
}

/// I tasti di una combinazione come vanno disegnati nelle impostazioni: "Ctrl", "Alt", "Spazio".
/// Le parti che non riconosce restano come sono scritte.
pub fn caps(text: &str) -> Vec<String> {
    text.split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|part| {
            let name = part.to_lowercase();
            if let Some(m) = modifier_from_name(&name) {
                return MODIFIERS.iter().find(|(vk, _)| *vk == m).map_or(part, |(_, n)| *n).to_string();
            }
            match key_from_name(&name) {
                Some(vk) => key_label(vk),
                None => part.to_string(),
            }
        })
        .collect()
}

/// L'etichetta di un tasto disegnato.
pub fn key_label(vk: VIRTUAL_KEY) -> String {
    match NAMED.iter().find(|(key, _, _)| *key == vk) {
        Some((_, _, label)) => label[usize::from(crate::i18n::english())].to_string(),
        None => key_name(vk).replace("Num", "Num "),
    }
}

/// Una scorciatoia globale toglie il suo tasto a tutte le altre app. Se è un tasto che serve
/// per scrivere o per muoversi nel testo, ci vuole almeno Ctrl, Alt o Win: altrimenti, per
/// esempio, Shift+A non scriverebbe più la A maiuscola. Restituisce il problema, se c'è.
pub fn hotkey_problem(combo: &Combo) -> Option<String> {
    let strong = combo.modifiers.iter().any(|m| matches!(*m, VK_CONTROL | VK_MENU | VK_LWIN));
    let alone = matches!(
        combo.key,
        VK_PAUSE | VK_SCROLL | VK_INSERT | VK_SNAPSHOT | VK_APPS
            | VK_MEDIA_PLAY_PAUSE | VK_MEDIA_NEXT_TRACK | VK_MEDIA_PREV_TRACK | VK_MEDIA_STOP
            | VK_VOLUME_UP | VK_VOLUME_DOWN | VK_VOLUME_MUTE
    ) || (VK_F1.0..=VK_F24.0).contains(&combo.key.0);
    if strong || alone {
        return None;
    }
    let what = if combo.modifiers.is_empty() { key_label(combo.key) } else { caps(&combo.text()).join("+") };
    Some(trf!("{what} serve per scrivere: aggiungi Ctrl, Alt o Win.", "{what} is needed for typing: add Ctrl, Alt or Win."))
}

/// I nomi dei modificatori, nell'ordine in cui si scrivono: "Ctrl", "Alt"…
pub fn modifier_names(modifiers: &[VIRTUAL_KEY]) -> Vec<String> {
    MODIFIERS.iter().filter(|(vk, _)| modifiers.contains(vk)).map(|(_, name)| name.to_string()).collect()
}

fn modifier_from_name(name: &str) -> Option<VIRTUAL_KEY> {
    match name {
        "ctrl" | "control" => Some(VK_CONTROL),
        "alt" => Some(VK_MENU),
        "shift" | "maiusc" => Some(VK_SHIFT),
        "win" | "windows" => Some(VK_LWIN),
        _ => None,
    }
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
        assert_eq!(parse(" win + shift + s "), Ok(Combo { modifiers: vec![VK_SHIFT, VK_LWIN], key: VIRTUAL_KEY(b'S' as u16) }));
        assert_eq!(parse("F12").map(|c| c.key), Ok(VK_F12));
        assert_eq!(parse("MediaPlayPause").map(|c| c.key), Ok(VK_MEDIA_PLAY_PAUSE));
        assert_eq!(parse("Ctrl+5").map(|c| c.key), Ok(VIRTUAL_KEY(b'5' as u16)));
    }

    #[test]
    fn text_is_normalized_and_parsed_back() {
        for (written, normal) in [
            ("alt+ctrl+w", "Ctrl+Alt+W"),
            ("Win+Shift+Space", "Shift+Win+Space"),
            ("f13", "F13"),
            ("Ctrl+num5", "Ctrl+Num5"),
            ("Ctrl+Alt+Pausa", "Ctrl+Alt+Pause"),
            ("ctrl+vk5d", "Ctrl+Menu"),
            ("mediaplaypause", "MediaPlayPause"),
        ] {
            let combo = parse(written).unwrap();
            assert_eq!(combo.text(), normal);
            assert_eq!(parse(normal), Ok(combo));
        }
    }

    #[test]
    fn every_key_name_parses_back() {
        for vk in 1..0xFF {
            let vk = VIRTUAL_KEY(vk);
            if [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN].contains(&vk) {
                continue;
            }
            let name = key_name(vk);
            let back = key_from_name(&name.to_lowercase());
            // Due codici possono scrivere lo stesso carattere: conta che si torni a un tasto.
            assert!(back.is_some(), "{name} ({:02X}) non si rilegge", vk.0);
        }
    }

    #[test]
    fn caps_are_readable() {
        assert_eq!(caps("ctrl+alt+space"), ["Ctrl", "Alt", "Spazio"]);
        assert_eq!(caps("Win+Shift+S"), ["Win", "Shift", "S"]);
        assert_eq!(caps("MediaPlayPause"), ["Play/Pausa"]);
        assert_eq!(caps("Ctrl+Num5"), ["Ctrl", "Num 5"]);
        assert_eq!(caps("Ctrl+Pippo"), ["Ctrl", "Pippo"]);
    }

    #[test]
    fn hotkeys_that_steal_typing_are_flagged() {
        let problem = |text: &str| hotkey_problem(&parse(text).unwrap());
        assert!(problem("A").is_some());
        assert!(problem("Shift+A").is_some());
        assert!(problem("Space").is_some());
        assert!(problem("Ctrl+A").is_none());
        assert!(problem("Win+Shift+Space").is_none());
        assert!(problem("F13").is_none());
        assert!(problem("Shift+F2").is_none());
        assert!(problem("Pause").is_none());
    }

    #[test]
    fn rejects_invalid_combos() {
        assert!(parse("Ctrl+Alt").is_err()); // solo modificatori
        assert!(parse("Ctrl+A+B").is_err()); // due tasti principali
        assert!(parse("Ctrl+Pippo").is_err()); // tasto inesistente
        assert!(parse("").is_err());
    }
}
