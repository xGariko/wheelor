//! Registrazione di una combinazione di tasti: si clicca il campo e si premono i tasti.
//!
//! Mentre il campo ascolta, un hook di tastiera di basso livello (WH_KEYBOARD_LL) riceve i tasti
//! prima di chiunque altro e li "mangia": così si possono registrare anche Win, Stamp, i tasti
//! multimediali e le combinazioni già usate da Windows o da altre app (compresa quella che apre
//! la ruota), senza che facciano altro. Ascolta solo con la finestra delle impostazioni in primo
//! piano: un tasto premuto altrove interrompe l'ascolto e arriva normalmente all'altra app.
//!
//! La callback dell'hook deve essere brevissima: aggiorna lo stato e rimanda il resto al ciclo
//! degli eventi di Slint (`invoke_from_event_loop`), che chiama `on_event` sullo stesso thread.

use std::cell::RefCell;
use std::time::Duration;

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::keys::Combo;
use crate::trf;

/// Cosa succede mentre il campo ascolta.
pub enum Event {
    /// Sono cambiati i modificatori tenuti premuti (si mostrano mentre si compone la combinazione).
    Held(Vec<VIRTUAL_KEY>),
    /// Combinazione completa: modificatori più un tasto.
    Done(Combo),
    /// Esc da solo, oppure un tasto premuto con un'altra finestra in primo piano.
    Cancelled,
}

#[derive(Default)]
struct State {
    hook: Option<HHOOK>,
    /// La finestra delle impostazioni.
    window: HWND,
    listening: bool,
    /// Tasti premuti che abbiamo mangiato: va mangiato anche il loro rilascio, altrimenti
    /// Windows vedrebbe un Win rilasciato senza essere stato premuto (e aprirebbe Start).
    down: Vec<VIRTUAL_KEY>,
    /// Eventi in attesa di arrivare a `on_event`.
    events: Vec<Event>,
    on_event: Option<Box<dyn Fn(Event)>>,
}

thread_local! {
    // L'hook viene chiamato sul thread che l'ha installato, quello dell'interfaccia.
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Inizia ad ascoltare. `window` è la finestra delle impostazioni.
pub fn start(window: HWND, on_event: impl Fn(Event) + 'static) -> Result<(), String> {
    STATE.with_borrow_mut(|s| {
        if s.hook.is_none() {
            let instance = unsafe { GetModuleHandleW(None) }.map_err(|e| e.message())?;
            let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance.into()), 0) };
            let error = |e: windows::core::Error| trf!("Impossibile leggere la tastiera: {}", "Can't read the keyboard: {}", e.message());
            s.hook = Some(hook.map_err(error)?);
        }
        s.window = window;
        s.listening = true;
        s.events.clear();
        s.on_event = Some(Box::new(on_event));
        Ok(())
    })
}

/// Smette di ascoltare (per esempio con un clic altrove). L'hook resta finché non vengono
/// rilasciati i tasti già mangiati.
pub fn stop() {
    STATE.with_borrow_mut(|s| {
        s.listening = false;
        s.events.clear();
        s.on_event = None;
    });
    release_when_idle();
}

extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        // try_borrow_mut: se lo stato fosse già in uso (non dovrebbe), il tasto passa e basta.
        let handled = STATE.with(|cell| {
            cell.try_borrow_mut()
                .map(|mut s| {
                    let foreground = unsafe { GetForegroundWindow() } == s.window;
                    s.key(VIRTUAL_KEY(info.vkCode as u16), down, foreground)
                })
                .unwrap_or_default()
        });
        if handled.notify {
            let _ = slint::invoke_from_event_loop(deliver);
        }
        if handled.eat {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[derive(Default)]
struct Handled {
    /// Il tasto non arriva a nessun altro.
    eat: bool,
    /// C'è qualcosa da comunicare (o l'hook da togliere).
    notify: bool,
}

impl State {
    /// Un tasto premuto (`down`) o rilasciato; `foreground`: la finestra delle impostazioni è in primo piano.
    fn key(&mut self, vk: VIRTUAL_KEY, down: bool, foreground: bool) -> Handled {
        let eaten = self.down.contains(&vk);
        if !down {
            if !eaten {
                return Handled::default();
            }
            self.down.retain(|&k| k != vk);
            if self.listening && is_modifier(vk) {
                self.events.push(Event::Held(self.held()));
            }
            // Rilasciato l'ultimo tasto dopo la fine dell'ascolto: si può togliere l'hook.
            return Handled { eat: true, notify: self.listening || self.down.is_empty() };
        }

        // Un tasto tenuto premuto si ripete: se l'avevamo mangiato, mangiamo anche le ripetizioni.
        if !self.listening || eaten {
            return Handled { eat: eaten, notify: false };
        }
        if !foreground {
            self.listening = false;
            self.events.push(Event::Cancelled);
            return Handled { eat: false, notify: true };
        }

        self.down.push(vk);
        if is_modifier(vk) {
            self.events.push(Event::Held(self.held()));
        } else if vk == VK_ESCAPE && self.held().is_empty() {
            self.listening = false;
            self.events.push(Event::Cancelled);
        } else {
            self.listening = false;
            self.events.push(Event::Done(Combo { modifiers: self.held(), key: vk }));
        }
        Handled { eat: true, notify: true }
    }

    /// I modificatori premuti, come li scrive `keys` (Ctrl, Alt, Shift, Win, in quest'ordine).
    fn held(&self) -> Vec<VIRTUAL_KEY> {
        let any = |keys: &[VIRTUAL_KEY]| keys.iter().any(|k| self.down.contains(k));
        [
            (VK_CONTROL, any(&[VK_CONTROL, VK_LCONTROL, VK_RCONTROL])),
            (VK_MENU, any(&[VK_MENU, VK_LMENU, VK_RMENU])),
            (VK_SHIFT, any(&[VK_SHIFT, VK_LSHIFT, VK_RSHIFT])),
            (VK_LWIN, any(&[VK_LWIN, VK_RWIN])),
        ]
        .into_iter()
        .filter_map(|(vk, held)| held.then_some(vk))
        .collect()
    }
}

fn is_modifier(vk: VIRTUAL_KEY) -> bool {
    matches!(
        vk,
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT | VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_MENU | VK_LMENU | VK_RMENU
            | VK_LWIN | VK_RWIN
    )
}

/// Consegna gli eventi in attesa. Gira nel ciclo degli eventi di Slint, fuori dall'hook.
fn deliver() {
    // La callback si prende fuori dallo stato: così può chiamare `start` o `stop` senza conflitti.
    let (events, callback) = STATE.with_borrow_mut(|s| (std::mem::take(&mut s.events), s.on_event.take()));
    if let Some(callback) = callback {
        for event in events {
            callback(event);
        }
        // Rimessa a posto solo se si ascolta ancora e nessuno ne ha impostata un'altra.
        STATE.with_borrow_mut(|s| {
            if s.listening && s.on_event.is_none() {
                s.on_event = Some(callback);
            }
        });
    }
    release_when_idle();
}

/// Toglie l'hook se non si ascolta più e non restano tasti da mangiare. Se un rilascio non
/// arrivasse mai (succede di rado, per esempio con Ctrl+Alt+Canc), lo toglie comunque dopo un po'.
fn release_when_idle() {
    let waiting = STATE.with_borrow_mut(|s| {
        if s.listening {
            return false;
        }
        if s.down.is_empty() {
            unhook(s);
            return false;
        }
        true
    });
    if waiting {
        slint::Timer::single_shot(Duration::from_secs(3), || {
            STATE.with_borrow_mut(|s| {
                if !s.listening {
                    s.down.clear();
                    unhook(s);
                }
            })
        });
    }
}

fn unhook(state: &mut State) {
    if let Some(hook) = state.hook.take() {
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
    state.on_event = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: VIRTUAL_KEY = VIRTUAL_KEY(b'W' as u16);

    fn listening() -> State {
        State { listening: true, ..State::default() }
    }

    fn done(state: &State) -> Option<String> {
        state.events.iter().find_map(|e| match e {
            Event::Done(combo) => Some(combo.text()),
            _ => None,
        })
    }

    #[test]
    fn records_a_combination_and_eats_every_key() {
        let mut s = listening();
        for vk in [VK_LCONTROL, VK_RMENU, W] {
            assert!(s.key(vk, true, true).eat);
        }
        assert_eq!(done(&s).as_deref(), Some("Ctrl+Alt+W"));
        assert!(!s.listening);
        // Ripetizioni e rilasci dei tasti mangiati restano mangiati; l'hook si toglie all'ultimo.
        assert!(s.key(W, true, true).eat);
        assert!(s.key(W, false, true).eat);
        assert!(s.key(VK_RMENU, false, true).eat);
        let last = s.key(VK_LCONTROL, false, true);
        assert!(last.eat && last.notify && s.down.is_empty());
        // Dopo, i tasti passano.
        assert!(!s.key(W, true, true).eat);
    }

    #[test]
    fn win_alone_does_not_finish_and_does_not_open_start() {
        let mut s = listening();
        assert!(s.key(VK_LWIN, true, true).eat);
        assert!(s.key(VK_LWIN, false, true).eat);
        assert!(s.listening && done(&s).is_none());
        s.key(VK_LWIN, true, true);
        s.key(VK_LSHIFT, true, true);
        s.key(VK_SPACE, true, true);
        assert_eq!(done(&s).as_deref(), Some("Shift+Win+Space"));
    }

    #[test]
    fn escape_cancels_but_ctrl_escape_is_recorded() {
        let mut s = listening();
        s.key(VK_ESCAPE, true, true);
        assert!(matches!(s.events.last(), Some(Event::Cancelled)) && !s.listening);

        let mut s = listening();
        s.key(VK_LCONTROL, true, true);
        s.key(VK_ESCAPE, true, true);
        assert_eq!(done(&s).as_deref(), Some("Ctrl+Esc"));
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
