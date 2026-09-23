//! Diagnostic: find usable microphone/headphone glyphs in the icon fonts that
//! ship with Windows.
//!
//! Segoe Fluent Icons (Windows 11) and Segoe MDL2 Assets (Windows 10) both
//! carry a full icon set. Using them beats hand-drawn vectors: they are
//! professionally hinted, pixel-aligned at small sizes, and cost nothing to
//! ship. This renders candidate code points with labels so the right ones can
//! be picked by eye.

use windows::Win32::Foundation::RECT;

use taskbar_widget::ui::render::{Canvas, Color, Font};

const BACKGROUND: Color = Color::rgb(0x2B, 0x2D, 0x31);
const FOREGROUND: Color = Color::rgb(0xDB, 0xDE, 0xE1);
const LABEL: Color = Color::rgb(0x80, 0x84, 0x8B);

/// Ranges known to contain audio-related icons.
const RANGES: &[(u32, u32)] = &[(0xE700, 0xE7FF), (0xEC00, 0xEC7F), (0xF780, 0xF7AF)];

fn main() {
    // With code points as arguments, render just those, large enough to judge.
    // Without, sweep the ranges to go looking for candidates.
    let picked: Vec<u32> = std::env::args()
        .skip(1)
        .filter_map(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).ok())
        .collect();

    let font_name = "Segoe Fluent Icons";
    let (glyph_size, cell_w, cell_h, columns) = if picked.is_empty() {
        (20, 46, 40, 16)
    } else {
        (40, 78, 74, 8)
    };

    let codes: Vec<u32> = if picked.is_empty() {
        RANGES.iter().flat_map(|(a, b)| *a..=*b).collect()
    } else {
        picked
    };

    let total = codes.len();
    let rows = total.div_ceil(columns) as i32;

    let width = columns as i32 * cell_w;
    let height = rows * cell_h;

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

    let glyph_font = Font::new(font_name, glyph_size, false).expect("icon font");
    let label_font = Font::system_ui(10, false).expect("label font");

    for (index, code) in codes.iter().enumerate() {
        let column = (index % columns) as i32;
        let row = (index / columns) as i32;
        let x = column * cell_w;
        let y = row * cell_h;

        if let Some(ch) = char::from_u32(*code) {
            let text = ch.to_string();
            let (w, _) = canvas.measure_text(&text, &glyph_font);
            canvas.draw_text(&text, &glyph_font, x + (cell_w - w) / 2, y + 2, FOREGROUND);
        }

        let label = format!("{code:04X}");
        let (lw, _) = canvas.measure_text(&label, &label_font);
        canvas.draw_text(
            &label,
            &label_font,
            x + (cell_w - lw) / 2,
            y + cell_h - 13,
            LABEL,
        );
    }

    write_bmp("glyph_probe.bmp", &mut canvas);
    println!("font: {font_name}");
    println!("wrote glyph_probe.bmp ({width}x{height}, {total} glyphs)");
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
