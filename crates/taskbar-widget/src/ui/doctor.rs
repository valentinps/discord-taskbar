//! `--doctor`: find out why nothing is showing.
//!
//! The widget is deliberately invisible when its integration has nothing to
//! say. That is right in normal use and useless when something is wrong,
//! because every failure looks identical from the outside: an empty taskbar.
//!
//! The widget's own checks are here; the integration contributes its section
//! as a [`Report`].
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
/// The integration's section is ready; `wparam` carries a boxed `String`.
const WM_APP_DISCOVERED: u32 = WM_APP + 2;

/// Marks used down the left of the report, so it skims.
/// Status markers. Public so an integration's lines line up with these.
pub const OK: &str = "  ok  ";
pub const BAD: &str = " FAIL ";
pub const WARN: &str = " warn ";
pub const INFO: &str = "      ";

/// Gather the local checks, show them, then fill in the integration's.
/// A button in the diagnostics window that tries something for real.
///
/// The report can only describe what is already written down. Some failures —
/// a correctly-created but misconfigured application, say — only show up when
/// you actually attempt the thing, which is what this is for.
pub struct Action {
    pub label: String,
    /// Printed the moment it is pressed, before the slow part starts.
    pub announcement: String,
    /// The slow part, run on a worker thread. A plain function pointer, so
    /// nothing here has to be `Send`.
    pub run: fn() -> String,
}

/// What an integration contributes to the report.
pub struct Report {
    /// Heading for the integration's own section, e.g. `DISCORD`.
    pub heading: String,
    /// Checks that can be answered from files and the local machine, shown
    /// under the widget's own settings section.
    pub local: String,
    /// A check that talks to the thing itself.
    ///
    /// Run on a worker thread and shown as "checking..." until it lands: a
    /// tool whose whole job is to explain a stuck program must never be the
    /// thing that hangs, and doing this up front would have meant no window
    /// at all while it waited.
    pub probe: fn() -> String,
    pub action: Option<Action>,
}

impl Default for Report {
    fn default() -> Self {
        Report {
            heading: "INTEGRATION".to_string(),
            local: String::new(),
            probe: || String::new(),
            action: None,
        }
    }
}

pub fn run(report: Report) {
    show(report);
}

/// Everything that can be answered without talking to anything.
///
/// Deliberately one long string rather than a structure: it exists to be read
/// by a person and pasted into a chat window.
pub fn gather_local(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Taskbar widget {} — diagnostics",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(out, "{}", "=".repeat(62));
    let _ = writeln!(out);

    section_app(&mut out);
    section_config(&mut out, report);
    section_taskbar(&mut out);

    // Filled in by the worker thread; see `Report::probe`.
    let _ = writeln!(out, "{}", report.heading);
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

/// The closing note, appended once the integration's section has landed.
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

fn section_config(out: &mut String, report: &Report) {
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

    // Whatever the integration can say about its own settings.
    let _ = config;
    out.push_str(&report.local);

    let _ = writeln!(out, "{INFO}image cache    {}", cache_dir().display());
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
    text: String,
    font: HFONT,
    /// Kept so the button can still run its action after the window is up.
    report: Report,
}

thread_local! {
    static DOCTOR: std::cell::RefCell<Option<Doctor>> = const { std::cell::RefCell::new(None) };
}

fn show(report: Report) {
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
            w!("Taskbar widget — Diagnostics"),
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

        let text = gather_local(&report);
        let probe = report.probe;
        let action_label = report.action.as_ref().map(|a| a.label.clone());

        DOCTOR.with(|cell| {
            *cell.borrow_mut() = Some(Doctor {
                text: text.clone(),
                font,
                report,
            })
        });

        build(hwnd, &text, action_label.as_deref(), &scale, font);

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);

        // Now that there is something on screen, go and ask.
        start_probe(hwnd, probe);

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

fn build(
    hwnd: HWND,
    report: &str,
    action: Option<&str>,
    scale: &impl Fn(i32) -> i32,
    font: HFONT,
) {
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
    let mut buttons = vec![
        ("Close".to_string(), ID_CLOSE, button_w),
        ("Save to file\u{2026}".to_string(), ID_SAVE, button_w),
        ("Copy".to_string(), ID_COPY, button_w),
    ];
    // Only when the integration offers one.
    if let Some(action) = action {
        buttons.push((action.to_string(), ID_LOGIN, scale(170)));
    }

    for (label, id, w) in buttons {
        button(
            hwnd,
            &label,
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

/// Fill in the integration's section from a worker thread.
///
/// Replaces the "checking..." placeholder rather than appending, so the
/// finished report reads as though it had been gathered in one go.
fn start_probe(hwnd: HWND, probe: fn() -> String) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        let text = probe() + &footer();

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

/// Run the integration's action on a worker thread and append what happened.
fn start_action(hwnd: HWND, announcement: &str, run: fn() -> String) {
    unsafe {
        if let Ok(control) = GetDlgItem(Some(hwnd), ID_LOGIN as i32) {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(control, false);
        }
    }

    append(hwnd, announcement);

    // HWND is not Send, so carry the raw value across.
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        let text = run();
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

fn replace_placeholder(hwnd: HWND, text: &str) {
    DOCTOR.with(|cell| {
        if let Some(doctor) = cell.borrow_mut().as_mut() {
            let placeholder = format!("{INFO}checking...
");
            if let Some(at) = doctor.text.find(&placeholder) {
                doctor.text.truncate(at);
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
        doctor.text.push_str(text);
        Some(doctor.text.clone())
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
            let text = DOCTOR.with(|cell| cell.borrow().as_ref().map(|d| d.text.clone()));
            match (id, text) {
                (ID_COPY, Some(text)) => copy(hwnd, &text),
                (ID_SAVE, Some(text)) => save(hwnd, &text),
                (ID_LOGIN, _) => {
                    let action = DOCTOR.with(|cell| {
                        cell.borrow().as_ref().and_then(|d| {
                            d.report
                                .action
                                .as_ref()
                                .map(|a| (a.announcement.clone(), a.run))
                        })
                    });
                    if let Some((announcement, run)) = action {
                        start_action(hwnd, &announcement, run);
                    }
                }
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
