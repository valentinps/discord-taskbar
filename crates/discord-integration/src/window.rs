//! Finding and raising the Discord desktop window.
//!
//! Clicking the server icon or the channel name is meant to take you to
//! Discord, which means locating a window that belongs to it. This lives with
//! the integration rather than in the widget's tray code, because "the app
//! this is about" is a different app for every integration.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Bring Discord's main window to the foreground.
///
/// Matching on the window class alone is not enough: `Chrome_WidgetWin_1` is
/// the class every Chromium app uses, so the first match is just as likely to
/// be a browser. The window has to be matched to a process actually called
/// Discord.
pub fn focus_discord() -> bool {
    let Some(hwnd) = find_discord_window() else {
        return false;
    };

    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }

        // Windows refuses SetForegroundWindow from a process that does not own
        // the foreground. Briefly sharing an input queue with whoever does is
        // the long-standing way around that.
        let foreground = GetForegroundWindow();
        let target_thread = GetWindowThreadProcessId(hwnd, None);
        let foreground_thread = if foreground.is_invalid() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let our_thread = GetCurrentThreadId();

        let mut attached = Vec::new();
        for thread in [foreground_thread, target_thread] {
            if thread != 0 && thread != our_thread && AttachThreadInput(our_thread, thread, true).as_bool() {
                attached.push(thread);
            }
        }

        let _ = BringWindowToTop(hwnd);
        let ok = SetForegroundWindow(hwnd).as_bool();

        for thread in attached {
            let _ = AttachThreadInput(our_thread, thread, false);
        }

        ok
    }
}

/// Find a visible, titled top-level window belonging to `Discord.exe`.
pub fn find_discord_window() -> Option<HWND> {
    struct Search {
        found: Option<HWND>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> windows::core::BOOL {
        unsafe {
            let search = &mut *(param.0 as *mut Search);

            if !IsWindowVisible(hwnd).as_bool() || GetWindowTextLengthW(hwnd) == 0 {
                return true.into();
            }

            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return true.into();
            }

            if process_is_discord(pid) {
                search.found = Some(hwnd);
                return false.into();
            }

            true.into()
        }
    }

    unsafe {
        let mut search = Search { found: None };
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
        search.found
    }
}

fn process_is_discord(pid: u32) -> bool {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };

        let mut buffer = [0u16; 512];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
        .is_ok();

        let _ = windows::Win32::Foundation::CloseHandle(process);
        if !ok {
            return false;
        }

        let path = String::from_utf16_lossy(&buffer[..length as usize]).to_ascii_lowercase();
        let name = path.rsplit(['\\', '/']).next().unwrap_or("");

        // Covers Discord, DiscordCanary, DiscordPTB and Vesktop.
        matches!(
            name,
            "discord.exe" | "discordcanary.exe" | "discordptb.exe" | "discorddevelopment.exe" | "vesktop.exe"
        )
    }
}

