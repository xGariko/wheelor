//! Rasterizza assets/logo.svg nelle dimensioni che servono:
//! - assets/wheelor.ico: icona dell'exe (tray, titolo, taskbar, Esplora file);
//! - assets/logo-256.png: per la documentazione;
//! - assets/installer/wizard-*.bmp: immagine in alto a destra dell'installer, una per scala dello schermo;
//! - assets/msix/*.png: le icone del pacchetto per il Microsoft Store (installer/msix).

use std::fs::File;
use std::path::Path;

use resvg::{tiny_skia, usvg};

/// Dimensioni richieste da Windows a 100–300% di scala.
const ICON_SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

/// Dimensioni consigliate da Inno Setup per WizardSmallImageFile, da 100% a 250%.
const WIZARD_SIZES: [u32; 7] = [55, 64, 83, 92, 110, 119, 138];

/// Dimensioni consigliate da Inno Setup per WizardImageFile (pannello a sinistra della pagina
/// finale), da 100% a 250%.
const PANEL_SIZES: [(u32, u32); 7] =
    [(164, 314), (192, 386), (246, 459), (273, 556), (328, 604), (355, 700), (410, 797)];

/// Icone del pacchetto MSIX: nome, misura a 100% e quanto spazio occupa il logo (il resto è
/// margine trasparente, come nelle icone di Windows). Ognuna in più scale dello schermo.
const MSIX_LOGOS: [(&str, u32, f32); 3] =
    [("Square44x44Logo", 44, 0.92), ("Square150x150Logo", 150, 0.6), ("StoreLogo", 50, 0.92)];
const MSIX_SCALES: [u32; 5] = [100, 125, 150, 200, 400];
/// Misure fisse dell'icona piccola (barra delle applicazioni, elenco delle app, Esplora file).
const MSIX_TARGET_SIZES: [u32; 9] = [16, 20, 24, 30, 32, 36, 40, 48, 256];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let svg = std::fs::read(assets.join("logo.svg"))?;
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())?;

    let mut icon = ico::IconDir::new(ico::ResourceType::Icon);
    for size in ICON_SIZES {
        let pixmap = render(&tree, size);
        // tiny-skia lavora con alpha premoltiplicata, il formato ICO vuole RGBA "dritto".
        let rgba = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        icon.add_entry(ico::IconDirEntry::encode(&ico::IconImage::from_rgba_data(size, size, rgba))?);
        if size == 256 {
            pixmap.save_png(assets.join("logo-256.png"))?;
        }
    }
    icon.write(File::create(assets.join("wheelor.ico"))?)?;

    let installer = assets.join("installer");
    std::fs::create_dir_all(&installer)?;
    for size in WIZARD_SIZES {
        std::fs::write(installer.join(format!("wizard-{size}.bmp")), bmp(&render(&tree, size)))?;
    }
    for (width, height) in PANEL_SIZES {
        std::fs::write(installer.join(format!("panel-{width}.bmp")), bmp(&panel(&tree, width, height)))?;
    }

    let msix = assets.join("msix");
    std::fs::create_dir_all(&msix)?;
    let mut count = 0;
    for (name, size, fill) in MSIX_LOGOS {
        for scale in MSIX_SCALES {
            let pixels = (size * scale).div_ceil(100);
            padded(&tree, pixels, fill).save_png(msix.join(format!("{name}.scale-{scale}.png")))?;
            count += 1;
        }
    }
    for size in MSIX_TARGET_SIZES {
        let pixmap = padded(&tree, size, 0.92);
        // "unplated": senza il riquadro colorato dietro, per la barra delle applicazioni.
        for suffix in ["", "_altform-unplated"] {
            pixmap.save_png(msix.join(format!("Square44x44Logo.targetsize-{size}{suffix}.png")))?;
            count += 1;
        }
    }

    println!(
        "Creati assets/wheelor.ico ({} dimensioni), assets/logo-256.png, {} immagini in assets/installer e {count} in assets/msix",
        ICON_SIZES.len(),
        WIZARD_SIZES.len() + PANEL_SIZES.len()
    );
    Ok(())
}

/// Pannello verticale: sfumatura scura sui toni del logo, con il logo poco sopra il centro.
fn panel(tree: &usvg::Tree, width: u32, height: u32) -> tiny_skia::Pixmap {
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("dimensione valida");
    let gradient = tiny_skia::LinearGradient::new(
        tiny_skia::Point::from_xy(0.0, 0.0),
        tiny_skia::Point::from_xy(0.0, height as f32),
        vec![
            tiny_skia::GradientStop::new(0.0, tiny_skia::Color::from_rgba8(0x1E, 0x3A, 0x3E, 0xFF)),
            tiny_skia::GradientStop::new(1.0, tiny_skia::Color::from_rgba8(0x0E, 0x17, 0x19, 0xFF)),
        ],
        tiny_skia::SpreadMode::Pad,
        tiny_skia::Transform::identity(),
    )
    .expect("sfumatura valida");
    let paint = tiny_skia::Paint { shader: gradient, ..Default::default() };
    let rect = tiny_skia::Rect::from_xywh(0.0, 0.0, width as f32, height as f32).expect("rettangolo valido");
    pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);

    let logo = width as f32 * 0.62;
    let scale = logo / tree.size().width().max(tree.size().height());
    let x = (width as f32 - logo) / 2.0;
    let y = height as f32 * 0.42 - logo / 2.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(x, y);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    pixmap
}

/// Il logo al centro di un quadrato di `size` pixel, grande `fill` volte il lato.
fn padded(tree: &usvg::Tree, size: u32, fill: f32) -> tiny_skia::Pixmap {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("dimensione valida");
    let logo = size as f32 * fill;
    let scale = logo / tree.size().width().max(tree.size().height());
    let offset = (size as f32 - logo) / 2.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(offset, offset);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    pixmap
}

fn render(tree: &usvg::Tree, size: u32) -> tiny_skia::Pixmap {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("dimensione valida");
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap
}

/// Bitmap Windows a 32 bit con trasparenza (alpha premoltiplicata, come quella di tiny-skia:
/// in Inno Setup si dichiara con WizardImageAlphaFormat=premultiplied).
fn bmp(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    let (width, height) = (pixmap.width(), pixmap.height());
    let pixels = width * height * 4;
    let mut out = Vec::with_capacity(54 + pixels as usize);
    // BITMAPFILEHEADER: firma, dimensione del file, due campi riservati, inizio dei pixel.
    out.extend(b"BM");
    out.extend((54 + pixels).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(54u32.to_le_bytes());
    // BITMAPINFOHEADER: altezza positiva = righe dal basso verso l'alto; 32 bit, non compressa.
    out.extend(40u32.to_le_bytes());
    out.extend((width as i32).to_le_bytes());
    out.extend((height as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // BI_RGB
    out.extend(pixels.to_le_bytes());
    out.extend(3780i32.to_le_bytes()); // 96 DPI in pixel per metro
    out.extend(3780i32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    for y in (0..height).rev() {
        for x in 0..width {
            let p = pixmap.pixel(x, y).expect("dentro l'immagine");
            out.extend([p.blue(), p.green(), p.red(), p.alpha()]);
        }
    }
    out
}
