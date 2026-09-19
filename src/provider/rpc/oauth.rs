//! OAuth2 for local RPC.
//!
//! Sequence on a cold start: `AUTHORIZE` over the pipe pops a consent dialog in
//! Discord and hands back a code; the code is exchanged for a token over HTTPS;
//! `AUTHENTICATE` then unlocks the voice commands. The token is cached so the
//! consent dialog appears exactly once.
//!
//! Note on scopes: for applications Discord has not approved, `rpc` is
//! restricted to the application *owner*. That is fine here — the account
//! running this app owns the application it authenticates against.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{RpcClient, RpcError, Result};
use crate::config::{config_dir, Credentials};
use crate::http;

pub const API_HOST: &str = "discord.com";
pub const TOKEN_PATH: &str = "/api/oauth2/token";

/// `rpc` gives voice channel/state commands, `rpc.voice.read` the speaking and
/// voice-settings events, and `rpc.voice.write` the ability to set mute and
/// deafen — which is what makes the widget's icons clickable.
///
/// Note that Discord allows only one RPC client to modify voice settings at a
/// time; whoever writes first locks the others out until it disconnects.
pub const SCOPES: &[&str] = &["rpc", "rpc.voice.read", "rpc.voice.write"];

/// Never actually visited — RPC returns the code over the pipe — but it must
/// match a redirect URI registered on the application.
pub const REDIRECT_URI: &str = "http://localhost";

/// Refresh this many seconds before actual expiry.
const EXPIRY_MARGIN_SECS: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StoredToken {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
    pub scope: String,
}

impl StoredToken {
    pub fn is_usable(&self) -> bool {
        !self.access_token.is_empty()
            && self.expires_at > now_secs() + EXPIRY_MARGIN_SECS
            && self.covers_required_scopes()
    }

    /// A token cached before a scope was added is not good enough any more, so
    /// adding one re-prompts instead of silently losing the new capability.
    pub fn covers_required_scopes(&self) -> bool {
        SCOPES
            .iter()
            .all(|needed| self.scope.split_whitespace().any(|have| have == *needed))
    }

    pub fn can_refresh(&self) -> bool {
        !self.refresh_token.is_empty()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn token_path() -> PathBuf {
    config_dir().join("token.json")
}

pub fn load_token() -> Option<StoredToken> {
    let text = std::fs::read_to_string(token_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_token(token: &StoredToken) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    let text = serde_json::to_string_pretty(token)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(token_path(), text)
}

pub fn clear_token() {
    let _ = std::fs::remove_file(token_path());
}

fn parse_token_response(body: &str) -> Result<StoredToken> {
    let value: Value = serde_json::from_str(body)
        .map_err(|e| RpcError::Protocol(format!("token response was not JSON: {e}")))?;

    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RpcError::Protocol(format!("token response had no access_token: {value}"))
        })?
        .to_string();

    let expires_in = value
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(604_800);

    Ok(StoredToken {
        access_token,
        refresh_token: value
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        expires_at: now_secs() + expires_in,
        scope: value
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Exchange an authorization code for an access token.
pub fn exchange_code(creds: &Credentials, code: &str) -> Result<StoredToken> {
    let response = http::post_form(
        API_HOST,
        TOKEN_PATH,
        &[
            ("client_id", creds.client_id.as_str()),
            ("client_secret", creds.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
        ],
    )?;

    if !response.is_success() {
        return Err(RpcError::Protocol(format!(
            "token exchange failed with HTTP {}: {}",
            response.status,
            response.body_string()
        )));
    }

    parse_token_response(&response.body_string())
}

/// Trade a refresh token for a fresh access token.
pub fn refresh_token(creds: &Credentials, refresh: &str) -> Result<StoredToken> {
    let response = http::post_form(
        API_HOST,
        TOKEN_PATH,
        &[
            ("client_id", creds.client_id.as_str()),
            ("client_secret", creds.client_secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
        ],
    )?;

    if !response.is_success() {
        return Err(RpcError::Protocol(format!(
            "token refresh failed with HTTP {}: {}",
            response.status,
            response.body_string()
        )));
    }

    parse_token_response(&response.body_string())
}

/// Ask Discord to show the consent dialog and return the resulting code.
/// Blocks until the user accepts or dismisses it.
pub fn authorize(client: &mut RpcClient, creds: &Credentials) -> Result<String> {
    let data = client.call(
        "AUTHORIZE",
        json!({
            "client_id": creds.client_id,
            "scopes": SCOPES,
        }),
    )?;

    data.get("code")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RpcError::Protocol(format!("AUTHORIZE returned no code: {data}")))
}

/// Present an access token to the RPC connection.
pub fn authenticate(client: &mut RpcClient, access_token: &str) -> Result<Value> {
    client.call("AUTHENTICATE", json!({ "access_token": access_token }))
}

/// Full login: reuse a cached token when possible, refresh it when stale, and
/// fall back to the consent dialog only when there is nothing usable.
///
/// `on_prompt` runs immediately before the consent dialog appears, so a caller
/// can tell the user to go look at Discord.
pub fn login(
    client: &mut RpcClient,
    creds: &Credentials,
    on_prompt: impl FnOnce(),
) -> Result<StoredToken> {
    if !creds.is_complete() {
        return Err(RpcError::Config(
            "client_id and client_secret are not set in config.json".to_string(),
        ));
    }

    if let Some(cached) = load_token() {
        if cached.is_usable() && authenticate(client, &cached.access_token).is_ok() {
            return Ok(cached);
        }

        if cached.can_refresh() && cached.covers_required_scopes() {
            if let Ok(refreshed) = refresh_token(creds, &cached.refresh_token) {
                if authenticate(client, &refreshed.access_token).is_ok() {
                    let _ = save_token(&refreshed);
                    return Ok(refreshed);
                }
            }
        }

        // Cached credentials are no longer good for anything.
        clear_token();
    }

    on_prompt();
    let code = authorize(client, creds)?;
    let token = exchange_code(creds, &code)?;
    authenticate(client, &token.access_token)?;
    let _ = save_token(&token);
    Ok(token)
}
