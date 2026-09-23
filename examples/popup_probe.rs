//! Diagnostic: render the volume popup at several levels.
//!
//! The popup only appears while the wheel is turning over a participant, which
//! makes it awkward to inspect live. This draws it at a spread of volumes over
//! a chequerboard, so the rounded corners and the alpha can be checked — it is
//! a top-level layered window, so unlike the widget it really is transparent.

use windows::Win32::Foundation::HWND;

use discord_taskbar::assets::images::ImageCache;
use discord_taskbar::model::Participant;
use discord_taskbar::integration::discord::view;
use discord_taskbar::ui::popup::{self, Meter};
use discord_taskbar::ui::render::{Canvas, Font};
use discord_taskbar::ui::theme::{Appearance, Theme};
use discord_taskbar::ui::Notifier;

const LEVELS: &[f32] = &[0.0, 35.0, 100.0, 150.0, 200.0];

fn main() {
    // With `--live`, put a real popup on screen where the app would and leave
    // it there, so the window itself can be inspected rather than just the
    // pixels it would have drawn. Rendering correctly and *presenting*
    // correctly are two different things.
    if std::env::args().any(|a| a == "--live") {
        live();
        return;
    }

    let theme = Theme::from(&Appearance::default());
    let font = Font::system_ui(theme.font_size, false).expect("font");
    let mut images = ImageCache::new(Notifier::new(HWND(std::ptr::null_mut()), 0));

    let participant =
        Participant::new("100000000008388608".to_string(), "Cleo".to_string());

    // Warm the avatar cache before drawing anything.
    let mut scratch = Canvas::new(1, 1).expect("scratch");
    {
        let shown = 100.0;
        let mut view = Meter {
            title: &participant.display_name,
            value_text: view::percent(shown),
            fraction: shown / theme.volume_ceiling(shown),
            fill: if shown > 100.5 { theme.danger } else { theme.speaking },
            image: Some(participant.avatar_ref(theme.avatar_size as u32)),
            image_size: theme.avatar_size,
            theme: &theme,
            font: &font,
            images: &mut images,
            dpi: 96,
        };
        popup::draw_meter(&mut scratch, &mut view);
    }
    for _ in 0..40 {
        if images.collect() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    images.collect();

    // Draw each level once to learn the size, then compose the sheet.
    let mut tiles = Vec::new();
    for level in LEVELS {
        let mut canvas = Canvas::new(1, 1).expect("tile");
        let shown = *level;
        let mut view = Meter {
            title: "Cleo",
            value_text: view::percent(shown),
            fraction: shown / theme.volume_ceiling(shown),
            fill: if shown > 100.5 { theme.danger } else { theme.speaking },
            image: Some(participant.avatar_ref(theme.avatar_size as u32)),
            image_size: theme.avatar_size,
            theme: &theme,
            font: &font,
            images: &mut images,
            dpi: 96,
        };
        let size = popup::draw_meter(&mut canvas, &mut view).expect("draw");
        tiles.push((canvas, size));
    }

    let (tile_w, tile_h) = tiles[0].1;
    let gap = 10;
    let width = tile_w + gap * 2;
    let height = (tile_h + gap) * tiles.len() as i32 + gap;

    let mut sheet = Canvas::new(width, height).expect("sheet");
    sheet.clear();

    // Chequerboard, so transparency is visible rather than assumed.
    for y in 0..height {
        for x in 0..width {
            let dark = ((x / 8) + (y / 8)) % 2 == 0;
            let shade = if dark { 0x33 } else { 0x55 };
            sheet.blend(x, y, 0xFF00_0000 | (shade << 16) | (shade << 8) | shade);
        }
    }

    for (index, (tile, size)) in tiles.iter_mut().enumerate() {
        let (w, h) = *size;
        let top = gap + index as i32 * (tile_h + gap);
        let pixels = tile.snapshot();
        for y in 0..h {
            for x in 0..w {
                sheet.blend(gap + x, top + y, pixels[(y * w + x) as usize]);
            }
        }
        println!("{:>5}%  {w}x{h}", LEVELS[index]);
    }
    write_bmp("popup_probe.bmp", &mut sheet);
    println!("\nwrote popup_probe.bmp ({width}x{height})");
}

fn write_bmp(path: &str, canvas: &mut Canvas) {
    let width = canvas.width();
    let height = canvas.height();
    let pixels = canvas.snapshot();

    let pixel_bytes = (width * height * 4) as u32;
    let offset = 54u32;

    let mut out = Vec::with_capacity((offset + pixel_bytes) as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(offset + pixel_bytes).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&(-height).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&pixel_bytes.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for pixel in &pixels {
        out.extend_from_slice(&pixel.to_le_bytes());
    }

    std::fs::write(path, out).expect("write bmp");
}

/// Show an actual popup window for a few seconds, exactly as the app does.
fn live() {
    use discord_taskbar::ui::taskbar;

    let theme = Theme::from(&Appearance::default());
    let font = Font::system_ui(theme.font_size, false).expect("font");
    let mut images = ImageCache::new(Notifier::new(HWND(std::ptr::null_mut()), 0));

    let participant =
        Participant::new("100000000008388608".to_string(), "Cleo".to_string());

    let Some(info) = taskbar::find() else {
        eprintln!("no taskbar");
        return;
    };

    let mut canvas = Canvas::new(1, 1).expect("canvas");
    let Some(hwnd) = popup::create() else {
        eprintln!("popup::create failed");
        return;
    };
    println!("popup hwnd = {hwnd:?}");

    for step in 0..24 {
        let volume = (step as f32 * 10.0) % 210.0;
        let shown = volume;
        let mut view = Meter {
            title: "Cleo",
            value_text: view::percent(shown),
            fraction: shown / theme.volume_ceiling(shown),
            fill: if shown > 100.5 { theme.danger } else { theme.speaking },
            image: Some(participant.avatar_ref(theme.avatar_size as u32)),
            image_size: theme.avatar_size,
            theme: &theme,
            font: &font,
            images: &mut images,
            dpi: info.dpi,
        };

        let Some((w, h)) = popup::draw_meter(&mut canvas, &mut view) else {
            eprintln!("draw_meter failed");
            return;
        };

        let margin = 6;
        let x = (info.rect.left + info.rect.right) / 2 - w / 2;
        let y = info.rect.top - h - margin;

        let shown = popup::present(hwnd, &canvas, x, y);
        if step == 0 {
            println!("present -> {shown} at ({x}, {y}) size {w}x{h}");
            if !shown {
                eprintln!(
                    "UpdateLayeredWindow failed: {}",
                    windows::core::Error::from_thread()
                );
            }
        }

        images.collect();
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    popup::destroy(hwnd);
    println!("done");
}
