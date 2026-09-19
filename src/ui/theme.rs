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
    /// What to paint behind the widget, where the background is transparent.
    ///
    /// Empty means sample the taskbar, which is what makes the widget vanish
    /// into it. Set it to a colour when sampling cannot work — a translucent
    /// or blurred taskbar has no single colour to find, and the sampled
    /// average is only ever an approximation of one.
    pub taskbar_background: String,
    pub text: String,
    pub text_dim: String,
    /// Ring drawn around whoever is talking.
    pub speaking: String,
    /// Muted / deafened badge fill.
    pub danger: String,

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
            taskbar_background: String::new(),
            text: "#DBDEE1".to_string(),
            text_dim: "#949BA4".to_string(),
            // Discord's speaking green.
            speaking: "#23A559".to_string(),
            danger: "#DA373C".to_string(),

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
    /// `None` means sample the taskbar instead.
    pub taskbar_background: Option<Color>,
    pub text: Color,
    pub text_dim: Color,
    pub speaking: Color,
    pub danger: Color,

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
    pub monitors: Vec<String>,
    pub x_offset: i32,
    pub y_offset: i32,
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
            taskbar_background: if a.taskbar_background.trim().is_empty() {
                None
            } else {
                Color::from_hex(a.taskbar_background.trim())
            },
            text: parse(&a.text, Color::rgb(0xDB, 0xDE, 0xE1)),
            text_dim: parse(&a.text_dim, Color::rgb(0x94, 0x9B, 0xA4)),
            speaking: parse(&a.speaking, Color::rgb(0x23, 0xA5, 0x59)),
            danger: parse(&a.danger, Color::rgb(0xDA, 0x37, 0x3C)),

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
