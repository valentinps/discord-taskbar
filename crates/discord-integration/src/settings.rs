//! What the Discord integration lets you change.
//!
//! These used to sit in the widget's `Appearance`, which meant the widget knew
//! what an avatar was. They now live in `integrations.discord` in the same
//! config file; `crate::config` moves them across on first load so an existing
//! install keeps its settings.

use serde::{Deserialize, Serialize};

use taskbar_widget::config::{Config, Credentials};
use taskbar_widget::ui::settings::{Field, Intro};

use super::volume;

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

/// Serialisable form, as it appears in `config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Stored {
    /// Application ID from <https://discord.com/developers/applications>.
    pub client_id: String,
    /// OAuth2 client secret for the same application.
    pub client_secret: String,

    pub avatar_size: i32,
    /// Overlap between adjacent avatars, so a full call stays compact. Zero
    /// butts them together; negative values leave a gap instead.
    pub avatar_overlap: i32,
    pub max_avatars: usize,
    /// Move whoever is speaking to the front. Turn off to keep the order
    /// stable so faces do not jump around mid-conversation.
    pub sort_by_speaking: bool,

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
    /// A hang-up button that leaves the voice channel.
    pub show_leave_button: bool,

    /// `local_mute`, `volume_reset`, or `none`.
    pub middle_click: String,
    /// Volume change per notch of the scroll wheel, over a participant.
    pub scroll_volume_step: i32,
    /// Shape of Discord's volume curve; see `super::volume`.
    ///
    /// Measured against a live client rather than documented, so it is a
    /// setting: if Discord retunes the curve this is a number to change rather
    /// than a rebuild.
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
}

impl Default for Stored {
    fn default() -> Self {
        Stored {
            client_id: String::new(),
            client_secret: String::new(),

            avatar_size: 22,
            avatar_overlap: 6,
            max_avatars: 6,
            sort_by_speaking: true,

            show_guild_name: false,
            show_guild_icon: true,
            guild_icon_size: 18,
            show_self_icons: true,
            clickable_self_icons: true,
            show_divider: true,
            show_leave_button: true,

            middle_click: "local_mute".to_string(),
            scroll_volume_step: 10,
            volume_curve: volume::DEFAULT_CURVE,
            volume_boost_db: volume::DEFAULT_BOOST_DB,
            max_volume: 200,
        }
    }
}

/// Checked, ready-to-use form of [`Stored`].
#[derive(Debug, Clone)]
pub struct Settings {
    pub avatar_size: i32,
    pub avatar_overlap: i32,
    pub max_avatars: usize,
    pub sort_by_speaking: bool,

    pub show_guild_name: bool,
    pub show_guild_icon: bool,
    pub guild_icon_size: i32,
    pub show_self_icons: bool,
    pub clickable_self_icons: bool,
    pub show_divider: bool,
    pub show_leave_button: bool,

    pub middle_click: MiddleClick,
    pub scroll_volume_step: i32,
    pub volume_curve: f32,
    pub volume_boost_db: f32,
    pub max_volume: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings::from(&Stored::default())
    }
}

impl From<&Stored> for Settings {
    fn from(s: &Stored) -> Self {
        Settings {
            avatar_size: s.avatar_size.max(8),
            // Negative is meaningful here: it becomes a gap between avatars.
            avatar_overlap: s.avatar_overlap.clamp(-40, s.avatar_size.max(8) - 4),
            max_avatars: s.max_avatars.clamp(1, 32),
            sort_by_speaking: s.sort_by_speaking,

            show_guild_name: s.show_guild_name,
            show_guild_icon: s.show_guild_icon,
            guild_icon_size: s.guild_icon_size.clamp(8, 64),
            show_self_icons: s.show_self_icons,
            clickable_self_icons: s.clickable_self_icons,
            show_divider: s.show_divider,
            show_leave_button: s.show_leave_button,

            middle_click: MiddleClick::parse(&s.middle_click),
            scroll_volume_step: s.scroll_volume_step.clamp(1, 50),
            volume_curve: s.volume_curve,
            volume_boost_db: s.volume_boost_db,
            max_volume: s.max_volume.clamp(100, 1000) as f32,
        }
    }
}

impl Settings {
    /// What Discord's own slider shows for a stored amplitude.
    ///
    /// RPC deals in amplitude; every number the user sees should be this.
    pub fn shown_volume(&self, amplitude: f32) -> f32 {
        volume::amplitude_to_perceptual(amplitude, self.volume_curve, self.volume_boost_db)
    }

    /// The amplitude to store for a number the user chose.
    pub fn stored_volume(&self, perceptual: f32) -> f32 {
        volume::perceptual_to_amplitude(perceptual, self.volume_curve, self.volume_boost_db)
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

/// This integration's key in `config.json`, and its `Integration::id`.
pub const ID: &str = "discord";

/// Where to make the Discord application. Shown in the settings window and
/// opened by the button beside it.
pub const PORTAL_URL: &str = "https://discord.com/developers/applications";

/// What the settings window says before any of the rows.
pub fn intro() -> Intro {
    Intro {
        text: concat!(
            "Set up a Discord application of your own, then paste its Client ID ",
            "and Client Secret below. Under OAuth2, add exactly  http://localhost  ",
            "as a redirect URI and save.

",
            "Nothing is sent anywhere except your own Discord client, on this machine.",
        )
        .to_string(),
        button: "Open Discord developer portal".to_string(),
        url: PORTAL_URL.to_string(),
    }
}

/// The Discord credentials stored in `config.json`.
pub fn credentials(config: &Config) -> Credentials {
    let stored: Stored = serde_json::from_value(config.integration(ID)).unwrap_or_default();
    Credentials {
        client_id: stored.client_id,
        client_secret: stored.client_secret,
    }
}

/// Write credentials into a config, leaving every other setting alone.
///
/// The installer's wizard collects them, and only this crate knows where they
/// belong in the file.
pub fn set_credentials(config: &mut Config, creds: &Credentials) {
    let mut stored: Stored =
        serde_json::from_value(config.integration(ID)).unwrap_or_default();
    stored.client_id = creds.client_id.trim().to_string();
    stored.client_secret = creds.client_secret.trim().to_string();

    if let Ok(value) = serde_json::to_value(&stored) {
        config.integrations.insert(ID.to_string(), value);
    }
}

/// The rows this integration adds to the settings window.
pub fn fields() -> Vec<Field> {
    let p = |key: &str| format!("integrations.discord.{key}");

    vec![
        Field::text("Discord application", "Client ID", p("client_id")),
        Field::text("Discord application", "Client secret", p("client_secret")),
        Field::number("Participants", "Avatar size", p("avatar_size"), 8, 64),
        Field::number(
            "Participants",
            "Overlap (negative gaps)",
            p("avatar_overlap"),
            -40,
            40,
        ),
        Field::number("Participants", "Most avatars shown", p("max_avatars"), 1, 32),
        Field::toggle(
            "Participants",
            "Speakers move to the front",
            p("sort_by_speaking"),
        ),
        Field::decimal("Participants", "Volume curve", p("volume_curve"), 0.1, 10.0),
        Field::decimal(
            "Participants",
            "Volume boost dB",
            p("volume_boost_db"),
            0.0,
            60.0,
        ),
        Field::number(
            "Participants",
            "Maximum volume %",
            p("max_volume"),
            100,
            1000,
        ),
        Field::number(
            "Participants",
            "Volume per wheel notch",
            p("scroll_volume_step"),
            1,
            50,
        ),
        Field::choice(
            "Participants",
            "Middle-click does",
            p("middle_click"),
            ["local_mute", "volume_reset", "none"],
        ),
        Field::toggle("What to show", "Server icon", p("show_guild_icon")),
        Field::toggle("What to show", "Server name", p("show_guild_name")),
        Field::toggle(
            "What to show",
            "Your mic and headphones",
            p("show_self_icons"),
        ),
        Field::toggle(
            "What to show",
            "Clicking those toggles them",
            p("clickable_self_icons"),
        ),
        Field::toggle("What to show", "Divider", p("show_divider")),
        Field::toggle("What to show", "Hang-up button", p("show_leave_button")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_max(max: i32) -> Settings {
        Settings::from(&Stored {
            max_volume: max,
            ..Stored::default()
        })
    }

    #[test]
    fn ceiling_is_the_configured_maximum_in_the_ordinary_case() {
        let settings = with_max(200);
        assert_eq!(settings.volume_ceiling(0.0), 200.0);
        assert_eq!(settings.volume_ceiling(100.0), 200.0);
        assert_eq!(settings.volume_ceiling(200.0), 200.0);
    }

    #[test]
    fn a_configured_maximum_is_used_as_given() {
        // What someone running a 400% plugin would set.
        let settings = with_max(400);
        assert_eq!(settings.volume_ceiling(350.0), 400.0);
        assert_eq!(settings.volume_ceiling(400.0), 400.0);
    }

    #[test]
    fn an_unconfigured_maximum_grows_to_the_next_hundred() {
        // The bar must not sit full across a whole range of values.
        let settings = with_max(200);
        assert_eq!(settings.volume_ceiling(250.0), 300.0);
        assert_eq!(settings.volume_ceiling(300.0), 300.0);
        assert_eq!(settings.volume_ceiling(301.0), 400.0);

        // Distinct volumes above the limit must fill the bar differently.
        let a = 250.0 / settings.volume_ceiling(250.0);
        let b = 300.0 / settings.volume_ceiling(300.0);
        assert!(a < b, "{a} should fill less of the bar than {b}");
    }

    #[test]
    fn the_ceiling_is_never_zero() {
        // Dividing the bar width by this must always be safe.
        assert!(with_max(100).volume_ceiling(0.0) > 0.0);
    }
}
