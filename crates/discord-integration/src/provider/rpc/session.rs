//! The long-running RPC session: connect, authenticate, subscribe, and fold
//! incoming events into a `VoiceStatus` the UI can render.
//!
//! Runs on its own thread and blocks on the pipe. Each change produces an
//! immutable snapshot handed to the UI thread, so the UI never touches
//! provider state and never blocks on I/O.

use std::time::Duration;

use serde_json::Value;

use super::{events, oauth, Event, RpcClient, RpcError, Result};
use taskbar_widget::config::Credentials;
use crate::model::{ConnectionState, Participant, VoiceStatus};
use crate::provider::{ProviderControl, ProviderEvent};
use taskbar_widget::ui::integration::EventSink;

/// Reconnect backoff bounds. Discord being closed is normal, not an error, so
/// we retry quietly rather than giving up.
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Run forever: connect, serve events, reconnect on failure.
pub fn run(sink: EventSink, control: ProviderControl) {
    let mut backoff = BACKOFF_MIN;

    loop {
        // Read afresh each time round, so credentials saved in the settings
        // window are picked up without restarting the app.
        let creds = control.credentials();
        let outcome = session(&creds, &sink, &control);
        control.detach();

        match outcome {
            // A clean end means Discord closed the connection; retry promptly.
            Ok(()) => backoff = BACKOFF_MIN,
            Err(RpcError::NotRunning(_)) => {
                sink.send(ProviderEvent::Offline("Discord is not running".to_string()));
            }
            Err(error) => {
                let fatal = matches!(error, RpcError::Config(_));
                sink.send(ProviderEvent::Offline(error.to_string()));
                if fatal {
                    // Bad credentials will not fix themselves by retrying, so
                    // wait for different ones rather than hammering Discord.
                    sink.send(ProviderEvent::Status(VoiceStatus::default().into()));
                    control.wait_for_credentials(&creds, None);
                    backoff = BACKOFF_MIN;
                    continue;
                }
            }
        }

        sink.send(ProviderEvent::Status(VoiceStatus::default().into()));
        // Cut short by new credentials, so saving them takes effect at once.
        control.wait_for_credentials(&creds, Some(backoff));
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

fn session(creds: &Credentials, sink: &EventSink, control: &ProviderControl) -> Result<()> {
    let mut client = RpcClient::connect(&creds.client_id)?;
    control.attach(client.raw_handle(), client.shared_writer());

    // A re-authorize request means the cached token is deliberately discarded.
    if control.take_reauthorize() {
        oauth::clear_token();
    }

    let self_id = client
        .ready_user
        .as_ref()
        .and_then(|u| u.get("id").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();

    oauth::login(&mut client, creds, || {
        sink.send(ProviderEvent::AwaitingAuthorization);
    })?;

    let mut subs = events::Subscriptions::new();
    subs.subscribe_global(&mut client)?;

    let mut status = VoiceStatus::default();

    // Seed from current state: the user may already have been in a call before
    // we started, and events only describe changes from here on.
    seed(&mut client, &mut subs, &mut status, &self_id)?;
    sink.send(ProviderEvent::Status(status.clone().into()));

    loop {
        let event = client.next_event()?;

        // An unsolicited ERROR is the reply to a fire-and-forget command, and
        // the only place a refused mute would ever show up.
        if event.name == "ERROR" {
            let message = event
                .data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Discord refused the request");
            sink.send(ProviderEvent::CommandFailed(message.to_string()));
            continue;
        }

        let changed = apply(&mut client, &mut subs, &mut status, &self_id, event)?;
        if changed {
            sink.send(ProviderEvent::Status(status.clone().into()));
        }
    }
}

/// Populate `status` from whatever channel the user is in right now.
fn seed(
    client: &mut RpcClient,
    subs: &mut events::Subscriptions,
    status: &mut VoiceStatus,
    self_id: &str,
) -> Result<()> {
    let Some(channel) = events::selected_voice_channel(client)? else {
        *status = VoiceStatus::default();
        subs.set_channel(client, None)?;
        return Ok(());
    };

    let channel_id = channel
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    status.channel_id = Some(channel_id.clone());
    status.channel_name = channel
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    status.connection = ConnectionState::Connected;

    status.guild_id = channel
        .get("guild_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    // Cleared before re-reading. A DM or group call has no guild at all, and
    // GET_GUILD can fail, so leaving the previous values in place would put
    // the server you just left next to the channel you just joined.
    status.guild_name = None;
    status.guild_icon_url = None;

    if let Some(guild) = status
        .guild_id
        .as_deref()
        .and_then(|id| events::guild(client, id).ok())
    {
        status.guild_name = guild
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string);
        status.guild_icon_url = guild
            .get("icon_url")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .map(str::to_string);
    }

    status.participants = channel
        .get("voice_states")
        .and_then(Value::as_array)
        .map(|states| {
            states
                .iter()
                .filter_map(|state| participant_from(state, self_id))
                .collect()
        })
        .unwrap_or_default();

    subs.set_channel(client, Some(&channel_id))?;
    Ok(())
}

/// Re-read the selected channel after being removed from `left`.
///
/// The removal can reach us a moment before Discord's own idea of the selected
/// channel catches up, in which case the first read still answers `left`. One
/// short retry covers that without leaving the widget on a channel we know we
/// are no longer in.
fn reseed_after_leaving(
    client: &mut RpcClient,
    subs: &mut events::Subscriptions,
    status: &mut VoiceStatus,
    self_id: &str,
    left: Option<&str>,
) -> Result<()> {
    let self_state = status.self_state;
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(400));
        }
        seed(client, subs, status, self_id)?;
        if status.channel_id.as_deref() != left || left.is_none() {
            break;
        }
    }
    // Mute/deafen survive changing or leaving a call, as they do for
    // VOICE_CHANNEL_SELECT; seeding from nothing would reset them.
    status.self_state = self_state;
    Ok(())
}

/// Fold one event into `status`. Returns whether anything visible changed.
fn apply(
    client: &mut RpcClient,
    subs: &mut events::Subscriptions,
    status: &mut VoiceStatus,
    self_id: &str,
    event: Event,
) -> Result<bool> {
    match event.name.as_str() {
        "VOICE_CHANNEL_SELECT" => {
            let channel_id = event
                .data
                .get("channel_id")
                .and_then(Value::as_str)
                .map(str::to_string);

            match channel_id {
                Some(_) => {
                    // Re-read the channel wholesale: it is one round trip and
                    // it gets the member list, names and guild in one go.
                    seed(client, subs, status, self_id)?;
                }
                None => {
                    subs.set_channel(client, None)?;
                    let self_state = status.self_state;
                    *status = VoiceStatus::default();
                    // Mute/deafen survive leaving a call.
                    status.self_state = self_state;
                }
            }
            Ok(true)
        }

        "VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE" => {
            let Some(updated) = participant_from(&event.data, self_id) else {
                return Ok(false);
            };

            match status
                .participants
                .iter_mut()
                .find(|p| p.user_id == updated.user_id)
            {
                Some(existing) => {
                    // Voice-state events carry no speaking flag, so `updated`
                    // always has it false. Carry the live one over before
                    // comparing: the old guard tested `speaking` against a
                    // copy of itself, which is always equal, while the struct
                    // comparison beside it always differed for whoever was
                    // talking — so every event about a speaker forced a redraw.
                    let mut updated = updated;
                    updated.speaking = existing.speaking;
                    if *existing == updated {
                        return Ok(false);
                    }
                    *existing = updated;
                }
                None => status.participants.push(updated),
            }
            Ok(true)
        }

        "VOICE_STATE_DELETE" => {
            let Some(user_id) = event.data.pointer("/user/id").and_then(Value::as_str) else {
                return Ok(false);
            };

            // Being moved or disconnected by someone else sends no
            // VOICE_CHANNEL_SELECT — that only fires for a channel you picked
            // yourself. All that arrives is this channel saying you left it,
            // so ask Discord where you are now: a new channel, or none.
            if user_id == self_id {
                let left = status.channel_id.clone();
                reseed_after_leaving(client, subs, status, self_id, left.as_deref())?;
                return Ok(true);
            }

            let before = status.participants.len();
            status.participants.retain(|p| p.user_id != user_id);
            Ok(status.participants.len() != before)
        }

        "SPEAKING_START" | "SPEAKING_STOP" => {
            let speaking = event.name == "SPEAKING_START";
            let Some(user_id) = event.data.get("user_id").and_then(Value::as_str) else {
                return Ok(false);
            };
            match status.participant_mut(user_id) {
                Some(participant) if participant.speaking != speaking => {
                    participant.speaking = speaking;
                    Ok(true)
                }
                _ => Ok(false),
            }
        }

        "VOICE_SETTINGS_UPDATE" => {
            let mute = event
                .data
                .get("mute")
                .and_then(Value::as_bool)
                .unwrap_or(status.self_state.mute);
            let deaf = event
                .data
                .get("deaf")
                .and_then(Value::as_bool)
                .unwrap_or(status.self_state.deaf);

            if status.self_state.mute == mute && status.self_state.deaf == deaf {
                return Ok(false);
            }

            status.self_state.mute = mute;
            status.self_state.deaf = deaf;

            // Keep our own avatar badge consistent with the toggles.
            if let Some(me) = status.participant_mut(self_id) {
                me.self_mute = mute;
                me.self_deaf = deaf;
            }
            Ok(true)
        }

        _ => Ok(false),
    }
}

/// Build a `Participant` from an RPC voice-state object.
///
/// Shape confirmed against live events:
/// `{ nick, user: { id, username, global_name, avatar }, voice_state: { mute,
/// deaf, self_mute, self_deaf, suppress } }`.
fn participant_from(state: &Value, self_id: &str) -> Option<Participant> {
    let user = state.get("user")?;
    let user_id = user.get("id").and_then(Value::as_str)?.to_string();

    let display_name = state
        .get("nick")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| user.get("global_name").and_then(Value::as_str))
        .or_else(|| user.get("username").and_then(Value::as_str))
        .unwrap_or("Unknown")
        .to_string();

    let voice_state = state.get("voice_state");
    let flag = |name: &str| {
        voice_state
            .and_then(|vs| vs.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };

    Some(Participant {
        // `mute` and `volume` sit at the top level of an RPC voice state and
        // are this client's own settings for that person, not the server's.
        local_mute: state.get("mute").and_then(Value::as_bool).unwrap_or(false),
        volume: state.get("volume").and_then(Value::as_f64).unwrap_or(100.0) as f32,
        is_self: user_id == self_id,
        user_id,
        display_name,
        avatar_hash: user
            .get("avatar")
            .and_then(Value::as_str)
            .map(str::to_string),
        speaking: false,
        self_mute: flag("self_mute"),
        self_deaf: flag("self_deaf"),
        // `suppress` is a stage-channel listener, which reads as muted.
        server_mute: flag("mute") || flag("suppress"),
        server_deaf: flag("deaf"),
    })
}
