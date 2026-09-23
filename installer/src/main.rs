//! Setup for Discord Taskbar.
//!
//! One executable does both jobs. Installing copies the embedded application
//! binary into place and copies *itself* alongside it as `uninstall.exe`, so
//! the uninstaller is by construction the same program that knows every path
//! the install wrote.
//!
//! Arguments:
//!
//! * (none) — run the install wizard.
//! * `--uninstall` — stage a copy into the temp folder and hand over to it,
//!   because a program cannot delete the folder it is running from.
//! * `--uninstall --staged <folder>` — what the staged copy is invoked with;
//!   it removes `<folder>` for real.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod wizard;

use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use taskbar_widget::ui::controls::wide;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| arguments.iter().any(|a| a == flag);

    if !has("--uninstall") {
        wizard::run(wizard::Mode::Install);
        return;
    }

    if let Some(directory) = value_of(&arguments, "--staged") {
        wizard::run(wizard::Mode::Uninstall(PathBuf::from(directory)));
        return;
    }

    stage_and_exit();
}

fn value_of(arguments: &[String], flag: &str) -> Option<String> {
    let index = arguments.iter().position(|a| a == flag)?;
    arguments.get(index + 1).cloned()
}

/// Copy ourselves to the temp folder and re-launch from there.
///
/// Windows holds an executable's file open while it runs, so an uninstaller
/// sitting inside the program folder can never remove that folder. Running
/// from somewhere else is the standard way out; the stray copy left in
/// `%TEMP%` is a few hundred kilobytes and Disk Cleanup takes care of it.
fn stage_and_exit() {
    let Ok(me) = std::env::current_exe() else {
        return;
    };
    let Some(directory) = me.parent().map(PathBuf::from) else {
        return;
    };

    let staged = std::env::temp_dir().join("discord-taskbar-uninstall.exe");
    if actions::copy_self(&staged).is_err() {
        // Falling back to removing what we can from here is better than doing
        // nothing: everything except the program folder itself still goes.
        wizard::run(wizard::Mode::Uninstall(directory));
        return;
    }

    unsafe {
        let file = wide(&staged.to_string_lossy());
        let arguments = wide(&format!("--uninstall --staged \"{}\"", directory.display()));
        let _ = ShellExecuteW(
            None,
            PCWSTR::null(),
            PCWSTR(file.as_ptr()),
            PCWSTR(arguments.as_ptr()),
            None,
            SW_SHOWNORMAL,
        );
    }
}
