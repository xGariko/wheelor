//! Segnaposto nei campi dei passi, scritti fra graffe e in inglese come tutto il file della
//! configurazione: `{selection}`, `{clipboard}`, `{date}`, `{ask:Cosa cerchi?}`…
//!
//! Il valore di un segnaposto finisce nel testo in modo diverso a seconda del campo:
//! - in un indirizzo web viene codificato (spazi, &, ? non rompono l'indirizzo);
//! - in un comando non viene incollato: diventa una variabile d'ambiente, così un testo
//!   selezionato con dentro `"; del …` resta testo e non diventa codice eseguito;
//! - altrove (testo da scrivere, appunti, opzioni di avvio…) entra così com'è.
//!
//! Le graffe con un nome sconosciuto restano come sono: `{ $_.Name }` in PowerShell funziona.

use windows::Win32::Globalization::{DATE_SHORTDATE, ENUM_DATE_FORMATS_FLAGS, GetDateFormatEx, GetTimeFormatEx, TIME_FORMAT_FLAGS, TIME_NOSECONDS};
use windows::core::{HSTRING, PCWSTR};

use crate::config::Shell;

/// I segnaposto, con il nome della variabile d'ambiente che li porta nei comandi.
pub const NAMES: [(&str, &str); 9] = [
    ("selection", "WHEELOR_SELECTION"),
    ("clipboard", "WHEELOR_CLIPBOARD"),
    ("app", "WHEELOR_APP"),
    ("title", "WHEELOR_TITLE"),
    ("wheel", "WHEELOR_WHEEL"),
    ("date", "WHEELOR_DATE"),
    ("time", "WHEELOR_TIME"),
    ("result", "WHEELOR_RESULT"),
    ("ask", "WHEELOR_ASK"),
];

/// Come inserire i valori nel testo.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Plain,
    /// Indirizzo web: i valori vengono codificati.
    Url,
    /// Comando: i valori diventano variabili d'ambiente.
    Shell(Shell),
}

/// Un pezzo di testo: testo normale o un segnaposto (con l'eventuale parte dopo i due punti).
#[derive(Debug, PartialEq)]
enum Part<'a> {
    Text(&'a str),
    Placeholder { name: &'a str, arg: Option<&'a str> },
}

/// Divide il testo in pezzi. Un segnaposto è `{nome}` o `{nome:qualcosa}` con un nome di `NAMES`.
fn parse(text: &str) -> Vec<Part<'_>> {
    let mut parts = Vec::new();
    let mut rest = text;
    let mut plain = 0; // byte di `rest` già visti che restano testo normale
    while let Some(open) = rest[plain..].find('{').map(|i| i + plain) {
        let Some(close) = rest[open..].find('}').map(|i| i + open) else { break };
        let inner = &rest[open + 1..close];
        let (name, arg) = match inner.split_once(':') {
            Some((name, arg)) => (name, Some(arg)),
            None => (inner, None),
        };
        // `ask` vuole la domanda; date e time accettano un formato; gli altri niente.
        let valid = NAMES.iter().any(|(n, _)| *n == name)
            && match name {
                "ask" => arg.is_some_and(|a| !a.trim().is_empty()),
                "date" | "time" => true,
                _ => arg.is_none(),
            };
        if !valid {
            plain = open + 1;
            continue;
        }
        if open > 0 {
            parts.push(Part::Text(&rest[..open]));
        }
        parts.push(Part::Placeholder { name, arg });
        rest = &rest[close + 1..];
        plain = 0;
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest));
    }
    parts
}

/// Il testo contiene il segnaposto `name`?
pub fn mentions(text: &str, name: &str) -> bool {
    parse(text).iter().any(|part| matches!(part, Part::Placeholder { name: n, .. } if *n == name))
}

/// Sostituisce i segnaposto con i valori dati da `value(nome, argomento)`. Restituisce il testo e,
/// per i comandi, le variabili d'ambiente da impostare.
pub fn expand<E>(
    text: &str,
    mode: Mode,
    mut value: impl FnMut(&str, Option<&str>) -> Result<String, E>,
) -> Result<(String, Vec<(String, String)>), E> {
    let mut out = String::with_capacity(text.len());
    let mut env: Vec<(String, String)> = Vec::new();
    for part in parse(text) {
        match part {
            Part::Text(text) => out.push_str(text),
            Part::Placeholder { name, arg } => {
                let value = value(name, arg)?;
                match mode {
                    Mode::Plain => out.push_str(&value),
                    Mode::Url => out.push_str(&url_encode(&value)),
                    Mode::Shell(shell) => {
                        // Senza argomento la variabile ha il nome del segnaposto; con un argomento
                        // (domande, formati di data) ognuno ha la sua, numerata.
                        let base = NAMES.iter().find(|(n, _)| *n == name).map_or("WHEELOR_VALUE", |(_, v)| v);
                        let var = match arg {
                            None => base.to_string(),
                            Some(_) => format!("{base}_{}", env.iter().filter(|(v, _)| v.starts_with(base)).count() + 1),
                        };
                        out.push_str(&match shell {
                            Shell::Powershell | Shell::Pwsh => format!("${{env:{var}}}"),
                            // Espansione ritardata (cmd /v:on): il valore non viene più interpretato.
                            Shell::Cmd => format!("!{var}!"),
                        });
                        if !env.iter().any(|(v, _)| *v == var) {
                            env.push((var, value));
                        }
                    }
                }
            }
        }
    }
    Ok((out, env))
}

/// Il campo "App, file o sito" contiene un indirizzo (https:, mailto:, ms-teams:…)? Le lettere di
/// unità (C:\…) no: lo schema di un indirizzo ha almeno due lettere.
pub fn is_url(target: &str) -> bool {
    let Some((scheme, _)) = target.trim().split_once(':') else { return false };
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
}

/// Codifica per un indirizzo web: restano solo lettere, cifre e - _ . ~ (RFC 3986).
fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// La data di oggi: nel formato breve di Windows o in `format` (es. "yyyy-MM-dd", "d MMMM").
pub fn date(format: Option<&str>) -> String {
    let format = format.map(HSTRING::from);
    let picture = format.as_ref().map_or(PCWSTR::null(), |f| PCWSTR(f.as_ptr()));
    let flags = if format.is_some() { ENUM_DATE_FORMATS_FLAGS(0) } else { DATE_SHORTDATE };
    let mut buffer = [0u16; 128];
    let len = unsafe { GetDateFormatEx(PCWSTR::null(), flags, None, picture, Some(&mut buffer), PCWSTR::null()) };
    String::from_utf16_lossy(&buffer[..(len.max(1) - 1) as usize])
}

/// L'ora attuale: nel formato breve di Windows (senza secondi) o in `format` (es. "HH.mm").
pub fn time(format: Option<&str>) -> String {
    let format = format.map(HSTRING::from);
    let picture = format.as_ref().map_or(PCWSTR::null(), |f| PCWSTR(f.as_ptr()));
    let flags = if format.is_some() { TIME_FORMAT_FLAGS(0) } else { TIME_NOSECONDS };
    let mut buffer = [0u16; 128];
    let len = unsafe { GetTimeFormatEx(PCWSTR::null(), flags, None, picture, Some(&mut buffer)) };
    String::from_utf16_lossy(&buffer[..(len.max(1) - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(text: &str, mode: Mode) -> (String, Vec<(String, String)>) {
        expand(text, mode, |name, arg| {
            Ok::<_, ()>(match (name, arg) {
                ("selection", _) => "ciao & \"mondo\"".to_string(),
                ("ask", Some(question)) => format!("<{question}>"),
                ("date", Some(format)) => format!("[{format}]"),
                (name, _) => name.to_uppercase(),
            })
        })
        .unwrap()
    }

    #[test]
    fn unknown_braces_stay() {
        assert_eq!(fill("{ $_.Name } {nope} {selection:x} {ask} {", Mode::Plain).0, "{ $_.Name } {nope} {selection:x} {ask} {");
        assert_eq!(fill("a{b{title}c}", Mode::Plain).0, "a{bTITLEc}");
    }

    #[test]
    fn values_are_placed_by_mode() {
        assert_eq!(fill("Cerca {selection}!", Mode::Plain).0, "Cerca ciao & \"mondo\"!");
        assert_eq!(fill("https://x.it/?q={selection}", Mode::Url).0, "https://x.it/?q=ciao%20%26%20%22mondo%22");
        let (text, env) = fill("echo \"{selection}\" {ask:Nome?} {ask:Età?} {date:yyyy}", Mode::Shell(Shell::Powershell));
        assert_eq!(text, "echo \"${env:WHEELOR_SELECTION}\" ${env:WHEELOR_ASK_1} ${env:WHEELOR_ASK_2} ${env:WHEELOR_DATE_1}");
        assert_eq!(env[0], ("WHEELOR_SELECTION".to_string(), "ciao & \"mondo\"".to_string()));
        assert_eq!(env[2], ("WHEELOR_ASK_2".to_string(), "<Età?>".to_string()));
        assert_eq!(env.len(), 4);
        assert_eq!(fill("echo {clipboard}{clipboard}", Mode::Shell(Shell::Cmd)), (
            "echo !WHEELOR_CLIPBOARD!!WHEELOR_CLIPBOARD!".to_string(),
            vec![("WHEELOR_CLIPBOARD".to_string(), "CLIPBOARD".to_string())]
        ));
    }

    #[test]
    fn mentions_and_urls() {
        assert!(mentions("a {selection} b", "selection"));
        assert!(!mentions("a {selections} b", "selection"));
        assert!(mentions("{date:dd/MM}", "date"));
        assert!(!mentions("{ selection }", "selection"));
        assert!(is_url("https://www.google.com/search?q={selection}"));
        assert!(is_url("mailto:?body={selection}"));
        assert!(is_url("ms-teams:"));
        assert!(!is_url(r"C:\Documenti\{date}.txt"));
        assert!(!is_url("notepad.exe"));
    }

    #[test]
    fn dates_are_formatted() {
        assert_eq!(date(Some("yyyy")).len(), 4);
        assert!(!date(None).is_empty());
        assert!(time(Some("HH:mm")).contains(':'));
    }
}
