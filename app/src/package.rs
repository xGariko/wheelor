//! La versione del Microsoft Store. È lo stesso exe, impacchettato in MSIX (vedi
//! installer/msix): Windows gli dà una "package identity" e lo installa in
//! C:\Program Files\WindowsApps. Rispetto all'installer cambiano poche cose:
//! - gli aggiornamenti li fa lo Store (le sue regole vietano di aggiornarsi da soli);
//! - l'avvio con Windows è un "startup task" dichiarato nel pacchetto, non la chiave Run;
//! - i file nuovi in %APPDATA% finiscono in una cartella privata del pacchetto, che Windows
//!   unisce a quella vera solo per Wheelor (Esplora file la vede altrove).

use std::sync::OnceLock;

use windows::ApplicationModel::{StartupTask, StartupTaskState};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Shell::{AO_NONE, ApplicationActivationManager, IApplicationActivationManager};
use windows::core::{HSTRING, PWSTR};

use crate::tr;

/// Id dell'app e dello startup task nel manifest del pacchetto (installer/msix/AppxManifest.xml).
const APP_ID: &str = "Wheelor";
const STARTUP_TASK: &str = "WheelorStartup";

/// Il "family name" del pacchetto (es. "12345Nome.Wheelor_abcdef"), se Wheelor gira come app
/// dello Store con la sua identità.
pub fn family_name() -> Option<&'static str> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(|| unsafe {
        let mut length = 0u32;
        // Prima chiamata: chiede solo la lunghezza. Senza pacchetto risponde APPMODEL_ERROR_NO_PACKAGE.
        if GetCurrentPackageFamilyName(&mut length, None) != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
        let mut buffer = vec![0u16; length as usize];
        GetCurrentPackageFamilyName(&mut length, Some(PWSTR(buffer.as_mut_ptr()))).ok().ok()?;
        Some(String::from_utf16_lossy(&buffer[..length.saturating_sub(1) as usize]))
    })
    .as_deref()
}

/// Wheelor è la versione dello Store? Vale anche per un processo che avesse perso l'identità
/// del pacchetto: l'exe sta comunque nella cartella delle app dello Store.
pub fn is_store() -> bool {
    static STORE: OnceLock<bool> = OnceLock::new();
    *STORE.get_or_init(|| {
        family_name().is_some()
            || std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().to_lowercase().contains("\\windowsapps\\"))
    })
}

/// Avvia un'altra copia di Wheelor con questi argomenti. Nella versione dello Store passa da
/// Windows (come il menu Start), così la nuova copia ha di sicuro l'identità del pacchetto.
pub fn launch_self(args: &[&str]) -> Result<(), String> {
    if let Some(family) = family_name() {
        let activated = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            CoCreateInstance::<_, IApplicationActivationManager>(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                .and_then(|manager| {
                    let app = HSTRING::from(format!("{family}!{APP_ID}"));
                    manager.ActivateApplication(&app, &HSTRING::from(args.join(" ")), AO_NONE)
                })
        };
        if activated.is_ok() {
            return Ok(());
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).args(args).spawn().map(drop).map_err(|e| e.to_string())
}

/// L'avvio con Windows della versione dello Store è attivo?
pub fn startup_enabled() -> bool {
    // Le chiamate WinRT aspettano un'operazione asincrona: su un thread a parte, così non
    // bloccano mai il thread dell'interfaccia.
    std::thread::spawn(|| {
        let task = StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK))?.join()?;
        task.State()
    })
    .join()
    .ok()
    .and_then(Result::ok)
    .is_some_and(|state| matches!(state, StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy))
}

pub fn set_startup(enabled: bool) -> Result<(), String> {
    let result = std::thread::spawn(move || -> windows::core::Result<StartupTaskState> {
        let task = StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK))?.join()?;
        if enabled {
            task.RequestEnableAsync()?.join()
        } else {
            task.Disable()?;
            task.State()
        }
    })
    .join()
    .map_err(|_| tr!("errore imprevisto", "unexpected error").to_string())?;
    match result {
        Ok(StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy) if enabled => Ok(()),
        Ok(_) if !enabled => Ok(()),
        // Disattivato a mano dall'utente (Task Manager o Impostazioni): solo lui può riattivarlo.
        Ok(StartupTaskState::DisabledByUser) => {
            Err(tr!(
                "è stato disattivato in Impostazioni > App > Avvio: riattivalo da lì",
                "it was turned off in Settings > Apps > Startup: turn it back on there"
            )
            .into())
        }
        Ok(_) => Err(tr!("Windows non lo consente (criteri di sistema)", "Windows doesn't allow it (system policy)").into()),
        Err(e) => Err(e.message()),
    }
}

/// La cartella che contiene davvero la configurazione, come la vede Esplora file. Nella versione
/// dello Store i file creati in %APPDATA% stanno nella cartella privata del pacchetto, a meno
/// che non esistessero già (per esempio da una vecchia installazione con l'installer).
pub fn visible_path(path: std::path::PathBuf) -> std::path::PathBuf {
    let (Some(family), Some(appdata), Some(local)) =
        (family_name(), std::env::var_os("APPDATA"), std::env::var_os("LOCALAPPDATA"))
    else {
        return path;
    };
    let Ok(relative) = path.strip_prefix(&appdata) else { return path };
    let private =
        std::path::Path::new(&local).join("Packages").join(family).join("LocalCache").join("Roaming").join(relative);
    if private.exists() { private } else { path }
}
