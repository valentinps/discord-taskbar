//! Entry point.
//!
//! A single named mutex keeps one instance per user session; a second launch
//! exits quietly rather than stacking widgets on the taskbar.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};

use taskbar_widget::config::Config;
use discord_integration::{self as discord, Discord};
use taskbar_widget::ui::integration::Integration;
use taskbar_widget::ui;

/// Give up rather than hang forever if the old process will not exit.
const RESTART_TIMEOUT_MS: u32 = 10_000;

fn main() {
    // `--demo` drives the interface from a synthetic call instead of Discord,
    // so UI work does not require being in a voice channel. It runs under its
    // own mutex so it can sit alongside the real instance.
    let demo = std::env::args().any(|arg| arg == "--demo");
    // `--settings` opens the settings window straight away, which is how the
    // installer hands over after copying the files.
    let settings = std::env::args().any(|arg| arg == "--settings");

    // `--doctor` reports why nothing is showing and exits. It runs before the
    // single-instance check on purpose: the usual reason for running it is
    // that something is wrong, and refusing to start because a copy is
    // already up would be exactly the wrong answer.
    if std::env::args().any(|arg| arg == "--doctor") {
        ui::doctor::run(discord::doctor::report());
        return;
    }

    // A restart launches the replacement before the old process has exited, so
    // the new one waits for it — otherwise the single-instance mutex below
    // would see the outgoing process and this one would quit immediately.
    if let Some(pid) = wait_for_argument() {
        wait_for_exit(pid);
    }

    if already_running(demo) {
        return;
    }

    let (config, warning) = Config::load_or_create();
    if let Some(warning) = &warning {
        eprintln!("discord-taskbar: {warning}");
    }

    // Choosing what the widget shows is the whole of this binary's job.
    // Everything below `ui::` would serve any other integration unchanged.
    let creds = discord::settings::credentials(&config);
    let mut integration = Discord::new(creds.clone(), demo);
    integration.apply_settings(&config.integration("discord"));
    let integration: Box<dyn Integration> = Box::new(integration);

    // Nothing works without a Discord application, so say so rather than
    // sitting there doing nothing. The settings window explains how.
    let needs_setup = !demo && !creds.is_complete();
    let setup_notice =
        needs_setup.then(|| "Set up Discord - see Settings".to_string());

    if let Err(error) = ui::host::run(
        config,
        integration,
        warning,
        setup_notice,
        needs_setup,
        settings,
    ) {
        eprintln!("discord-taskbar: {error}");
        std::process::exit(1);
    }
}

/// `--wait-for <pid>`, passed by a restart.
fn wait_for_argument() -> Option<u32> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--wait-for" {
            return args.next()?.parse().ok();
        }
    }
    None
}

fn wait_for_exit(pid: u32) {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else {
            // Already gone, which is the outcome we wanted anyway.
            return;
        };
        let _ = WaitForSingleObject(process, RESTART_TIMEOUT_MS);
        let _ = CloseHandle(process);
    }
}

fn already_running(demo: bool) -> bool {
    unsafe {
        let name = if demo {
            w!(r"Local\DiscordTaskbarSingleInstanceDemo")
        } else {
            w!(r"Local\DiscordTaskbarSingleInstance")
        };
        // Leaked deliberately: the handle must outlive main to hold the name.
        match CreateMutexW(None, true, name) {
            Ok(_) => GetLastError() == ERROR_ALREADY_EXISTS,
            Err(_) => false,
        }
    }
}
