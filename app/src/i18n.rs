//! Lingua dell'interfaccia: italiano o inglese.
//!
//! - I testi della finestra delle impostazioni sono scritti in italiano in ui/settings.slint
//!   dentro `@tr(…)`; la traduzione inglese sta in translations/en/LC_MESSAGES/wheelor.po ed è
//!   incorporata nell'exe (vedi build.rs). `apply_to_slint` sceglie quale mostrare.
//! - I testi preparati in Rust (notifiche, menu, errori, righe degli elenchi…) usano le macro
//!   `tr!` e `trf!`, con le due versioni una accanto all'altra.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use windows::Win32::Globalization::GetUserDefaultUILanguage;

/// Lingua scelta nelle impostazioni. Nel file: `language = "it"` o `"en"`; senza, quella di Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[serde(rename = "it")]
    Italian,
    #[serde(rename = "en")]
    English,
}

/// Finché non viene chiamata `set` (per esempio nei test) i testi sono in italiano.
static ENGLISH: AtomicBool = AtomicBool::new(false);

/// Imposta la lingua: quella scelta oppure, con `None`, quella di Windows (italiano se Windows
/// è in italiano, inglese per tutte le altre lingue).
pub fn set(choice: Option<Language>) {
    ENGLISH.store(resolve(choice) == Language::English, Ordering::Relaxed);
}

/// La lingua che risulta da una scelta (`None` = quella di Windows).
pub fn resolve(choice: Option<Language>) -> Language {
    choice.unwrap_or_else(system)
}

/// La lingua dell'interfaccia di Windows, ridotta a quelle che Wheelor conosce.
pub fn system() -> Language {
    // I 10 bit bassi del LANGID sono la lingua principale; 0x10 = LANG_ITALIAN.
    let italian = unsafe { GetUserDefaultUILanguage() } & 0x3FF == 0x10;
    if italian { Language::Italian } else { Language::English }
}

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// Mostra la finestra delle impostazioni nella lingua attuale (le traduzioni sono nell'exe).
pub fn apply_to_slint() {
    // "" = la lingua in cui sono scritti i testi, l'italiano.
    let _ = slint::select_bundled_translation(if english() { "en" } else { "" });
}

/// Testo fisso nelle due lingue: `tr!("Esci", "Exit")`.
#[macro_export]
macro_rules! tr {
    ($it:literal, $en:literal $(,)?) => {
        if $crate::i18n::english() { $en } else { $it }
    };
}

/// Testo con valori, come `format!`: `trf!("Ciao {name}", "Hello {name}")`.
#[macro_export]
macro_rules! trf {
    ($it:literal, $en:literal $(, $($arg:tt)*)?) => {
        if $crate::i18n::english() { format!($en $(, $($arg)*)?) } else { format!($it $(, $($arg)*)?) }
    };
}
