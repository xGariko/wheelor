//! Aggiornamenti. A ogni release il sito pubblica https://wheelor.app/latest.json con versione,
//! indirizzo, peso e hash dell'installer (vedi installer/build.ps1 e il README nella radice).
//! Wheelor lo legge e, se c'è una versione più nuova, la propone. Aggiornare vuol dire scaricare
//! l'installer dalla GitHub Release, controllarne l'hash e avviarlo in modalità silenziosa:
//! chiude Wheelor, sostituisce l'exe e lo fa ripartire (vedi installer/wheelor.iss).

mod http;

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};

use crate::{tr, trf};

const LATEST_URL: &str = "https://wheelor.app/latest.json";
/// Gli installer si scaricano solo da qui: anche se il sito venisse manomesso,
/// Wheelor non eseguirebbe un programma preso altrove.
const DOWNLOADS: &str = "https://github.com/xGariko/wheelor/releases/download/";
const RELEASES: &str = "https://github.com/xGariko/wheelor/releases";
/// Prefisso dei file scaricati nella cartella temporanea.
const SETUP_PREFIX: &str = "Wheelor-Setup-";

/// Il contenuto di latest.json.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Release {
    pub version: String,
    pub installer: Installer,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Installer {
    pub url: String,
    /// Byte.
    pub size: u64,
    /// In esadecimale.
    pub sha256: String,
}

impl Release {
    /// La pagina della release su GitHub, con le novità.
    pub fn notes_url(&self) -> String {
        format!("{RELEASES}/tag/v{}", self.version)
    }

    /// Il peso dell'installer, es. "6,3 MB".
    pub fn size_text(&self) -> String {
        let text = format!("{:.1} MB", self.installer.size as f64 / 1_048_576.0);
        if crate::i18n::english() { text } else { text.replace('.', ",") }
    }
}

/// La versione in uso, es. "0.2.0".
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// La versione più nuova, se è più recente di quella in uso. Aspetta la rete (al massimo una
/// trentina di secondi): va chiamata da un thread a parte.
pub fn check() -> Result<Option<Release>, String> {
    let body = http::get(LATEST_URL, |_, _| {})?;
    let release: Release =
        serde_json::from_slice(&body).map_err(|e| trf!("risposta di wheelor.app non valida ({e})", "invalid answer from wheelor.app ({e})"))?;
    Ok(is_newer(&release.version, current()).then_some(release))
}

/// Scarica l'installer nella cartella temporanea e controlla che sia integro.
/// `progress(scaricati, totali)` viene chiamata durante il download.
pub fn download(release: &Release, progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    let url = &release.installer.url;
    if !url.starts_with(DOWNLOADS) {
        return Err(trf!("indirizzo di download inatteso: {url}", "unexpected download address: {url}"));
    }
    // La versione diventa parte del nome del file: solo caratteri innocui.
    if !release.version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(trf!("versione non valida: {}", "invalid version: {}", release.version));
    }
    let data = http::get(url, progress)?;
    if !sha256(&data)?.eq_ignore_ascii_case(release.installer.sha256.trim()) {
        return Err(tr!("il file scaricato è danneggiato, riprova più tardi", "the downloaded file is damaged, try again later").into());
    }
    let path = std::env::temp_dir().join(format!("{SETUP_PREFIX}{}.exe", release.version));
    fs::write(&path, data).map_err(|e| trf!("impossibile salvare {}: {e}", "can't save {}: {e}", path.display()))?;
    Ok(path)
}

/// Avvia l'installer in modalità silenziosa: mostra solo l'avanzamento, chiude Wheelor,
/// lo aggiorna e lo fa ripartire. Le impostazioni dell'utente restano dove sono.
pub fn install(setup: &Path) -> Result<(), String> {
    std::process::Command::new(setup)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .spawn()
        .map(drop)
        .map_err(|e| trf!("impossibile avviare l'installazione: {e}", "can't start the installation: {e}"))
}

/// Elimina gli installer scaricati per gli aggiornamenti precedenti (quello in uso, se c'è,
/// è bloccato e resta: sparirà la volta dopo).
pub fn clean_up() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(SETUP_PREFIX) && name.ends_with(".exe") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Hash SHA-256 in esadecimale, calcolato da Windows (CNG).
fn sha256(data: &[u8]) -> Result<String, String> {
    let mut hash = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut hash) }
        .ok()
        .map_err(|e| trf!("impossibile controllare il file scaricato ({})", "can't check the downloaded file ({})", e.message()))?;
    Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
}

/// `a` è una versione più recente di `b`? Formato "1.2.3" con un'eventuale pre-release,
/// "1.2.3-beta.1", che viene prima della versione finale (come in semver).
pub fn is_newer(a: &str, b: &str) -> bool {
    compare(a, b) == Ordering::Greater
}

fn compare(a: &str, b: &str) -> Ordering {
    let split = |v: &str| {
        let v = v.trim().trim_start_matches('v');
        match v.split_once('-') {
            Some((core, pre)) => (core.to_string(), Some(pre.to_string())),
            None => (v.to_string(), None),
        }
    };
    let ((core_a, pre_a), (core_b, pre_b)) = (split(a), split(b));
    let numbers = |core: &str| core.split('.').map(|n| n.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (mut na, mut nb) = (numbers(&core_a), numbers(&core_b));
    let len = na.len().max(nb.len());
    na.resize(len, 0);
    nb.resize(len, 0);
    na.cmp(&nb).then_with(|| match (pre_a, pre_b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => compare_pre(&x, &y),
    })
}

/// Confronta le pre-release parte per parte: i numeri come numeri, il resto come testo,
/// e un numero viene prima di una parola ("beta.2" < "beta.10" < "rc.1").
fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_ordered_like_semver() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0-beta.1"));
        assert!(is_newer("0.1.0", "0.1.0-beta.1"));
        assert!(is_newer("0.1.0-beta.2", "0.1.0-beta.1"));
        assert!(is_newer("0.1.0-beta.10", "0.1.0-beta.2"));
        assert!(is_newer("0.1.0-rc.1", "0.1.0-beta.5"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta.1", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0-beta.1"));
    }

    #[test]
    fn latest_json_is_understood() {
        let text = r#"{
            "version": "0.2.0",
            "date": "2026-10-20",
            "installer": {
                "file": "Wheelor-Setup-0.2.0.exe",
                "url": "https://github.com/xGariko/wheelor/releases/download/v0.2.0/Wheelor-Setup-0.2.0.exe",
                "size": 6605443,
                "sha256": "98f275ecde12da94d3fecc95ac46a8cce606c2bd6d3b7b9557ad22b332c3c0c8"
            }
        }"#;
        let release: Release = serde_json::from_str(text).unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.size_text(), "6,3 MB");
        assert_eq!(release.notes_url(), "https://github.com/xGariko/wheelor/releases/tag/v0.2.0");
    }

    #[test]
    fn sha256_matches_the_standard() {
        assert_eq!(sha256(b"abc").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn downloads_only_from_github() {
        let release = Release {
            version: "9.9.9".into(),
            installer: Installer { url: "https://example.com/setup.exe".into(), size: 1, sha256: String::new() },
        };
        assert!(download(&release, |_, _| {}).unwrap_err().contains("inatteso"));
    }

    /// Legge davvero https://wheelor.app/latest.json: `cargo test real_check -- --ignored --nocapture`.
    #[test]
    #[ignore = "usa la rete"]
    fn real_check() {
        let body = http::get(LATEST_URL, |_, _| {}).unwrap();
        let release: Release = serde_json::from_slice(&body).unwrap();
        println!("{} → {release:?} (più nuova: {})", current(), is_newer(&release.version, current()));
    }

    /// Scarica davvero l'ultimo installer e ne controlla l'hash (poi lo elimina):
    /// `cargo test real_download -- --ignored --nocapture`.
    #[test]
    #[ignore = "usa la rete e scarica qualche MB"]
    fn real_download() {
        let body = http::get(LATEST_URL, |_, _| {}).unwrap();
        let release: Release = serde_json::from_slice(&body).unwrap();
        let mut calls = 0;
        let path = download(&release, |done, total| {
            calls += 1;
            assert!(done <= total);
        })
        .unwrap();
        println!("{} ({} blocchi)", path.display(), calls);
        assert_eq!(fs::metadata(&path).unwrap().len(), release.installer.size);
        fs::remove_file(path).unwrap();
    }
}
