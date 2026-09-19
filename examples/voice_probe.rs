//! Diagnostic: time `SET_VOICE_SETTINGS` end to end.
//!
//! Toggling mute from the widget was reported as taking seconds, and sometimes
//! not working at all. This measures the two things that could explain that:
//! how long Discord takes to acknowledge the command, and how long until the
//! confirming `VOICE_SETTINGS_UPDATE` event arrives.
//!
//! Run it while Discord is open (you do not need to be in a call).

use std::time::Instant;

use serde_json::{json, Value};

use discord_taskbar::config::Config;
use discord_taskbar::provider::rpc::{events, oauth, RpcClient, RpcError};

fn main() {
    if let Err(e) = run() {
        eprintln!("[voice-probe] FAILED: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), RpcError> {
    let (config, _) = Config::load_or_create();

    let mut client = RpcClient::connect(&config.discord.client_id)?;
    oauth::login(&mut client, &config.discord, || {
        println!("authorize in Discord...");
    })?;
    println!("authenticated");

    client.subscribe("VOICE_SETTINGS_UPDATE", Value::Null)?;

    let settings = events::voice_settings(&mut client)?;
    println!(
        "initial: mute={:?} deaf={:?}",
        settings.get("mute").and_then(Value::as_bool),
        settings.get("deaf").and_then(Value::as_bool)
    );
    println!();

    // Alternate mute on/off a few times and time each leg.
    for round in 0..3 {
        for mute in [true, false] {
            let label = if mute { "mute  " } else { "unmute" };
            let start = Instant::now();

            // Time the command's own acknowledgement.
            let result = client.call("SET_VOICE_SETTINGS", json!({ "mute": mute }));
            let acked = start.elapsed();

            match &result {
                Ok(data) => {
                    println!(
                        "round {round} {label}: ACK in {:>7.1}ms  -> mute={:?} deaf={:?}",
                        acked.as_secs_f64() * 1000.0,
                        data.get("mute").and_then(Value::as_bool),
                        data.get("deaf").and_then(Value::as_bool),
                    );
                }
                Err(e) => {
                    println!(
                        "round {round} {label}: ERROR after {:>7.1}ms -> {e}",
                        acked.as_secs_f64() * 1000.0
                    );
                }
            }

            // Then wait for the broadcast that the widget actually reacts to.
            let waited = wait_for_settings_event(&mut client, mute, start)?;
            match waited {
                Some(delay) => println!(
                    "                 VOICE_SETTINGS_UPDATE after {:>7.1}ms",
                    delay * 1000.0
                ),
                None => println!("                 no matching VOICE_SETTINGS_UPDATE within 5s"),
            }
            println!();
        }
    }

    Ok(())
}

/// Read events until one reports the expected mute state, or 5 s pass.
fn wait_for_settings_event(
    client: &mut RpcClient,
    expected_mute: bool,
    start: Instant,
) -> Result<Option<f64>, RpcError> {
    loop {
        if start.elapsed().as_secs_f64() > 5.0 {
            return Ok(None);
        }

        let event = client.next_event()?;
        if event.name != "VOICE_SETTINGS_UPDATE" {
            continue;
        }

        let mute = event.data.get("mute").and_then(Value::as_bool);
        if mute == Some(expected_mute) {
            return Ok(Some(start.elapsed().as_secs_f64()));
        }
        println!(
            "                 (interim VOICE_SETTINGS_UPDATE mute={mute:?} at {:>7.1}ms)",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}
