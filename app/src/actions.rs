//! Esecuzione delle azioni. Ogni azione gira su un thread a parte,
//! così né la ruota né l'interfaccia restano mai bloccate.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use crate::config::{Action, Shell, WindowMode};
use crate::keys;

/// Informazioni sulla finestra in primo piano quando si è aperta la ruota.
/// I comandi le ricevono come variabili d'ambiente WHEELOR_APP, WHEELOR_TITLE e WHEELOR_PROFILE.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Percorso dell'eseguibile, es. C:\Program Files\...\Code.exe
    pub app: String,
    pub title: String,
    /// Nome della ruota di partenza (la principale o il profilo dell'app).
    pub profile: String,
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
            }
        }
    }
}

/// Avvia l'azione su un thread separato. Se fallisce, chiama `on_error` con il messaggio.
///
/// `move` trasferisce al thread la proprietà di `action`, `context` e `on_error`:
/// il compilatore verifica che siano sicuri da spostare tra thread (`Send`).
pub fn run(action: Action, context: Context, on_error: impl FnOnce(String) + Send + 'static) {
    thread::spawn(move || {
        // ShellExecuteEx può usare COM: ogni thread che lo chiama deve inizializzarlo.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if let Err(message) = execute(&action, &context) {
            on_error(message);
        }
    });
}

fn execute(action: &Action, context: &Context) -> Result<(), String> {
    match action {
        Action::Command { command, shell, window, cwd, admin } => {
            run_command(command, *shell, *window, cwd, *admin, context)
        }
        Action::Open { target, args } => shell_execute(w!("open"), &expand(target), &expand(args), "", SW_SHOWNORMAL),
        Action::Keys { keys: text } => {
            let combo = keys::parse(text)?;
            keys::wait_for_modifiers_release();
            keys::send(&combo);
            Ok(())
        }
        Action::Text { text } => {
            keys::wait_for_modifiers_release();
            keys::type_text(text);
            Ok(())
        }
    }
}

fn run_command(
    command: &str,
    shell: Shell,
    window: WindowMode,
    cwd: &str,
    admin: bool,
    context: &Context,
) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("Il comando è vuoto".into());
    }
    let (program, args) = command_line(command, shell, window);
    let cwd = if cwd.trim().is_empty() { expand("%USERPROFILE%") } else { expand(cwd) };

    // L'esecuzione come amministratore passa da ShellExecute (mostra il prompt UAC).
    if admin {
        let show = if window == WindowMode::Hidden { SW_HIDE } else { SW_SHOWNORMAL };
        return shell_execute(w!("runas"), program, &args, &cwd, show);
    }

    let mut process = Command::new(program);
    // raw_arg: gli argomenti passano esattamente come li abbiamo costruiti, senza altro escaping.
    process
        .raw_arg(&args)
        .current_dir(&cwd)
        .env("WHEELOR_APP", &context.app)
        .env("WHEELOR_TITLE", &context.title)
        .env("WHEELOR_PROFILE", &context.profile);

    if window != WindowMode::Hidden {
        process.creation_flags(CREATE_NEW_CONSOLE.0);
        process.spawn().map_err(|e| format!("Impossibile avviare {program}: {e}"))?;
        return Ok(());
    }

    // In background: niente finestra, e aspettiamo la fine per segnalare eventuali errori.
    let output = process
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Impossibile avviare {program}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("nessun dettaglio");
    let code = output.status.code().unwrap_or(-1);
    Err(format!("Uscito con codice {code}: {detail}"))
}

/// Programma e argomenti per eseguire `command` nella shell scelta.
fn command_line(command: &str, shell: Shell, window: WindowMode) -> (&'static str, String) {
    let keep_open = window == WindowMode::KeepOpen;
    match shell {
        // /s: cmd toglie solo le virgolette esterne e lascia il comando intatto.
        Shell::Cmd => ("cmd.exe", format!("/d /s {} \"{command}\"", if keep_open { "/k" } else { "/c" })),
        Shell::Powershell | Shell::Pwsh => {
            let program = if shell == Shell::Pwsh { "pwsh.exe" } else { "powershell.exe" };
            // -EncodedCommand: il comando viaggia in base64, quindi virgolette e caratteri
            // speciali arrivano a PowerShell intatti, senza problemi di escaping.
            let encoded = base64(&command.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>());
            let no_exit = if keep_open { " -NoExit" } else { "" };
            (program, format!("-NoLogo{no_exit} -EncodedCommand {encoded}"))
        }
    }
}

fn shell_execute(verb: PCWSTR, file: &str, args: &str, dir: &str, show: SHOW_WINDOW_CMD) -> Result<(), String> {
    // Le HSTRING devono restare vive finché ShellExecuteExW usa i loro puntatori.
    let (file_w, args_w, dir_w) = (HSTRING::from(file), HSTRING::from(args), HSTRING::from(dir));
    let optional = |s: &HSTRING| if s.is_empty() { PCWSTR::null() } else { PCWSTR(s.as_ptr()) };
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: verb,
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: optional(&args_w),
        lpDirectory: optional(&dir_w),
        nShow: show.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| format!("Impossibile aprire \"{file}\": {}", e.message()))
}

/// Espande le %VARIABILI% d'ambiente e la ~ iniziale (cartella utente).
fn expand(text: &str) -> String {
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
        let (program, args) = command_line(r#"echo "ciao" & dir"#, Shell::Cmd, WindowMode::KeepOpen);
        assert_eq!(program, "cmd.exe");
        assert_eq!(args, r#"/d /s /k "echo "ciao" & dir""#);
    }

    #[test]
    fn expand_replaces_tilde_and_variables() {
        let home = std::env::var("USERPROFILE").unwrap();
        assert_eq!(expand("~\\Desktop"), format!("{home}\\Desktop"));
        assert_eq!(expand("%USERPROFILE%"), home);
    }
}
