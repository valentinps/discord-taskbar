//! Notification-area icon and its context menu.
//!
//! The widget itself is small and easy to miss, so the tray icon is the
//! reliable way to reach the app — quit it, re-authorize, or find the config.
//!
//! The icon is drawn at runtime with the same primitives as the widget, so it
//! is crisp at any DPI and adds nothing to the binary.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject, HBITMAP};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::assets::icons::{Glyph, IconFonts};
use crate::ui::integration::TrayItem;

use super::render::{Canvas, Color};

pub const TRAY_ID: u32 = 1;

/// Menu command identifiers.
pub const CMD_FOCUS_DISCORD: usize = 100;
pub const CMD_REAUTHORIZE: usize = 101;
pub const CMD_OPEN_CONFIG: usize = 102;
pub const CMD_SETTINGS: usize = 106;
pub const CMD_RECONNECT: usize = 103;
pub const CMD_RESTART: usize = 104;
pub const CMD_QUIT: usize = 105;
pub const CMD_DOCTOR: usize = 107;

/// One command per monitor in the "Show on" submenu, offset by index.
pub const CMD_MONITOR_BASE: usize = 300;
/// Selects every monitor at once.
pub const CMD_MONITOR_ALL: usize = 299;

/// Discord blurple, so the tray icon is recognisable at a glance.
///
/// Moves to the integration once it supplies its own branding; for now it is
/// the default handed to `Tray::new`.
pub const BRAND: Color = Color::rgb(0x58, 0x65, 0xF2);

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
}

impl Tray {
    /// `glyph` and `colour` are the picture in the notification area, so the
    /// integration decides what the widget looks like there.
    pub fn new(hwnd: HWND, callback_message: u32, glyph: Glyph, colour: Color) -> Option<Self> {
        let icon = build_icon(glyph, colour)?;

        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: callback_message,
            hIcon: icon,
            ..Default::default()
        };
        write_tip(&mut data.szTip, "Discord Taskbar");

        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data).as_bool() };
        if !added {
            unsafe {
                let _ = DestroyIcon(icon);
            }
            return None;
        }

        Some(Tray { hwnd, icon })
    }

    /// Update the hover tooltip, e.g. with the current channel.
    pub fn set_tooltip(&self, text: &str) {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            uFlags: NIF_TIP,
            ..Default::default()
        };
        write_tip(&mut data.szTip, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    /// Show the context menu at the cursor and return the chosen command.
    ///
    /// `monitors` is the display list with a flag for whether the widget is
    /// currently shown there — editing that here saves the user having to
    /// look up device names to put in the config file.
    /// Show the notification-area menu and block until something is chosen.
    ///
    /// The rows are handed in rather than built here: the widget contributes
    /// its own and the integration contributes the rest, and neither has to
    /// know about the other's.
    pub fn show_menu(&self, items: &[TrayItem]) -> Option<usize> {
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            // Submenus are owned by the parent once appended, so destroying
            // the root destroys them all.
            append(menu, items);
            let choice = track(menu, self.hwnd);
            let _ = DestroyMenu(menu);
            choice
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            let _ = DestroyIcon(self.icon);
        }
    }
}

fn write_tip(buffer: &mut [u16; 128], text: &str) {
    let encoded: Vec<u16> = text.encode_utf16().take(buffer.len() - 1).collect();
    buffer.fill(0);
    buffer[..encoded.len()].copy_from_slice(&encoded);
}

/// Draw a headphone glyph into a 32-bit bitmap and wrap it as an `HICON`.
/// Append rows to an already-created menu, recursing into submenus.
fn append(menu: HMENU, items: &[TrayItem]) {
    unsafe {
        for item in items {
            match item {
                TrayItem::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                TrayItem::Command {
                    id,
                    label,
                    checked,
                    enabled,
                } => {
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let text = wide(label);
                    let _ = AppendMenuW(menu, flags, *id, PCWSTR(text.as_ptr()));
                }
                TrayItem::Submenu { label, items } => {
                    let Ok(sub) = CreatePopupMenu() else { continue };
                    append(sub, items);
                    let text = wide(label);
                    let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(text.as_ptr()));
                }
            }
        }
    }
}

fn build_icon(glyph: Glyph, colour: Color) -> Option<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);

    let mut canvas = Canvas::new(size, size)?;
    canvas.clear();

    let mut fonts = IconFonts::new();
    fonts.draw(
        &mut canvas,
        glyph,
        0,
        0,
        size,
        colour,
        Color::TRANSPARENT,
    );

    let colour: HBITMAP = canvas.take_bitmap();

    unsafe {
        // A 1bpp mask is required even though the colour bitmap's alpha is what
        // actually gets used; an all-zero mask means "show every pixel".
        let mask = CreateBitmap(size, size, 1, 1, None);

        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: colour,
        };

        let icon = CreateIconIndirect(&info).ok();

        let _ = DeleteObject(mask.into());
        let _ = DeleteObject(colour.into());

        icon
    }
}

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
fn find_discord_window() -> Option<HWND> {
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

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Show a popup at the cursor and return the chosen command.
unsafe fn track(menu: HMENU, owner: HWND) -> Option<usize> {
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);

    // Required so the menu dismisses when the user clicks elsewhere.
    let _ = SetForegroundWindow(owner);

    let choice = TrackPopupMenuEx(
        menu,
        (TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY).0,
        cursor.x,
        cursor.y,
        owner,
        None,
    );

    match choice.0 {
        0 => None,
        command => Some(command as usize),
    }
}

/// Relaunch the app so it picks up an edited config, then quit.
///
/// The replacement is told to wait for this process id before it does anything,
/// because the single-instance mutex is still held until we actually exit — a
/// naive relaunch would just see itself as a duplicate and give up.
pub fn restart() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };

    let spawned = std::process::Command::new(exe)
        .arg("--wait-for")
        .arg(std::process::id().to_string())
        .spawn()
        .is_ok();

    if spawned {
        unsafe {
            PostQuitMessage(0);
        }
    }
}

/// Open a folder in Explorer.
pub fn open_folder(path: &std::path::Path) {
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            windows::core::w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

