//! Colours and metrics, all configurable.
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
    /// Ring drawn around whoever is talking.
    pub speaking: String,
    /// Muted / deafened badge fill.
    pub danger: String,
    /// Stands in for a picture that has not downloaded yet.
    pub placeholder: String,

    pub corner_radius: i32,
    pub padding: i32,
    /// Gap between elements.
    pub spacing: i32,
    pub avatar_size: i32,
    /// Overlap between adjacent avatars, so a full call stays compact.
    /// Zero butts them together; negative values leave a gap instead.
    pub avatar_overlap: i32,
    pub max_avatars: usize,
    /// Move whoever is speaking to the front. Turn off to keep the order
    /// stable so faces do not jump around mid-conversation.
    pub sort_by_speaking: bool,
    pub font_size: i32,
    pub icon_size: i32,
    /// Widget height at 96 DPI. `0` means fill the taskbar, which combined
    /// with a transparent `background` makes the widget look like part of the
    /// bar rather than a panel sitting on it.
    pub height: i32,
    /// Longest the channel/server label may get before it is ellipsised.
    pub max_label_width: i32,

    pub show_guild_name: bool,
    /// Show the server's icon in place of its name.
    pub show_guild_icon: bool,
    pub guild_icon_size: i32,
    pub show_self_icons: bool,
    /// Let clicking the microphone / headphone glyphs toggle mute and deafen.
    /// Needs the `rpc.voice.write` scope, so changing this means re-authorizing.
    pub clickable_self_icons: bool,
    /// A thin rule between the participants and your own controls.
    pub show_divider: bool,
    pub divider: String,
    /// A hang-up button that leaves the voice channel.
    pub show_leave_button: bool,
    /// What the middle mouse button does over a participant:
    /// `local_mute`, `volume_reset`, or `none`.
    pub middle_click: String,
    /// Volume change per notch of the scroll wheel, over a participant.
    pub scroll_volume_step: i32,
    /// Shape of Discord's volume curve; see `crate::volume`.
    ///
    /// Measured against a live client rather than documented, so it is a
    /// setting: if Discord retunes the curve this is a number to change
    /// rather than a rebuild.
    pub volume_curve: f32,
    /// Decibels Discord spreads the boost range (100%..200%) over.
    pub volume_boost_db: f32,
    /// Top of the per-user volume scale, as a percentage.
    ///
    /// This is a *perceptual* percentage — the number Discord's own slider
    /// shows — so it lines up with whatever limit the client allows.
    ///
    /// Discord's own limit is 200, and client plugins exist that raise it.
    /// Setting this to match such a plugin is what keeps the bar and the
    /// number agreeing with Discord when one is in use.
    pub max_volume: i32,
    /// Which displays to show the widget on.
    ///
    /// `["primary"]`, `["all"]`, or a list of monitor indices (`["0", "1"]`)
    /// or device names (`["\\.\DISPLAY1"]`). The tray menu edits this,
    /// which saves having to look the names up.
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
            // Discord's speaking green.
            speaking: "#23A559".to_string(),
            danger: "#DA373C".to_string(),
            placeholder: "#4E5058".to_string(),

            corner_radius: 6,
            padding: 10,
            spacing: 8,
            avatar_size: 22,
            avatar_overlap: 6,
            max_avatars: 6,
            sort_by_speaking: true,
            font_size: 12,
            icon_size: 16,
            height: 32,
            max_label_width: 220,

            show_guild_name: false,
            show_guild_icon: true,
            guild_icon_size: 18,
            show_self_icons: true,
            clickable_self_icons: true,
            show_divider: true,
            divider: "#4E5058".to_string(),
            show_leave_button: true,
            middle_click: "local_mute".to_string(),
            scroll_volume_step: 10,
            volume_curve: crate::volume::DEFAULT_CURVE,
            volume_boost_db: crate::volume::DEFAULT_BOOST_DB,
            max_volume: 200,
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
    pub speaking: Color,
    pub danger: Color,
    pub placeholder: Color,

    pub corner_radius: i32,
    pub padding: i32,
    pub spacing: i32,
    pub avatar_size: i32,
    pub avatar_overlap: i32,
    pub max_avatars: usize,
    pub sort_by_speaking: bool,
    pub font_size: i32,
    pub icon_size: i32,
    pub height: i32,
    pub max_label_width: i32,

    pub show_guild_name: bool,
    pub show_guild_icon: bool,
    pub guild_icon_size: i32,
    pub show_self_icons: bool,
    pub clickable_self_icons: bool,
    pub show_divider: bool,
    pub divider: Color,
    pub show_leave_button: bool,
    pub middle_click: MiddleClick,
    pub scroll_volume_step: i32,
    pub volume_curve: f32,
    pub volume_boost_db: f32,
    pub max_volume: f32,
    pub monitors: Vec<String>,
    pub x_offset: i32,
    pub y_offset: i32,
}

impl Theme {
    /// What Discord's own slider shows for a stored amplitude.
    ///
    /// RPC deals in amplitude; every number the user sees should be this.
    pub fn shown_volume(&self, amplitude: f32) -> f32 {
        crate::volume::amplitude_to_perceptual(amplitude, self.volume_curve, self.volume_boost_db)
    }

    /// The amplitude to store for a number the user chose.
    pub fn stored_volume(&self, perceptual: f32) -> f32 {
        crate::volume::perceptual_to_amplitude(perceptual, self.volume_curve, self.volume_boost_db)
    }

    /// The top of the volume scale to draw and map against.
    ///
    /// Normally just the configured maximum. Set that to match whatever
    /// Discord's own slider allows — 200 by default, more if a client plugin
    /// has raised it — and the bar and the number agree with Discord.
    ///
    /// The rest of this exists for when it has not been set. A volume above
    /// the ceiling would otherwise peg the bar at full while the number kept
    /// climbing, so the scale grows to the next whole hundred instead.
    /// Rounding up rather than taking the value itself matters: a ceiling
    /// equal to the current volume would leave the bar full at every value
    /// above the configured maximum, which is no more informative than
    /// clipping it.
    pub fn volume_ceiling(&self, current: f32) -> f32 {
        if current <= self.max_volume {
            return self.max_volume.max(1.0);
        }
        (current / 100.0).ceil() * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme_with_max(max: i32) -> Theme {
        Theme::from(&Appearance {
            max_volume: max,
            ..Appearance::default()
        })
    }

    #[test]
    fn ceiling_is_the_configured_maximum_in_the_ordinary_case() {
        let theme = theme_with_max(200);
        assert_eq!(theme.volume_ceiling(0.0), 200.0);
        assert_eq!(theme.volume_ceiling(100.0), 200.0);
        assert_eq!(theme.volume_ceiling(200.0), 200.0);
    }

    #[test]
    fn a_configured_maximum_is_used_as_given() {
        // What someone running a 400% plugin would set.
        let theme = theme_with_max(400);
        assert_eq!(theme.volume_ceiling(350.0), 400.0);
        assert_eq!(theme.volume_ceiling(400.0), 400.0);
    }

    #[test]
    fn an_unconfigured_maximum_grows_to_the_next_hundred() {
        // The bar must not sit full across a whole range of values.
        let theme = theme_with_max(200);
        assert_eq!(theme.volume_ceiling(250.0), 300.0);
        assert_eq!(theme.volume_ceiling(300.0), 300.0);
        assert_eq!(theme.volume_ceiling(301.0), 400.0);

        // Distinct volumes above the limit must fill the bar differently.
        let a = 250.0 / theme.volume_ceiling(250.0);
        let b = 300.0 / theme.volume_ceiling(300.0);
        assert!(a < b, "{a} should fill less of the bar than {b}");
    }

    #[test]
    fn the_ceiling_is_never_zero() {
        // Dividing the bar width by this must always be safe.
        assert!(theme_with_max(100).volume_ceiling(0.0) > 0.0);
    }
}

/// What the middle mouse button does over a participant.
///
/// Server mute, server deafen and disconnecting another user are deliberately
/// absent: Discord's RPC API has no command for any of them. They are guild
/// moderation actions and would need a bot with those permissions, which is a
/// different piece of software.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddleClick {
    None,
    LocalMute,
    VolumeReset,
}

impl MiddleClick {
    fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "local_mute" => MiddleClick::LocalMute,
            "volume_reset" => MiddleClick::VolumeReset,
            _ => MiddleClick::None,
        }
    }
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
            background: parse(&a.background, parse(&defaults.background, Color::rgb(43, 45, 49))),
            text: parse(&a.text, Color::rgb(0xDB, 0xDE, 0xE1)),
            text_dim: parse(&a.text_dim, Color::rgb(0x94, 0x9B, 0xA4)),
            speaking: parse(&a.speaking, Color::rgb(0x23, 0xA5, 0x59)),
            danger: parse(&a.danger, Color::rgb(0xDA, 0x37, 0x3C)),
            placeholder: parse(&a.placeholder, Color::rgb(0x4E, 0x50, 0x58)),

            corner_radius: a.corner_radius.max(0),
            padding: a.padding.max(0),
            spacing: a.spacing.max(0),
            avatar_size: a.avatar_size.max(8),
            // Negative is meaningful here: it becomes a gap between avatars.
            avatar_overlap: a.avatar_overlap.clamp(-40, a.avatar_size.max(8) - 4),
            max_avatars: a.max_avatars.clamp(1, 32),
            sort_by_speaking: a.sort_by_speaking,
            font_size: a.font_size.clamp(6, 40),
            icon_size: a.icon_size.max(6),
            // 0 is meaningful: fill the taskbar.
            height: if a.height == 0 { 0 } else { a.height.clamp(12, 80) },
            max_label_width: a.max_label_width.max(40),

            show_guild_name: a.show_guild_name,
            show_guild_icon: a.show_guild_icon,
            guild_icon_size: a.guild_icon_size.clamp(8, 48),
            show_self_icons: a.show_self_icons,
            clickable_self_icons: a.clickable_self_icons,
            show_divider: a.show_divider,
            divider: parse(&a.divider, Color::rgb(0x4E, 0x50, 0x58)),
            show_leave_button: a.show_leave_button,
            middle_click: MiddleClick::parse(&a.middle_click),
            scroll_volume_step: a.scroll_volume_step.clamp(1, 50),
            volume_curve: a.volume_curve.clamp(0.1, 10.0),
            volume_boost_db: a.volume_boost_db.clamp(0.1, 60.0),
            max_volume: a.max_volume.clamp(100, 1000) as f32,
            monitors: if a.monitors.is_empty() {
                vec!["primary".to_string()]
            } else {
                a.monitors.clone()
            },
            x_offset: a.x_offset,
            y_offset: a.y_offset,
        }
    }
}

impl Theme {
    /// Whether the widget should appear on a given taskbar.
    ///
    /// A selector is `primary`, `all`, a monitor index, or a device name.
    /// Indices are what the tray menu writes because they are what a person
    /// can actually read; device names are accepted because they survive a
    /// display being unplugged and re-plugged in a different order.
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
