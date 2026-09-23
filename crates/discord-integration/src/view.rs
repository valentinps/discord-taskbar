//! Turning Discord's voice state into blocks the widget draws.
//!
//! Everything the taskbar shows for Discord is decided here. There is no
//! drawing code: the shapes, the ring around whoever is talking, the mute
//! badge and the overlap between avatars are all described as data, and
//! `taskbar_widget::ui::block` does the rest.
//!
//! Adding something to the widget — a ping readout, a screenshare marker — is
//! a matter of pushing another block onto this list.

use taskbar_widget::assets::images::OVERSAMPLE;
use crate::model::VoiceStatus;
use taskbar_widget::ui::block::{Block, BlockId, Content, Ring, Shape, Span};
use taskbar_widget::ui::theme::Theme;

use super::icons as glyphs;
use super::settings::Settings;

/// Clicking here brings the Discord window forward.
pub const ID_FOCUS: &str = "focus";
/// Your own microphone and headphones.
pub const ID_MUTE: &str = "self:mute";
pub const ID_DEAFEN: &str = "self:deafen";
/// The hang-up button.
pub const ID_LEAVE: &str = "leave";
/// One participant, as `user:<id>`.
pub const ID_USER: &str = "user";

/// How Discord writes a volume: the perceptual percentage from its own
/// slider, with no decimal places.
pub fn percent(value: f32) -> String {
    format!("{}%", value.round() as i32)
}

/// The user this block is about, if it is about one.
pub fn user_of(id: &BlockId) -> Option<&str> {
    id.suffix(ID_USER)
}

/// Between the server icon and the channel name.
const CHEVRON: &str = "›";
/// Between the channel name and the server name.
const NAME_SEPARATOR: &str = "  ·  ";

/// Everything the widget should show for `status`, left to right.
pub fn blocks(status: &VoiceStatus, theme: &Theme, settings: &Settings) -> Vec<Block> {
    let mut blocks = Vec::new();

    if let Some(icon) = guild_icon(status, theme, settings) {
        blocks.push(icon);
        // Only worth a chevron when there is something on both sides of it.
        if status.channel_name.is_some() {
            blocks.push(Block::new(Content::Text {
                spans: vec![Span::new(CHEVRON, theme.text_dim)],
                max_width: None,
            }));
        }
    }

    if let Some(label) = channel_label(status, theme, settings) {
        blocks.push(label);
    }

    if let Some(row) = avatar_row(status, theme, settings) {
        blocks.push(row);
    }

    if settings.show_divider && !status.participants.is_empty() {
        blocks.push(Block::new(Content::Rule {
            color: theme.divider,
        }));
    }

    if let Some(icons) = self_icons(status, theme, settings) {
        blocks.push(icons);
    }

    if settings.show_leave_button && status.is_connected() {
        blocks.push(
            Block::new(Content::Icon {
                glyph: glyphs::HANG_UP,
                size: theme.icon_size,
                color: theme.danger,
            })
            .with_id(ID_LEAVE),
        );
    }

    blocks
}

/// The server's icon, as a circle.
///
/// People recognise servers by icon far more than by name, and an icon costs a
/// fraction of the width a name does — which matters on a taskbar.
fn guild_icon(status: &VoiceStatus, theme: &Theme, settings: &Settings) -> Option<Block> {
    if !settings.show_guild_icon {
        return None;
    }
    let url = status.guild_icon_url.as_ref()?;
    let id = status.guild_id.as_ref()?;

    let size = settings.guild_icon_size;
    // Discord's CDN takes a size hint, and asking for a small image saves both
    // bandwidth and decode time.
    let separator = if url.contains('?') { '&' } else { '?' };
    let sized = format!(
        "{url}{separator}size={}",
        (size as u32 * OVERSAMPLE).next_power_of_two()
    );

    Some(
        Block::new(Content::Image {
            image: Some(taskbar_widget::assets::images::ImageRef::new(
                format!("guild_{id}"),
                sized,
            )),
            size,
            shape: Shape::Circle,
            opacity: 1.0,
            ring: None,
            badge: None,
            placeholder: theme.placeholder,
        })
        .with_id(ID_FOCUS),
    )
}

/// Channel name, optionally followed by the server name in a dimmer colour.
///
/// Channel first deliberately: when the label has to be ellipsised it is the
/// server name that should lose characters, not the channel you are sitting in
/// — which is what the first span's priority in `Content::Text` gives us.
fn channel_label(status: &VoiceStatus, theme: &Theme, settings: &Settings) -> Option<Block> {
    let channel = status.channel_name.clone()?;
    if channel.is_empty() {
        return None;
    }

    let mut spans = vec![Span::new(channel, theme.text)];
    if settings.show_guild_name {
        if let Some(guild) = &status.guild_name {
            spans.push(Span::new(
                format!("{NAME_SEPARATOR}{guild}"),
                theme.text_dim,
            ));
        }
    }

    Some(
        Block::new(Content::Text {
            spans,
            max_width: Some(theme.max_label_width),
        })
        .with_id(ID_FOCUS),
    )
}

/// Circular avatars, ringed while their owner is talking and badged when they
/// are muted or deafened.
fn avatar_row(status: &VoiceStatus, theme: &Theme, settings: &Settings) -> Option<Block> {
    let people = status.sorted_participants(settings.sort_by_speaking);
    if people.is_empty() {
        return None;
    }

    let size = settings.avatar_size;
    let items: Vec<Block> = people
        .into_iter()
        .take(settings.max_avatars)
        .map(|participant| {
            let badge = (participant.is_deafened() || participant.is_muted()).then(|| {
                taskbar_widget::ui::block::Badge {
                    glyph: if participant.is_deafened() {
                        glyphs::HEADPHONES
                    } else {
                        glyphs::MICROPHONE
                    },
                    fill: theme.danger,
                }
            });

            Block::new(Content::Image {
                image: Some(participant.avatar_ref(size as u32)),
                size,
                shape: Shape::Circle,
                // Someone silenced on our end is dimmed, which distinguishes
                // it from the red badge that means *they* muted themselves.
                opacity: if participant.is_locally_silenced() {
                    0.35
                } else {
                    1.0
                },
                ring: participant.speaking.then(|| Ring::new(theme.accent)),
                badge,
                placeholder: theme.placeholder,
            })
            .with_id(format!("{ID_USER}:{}", participant.user_id))
        })
        .collect();

    Some(Block::new(Content::Cluster {
        items,
        overlap: settings.avatar_overlap,
    }))
}

/// Your own microphone and headphone state, optionally clickable to toggle.
///
/// A cluster with a negative overlap rather than two separate blocks: these
/// two belong together and sit closer than the widget's normal spacing.
fn self_icons(status: &VoiceStatus, theme: &Theme, settings: &Settings) -> Option<Block> {
    if !settings.show_self_icons {
        return None;
    }

    let state = status.self_state;
    let muted = state.is_muted();
    let size = theme.icon_size;

    let mic = Content::Icon {
        glyph: if muted {
            glyphs::MICROPHONE_OFF
        } else {
            glyphs::MICROPHONE
        },
        size,
        color: if muted { theme.danger } else { theme.text_dim },
    };
    let ear = Content::Icon {
        glyph: if state.deaf {
            glyphs::HEADPHONES_OFF
        } else {
            glyphs::HEADPHONES
        },
        size,
        color: if state.deaf {
            theme.danger
        } else {
            theme.text_dim
        },
    };

    // No ids when they are for looking at rather than pressing, which is also
    // what keeps the hand cursor off them.
    let (mut mic, mut ear) = (Block::new(mic), Block::new(ear));
    if settings.clickable_self_icons {
        mic = mic.with_id(ID_MUTE);
        ear = ear.with_id(ID_DEAFEN);
    }

    Some(Block::new(Content::Cluster {
        items: vec![mic, ear],
        overlap: -6,
    }))
}
