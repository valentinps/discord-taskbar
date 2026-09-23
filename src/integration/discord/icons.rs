//! The glyphs the Discord integration draws.
//!
//! Code points from Segoe Fluent Icons, with Segoe MDL2 Assets fallbacks for
//! Windows 10 — see `crate::assets::icons` for how the two are resolved.

use crate::assets::icons::Glyph;

pub const MICROPHONE: Glyph = Glyph::new('\u{E720}');

/// Segoe MDL2 Assets has no slashed microphone, so it falls back to the plain
/// one with a strike drawn by hand.
pub const MICROPHONE_OFF: Glyph = Glyph::with_fallback('\u{F781}', '\u{E720}');

pub const HEADPHONES: Glyph = Glyph::new('\u{E7F6}');

/// Neither font family has a headphones-off glyph, so this is always struck.
pub const HEADPHONES_OFF: Glyph = HEADPHONES.struck();

/// A phone being hung up — Discord's own metaphor for leaving a call.
pub const HANG_UP: Glyph = Glyph::new('\u{E778}');

pub const VOLUME: Glyph = Glyph::new('\u{E767}');
pub const VOLUME_MUTED: Glyph = Glyph::new('\u{E74F}');
