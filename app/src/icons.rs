//! Catalogo delle icone Segoe Fluent Icons (codice esadecimale + nome inglese).

use std::sync::OnceLock;

use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{BOOL, HSTRING};

/// Font delle icone: Segoe Fluent Icons (Windows 11) o, se manca, Segoe MDL2 Assets
/// (Windows 10), che usa gli stessi codici per quasi tutte le icone.
pub fn font_family() -> &'static str {
    static FAMILY: OnceLock<&'static str> = OnceLock::new();
    FAMILY.get_or_init(|| if font_installed("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" })
}

/// Il font `name` è installato nel sistema? Lo chiediamo a DirectWrite.
pub fn font_installed(name: &str) -> bool {
    unsafe {
        let Ok(factory) = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) else { return false };
        let mut collection = None;
        if factory.GetSystemFontCollection(&mut collection, false).is_err() {
            return false;
        }
        let Some(collection) = collection else { return false };
        let (mut index, mut exists) = (0, BOOL::default());
        collection.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}

/// Tabella "CODICE Nome" estratta dalla documentazione Microsoft, incorporata nell'exe.
const TABLE: &str = include_str!("icons.txt");

/// Parole italiane comuni → termine inglese usato nei nomi delle icone.
const ITALIAN: &[(&str, &str)] = &[
    ("aggiungi", "add"),
    ("blocca", "lock"),
    ("calendario", "calendar"),
    ("cartella", "folder"),
    ("casa", "home"),
    ("cerca", "search"),
    ("chiudi", "cancel"),
    ("codice", "code"),
    ("copia", "copy"),
    ("documento", "document"),
    ("elimina", "delete"),
    ("foto", "photo"),
    ("fotocamera", "camera"),
    ("impostazioni", "setting"),
    ("incolla", "paste"),
    ("internet", "globe"),
    ("microfono", "microphone"),
    ("modifica", "edit"),
    ("musica", "music"),
    ("orologio", "clock"),
    ("persona", "contact"),
    ("posta", "mail"),
    ("preferiti", "favorite"),
    ("rete", "network"),
    ("riproduci", "play"),
    ("salva", "save"),
    ("schermo", "monitor"),
    ("spegni", "power"),
    ("stampa", "print"),
    ("stella", "star"),
    ("taglia", "cut"),
    ("telefono", "phone"),
    ("terminale", "commandprompt"),
    ("utente", "contact"),
];

/// Massimo di risultati mostrati insieme, per mantenere la griglia reattiva.
const MAX_RESULTS: usize = 600;

pub struct Icon {
    pub code: &'static str,
    pub name: &'static str,
}

/// Tutte le icone del catalogo. Restituisce un iteratore "pigro": le righe vengono
/// lette solo mentre qualcuno scorre i risultati, senza creare liste intermedie.
pub fn all() -> impl Iterator<Item = Icon> {
    TABLE
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once(' '))
        .map(|(code, name)| Icon { code, name })
}

/// Icone il cui nome o codice contiene `query` (anche in italiano, per le parole più comuni).
pub fn search(query: &str) -> Vec<Icon> {
    let query = query.trim().to_lowercase();
    let english = ITALIAN
        .iter()
        .find(|(it, _)| !query.is_empty() && it.starts_with(&query))
        .map(|(_, en)| *en);

    all()
        .filter(|icon| {
            let name = icon.name.to_lowercase();
            query.is_empty()
                || name.contains(&query)
                || icon.code.eq_ignore_ascii_case(&query)
                || english.is_some_and(|en| name.contains(en))
        })
        .take(MAX_RESULTS)
        .collect()
}

/// Codice esadecimale ("E756") → carattere del font Segoe Fluent Icons. Vuoto se non valido.
pub fn glyph(code: &str) -> String {
    let code = code.trim().trim_start_matches("U+").trim_start_matches("0x");
    u32::from_str_radix(code, 16).ok().and_then(char::from_u32).map(String::from).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_loaded() {
        assert!(all().count() > 1000);
        assert!(all().any(|i| i.code == "E8B7" && i.name == "Folder"));
    }

    #[test]
    fn search_finds_by_name_code_and_italian() {
        assert!(search("folder").iter().any(|i| i.code == "E8B7"));
        assert!(search("e8b7").iter().any(|i| i.code == "E8B7"));
        assert!(search("cartella").iter().any(|i| i.code == "E8B7"));
    }

    #[test]
    fn fonts_are_detected() {
        assert!(font_installed("Segoe UI"));
        assert!(!font_installed("Font Che Non Esiste"));
        assert!(font_family().starts_with("Segoe"));
    }

    #[test]
    fn glyph_converts_codes() {
        assert_eq!(glyph("E756"), "\u{E756}");
        assert_eq!(glyph("U+e756"), "\u{E756}");
        assert_eq!(glyph("pippo"), "");
    }
}
