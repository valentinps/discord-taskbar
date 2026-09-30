//! Updating the app from its GitHub releases.
//!
//! A background thread asks GitHub for the latest release, and if it is newer
//! than this build, downloads the release's copy of the executable into a
//! staging folder. Nothing is replaced while the app is running: the tray icon
//! gets a badge and a menu row, and the swap happens either when that row is
//! clicked or, failing that, the next time the app starts.
//!
//! Replacing a running executable works on Windows as long as it is renamed
//! out of the way first rather than overwritten; the old copy is deleted on
//! the next start, once nothing has it open.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::http;
use crate::ui::Notifier;

/// Long enough after startup not to compete with everything else that starts
/// at sign-in.
const FIRST_CHECK: Duration = Duration::from_secs(20);
/// GitHub allows sixty unauthenticated requests an hour; this is four a day.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Where updates come from, and what this build is.
#[derive(Debug, Clone)]
pub struct Source {
    /// `owner/name` on GitHub.
    pub repo: &'static str,
    /// The release asset that is the application itself — not the installer.
    pub asset: &'static str,
    /// This build's version, normally `env!("CARGO_PKG_VERSION")`.
    pub version: &'static str,
    /// Only a copy running from here updates itself. A build run straight out
    /// of `target\release` replacing itself with the published one would be a
    /// baffling thing to happen in the middle of working on it.
    pub installed_dir: PathBuf,
}

impl Source {
    /// Whether this process is the installed copy.
    pub fn applies(&self) -> bool {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let (Some(here), Ok(installed)) = (
            exe.parent().and_then(|p| p.canonicalize().ok()),
            self.installed_dir.canonicalize(),
        ) else {
            return false;
        };
        here == installed
    }
}

/// A downloaded update, waiting to be installed.
#[derive(Debug, Clone)]
pub struct Ready {
    pub version: String,
    pub path: PathBuf,
}

/// The most recent download, for the UI thread to collect.
static READY: Mutex<Option<Ready>> = Mutex::new(None);

/// What the background thread has found, if anything.
pub fn ready() -> Option<Ready> {
    READY.lock().ok().and_then(|slot| slot.clone())
}

/// `%LOCALAPPDATA%\discord-taskbar\update`.
fn staging_dir() -> PathBuf {
    crate::config::cache_dir()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("update")
}

fn staged_path(version: &str) -> PathBuf {
    staging_dir().join(format!("discord-taskbar-{version}.exe"))
}

/// Check now and then every few hours, posting to `notifier` when an update
/// has been downloaded.
pub fn spawn(source: Source, notifier: Notifier) {
    let _ = std::thread::Builder::new()
        .name("updater".to_string())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK);
            loop {
                match check(&source) {
                    Ok(Some(ready)) => {
                        if let Ok(mut slot) = READY.lock() {
                            *slot = Some(ready);
                        }
                        notifier.notify();
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("discord-taskbar: update check: {error}"),
                }
                std::thread::sleep(CHECK_EVERY);
            }
        });
}

/// Look for a newer release, and download it if there is one.
fn check(source: &Source) -> Result<Option<Ready>, String> {
    let response = http::get_with_headers(
        "api.github.com",
        &format!("/repos/{}/releases/latest", source.repo),
        "Accept: application/vnd.github+json\r\n",
    )
    .map_err(|e| e.to_string())?;
    if response.status == 404 {
        // No release published yet.
        return Ok(None);
    }
    if !response.is_success() {
        return Err(format!("GitHub answered {}", response.status));
    }

    let release: Value =
        serde_json::from_slice(&response.body).map_err(|e| format!("bad release JSON: {e}"))?;
    let tag = release["tag_name"].as_str().unwrap_or_default();
    if !is_newer(tag, source.version) {
        return Ok(None);
    }
    let version = tag.trim_start_matches('v').to_string();

    let asset = release["assets"]
        .as_array()
        .and_then(|assets| assets.iter().find(|a| a["name"] == source.asset))
        .ok_or_else(|| format!("release {tag} has no {}", source.asset))?;
    let url = asset["browser_download_url"].as_str().unwrap_or_default();
    let size = asset["size"].as_u64().unwrap_or(0);

    let path = staged_path(&version);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == size) {
        return Ok(Some(Ready { version, path }));
    }

    let (host, url_path) = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .ok_or_else(|| format!("unexpected download URL {url}"))?;
    // GitHub redirects to its storage host; WinHTTP follows it.
    let download = http::get(host, &format!("/{url_path}")).map_err(|e| e.to_string())?;
    if !download.is_success() {
        return Err(format!("download answered {}", download.status));
    }
    // A truncated download, or an error page served with a 200, must never
    // end up as the executable that runs at sign-in.
    if download.body.len() as u64 != size || !download.body.starts_with(b"MZ") {
        return Err("downloaded file is not the release's executable".to_string());
    }

    std::fs::create_dir_all(staging_dir()).map_err(|e| e.to_string())?;
    let partial = path.with_extension("partial");
    std::fs::write(&partial, &download.body).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;

    Ok(Some(Ready { version, path }))
}

/// Swap the running executable for `ready` and start the new one.
///
/// The new copy is told to wait for this process to exit, the same as a
/// restart. The caller quits once this returns `Ok`.
pub fn install(ready: &Ready) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = exe.with_extension("old");

    // A previous update's leftover, if its first start could not delete it.
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("could not move the app aside: {e}"))?;
    if let Err(error) = std::fs::copy(&ready.path, &exe) {
        // Put things back rather than leave nothing to start at sign-in.
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("could not copy the update in: {error}"));
    }
    let _ = std::fs::remove_file(&ready.path);

    std::process::Command::new(&exe)
        .arg("--wait-for")
        .arg(std::process::id().to_string())
        .spawn()
        .map_err(|e| format!("could not start the updated app: {e}"))?;
    Ok(())
}

/// At startup: tidy up after an earlier update, and install one that was
/// downloaded but never applied.
///
/// Returns true when an update was installed and the updated copy started, in
/// which case this process should exit straight away.
pub fn at_startup(source: &Source) -> bool {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old"));
    }

    let Ok(entries) = std::fs::read_dir(staging_dir()) else {
        return false;
    };
    let mut newest: Option<Ready> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let Some(version) = name
            .strip_prefix("discord-taskbar-")
            .and_then(|rest| rest.strip_suffix(".exe"))
        else {
            continue;
        };
        if !is_newer(version, source.version) {
            // Already installed, or older: nothing to keep it for.
            let _ = std::fs::remove_file(&path);
            continue;
        }
        if newest.as_ref().is_none_or(|n| is_newer(version, &n.version)) {
            newest = Some(Ready {
                version: version.to_string(),
                path,
            });
        }
    }

    match newest {
        Some(ready) => install(&ready).is_ok(),
        None => false,
    }
}

/// `v1.2.3` against `1.2.0`: whether `candidate` is the later version.
fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.trim().trim_start_matches('v');
    // Ignore anything after the numbers, such as `-beta`.
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.2.0"));
        assert!(is_newer("1.0", "0.9.9"));
    }

    #[test]
    fn a_tag_that_is_not_a_version_is_never_newer() {
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
    }
}
