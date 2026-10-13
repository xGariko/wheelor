//! Registrazione di una combinazione di tasti: si clicca il campo e si premono i tasti.
//!
//! I tasti premuti insieme formano la combinazione; quando per 2 secondi non è premuto niente,
//! è quella scelta. Tasti premuti dopo aver rilasciato tutto ne iniziano una nuova, che prende
//! il posto della precedente: conta l'ultima.
//!
//! Mentre il campo ascolta, un hook di tastiera di basso livello (WH_KEYBOARD_LL) riceve i tasti
//! prima di chiunque altro e li "mangia": così si possono registrare anche Win, Stamp, i tasti
//! multimediali e le combinazioni già usate da Windows o da altre app (compresa quella che apre
//! la ruota), senza che facciano altro. Ascolta solo con le impostazioni in primo piano: un tasto
//! premuto altrove interrompe l'ascolto e arriva normalmente all'altra app.
//!
//! Windows non chiama l'hook di un processo che, con la sua finestra in primo piano, è registrato
//! per il Raw Input della tastiera: winit lo fa, per dei DeviceEvent che Slint non usa. Quindi,
//! prima di ascoltare, quella registrazione si toglie (`stop_raw_keyboard_input`).
//!
//! L'hook ha un thread tutto suo, che non fa altro. Windows aspetta la sua risposta prima di
//! consegnare il tasto a chiunque: sul thread dell'interfaccia, mentre Skia ridisegna la finestra,
//! ogni tasto di tutto il sistema arriverebbe in ritardo e, oltre il limite di Windows
//! (LowLevelHooksTimeout), l'hook verrebbe tolto senza avvisare. La callback e il timer dei
//! 2 secondi restano nel ciclo degli eventi di Slint (`invoke_from_event_loop`).

use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::{RAWINPUTDEVICE, RIDEV_REMOVE, RegisterRawInputDevices};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::keys::Combo;
use crate::trf;

/// Quanto deve passare senza tasti premuti perché la combinazione sia quella scelta
/// (la barra del campo in settings.slint dura altrettanto).
pub const QUIET: Duration = Duration::from_secs(2);

/// Cosa succede mentre il campo ascolta.
pub enum Event {
    /// È cambiata la combinazione che si sta componendo: i modificatori (nell'ordine di `keys`) e
    /// il tasto principale, se c'è già. `confirming`: tutto rilasciato, e senza altri tasti fra
    /// `QUIET` sarà quella scelta.
    Changed { modifiers: Vec<VIRTUAL_KEY>, key: Option<VIRTUAL_KEY>, confirming: bool },
    /// Combinazione scelta: modificatori più un tasto.
    Done(Combo),
    /// Esc da solo, oppure un tasto premuto con un'altra finestra in primo piano.
    Cancelled,
}

/// Condiviso fra il thread dell'hook e quello dell'interfaccia.
struct State {
    /// Il thread dell'hook, finché serve. Uno congedato (sta per uscire) lascia passare tutto.
    thread: Option<u32>,
    listening: bool,
    /// Tasti premuti che abbiamo mangiato: va mangiato anche il loro rilascio, altrimenti
    /// Windows vedrebbe un Win rilasciato senza essere stato premuto (e aprirebbe Start).
    down: Vec<VIRTUAL_KEY>,
    /// La combinazione che si sta componendo.
    modifiers: Vec<VIRTUAL_KEY>,
    key: Option<VIRTUAL_KEY>,
    /// L'ultimo tasto premuto, ripetuto o rilasciato: un tasto tenuto premuto si ripete di
    /// continuo, quindi da qui in avanti non c'è niente di premuto.
    last: Option<Instant>,
    /// Eventi in attesa di arrivare alla callback.
    events: Vec<Event>,
}

static STATE: Mutex<State> = Mutex::new(State {
    thread: None,
    listening: false,
    down: Vec::new(),
    modifiers: Vec::new(),
    key: None,
    last: None,
    events: Vec::new(),
});

type Callback = Box<dyn Fn(Event)>;

thread_local! {
    // Solo sul thread dell'interfaccia: chi riceve gli eventi e il timer dei 2 secondi.
    static ON_EVENT: RefCell<Option<Callback>> = const { RefCell::new(None) };
    static QUIET_TIMER: slint::Timer = slint::Timer::default();
}

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Inizia ad ascoltare.
pub fn start(on_event: impl Fn(Event) + 'static) -> Result<(), String> {
    stop_raw_keyboard_input();
    let mut s = state();
    if s.thread.is_none() {
        // Lo stato resta bloccato mentre il thread parte: l'hook lo usa solo dopo averci risposto.
        let error = |e: String| trf!("Impossibile leggere la tastiera: {}", "Can't read the keyboard: {}", e);
        s.thread = Some(spawn_hook().map_err(error)?);
    }
    s.listening = true;
    s.modifiers.clear();
    s.key = None;
    s.last = None;
    s.events.clear();
    drop(s);
    ON_EVENT.set(Some(Box::new(on_event)));
    Ok(())
}

/// Smette di ascoltare (per esempio con un clic altrove). L'hook resta finché non vengono
/// rilasciati i tasti già mangiati.
pub fn stop() {
    {
        let mut s = state();
        s.listening = false;
        s.events.clear();
    }
    ON_EVENT.set(None);
    release_when_idle();
}

/// Toglie la registrazione di questo processo al Raw Input della tastiera (quella del mouse resta).
/// winit la fa all'avvio per i suoi DeviceEvent, che Slint non usa; ma finché c'è, con le
/// impostazioni in primo piano Windows non chiama l'hook e nessun tasto viene registrato.
fn stop_raw_keyboard_input() {
    // Pagina 1 (Generic Desktop), uso 6: la tastiera. Per toglierla la finestra dev'essere nulla.
    let keyboard = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: RIDEV_REMOVE, hwndTarget: HWND::default() };
    unsafe {
        let _ = RegisterRawInputDevices(&[keyboard], size_of::<RAWINPUTDEVICE>() as u32);
    }
}

/// Avvia il thread dell'hook e aspetta che l'hook sia installato. Restituisce l'id del thread.
fn spawn_hook() -> Result<u32, String> {
    let (sender, receiver) = mpsc::channel();
    let body = move || {
        let mut msg = MSG::default();
        // Crea la coda dei messaggi: senza, il WM_QUIT di `quit` andrebbe perso.
        unsafe {
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        }
        let installed = unsafe {
            GetModuleHandleW(None).and_then(|instance| SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance.into()), 0))
        };
        let hook = match installed {
            Ok(hook) => hook,
            Err(e) => {
                let _ = sender.send(Err(e.message()));
                return;
            }
        };
        let _ = sender.send(Ok(unsafe { GetCurrentThreadId() }));
        // Windows chiama l'hook dentro GetMessageW; il ciclo finisce con WM_QUIT.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
    };
    thread::Builder::new().name("capture".into()).spawn(body).map_err(|e| e.to_string())?;
    receiver.recv().map_err(|e| e.to_string())?
}

extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let front = down && settings_in_front();
        let handled = {
            let mut s = state();
            if s.thread != Some(unsafe { GetCurrentThreadId() }) {
                Handled::default()
            } else {
                let handled = s.key(VIRTUAL_KEY(info.vkCode as u16), down, front);
                // Finito di ascoltare e rilasciato l'ultimo tasto mangiato: l'hook non serve più.
                if !s.listening && s.down.is_empty() {
                    s.quit();
                }
                handled
            }
        };
        if handled.notify {
            let _ = slint::invoke_from_event_loop(deliver);
        }
        if handled.eat {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// In primo piano c'è una finestra di questo processo, cioè le impostazioni.
fn settings_in_front() -> bool {
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&raw mut process)) };
    process == unsafe { GetCurrentProcessId() }
}

#[derive(Default)]
struct Handled {
    /// Il tasto non arriva a nessun altro.
    eat: bool,
    /// C'è qualcosa da comunicare.
    notify: bool,
}

impl State {
    /// Un tasto premuto (`down`) o rilasciato; `front`: le impostazioni sono in primo piano.
    fn key(&mut self, vk: VIRTUAL_KEY, down: bool, front: bool) -> Handled {
        let eaten = self.down.contains(&vk);
        if !down {
            if !eaten {
                return Handled::default();
            }
            self.down.retain(|&k| k != vk);
            if !self.listening {
                return Handled { eat: true, notify: false };
            }
            self.last = Some(Instant::now());
            // Rilasciato tutto: il campo lo mostra (inizia l'attesa dei 2 secondi).
            let all = self.down.is_empty();
            if all {
                self.events.push(self.changed());
            }
            return Handled { eat: true, notify: all };
        }

        // Un tasto tenuto premuto si ripete: se l'avevamo mangiato, mangiamo anche le ripetizioni.
        if eaten {
            if self.listening {
                self.last = Some(Instant::now());
            }
            return Handled { eat: true, notify: false };
        }
        if !self.listening {
            return Handled::default();
        }
        if !front {
            self.listening = false;
            self.events.push(Event::Cancelled);
            return Handled { eat: false, notify: true };
        }

        let first = self.down.is_empty();
        self.down.push(vk);
        self.last = Some(Instant::now());
        if first && vk == VK_ESCAPE {
            self.listening = false;
            self.events.push(Event::Cancelled);
            return Handled { eat: true, notify: true };
        }
        if first {
            self.modifiers.clear();
            self.key = None;
        }
        match modifier(vk) {
            Some(m) if !self.modifiers.contains(&m) => {
                self.modifiers.push(m);
                self.modifiers.sort_by_key(|m| MODIFIERS.iter().position(|k| k == m));
            }
            Some(_) => {}
            None => self.key = Some(vk),
        }
        self.events.push(self.changed());
        Handled { eat: true, notify: true }
    }

    /// `QUIET` senza tasti: la combinazione è quella scelta. Se ci sono solo modificatori non è
    /// una combinazione: si ricomincia da capo. I tasti ancora segnati come premuti non lo sono
    /// davvero (si ripeterebbero): il loro rilascio è andato perso, per esempio con Ctrl+Alt+Canc.
    fn finish(&mut self) {
        self.down.clear();
        self.last = None;
        match self.key.take() {
            Some(key) => {
                self.listening = false;
                self.events.push(Event::Done(Combo { modifiers: std::mem::take(&mut self.modifiers), key }));
            }
            None => {
                self.modifiers.clear();
                self.events.push(self.changed());
            }
        }
    }

    fn changed(&self) -> Event {
        let confirming = self.down.is_empty() && self.key.is_some();
        Event::Changed { modifiers: self.modifiers.clone(), key: self.key, confirming }
    }

    /// Congeda il thread dell'hook.
    fn quit(&mut self) {
        if let Some(thread) = self.thread.take() {
            unsafe {
                let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// I modificatori come li scrive `keys`, in quest'ordine.
const MODIFIERS: [VIRTUAL_KEY; 4] = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN];

/// Il modificatore di un tasto (destro e sinistro sono lo stesso), se lo è.
fn modifier(vk: VIRTUAL_KEY) -> Option<VIRTUAL_KEY> {
    match vk {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => Some(VK_CONTROL),
        VK_MENU | VK_LMENU | VK_RMENU => Some(VK_MENU),
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => Some(VK_SHIFT),
        VK_LWIN | VK_RWIN => Some(VK_LWIN),
        _ => None,
    }
}

/// Consegna gli eventi in attesa. Gira nel ciclo degli eventi di Slint, fuori dall'hook.
fn deliver() {
    let (events, composing) = {
        let mut s = state();
        (std::mem::take(&mut s.events), s.listening && s.last.is_some())
    };
    // La callback si prende fuori: così può chiamare `start` o `stop` senza conflitti.
    if let Some(callback) = ON_EVENT.take() {
        for event in events {
            callback(event);
        }
        // Rimessa a posto solo se si ascolta ancora e nessuno ne ha impostata un'altra.
        ON_EVENT.with_borrow_mut(|slot| {
            if slot.is_none() && state().listening {
                *slot = Some(callback);
            }
        });
    }
    if composing {
        QUIET_TIMER.with(|timer| {
            if !timer.running() {
                timer.start(slint::TimerMode::SingleShot, QUIET, tick);
            }
        });
    }
    release_when_idle();
}

/// Il timer dei 2 secondi: se nel frattempo è arrivato un tasto, aspetta il resto.
fn tick() {
    let left = {
        let mut s = state();
        let last = match s.last {
            Some(last) if s.listening => last,
            _ => return,
        };
        let left = QUIET.saturating_sub(last.elapsed());
        if left.is_zero() {
            s.finish();
        }
        left
    };
    if left.is_zero() {
        deliver();
    } else {
        QUIET_TIMER.with(|timer| timer.start(slint::TimerMode::SingleShot, left, tick));
    }
}

/// Congeda l'hook se non si ascolta più e non restano tasti da mangiare. Se un rilascio non
/// arrivasse mai (succede di rado, per esempio con Ctrl+Alt+Canc), lo congeda comunque dopo un po'.
fn release_when_idle() {
    let waiting = {
        let mut s = state();
        if s.listening || s.thread.is_none() {
            return;
        }
        if s.down.is_empty() {
            s.quit();
        }
        s.thread.is_some()
    };
    if waiting {
        slint::Timer::single_shot(Duration::from_secs(3), || {
            let mut s = state();
            if !s.listening {
                s.down.clear();
                s.quit();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: VIRTUAL_KEY = VIRTUAL_KEY(b'A' as u16);
    const W: VIRTUAL_KEY = VIRTUAL_KEY(b'W' as u16);

    fn listening() -> State {
        State { thread: None, listening: true, down: Vec::new(), modifiers: Vec::new(), key: None, last: None, events: Vec::new() }
    }

    fn done(state: &State) -> Option<String> {
        state.events.iter().find_map(|e| match e {
            Event::Done(combo) => Some(combo.text()),
            _ => None,
        })
    }

    /// Lo stato dell'ultimo `Changed`: (tasti come in `Combo::text`, confirming).
    fn shown(state: &State) -> Option<(String, bool)> {
        state.events.iter().rev().find_map(|e| match e {
            Event::Changed { modifiers, key, confirming } => {
                let mut parts = crate::keys::modifier_names(modifiers);
                if let Some(key) = key {
                    parts.push(Combo { modifiers: Vec::new(), key: *key }.text());
                }
                Some((parts.join("+"), *confirming))
            }
            _ => None,
        })
    }

    fn press(s: &mut State, keys: &[VIRTUAL_KEY]) {
        for &vk in keys {
            assert!(s.key(vk, true, true).eat);
        }
    }

    fn release(s: &mut State, keys: &[VIRTUAL_KEY]) {
        for &vk in keys {
            assert!(s.key(vk, false, true).eat);
        }
    }

    #[test]
    fn records_until_quiet_and_eats_every_key() {
        let mut s = listening();
        press(&mut s, &[VK_LCONTROL, VK_RMENU, W]);
        assert_eq!(shown(&s), Some(("Ctrl+Alt+W".into(), false)));
        // Ripetizioni mangiate, e finché c'è un tasto premuto si continua ad ascoltare.
        let repeat = s.key(W, true, true);
        assert!(repeat.eat && !repeat.notify);
        release(&mut s, &[W, VK_RMENU]);
        assert!(s.listening && done(&s).is_none());
        let last = s.key(VK_LCONTROL, false, true);
        assert!(last.eat && last.notify);
        assert_eq!(shown(&s), Some(("Ctrl+Alt+W".into(), true)));
        assert!(s.last.is_some() && done(&s).is_none());

        s.finish();
        assert_eq!(done(&s).as_deref(), Some("Ctrl+Alt+W"));
        assert!(!s.listening);
        // Dopo, i tasti passano.
        assert!(!s.key(W, true, true).eat);
    }

    #[test]
    fn modifiers_count_even_if_released_before_the_key() {
        let mut s = listening();
        press(&mut s, &[VK_LWIN, VK_LSHIFT]);
        release(&mut s, &[VK_LSHIFT]);
        press(&mut s, &[VK_SPACE]);
        release(&mut s, &[VK_SPACE, VK_LWIN]);
        s.finish();
        assert_eq!(done(&s).as_deref(), Some("Shift+Win+Space"));
    }

    #[test]
    fn the_last_combination_wins() {
        let mut s = listening();
        press(&mut s, &[VK_LCONTROL, A]);
        release(&mut s, &[A, VK_LCONTROL]);
        press(&mut s, &[VK_LMENU, W]);
        assert_eq!(shown(&s), Some(("Alt+W".into(), false)));
        release(&mut s, &[W, VK_LMENU]);
        s.finish();
        assert_eq!(done(&s).as_deref(), Some("Alt+W"));
    }

    #[test]
    fn only_modifiers_start_over_and_win_does_not_open_start() {
        let mut s = listening();
        press(&mut s, &[VK_LWIN]);
        release(&mut s, &[VK_LWIN]);
        assert_eq!(shown(&s), Some(("Win".into(), false)));
        s.finish();
        assert!(s.listening && done(&s).is_none());
        assert_eq!(shown(&s), Some((String::new(), false)));
        press(&mut s, &[VK_F5]);
        release(&mut s, &[VK_F5]);
        s.finish();
        assert_eq!(done(&s).as_deref(), Some("F5"));
    }

    #[test]
    fn escape_cancels_but_ctrl_escape_is_recorded() {
        let mut s = listening();
        press(&mut s, &[VK_ESCAPE]);
        assert!(matches!(s.events.last(), Some(Event::Cancelled)) && !s.listening);
        // Anche il rilascio di Esc è mangiato: non arriva alla finestra.
        release(&mut s, &[VK_ESCAPE]);

        let mut s = listening();
        press(&mut s, &[VK_LCONTROL, VK_ESCAPE]);
        release(&mut s, &[VK_ESCAPE, VK_LCONTROL]);
        s.finish();
        assert_eq!(done(&s).as_deref(), Some("Ctrl+Esc"));
    }

    #[test]
    fn lost_releases_do_not_block_the_next_combination() {
        let mut s = listening();
        // Ctrl+Alt+Canc: i rilasci di Ctrl e Alt non arrivano.
        press(&mut s, &[VK_LCONTROL, VK_LMENU]);
        s.finish();
        assert!(s.listening && s.down.is_empty());
        press(&mut s, &[VK_LCONTROL, W]);
        release(&mut s, &[W, VK_LCONTROL]);
        s.finish();
        assert_eq!(done(&s).as_deref(), Some("Ctrl+W"));
    }

    /// Con una finestra in primo piano registrata al Raw Input come fa winit, l'hook riceve
    /// comunque i tasti. Apre una finestra per un attimo e simula F24 (un tasto che non fa nulla).
    /// `cargo test hook_works_with_raw_input_in_front -- --ignored`
    #[test]
    #[ignore = "apre una finestra e simula un tasto"]
    fn hook_works_with_raw_input_in_front() {
        use windows::core::w;
        let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
        let window = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Wheelor"), style, 100, 100, 300, 100, None, None, None, None) };
        let window = window.unwrap();
        let pump = |time: Duration| {
            let end = Instant::now() + time;
            let mut msg = MSG::default();
            while Instant::now() < end {
                while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                    unsafe { DispatchMessageW(&msg) };
                }
                thread::sleep(Duration::from_millis(5));
            }
        };
        unsafe {
            let _ = SetForegroundWindow(window);
        }
        // Mouse e tastiera, come `register_all_mice_and_keyboards_for_raw_input` di winit.
        let device = |usage| RAWINPUTDEVICE { usUsagePage: 1, usUsage: usage, dwFlags: windows::Win32::UI::Input::RIDEV_DEVNOTIFY, hwndTarget: window };
        unsafe { RegisterRawInputDevices(&[device(2), device(6)], size_of::<RAWINPUTDEVICE>() as u32) }.unwrap();
        pump(Duration::from_millis(300));
        assert!(settings_in_front(), "la finestra di prova non è in primo piano");

        start(|_| {}).unwrap();
        let key = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_F24, dwFlags: flags, ..Default::default() } },
        };
        unsafe { SendInput(&[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)], size_of::<INPUT>() as i32) };
        pump(Duration::from_millis(500));

        let mut s = state();
        let key = s.key;
        s.listening = false;
        s.quit();
        drop(s);
        unsafe {
            let _ = DestroyWindow(window);
        }
        assert_eq!(key, Some(VK_F24));
    }

    #[test]
    fn keys_for_other_windows_pass_and_stop_listening() {
        let mut s = listening();
        let handled = s.key(W, true, false);
        assert!(!handled.eat && handled.notify);
        assert!(!s.listening && matches!(s.events.last(), Some(Event::Cancelled)));
        // Il rilascio di un tasto premuto prima di iniziare non viene toccato.
        assert!(!s.key(VK_LCONTROL, false, true).eat);
    }
}
