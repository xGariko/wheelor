//! La ruota: finestra trasparente sempre in primo piano. Viene disegnata con Direct2D
//! su una bitmap in memoria e mostrata con UpdateLayeredWindow (alpha per pixel).
//!
//! Aspetto: quello del sito (web/src/style.css) e del logo. Un disco scuro, le azioni come
//! palline chiare su un'orbita, la stella al centro. Lo spicchio puntato si accende del colore
//! di accento e un arco attorno al mozzo indica la direzione.
//!
//! Movimento: le animazioni hanno una durata fissa (apertura, chiusura) oppure inseguono un
//! obiettivo (lo spicchio puntato). Un timer ridisegna circa 60 volte al secondo solo finché
//! qualcosa si muove: a ruota ferma la CPU resta a zero. La selezione dipende solo dalla
//! posizione del mouse, mai dall'animazione: si può scegliere anche mentre la ruota si apre.
//! Se in Windows gli effetti di animazione sono disattivati, tutto appare subito.

use std::cell::RefCell;
use std::f32::consts::PI;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Error, HSTRING, PCWSTR, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

use crate::icons;

const CLASS_NAME: PCWSTR = w!("WheelorOverlay");

// Geometria in DIP (pixel logici a 96 DPI): viene scalata in base al monitor.
const RADIUS: f32 = 152.0;
const HUB_RADIUS: f32 = 36.0;
/// Raggio dell'anello attorno al mozzo su cui scorre l'indicatore di direzione.
/// È anche il confine della zona morta: lì dentro non si seleziona nulla.
pub const DEAD_ZONE: f32 = 41.0;
/// Distanza dal centro delle palline.
const ORBIT: f32 = 96.0;
const BALL: f32 = 21.0;
const LABEL_WIDTH: f32 = 64.0;
const SHADOW: f32 = 18.0;
const WINDOW_SIZE: f32 = 2.0 * (RADIUS + SHADOW);

// Palette del sito.
const BG: u32 = 0x081314;
const DISC: u32 = 0x0B191A;
const LINE: u32 = 0xA5CECD; // linee e bordi, sempre con alpha bassa
const SOFT: u32 = 0xB9D3D1;
const HEADING: u32 = 0xEEF7F6;
const ACCENT: u32 = 0x6FE0E6;
const ON_BALL: u32 = 0x0E2A2C;
const ON_ACCENT: u32 = 0x06191B;

// Tempi delle animazioni, in secondi.
const OPEN_FADE: f32 = 0.11;
const OPEN_GROW: f32 = 0.18;
/// Ogni pallina parte dal centro e raggiunge l'orbita; una dopo l'altra, in senso orario.
const BALL_TIME: f32 = 0.20;
const BALL_STAGGER: f32 = 0.010;
const STAR_DELAY: f32 = 0.03;
const STAR_TIME: f32 = 0.28;
const CLOSE_TIME: f32 = 0.12;
// Costanti di tempo degli inseguimenti: in tanti secondi si copre il 63% della distanza.
// Lo spicchio puntato si accende in fretta e si spegne con più calma.
const HEAT_IN: f32 = 0.035;
const HEAT_OUT: f32 = 0.07;
const GLIDE: f32 = 0.045;
const FADE: f32 = 0.05;
/// Passo massimo di un fotogramma: dopo una pausa lunga l'animazione riprende senza salti.
const MAX_STEP: f32 = 0.05;
/// Il primo fotogramma mostra già la ruota un po' avanti, invece di un disco invisibile.
const FIRST_FRAME: f32 = 0.016;

/// Timer dei fotogrammi. Windows arrotonda i timer a circa 16 ms: circa 60 fotogrammi al secondo.
const TIMER_FRAME: usize = 1;
const FRAME_MS: u32 = 10;

// Stella del logo: le curve di Bézier del file SVG, con l'origine spostata al centro della stella.
const STAR_HALF_WIDTH: f32 = 343.0;
const STAR_HALF_HEIGHT: f32 = 419.0;
const STAR_START: (f32, f32) = (-343.0, 0.0);
const STAR_CURVES: [[(f32, f32); 3]; 8] = [
    [(-343.0, 0.0), (-183.818, -24.963), (-111.5, -89.5)],
    [(-10.14, -179.95), (0.0, -419.0), (0.0, -419.0)],
    [(0.0, -419.0), (10.14, -179.95), (111.5, -89.5)],
    [(183.82, -24.963), (343.0, 0.0), (343.0, 0.0)],
    [(343.0, 0.0), (183.82, 24.96), (111.5, 89.5)],
    [(10.14, 179.95), (0.0, 419.0), (0.0, 419.0)],
    [(0.0, 419.0), (-10.14, 179.95), (-111.5, 89.5)],
    [(-183.818, 24.96), (-343.0, 0.0), (-343.0, 0.0)],
];

/// Quello che serve per disegnare uno spicchio.
pub struct SliceView {
    /// Carattere del font Segoe Fluent Icons (vuoto = al suo posto l'iniziale dell'etichetta).
    pub icon: String,
    pub label: String,
    /// Apre un'altra ruota: sulla pallina compare una freccetta.
    pub submenu: bool,
}

/// Cosa mostra il mozzo quando nessuno spicchio è puntato.
pub enum Hub {
    /// La stella del logo: ruota principale.
    Logo,
    /// Il nome della ruota: profilo di un'app.
    Title(String),
    /// Freccia "indietro" e nome: siamo in un sottomenu.
    Back(String),
}

/// Come entra una ruota: le palline escono dal centro quando si apre (anche un sottomenu),
/// rientrano verso l'orbita quando si torna indietro.
#[derive(Clone, Copy, PartialEq)]
pub enum Entrance {
    Open,
    Back,
}

/// Glifo "indietro" di Segoe Fluent Icons.
const BACK_GLYPH: &str = "\u{E72B}";

thread_local! {
    // Stato della ruota. thread_local: esiste solo sul thread UI.
    // RefCell: permette di modificarlo, controllando i prestiti a runtime.
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

/// Crea la finestra (nascosta) e le risorse di disegno. Da chiamare una volta all'avvio,
/// così all'apertura resta solo da disegnare.
pub fn init(instance: HINSTANCE) -> Result<()> {
    let overlay = Overlay::new(instance)?;
    OVERLAY.set(Some(overlay));
    Ok(())
}

/// Libera finestra e risorse di disegno. Va chiamata prima di uscire: lasciarle ai distruttori
/// dei thread_local vorrebbe dire rilasciare oggetti Direct2D e DirectWrite mentre Windows sta
/// già scaricando le loro librerie, e il processo andrebbe in crash proprio alla chiusura.
pub fn shutdown() {
    if let Some(overlay) = OVERLAY.take() {
        unsafe {
            let _ = DestroyWindow(overlay.hwnd);
        }
        // `overlay` esce di scope qui: le interfacce COM vengono rilasciate (Release) adesso.
    }
}

pub fn show(cursor: POINT, entrance: Entrance) -> Result<()> {
    with_overlay(|o| o.show(cursor, entrance))
}

/// Nasconde subito la ruota: si usa quando parte un'azione, che non deve trovarsela davanti
/// (per esempio uno screenshot).
pub fn hide() {
    with_overlay(|o| o.hide());
}

/// Chiude la ruota con una breve dissolvenza: si usa quando la scelta viene annullata.
pub fn dismiss() {
    with_overlay(|o| o.dismiss());
}

/// Ingrandisce la ruota (1.0 = misura normale); vale dalla prossima `show`.
pub fn set_zoom(zoom: f32) {
    with_overlay(|o| o.zoom = zoom);
}

/// Imposta il contenuto della ruota; si vede alla prossima `show`.
pub fn set_wheel(slices: Vec<SliceView>, hub: Hub) {
    with_overlay(|o| {
        o.slices = slices;
        o.hub = hub;
    });
}

/// Aggiorna lo spicchio evidenziato in base alla posizione del mouse (pixel fisici).
pub fn hover_at(point: POINT) {
    with_overlay(|o| o.hover_at(point));
}

/// Spicchio attualmente evidenziato, se c'è.
pub fn hovered() -> Option<usize> {
    with_overlay(|o| o.hover)
}

/// Esegue `f` sull'overlay. `expect` interrompe il programma se `init` non è stato chiamato:
/// sarebbe un bug nostro, non un errore da gestire.
fn with_overlay<R>(f: impl FnOnce(&mut Overlay) -> R) -> R {
    OVERLAY.with_borrow_mut(|o| f(o.as_mut().expect("overlay::init non chiamato")))
}

struct Overlay {
    hwnd: HWND,
    factory: ID2D1Factory,
    target: ID2D1DCRenderTarget,
    paint: Paint,
    icon_format: IDWriteTextFormat,
    label_format: IDWriteTextFormat,
    hub_format: IDWriteTextFormat,
    initial_format: IDWriteTextFormat,
    slices: Vec<SliceView>,
    hub: Hub,
    /// Presente solo mentre la ruota è visibile.
    frame: Option<Frame>,
    hover: Option<usize>,
    motion: Motion,
    /// Dimensione scelta nelle impostazioni (1.0 = normale).
    zoom: f32,
    /// Il timer dei fotogrammi è attivo.
    ticking: bool,
    last_frame: Instant,
}

/// Pennelli e stili creati una volta sola sul render target. I gradienti radiali vengono
/// spostati e ridimensionati a ogni uso (SetCenter, SetRadius…), senza ricrearli.
struct Paint {
    solid: ID2D1SolidColorBrush,
    /// Ombra del disco: piena fin quasi al bordo, poi sfuma.
    disc_shadow: ID2D1RadialGradientBrush,
    /// Macchia morbida nera: ombra delle palline.
    shade: ID2D1RadialGradientBrush,
    /// Macchia morbida del colore di accento: bagliori.
    glow: ID2D1RadialGradientBrush,
    /// Pallina chiara, illuminata dall'alto a destra come sul sito.
    ball: ID2D1RadialGradientBrush,
    /// Riflesso bianco sulla pallina accesa.
    shine: ID2D1RadialGradientBrush,
    star: ID2D1RadialGradientBrush,
    round: ID2D1StrokeStyle,
}

/// Dove e come è mostrata la ruota in questo momento.
struct Frame {
    surface: Surface,
    /// Angolo in alto a sinistra della finestra (pixel fisici).
    origin: POINT,
    /// Centro della ruota (pixel fisici).
    center: POINT,
    size: i32,
    /// DPI del monitor / 96.
    scale: f32,
}

/// Lo stato delle animazioni. `draw` lo legge e basta: tutto si muove in `step`.
struct Motion {
    /// false se in Windows gli effetti di animazione sono disattivati.
    enabled: bool,
    entrance: Entrance,
    /// Secondi dall'apertura.
    opened: f32,
    /// Secondi dall'inizio della chiusura, se la ruota si sta chiudendo.
    closing: Option<f32>,
    /// Quanto è acceso ogni spicchio, da 0 a 1.
    heat: Vec<f32>,
    /// Direzione dell'arco attorno al mozzo (radianti) e sua opacità.
    pointer: f32,
    pointer_alpha: f32,
    /// Il mozzo passa dal suo contenuto (0) al nome dello spicchio puntato (1).
    hub_mix: f32,
    /// L'ultimo spicchio puntato: il suo nome resta nel mozzo mentre sfuma.
    hub_index: Option<usize>,
}

impl Overlay {
    fn new(instance: HINSTANCE) -> Result<Self> {
        unsafe {
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(Error::from_thread());
            }

            // LAYERED: trasparenza per pixel. TRANSPARENT: i clic la attraversano.
            // NOACTIVATE: non ruba mai il focus all'app in uso. TOOLWINDOW: fuori da taskbar e Alt+Tab.
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS_NAME,
                w!("Wheelor"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )?;

            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            // SOFTWARE: disegno su CPU, senza caricare il driver della GPU (molta meno RAM).
            // Per una bitmap di poche centinaia di pixel bastano pochi millisecondi.
            let target = factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                ..Default::default()
            })?;
            // Il ClearType non funziona su sfondo trasparente: antialiasing in scala di grigi.
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let paint = Paint::new(&factory, &target)?;

            // Su Windows 10 mancano i font di Windows 11: si ripiega su quelli equivalenti.
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icons = HSTRING::from(icons::font_family());
            let text = HSTRING::from(match icons::font_installed("Segoe UI Variable Text") {
                true => "Segoe UI Variable Text",
                false => "Segoe UI",
            });
            let icon_format = text_format(&dwrite, &icons, 18.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let label_format = text_format(&dwrite, &text, 11.5, DWRITE_FONT_WEIGHT_NORMAL)?;
            let hub_format = text_format(&dwrite, &text, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let initial_format = text_format(&dwrite, &text, 16.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            // Le etichette troppo lunghe finiscono con "…"; nel mozzo possono andare su due righe.
            trim_with_ellipsis(&dwrite, &label_format)?;
            trim_with_ellipsis(&dwrite, &hub_format)?;
            hub_format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;

            Ok(Self {
                hwnd,
                factory,
                target,
                paint,
                icon_format,
                label_format,
                hub_format,
                initial_format,
                slices: Vec::new(),
                hub: Hub::Logo,
                frame: None,
                hover: None,
                motion: Motion::new(0, Entrance::Open, false),
                zoom: 1.0,
                ticking: false,
                last_frame: Instant::now(),
            })
        }
    }

    fn show(&mut self, cursor: POINT, entrance: Entrance) -> Result<()> {
        let (dpi, work) = monitor_at(cursor)?;
        // Ingrandire la ruota equivale a disegnarla come su uno schermo con più DPI:
        // tutto cresce insieme e resta nitido.
        let dpi = dpi as f32 * self.zoom;

        // Dimensione in pixel reali; centrata sul cursore ma sempre dentro l'area di lavoro.
        let size = (WINDOW_SIZE * dpi / 96.0).round() as i32;
        let origin = POINT {
            x: (cursor.x - size / 2).min(work.right - size).max(work.left),
            y: (cursor.y - size / 2).min(work.bottom - size).max(work.top),
        };

        // Vicino ai bordi la ruota non è centrata sul cursore: lo spostiamo al centro,
        // così la selezione per direzione corrisponde a quello che si vede.
        let frame = Frame::new(&self.target, dpi, origin)?;
        if frame.center != cursor {
            unsafe {
                let _ = SetCursorPos(frame.center.x, frame.center.y);
            }
        }

        self.frame = Some(frame);
        self.hover = None;
        self.motion = Motion::new(self.slices.len(), entrance, animations_enabled());
        self.present()?;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        self.animate();
        Ok(())
    }

    fn hide(&mut self) {
        self.stop_ticking();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        // Assegnando None, il vecchio Frame viene distrutto e la sua Surface liberata (Drop).
        self.frame = None;
        self.hover = None;
        self.motion.closing = None;
    }

    fn dismiss(&mut self) {
        if self.frame.is_none() {
            return;
        }
        if !self.motion.enabled {
            self.hide();
            return;
        }
        self.hover = None;
        // Una seconda richiesta non fa ripartire la dissolvenza già in corso.
        self.motion.closing.get_or_insert(0.0);
        self.animate();
    }

    fn hover_at(&mut self, point: POINT) {
        // `let ... else`: se la ruota non è visibile (o si sta chiudendo) non c'è niente da fare.
        let Some(frame) = &self.frame else { return };
        if self.motion.closing.is_some() {
            return;
        }

        let dx = (point.x - frame.center.x) as f32 / frame.scale;
        let dy = (point.y - frame.center.y) as f32 / frame.scale;
        // Nel mozzo non si seleziona nulla; fuori conta solo la direzione, anche oltre il bordo.
        let n = self.slices.len();
        let hover = if n == 0 || dx.hypot(dy) < DEAD_ZONE { None } else { Some(slice_at(dy.atan2(dx), n)) };

        if hover != self.hover {
            self.hover = hover;
            self.animate();
        }
    }

    /// Fa partire il timer dei fotogrammi (se non è già attivo) e disegna subito il primo passo,
    /// senza aspettare il timer: la risposta al mouse è immediata.
    fn animate(&mut self) {
        if !self.ticking {
            self.ticking = true;
            // Il primo passo vale mezzo fotogramma: parte subito, ma senza salti.
            self.last_frame = Instant::now() - Duration::from_millis(8);
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_FRAME, FRAME_MS, None);
            }
        }
        self.tick();
    }

    /// Un fotogramma: fa avanzare le animazioni e ridisegna. Quando tutto è fermo, spegne il timer.
    fn tick(&mut self) {
        if self.frame.is_none() {
            self.stop_ticking();
            return;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(MAX_STEP);
        self.last_frame = now;

        let moving = self.motion.step(dt, self.hover, self.slices.len());
        if self.motion.closed() {
            self.hide();
            return;
        }
        // Un errore di disegno salta un fotogramma: non c'è niente di meglio da fare.
        let _ = self.present();
        if !moving {
            self.stop_ticking();
        }
    }

    fn stop_ticking(&mut self) {
        if self.ticking {
            self.ticking = false;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_FRAME);
            }
        }
    }

    /// Ridisegna la ruota e la consegna alla finestra.
    fn present(&self) -> Result<()> {
        let Some(frame) = &self.frame else { return Ok(()) };
        self.draw()?;

        // La dissolvenza di apertura e chiusura non richiede di ridisegnare: è l'opacità
        // dell'intera finestra, applicata da Windows.
        let (_, alpha) = self.motion.disc();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (alpha * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&frame.origin),
                Some(&SIZE { cx: frame.size, cy: frame.size }),
                Some(frame.surface.dc),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }

    fn draw(&self) -> Result<()> {
        let t = &self.target;
        let p = &self.paint;
        let m = &self.motion;
        let center = Vector2::new(WINDOW_SIZE / 2.0, WINDOW_SIZE / 2.0);
        let n = self.slices.len();

        // Tutto il disco si ingrandisce attorno al centro mentre si apre o si chiude.
        let (scale, _) = m.disc();
        let base = Matrix3x2::scale_around(scale, scale, center);

        unsafe {
            t.BeginDraw();
            t.SetTransform(&base);
            t.Clear(Some(&rgba(0, 0.0)));

            // Ombra morbida, un po' più in basso del disco: la luce viene dall'alto.
            let below = Vector2::new(center.X, center.Y + 4.0);
            set_radial(&p.disc_shadow, below, RADIUS + SHADOW);
            t.FillEllipse(&circle(below, RADIUS + SHADOW), &p.disc_shadow);

            // Disco con un riflesso dell'accento al centro e un bordo sottile.
            self.fill(center, RADIUS, rgba(DISC, 0.97));
            set_radial(&p.glow, center, RADIUS * 0.7);
            p.glow.SetOpacity(0.12);
            t.FillEllipse(&circle(center, RADIUS * 0.7), &p.glow);
            self.stroke(center, RADIUS - 0.5, rgba(LINE, 0.14), 1.0);
            // L'orbita delle palline e l'anello della zona morta, come gli anelli del sito.
            self.stroke(center, ORBIT, rgba(LINE, 0.09), 1.0);
            self.stroke(center, DEAD_ZONE, rgba(LINE, 0.12), 1.0);

            self.draw_pointer(center, n)?;
            for i in 0..n {
                self.draw_ball(center, base, i, n)?;
            }
            t.SetTransform(&base);
            self.draw_hub(center, base)?;

            t.EndDraw(None, None)
        }
    }

    /// Arco luminoso sull'anello del mozzo, rivolto verso lo spicchio puntato.
    fn draw_pointer(&self, center: Vector2, n: usize) -> Result<()> {
        let alpha = self.motion.pointer_alpha;
        if alpha < 0.01 || n == 0 {
            return Ok(());
        }
        // Poco più di metà dello spicchio, ma mai più di 56° (con pochi spicchi sarebbe enorme).
        let half = (PI / n as f32 * 0.6).min(0.49);
        let (from, to) = (self.motion.pointer - half, self.motion.pointer + half);
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(polar(center, from, DEAD_ZONE), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&arc(polar(center, to, DEAD_ZONE), DEAD_ZONE, D2D1_SWEEP_DIRECTION_CLOCKWISE));
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            // Bagliore: tratti larghi e trasparenti sotto quello pieno.
            for (width, strength) in [(11.0, 0.08), (6.5, 0.18), (3.0, 1.0)] {
                self.paint.solid.SetColor(&rgba(ACCENT, strength * alpha));
                self.target.DrawGeometry(&geometry, &self.paint.solid, width, &self.paint.round);
            }
        }
        Ok(())
    }

    /// Una pallina con la sua icona e, sotto, l'etichetta.
    fn draw_ball(&self, center: Vector2, base: Matrix3x2, i: usize, n: usize) -> Result<()> {
        let t = &self.target;
        let p = &self.paint;
        let slice = &self.slices[i];
        let pose = self.motion.ball(i);
        let heat = self.motion.heat.get(i).copied().unwrap_or(0.0);
        if pose.alpha <= 0.0 {
            return Ok(());
        }

        // Quella puntata si sporge un poco verso l'esterno e si ingrandisce.
        let at = polar(center, slice_angle(i, n), ORBIT * pose.radius + 3.0 * heat);
        let scale = pose.scale * (1.0 + 0.12 * heat);
        let r = BALL;
        unsafe {
            // Si disegna la pallina alla sua misura normale e la si ingrandisce con una trasformazione:
            // icona, alone e freccetta crescono insieme a lei.
            t.SetTransform(&(Matrix3x2::scale_around(scale, scale, at) * base));

            // Ombra sotto la pallina; quella accesa galleggia nel suo bagliore.
            let below = Vector2::new(at.X, at.Y + 3.0);
            set_radial(&p.shade, below, r + 7.0);
            p.shade.SetOpacity(0.45 * pose.alpha * (1.0 - 0.6 * heat));
            t.FillEllipse(&circle(below, r + 7.0), &p.shade);

            if heat > 0.005 {
                set_radial(&p.glow, at, r + 26.0);
                p.glow.SetOpacity(0.6 * heat * pose.alpha);
                t.FillEllipse(&circle(at, r + 26.0), &p.glow);
                // Anello tenue che si allarga attorno alla pallina accesa.
                self.fill(at, r + 5.0 * heat, rgba(ACCENT, 0.16 * heat * pose.alpha));
            }

            // La pallina chiara; quella accesa si colora d'accento con un riflesso bianco.
            let light = Vector2::new(at.X + 0.36 * r, at.Y - 0.52 * r);
            set_radial(&p.ball, light, 2.04 * r);
            p.ball.SetOpacity(pose.alpha);
            t.FillEllipse(&circle(at, r), &p.ball);
            if heat > 0.005 {
                self.fill(at, r, rgba(ACCENT, heat * pose.alpha));
                set_radial(&p.shine, light, 2.04 * r);
                p.shine.SetOpacity(heat * pose.alpha);
                t.FillEllipse(&circle(at, r), &p.shine);
            }

            // Icona, oppure l'iniziale dell'etichetta.
            let ink = mix(rgba(ON_BALL, pose.alpha), rgba(ON_ACCENT, pose.alpha), heat);
            p.solid.SetColor(&ink);
            let inside = centered_rect(at.X, at.Y, 2.0 * r, 2.0 * r);
            if slice.icon.is_empty() {
                let initial: String = slice.label.chars().take(1).flat_map(char::to_uppercase).collect();
                draw_text(t, &initial, &self.initial_format, inside, &p.solid);
            } else {
                draw_text(t, &slice.icon, &self.icon_format, inside, &p.solid);
            }

            if slice.submenu {
                self.draw_badge(Vector2::new(at.X + 0.74 * r, at.Y - 0.74 * r), pose.alpha)?;
            }

            // L'etichetta non si ingrandisce: segue solo il bordo della pallina.
            t.SetTransform(&base);
            if pose.label > 0.0 {
                let color = mix(rgba(SOFT, 0.85), rgba(HEADING, 1.0), heat);
                p.solid.SetColor(&D2D1_COLOR_F { a: color.a * pose.label, ..color });
                let y = at.Y + r * scale + 10.0;
                draw_text(t, &slice.label, &self.label_format, centered_rect(at.X, y, LABEL_WIDTH, 16.0), &p.solid);
            }
        }
        Ok(())
    }

    /// Bollino scuro con una freccetta: la pallina apre un sottomenu.
    fn draw_badge(&self, at: Vector2, alpha: f32) -> Result<()> {
        self.fill(at, 7.5, rgba(DISC, alpha));
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(Vector2::new(at.X - 1.3, at.Y - 3.0), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddLine(Vector2::new(at.X + 1.7, at.Y));
            sink.AddLine(Vector2::new(at.X - 1.3, at.Y + 3.0));
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            self.paint.solid.SetColor(&rgba(ACCENT, alpha));
            self.target.DrawGeometry(&geometry, &self.paint.solid, 1.6, &self.paint.round);
        }
        Ok(())
    }

    /// Mozzo centrale: logo, profilo o "indietro"; mentre si punta uno spicchio, il suo nome.
    fn draw_hub(&self, center: Vector2, base: Matrix3x2) -> Result<()> {
        let t = &self.target;
        let m = &self.motion;
        self.fill(center, HUB_RADIUS, rgba(BG, 1.0));
        self.stroke(center, HUB_RADIUS - 0.5, rgba(LINE, 0.16), 1.0);

        // I due contenuti si scambiano con una dissolvenza incrociata.
        let idle = 1.0 - m.hub_mix;
        let text_box = |dy: f32| centered_rect(center.X, center.Y + dy, 2.0 * HUB_RADIUS - 10.0, 34.0);
        if idle > 0.01 {
            match &self.hub {
                Hub::Logo => {
                    // All'apertura la stella compare con un piccolo scatto e una rotazione, come sul sito.
                    let pop = m.star();
                    let size = lerp(0.55, 1.0, ease_out_back(pop)) * (1.0 - 0.25 * m.hub_mix);
                    let turn = lerp(-35.0, 0.0, ease_out(pop));
                    unsafe {
                        set_radial(&self.paint.glow, center, HUB_RADIUS - 2.0);
                        self.paint.glow.SetOpacity(0.32 * idle * pop);
                        t.FillEllipse(&circle(center, HUB_RADIUS - 2.0), &self.paint.glow);
                        t.SetTransform(&(Matrix3x2::rotation_around(turn, center) * base));
                    }
                    self.draw_star(center, 32.0 * size, idle * pop.min(1.0))?;
                    unsafe { t.SetTransform(&base) };
                }
                Hub::Title(name) => {
                    self.paint_text(name, &self.hub_format, text_box(0.0), rgba(HEADING, idle));
                }
                Hub::Back(name) => {
                    let arrow = centered_rect(center.X, center.Y - 11.0, 30.0, 20.0);
                    self.paint_text(BACK_GLYPH, &self.icon_format, arrow, rgba(SOFT, 0.7 * idle));
                    let below = centered_rect(center.X, center.Y + 10.0, 2.0 * HUB_RADIUS - 14.0, 18.0);
                    self.paint_text(name, &self.label_format, below, rgba(HEADING, idle));
                }
            }
        }
        if m.hub_mix > 0.01
            && let Some(slice) = m.hub_index.and_then(|i| self.slices.get(i))
        {
            // Il nome sale di qualche pixel mentre compare.
            let rect = text_box(4.0 * (1.0 - m.hub_mix));
            self.paint_text(&slice.label, &self.hub_format, rect, rgba(HEADING, m.hub_mix));
        }
        Ok(())
    }

    /// La stella del logo (assets/logo.svg), alta `height` DIP, con lo stesso gradiente.
    fn draw_star(&self, center: Vector2, height: f32, alpha: f32) -> Result<()> {
        let scale = height / (2.0 * STAR_HALF_HEIGHT);
        let point = |(x, y): (f32, f32)| Vector2::new(center.X + x * scale, center.Y + y * scale);
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(point(STAR_START), D2D1_FIGURE_BEGIN_FILLED);
            for [p1, p2, p3] in STAR_CURVES {
                sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: point(p1), point2: point(p2), point3: point(p3) });
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;

            let star = &self.paint.star;
            star.SetCenter(center);
            star.SetRadiusX(STAR_HALF_WIDTH * scale);
            star.SetRadiusY(STAR_HALF_HEIGHT * scale);
            star.SetOpacity(alpha);
            self.target.FillGeometry(&geometry, star, None);
        }
        Ok(())
    }

    fn fill(&self, center: Vector2, radius: f32, color: D2D1_COLOR_F) {
        unsafe {
            self.paint.solid.SetColor(&color);
            self.target.FillEllipse(&circle(center, radius), &self.paint.solid);
        }
    }

    fn stroke(&self, center: Vector2, radius: f32, color: D2D1_COLOR_F, width: f32) {
        unsafe {
            self.paint.solid.SetColor(&color);
            self.target.DrawEllipse(&circle(center, radius), &self.paint.solid, width, None);
        }
    }

    fn paint_text(&self, text: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, color: D2D1_COLOR_F) {
        unsafe { self.paint.solid.SetColor(&color) };
        draw_text(&self.target, text, format, rect, &self.paint.solid);
    }
}

impl Paint {
    fn new(factory: &ID2D1Factory, target: &ID2D1DCRenderTarget) -> Result<Self> {
        let black = |alpha| rgba(0, alpha);
        let accent = |alpha| rgba(ACCENT, alpha);
        let white = |alpha| rgba(0xFFFFFF, alpha);
        unsafe {
            Ok(Self {
                solid: target.CreateSolidColorBrush(&rgba(0xFFFFFF, 1.0), None)?,
                disc_shadow: radial(target, &[(0.0, black(0.42)), (0.86, black(0.42)), (0.93, black(0.16)), (1.0, black(0.0))])?,
                shade: radial(target, &[(0.0, black(1.0)), (0.5, black(0.45)), (1.0, black(0.0))])?,
                glow: radial(target, &[(0.0, accent(1.0)), (0.5, accent(0.45)), (1.0, accent(0.0))])?,
                // I colori e le posizioni del gradiente delle palline del sito (.wl-ball).
                ball: radial(target, &[(0.0, rgba(0xF6FCFB, 1.0)), (0.28, rgba(0xD2E8E6, 1.0)), (0.64, rgba(0xA5CECD, 1.0))])?,
                shine: radial(target, &[(0.0, white(0.92)), (0.52, white(0.0))])?,
                star: radial(target, &[(0.0, rgba(0x83EAF0, 1.0)), (1.0, rgba(0x49C8CE, 1.0))])?,
                round: factory.CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        ..Default::default()
                    },
                    None,
                )?,
            })
        }
    }
}

impl Frame {
    /// Prepara una bitmap della misura della ruota per il monitor con questi DPI.
    fn new(target: &ID2D1DCRenderTarget, dpi: f32, origin: POINT) -> Result<Self> {
        let scale = dpi / 96.0;
        let size = (WINDOW_SIZE * scale).round() as i32;
        let surface = Surface::new(size, size)?;
        unsafe {
            target.SetDpi(dpi, dpi);
            target.BindDC(surface.dc, &RECT { left: 0, top: 0, right: size, bottom: size })?;
        }
        let center = POINT { x: origin.x + size / 2, y: origin.y + size / 2 };
        Ok(Self { surface, origin, center, size, scale })
    }
}

/// Posa di una pallina durante l'entrata.
struct BallPose {
    /// Distanza dal centro, in frazioni dell'orbita.
    radius: f32,
    scale: f32,
    alpha: f32,
    /// Opacità dell'etichetta: compare quando la pallina è quasi arrivata.
    label: f32,
}

impl Motion {
    fn new(n: usize, entrance: Entrance, enabled: bool) -> Self {
        Self {
            enabled,
            entrance,
            // Senza animazioni la ruota nasce già aperta.
            opened: if enabled { FIRST_FRAME } else { f32::MAX },
            closing: None,
            heat: vec![0.0; n],
            pointer: 0.0,
            pointer_alpha: 0.0,
            hub_mix: 0.0,
            hub_index: None,
        }
    }

    /// Fa avanzare tutto di `dt` secondi. Restituisce true se qualcosa si sta ancora muovendo.
    fn step(&mut self, dt: f32, hover: Option<usize>, n: usize) -> bool {
        // Senza animazioni ogni passo è lunghissimo: tutto arriva subito a destinazione.
        let dt = if self.enabled { dt } else { 1000.0 };
        self.opened += dt;
        let mut moving = self.opened < self.open_time(n);
        if let Some(closing) = &mut self.closing {
            *closing += dt;
            moving = true;
        }

        for (i, heat) in self.heat.iter_mut().enumerate() {
            let target = if hover == Some(i) { 1.0 } else { 0.0 };
            let tau = if target > *heat { HEAT_IN } else { HEAT_OUT };
            moving |= approach(heat, target, dt, tau);
        }
        if let Some(i) = hover {
            let target = slice_angle(i, n);
            // Se l'arco era spento riappare già nella direzione giusta, senza fare il giro.
            if self.pointer_alpha < 0.05 {
                self.pointer = target;
            }
            moving |= approach_angle(&mut self.pointer, target, dt, GLIDE);
            self.hub_index = Some(i);
        }
        let shown = if hover.is_some() { 1.0 } else { 0.0 };
        moving |= approach(&mut self.pointer_alpha, shown, dt, FADE);
        moving |= approach(&mut self.hub_mix, shown, dt, FADE);
        moving
    }

    /// Durata dell'entrata: l'ultima pallina parte dopo tutte le altre.
    fn open_time(&self, n: usize) -> f32 {
        let balls = BALL_STAGGER * n.saturating_sub(1) as f32 + BALL_TIME;
        OPEN_GROW.max(balls).max(STAR_DELAY + STAR_TIME)
    }

    fn closed(&self) -> bool {
        self.closing.is_some_and(|t| t >= CLOSE_TIME)
    }

    /// Scala del disco e opacità della finestra.
    fn disc(&self) -> (f32, f32) {
        let from = match self.entrance {
            Entrance::Open => 0.92,
            Entrance::Back => 1.05,
        };
        let mut scale = lerp(from, 1.0, ease_out(progress(self.opened, 0.0, OPEN_GROW)));
        let mut alpha = ease_out(progress(self.opened, 0.0, OPEN_FADE));
        if let Some(closing) = self.closing {
            let k = ease_out(progress(closing, 0.0, CLOSE_TIME));
            scale *= lerp(1.0, 0.95, k);
            alpha *= 1.0 - k;
        }
        (scale, alpha)
    }

    fn ball(&self, i: usize) -> BallPose {
        let (stagger, from, scale_from) = match self.entrance {
            Entrance::Open => (BALL_STAGGER, 0.55, 0.6),
            Entrance::Back => (0.0, 1.12, 0.9),
        };
        let t = progress(self.opened, i as f32 * stagger, BALL_TIME);
        // In apertura un leggero rimbalzo oltre l'orbita; tornando indietro, nessuno.
        let travel = match self.entrance {
            Entrance::Open => ease_out_back(t),
            Entrance::Back => ease_out(t),
        };
        BallPose {
            radius: lerp(from, 1.0, travel),
            scale: lerp(scale_from, 1.0, ease_out(t)),
            alpha: (t * 3.0).min(1.0),
            label: progress(t, 0.3, 0.7),
        }
    }

    /// Avanzamento della stella del logo (0..1). Tornando indietro è già al suo posto.
    fn star(&self) -> f32 {
        match self.entrance {
            Entrance::Open => progress(self.opened, STAR_DELAY, STAR_TIME),
            Entrance::Back => 1.0,
        }
    }
}

/// Bitmap in memoria (32 bit BGRA) su cui disegna Direct2D.
/// Implementa `Drop`: quando esce di scope, le risorse GDI vengono liberate in automatico.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    /// I pixel della bitmap (li leggono i test).
    #[cfg_attr(not(test), allow(dead_code))]
    bits: *mut u32,
}

impl Surface {
    fn new(width: i32, height: i32) -> Result<Self> {
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // negativo = righe dall'alto verso il basso
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let dc = CreateCompatibleDC(None);
            let previous = SelectObject(dc, bitmap.into());
            Ok(Self { dc, bitmap, previous, bits: bits.cast() })
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Rete di sicurezza: anche se in qualche modo cliccata, la finestra non si attiva.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER if wparam.0 == TIMER_FRAME => {
            // try_borrow_mut invece di with_borrow_mut: se per qualche motivo l'overlay fosse già
            // in uso, si salta un fotogramma invece di andare in panic.
            OVERLAY.with(|cell| {
                if let Ok(mut overlay) = cell.try_borrow_mut()
                    && let Some(overlay) = overlay.as_mut()
                {
                    overlay.tick();
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// DPI e area di lavoro (schermo meno taskbar) del monitor che contiene `point`.
fn monitor_at(point: POINT) -> Result<(u32, RECT)> {
    unsafe {
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let (mut dpi_x, mut dpi_y) = (96, 96);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)?;
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(monitor, &mut info).ok()?;
        Ok((dpi_x, info.rcWork))
    }
}

/// Impostazioni > Accessibilità > Effetti visivi > Effetti di animazione.
fn animations_enabled() -> bool {
    let mut enabled = BOOL(1);
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut BOOL as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // Se non si riesce a leggere, si anima: è il comportamento predefinito di Windows.
    read.is_err() || enabled.as_bool()
}

fn text_format(
    dwrite: &IDWriteFactory,
    family: &HSTRING,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = dwrite.CreateTextFormat(
            family,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!(""),
        )?;
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }
}

/// Il testo che non entra nel suo rettangolo finisce con "…" invece di uscire dai bordi.
fn trim_with_ellipsis(dwrite: &IDWriteFactory, format: &IDWriteTextFormat) -> Result<()> {
    unsafe {
        let sign = dwrite.CreateEllipsisTrimmingSign(format)?;
        let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, ..Default::default() };
        format.SetTrimming(&trimming, &sign)
    }
}

fn draw_text(
    target: &ID2D1DCRenderTarget,
    text: &str,
    format: &IDWriteTextFormat,
    rect: D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        target.DrawText(
            &wide,
            format,
            &rect,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

/// Pennello a gradiente radiale. `stops`: posizione (0 = centro, 1 = bordo) e colore.
fn radial(target: &ID2D1DCRenderTarget, stops: &[(f32, D2D1_COLOR_F)]) -> Result<ID2D1RadialGradientBrush> {
    let stops: Vec<D2D1_GRADIENT_STOP> =
        stops.iter().map(|&(position, color)| D2D1_GRADIENT_STOP { position, color }).collect();
    unsafe {
        let stops = target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
        let properties = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
            center: Vector2::zero(),
            gradientOriginOffset: Vector2::zero(),
            radiusX: 1.0,
            radiusY: 1.0,
        };
        target.CreateRadialGradientBrush(&properties, None, &stops)
    }
}

/// Sposta un gradiente radiale: centro in `center`, bordo a distanza `radius`.
fn set_radial(brush: &ID2D1RadialGradientBrush, center: Vector2, radius: f32) {
    unsafe {
        brush.SetCenter(center);
        brush.SetRadiusX(radius);
        brush.SetRadiusY(radius);
    }
}

/// Angolo (radianti) al centro dello spicchio `i` di `n`: il primo è in alto, poi in senso orario.
fn slice_angle(i: usize, n: usize) -> f32 {
    -PI / 2.0 + i as f32 * 2.0 * PI / n as f32
}

/// Indice dello spicchio che contiene la direzione `angle` (radianti, come restituito da `atan2`).
pub fn slice_at(angle: f32, n: usize) -> usize {
    let step = 2.0 * PI / n as f32;
    // Ruotiamo in modo che lo spicchio 0 (in alto) inizi da 0 radianti.
    let turned = (angle + PI / 2.0 + step / 2.0).rem_euclid(2.0 * PI);
    (turned / step) as usize % n
}

/// Avvicina `value` a `target` in modo esponenziale: in `tau` secondi copre il 63% della distanza,
/// e un nuovo obiettivo a metà strada viene inseguito senza scatti.
/// Restituisce true se manca ancora qualcosa (sotto una soglia minima, arriva e basta).
fn approach(value: &mut f32, target: f32, dt: f32, tau: f32) -> bool {
    *value = target + (*value - target) * (-dt / tau).exp();
    if (*value - target).abs() < 0.002 {
        *value = target;
        false
    } else {
        true
    }
}

/// Come `approach`, ma per un angolo: gira dalla parte più corta.
fn approach_angle(angle: &mut f32, target: f32, dt: f32, tau: f32) -> bool {
    // Differenza tra -PI e PI: da 350° a 10° si passa per 0°, non per 180°.
    let mut rest = (target - *angle + PI).rem_euclid(2.0 * PI) - PI;
    let moving = approach(&mut rest, 0.0, dt, tau);
    *angle = target - rest;
    moving
}

/// Quanto è avanzata un'animazione che parte dopo `delay` e dura `duration` (0..1).
fn progress(time: f32, delay: f32, duration: f32) -> f32 {
    ((time - delay) / duration).clamp(0.0, 1.0)
}

/// Parte veloce e rallenta all'arrivo: la curva più naturale per ciò che entra o risponde.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Come `ease_out`, ma supera l'arrivo del 7% circa e ci torna: un piccolo rimbalzo.
fn ease_out_back(t: f32) -> f32 {
    const S: f32 = 1.4;
    let u = t - 1.0;
    1.0 + (S + 1.0) * u.powi(3) + S * u.powi(2)
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// Colore intermedio tra `a` (t = 0) e `b` (t = 1).
fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, t: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: lerp(a.r, b.r, t), g: lerp(a.g, b.g, t), b: lerp(a.b, b.b, t), a: lerp(a.a, b.a, t) }
}

/// Punto a distanza `radius` dal centro, lungo la direzione `angle`.
fn polar(center: Vector2, angle: f32, radius: f32) -> Vector2 {
    let (sin, cos) = angle.sin_cos();
    Vector2::new(center.X + cos * radius, center.Y + sin * radius)
}

fn arc(to: Vector2, radius: f32, sweep: D2D1_SWEEP_DIRECTION) -> D2D1_ARC_SEGMENT {
    D2D1_ARC_SEGMENT {
        point: to,
        size: D2D_SIZE_F { width: radius, height: radius },
        rotationAngle: 0.0,
        sweepDirection: sweep,
        arcSize: D2D1_ARC_SIZE_SMALL,
    }
}

fn circle(center: Vector2, radius: f32) -> D2D1_ELLIPSE {
    D2D1_ELLIPSE { point: center, radiusX: radius, radiusY: radius }
}

fn centered_rect(cx: f32, cy: f32, width: f32, height: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: cx - width / 2.0,
        top: cy - height / 2.0,
        right: cx + width / 2.0,
        bottom: cy + height / 2.0,
    }
}

/// Colore da esadecimale, come nei CSS, con la sua opacità: rgba(0x6FE0E6, 0.5).
fn rgba(hex: u32, alpha: f32) -> D2D1_COLOR_F {
    let channel = |shift: u32| ((hex >> shift) & 0xFF) as f32 / 255.0;
    D2D1_COLOR_F { r: channel(16), g: channel(8), b: channel(0), a: alpha }
}

// I test vengono compilati solo con `cargo test`, mai nell'exe finale.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_at_points_in_the_right_direction() {
        // In Windows l'asse y va verso il basso: -PI/2 è "su", PI/2 è "giù".
        assert_eq!(slice_at(-PI / 2.0, 8), 0); // su
        assert_eq!(slice_at(0.0, 8), 2); // destra
        assert_eq!(slice_at(PI / 2.0, 8), 4); // giù
        assert_eq!(slice_at(PI, 8), 6); // sinistra
        assert_eq!(slice_at(-PI, 8), 6); // sinistra, dall'altro lato
    }

    #[test]
    fn slice_at_handles_edges_between_slices() {
        let step = 2.0 * PI / 8.0;
        // Appena a sinistra e appena a destra del bordo tra lo spicchio 0 e l'1.
        assert_eq!(slice_at(-PI / 2.0 + step / 2.0 - 0.01, 8), 0);
        assert_eq!(slice_at(-PI / 2.0 + step / 2.0 + 0.01, 8), 1);
    }

    #[test]
    fn slice_at_matches_slice_angle() {
        for n in 2..=12 {
            for i in 0..n {
                assert_eq!(slice_at(slice_angle(i, n), n), i, "n = {n}, i = {i}");
            }
        }
    }

    #[test]
    fn approach_angle_takes_the_short_way() {
        // Da 170° a -170°: 20° passando per 180°, non 340° all'indietro.
        let mut angle = 170f32.to_radians();
        approach_angle(&mut angle, (-170f32).to_radians(), 0.01, 0.05);
        let degrees = angle.to_degrees().rem_euclid(360.0);
        assert!(degrees > 170.0 && degrees < 190.0, "angolo: {degrees}");
    }

    #[test]
    fn motion_settles() {
        // Qualunque cosa succeda, dopo un po' l'animazione si ferma (e con lei il timer).
        let mut motion = Motion::new(8, Entrance::Open, true);
        let mut frames = 0;
        while motion.step(0.016, Some(3), 8) {
            frames += 1;
            assert!(frames < 100, "l'animazione non si ferma");
        }
        assert_eq!(motion.heat[3], 1.0);
        assert_eq!(motion.pointer, slice_angle(3, 8));

        motion.closing = Some(0.0);
        while motion.step(0.016, None, 8) && !motion.closed() {}
        assert!(motion.closed());
    }

    #[test]
    fn motion_off_jumps_to_the_end() {
        let mut motion = Motion::new(8, Entrance::Open, false);
        assert!(!motion.step(0.016, Some(2), 8));
        assert_eq!(motion.heat[2], 1.0);
        assert_eq!(motion.disc(), (1.0, 1.0));
    }

    /// Disegna la ruota in vari momenti delle animazioni e salva un foglio di provini in
    /// target/wheel.bmp, sopra uno sfondo che imita un desktop. Serve a controllare l'aspetto:
    /// `cargo test snapshots -- --ignored --nocapture`
    #[test]
    #[ignore = "scrive un'immagine in target/"]
    fn snapshots() {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;

        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let mut overlay = Overlay::new(instance).unwrap();
        let dpi = 144.0; // 150%
        let slice = |icon: &str, label: &str, submenu: bool| SliceView {
            icon: if icon.is_empty() { String::new() } else { icons::glyph(icon) },
            label: label.into(),
            submenu,
        };
        let eight = || {
            vec![
                slice("E8C8", "Copia", false),
                slice("E722", "Screenshot", false),
                slice("E756", "Terminale", false),
                slice("E721", "Cerca", false),
                slice("E77F", "Incolla", false),
                slice("E8D6", "Media", true),
                slice("E72E", "Blocca schermo", false),
                slice("", "Nota veloce", false),
            ]
        };

        // (descrizione, spicchi, mozzo, entrata, istante dell'apertura, spicchio acceso, chiusura)
        type Shot = (&'static str, Vec<SliceView>, Hub, Entrance, f32, Option<(usize, f32)>, Option<f32>);
        let shots: Vec<Shot> = vec![
            ("apertura 30 ms", eight(), Hub::Logo, Entrance::Open, 0.03, None, None),
            ("apertura 80 ms", eight(), Hub::Logo, Entrance::Open, 0.08, None, None),
            ("apertura 140 ms", eight(), Hub::Logo, Entrance::Open, 0.14, None, None),
            ("aperta", eight(), Hub::Logo, Entrance::Open, 1.0, None, None),
            ("puntata", eight(), Hub::Logo, Entrance::Open, 1.0, Some((1, 1.0)), None),
            ("passaggio", eight(), Hub::Logo, Entrance::Open, 1.0, Some((2, 0.5)), None),
            ("sottomenu", eight().into_iter().take(5).collect(), Hub::Back("Media".into()), Entrance::Open, 1.0, Some((4, 1.0)), None),
            ("profilo", eight().into_iter().take(3).collect(), Hub::Title("VS Code".into()), Entrance::Open, 1.0, None, None),
            ("indietro 60 ms", eight(), Hub::Logo, Entrance::Back, 0.06, None, None),
            ("chiusura 50 ms", eight(), Hub::Logo, Entrance::Open, 1.0, Some((1, 1.0)), Some(0.05)),
        ];

        let tile = (WINDOW_SIZE * dpi / 96.0).round() as usize;
        let columns = 4;
        let rows = shots.len().div_ceil(columns);
        let (width, height) = (tile * columns, tile * rows);
        // Sfondo: un gradiente dal chiaro allo scuro, per vedere la ruota su entrambi.
        let mut sheet: Vec<[f32; 3]> = (0..width * height)
            .map(|i| {
                let (x, y) = ((i % width) as f32 / width as f32, (i / width) as f32 / height as f32);
                let k = (x * 0.6 + y * 0.4).clamp(0.0, 1.0);
                [lerp(0.86, 0.12, k), lerp(0.88, 0.20, k), lerp(0.90, 0.30, k)]
            })
            .collect();

        let mut timings = Vec::new();
        for (index, (name, slices, hub, entrance, opened, hover, closing)) in shots.into_iter().enumerate() {
            let n = slices.len();
            overlay.slices = slices;
            overlay.hub = hub;
            overlay.frame = Some(Frame::new(&overlay.target, dpi, POINT::default()).unwrap());
            let mut motion = Motion::new(n, entrance, true);
            motion.opened = opened;
            if let Some((i, heat)) = hover {
                motion.heat[i] = heat;
                motion.pointer = slice_angle(i, n);
                if heat < 1.0 {
                    // A metà strada tra lo spicchio precedente e questo.
                    motion.heat[i - 1] = 1.0 - heat;
                    motion.pointer = lerp(slice_angle(i - 1, n), slice_angle(i, n), heat);
                }
                motion.pointer_alpha = 1.0;
                motion.hub_mix = 1.0;
                motion.hub_index = Some(i);
            }
            motion.closing = closing;
            overlay.motion = motion;

            let start = Instant::now();
            overlay.draw().unwrap();
            timings.push((name, start.elapsed()));
            unsafe {
                let _ = GdiFlush();
            }

            let (_, alpha) = overlay.motion.disc();
            let frame = overlay.frame.as_ref().unwrap();
            let pixels = unsafe { std::slice::from_raw_parts(frame.surface.bits, tile * tile) };
            let (left, top) = ((index % columns) * tile, (index / columns) * tile);
            for (i, &pixel) in pixels.iter().enumerate() {
                let (x, y) = (left + i % tile, top + i / tile);
                let dst = &mut sheet[y * width + x];
                // Pixel premoltiplicati: risultato = sorgente + sfondo × (1 - alpha), come fa Windows.
                let a = (pixel >> 24) as f32 / 255.0 * alpha;
                for (c, shift) in [(0, 16), (1, 8), (2, 0)] {
                    let src = ((pixel >> shift) & 0xFF) as f32 / 255.0 * alpha;
                    dst[c] = src + dst[c] * (1.0 - a);
                }
            }
        }

        for (name, time) in timings {
            println!("{name:>16}: {:.2} ms", time.as_secs_f64() * 1000.0);
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("wheel.bmp");
        write_bmp(&path, width, height, &sheet);
        println!("{}", path.display());
    }

    /// BMP a 24 bit, dall'alto verso il basso.
    fn write_bmp(path: &std::path::Path, width: usize, height: usize, pixels: &[[f32; 3]]) {
        let row = (width * 3).div_ceil(4) * 4;
        let mut data = Vec::with_capacity(54 + row * height);
        let size = (54 + row * height) as u32;
        data.extend_from_slice(b"BM");
        data.extend_from_slice(&size.to_le_bytes());
        data.extend_from_slice(&[0; 4]);
        data.extend_from_slice(&54u32.to_le_bytes());
        data.extend_from_slice(&40u32.to_le_bytes());
        data.extend_from_slice(&(width as i32).to_le_bytes());
        data.extend_from_slice(&(-(height as i32)).to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&24u16.to_le_bytes());
        data.extend_from_slice(&[0; 24]);
        for y in 0..height {
            let start = data.len();
            for x in 0..width {
                let [r, g, b] = pixels[y * width + x];
                let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                data.extend_from_slice(&[byte(b), byte(g), byte(r)]);
            }
            data.resize(start + row, 0);
        }
        std::fs::write(path, data).unwrap();
    }
}
