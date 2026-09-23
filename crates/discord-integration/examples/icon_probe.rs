//! Diagnostic: render the glyphs at every size they are actually used at.
//!
//! These icons live at 10–20 px, where geometry that looks fine on paper turns
//! to mush. This sheet makes that visible in one glance.

use windows::Win32::Foundation::RECT;

use taskbar_widget::assets::icons::{self, IconFonts};
use discord_integration::icons as glyphs;
use taskbar_widget::ui::render::{Canvas, Color};

const SIZES: &[i32] = &[9, 11, 14, 16, 20, 32];
const BACKGROUND: Color = Color::rgb(0x2B, 0x2D, 0x31);
const FOREGROUND: Color = Color::rgb(0xDB, 0xDE, 0xE1);
const DANGER: Color = Color::rgb(0xDA, 0x37, 0x3C);

fn main() {
    let pad = 8;
    let cell = 40;
    let rows = 6; // mic, mic-slash, ear, ear-slash, badge-mute, badge-deaf
    let width = pad + SIZES.len() as i32 * cell;
    let height = pad + rows * cell;

    let mut canvas = Canvas::new(width, height).expect("canvas");
    canvas.clear();
    canvas.fill_rect(
        RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
        BACKGROUND,
    );

    let mut fonts = IconFonts::new();
    println!("icon font family: {}", fonts.family());

    for (column, &size) in SIZES.iter().enumerate() {
        let x = pad + column as i32 * cell;
        let centre = |row: i32| pad + row * cell + (cell - size) / 2 - pad / 2;

        fonts.draw(&mut canvas, glyphs::MICROPHONE, x, centre(0), size, FOREGROUND, BACKGROUND);
        fonts.draw(&mut canvas, glyphs::MICROPHONE_OFF, x, centre(1), size, DANGER, BACKGROUND);
        fonts.draw(&mut canvas, glyphs::HEADPHONES, x, centre(2), size, FOREGROUND, BACKGROUND);
        fonts.draw(&mut canvas, glyphs::HEADPHONES_OFF, x, centre(3), size, DANGER, BACKGROUND);

        // Badges, as drawn over an avatar corner.
        for (row, glyph) in [(4, glyphs::MICROPHONE), (5, glyphs::HEADPHONES)] {
            let radius = size as f32 / 2.0;
            let cx = x as f32 + radius;
            let cy = centre(row) as f32 + radius;
            icons::badge(&mut canvas, &mut fonts, cx, cy, radius, glyph, DANGER, BACKGROUND);
        }
    }

    write_bmp("icon_probe.bmp", &mut canvas);
    println!("sizes: {SIZES:?}");
    println!("rows:  mic, mic-slash, headphones, headphones-slash, badge-mute, badge-deafen");
    println!("wrote icon_probe.bmp ({width}x{height})");
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
