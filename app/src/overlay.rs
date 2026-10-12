//! La ruota: finestra trasparente sempre in primo piano. Viene disegnata con Direct2D
//! su una bitmap in memoria e mostrata con UpdateLayeredWindow (alpha per pixel).

use std::cell::RefCell;
use std::f32::consts::PI;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Error, HSTRING, PCWSTR, Result, w};
use windows_numerics::Vector2;

use crate::icons;

const CLASS_NAME: PCWSTR = w!("WheelorOverlay");

// Geometria in DIP (pixel logici a 96 DPI): viene scalata in base al monitor.
const OUTER_RADIUS: f32 = 140.0;
const INNER_RADIUS: f32 = 46.0;
const SHADOW: f32 = 14.0;
const HIGHLIGHT_GAP: f32 = 4.0;
const WINDOW_SIZE: f32 = 2.0 * (OUTER_RADIUS + SHADOW);

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
    /// Carattere del font Segoe Fluent Icons (vuoto = nessuna icona).
    pub icon: String,
    pub label: String,
    /// Apre un'altra ruota: sul bordo compare una freccetta verso l'esterno.
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

pub fn show(cursor: POINT) -> Result<()> {
    with_overlay(|o| o.show(cursor))
}

pub fn hide() {
    with_overlay(|o| o.hide());
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
    let _ = with_overlay(|o| o.hover_at(point));
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
    icon_format: IDWriteTextFormat,
    label_format: IDWriteTextFormat,
    hub_format: IDWriteTextFormat,
    slices: Vec<SliceView>,
    hub: Hub,
    /// Presente solo mentre la ruota è visibile.
    frame: Option<Frame>,
    hover: Option<usize>,
    accent: D2D1_COLOR_F,
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
            // Per una bitmap di poche centinaia di pixel bastano frazioni di millisecondo.
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

            // Su Windows 10 mancano i font di Windows 11: si ripiega su quelli equivalenti.
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icons = HSTRING::from(icons::font_family());
            let text = HSTRING::from(match icons::font_installed("Segoe UI Variable Text") {
                true => "Segoe UI Variable Text",
                false => "Segoe UI",
            });
            let icon_format = text_format(&dwrite, &icons, 20.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let label_format = text_format(&dwrite, &text, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let hub_format = text_format(&dwrite, &text, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;

            Ok(Self {
                hwnd,
                factory,
                target,
                icon_format,
                label_format,
                hub_format,
                slices: Vec::new(),
                hub: Hub::Logo,
                frame: None,
                hover: None,
                accent: accent_color(),
            })
        }
    }

    fn show(&mut self, cursor: POINT) -> Result<()> {
        let (dpi, work) = monitor_at(cursor)?;
        let scale = dpi as f32 / 96.0;

        // Dimensione in pixel reali; centrata sul cursore ma sempre dentro l'area di lavoro.
        let size = (WINDOW_SIZE * scale).round() as i32;
        let origin = POINT {
            x: (cursor.x - size / 2).min(work.right - size).max(work.left),
            y: (cursor.y - size / 2).min(work.bottom - size).max(work.top),
        };
        let center = POINT { x: origin.x + size / 2, y: origin.y + size / 2 };

        // Vicino ai bordi la ruota non è centrata sul cursore: lo spostiamo al centro,
        // così la selezione per direzione corrisponde a quello che si vede.
        if center != cursor {
            unsafe {
                let _ = SetCursorPos(center.x, center.y);
            }
        }

        let surface = Surface::new(size, size)?;
        unsafe {
            self.target.SetDpi(dpi as f32, dpi as f32);
            self.target.BindDC(surface.dc, &RECT { left: 0, top: 0, right: size, bottom: size })?;
        }
        self.accent = accent_color();
        self.hover = None;
        self.frame = Some(Frame { surface, origin, center, size, scale });
        self.present()?;

        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(())
    }

    fn hide(&mut self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        // Assegnando None, il vecchio Frame viene distrutto e la sua Surface liberata (Drop).
        self.frame = None;
        self.hover = None;
    }

    fn hover_at(&mut self, point: POINT) -> Result<()> {
        // `let ... else`: se la ruota non è visibile non c'è niente da fare.
        let Some(frame) = &self.frame else { return Ok(()) };

        let dx = (point.x - frame.center.x) as f32 / frame.scale;
        let dy = (point.y - frame.center.y) as f32 / frame.scale;
        // Nel mozzo non si seleziona nulla; fuori conta solo la direzione, anche oltre il bordo.
        let n = self.slices.len();
        let hover = if n == 0 || dx.hypot(dy) < INNER_RADIUS {
            None
        } else {
            Some(slice_at(dy.atan2(dx), n))
        };

        if hover != self.hover {
            self.hover = hover;
            self.present()?;
        }
        Ok(())
    }

    /// Ridisegna la ruota e la consegna alla finestra.
    fn present(&self) -> Result<()> {
        let Some(frame) = &self.frame else { return Ok(()) };
        self.draw()?;

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
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
        let center = Vector2::new(WINDOW_SIZE / 2.0, WINDOW_SIZE / 2.0);
        let n = self.slices.len();
        let white = |alpha| color(1.0, 1.0, 1.0, alpha);
        // Sul colore di accento il testo deve restare leggibile, anche se l'accento è chiaro.
        let on_accent = if is_light(&self.accent) { color(0.0, 0.0, 0.0, 0.9) } else { white(1.0) };

        unsafe {
            t.BeginDraw();
            t.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            let brush = t.CreateSolidColorBrush(&white(1.0), None)?;

            // Ombra morbida: cerchi concentrici sempre più trasparenti verso l'esterno.
            for i in 0..SHADOW as i32 {
                let fade = 1.0 - i as f32 / SHADOW;
                brush.SetColor(&color(0.0, 0.0, 0.0, 0.04 * fade));
                t.FillEllipse(&circle(center, OUTER_RADIUS + i as f32), &brush);
            }

            // Corpo della ruota con un bordo sottile.
            brush.SetColor(&color(0.13, 0.13, 0.14, 0.97));
            t.FillEllipse(&circle(center, OUTER_RADIUS), &brush);
            brush.SetColor(&white(0.10));
            t.DrawEllipse(&circle(center, OUTER_RADIUS - 0.5), &brush, 1.0, None);

            // Spicchio evidenziato (con un solo spicchio è tutta la corona).
            if let Some(index) = self.hover {
                brush.SetColor(&self.accent);
                if n == 1 {
                    t.FillEllipse(&circle(center, OUTER_RADIUS - HIGHLIGHT_GAP), &brush);
                } else {
                    let wedge = self.wedge(center, index, n)?;
                    t.FillGeometry(&wedge, &brush, None);
                }
            }

            // Separatori tra uno spicchio e l'altro.
            if n > 1 {
                brush.SetColor(&white(0.10));
                let half_slice = PI / n as f32;
                for i in 0..n {
                    let edge = slice_angle(i, n) - half_slice;
                    let from = polar(center, edge, INNER_RADIUS + 8.0);
                    let to = polar(center, edge, OUTER_RADIUS - 8.0);
                    t.DrawLine(from, to, &brush, 1.0, None);
                }
            }

            // Icona ed etichetta di ogni spicchio (senza icona, l'etichetta va al centro).
            let middle = (INNER_RADIUS + OUTER_RADIUS) / 2.0;
            for (i, slice) in self.slices.iter().enumerate() {
                let p = polar(center, slice_angle(i, n), middle);
                let label_y = if slice.icon.is_empty() { p.Y } else { p.Y + 15.0 };
                // I colori vanno in variabili locali: SetColor riceve un puntatore grezzo, e un
                // `&white(..)` dentro un if/else verrebbe distrutto a fine blocco (puntatore pendente).
                let (icon_color, label_color) = if self.hover == Some(i) {
                    (on_accent, on_accent)
                } else {
                    (white(0.95), white(0.70))
                };
                brush.SetColor(&icon_color);
                draw_text(t, &slice.icon, &self.icon_format, centered_rect(p.X, p.Y - 9.0, 40.0, 28.0), &brush);
                brush.SetColor(&label_color);
                draw_text(t, &slice.label, &self.label_format, centered_rect(p.X, label_y, 80.0, 18.0), &brush);
                if slice.submenu {
                    self.draw_chevron(center, slice_angle(i, n), &brush)?;
                }
            }

            // Mozzo centrale: il nome dello spicchio puntato, altrimenti logo, profilo o "indietro".
            brush.SetColor(&color(0.09, 0.09, 0.10, 1.0));
            t.FillEllipse(&circle(center, INNER_RADIUS), &brush);
            brush.SetColor(&white(0.10));
            t.DrawEllipse(&circle(center, INNER_RADIUS - 0.5), &brush, 1.0, None);
            let hub_text = centered_rect(center.X, center.Y, 2.0 * INNER_RADIUS - 8.0, 20.0);
            match (self.hover, &self.hub) {
                (Some(index), _) => {
                    brush.SetColor(&white(0.95));
                    draw_text(t, &self.slices[index].label, &self.hub_format, hub_text, &brush);
                }
                (None, Hub::Logo) => self.draw_star(center, 36.0)?,
                (None, Hub::Title(name)) => {
                    brush.SetColor(&white(0.95));
                    draw_text(t, name, &self.hub_format, hub_text, &brush);
                }
                (None, Hub::Back(name)) => {
                    brush.SetColor(&white(0.60));
                    let arrow = centered_rect(center.X, center.Y - 10.0, 40.0, 24.0);
                    draw_text(t, BACK_GLYPH, &self.icon_format, arrow, &brush);
                    brush.SetColor(&white(0.95));
                    let below = centered_rect(center.X, center.Y + 13.0, 2.0 * INNER_RADIUS - 12.0, 18.0);
                    draw_text(t, name, &self.hub_format, below, &brush);
                }
            }

            t.EndDraw(None, None)
        }
    }

    /// La stella del logo (assets/logo.svg), alta `height` DIP, con lo stesso gradiente.
    fn draw_star(&self, center: Vector2, height: f32) -> Result<()> {
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

            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: rgb(0x83EAF0) },
                D2D1_GRADIENT_STOP { position: 1.0, color: rgb(0x49C8CE) },
            ];
            let stops = self.target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            let gradient = self.target.CreateRadialGradientBrush(
                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center,
                    gradientOriginOffset: Vector2::zero(),
                    radiusX: STAR_HALF_WIDTH * scale,
                    radiusY: STAR_HALF_HEIGHT * scale,
                },
                None,
                &stops,
            )?;
            self.target.FillGeometry(&geometry, &gradient, None);
        }
        Ok(())
    }

    /// Freccetta ">" sul bordo esterno, rivolta verso l'esterno lungo la direzione `angle`.
    fn draw_chevron(&self, center: Vector2, angle: f32, brush: &ID2D1SolidColorBrush) -> Result<()> {
        let (sin, cos) = angle.sin_cos();
        let tip = polar(center, angle, OUTER_RADIUS - 8.0);
        let base = polar(center, angle, OUTER_RADIUS - 12.0);
        // Le due ali si allontanano dalla base in perpendicolare alla direzione (-sin, cos).
        let wing = |side: f32| Vector2::new(base.X - sin * 4.5 * side, base.Y + cos * 4.5 * side);
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(wing(1.0), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddLine(tip);
            sink.AddLine(wing(-1.0));
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            self.target.DrawGeometry(&geometry, brush, 1.5, None);
        }
        Ok(())
    }

    /// Forma dello spicchio evidenziato: una corona circolare tra due angoli,
    /// rientrata di HIGHLIGHT_GAP dai bordi.
    fn wedge(&self, center: Vector2, index: usize, n: usize) -> Result<ID2D1PathGeometry> {
        let mid = slice_angle(index, n);
        let half = PI / n as f32;
        let inner = INNER_RADIUS + HIGHLIGHT_GAP;
        let outer = OUTER_RADIUS - HIGHLIGHT_GAP;
        // Lo stesso spazio in DIP corrisponde a un angolo più piccolo sul bordo esterno.
        let (inner_start, inner_end) = (mid - half + HIGHLIGHT_GAP / inner, mid + half - HIGHLIGHT_GAP / inner);
        let (outer_start, outer_end) = (mid - half + HIGHLIGHT_GAP / outer, mid + half - HIGHLIGHT_GAP / outer);

        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(polar(center, inner_start, inner), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(polar(center, outer_start, outer));
            sink.AddArc(&arc(polar(center, outer_end, outer), outer, D2D1_SWEEP_DIRECTION_CLOCKWISE));
            sink.AddLine(polar(center, inner_end, inner));
            sink.AddArc(&arc(polar(center, inner_start, inner), inner, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geometry)
        }
    }
}

/// Bitmap in memoria (32 bit BGRA) su cui disegna Direct2D.
/// Implementa `Drop`: quando esce di scope, le risorse GDI vengono liberate in automatico.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
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
            Ok(Self { dc, bitmap, previous })
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

/// Colore di accento di Windows (Impostazioni > Personalizzazione > Colori).
fn accent_color() -> D2D1_COLOR_F {
    let mut argb = 0u32;
    let mut opaque = BOOL::default();
    if unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) }.is_err() {
        return rgb(0x0078D4); // blu di Windows come ripiego
    }
    rgb(argb & 0xFFFFFF) // il colore arriva come 0xAARRGGBB: scartiamo l'alpha
}

fn is_light(c: &D2D1_COLOR_F) -> bool {
    0.299 * c.r + 0.587 * c.g + 0.114 * c.b > 0.6
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

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// Colore opaco da esadecimale, come nei CSS: rgb(0x49C8CE).
fn rgb(hex: u32) -> D2D1_COLOR_F {
    let channel = |shift: u32| ((hex >> shift) & 0xFF) as f32 / 255.0;
    color(channel(16), channel(8), channel(0), 1.0)
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
}
