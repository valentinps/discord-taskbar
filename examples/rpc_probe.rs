//! Phase 0 spike: prove the RPC route works end to end before any UI exists.
//!
//! Verifies, in order: the IPC pipe is reachable, the handshake succeeds, the
//! `rpc` + `rpc.voice.read` scopes are granted to us as the application owner,
//! and that voice/speaking events actually flow.
//!
//! Run with `cargo run --example rpc_probe`, then join a voice channel and
//! talk, mute, deafen, and move channels while watching the output.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use discord_taskbar::config::Config;
use discord_taskbar::integration::discord::provider::rpc::{events, oauth, Event, RpcClient, RpcError};

fn main() {
    match run() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("\n[probe] FAILED: {e}");
            explain(&e);
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), RpcError> {
    let (config, _) = Config::load_or_create();
    let creds = discord_taskbar::integration::discord::settings::credentials(&config);

    if !creds.is_complete() {
        print_setup_instructions();
        return Err(RpcError::Config(
            "client_id / client_secret not configured".to_string(),
        ));
    }

    log("connecting to Discord IPC...");
    let mut client = RpcClient::connect(&creds.client_id)?;

    let who = client
        .ready_user
        .as_ref()
        .and_then(|u| {
            u.get("username")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "<unknown>".to_string());
    log(&format!(
        "handshake OK on discord-ipc-{} as {}",
        client.pipe_index(),
        who
    ));

    log("authenticating...");
    let token = oauth::login(&mut client, &creds, || {
        log(">>> Discord should now be showing an authorization dialog. Click Authorize. <<<");
    })?;
    log(&format!("authenticated; granted scopes: {}", token.scope));

    let mut subs = events::Subscriptions::new();
    subs.subscribe_global(&mut client)?;
    log(&format!("subscribed to {:?}", events::GLOBAL_EVENTS));

    // Seed from current state so we are correct even if the user was already
    // sitting in a call before we started.
    match events::selected_voice_channel(&mut client)? {
        Some(channel) => {
            describe_channel(&mut client, &channel);
            if let Some(id) = channel.get("id").and_then(Value::as_str) {
                subs.set_channel(&mut client, Some(id))?;
                log(&format!("subscribed to channel events for {id}"));
            }
        }
        None => log("not currently in a voice channel"),
    }

    log("listening for events - join a call, talk, mute, deafen, switch channels. Ctrl+C to stop.");
    println!();

    loop {
        let event = client.next_event()?;
        print_event(&event);

        if event.name == "VOICE_CHANNEL_SELECT" {
            let channel_id = event
                .data
                .get("channel_id")
                .and_then(Value::as_str)
                .map(str::to_string);

            subs.set_channel(&mut client, channel_id.as_deref())?;

            match channel_id {
                Some(id) => {
                    log(&format!("re-scoped channel subscriptions to {id}"));
                    if let Some(channel) = events::selected_voice_channel(&mut client)? {
                        describe_channel(&mut client, &channel);
                    }
                }
                None => log("left voice; channel subscriptions torn down"),
            }
        }
    }
}

fn describe_channel(client: &mut RpcClient, channel: &Value) {
    let name = channel
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("<no name>");

    let guild_name = channel
        .get("guild_id")
        .and_then(Value::as_str)
        .and_then(|gid| events::guild(client, gid).ok())
        .and_then(|g| {
            g.get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Direct Message".to_string());

    log(&format!("in voice: {guild_name} / {name}"));

    if let Some(states) = channel.get("voice_states").and_then(Value::as_array) {
        log(&format!("{} participant(s):", states.len()));
        for state in states {
            let display = state
                .pointer("/nick")
                .and_then(Value::as_str)
                .or_else(|| state.pointer("/user/global_name").and_then(Value::as_str))
                .or_else(|| state.pointer("/user/username").and_then(Value::as_str))
                .unwrap_or("<unknown>");

            let avatar = state
                .pointer("/user/avatar")
                .and_then(Value::as_str)
                .unwrap_or("<none>");

            let self_mute = state
                .pointer("/voice_state/self_mute")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let self_deaf = state
                .pointer("/voice_state/self_deaf")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            println!(
                "         - {display:<24} avatar={avatar:<34} mute={self_mute:<5} deaf={self_deaf}"
            );
        }
    }
}

fn print_event(event: &Event) {
    // Keep the high-frequency events on one compact line; dump the rest whole
    // so we can see the exact field names Discord sends.
    match event.name.as_str() {
        "SPEAKING_START" | "SPEAKING_STOP" => {
            let user = event
                .data
                .get("user_id")
                .and_then(Value::as_str)
                .unwrap_or("?");
            log(&format!("{:<22} user_id={user}", event.name));
        }
        "VOICE_SETTINGS_UPDATE" => {
            let mute = event.data.get("mute").and_then(Value::as_bool);
            let deaf = event.data.get("deaf").and_then(Value::as_bool);
            log(&format!(
                "{:<22} mute={mute:?} deaf={deaf:?}",
                event.name
            ));
        }
        _ => {
            log(&format!("{:<22} {}", event.name, compact(&event.data)));
        }
    }
}

fn compact(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    if text.len() > 400 {
        format!("{}... [{} bytes]", &text[..400], text.len())
    } else {
        text
    }
}

fn log(message: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % 86400)
        .unwrap_or(0);
    println!(
        "[{:02}:{:02}:{:02}] {message}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    );
}

fn print_setup_instructions() {
    let path = discord_taskbar::config::config_path();
    println!(
        "
This probe needs a Discord application of your own.

  1. Go to https://discord.com/developers/applications and click 'New Application'.
     Name it anything - it is only ever used locally.
  2. Open the 'OAuth2' tab.
     - Copy the CLIENT ID.
     - Click 'Reset Secret' and copy the CLIENT SECRET.
     - Under Redirects, add exactly:  {redirect}
       and click 'Save Changes'.
  3. Paste both values into:
       {path}

     so the file reads:

       {{
         \"discord\": {{
           \"client_id\": \"...\",
           \"client_secret\": \"...\"
         }}
       }}

  4. Re-run: cargo run --example rpc_probe

The config file has been created for you with empty values.
",
        redirect = oauth::REDIRECT_URI,
        path = path.display()
    );
}

fn explain(error: &RpcError) {
    match error {
        RpcError::NotRunning(_) => {
            eprintln!("Discord does not appear to be running. Start it and try again.");
        }
        RpcError::Remote { code, message } => {
            eprintln!("Discord rejected the request (code {code}): {message}");
            match code {
                4006 => eprintln!(
                    "\nCode 4006 means the requested scopes were refused. This is the assumption \
                     the design rests on:\nthe 'rpc' scope is owner-only for unapproved apps, so \
                     the Discord account running the client must be\nthe same account that owns \
                     the application in the developer portal. Check that first."
                ),
                4007 => eprintln!("\nThe access token is invalid. Delete token.json and re-run."),
                4011 => eprintln!("\nYou dismissed the authorization dialog. Re-run and accept it."),
                _ => {}
            }
        }
        RpcError::Config(_) => {}
        _ => {}
    }
}
