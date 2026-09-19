//! What the widget displays, independent of where it came from.
//!
//! The RPC provider fills this in today; a Vencord provider could fill in the
//! same struct tomorrow. Nothing in `ui/` knows which.

use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SelfState {
    pub mute: bool,
    pub deaf: bool,
}

impl SelfState {
    /// Deafening yourself in Discord always mutes you too, but the voice
    /// settings can report `mute: false` alongside `deaf: true`. Display has to
    /// follow what is actually happening to the microphone, not the raw flag.
    pub fn is_muted(&self) -> bool {
        self.mute || self.deaf
    }
}

// Not `Eq`: volume is a float. Equality is only used to skip redundant
// redraws, where partial equality is exactly the right semantics.
#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    pub user_id: String,
    /// Server nickname, global name, or username — whichever Discord gives us.
    pub display_name: String,
    /// Avatar hash; `None` means the user has no custom avatar.
    pub avatar_hash: Option<String>,
    pub speaking: bool,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub server_mute: bool,
    pub server_deaf: bool,
    /// Muted locally, by us - nobody else is affected.
    pub local_mute: bool,
    /// Local playback volume, 0-200, where 100 is unchanged.
    pub volume: f32,
    pub is_self: bool,
}

impl Participant {
    pub fn new(user_id: String, display_name: String) -> Self {
        Participant {
            user_id,
            display_name,
            avatar_hash: None,
            speaking: false,
            self_mute: false,
            self_deaf: false,
            server_mute: false,
            server_deaf: false,
            local_mute: false,
            volume: 100.0,
            is_self: false,
        }
    }

    /// Whether this person's microphone is off.
    ///
    /// Self-deafening implies self-muting — that is how the Discord client
    /// behaves, and the voice state does not always say so explicitly. Server
    /// deafen is deliberately *not* included: a server-side sound mute can
    /// leave someone with their microphone still live.
    pub fn is_muted(&self) -> bool {
        self.self_mute || self.server_mute || self.self_deaf
    }

    /// Whether we have silenced this person on our end only.
    pub fn is_locally_silenced(&self) -> bool {
        self.local_mute || self.volume <= 0.5
    }

    /// Whether this person cannot hear.
    pub fn is_deafened(&self) -> bool {
        self.self_deaf || self.server_deaf
    }

    /// CDN path for this user's avatar at `size` px, or the shared default.
    ///
    /// Always `.png`: Discord will happily serve WebP, but WIC has no
    /// guaranteed WebP decoder, and PNG avoids the whole question.
    pub fn avatar_path(&self, size: u32) -> String {
        match &self.avatar_hash {
            Some(hash) => format!("/avatars/{}/{}.png?size={}", self.user_id, hash, size),
            None => {
                // Default avatars are indexed by (id >> 22) % 6 for migrated
                // accounts; the legacy discriminator scheme is gone.
                let index = self
                    .user_id
                    .parse::<u64>()
                    .map(|id| (id >> 22) % 6)
                    .unwrap_or(0);
                format!("/embed/avatars/{index}.png")
            }
        }
    }

    /// Cache key — stable across restarts, safe as a filename.
    pub fn avatar_key(&self) -> String {
        match &self.avatar_hash {
            Some(hash) => format!("{}_{}", self.user_id, hash),
            None => {
                let index = self
                    .user_id
                    .parse::<u64>()
                    .map(|id| (id >> 22) % 6)
                    .unwrap_or(0);
                format!("default_{index}")
            }
        }
    }
}

/// How the voice connection itself is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    /// Discord reported a state we do not specifically handle.
    Other,
}

impl ConnectionState {
    pub fn from_rpc(state: &str) -> Self {
        match state {
            "DISCONNECTED" => ConnectionState::Disconnected,
            "AWAITING_ENDPOINT" | "AUTHENTICATING" | "CONNECTING" | "RTC_CONNECTING" => {
                ConnectionState::Connecting
            }
            "CONNECTED" | "VOICE_CONNECTED" | "RTC_CONNECTED" => ConnectionState::Connected,
            _ => ConnectionState::Other,
        }
    }
}

/// Everything the widget needs for one frame.
#[derive(Debug, Clone, Default)]
pub struct VoiceStatus {
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    /// `None` for a DM or group call.
    pub guild_id: Option<String>,
    pub guild_name: Option<String>,
    /// Absolute URL of the server's icon, straight from `GET_GUILD`.
    pub guild_icon_url: Option<String>,
    pub participants: Vec<Participant>,
    pub self_state: SelfState,
    pub connection: ConnectionState,
}

impl VoiceStatus {
    pub fn is_connected(&self) -> bool {
        self.channel_id.is_some()
    }

    /// Channel and server as one line, e.g. `My Server / General`.
    pub fn location_label(&self) -> String {
        match (&self.guild_name, &self.channel_name) {
            (Some(guild), Some(channel)) => format!("{guild} / {channel}"),
            (None, Some(channel)) => channel.clone(),
            _ => String::new(),
        }
    }

    pub fn participant_mut(&mut self, user_id: &str) -> Option<&mut Participant> {
        self.participants.iter_mut().find(|p| p.user_id == user_id)
    }

    /// Ordered for display.
    ///
    /// With `by_speaking`, whoever is talking is pulled to the front so the
    /// people worth looking at survive the `max_avatars` cut. Without it the
    /// order is fixed and alphabetical, so faces stay put during a
    /// conversation instead of shuffling on every utterance.
    pub fn sorted_participants(&self, by_speaking: bool) -> Vec<&Participant> {
        let mut sorted: Vec<&Participant> = self.participants.iter().collect();
        if by_speaking {
            sorted.sort_by(|a, b| {
                b.speaking
                    .cmp(&a.speaking)
                    .then_with(|| b.is_self.cmp(&a.is_self))
                    .then_with(|| a.display_name.cmp(&b.display_name))
            });
        } else {
            sorted.sort_by(|a, b| {
                b.is_self
                    .cmp(&a.is_self)
                    .then_with(|| a.display_name.cmp(&b.display_name))
            });
        }
        sorted
    }
}

/// Shared immutable snapshot handed from the provider thread to the UI thread.
pub type StatusSnapshot = Arc<VoiceStatus>;
