// App GUI: niente finestra console all'avvio.
#![windows_subsystem = "windows"]

mod actions;
mod autostart;
mod config;
mod crash;
mod hotkey;
mod i18n;
mod icons;
mod input;
mod keys;
mod overlay;
mod package;
mod settings;
mod tray;
mod update;
mod wheel;

use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, PCWSTR, Result, w};

pub const MAIN_CLASS: PCWSTR = w!("WheelorMain");

/// Inviato dalla finestra delle impostazioni dopo aver salvato: ricarica la configurazione.
pub const WM_RELOAD_CONFIG: u32 = WM_APP + 20;
/// Inviato da una seconda copia avviata dal menu Start: apre le impostazioni.
const WM_OPEN_SETTINGS: u32 = WM_APP + 21;
/// Inviato dal thread che controlla gli aggiornamenti quando trova una nuova versione (in `FOUND`).
const WM_UPDATE_FOUND: u32 = WM_APP + 22;

/// Timer che riprova ad aggiungere l'icona nella tray: all'accesso a Windows
/// la barra delle applicazioni potrebbe non essere ancora pronta.
const TIMER_TRAY: usize = 2;
const TRAY_RETRY_MS: u32 = 2000;
const TRAY_ATTEMPTS: u32 = 30;

/// Timer del controllo degli aggiornamenti: il primo poco dopo l'avvio (non subito, per non
/// rallentare l'accesso a Windows), poi a intervalli regolari finché Wheelor resta acceso.
const TIMER_UPDATE: usize = 3;
const FIRST_CHECK_MS: u32 = 20_000;
const CHECK_EVERY_MS: u32 = 6 * 60 * 60 * 1000;

/// L'ultima versione trovata dal thread di controllo, in attesa che il thread UI la annunci.
static FOUND: Mutex<Option<String>> = Mutex::new(None);

/// Cosa dire quando compare l'icona.
#[derive(Clone, Copy, PartialEq)]
enum Greeting {
    None,
    /// Avviato a mano (non all'accesso): una notifica dice come si usa.
    Welcome,
    /// Riavviato dall'installer dopo un aggiornamento.
    Updated,
}

thread_local! {
    /// Data di modifica del file di configurazione caricato.
    static CONFIG_LOADED: Cell<Option<SystemTime>> = const { Cell::new(None) };
    static TRAY_TRIES: Cell<u32> = const { Cell::new(0) };
    static GREETING: Cell<Greeting> = const { Cell::new(Greeting::None) };
    /// Il controllo periodico degli aggiornamenti è attivo.
    static CHECKING: Cell<bool> = const { Cell::new(false) };
    /// L'ultima versione annunciata: la stessa non viene annunciata due volte.
    static ANNOUNCED: RefCell<String> = const { RefCell::new(String::new()) };
}

// Riga di comando:
//   wheelor.exe               avvia Wheelor; se è già attivo, apre le impostazioni
//   wheelor.exe --background  avvia Wheelor senza notifiche (avvio automatico con Windows)
//               --updated     (con --background) dopo un aggiornamento: lo dice con una notifica
//   wheelor.exe --settings    finestra delle impostazioni (processo separato)
//               --update      (con --settings) propone subito di installare la nuova versione
//   wheelor.exe --quit        chiude Wheelor in esecuzione (installer, script)
fn main() -> Result<()> {
    crash::install();
    // La lingua di Windows finché non si legge la configurazione (che può sceglierne un'altra):
    // al primo avvio la configurazione di esempio nasce in questa lingua.
    i18n::set(None);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if has("--settings") {
        settings::run();
        return Ok(());
    }
    if has("--quit") {
        quit_running();
        return Ok(());
    }

    // Istanza unica: se il mutex esiste già, Wheelor è già in esecuzione.
    let _mutex = unsafe { CreateMutexW(None, false, w!("Local\\Wheelor.SingleInstance"))? };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // Chi lo riapre dal menu Start vuole le impostazioni; l'avvio automatico non vuole niente.
        if !has("--background")
            && let Ok(hwnd) = unsafe { FindWindowW(MAIN_CLASS, PCWSTR::null()) }
        {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
            }
        }
        return Ok(());
    }

    // Di solito è già impostato dal manifest dell'exe (vedi build.rs): in quel caso la chiamata
    // fallisce senza conseguenze.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    GREETING.set(match (has("--background"), has("--updated")) {
        (_, true) => Greeting::Updated,
        (false, false) => Greeting::Welcome,
        (true, false) => Greeting::None,
    });
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let hwnd = create_main_window(instance)?;
    overlay::init(instance)?;
    apply_config(hwnd);
    add_tray(hwnd);

    // Message loop: GetMessageW resta bloccata finché non arriva un messaggio,
    // quindi a riposo il processo usa 0% di CPU. Restituisce 0 su WM_QUIT, -1 su errore.
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    overlay::shutdown();
    Ok(())
}

/// Finestra mai mostrata: serve solo a ricevere i messaggi (tray, hotkey, input).
fn create_main_window(instance: HINSTANCE) -> Result<HWND> {
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: MAIN_CLASS,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(Error::from_thread());
        }
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            MAIN_CLASS,
            w!("Wheelor"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    }
}

/// Windows chiama questa funzione per ogni messaggio destinato alla finestra.
/// WM_CLOSE (da `--quit` o dall'installer) arriva a DefWindowProcW, che distrugge la finestra.
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        tray::WM_TRAY => {
            match tray::on_message(hwnd, wparam, lparam) {
                Some(tray::Command::Settings) => open_settings(hwnd, false),
                Some(tray::Command::Update) => open_settings(hwnd, true),
                Some(tray::Command::Exit) => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                None => {}
            }
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == hotkey::ID_WHEEL => {
            if wheel::is_open() {
                wheel::close();
            } else {
                // Il file potrebbe essere stato modificato a mano: in tal caso lo ricarichiamo.
                if config::modified() != CONFIG_LOADED.get() {
                    apply_config(hwnd);
                }
                if let Err(e) = wheel::open(hwnd) {
                    wheel::close();
                    tray::notify(hwnd, tr!("Impossibile aprire la ruota", "Can't open the wheel"), &e);
                }
            }
            LRESULT(0)
        }
        input::WM_WHEEL_MOVE..=input::WM_WHEEL_CONFIRM => {
            wheel::on_input(hwnd, msg, wparam, lparam);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == wheel::TIMER_DWELL => {
            wheel::on_timer(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_TRAY => {
            add_tray(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_UPDATE => {
            check_for_update(hwnd);
            LRESULT(0)
        }
        WM_UPDATE_FOUND => {
            let found = FOUND.lock().map(|mut f| f.take()).unwrap_or_default();
            if let Some(version) = found
                && ANNOUNCED.with_borrow(|a| *a != version)
            {
                tray::offer_update(hwnd, &version);
                ANNOUNCED.set(version);
            }
            LRESULT(0)
        }
        WM_RELOAD_CONFIG => {
            apply_config(hwnd);
            LRESULT(0)
        }
        WM_OPEN_SETTINGS => {
            open_settings(hwnd, false);
            LRESULT(0)
        }
        // Disconnessione, spegnimento o un installer (Gestione riavvio) chiedono di chiudere:
        // usciamo in modo pulito invece di aspettare di essere terminati.
        WM_ENDSESSION if wparam.0 != 0 => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            wheel::close();
            hotkey::unregister(hwnd);
            tray::remove(hwnd);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if m == tray::taskbar_created() => {
            // Explorer è ripartito (o è appena partito): l'icona va aggiunta di nuovo.
            TRAY_TRIES.set(0);
            add_tray(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Legge la configurazione e applica hotkey e ruote.
/// Se il file contiene errori, resta in uso la configurazione precedente.
fn apply_config(hwnd: HWND) {
    match config::load() {
        Ok(config) => {
            i18n::set(config.language);
            if let Err(e) = hotkey::register(hwnd, &config.hotkey) {
                tray::notify(hwnd, tr!("Scorciatoia non valida", "Invalid shortcut"), &e);
            }
            // La versione dello Store la aggiorna lo Store.
            schedule_update_checks(hwnd, config.check_updates && !package::is_store());
            wheel::set_config(config);
        }
        Err(e) => tray::notify(hwnd, tr!("Errore nella configurazione", "Configuration error"), &e),
    }
    // Dopo `load`: se ha convertito il vecchio formato, il file è appena stato riscritto.
    CONFIG_LOADED.set(config::modified());
}

/// Aggiunge l'icona nella tray. Se la barra delle applicazioni non è pronta, riprova ogni
/// TRAY_RETRY_MS (al massimo TRAY_ATTEMPTS volte); arriverà comunque anche TaskbarCreated.
fn add_tray(hwnd: HWND) {
    if tray::add(hwnd).is_ok() {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_TRAY);
        }
        let hotkey = hotkey::current();
        match GREETING.replace(Greeting::None) {
            Greeting::Welcome => {
                let text = trf!(
                    "Premi {hotkey} per aprire la ruota. Clic sull'icona per personalizzarla.",
                    "Press {hotkey} to open the wheel. Click the icon to customize it."
                );
                tray::notify(hwnd, tr!("Wheelor è attivo", "Wheelor is running"), &text);
            }
            Greeting::Updated => {
                let version = update::current();
                let title = trf!("Wheelor è aggiornato alla versione {version}", "Wheelor has been updated to version {version}");
                let text = trf!(
                    "Tutto come prima: premi {hotkey} per aprire la ruota.",
                    "Everything is as before: press {hotkey} to open the wheel."
                );
                tray::notify(hwnd, &title, &text);
            }
            Greeting::None => {}
        }
        return;
    }
    let tries = TRAY_TRIES.get() + 1;
    TRAY_TRIES.set(tries);
    unsafe {
        if tries == 1 {
            SetTimer(Some(hwnd), TIMER_TRAY, TRAY_RETRY_MS, None);
        } else if tries >= TRAY_ATTEMPTS {
            let _ = KillTimer(Some(hwnd), TIMER_TRAY);
        }
    }
}

/// `--quit`: chiede a Wheelor in esecuzione di chiudersi e aspetta (al massimo 5 secondi).
fn quit_running() {
    let running = || unsafe { FindWindowW(MAIN_CLASS, PCWSTR::null()) }.ok();
    let Some(hwnd) = running() else { return };
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while running().is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Icona dell'app (risorsa 1 dell'exe, vedi build.rs) alla dimensione di sistema `metric`
/// (SM_CXSMICON piccola, SM_CXICON grande) per i DPI attuali.
pub fn app_icon(metric: SYSTEM_METRICS_INDEX) -> Option<HICON> {
    unsafe {
        let size = GetSystemMetricsForDpi(metric, GetDpiForSystem());
        let instance = GetModuleHandleW(None).ok()?;
        // MAKEINTRESOURCE(1): una risorsa si può indicare con un numero al posto del nome.
        let id = PCWSTR(1 as *const u16);
        let handle = LoadImageW(Some(instance.into()), id, IMAGE_ICON, size, size, LR_DEFAULTCOLOR).ok()?;
        Some(HICON(handle.0))
    }
}

/// Avvia (o riattiva) il controllo periodico degli aggiornamenti, oppure lo ferma.
fn schedule_update_checks(hwnd: HWND, enabled: bool) {
    if CHECKING.replace(enabled) == enabled {
        return;
    }
    unsafe {
        if enabled {
            SetTimer(Some(hwnd), TIMER_UPDATE, FIRST_CHECK_MS, None);
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_UPDATE);
        }
    }
}

/// Controlla su un altro thread (la rete può essere lenta); se c'è una versione nuova, la
/// mette in `FOUND` e avvisa il thread UI. Gli errori si ignorano: si riprova al prossimo giro.
fn check_for_update(hwnd: HWND) {
    // Il prossimo controllo tra CHECK_EVERY_MS (stesso id: sostituisce il timer del primo).
    unsafe {
        SetTimer(Some(hwnd), TIMER_UPDATE, CHECK_EVERY_MS, None);
    }
    let target = hwnd.0 as usize; // un HWND non passa tra thread: ne passiamo il valore
    std::thread::spawn(move || {
        update::clean_up();
        if let Ok(Some(release)) = update::check() {
            if let Ok(mut found) = FOUND.lock() {
                *found = Some(release.version);
            }
            unsafe {
                let _ = PostMessageW(Some(HWND(target as *mut _)), WM_UPDATE_FOUND, WPARAM(0), LPARAM(0));
            }
        }
    });
}

/// Apre la finestra delle impostazioni; con `update`, propone subito l'aggiornamento.
fn open_settings(hwnd: HWND, update: bool) {
    let args: &[&str] = if update { &["--settings", "--update"] } else { &["--settings"] };
    if let Err(e) = package::launch_self(args) {
        tray::notify(hwnd, tr!("Impossibile aprire le impostazioni", "Can't open the settings"), &e);
    }
}
