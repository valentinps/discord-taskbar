//! Diagnostic: recover the slash geometry baked into the `MicOff` glyph.
//!
//! The microphone-off icon comes from the font with Microsoft's own diagonal
//! already drawn; the headphones have no off variant, so that slash is drawn by
//! hand. Two different angles next to each other look like a mistake, so this
//! measures the font's slash and reports it as fractions of the em box, ready
//! to hardcode.
//!
//! Method: render `MicOff` and plain `Microphone` at a large size, subtract one
//! from the other, and fit a line through whatever is left.

use windows::Win32::Foundation::RECT;

use discord_taskbar::ui::render::{Canvas, Color, Font};

const SIZE: i32 = 128;
const MIC: char = '\u{E720}';
const MIC_OFF: char = '\u{F781}';

fn main() {
    let Some(font) = Font::new("Segoe Fluent Icons", SIZE, false) else {
        eprintln!("Segoe Fluent Icons not available");
        return;
    };

    let plain = coverage(&font, MIC);
    let slashed = coverage(&font, MIC_OFF);

    // Pixels present in the slashed glyph but not the plain one are the slash
    // (plus a little fringe where the cut-out bit into the microphone).
    let mut points = Vec::new();
    for y in 0..SIZE {
        for x in 0..SIZE {
            let index = (y * SIZE + x) as usize;
            if slashed[index] > 160 && plain[index] < 64 {
                points.push((x as f64, y as f64));
            }
        }
    }

    if points.len() < 32 {
        eprintln!("only {} differing pixels; cannot fit a line", points.len());
        return;
    }

    // Least-squares fit of y = m*x + c.
    let n = points.len() as f64;
    let sum_x: f64 = points.iter().map(|p| p.0).sum();
    let sum_y: f64 = points.iter().map(|p| p.1).sum();
    let sum_xy: f64 = points.iter().map(|p| p.0 * p.1).sum();
    let sum_xx: f64 = points.iter().map(|p| p.0 * p.0).sum();

    let slope = (n * sum_xy - sum_x * sum_y) / (n * sum_xx - sum_x * sum_x);
    let intercept = (sum_y - slope * sum_x) / n;

    let min_x = points.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let max_x = points.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let min_y = points.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let max_y = points.iter().map(|p| p.1).fold(f64::MIN, f64::max);

    // Thickness: area divided by length.
    let length = ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt();
    let thickness = n / length;

    let em = SIZE as f64;
    println!("slash pixels: {}", points.len());
    println!("fit: y = {slope:.4}x + {intercept:.2}");
    println!("angle from horizontal: {:.2} deg", slope.atan().to_degrees());
    println!();
    println!("as fractions of the em box ({SIZE} px):");
    println!("  start  ({:.4}, {:.4})", min_x / em, (slope * min_x + intercept) / em);
    println!("  end    ({:.4}, {:.4})", max_x / em, (slope * max_x + intercept) / em);
    println!("  x span {:.4} .. {:.4}", min_x / em, max_x / em);
    println!("  y span {:.4} .. {:.4}", min_y / em, max_y / em);
    println!("  thickness {:.4}  ({thickness:.1} px at {SIZE})", thickness / em);
}

/// Render one glyph and return its per-pixel coverage.
fn coverage(font: &Font, glyph: char) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE, SIZE).expect("canvas");
    canvas.clear();
    canvas.fill_rect(
        RECT { left: 0, top: 0, right: SIZE, bottom: SIZE },
        Color::rgb(0, 0, 0),
    );

    let text = glyph.to_string();
    let (w, h) = canvas.measure_text(&text, font);
    canvas.draw_text(&text, font, (SIZE - w) / 2, (SIZE - h) / 2, Color::WHITE);

    canvas
        .snapshot()
        .iter()
        .map(|p| ((p >> 16) & 0xFF) as u8)
        .collect()
}
