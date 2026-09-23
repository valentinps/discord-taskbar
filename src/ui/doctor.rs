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

use crate::config::{cache_dir, config_dir, config_path, Config, Credentials};
use crate::integration::discord::provider::rpc::oauth;
use crate::integration::discord::provider::rpc::{RpcClient, RpcError};
use crate::ui::controls::{button, child, wide, Place};
use crate::ui::taskbar;

const CLASS_NAME: PCWSTR = w!("DiscordTaskbarDoctor");

const ID_COPY: usize = 1;
const ID_SAVE: usize = 2;
const ID_CLOSE: usize = 3;
const ID_REPORT: usize = 4;
const ID_LOGIN: usize = 5;

/// The sign-in test finished; `wparam` carries a boxed `String`.
const WM_APP_RESULT: u32 = WM_APP + 1;
/// The Discord section is ready; `wparam` carries a boxed `String`.
const WM_APP_DISCOVERED: u32 = WM_APP + 2;

/// Marks used down the left of the report, so it skims.
const OK: &str = "  ok  ";
const BAD: &str = " FAIL ";
const WARN: &str = " warn ";
const INFO: &str = "      ";

/// Gather the local checks, show them, then fill in the Discord ones.
pub fn run() {
    show(&gather_local());
}

/// Everything that can be answered without talking to anything.
///
/// Deliberately one long string rather than a structure: it exists to be read
/// by a person and pasted into a chat window.
///
/// The Discord section is *not* here. Reaching Discord means a named-pipe
/// connect and a handshake that waits for a reply, and a tool whose whole job
/// is to explain a stuck program must never be the thing that hangs. Doing it
/// up front would have meant no window at all while it waited.
pub fn gather_local() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Discord Taskbar {} — diagnostics", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "{}", "=".repeat(62));
    let _ = writeln!(out);

    section_app(&mut out);
    section_config(&mut out);
    section_taskbar(&mut out);

    let _ = writeln!(out, "DISCORD");
    let _ = writeln!(out, "{INFO}checking...");
    out
}

/// Convert to CRLF, idempotently.
///
/// The report is assembled with `writeln!`, which emits a bare newline. A
/// Win32 EDIT control does not treat that as a line break at all - it renders
/// the whole report as a single very long line - and nor does Notepad. Every
/// point where this text leaves the module goes through here.
fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// The closing note, appended once the Discord section has landed.
fn footer() -> String {
    format!(
        "\r\n{}\r\nSend this whole report to whoever is helping you. It contains no\r\n\
         password and no message content; your client secret is not included.\r\n",
        "=".repeat(62)
    )
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

    // The single most useful line in the report. Every other check can pass
    // on a machine where the widget has simply never been started, and from
    // the outside that looks exactly like a broken install.
    if app_is_running() {
        let _ = writeln!(out, "{OK}widget         running");
    } else {
        let _ = writeln!(
            out,
            "{BAD}widget         NOT RUNNING — nothing will ever appear in the"
        );
        let _ = writeln!(
            out,
            "{INFO}               taskbar until it is started. Launch\n\
             {INFO}               discord-taskbar.exe from the install folder,\n\
             {INFO}               or sign out and back in if you asked setup to\n\
             {INFO}               start it automatically."
        );
    }
    let _ = writeln!(out);
}

/// Whether a copy of the widget is up, by probing the single-instance mutex.
///
/// Cheaper and more exact than enumerating processes: it is the very object
/// that decides whether a second launch is allowed to proceed, so it cannot
/// disagree with the app about what "already running" means.
fn app_is_running() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_ACCESS_RIGHTS};

    // SYNCHRONIZE. Opening for anything more would fail against a mutex
    // created by another session, and all we need is to know it exists.
    const SYNCHRONIZE: SYNCHRONIZATION_ACCESS_RIGHTS = SYNCHRONIZATION_ACCESS_RIGHTS(0x0010_0000);

    unsafe {
        match OpenMutexW(SYNCHRONIZE, false, w!(r"Local\DiscordTaskbarSingleInstance")) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
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

    let (config, warning) = Config::load();
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

    let (config, _) = Config::load();
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

        // Now that there is something on screen, go and ask Discord.
        probe_discord(hwnd);

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
        &to_crlf(report),
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

    // Laid out right to left, because the rightmost button is the one with a
    // fixed home and the widths differ.
    let y = height - button_h - margin;
    let gap = scale(8);
    let mut right = width - margin;
    for (label, id, w) in [
        ("Close", ID_CLOSE, button_w),
        ("Save to file\u{2026}", ID_SAVE, button_w),
        ("Copy", ID_COPY, button_w),
        ("Try to sign in now", ID_LOGIN, scale(170)),
    ] {
        button(
            hwnd,
            label,
            id,
            Place {
                x: right - w,
                y,
                w,
                h: button_h,
            },
            font,
        );
        right -= w + gap;
    }
}

/// Fill in the Discord section from a worker thread.
///
/// Replaces the "checking..." placeholder rather than appending, so the
/// finished report reads as though it had been gathered in one go.
fn probe_discord(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        let mut text = String::new();
        section_discord(&mut text);
        text.push_str(&footer());

        let boxed = Box::into_raw(Box::new(text)) as usize;
        unsafe {
            let _ = PostMessageW(
                Some(HWND(target as *mut std::ffi::c_void)),
                WM_APP_DISCOVERED,
                WPARAM(boxed),
                LPARAM(0),
            );
        }
    });
}

/// Run the real sign-in, on a worker thread, and append what happened.
///
/// The handshake in the report above proves only the client id. Authorisation
/// is a separate exchange that uses the client secret and the registered
/// redirect URI, and it is where a correctly-created-but-misconfigured
/// application actually fails — so the only way to diagnose it is to try it.
fn try_sign_in(hwnd: HWND) {
    let (config, _) = Config::load();
    let credentials = config.discord.clone();

    if !credentials.is_complete() {
        append(
            hwnd,
            &format!("\r\nSIGN-IN TEST\r\n{BAD}cannot try     no client id or secret set\r\n"),
        );
        return;
    }

    unsafe {
        if let Ok(control) = GetDlgItem(Some(hwnd), ID_LOGIN as i32) {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(control, false);
        }
    }

    append(
        hwnd,
        &format!(
            "\r\nSIGN-IN TEST\r\n\
             {INFO}asking Discord to authorise. Switch to Discord and approve\r\n\
             {INFO}the prompt; it can open behind the main window.\r\n"
        ),
    );

    // HWND is not Send, so carry the raw value across.
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        let text = sign_in(&credentials);
        let boxed = Box::into_raw(Box::new(text)) as usize;
        unsafe {
            let _ = PostMessageW(
                Some(HWND(target as *mut std::ffi::c_void)),
                WM_APP_RESULT,
                WPARAM(boxed),
                LPARAM(0),
            );
        }
    });
}

fn sign_in(credentials: &Credentials) -> String {
    let mut client = match RpcClient::connect(&credentials.client_id) {
        Ok(client) => client,
        Err(error) => return format!("{BAD}connect        {error}\r\n"),
    };

    match oauth::login(&mut client, credentials, || {}) {
        Ok(token) => format!(
            "{OK}signed in      it worked; token saved\r\n\
             {INFO}scopes         {}\r\n\
             {INFO}               Start discord-taskbar.exe and join a voice\r\n\
             {INFO}               channel — it should appear now.\r\n",
            token.scope
        ),
        Err(error) => explain(&error),
    }
}

/// Turn an authorisation failure into something actionable.
fn explain(error: &RpcError) -> String {
    let text = error.to_string();
    let lower = text.to_lowercase();
    let mut out = format!("{BAD}sign-in        {text}\r\n");

    let advice = if lower.contains("redirect") || lower.contains("invalid_request") {
        Some(
            "Discord refused the token exchange. Almost always this means\n\
             http://localhost is not registered on your application. Open\n\
             the OAuth2 page, add it under Redirects, and Save Changes.",
        )
    } else if lower.contains("invalid_client") {
        Some(
            "Discord rejected the client secret. Reset it on the OAuth2\n\
             page, copy the new one, and paste it into Settings.",
        )
    } else if lower.contains("invalid_grant") {
        Some(
            "The authorisation code was refused. This is usually the\n\
             redirect URI differing from the registered one — it must be\n\
             exactly http://localhost, with no trailing slash.",
        )
    } else if lower.contains("denied") || lower.contains("4001") || lower.contains("cancel") {
        Some(
            "The prompt was dismissed rather than approved. Run this again\n\
             and press Authorize in Discord.",
        )
    } else if lower.contains("not running") || lower.contains("pipe") {
        Some("Discord closed midway through. Reopen it and try again.")
    } else {
        None
    };

    if let Some(advice) = advice {
        for line in advice.lines() {
            let _ = writeln!(out, "{INFO}               {}\r", line.trim());
        }
    }
    out
}

/// Swap the "checking..." line for the finished Discord section.
fn replace_placeholder(hwnd: HWND, text: &str) {
    DOCTOR.with(|cell| {
        if let Some(doctor) = cell.borrow_mut().as_mut() {
            let placeholder = format!("{INFO}checking...
");
            if let Some(at) = doctor.report.find(&placeholder) {
                doctor.report.truncate(at);
            }
        }
    });
    append(hwnd, text);
}

/// Add text to the end of the report and scroll to it.
fn append(hwnd: HWND, text: &str) {
    let full = DOCTOR.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let doctor = borrow.as_mut()?;
        doctor.report.push_str(text);
        Some(doctor.report.clone())
    });

    let Some(full) = full else { return };

    unsafe {
        let Ok(control) = GetDlgItem(Some(hwnd), ID_REPORT as i32) else {
            return;
        };
        let display = to_crlf(&full);
        let wide_text = wide(&display);
        let _ = SetWindowTextW(control, PCWSTR(wide_text.as_ptr()));

        // Park the caret at the end so the new lines are on screen.
        // windows-rs does not re-export the EM_* messages here.
        const EM_SETSEL: u32 = 0x00B1;
        const EM_SCROLLCARET: u32 = 0x00B7;
        let end = display.chars().count() as isize;
        let _ = SendMessageW(
            control,
            EM_SETSEL,
            Some(WPARAM(end as usize)),
            Some(LPARAM(end)),
        );
        let _ = SendMessageW(control, EM_SCROLLCARET, Some(WPARAM(0)), Some(LPARAM(0)));
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

        let text = wide(&to_crlf(report));
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
        .and_then(|()| std::fs::write(&path, to_crlf(report)))
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
                (ID_LOGIN, _) => try_sign_in(hwnd),
                (ID_CLOSE, _) => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_APP_DISCOVERED => {
            if wparam.0 != 0 {
                let text = *Box::from_raw(wparam.0 as *mut String);
                replace_placeholder(hwnd, &text);
            }
            if let Ok(control) = GetDlgItem(Some(hwnd), ID_LOGIN as i32) {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(control, true);
            }
            LRESULT(0)
        }

        WM_APP_RESULT => {
            if wparam.0 != 0 {
                let text = *Box::from_raw(wparam.0 as *mut String);
                append(hwnd, &text);
            }
            if let Ok(control) = GetDlgItem(Some(hwnd), ID_LOGIN as i32) {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(control, true);
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
