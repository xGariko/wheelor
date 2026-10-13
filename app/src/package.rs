//! La versione del Microsoft Store. È lo stesso exe, impacchettato in MSIX (vedi
//! installer/msix): Windows gli dà una "package identity" e lo installa in
//! C:\Program Files\WindowsApps. Rispetto all'installer cambiano poche cose:
//! - gli aggiornamenti li fa lo Store (le sue regole vietano di aggiornarsi da soli);
//! - l'avvio con Windows è un "startup task" dichiarato nel pacchetto, non la chiave Run;
//! - i file nuovi in %APPDATA% finiscono in una cartella privata del pacchetto, che Windows
//!   unisce a quella vera solo per Wheelor (Esplora file la vede altrove).
//!
//! Le due versioni possono essere installate insieme (chi aveva l'installer e poi scarica dallo
//! Store): allora comanda quella dello Store. La copia dell'installer le passa la mano appena
//! parte (`store_installed`, `hand_over_to_store`) e non si aggiorna più da sola
//! (`can_self_update`); quella dello Store chiude una copia dell'installer già in esecuzione, che
//! nelle versioni fino alla 0.3.0 non sa farsi da parte (`is_installer_copy`, vedi main).

use std::path::Path;
use std::sync::OnceLock;

use windows::ApplicationModel::{StartupTask, StartupTaskState};
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HWND};
use windows::Win32::Storage::Packaging::Appx::{GetCurrentPackageFamilyName, GetPackagesByPackageFamily};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::{AO_NONE, ApplicationActivationManager, IApplicationActivationManager};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::{HSTRING, PWSTR};

use crate::tr;

/// Id dell'app e dello startup task nel manifest del pacchetto (installer/msix/AppxManifest.xml).
const APP_ID: &str = "Wheelor";
const STARTUP_TASK: &str = "WheelorStartup";
/// Il family name del pacchetto dello Store: il nome di installer/msix/identity.json più un hash
/// dell'editore, lo stesso per tutte le versioni (Partner Center > Identità del prodotto).
const STORE_FAMILY: &str = "GabrieleIovino.Wheelor_4s4a32g66kqd2";

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

/// La versione dello Store è installata per questo utente? (Per la copia dell'installer.)
pub fn store_installed() -> bool {
    let (mut count, mut length) = (0u32, 0u32);
    // Senza spazio per i nomi risponde solo quanti pacchetti di questa famiglia ci sono.
    let _ = unsafe { GetPackagesByPackageFamily(&HSTRING::from(STORE_FAMILY), &mut count, None, &mut length, None) };
    count > 0
}

/// Wheelor può aggiornarsi da solo, scaricando e avviando l'installer? Non nella versione dello
/// Store (la aggiorna lo Store), e nemmeno in una copia dell'installer quando è installata anche
/// quella dello Store: l'installer rimetterebbe in piedi la seconda copia.
pub fn can_self_update() -> bool {
    !is_store() && !store_installed()
}

/// Questo exe l'ha installato l'installer (Inno Setup lascia accanto il suo disinstallatore)?
/// Le build di sviluppo in target\ no: non passano la mano allo Store e lo Store non le chiude.
pub fn is_installer_copy(exe: &Path) -> bool {
    exe.parent().is_some_and(|dir| dir.join("unins000.exe").exists())
}

/// L'exe del processo che possiede la finestra `hwnd`.
pub fn window_exe(hwnd: HWND) -> Option<std::path::PathBuf> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..length as usize]).into())
    }
}

/// Avvia l'app `family` del pacchetto con questi argomenti, passando da Windows come il menu Start.
fn activate(family: &str, args: &[&str]) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)?;
        let app = HSTRING::from(format!("{family}!{APP_ID}"));
        manager.ActivateApplication(&app, &HSTRING::from(args.join(" ")), AO_NONE)?;
    }
    Ok(())
}

/// La copia dell'installer passa la mano a quella dello Store: la avvia con questi argomenti.
/// `false` se non ci riesce (allora continua lei, come sempre).
pub fn hand_over_to_store(args: &[&str]) -> bool {
    activate(STORE_FAMILY, args).is_ok()
}

/// Avvia un'altra copia di Wheelor con questi argomenti. Nella versione dello Store passa da
/// Windows (come il menu Start), così la nuova copia ha di sicuro l'identità del pacchetto.
pub fn launch_self(args: &[&str]) -> Result<(), String> {
    if let Some(family) = family_name()
        && activate(family, args).is_ok()
    {
        return Ok(());
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
