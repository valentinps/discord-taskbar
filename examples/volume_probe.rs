//! Diagnostic: what volume does Discord actually report, and what does it
//! accept?
//!
//! The widget and Discord's own slider were showing different numbers, and
//! scaling differently. Both read the same field, so before changing any
//! arithmetic it is worth seeing the raw values.
//!
//! Prints every participant's volume exactly as it arrives over RPC. With
//! `--probe-max` it then walks a single user's volume upward to find the
//! ceiling Discord will actually keep — which is the number that matters for
//! anyone running a plugin that raises the 200% limit.
//!
//! Usage:
//!   volume_probe                      list what Discord reports
//!   volume_probe --probe-max <id>     find the real ceiling for one user
//!                                     (restores the original value after)

use serde_json::{json, Value};

use discord_taskbar::config::Config;
use discord_taskbar::provider::rpc::{oauth, RpcClient, RpcError};

fn main() {
    if let Err(e) = run() {
        eprintln!("[volume-probe] FAILED: {e}");
        std::process::exit(1);
    }
}

/// A `--name value` pair from the command line.
fn flag(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).cloned()
}

fn run() -> Result<(), RpcError> {
    let (config, _) = Config::load_or_create();
    let mut client = RpcClient::connect(&config.discord.client_id)?;
    let token = oauth::login(&mut client, &config.discord, || {
        println!("authorize in Discord...");
    })?;
    let _ = token;
    println!("authenticated");

    let channel = client.call("GET_SELECTED_VOICE_CHANNEL", json!({}))?;
    let states = match channel.get("voice_states").and_then(Value::as_array) {
        Some(states) if !states.is_empty() => states.clone(),
        _ => {
            println!("not in a voice channel; join one and run this again");
            return Ok(());
        }
    };

    println!();
    println!(
        "{:<22} {:<20} {:>10}  raw json for volume",
        "user", "id", "volume"
    );
    println!("{}", "-".repeat(78));

    for state in &states {
        let user = state.get("user");
        let name = user
            .and_then(|u| u.get("global_name").and_then(Value::as_str))
            .or_else(|| user.and_then(|u| u.get("username").and_then(Value::as_str)))
            .unwrap_or("?");
        let id = user
            .and_then(|u| u.get("id").and_then(Value::as_str))
            .unwrap_or("?");
        let raw = state.get("volume").cloned().unwrap_or(Value::Null);
        let parsed = state
            .get("volume")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        println!("{name:<22} {id:<20} {parsed:>10.3}  {raw}");
    }

    println!();
    println!("Compare these with the per-user sliders in Discord itself.");
    println!("If they differ, the widget is not the thing converting them.");

    // `--set <id> --value <n>` writes one exact volume and leaves it there, so
    // Discord's own slider can be read against a number we chose. Comparing
    // two readings passively cannot tell "the scales differ" apart from "the
    // value moved between the two looks".
    if let Some(user) = flag("--set") {
        let value: f64 = flag("--value").and_then(|v| v.parse().ok()).unwrap_or(150.0);
        let reply = client.call(
            "SET_USER_VOICE_SETTINGS",
            json!({ "user_id": user, "volume": value }),
        )?;
        let kept = reply
            .get("volume")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        println!();
        println!("asked Discord for {value}, it kept {kept}");
        println!("now read that user's slider in Discord and compare with {value}.");
        return Ok(());
    }

    if let Some(target) = flag("--probe-max") {
        probe_ceiling(&mut client, &target, &states)?;
    } else {
        println!();
        println!("--set <id> --value <n>   write one exact volume and stop");
        println!("--probe-max <id>         find the ceiling Discord will keep");
    }

    Ok(())
}

/// Set a user's volume to increasing values and read back what stuck.
///
/// Discord documents 200 as the maximum, but a client plugin can raise it.
/// Rather than trusting either number, this asks and reports what came back.
fn probe_ceiling(
    client: &mut RpcClient,
    user_id: &str,
    states: &[Value],
) -> Result<(), RpcError> {
    let original = states
        .iter()
        .find(|s| s.pointer("/user/id").and_then(Value::as_str) == Some(user_id))
        .and_then(|s| s.get("volume"))
        .and_then(Value::as_f64)
        .unwrap_or(100.0);

    println!();
    println!("probing the ceiling for {user_id} (currently {original})");
    println!("{:>10}  {:>12}", "asked for", "kept");
    println!("{}", "-".repeat(26));

    for asked in [50.0, 100.0, 150.0, 200.0, 250.0, 300.0, 400.0, 500.0] {
        let reply = client.call(
            "SET_USER_VOICE_SETTINGS",
            json!({ "user_id": user_id, "volume": asked }),
        )?;
        let kept = reply
            .get("volume")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        println!("{asked:>10.0}  {kept:>12.3}");
        std::thread::sleep(std::time::Duration::from_millis(120));
    }

    // Put it back; nobody wants a diagnostic that leaves them deafened.
    let _ = client.call(
        "SET_USER_VOICE_SETTINGS",
        json!({ "user_id": user_id, "volume": original }),
    )?;
    println!("restored to {original}");

    Ok(())
}
