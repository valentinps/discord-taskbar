//! Microphone and headphone glyphs, taken from Windows' own icon font.
//!
//! These were hand-drawn vectors to begin with, and they looked it: at the
//! 9–20 px they are actually used, hand-rolled geometry turns to mush.
//! Segoe Fluent Icons ships with Windows 11 and carries properly hinted
//! versions of exactly the glyphs needed, so the font does the work.
//!
//! Windows 10 has the older Segoe MDL2 Assets, which has the microphone and
//! headphones but not the slashed microphone; `IconFonts` falls back to that
//! and draws the slash by hand when it has to.

use std::collections::HashMap;

use crate::ui::render::{Canvas, Color, Font};

/// Segoe Fluent Icons / Segoe MDL2 Assets code points.
const GLYPH_MICROPHONE: char = '\u{E720}';
const GLYPH_MICROPHONE_OFF: char = '\u{F781}';
const GLYPH_HEADPHONES: char = '\u{E7F6}';
/// A phone being hung up - Discord's own metaphor for leaving a call.
const GLYPH_HANG_UP: char = '\u{E778}';
const GLYPH_VOLUME: char = '\u{E767}';
const GLYPH_VOLUME_MUTED: char = '\u{E74F}';

const FLUENT: &str = "Segoe Fluent Icons";
const MDL2: &str = "Segoe MDL2 Assets";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Microphone,
    MicrophoneOff,
    Headphones,
    HeadphonesOff,
    HangUp,
    Volume,
    VolumeMuted,
}

/// Icon fonts, cached per pixel size.
///
/// Sizes vary (self icons, avatar badges, different DPI), and creating a GDI
/// font on every repaint would be wasteful, so they are made once and kept.
pub struct IconFonts {
    family: &'static str,
    /// True when the family has the slashed-microphone glyph.
    has_slashed: bool,
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
            has_slashed: fluent,
            cache: HashMap::new(),
        }
    }

    pub fn family(&self) -> &'static str {
        self.family
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

    /// Width the icon will occupy at `size`.
    pub fn measure(&mut self, canvas: &Canvas, icon: Icon, size: i32) -> i32 {
        let Some((glyph, _)) = self.resolve(icon) else {
            return size;
        };
        match self.font(size) {
            Some(font) => canvas.measure_text(&glyph.to_string(), font).0,
            None => size,
        }
    }

    /// Which code point to draw, and whether a slash has to be added by hand.
    fn resolve(&self, icon: Icon) -> Option<(char, bool)> {
        Some(match icon {
            Icon::Microphone => (GLYPH_MICROPHONE, false),
            Icon::MicrophoneOff => {
                if self.has_slashed {
                    (GLYPH_MICROPHONE_OFF, false)
                } else {
                    (GLYPH_MICROPHONE, true)
                }
            }
            Icon::Headphones => (GLYPH_HEADPHONES, false),
            // No headphones-off glyph exists in either family.
            Icon::HeadphonesOff => (GLYPH_HEADPHONES, true),
            Icon::HangUp => (GLYPH_HANG_UP, false),
            Icon::Volume => (GLYPH_VOLUME, false),
            Icon::VolumeMuted => (GLYPH_VOLUME_MUTED, false),
        })
    }

    /// Draw `icon` centred in a `size`-wide box at `(x, y)`.
    ///
    /// `backdrop` is only used to cut the gap around a hand-drawn slash.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        icon: Icon,
        x: i32,
        y: i32,
        size: i32,
        color: Color,
        backdrop: Color,
    ) {
        let Some((glyph, needs_slash)) = self.resolve(icon) else {
            return;
        };

        let text = glyph.to_string();
        let (width, height) = match self.font(size) {
            Some(font) => canvas.measure_text(&text, font),
            None => return,
        };

        let draw_x = x + (size - width) / 2;
        let draw_y = y + (size - height) / 2;

        if let Some(font) = self.font(size) {
            canvas.draw_text(&text, font, draw_x, draw_y, color);
        }

        if needs_slash {
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

/// Mute / deafen marker drawn over the corner of an avatar.
///
/// No slash inside the badge: at nine pixels across, a mic-slash and a
/// headphone-slash are the same smear. The red disc already means "off", so the
/// glyph only has to answer *which* — microphone or headphones — and it gets
/// the whole badge to do that in.
#[allow(clippy::too_many_arguments)]
pub fn badge(
    canvas: &mut Canvas,
    fonts: &mut IconFonts,
    centre_x: f32,
    centre_y: f32,
    radius: f32,
    deafened: bool,
    fill: Color,
    ring: Color,
) {
    canvas.fill_circle(centre_x, centre_y, radius + 1.0, ring);
    canvas.fill_circle(centre_x, centre_y, radius, fill);

    let glyph_size = (radius * 1.6).round().max(6.0) as i32;
    let gx = (centre_x - glyph_size as f32 / 2.0).round() as i32;
    let gy = (centre_y - glyph_size as f32 / 2.0).round() as i32;

    let icon = if deafened {
        Icon::Headphones
    } else {
        Icon::Microphone
    };
    fonts.draw(canvas, icon, gx, gy, glyph_size, Color::WHITE, fill);
}
