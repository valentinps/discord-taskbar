//! Subscription bookkeeping.
//!
//! Some events are global; the interesting ones (voice states, speaking) are
//! scoped to a channel id and must be re-subscribed every time the user moves
//! between channels.

use serde_json::{json, Value};

use super::{RpcClient, Result};

/// Subscribed once per connection, no arguments.
///
/// `VOICE_CONNECTION_STATUS` is deliberately absent. It looks useful, but in
/// practice Discord pushes it every ~5 seconds carrying a full ping history —
/// measured at 6.8 KB per event. Deserialising that forever to learn nothing
/// we display is exactly the kind of idle cost this app is supposed to avoid.
/// Connection state is inferred from `VOICE_CHANNEL_SELECT` instead.
pub const GLOBAL_EVENTS: &[&str] = &["VOICE_CHANNEL_SELECT", "VOICE_SETTINGS_UPDATE"];

/// Subscribed per channel, with `{ "channel_id": ... }`.
pub const CHANNEL_EVENTS: &[&str] = &[
    "VOICE_STATE_CREATE",
    "VOICE_STATE_UPDATE",
    "VOICE_STATE_DELETE",
    "SPEAKING_START",
    "SPEAKING_STOP",
];

#[derive(Default)]
pub struct Subscriptions {
    channel_id: Option<String>,
}

impl Subscriptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe_global(&self, client: &mut RpcClient) -> Result<()> {
        for evt in GLOBAL_EVENTS {
            client.subscribe(evt, Value::Null)?;
        }
        Ok(())
    }

    /// Point the channel-scoped subscriptions at `channel_id` (or tear them
    /// down when it is `None`). A no-op if the channel has not changed.
    pub fn set_channel(
        &mut self,
        client: &mut RpcClient,
        channel_id: Option<&str>,
    ) -> Result<bool> {
        if self.channel_id.as_deref() == channel_id {
            return Ok(false);
        }

        if let Some(previous) = self.channel_id.take() {
            for evt in CHANNEL_EVENTS {
                // Best-effort: Discord may already have dropped these when the
                // channel went away, and that is not an error worth failing on.
                let _ = client.unsubscribe(evt, json!({ "channel_id": previous }));
            }
        }

        if let Some(next) = channel_id {
            for evt in CHANNEL_EVENTS {
                client.subscribe(evt, json!({ "channel_id": next }))?;
            }
            self.channel_id = Some(next.to_string());
        }

        Ok(true)
    }
}

/// Fetch the voice channel the user is currently in, if any.
pub fn selected_voice_channel(client: &mut RpcClient) -> Result<Option<Value>> {
    let data = client.call("GET_SELECTED_VOICE_CHANNEL", Value::Null)?;
    Ok(if data.is_null() { None } else { Some(data) })
}

pub fn guild(client: &mut RpcClient, guild_id: &str) -> Result<Value> {
    client.call("GET_GUILD", json!({ "guild_id": guild_id }))
}

/// Read the local client's voice settings.
pub fn voice_settings(client: &mut RpcClient) -> Result<Value> {
    client.call("GET_VOICE_SETTINGS", Value::Null)
}
