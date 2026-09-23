//! What the Discord integration contributes to the diagnostics window.
//!
//! The widget's own report covers the exe, the config file and the taskbar.
//! Everything here is about Discord specifically: the credentials, the OAuth
//! token, whether the desktop client is running, and — behind a button —
//! whether authorisation actually works.

use std::fmt::Write;

use taskbar_widget::config::{Config, Credentials};
use taskbar_widget::ui::doctor::{Action, Report, BAD, INFO, OK, WARN};

use super::provider::rpc::oauth;
use super::provider::rpc::{RpcClient, RpcError};

/// Everything the diagnostics window needs from this integration.
pub fn report() -> Report {
    Report {
        heading: "DISCORD".to_string(),
        local: local(),
        probe,
        action: Some(Action {
            label: "Try to sign in now".to_string(),
            announcement: format!(
                "

SIGN-IN TEST

                 {INFO}asking Discord to authorise. Switch to Discord and approve

                 {INFO}the prompt; it can open behind the main window.

"
            ),
            run: try_sign_in,
        }),
    }
}

/// The credentials and token, as written down. Fast; no network, no pipe.
fn local() -> String {
    let mut out = String::new();
    let out = &mut out;
    let (config, _) = Config::load();
    let creds = crate::settings::credentials(&config);
    let id = creds.client_id.trim();
    if id.is_empty() {
        let _ = writeln!(
            out,
            "{BAD}client id      not set — open Settings and paste it in"
        );
    } else if !id.chars().all(|c| c.is_ascii_digit()) {
        let _ = writeln!(
            out,
            "{BAD}client id      \"{id}\" is not all digits; that is not a client id"
        );
    } else if id.len() < 17 || id.len() > 20 {
        let _ = writeln!(
            out,
            "{WARN}client id      {id} ({} digits, expected 17-20)",
            id.len()
        );
    } else {
        let _ = writeln!(out, "{OK}client id      {id}");
    }

    // Never printed, only measured: the report is meant to be pasted around.
    let creds = crate::settings::credentials(&config);
    let secret = creds.client_secret.trim();
    if secret.is_empty() {
        let _ = writeln!(
            out,
            "{BAD}client secret  not set — open Settings and paste it in"
        );
    } else if secret.len() < 30 {
        let _ = writeln!(
            out,
            "{WARN}client secret  set, but only {} characters — is it truncated?",
            secret.len()
        );
    } else {
        let _ = writeln!(out, "{OK}client secret  set ({} characters)", secret.len());
    }

    let token = oauth::token_path();
    match oauth::load_token() {
        None if token.exists() => {
            let _ = writeln!(out, "{WARN}token.json     present but unreadable");
        }
        None => {
            let _ = writeln!(
                out,
                "{INFO}token.json     not yet written — normal until you approve\n\
                 {INFO}               the prompt in Discord for the first time"
            );
        }
        Some(stored) if stored.is_usable() => {
            let _ = writeln!(out, "{OK}token.json     valid, no prompt needed");
        }
        Some(stored) if !stored.covers_required_scopes() => {
            let _ = writeln!(
                out,
                "{WARN}token.json     missing a scope; Discord will prompt again"
            );
            let _ = writeln!(out, "{INFO}               has:  {}", stored.scope);
            let _ = writeln!(out, "{INFO}               needs: {}", oauth::SCOPES.join(" "));
        }
        Some(stored) => {
            let _ = writeln!(
                out,
                "{INFO}token.json     expired{}",
                if stored.can_refresh() {
                    ", will refresh by itself"
                } else {
                    "; Discord will prompt again"
                }
            );
        }
    }

    std::mem::take(out)
}

pub fn probe() -> String {
    let mut out = String::new();
    let out = &mut out;

    // Which pipes answer tells us whether the desktop client is up, without
    // needing to enumerate processes.
    let pipes: Vec<u32> = (0..10)
        .filter(|index| {
            std::fs::metadata(format!(r"\\.\pipe\discord-ipc-{index}")).is_ok()
        })
        .collect();

    if pipes.is_empty() {
        let _ = writeln!(
            out,
            "{BAD}ipc pipe       none found — the Discord desktop app is not"
        );
        let _ = writeln!(
            out,
            "{INFO}               running. The browser version cannot be read."
        );
        let _ = writeln!(out);
        return std::mem::take(out);
    }

    let _ = writeln!(
        out,
        "{OK}ipc pipe       discord-ipc-{} answering",
        pipes
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", discord-ipc-")
    );

    let (config, _) = Config::load();
    let creds = crate::settings::credentials(&config);
    let id = creds.client_id.trim();
    if id.is_empty() {
        let _ = writeln!(
            out,
            "{INFO}handshake      skipped, no client id to try it with"
        );
        let _ = writeln!(out);
        return std::mem::take(out);
    }

    // A handshake is enough to prove the client id: it is rejected before any
    // authorisation happens, so this never pops a prompt.
    match RpcClient::connect(id) {
        Ok(client) => {
            let _ = writeln!(out, "{OK}handshake      accepted, client id is valid");
            match client
                .ready_user
                .as_ref()
                .and_then(|user| user.get("username").and_then(|v| v.as_str()))
            {
                Some(name) => {
                    let _ = writeln!(out, "{OK}signed in as   {name}");
                    let _ = writeln!(
                        out,
                        "{INFO}               this account must be the one that OWNS the\n\
                         {INFO}               Discord application above, or authorisation\n\
                         {INFO}               will be refused"
                    );
                }
                None => {
                    let _ = writeln!(out, "{WARN}signed in as   unknown — no user in the READY frame");
                }
            }
        }
        Err(error) => {
            let _ = writeln!(out, "{BAD}handshake      rejected: {error}");
            let text = error.to_string();
            if text.contains("4000") || text.to_lowercase().contains("client id") {
                let _ = writeln!(
                    out,
                    "{INFO}               Discord does not recognise this client id.\n\
                     {INFO}               Check it against the OAuth2 page of YOUR OWN\n\
                     {INFO}               application — somebody else's will not work."
                );
            }
        }
    }
    let _ = writeln!(out);
    std::mem::take(out)
}

/// Run the real sign-in and report what happened.
///
/// The handshake in the report above proves only the client id. Authorisation
/// is a separate exchange that uses the client secret and the registered
/// redirect URI, and it is where a correctly-created-but-misconfigured
/// application actually fails — so the only way to diagnose it is to try it.
fn try_sign_in() -> String {
    let (config, _) = Config::load();
    let credentials = super::settings::credentials(&config);

    if !credentials.is_complete() {
        return format!("{BAD}cannot try     no client id or secret set

");
    }

    sign_in(&credentials)
}

fn sign_in(credentials: &Credentials) -> String {
    let mut client = match RpcClient::connect(&credentials.client_id) {
        Ok(client) => client,
        Err(error) => return format!("{BAD}connect        {error}\r\n"),
    };

    match oauth::login(&mut client, credentials, || {}) {
        Ok(token) => format!(
            "{OK}signed in      it worked; token saved\r\n\
             {INFO}scopes         {}\r\n\
             {INFO}               Start discord-taskbar.exe and join a voice\r\n\
             {INFO}               channel — it should appear now.\r\n",
            token.scope
        ),
        Err(error) => explain(&error),
    }
}

/// Turn an authorisation failure into something actionable.
fn explain(error: &RpcError) -> String {
    let text = error.to_string();
    let lower = text.to_lowercase();
    let mut out = format!("{BAD}sign-in        {text}\r\n");

    let advice = if lower.contains("redirect") || lower.contains("invalid_request") {
        Some(
            "Discord refused the token exchange. Almost always this means\n\
             http://localhost is not registered on your application. Open\n\
             the OAuth2 page, add it under Redirects, and Save Changes.",
        )
    } else if lower.contains("invalid_client") {
        Some(
            "Discord rejected the client secret. Reset it on the OAuth2\n\
             page, copy the new one, and paste it into Settings.",
        )
    } else if lower.contains("invalid_grant") {
        Some(
            "The authorisation code was refused. This is usually the\n\
             redirect URI differing from the registered one — it must be\n\
             exactly http://localhost, with no trailing slash.",
        )
    } else if lower.contains("denied") || lower.contains("4001") || lower.contains("cancel") {
        Some(
            "The prompt was dismissed rather than approved. Run this again\n\
             and press Authorize in Discord.",
        )
    } else if lower.contains("not running") || lower.contains("pipe") {
        Some("Discord closed midway through. Reopen it and try again.")
    } else {
        None
    };

    if let Some(advice) = advice {
        for line in advice.lines() {
            let _ = writeln!(out, "{INFO}               {}\r", line.trim());
        }
    }
    out
}
