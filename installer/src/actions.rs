//! Everything setup actually does to the machine.
//!
//! All of it is per-user: files under `%LOCALAPPDATA%`, registry values under
//! `HKEY_CURRENT_USER`, shortcuts in the user's own Start menu. Nothing needs
//! administrator rights and nothing touches another account, which is what
//! makes this safe to hand to a friend.
//!
//! Every step records what it did in a `Report`, so the last page of the
//! wizard can show exactly what happened rather than a bare "Success".

use std::path::{Path, PathBuf};

use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};
use windows::Win32::UI::Shell::{
    IShellLinkW, SHGetKnownFolderPath, ShellLink, FOLDERID_Desktop, FOLDERID_Programs,
    KF_FLAG_DEFAULT,
};

use taskbar_widget::ui::controls::wide;

/// The application binary, baked in at build time by `make-installer.sh`.
///
/// Embedding it is what makes setup a single file somebody can download and
/// run, with no archive to extract and no second download to find.
pub const PAYLOAD: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/payload/discord-taskbar.exe"
));

pub const APP_NAME: &str = "Discord Taskbar";
pub const EXE_NAME: &str = "discord-taskbar.exe";
pub const UNINSTALL_NAME: &str = "uninstall.exe";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\DiscordTaskbar";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "DiscordTaskbar";

/// A running commentary on what setup did, shown on the final page.
#[derive(Default)]
pub struct Report {
    pub lines: Vec<String>,
    pub failed: bool,
}

impl Report {
    pub fn ok(&mut self, text: impl Into<String>) {
        self.lines.push(format!("\u{2713}  {}", text.into()));
    }

    pub fn fail(&mut self, text: impl Into<String>) {
        self.failed = true;
        self.lines.push(format!("\u{2717}  {}", text.into()));
    }

    /// Record the outcome of one step in one line.
    pub fn step(&mut self, what: &str, result: Result<(), String>) {
        match result {
            Ok(()) => self.ok(what.to_string()),
            Err(error) => self.fail(format!("{what} — {error}")),
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\r\n")
    }
}

/// What the user chose on the options page.
pub struct Options {
    pub directory: PathBuf,
    pub run_at_signin: bool,
    pub start_menu: bool,
    pub desktop: bool,
}

/// `%LOCALAPPDATA%\Programs\Discord Taskbar` — the convention for a per-user
/// install, and where Windows already expects to find one.
pub fn default_directory() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    Path::new(&base).join("Programs").join(APP_NAME)
}

/// Copy in the files, write the registry entries, make the shortcuts.
pub fn install(options: &Options) -> Report {
    let mut report = Report::default();

    // An upgrade over a running copy would fail on a sharing violation, so
    // close the old one first — and not only one in the target folder. A copy
    // running from anywhere else holds the single-instance lock, so the new
    // one would start, find it, and quietly exit, leaving the old version
    // (and its old settings window) as the one on screen.
    if stop_running(None) {
        report.ok("Closed the running copy");
    }

    let exe = options.directory.join(EXE_NAME);
    let uninstaller = options.directory.join(UNINSTALL_NAME);

    report.step(
        &format!("Installed to {}", options.directory.display()),
        std::fs::create_dir_all(&options.directory)
            .map_err(|e| e.to_string())
            .and_then(|()| std::fs::write(&exe, PAYLOAD).map_err(|e| e.to_string())),
    );
    if report.failed {
        // Without the binary in place nothing else is worth attempting.
        return report;
    }

    // Setup doubles as its own uninstaller: it already knows every path it
    // wrote, so there is no second program to keep in step with this one.
    report.step("Created the uninstaller", copy_self(&uninstaller));

    report.step(
        "Added an entry to Installed apps",
        write_uninstall_entry(&options.directory, &exe, &uninstaller),
    );

    if options.run_at_signin {
        report.step("Set to start when you sign in", set_run_at_signin(&exe));
    } else {
        let _ = clear_run_at_signin();
    }

    if options.start_menu {
        report.step(
            "Added a Start menu shortcut",
            known_folder(&FOLDERID_Programs)
                .map(|dir| dir.join(format!("{APP_NAME}.lnk")))
                .ok_or_else(|| "could not locate the Start menu".to_string())
                .and_then(|path| shortcut(&path, &exe, &options.directory)),
        );
    }

    if options.desktop {
        report.step(
            "Added a desktop shortcut",
            known_folder(&FOLDERID_Desktop)
                .map(|dir| dir.join(format!("{APP_NAME}.lnk")))
                .ok_or_else(|| "could not locate the desktop".to_string())
                .and_then(|path| shortcut(&path, &exe, &options.directory)),
        );
    }

    report
}

/// Undo an install. `purge` additionally removes settings and cached avatars.
pub fn uninstall(directory: &Path, purge: bool) -> Report {
    let mut report = Report::default();

    // Only the copy being removed; one running from elsewhere is not ours to
    // close.
    if stop_running(Some(directory)) {
        report.ok("Closed the running copy");
    }

    let _ = clear_run_at_signin();
    report.ok("Removed the sign-in entry");

    for folder in [FOLDERID_Programs, FOLDERID_Desktop] {
        if let Some(path) = known_folder(&folder).map(|d| d.join(format!("{APP_NAME}.lnk"))) {
            let _ = std::fs::remove_file(path);
        }
    }
    report.ok("Removed the shortcuts");

    report.step("Removed the Installed apps entry", delete_key(UNINSTALL_KEY));

    if purge {
        let config = taskbar_widget::config::config_dir();
        let cache = taskbar_widget::config::cache_dir();
        let _ = std::fs::remove_dir_all(config);
        // `cache_dir()` is a subfolder; take its parent so nothing is left.
        let _ = std::fs::remove_dir_all(cache.parent().unwrap_or(&cache));
        report.ok("Removed settings and cached avatars");
    } else {
        report.ok("Kept your settings, in case you reinstall");
    }

    // The uninstaller runs from a copy in the temp folder precisely so the
    // program folder is no longer in use and can go in one piece.
    report.step(
        "Removed the program files",
        std::fs::remove_dir_all(directory).map_err(|e| e.to_string()),
    );

    report
}

/// Terminate every running copy of the app, or with `directory`, only the
/// ones running out of it.
///
/// Returns whether anything was actually closed.
fn stop_running(directory: Option<&Path>) -> bool {
    let target = EXE_NAME.to_lowercase();
    let mut stopped = false;

    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                if string_from(&entry.szExeFile).to_lowercase() == target
                    && kill_if_inside(entry.th32ProcessID, directory)
                {
                    stopped = true;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }

    stopped
}

fn kill_if_inside(pid: u32, directory: Option<&Path>) -> bool {
    unsafe {
        let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE;
        let Ok(process) = OpenProcess(access, false, pid) else {
            return false;
        };

        let mut buffer = [0u16; MAX_PATH as usize];
        let mut length = buffer.len() as u32;
        let image = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
        .is_ok()
        .then(|| String::from_utf16_lossy(&buffer[..length as usize]));

        let inside = match directory {
            Some(directory) => {
                let prefix = directory.to_string_lossy().to_lowercase();
                image
                    .map(|p| p.to_lowercase().starts_with(&prefix))
                    .unwrap_or(false)
            }
            None => true,
        };

        if inside {
            let _ = TerminateProcess(process, 0);
            // Wait for it to actually go, so the copy below does not race it.
            let _ = WaitForSingleObject(process, 4_000);
        }
        let _ = CloseHandle(process);
        inside
    }
}

/// Copy this executable to `destination`, which is how the uninstaller and
/// the staged temporary copy are both made.
pub fn copy_self(destination: &Path) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    if me == destination {
        return Ok(());
    }
    std::fs::copy(me, destination)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn write_uninstall_entry(directory: &Path, exe: &Path, uninstaller: &Path) -> Result<(), String> {
    let key = create_key(UNINSTALL_KEY)?;
    let size_kb = (PAYLOAD.len() / 1024) as u32;

    let result = (|| -> Result<(), String> {
        set_string(key, "DisplayName", APP_NAME)?;
        set_string(key, "DisplayVersion", VERSION)?;
        set_string(key, "Publisher", APP_NAME)?;
        set_string(key, "InstallLocation", &directory.to_string_lossy())?;
        set_string(key, "DisplayIcon", &exe.to_string_lossy())?;
        set_string(
            key,
            "UninstallString",
            &format!("\"{}\" --uninstall", uninstaller.display()),
        )?;
        // There is nothing to repair or reconfigure, so hide those buttons.
        set_dword(key, "NoModify", 1)?;
        set_dword(key, "NoRepair", 1)?;
        set_dword(key, "EstimatedSize", size_kb)?;
        Ok(())
    })();

    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

fn set_run_at_signin(exe: &Path) -> Result<(), String> {
    let key = create_key(RUN_KEY)?;
    let result = set_string(key, RUN_VALUE, &format!("\"{}\"", exe.display()));
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

fn clear_run_at_signin() -> Result<(), String> {
    let key = create_key(RUN_KEY)?;
    let name = wide(RUN_VALUE);
    unsafe {
        let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        let _ = RegCloseKey(key);
    }
    Ok(())
}

fn create_key(path: &str) -> Result<HKEY, String> {
    unsafe {
        let name = wide(path);
        let mut key = HKEY::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        );
        if status.is_ok() {
            Ok(key)
        } else {
            Err(format!("registry error {}", status.0))
        }
    }
}

fn delete_key(path: &str) -> Result<(), String> {
    unsafe {
        let name = wide(path);
        let status = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr()));
        if status.is_ok() {
            Ok(())
        } else {
            Err(format!("registry error {}", status.0))
        }
    }
}

fn set_string(key: HKEY, name: &str, value: &str) -> Result<(), String> {
    unsafe {
        let name = wide(name);
        let value = wide(value);
        let bytes =
            std::slice::from_raw_parts(value.as_ptr() as *const u8, std::mem::size_of_val(&value[..]));
        let status = RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes));
        if status.is_ok() {
            Ok(())
        } else {
            Err(format!("registry error {}", status.0))
        }
    }
}

fn set_dword(key: HKEY, name: &str, value: u32) -> Result<(), String> {
    unsafe {
        let name = wide(name);
        let bytes = value.to_le_bytes();
        let status = RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_DWORD, Some(&bytes));
        if status.is_ok() {
            Ok(())
        } else {
            Err(format!("registry error {}", status.0))
        }
    }
}

/// Write a `.lnk` pointing at `target`.
fn shortcut(path: &Path, target: &Path, working: &Path) -> Result<(), String> {
    unsafe {
        // Shortcuts are the one part of setup that needs COM. Initialising it
        // here rather than at startup keeps the failure local to this step.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let result = (|| -> Result<(), String> {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;

            let target = wide(&target.to_string_lossy());
            let working = wide(&working.to_string_lossy());
            let description = wide("Discord voice status in the taskbar");

            link.SetPath(PCWSTR(target.as_ptr()))
                .map_err(|e| e.message())?;
            link.SetWorkingDirectory(PCWSTR(working.as_ptr()))
                .map_err(|e| e.message())?;
            link.SetDescription(PCWSTR(description.as_ptr()))
                .map_err(|e| e.message())?;

            let file: IPersistFile = link.cast().map_err(|e| e.message())?;
            let path = wide(&path.to_string_lossy());
            file.Save(PCWSTR(path.as_ptr()), true)
                .map_err(|e| e.message())
        })();

        CoUninitialize();
        result
    }
}

/// Resolve a known folder, which is how the desktop is found when OneDrive
/// has redirected it away from `%USERPROFILE%\Desktop`.
fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok()?;
        CoTaskMemFree(Some(path.0 as *const _));
        Some(PathBuf::from(text))
    }
}

/// A NUL-terminated wide field out of a Win32 struct.
fn string_from(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}
