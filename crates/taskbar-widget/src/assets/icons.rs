//! Drawing glyphs from Windows' own icon fonts.
//!
//! Icons were hand-drawn vectors to begin with, and they looked it: at the
//! 9–20 px they are actually used, hand-rolled geometry turns to mush.
//! Segoe Fluent Icons ships with Windows 11 and carries properly hinted
//! versions of the glyphs an integration is likely to want, so the font does
//! the work.
//!
//! Windows 10 has the older Segoe MDL2 Assets, which is missing some of the
//! newer code points — the slashed microphone among them. A `Glyph` may name a
//! `fallback` for that case, which is drawn with a hand-made strike instead.
//!
//! Nothing here knows what any particular glyph *means*. Integrations own
//! their own code points; this module only resolves and draws them.

use std::collections::HashMap;

use crate::ui::render::{Canvas, Color, Font};

const FLUENT: &str = "Segoe Fluent Icons";
const MDL2: &str = "Segoe MDL2 Assets";

/// One icon, as a code point plus what to do when the font lacks it.
///
/// Deliberately a plain value rather than an enum of known icons: the widget
/// draws whatever an integration asks for, and a fixed enum would mean editing
/// the core every time an integration wanted a new picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    /// Preferred code point, in Segoe Fluent Icons.
    pub code: char,
    /// Drawn instead, with a diagonal strike added by hand, when only the
    /// older Segoe MDL2 Assets is available. `None` means `code` is present in
    /// both families.
    pub fallback: Option<char>,
    /// Strike the glyph whichever code point is used — for "off" states that
    /// neither font family has a variant of.
    pub strike: bool,
}

impl Glyph {
    /// A glyph both font families carry.
    pub const fn new(code: char) -> Self {
        Glyph {
            code,
            fallback: None,
            strike: false,
        }
    }

    /// A glyph only Segoe Fluent Icons carries; elsewhere `fallback` is drawn
    /// with a strike through it.
    pub const fn with_fallback(code: char, fallback: char) -> Self {
        Glyph {
            code,
            fallback: Some(fallback),
            strike: false,
        }
    }

    /// This glyph, struck through.
    pub const fn struck(self) -> Self {
        Glyph {
            strike: true,
            ..self
        }
    }
}

/// Icon fonts, cached per pixel size.
///
/// Sizes vary (widget icons, avatar badges, different DPI), and creating a GDI
/// font on every repaint would be wasteful, so they are made once and kept.
pub struct IconFonts {
    family: &'static str,
    /// True when the richer Segoe Fluent Icons set is present.
    fluent: bool,
    cache: HashMap<i32, Option<Font>>,
}

impl Default for IconFonts {
    fn default() -> Self {
        Self::new()
    }
}

impl IconFonts {
    pub fn new() -> Self {
        let fluent = Font::family_exists(FLUENT);
        IconFonts {
            family: if fluent { FLUENT } else { MDL2 },
            fluent,
            cache: HashMap::new(),
        }
    }

    pub fn family(&self) -> &'static str {
        self.family
    }

    /// Whether the richer Segoe Fluent Icons set is available.
    pub fn has_fluent(&self) -> bool {
        self.fluent
    }

    fn font(&mut self, size: i32) -> Option<&Font> {
        self.cache
            .entry(size.max(6))
            .or_insert_with(|| Font::new(self.family, size.max(6), false))
            .as_ref()
    }

    /// Drop cached fonts, e.g. after a DPI change.
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Width the glyph will occupy at `size`.
    pub fn measure(&mut self, canvas: &Canvas, glyph: Glyph, size: i32) -> i32 {
        let (code, _) = self.resolve(glyph);
        match self.font(size) {
            Some(font) => canvas.measure_text(&code.to_string(), font).0,
            None => size,
        }
    }

    /// Which code point to draw, and whether a strike has to be added by hand.
    fn resolve(&self, glyph: Glyph) -> (char, bool) {
        match glyph.fallback {
            // The preferred code point is missing from this family, so draw
            // the older one and mark it struck by hand.
            Some(fallback) if !self.fluent => (fallback, true),
            _ => (glyph.code, glyph.strike),
        }
    }

    /// Draw `glyph` centred in a `size`-wide box at `(x, y)`.
    ///
    /// `backdrop` is only used to cut the gap around a hand-drawn strike.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        glyph: Glyph,
        x: i32,
        y: i32,
        size: i32,
        color: Color,
        backdrop: Color,
    ) {
        let (code, needs_strike) = self.resolve(glyph);

        let text = code.to_string();
        let (width, height) = match self.font(size) {
            Some(font) => canvas.measure_text(&text, font),
            None => return,
        };

        let draw_x = x + (size - width) / 2;
        let draw_y = y + (size - height) / 2;

        if let Some(font) = self.font(size) {
            canvas.draw_text(&text, font, draw_x, draw_y, color);
        }

        if needs_strike {
            draw_slash(canvas, x as f32, y as f32, size as f32, color, backdrop);
        }
    }
}

/// Diagonal strike for glyphs the font has no "off" variant of.
///
/// The geometry is copied from Segoe Fluent Icons' own `MicOff` glyph rather
/// than invented, because a hand-drawn slash sitting next to the font's one
/// looks like a bug the moment the two appear together. Measured off `F781` by
/// `examples/slash_probe.rs`: a 44.9-degree stroke running corner to corner
/// across the em box, about 5% of the em thick, descending left to right.
///
/// The first, wider stroke cuts a gap in the backdrop colour so the slash reads
/// as passing over the glyph instead of merging with it.
fn draw_slash(canvas: &mut Canvas, x: f32, y: f32, size: f32, color: Color, backdrop: Color) {
    let span = size * 0.99;
    let line = (size * 0.05).max(1.0);
    let gap = line + (size * 0.055).max(1.4);

    let (x0, y0) = (x, y);
    let (x1, y1) = (x + span, y + span);

    canvas.stroke_line(x0, y0, x1, y1, gap, backdrop);
    canvas.stroke_line(x0, y0, x1, y1, line, color);
}

/// A small marker drawn over the corner of an image — a mute badge on an
/// avatar, a playing marker on album art.
///
/// The glyph is drawn plain, without any strike: at nine pixels across a
/// slashed glyph and its unslashed twin are the same smear. The coloured disc
/// is what carries the meaning; the glyph only has to say *which* thing, and it
/// gets the whole badge to do that in.
#[allow(clippy::too_many_arguments)]
pub fn badge(
    canvas: &mut Canvas,
    fonts: &mut IconFonts,
    centre_x: f32,
    centre_y: f32,
    radius: f32,
    glyph: Glyph,
    fill: Color,
    ring: Color,
) {
    canvas.fill_circle(centre_x, centre_y, radius + 1.0, ring);
    canvas.fill_circle(centre_x, centre_y, radius, fill);

    let glyph_size = (radius * 1.6).round().max(6.0) as i32;
    let gx = (centre_x - glyph_size as f32 / 2.0).round() as i32;
    let gy = (centre_y - glyph_size as f32 / 2.0).round() as i32;

    // Never struck, whatever the caller's glyph says.
    let plain = Glyph {
        strike: false,
        ..glyph
    };
    fonts.draw(canvas, plain, gx, gy, glyph_size, Color::WHITE, fill);
}
