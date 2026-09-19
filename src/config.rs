//! Configuration and on-disk paths.
//!
//! Everything user-tunable lives in `%APPDATA%\discord-taskbar\config.json`.
//! Fields carry `#[serde(default)]` so adding options later never invalidates
//! an existing config file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `%APPDATA%\discord-taskbar` — config and OAuth token.
pub fn config_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    Path::new(&base).join("discord-taskbar")
}

/// `%LOCALAPPDATA%\discord-taskbar\cache` — avatar bitmaps.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    Path::new(&base).join("discord-taskbar").join("cache")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Credentials {
    /// Application ID from <https://discord.com/developers/applications>.
    pub client_id: String,
    /// OAuth2 client secret for the same application.
    pub client_secret: String,
}

impl Credentials {
    pub fn is_complete(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub discord: Credentials,
    pub appearance: crate::ui::theme::Appearance,
}

/// Decode config bytes as UTF-8, tolerating a byte-order mark.
///
/// Windows text editors — Notepad and PowerShell's `Set-Content -Encoding utf8`
/// among them — happily prepend a BOM, and `serde_json` rejects it with a
/// baffling "expected value at line 1 column 1".
fn decode(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(body).into_owned()
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config i/o error: {e}"),
            ConfigError::Parse(e) => write!(f, "config parse error: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Load the config, creating it on first run.
    ///
    /// Returns the config plus a warning to show the user if the file could
    /// not be read. A broken config must not be fatal: the release build has
    /// no console, so exiting on a parse error means the app simply vanishes
    /// with no indication why. Instead it starts on defaults and says so.
    ///
    /// A file that failed to parse is deliberately *not* rewritten — that
    /// would overwrite the user's credentials with blanks over a stray comma.
    pub fn load_or_create() -> (Self, Option<String>) {
        let path = config_path();

        if !path.exists() {
            let config = Config::default();
            let warning = config
                .save()
                .err()
                .map(|e| format!("Could not create config.json: {e}"));
            return (config, warning);
        }

        let text = match std::fs::read(&path) {
            Ok(bytes) => decode(&bytes),
            Err(e) => {
                return (
                    Config::default(),
                    Some(format!("Could not read config.json: {e}")),
                )
            }
        };

        match serde_json::from_str::<Config>(&text) {
            Ok(config) => {
                // Rewrite so every field is present in the file. Without this,
                // options that fall back to their default never appear, and an
                // option you cannot see is an option you cannot change.
                let warning = config
                    .save()
                    .err()
                    .map(|e| format!("Could not write config.json: {e}"));
                (config, warning)
            }
            Err(e) => (
                Config::default(),
                Some(format!("config.json is invalid ({e}); using defaults")),
            ),
        }
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).map_err(ConfigError::Io)?;
        let text = serde_json::to_string_pretty(self).map_err(ConfigError::Parse)?;
        std::fs::write(config_path(), text).map_err(ConfigError::Io)
    }
}
