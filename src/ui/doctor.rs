//! `--doctor`: find out why nothing is showing.
//!
//! The widget is deliberately invisible when there is nothing to say — not in
//! a call, Discord not running, no credentials yet. That is right in normal
//! use and useless when something is wrong, because every failure looks
//! identical from the outside: an empty taskbar.
//!
//! This runs every check the app makes at startup, in order, and prints what
//! it found. It changes nothing: no OAuth prompt, no config rewrite, no
//! widget. It is safe to run at any time, including while the app is running.

use std::fmt::Write as _;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, GetSysColorBrush, COLOR_WINDOW, HFONT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{cache_dir, config_dir, config_path, Config};
use crate::provider::rpc::oauth;
use crate::provider::rpc::RpcClient;
use crate::ui::controls::{button, child, wide, Place};
use crate::ui::taskbar;

const CLASS_NAME: PCWSTR = w!("DiscordTaskbarDoctor");

const ID_COPY: usize = 1;
const ID_SAVE: usize = 2;
const ID_CLOSE: usize = 3;
const ID_REPORT: usize = 4;

/// Marks used down the left of the report, so it skims.
const OK: &str = "  ok  ";
const BAD: &str = " FAIL ";
const WARN: &str = " warn ";
const INFO: &str = "      ";

/// Gather the report and show it.
pub fn run() {
    let report = gather();
    show(&report);
}

/// Run every check and render the result as plain text.
///
/// Deliberately one long string rather than a structure: it exists to be read
/// by a person and pasted into a chat window.
pub fn gather() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Discord Taskbar {} — diagnostics", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "{}", "=".repeat(62));
    let _ = writeln!(out);

    section_app(&mut out);
    section_config(&mut out);
    section_discord(&mut out);
    section_taskbar(&mut out);

    let _ = writeln!(out);
    let _ = writeln!(out, "{}", "=".repeat(62));
    let _ = writeln!(
        out,
        "Send this whole report to whoever is helping you. It contains no\n\
         password and no message content; your client secret is not included."
    );
    out
}

fn section_app(out: &mut String) {
    let _ = writeln!(out, "APPLICATION");

    match std::env::current_exe() {
        Ok(path) => {
            let _ = writeln!(out, "{INFO}running from   {}", path.display());
            match std::fs::metadata(&path) {
                Ok(meta) => {
                    let _ = writeln!(out, "{INFO}size           {} bytes", meta.len());
                }
                Err(e) => {
                    let _ = writeln!(out, "{WARN}size           unreadable ({e})");
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "{BAD}running from   unknown ({e})");
        }
    }

    let _ = writeln!(
        out,
        "{INFO}architecture   {}",
        std::env::consts::ARCH
    );

    // If this build could not load, the process would not be here to say so —
    // but it is worth stating, because a missing redistributable is exactly
    // the failure that leaves no trace at all.
    let _ = writeln!(
        out,
        "{OK}C runtime      linked statically, no redistributable needed"
    );
    let _ = writeln!(out);
}

fn section_config(out: &mut String) {
    let _ = writeln!(out, "SETTINGS");
    let _ = writeln!(out, "{INFO}folder         {}", config_dir().display());

    let path = config_path();
    if !path.exists() {
        let _ = writeln!(out, "{BAD}config.json    missing — the app has never run");
        let _ = writeln!(out);
        return;
    }

    let (config, warning) = Config::load_or_create();
    match warning {
        Some(warning) => {
            let _ = writeln!(out, "{BAD}config.json    {warning}");
        }
        None => {
            let _ = writeln!(out, "{OK}config.json    readable");
        }
    }

    let id = config.discord.client_id.trim();
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
    let secret = config.discord.client_secret.trim();
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

    let _ = writeln!(out, "{INFO}avatar cache   {}", cache_dir().display());
    let _ = writeln!(out);
}

fn section_discord(out: &mut String) {
    let _ = writeln!(out, "DISCORD");

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
        return;
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

    let (config, _) = Config::load_or_create();
    let id = config.discord.client_id.trim();
    if id.is_empty() {
        let _ = writeln!(
            out,
            "{INFO}handshake      skipped, no client id to try it with"
        );
        let _ = writeln!(out);
        return;
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
}

fn section_taskbar(out: &mut String) {
    let _ = writeln!(out, "TASKBAR");

    let bars = taskbar::find_all();
    if bars.is_empty() {
        let _ = writeln!(
            out,
            "{BAD}Shell_TrayWnd  not found — explorer is not running, or the"
        );
        let _ = writeln!(
            out,
            "{INFO}               taskbar is not the classic kind the widget needs"
        );
        let _ = writeln!(out);
        return;
    }

    let _ = writeln!(out, "{OK}taskbars       {} found", bars.len());

    for bar in &bars {
        let _ = writeln!(
            out,
            "{INFO}  {} {}  {}x{} at ({},{})  {:?} edge  {} dpi",
            bar.monitor,
            if bar.is_primary { "(primary)" } else { "         " },
            bar.width(),
            bar.height(),
            bar.rect.left,
            bar.rect.top,
            bar.edge,
            bar.dpi,
        );
        let _ = writeln!(
            out,
            "{INFO}     task strip {}   notification area {}",
            present(bar.rebar_rect),
            present(bar.notify_rect),
        );
    }

    if bars.iter().all(|b| b.rebar_rect.is_none() && b.notify_rect.is_none()) {
        let _ = writeln!(
            out,
            "{WARN}               neither the task strip nor the notification area"
        );
        let _ = writeln!(
            out,
            "{INFO}               was found inside Shell_TrayWnd. That is what a"
        );
        let _ = writeln!(
            out,
            "{INFO}               stock Windows 11 taskbar looks like; the widget"
        );
        let _ = writeln!(
            out,
            "{INFO}               expects the classic one (StartAllBack, ExplorerPatcher)."
        );
    }
    let _ = writeln!(out);
}

fn present(rect: Option<RECT>) -> &'static str {
    if rect.is_some() {
        "found"
    } else {
        "MISSING"
    }
}

// ---------------------------------------------------------------- window ---

struct Doctor {
    report: String,
    font: HFONT,
}

thread_local! {
    static DOCTOR: std::cell::RefCell<Option<Doctor>> = const { std::cell::RefCell::new(None) };
}

fn show(report: &str) {
    if !register_class() {
        return;
    }

    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };

        let Ok(hwnd) = CreateWindowExW(
            WS_EX_APPWINDOW,
            CLASS_NAME,
            w!("Discord Taskbar — Diagnostics"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };

        let dpi = match GetDpiForWindow(hwnd) {
            0 => 96,
            d => d,
        };
        let scale = |v: i32| (v as i64 * dpi as i64 / 96) as i32;

        // Fixed pitch, because the report is laid out in columns.
        let font = mono_font(scale(9));

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: scale(720),
            bottom: scale(620),
        };
        let _ = AdjustWindowRectEx(&mut rect, WS_OVERLAPPEDWINDOW, false, WS_EX_APPWINDOW);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER,
        );

        DOCTOR.with(|cell| {
            *cell.borrow_mut() = Some(Doctor {
                report: report.to_string(),
                font,
            })
        });

        build(hwnd, report, &scale, font);

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).into() {
            if IsDialogMessageW(hwnd, &message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn build(hwnd: HWND, report: &str, scale: &impl Fn(i32) -> i32, font: HFONT) {
    let margin = scale(12);
    let button_h = scale(30);
    let button_w = scale(120);
    let width = scale(720);
    let height = scale(620);

    child(
        hwnd,
        w!("EDIT"),
        report,
        WINDOW_STYLE(
            WS_BORDER.0
                | WS_VSCROLL.0
                | WS_HSCROLL.0
                | WS_TABSTOP.0
                | ES_MULTILINE as u32
                | ES_READONLY as u32,
        ),
        ID_REPORT,
        Place {
            x: margin,
            y: margin,
            w: width - margin * 2,
            h: height - button_h - margin * 3,
        },
        font,
    );

    let y = height - button_h - margin;
    let gap = scale(8);
    for (index, (label, id)) in [
        ("Close", ID_CLOSE),
        ("Save to file\u{2026}", ID_SAVE),
        ("Copy", ID_COPY),
    ]
    .into_iter()
    .enumerate()
    {
        let slot = index as i32 + 1;
        button(
            hwnd,
            label,
            id,
            Place {
                x: width - margin - button_w * slot - gap * (slot - 1),
                y,
                w: button_w,
                h: button_h,
            },
            font,
        );
    }
}

fn mono_font(height: i32) -> HFONT {
    use windows::Win32::Graphics::Gdi::{
        CreateFontW, ANSI_CHARSET, CLIP_DEFAULT_PRECIS, FF_MODERN, FIXED_PITCH, FW_NORMAL,
        OUT_TT_PRECIS, PROOF_QUALITY,
    };
    unsafe {
        let name = wide("Consolas");
        CreateFontW(
            -height,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            ANSI_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            PROOF_QUALITY,
            (FIXED_PITCH.0 | FF_MODERN.0) as u32,
            PCWSTR(name.as_ptr()),
        )
    }
}

/// Put the report on the clipboard.
fn copy(hwnd: HWND, report: &str) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();

        let text = wide(report);
        let bytes = std::mem::size_of_val(&text[..]);
        if let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let target = GlobalLock(handle);
            if !target.is_null() {
                std::ptr::copy_nonoverlapping(text.as_ptr() as *const u8, target as *mut u8, bytes);
                let _ = GlobalUnlock(handle);
                // Ownership passes to the clipboard on success, so this must
                // not be freed here.
                let _ = SetClipboardData(13u32, Some(HANDLE(handle.0))); // CF_UNICODETEXT
            }
        }
        let _ = CloseClipboard();
    }
}

/// Write the report next to the config, where it is easy to find and attach.
fn save(hwnd: HWND, report: &str) {
    let path = config_dir().join("diagnostics.txt");
    let message = match std::fs::create_dir_all(config_dir())
        .and_then(|()| std::fs::write(&path, report.replace('\n', "\r\n")))
    {
        Ok(()) => format!("Saved to\n{}", path.display()),
        Err(error) => format!("Could not save:\n{error}"),
    };

    unsafe {
        let text = wide(&message);
        MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            w!("Diagnostics"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn register_class() -> bool {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return false;
        };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&class) != 0
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CTLCOLORSTATIC => LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize),

        WM_COMMAND => {
            let id = wparam.0 & 0xFFFF;
            let report = DOCTOR.with(|cell| {
                cell.borrow().as_ref().map(|d| d.report.clone())
            });
            match (id, report) {
                (ID_COPY, Some(report)) => copy(hwnd, &report),
                (ID_SAVE, Some(report)) => save(hwnd, &report),
                (ID_CLOSE, _) => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }

        WM_DESTROY => {
            DOCTOR.with(|cell| {
                if let Some(doctor) = cell.borrow_mut().take() {
                    let _ = DeleteObject(doctor.font.into());
                }
            });
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
