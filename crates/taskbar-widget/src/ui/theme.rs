//! The widget's colours and metrics, all configurable.
//!
//! Only what the widget itself needs to draw anything at all. Sizes that
//! belong to a particular kind of content — how big an avatar is, how many of
//! them fit — are the integration's, and live in its own section of the
//! config.
//!
//! Metrics are expressed at 96 DPI and scaled at use.

use serde::{Deserialize, Serialize};

use super::render::Color;

fn parse(text: &str, fallback: Color) -> Color {
    Color::from_hex(text).unwrap_or(fallback)
}

/// Serialisable appearance settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Widget background, `#RRGGBB` or `#RRGGBBAA`.
    pub background: String,
    pub text: String,
    pub text_dim: String,
    /// The one colour that means "live" — a ring around whoever is talking, a
    /// slider's fill.
    ///
    /// Called `speaking` before the widget had anything but voice to show; the
    /// old name is still accepted so existing configs keep working.
    #[serde(alias = "speaking")]
    pub accent: String,
    /// Anything wrong, off or muted.
    pub danger: String,
    /// Stands in for a picture that has not downloaded yet.
    pub placeholder: String,
    /// A thin rule between groups.
    pub divider: String,

    pub corner_radius: i32,
    pub padding: i32,
    /// Gap between blocks.
    pub spacing: i32,
    pub font_size: i32,
    /// Box size for a glyph.
    pub icon_size: i32,
    /// Widget height at 96 DPI. `0` means fill the taskbar, which combined
    /// with a transparent `background` makes the widget look like part of the
    /// bar rather than a panel sitting on it.
    pub height: i32,
    /// Longest a text block may get before it is ellipsised.
    pub max_label_width: i32,

    /// Which displays to show the widget on.
    ///
    /// `["primary"]`, `["all"]`, or a list of monitor indices (`["0", "1"]`)
    /// or device names (`["\\.\DISPLAY1"]`). The tray menu edits this, which
    /// saves having to look the names up.
    pub monitors: Vec<String>,
    /// Nudge the widget horizontally, in device pixels.
    pub x_offset: i32,
    pub y_offset: i32,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            background: "#2B2D31D8".to_string(),
            text: "#DBDEE1".to_string(),
            text_dim: "#949BA4".to_string(),
            // Discord's speaking green, which is as good a default as any.
            accent: "#23A559".to_string(),
            danger: "#DA373C".to_string(),
            placeholder: "#4E5058".to_string(),
            divider: "#4E5058".to_string(),

            corner_radius: 6,
            padding: 10,
            spacing: 8,
            font_size: 12,
            icon_size: 16,
            height: 32,
            max_label_width: 220,

            monitors: vec!["primary".to_string()],
            x_offset: 0,
            y_offset: 0,
        }
    }
}

/// Parsed, ready-to-draw form of `Appearance`.
#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub text: Color,
    pub text_dim: Color,
    pub accent: Color,
    pub danger: Color,
    pub placeholder: Color,
    pub divider: Color,

    pub corner_radius: i32,
    pub padding: i32,
    pub spacing: i32,
    pub font_size: i32,
    pub icon_size: i32,
    pub height: i32,
    pub max_label_width: i32,

    pub monitors: Vec<String>,
    pub x_offset: i32,
    pub y_offset: i32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::from(&Appearance::default())
    }
}

impl From<&Appearance> for Theme {
    fn from(a: &Appearance) -> Self {
        let defaults = Appearance::default();
        Theme {
            background: parse(
                &a.background,
                parse(&defaults.background, Color::rgb(43, 45, 49)),
            ),
            text: parse(&a.text, Color::rgb(0xDB, 0xDE, 0xE1)),
            text_dim: parse(&a.text_dim, Color::rgb(0x94, 0x9B, 0xA4)),
            accent: parse(&a.accent, Color::rgb(0x23, 0xA5, 0x59)),
            danger: parse(&a.danger, Color::rgb(0xDA, 0x37, 0x3C)),
            placeholder: parse(&a.placeholder, Color::rgb(0x4E, 0x50, 0x58)),
            divider: parse(&a.divider, Color::rgb(0x4E, 0x50, 0x58)),

            corner_radius: a.corner_radius.max(0),
            padding: a.padding.max(0),
            spacing: a.spacing.max(0),
            font_size: a.font_size.clamp(6, 40),
            icon_size: a.icon_size.clamp(6, 48),
            height: a.height.clamp(0, 80),
            max_label_width: a.max_label_width.clamp(40, 2000),

            monitors: a.monitors.clone(),
            x_offset: a.x_offset,
            y_offset: a.y_offset,
        }
    }
}

impl Theme {
    /// Whether the widget should appear on a given display.
    ///
    /// Selectors are matched case-insensitively and may be `all`, `primary`, a
    /// monitor index, or a device name.
    pub fn wants_monitor(&self, index: usize, device: &str, is_primary: bool) -> bool {
        self.monitors.iter().any(|selector| {
            let selector = selector.trim();
            if selector.eq_ignore_ascii_case("all") {
                return true;
            }
            if selector.eq_ignore_ascii_case("primary") {
                return is_primary;
            }
            if let Ok(wanted) = selector.parse::<usize>() {
                return wanted == index;
            }
            selector.eq_ignore_ascii_case(device)
        })
    }
}
