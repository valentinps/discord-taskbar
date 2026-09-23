//! Configuration and on-disk paths.
//!
//! Everything user-tunable lives in `%APPDATA%\discord-taskbar\config.json`.
//! Fields carry `#[serde(default)]` so adding options later never invalidates
//! an existing config file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// The widget's own look. Nothing here is specific to what is being
    /// shown.
    pub appearance: crate::ui::theme::Appearance,
    /// One section per integration, keyed by its `id`. Opaque here: only the
    /// integration knows what its own settings mean, and keeping them as
    /// `Value` is what lets a new one be added without touching this file.
    pub integrations: BTreeMap<String, Value>,
}

impl Config {
    /// One integration's section, or an empty object if it has none yet.
    pub fn integration(&self, id: &str) -> Value {
        self.integrations
            .get(id)
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()))
    }
}

/// Settings that used to live under `appearance` before integrations had
/// sections of their own.
///
/// Listed rather than detected: `appearance` still has keys of its own, and
/// moving anything unrecognised would sweep up a typo along with the rest.
const MOVED_TO_DISCORD: &[&str] = &[
    "avatar_size",
    "avatar_overlap",
    "max_avatars",
    "sort_by_speaking",
    "show_guild_name",
    "show_guild_icon",
    "guild_icon_size",
    "show_self_icons",
    "clickable_self_icons",
    "show_divider",
    "show_leave_button",
    "middle_click",
    "scroll_volume_step",
    "volume_curve",
    "volume_boost_db",
    "max_volume",
];

/// Move an older config's Discord settings into `integrations.discord`.
///
/// Runs on the raw JSON before it is deserialised, because by then the keys
/// that moved have already been dropped on the floor by `serde(default)` — a
/// silent reset of every setting the user had chosen.
fn migrate(raw: &mut Value) {
    let Some(root) = raw.as_object_mut() else {
        return;
    };

    // Only ever run once: a config that already has the section is current.
    let already = root
        .get("integrations")
        .and_then(Value::as_object)
        .is_some_and(|m| m.contains_key("discord"));
    if already {
        return;
    }

    let mut discord = serde_json::Map::new();

    // Credentials were a top-level `discord` object of their own.
    if let Some(Value::Object(creds)) = root.remove("discord") {
        for (key, value) in creds {
            discord.insert(key, value);
        }
    }

    if let Some(Value::Object(appearance)) = root.get_mut("appearance") {
        for key in MOVED_TO_DISCORD {
            if let Some(value) = appearance.remove(*key) {
                discord.insert((*key).to_string(), value);
            }
        }
    }

    if discord.is_empty() {
        return;
    }

    let integrations = root
        .entry("integrations")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(map) = integrations.as_object_mut() {
        map.insert("discord".to_string(), Value::Object(discord));
    }
}

/// Parse config text, migrating an older layout on the way through.
fn parse(text: &str) -> Result<Config, serde_json::Error> {
    let mut raw: Value = serde_json::from_str(text)?;
    migrate(&mut raw);
    serde_json::from_value(raw)
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

        match parse(&text) {
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

    /// Read the config without writing anything back.
    ///
    /// `load_or_create` rewrites the file so every field is visible, which is
    /// right at startup and wrong for a diagnostic: `--doctor` says in as many
    /// words that it changes nothing, and it may well be run while the app is
    /// up. Errors come back as a warning string, the same shape as
    /// `load_or_create`, so callers report them identically.
    pub fn load() -> (Self, Option<String>) {
        let path = config_path();
        if !path.exists() {
            return (Config::default(), None);
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

        match parse(&text) {
            Ok(config) => (config, None),
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
