//! Esecuzione delle azioni: i passi, uno dopo l'altro, su un thread tutto loro (l'esecutore),
//! così né la ruota né l'interfaccia restano mai bloccate. Un'azione avviata mentre un'altra è
//! in corso aspetta il suo turno: due azioni che premono tasti insieme si mescolerebbero.
//!
//! Esc ferma l'azione in corso e quelle in attesa (`cancel`; il tasto lo guarda `input::watch_escape`
//! mentre l'esecutore lavora, vedi `on_busy`).

mod clipboard;
mod placeholders;
pub mod window;

use std::collections::HashMap;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::*;
use windows::Win32::Globalization::{GetOEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use crate::config::{ClipboardOp, Item, Shell, Step, SystemOp, WindowMode};
use crate::keys::{self, Combo};
use crate::{tr, trf};
use placeholders::Mode;

/// Quante azioni una dentro l'altra (passi "Esegui azione") al massimo: i cicli li esclude già
/// `Config::check`, questo è solo un paracadute.
const MAX_DEPTH: usize = 8;

/// Informazioni sulla finestra in primo piano quando si è aperta la ruota.
/// I comandi le ricevono come variabili d'ambiente WHEELOR_APP, WHEELOR_TITLE e WHEELOR_PROFILE.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Percorso dell'eseguibile, es. C:\Program Files\...\Code.exe
    pub app: String,
    pub title: String,
    /// Nome della ruota di partenza (la principale o il profilo dell'app).
    pub profile: String,
    /// La finestra stessa, come numero: un HWND contiene un puntatore e non passa tra thread.
    pub window: isize,
}

impl Context {
    pub fn capture() -> Self {
        unsafe {
            let hwnd = GetForegroundWindow();
            let mut title = [0u16; 512];
            let len = GetWindowTextW(hwnd, &mut title).max(0) as usize;
            Self {
                app: process_path(hwnd).unwrap_or_default(),
                title: String::from_utf16_lossy(&title[..len]),
                profile: String::new(),
                window: hwnd.0 as isize,
            }
        }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.window as *mut _)
    }
}

/// Un'azione da eseguire, con tutto ciò che le serve.
pub struct Job {
    pub label: String,
    pub steps: Vec<Step>,
    /// La libreria, per i passi "Esegui azione".
    pub library: Vec<Item>,
    pub context: Context,
}

impl Job {
    pub fn new(item: &Item, library: &[Item], context: Context) -> Self {
        Self { label: item.label.clone(), steps: item.steps.clone(), library: library.to_vec(), context }
    }

    /// Qualche passo (anche di un'azione eseguita da un passo) lavora sulla finestra in primo
    /// piano: tasti, testo, appunti, finestre. Provandola dalle impostazioni conviene aspettare
    /// che l'utente passi all'app giusta.
    pub fn uses_foreground(&self) -> bool {
        fn check(steps: &[Step], library: &[Item], depth: usize) -> bool {
            steps.iter().any(|step| match step {
                Step::Keys { .. } | Step::Text { .. } | Step::Clipboard { .. } | Step::Window { .. } => true,
                Step::Open { target, args } => placeholders::mentions(target, "selection") || placeholders::mentions(args, "selection"),
                Step::Command { command, .. } => placeholders::mentions(command, "selection"),
                Step::Run { action } => {
                    depth < MAX_DEPTH
                        && library.iter().find(|a| a.id == *action).is_some_and(|a| check(&a.steps, library, depth + 1))
                }
                _ => false,
            })
        }
        check(&self.steps, &self.library, 0)
    }
}

/// Perché un'azione non è arrivata in fondo: titolo e testo della notifica.
#[derive(Debug)]
pub struct Failure {
    pub title: String,
    pub message: String,
}

/// Perché un passo si è fermato.
enum Stop {
    /// Esc, oppure "Annulla" alla domanda di un `{ask:…}`: niente notifica.
    Cancelled,
    Failed(String),
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Stop::Failed(message)
    }
}

type Task = Box<dyn FnOnce() + Send>;

/// La coda dell'esecutore: il thread parte alla prima azione.
static QUEUE: OnceLock<Mutex<mpsc::Sender<Task>>> = OnceLock::new();
/// Cresce a ogni `cancel`: un'azione accodata prima si accorge di essere stata fermata.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Avvisato quando l'esecutore inizia (true) e finisce (false) un'azione.
static BUSY: OnceLock<Box<dyn Fn(bool) + Send + Sync>> = OnceLock::new();

/// `callback(true)` quando l'esecutore inizia un'azione, `callback(false)` quando la finisce.
pub fn on_busy(callback: impl Fn(bool) + Send + Sync + 'static) {
    let _ = BUSY.set(Box::new(callback));
}

/// Mette l'azione in coda. Se non arriva in fondo, `on_error` riceve il perché (dal thread
/// dell'esecutore).
pub fn run(job: Job, on_error: impl FnOnce(Failure) + Send + 'static) {
    let generation = GENERATION.load(Ordering::SeqCst);
    let task: Task = Box::new(move || {
        if let Err(failure) = execute(&job, generation) {
            on_error(failure);
        }
    });
    let queue = QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Task>();
        thread::spawn(move || {
            // ShellExecuteEx può usare COM: il thread che lo chiama deve inizializzarlo.
            let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            for task in receiver {
                let busy = |state| {
                    if let Some(callback) = BUSY.get() {
                        callback(state);
                    }
                };
                busy(true);
                task();
                busy(false);
            }
        });
        Mutex::new(sender)
    });
    if let Ok(sender) = queue.lock() {
        let _ = sender.send(task);
    }
}

/// Esegue l'azione su questo thread e aspetta che finisca (`wheelor.exe --run` senza Wheelor attivo).
pub fn run_here(job: Job) -> Result<(), Failure> {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    execute(&job, GENERATION.load(Ordering::SeqCst))
}

/// Ferma l'azione in corso e quelle in coda.
pub fn cancel() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

fn execute(job: &Job, generation: u64) -> Result<(), Failure> {
    let mut runner = Runner { job, generation, selection: String::new(), result: String::new(), answers: HashMap::new(), depth: 0 };
    if runner.cancelled() {
        return Ok(());
    }
    // La selezione si copia subito, prima che qualche passo cambi la finestra in primo piano.
    if runner.mentions(&job.steps, "selection", 0) {
        runner.capture_selection();
    }
    let total = job.steps.len();
    for (index, step) in job.steps.iter().enumerate() {
        match runner.step(step) {
            Ok(()) => {}
            Err(Stop::Cancelled) => return Ok(()),
            Err(Stop::Failed(message)) => {
                let label = &job.label;
                return Err(match total {
                    1 => Failure { title: trf!("{label}: azione non riuscita", "{label}: action failed"), message },
                    _ => Failure {
                        title: trf!("{label}: passo {} di {total} non riuscito", "{label}: step {} of {total} failed", index + 1),
                        message: format!("{} · {message}", step.name()),
                    },
                });
            }
        }
        if runner.cancelled() {
            return Ok(());
        }
    }
    Ok(())
}

/// Lo stato di un'azione mentre gira: i valori dei segnaposto che non si leggono al momento.
struct Runner<'a> {
    job: &'a Job,
    generation: u64,
    /// Il testo selezionato quando è partita l'azione.
    selection: String,
    /// L'uscita dell'ultimo comando nascosto, per `{result}`.
    result: String,
    /// Le risposte già date ai `{ask:…}`: la stessa domanda non si fa due volte.
    answers: HashMap<String, String>,
    depth: usize,
}

impl Runner<'_> {
    fn cancelled(&self) -> bool {
        GENERATION.load(Ordering::SeqCst) != self.generation
    }

    /// Qualche passo usa il segnaposto `name`? Guarda anche dentro le azioni eseguite dai passi.
    fn mentions(&self, steps: &[Step], name: &str, depth: usize) -> bool {
        steps.iter().any(|step| match step {
            Step::Run { action } => {
                depth < MAX_DEPTH
                    && self.job.library.iter().find(|a| a.id == *action).is_some_and(|a| self.mentions(&a.steps, name, depth + 1))
            }
            step => step.texts().iter().any(|text| placeholders::mentions(text, name)),
        })
    }

    /// Copia la selezione con Ctrl+C e rimette negli appunti quello che c'era.
    fn capture_selection(&mut self) {
        let saved = clipboard::save();
        if clipboard::copy_selection(&|| self.cancelled()) {
            self.selection = clipboard::text();
            clipboard::restore(saved);
        }
    }

    /// Aspetta, controllando spesso se nel frattempo è stato premuto Esc.
    fn sleep(&self, duration: Duration) -> Result<(), Stop> {
        let deadline = std::time::Instant::now() + duration;
        while std::time::Instant::now() < deadline {
            if self.cancelled() {
                return Err(Stop::Cancelled);
            }
            thread::sleep(Duration::from_millis(20).min(deadline - std::time::Instant::now()));
        }
        Ok(())
    }

    /// Il valore di un segnaposto.
    fn value(&mut self, name: &str, arg: Option<&str>) -> Result<String, Stop> {
        let context = &self.job.context;
        Ok(match name {
            "selection" => self.selection.clone(),
            "clipboard" => clipboard::text(),
            "app" => context.app.clone(),
            "title" => context.title.clone(),
            "wheel" => context.profile.clone(),
            "date" => placeholders::date(arg),
            "time" => placeholders::time(arg),
            "result" => self.result.clone(),
            "ask" => self.ask(arg.unwrap_or_default())?,
            _ => String::new(),
        })
    }

    fn expand(&mut self, text: &str, mode: Mode) -> Result<(String, Vec<(String, String)>), Stop> {
        placeholders::expand(text, mode, |name, arg| self.value(name, arg))
    }

    fn expand_plain(&mut self, text: &str) -> Result<String, Stop> {
        Ok(self.expand(text, Mode::Plain)?.0)
    }

    /// Chiede un testo con una piccola finestra (`wheelor.exe --ask`, vedi `settings::ask`).
    fn ask(&mut self, question: &str) -> Result<String, Stop> {
        if let Some(answer) = self.answers.get(question) {
            return Ok(answer.clone());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let output = Command::new(exe)
            .arg("--ask")
            .arg(question)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| trf!("Impossibile chiedere «{question}»: {e}", "Can't ask “{question}”: {e}"))?;
        if !output.status.success() {
            return Err(Stop::Cancelled);
        }
        let answer = String::from_utf8_lossy(&output.stdout).trim_end_matches(['\r', '\n']).to_string();
        self.answers.insert(question.to_string(), answer.clone());
        Ok(answer)
    }

    fn step(&mut self, step: &Step) -> Result<(), Stop> {
        match step {
            Step::Command { command, shell, window, cwd, admin, wait } => {
                let (command, env) = self.expand(command, Mode::Shell(*shell))?;
                let cwd = self.expand_plain(&expand_env(cwd))?;
                let run = CommandRun { shell: *shell, window: *window, admin: *admin, wait: *wait };
                if let Some(output) = run.start(&command, env, &cwd, &self.job.context, &|| self.cancelled())? {
                    self.result = output;
                }
            }
            Step::Open { target, args } => {
                // Prima le %VARIABILI% di Windows, poi i segnaposto: così un testo selezionato
                // che contiene dei % non viene mai espanso.
                let mode = if placeholders::is_url(target) { Mode::Url } else { Mode::Plain };
                let target = self.expand(&expand_env(target.trim()), mode)?.0;
                let args = self.expand_plain(&expand_env(args))?;
                if target.trim().is_empty() {
                    return Err(tr!("Manca cosa aprire", "Nothing to open").to_string().into());
                }
                shell_execute(w!("open"), target.trim(), &args, "", SW_SHOWNORMAL)?;
            }
            Step::Keys { keys: text } => {
                let combo = keys::parse(text)?;
                keys::wait_for_modifiers_release();
                keys::send(&combo);
            }
            Step::Text { text } => {
                let text = self.expand_plain(text)?;
                keys::wait_for_modifiers_release();
                keys::type_text(&text);
            }
            Step::Wait { seconds } => self.sleep(Duration::from_secs_f32(seconds.clamp(0.0, 3600.0)))?,
            Step::WaitWindow { app, title, timeout } => {
                let (app, title) = (self.expand_plain(app)?, self.expand_plain(title)?);
                window::wait_for(&app, &title, *timeout, &|| self.cancelled())?;
            }
            Step::Window { op } => window::apply(*op, self.job.context.hwnd())?,
            Step::System { op } => system(*op)?,
            Step::Clipboard { op, text } => match op {
                ClipboardOp::Copy => {
                    if !clipboard::copy_selection(&|| self.cancelled()) && !self.cancelled() {
                        return Err(tr!("Non è stato copiato niente: c'era del testo selezionato?", "Nothing was copied: was any text selected?")
                            .to_string()
                            .into());
                    }
                }
                ClipboardOp::Paste => clipboard::paste(),
                ClipboardOp::Set => {
                    let text = self.expand_plain(text)?;
                    clipboard::set_text(&text)?;
                }
            },
            Step::Run { action } => {
                if self.depth >= MAX_DEPTH {
                    return Err(tr!("Troppe azioni una dentro l'altra", "Too many actions inside each other").to_string().into());
                }
                let Some(item) = self.job.library.iter().find(|a| a.id == *action) else {
                    return Err(trf!("L'azione «{action}» non esiste più", "The action “{action}” doesn't exist anymore").into());
                };
                let (label, steps) = (item.label.clone(), item.steps.clone());
                self.depth += 1;
                let result = steps.iter().enumerate().try_for_each(|(index, step)| match self.step(step) {
                    Err(Stop::Failed(message)) => {
                        let number = index + 1;
                        Err(trf!("«{label}», passo {number}: {message}", "“{label}”, step {number}: {message}").into())
                    }
                    Ok(()) if self.cancelled() => Err(Stop::Cancelled),
                    other => other,
                });
                self.depth -= 1;
                result?;
            }
        }
        Ok(())
    }
}

fn system(op: SystemOp) -> Result<(), String> {
    let press = |key: VIRTUAL_KEY| keys::send(&Combo { modifiers: Vec::new(), key });
    match op {
        SystemOp::Lock => unsafe { LockWorkStation() }.map_err(|e| e.message())?,
        SystemOp::Sleep => {
            // Un attimo per lasciar rilasciare il tasto del mouse o la scorciatoia.
            thread::sleep(Duration::from_millis(300));
            if !unsafe { SetSuspendState(false, false, false) } {
                return Err(tr!("Windows non è andato in sospensione", "Windows didn't go to sleep").into());
            }
        }
        SystemOp::ScreenOff => {
            // Prima di spegnere, un attimo: il mouse che si muove ancora lo riaccenderebbe subito.
            thread::sleep(Duration::from_millis(500));
            unsafe {
                PostMessageW(Some(HWND_BROADCAST), WM_SYSCOMMAND, WPARAM(SC_MONITORPOWER as usize), LPARAM(2))
                    .map_err(|e| e.message())?;
            }
        }
        SystemOp::VolumeUp => press(VK_VOLUME_UP),
        SystemOp::VolumeDown => press(VK_VOLUME_DOWN),
        SystemOp::Mute => press(VK_VOLUME_MUTE),
        SystemOp::PlayPause => press(VK_MEDIA_PLAY_PAUSE),
        SystemOp::NextTrack => press(VK_MEDIA_NEXT_TRACK),
        SystemOp::PreviousTrack => press(VK_MEDIA_PREV_TRACK),
    }
    Ok(())
}

/// Come eseguire un comando.
struct CommandRun {
    shell: Shell,
    window: WindowMode,
    admin: bool,
    wait: bool,
}

impl CommandRun {
    /// Avvia il comando; se è nascosto restituisce quello che ha scritto (per `{result}`).
    fn start(
        &self,
        command: &str,
        env: Vec<(String, String)>,
        cwd: &str,
        context: &Context,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<String>, String> {
        if command.trim().is_empty() {
            return Err(tr!("Il comando è vuoto", "The command is empty").into());
        }
        let cwd = if cwd.trim().is_empty() { expand_env("%USERPROFILE%") } else { cwd.to_string() };
        let hidden = self.window == WindowMode::Hidden;
        let mut env = env;
        env.extend([
            ("WHEELOR_APP".to_string(), context.app.clone()),
            ("WHEELOR_TITLE".to_string(), context.title.clone()),
            ("WHEELOR_PROFILE".to_string(), context.profile.clone()),
        ]);
        // cmd espande le %VARIABILI% prima di leggere & e |: i segnaposto usano l'espansione
        // ritardata (!VARIABILE!), attiva solo se servono perché cambia il significato di "!".
        let delayed = self.shell == Shell::Cmd && command.contains("!WHEELOR_");

        // L'esecuzione come amministratore passa da ShellExecute (mostra il prompt UAC). Il processo
        // nuovo non eredita le nostre variabili d'ambiente: in PowerShell le impostiamo all'inizio
        // del comando stesso.
        if self.admin {
            let command = match self.shell {
                Shell::Cmd if delayed => {
                    return Err(tr!(
                        "Nei comandi del Prompt dei comandi come amministratore i segnaposto non funzionano: usa PowerShell",
                        "Placeholders don't work in Command Prompt commands run as administrator: use PowerShell"
                    )
                    .into());
                }
                Shell::Cmd => command.to_string(),
                Shell::Powershell | Shell::Pwsh => {
                    let mut prelude: String =
                        env.iter().map(|(name, value)| format!("$env:{name}='{}'; ", value.replace('\'', "''"))).collect();
                    prelude.push_str(command);
                    prelude
                }
            };
            let (program, args) = command_line(&command, self.shell, self.window, false, false);
            let show = if hidden { SW_HIDE } else { SW_SHOWNORMAL };
            let process = shell_execute_process(w!("runas"), program, &args, &cwd, show)?;
            if hidden || self.wait {
                wait_process(process, cancelled)?;
            } else {
                unsafe {
                    let _ = CloseHandle(process);
                }
            }
            return Ok(None);
        }

        let (program, args) = command_line(command, self.shell, self.window, delayed, hidden);
        let mut process = Command::new(program);
        // raw_arg: gli argomenti passano esattamente come li abbiamo costruiti, senza altro escaping.
        process.raw_arg(&args).current_dir(&cwd).envs(env);
        let start_error = |e: std::io::Error| trf!("Impossibile avviare {program}: {e}", "Can't start {program}: {e}");

        if !hidden {
            process.creation_flags(CREATE_NEW_CONSOLE.0);
            let child = process.spawn().map_err(start_error)?;
            if self.wait {
                wait_child(child, cancelled)?;
            }
            return Ok(None);
        }

        // In background: niente finestra, e aspettiamo la fine per segnalare eventuali errori.
        // L'uscita si legge su due thread: un comando che scrive molto riempirebbe il tubo e si
        // fermerebbe ad aspettare che qualcuno legga.
        let mut child = process
            .creation_flags(CREATE_NO_WINDOW.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(start_error)?;
        let reader = |pipe: Option<Box<dyn Read + Send>>| {
            thread::spawn(move || {
                let mut bytes = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut bytes);
                }
                decode(bytes)
            })
        };
        let stdout = reader(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let stderr = reader(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let Some(code) = wait_child(child, cancelled)? else { return Ok(None) };
        let output = stdout.join().unwrap_or_default();
        if code == 0 {
            return Ok(Some(output.trim_end().to_string()));
        }
        let stderr = stderr.join().unwrap_or_default();
        let detail = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(tr!("nessun dettaglio", "no details"));
        Err(trf!("Uscito con codice {code}: {detail}", "Exited with code {code}: {detail}"))
    }
}

/// Aspetta la fine di un processo e ne restituisce il codice d'uscita; `None` se nel frattempo
/// l'azione è stata fermata (il processo continua per conto suo).
fn wait_child(mut child: Child, cancelled: &dyn Fn() -> bool) -> Result<Option<i32>, String> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status.code().unwrap_or(-1))),
            Ok(None) if cancelled() => return Ok(None),
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Come `wait_child`, per un processo avviato con ShellExecuteEx.
fn wait_process(process: HANDLE, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let result = loop {
        if unsafe { WaitForSingleObject(process, 50) } != WAIT_TIMEOUT {
            let mut code = 0u32;
            let _ = unsafe { GetExitCodeProcess(process, &mut code) };
            let code = code as i32;
            break match code {
                0 => Ok(()),
                code => Err(trf!("Uscito con codice {code}", "Exited with code {code}")),
            };
        }
        if cancelled() {
            break Ok(());
        }
    };
    unsafe {
        let _ = CloseHandle(process);
    }
    result
}

/// L'uscita di un comando: UTF-8 (PowerShell, a cui lo chiediamo) o, se non lo è, la codepage
/// della console (cmd e i programmi che usano quella).
fn decode(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => {
            let bytes = e.into_bytes();
            let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
            unsafe {
                let codepage = GetOEMCP();
                let len = MultiByteToWideChar(codepage, flags, &bytes, None);
                let mut wide = vec![0u16; len.max(0) as usize];
                MultiByteToWideChar(codepage, flags, &bytes, Some(&mut wide));
                String::from_utf16_lossy(&wide)
            }
        }
    }
}

/// Programma e argomenti per eseguire `command` nella shell scelta. `delayed`: cmd con
/// l'espansione ritardata; `utf8`: PowerShell scrive l'uscita in UTF-8 (per leggerla).
fn command_line(command: &str, shell: Shell, window: WindowMode, delayed: bool, utf8: bool) -> (&'static str, String) {
    let keep_open = window == WindowMode::KeepOpen;
    match shell {
        // /s: cmd toglie solo le virgolette esterne e lascia il comando intatto.
        Shell::Cmd => {
            let delayed = if delayed { "/v:on " } else { "" };
            ("cmd.exe", format!("/d {delayed}/s {} \"{command}\"", if keep_open { "/k" } else { "/c" }))
        }
        Shell::Powershell | Shell::Pwsh => {
            let program = if shell == Shell::Pwsh { "pwsh.exe" } else { "powershell.exe" };
            let command = match utf8 {
                true => format!("[Console]::OutputEncoding = [Text.Encoding]::UTF8; {command}"),
                false => command.to_string(),
            };
            // -EncodedCommand: il comando viaggia in base64, quindi virgolette e caratteri
            // speciali arrivano a PowerShell intatti, senza problemi di escaping.
            let encoded = base64(&command.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>());
            let no_exit = if keep_open { " -NoExit" } else { "" };
            (program, format!("-NoLogo{no_exit} -EncodedCommand {encoded}"))
        }
    }
}

fn shell_execute(verb: PCWSTR, file: &str, args: &str, dir: &str, show: SHOW_WINDOW_CMD) -> Result<(), String> {
    shell_execute_ex(verb, file, args, dir, show, false).map(|_| ())
}

/// Come `shell_execute`, ma restituisce il processo avviato (da chiudere con CloseHandle).
fn shell_execute_process(verb: PCWSTR, file: &str, args: &str, dir: &str, show: SHOW_WINDOW_CMD) -> Result<HANDLE, String> {
    shell_execute_ex(verb, file, args, dir, show, true)?
        .ok_or_else(|| tr!("Il processo non è partito", "The process didn't start").to_string())
}

fn shell_execute_ex(verb: PCWSTR, file: &str, args: &str, dir: &str, show: SHOW_WINDOW_CMD, process: bool) -> Result<Option<HANDLE>, String> {
    // Le HSTRING devono restare vive finché ShellExecuteExW usa i loro puntatori.
    let (file_w, args_w, dir_w) = (HSTRING::from(file), HSTRING::from(args), HSTRING::from(dir));
    let optional = |s: &HSTRING| if s.is_empty() { PCWSTR::null() } else { PCWSTR(s.as_ptr()) };
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: if process { SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS } else { SEE_MASK_NOASYNC },
        lpVerb: verb,
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: optional(&args_w),
        lpDirectory: optional(&dir_w),
        nShow: show.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }
        .map_err(|e| trf!("Impossibile aprire \"{file}\": {}", "Can't open \"{file}\": {}", e.message()))?;
    Ok((!info.hProcess.is_invalid()).then_some(info.hProcess))
}

/// Espande le %VARIABILI% d'ambiente e la ~ iniziale (cartella utente).
fn expand_env(text: &str) -> String {
    let text = match text.strip_prefix('~') {
        Some(rest) => format!("%USERPROFILE%{rest}"),
        None => text.to_string(),
    };
    let source = HSTRING::from(text.as_str());
    unsafe {
        let needed = ExpandEnvironmentStringsW(&source, None) as usize;
        let mut buffer = vec![0u16; needed];
        let written = ExpandEnvironmentStringsW(&source, Some(&mut buffer)) as usize;
        if written == 0 {
            return text;
        }
        String::from_utf16_lossy(&buffer[..written - 1]) // senza lo 0 finale
    }
}

/// Percorso dell'eseguibile che possiede la finestra.
pub fn process_path(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

/// Codifica base64 standard: ogni 3 byte diventano 4 caratteri.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WindowOp;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn cmd_keeps_the_command_intact() {
        let (program, args) = command_line(r#"echo "ciao" & dir"#, Shell::Cmd, WindowMode::KeepOpen, false, false);
        assert_eq!(program, "cmd.exe");
        assert_eq!(args, r#"/d /s /k "echo "ciao" & dir""#);
        let (_, args) = command_line("echo !WHEELOR_SELECTION!", Shell::Cmd, WindowMode::Hidden, true, true);
        assert_eq!(args, r#"/d /v:on /s /c "echo !WHEELOR_SELECTION!""#);
    }

    #[test]
    fn expand_env_replaces_tilde_and_variables() {
        let home = std::env::var("USERPROFILE").unwrap();
        assert_eq!(expand_env("~\\Desktop"), format!("{home}\\Desktop"));
        assert_eq!(expand_env("%USERPROFILE%"), home);
    }

    fn job(steps: Vec<Step>) -> Job {
        Job { label: "Prova".into(), steps, library: Vec::new(), context: Context::default() }
    }

    #[test]
    fn hidden_commands_pass_their_output_on() {
        let command = |command: &str, shell| Step::Command {
            command: command.into(),
            shell,
            window: WindowMode::Hidden,
            cwd: String::new(),
            admin: false,
            wait: false,
        };
        // Il risultato del primo comando arriva al secondo come variabile, accenti compresi.
        let write = command("Write-Output 'perché sì'", Shell::Powershell);
        let run = |second: Step| execute(&job(vec![write.clone(), second]), GENERATION.load(Ordering::SeqCst));
        run(command("if ({result} -ne 'perché sì') { exit 4 }", Shell::Powershell)).unwrap();
        run(command("if not \"{result}\"==\"perché sì\" exit /b 5", Shell::Cmd)).unwrap();
        // Tra apici singoli PowerShell non espande la variabile: il secondo comando esce con 3.
        let failure = run(command("if ('{result}' -ne 'perché sì') { exit 3 }", Shell::Powershell)).unwrap_err();
        assert!(failure.title.contains("passo 2 di 2") && failure.message.contains("codice 3"), "{failure:?}");
    }

    #[test]
    fn steps_stop_at_the_first_failure() {
        let steps = vec![Step::Wait { seconds: 0.0 }, Step::Keys { keys: "Ctrl+Nonesiste".into() }, Step::Window { op: WindowOp::Close }];
        let failure = execute(&job(steps), GENERATION.load(Ordering::SeqCst)).unwrap_err();
        assert!(failure.title.contains("passo 2 di 3"), "{failure:?}");
        assert!(failure.message.starts_with("Tasti"), "{failure:?}");
    }

    #[test]
    fn cancel_stops_waiting() {
        let generation = GENERATION.load(Ordering::SeqCst);
        let start = std::time::Instant::now();
        let handle = thread::spawn(move || execute(&job(vec![Step::Wait { seconds: 5.0 }]), generation));
        thread::sleep(Duration::from_millis(100));
        cancel();
        assert!(handle.join().unwrap().is_ok());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
