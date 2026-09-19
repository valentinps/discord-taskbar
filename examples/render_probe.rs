//! Diagnostic: render the widget's content to a raw buffer so the drawing code
//! can be inspected independently of whether the taskbar composites it.
//!
//! Writes `<width> <height>` then premultiplied BGRA rows to `render_probe.bin`.

use discord_taskbar::ui::render::{Canvas, Color, Font};
use windows::Win32::Foundation::RECT;

fn main() {
    let width = 200;
    let height = 32;

    let mut canvas = Canvas::new(width, height).expect("canvas");
    let font = Font::system_ui(12, false).expect("font");

    canvas.clear();
    canvas.fill_round_rect(
        RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
        6,
        Color::rgba(0x2B, 0x2D, 0x31, 0xD8),
    );
    canvas.draw_text(
        "Discord Taskbar",
        &font,
        12,
        (height - font.height) / 2,
        Color::rgb(0xDB, 0xDE, 0xE1),
    );
    // A green ring, so the anti-aliased circle path gets exercised too.
    canvas.stroke_circle(width as f32 - 20.0, height as f32 / 2.0, 10.0, 2.0, Color::rgb(0x23, 0xA5, 0x59));

    let pixels = canvas.snapshot();
    let mut out = Vec::new();
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    for pixel in &pixels {
        out.extend_from_slice(&pixel.to_le_bytes());
    }

    std::fs::write("render_probe.bin", &out).expect("write");

    let opaque = pixels.iter().filter(|p| (*p >> 24) != 0).count();
    println!(
        "wrote render_probe.bin: {width}x{height}, {opaque}/{} pixels with non-zero alpha",
        pixels.len()
    );
    println!("font height = {}, ascent = {}", font.height, font.ascent);
    println!("sample pixels:");
    for y in [2, height / 2] {
        let row: Vec<String> = (0..8)
            .map(|i| format!("{:08X}", pixels[(y * width + i * 20) as usize]))
            .collect();
        println!("  y={y:<3} {}", row.join(" "));
    }
}
