//! The setup wizard window.
//!
//! One window, four pages, rebuilt in place. Each page destroys the previous
//! page's controls and creates its own, so there is no tab control, no
//! property sheet and no hidden pages fighting over the tab order.
//!
//! Values are read back out of the controls before a page is torn down and
//! kept on `Wizard`, which is what lets Back work.

use std::path::PathBuf;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC,
    GetSysColorBrush, InvalidateRect, ReleaseDC, SelectObject, COLOR_BTNFACE, COLOR_WINDOW,
    DT_CALCRECT, DT_WORDBREAK, HFONT, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
};
use windows::Win32::UI::Shell::{SHBrowseForFolderW, SHGetPathFromIDListW, ShellExecuteW, BROWSEINFOW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS};
use windows::Win32::UI::WindowsAndMessaging::*;

use discord_taskbar::config::Config;
use discord_taskbar::ui::controls::{
    button, checkbox, child, edit, is_checked, set_checked, static_text, text_of, ui_font, wide,
    Place,
};

use crate::actions::{self, Options, Report, APP_NAME, EXE_NAME, VERSION};

const CLASS_NAME: PCWSTR = w!("DiscordTaskbarSetup");

/// Where to make the Discord application.
const PORTAL_URL: &str = "https://discord.com/developers/applications";

// Footer.
const ID_BACK: usize = 1;
const ID_NEXT: usize = 2;
const ID_CANCEL: usize = 3;
// Discord page.
const ID_PORTAL: usize = 10;
const ID_CLIENT_ID: usize = 11;
const ID_CLIENT_SECRET: usize = 12;
// Options page.
const ID_DIRECTORY: usize = 20;
const ID_BROWSE: usize = 21;
const ID_RUN_AT_SIGNIN: usize = 22;
const ID_START_MENU: usize = 23;
const ID_DESKTOP: usize = 24;
// Final page.
const ID_LAUNCH: usize = 30;
// Uninstall page.
const ID_PURGE: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Welcome,
    Discord,
    Options,
    Confirm,
    Done,
}

pub enum Mode {
    Install,
    /// Removing an existing install, which lives in this folder.
    Uninstall(PathBuf),
}

struct Wizard {
    hwnd: HWND,
    mode: Mode,
    page: Page,

    dpi: u32,
    font: HFONT,
    bold: HFONT,
    heading: HFONT,

    /// Controls belonging to the current page, destroyed on every transition.
    body: Vec<HWND>,

    // Carried across pages, so Back does not lose what was typed.
    client_id: String,
    client_secret: String,
    directory: String,
    run_at_signin: bool,
    start_menu: bool,
    desktop: bool,
    purge: bool,

    report: Option<Report>,
}

thread_local! {
    static WIZARD: std::cell::RefCell<Option<Wizard>> = const { std::cell::RefCell::new(None) };
}

fn with_wizard<R>(f: impl FnOnce(&mut Wizard) -> R) -> Option<R> {
    WIZARD.with(|cell| cell.borrow_mut().as_mut().map(f))
}

/// Run setup. Returns once the window closes.
pub fn run(mode: Mode) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
    }

    if !register_class() {
        return;
    }

    // An install that is really an upgrade should show what is already
    // configured rather than making the user find their client id again.
    let (existing, _) = Config::load_or_create();

    let uninstalling = matches!(mode, Mode::Uninstall(_));
    let caption = if uninstalling {
        format!("Remove {APP_NAME}")
    } else {
        format!("{APP_NAME} Setup")
    };

    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        let caption = wide(&caption);

        let Ok(hwnd) = CreateWindowExW(
            WS_EX_APPWINDOW,
            CLASS_NAME,
            PCWSTR(caption.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
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

        let dpi = match windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) {
            0 => 96,
            d => d,
        };

        let wizard = Wizard {
            hwnd,
            page: if uninstalling { Page::Confirm } else { Page::Welcome },
            mode,
            dpi,
            font: ui_font(scale_by(dpi, 9), false),
            bold: ui_font(scale_by(dpi, 9), true),
            heading: ui_font(scale_by(dpi, 14), true),
            body: Vec::new(),
            client_id: existing.discord.client_id.clone(),
            client_secret: existing.discord.client_secret.clone(),
            directory: actions::default_directory().to_string_lossy().into_owned(),
            run_at_signin: true,
            start_menu: true,
            desktop: false,
            purge: false,
            report: None,
        };

        WIZARD.with(|cell| *cell.borrow_mut() = Some(wizard));

        size_window(hwnd, dpi);
        build_footer();
        build_page();
        // Without this the first page keeps the raw footer: Back live when
        // there is nothing to go back to, and "Next" where the uninstall
        // page should say "Remove".
        update_footer();

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).into() {
            // Without this the arrow keys and Tab do not move between
            // controls, which is the first thing anybody tries.
            if IsDialogMessageW(hwnd, &message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn scale_by(dpi: u32, value: i32) -> i32 {
    (value as i64 * dpi as i64 / 96) as i32
}

/// How tall `text` will be once wrapped to `width`.
///
/// Sizing a block of prose by counting the lines in the source is wrong the
/// moment one of them wraps, or the font changes, or the display is at 125%.
/// Asking GDI is exact and costs one device context.
fn measure(hwnd: HWND, font: HFONT, text: &str, width: i32) -> i32 {
    unsafe {
        let hdc = GetDC(Some(hwnd));
        let previous = SelectObject(hdc, font.into());

        let mut buffer: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: 0,
        };
        DrawTextW(hdc, &mut buffer, &mut rect, DT_CALCRECT | DT_WORDBREAK);

        SelectObject(hdc, previous);
        ReleaseDC(Some(hwnd), hdc);
        rect.bottom - rect.top
    }
}

impl Wizard {
    fn scale(&self, value: i32) -> i32 {
        scale_by(self.dpi, value)
    }

    /// Client width and height. Fixed: nothing here benefits from resizing.
    fn size(&self) -> (i32, i32) {
        (self.scale(660), self.scale(600))
    }

    fn footer_top(&self) -> i32 {
        self.size().1 - self.scale(58)
    }

    /// The region pages draw into, below the heading and above the footer.
    fn body_area(&self) -> RECT {
        let (w, _) = self.size();
        RECT {
            left: self.scale(24),
            top: self.scale(104),
            right: w - self.scale(24),
            bottom: self.footer_top() - self.scale(16),
        }
    }

    fn clear_body(&mut self) {
        for hwnd in self.body.drain(..) {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }

    fn installing(&self) -> bool {
        matches!(self.mode, Mode::Install)
    }

    /// Read the current page's controls into the carried state.
    fn capture(&mut self) {
        let get = |id: usize| unsafe { GetDlgItem(Some(self.hwnd), id as i32).ok() };

        match self.page {
            Page::Discord => {
                if let Some(hwnd) = get(ID_CLIENT_ID) {
                    self.client_id = text_of(hwnd).trim().to_string();
                }
                if let Some(hwnd) = get(ID_CLIENT_SECRET) {
                    self.client_secret = text_of(hwnd).trim().to_string();
                }
            }
            Page::Options => {
                if let Some(hwnd) = get(ID_DIRECTORY) {
                    let typed = text_of(hwnd).trim().to_string();
                    if !typed.is_empty() {
                        self.directory = typed;
                    }
                }
                if let Some(hwnd) = get(ID_RUN_AT_SIGNIN) {
                    self.run_at_signin = is_checked(hwnd);
                }
                if let Some(hwnd) = get(ID_START_MENU) {
                    self.start_menu = is_checked(hwnd);
                }
                if let Some(hwnd) = get(ID_DESKTOP) {
                    self.desktop = is_checked(hwnd);
                }
            }
            Page::Confirm => {
                if let Some(hwnd) = get(ID_PURGE) {
                    self.purge = is_checked(hwnd);
                }
            }
            _ => {}
        }
    }

    fn next_page(&self) -> Option<Page> {
        match (self.page, self.installing()) {
            (Page::Welcome, _) => Some(Page::Discord),
            (Page::Discord, _) => Some(Page::Options),
            (Page::Options, _) => Some(Page::Done),
            (Page::Confirm, _) => Some(Page::Done),
            (Page::Done, _) => None,
        }
    }

    fn previous_page(&self) -> Option<Page> {
        match self.page {
            Page::Discord => Some(Page::Welcome),
            Page::Options => Some(Page::Discord),
            _ => None,
        }
    }
}

/// The Back / Next / Cancel row, which outlives every page.
fn build_footer() {
    with_wizard(|wizard| {
        let (width, _) = wizard.size();
        let w = wizard.scale(100);
        let h = wizard.scale(30);
        let y = wizard.footer_top() + wizard.scale(14);
        let right = width - wizard.scale(24);
        let gap = wizard.scale(8);

        let at = |slot: i32| Place {
            x: right - w * slot - gap * (slot - 1),
            y,
            w,
            h,
        };

        button(wizard.hwnd, "Cancel", ID_CANCEL, at(1), wizard.font);
        button(wizard.hwnd, "Next", ID_NEXT, at(2), wizard.font);
        button(wizard.hwnd, "Back", ID_BACK, at(3), wizard.font);
    });
}

/// Enable, rename and hide the footer buttons for the current page.
fn update_footer() {
    with_wizard(|wizard| unsafe {
        let done = wizard.page == Page::Done;
        let set = |id: usize, text: PCWSTR, visible: bool, enabled: bool| {
            if let Ok(hwnd) = GetDlgItem(Some(wizard.hwnd), id as i32) {
                let _ = SetWindowTextW(hwnd, text);
                let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
                let _ = EnableWindow(hwnd, enabled);
            }
        };

        set(
            ID_BACK,
            w!("Back"),
            !done,
            wizard.previous_page().is_some(),
        );
        set(
            ID_NEXT,
            match (done, wizard.page, wizard.installing()) {
                (true, _, _) => w!("Finish"),
                (_, Page::Options, _) => w!("Install"),
                (_, Page::Confirm, _) => w!("Remove"),
                _ => w!("Next"),
            },
            true,
            true,
        );
        set(ID_CANCEL, w!("Cancel"), !done, true);

        // Enter should do the obvious thing.
        if let Ok(next) = GetDlgItem(Some(wizard.hwnd), ID_NEXT as i32) {
            let _ = SetFocus(Some(next));
        }
    });
}

fn go(page: Page) {
    with_wizard(|wizard| {
        wizard.capture();
        wizard.clear_body();
        wizard.page = page;
    });

    // Running the install as the final page is entered means the report is
    // ready by the time that page draws itself.
    if with_wizard(|w| w.page == Page::Done) == Some(true) {
        perform();
    }

    build_page();
    update_footer();
    with_wizard(|wizard| unsafe {
        let _ = InvalidateRect(Some(wizard.hwnd), None, true);
    });
}

/// Do the work the wizard was opened for.
fn perform() {
    let report = with_wizard(|wizard| match &wizard.mode {
        Mode::Install => {
            let options = Options {
                directory: PathBuf::from(&wizard.directory),
                run_at_signin: wizard.run_at_signin,
                start_menu: wizard.start_menu,
                desktop: wizard.desktop,
            };
            let mut report = actions::install(&options);

            // Credentials go in after the files, so a failed copy does not
            // leave a config pointing at nothing.
            if !report.failed && !(wizard.client_id.is_empty() && wizard.client_secret.is_empty()) {
                let (mut config, _) = Config::load_or_create();
                config.discord.client_id = wizard.client_id.clone();
                config.discord.client_secret = wizard.client_secret.clone();
                report.step(
                    "Saved your Discord application details",
                    config.save().map_err(|e| e.to_string()),
                );
            }
            report
        }
        Mode::Uninstall(directory) => actions::uninstall(directory, wizard.purge),
    });

    with_wizard(|wizard| wizard.report = report);
}

fn build_page() {
    let page = match with_wizard(|w| w.page) {
        Some(page) => page,
        None => return,
    };
    match page {
        Page::Welcome => page_welcome(),
        Page::Discord => page_discord(),
        Page::Options => page_options(),
        Page::Confirm => page_confirm(),
        Page::Done => page_done(),
    }
}

/// Heading and subheading, drawn as controls so they share the page font.
fn heading(title: &str, subtitle: &str) {
    with_wizard(|wizard| {
        let (width, _) = wizard.size();
        let x = wizard.scale(24);
        let w = width - wizard.scale(48);

        let title = static_text(
            wizard.hwnd,
            title,
            Place {
                x,
                y: wizard.scale(22),
                w,
                h: wizard.scale(30),
            },
            wizard.heading,
        );
        let subtitle = static_text(
            wizard.hwnd,
            subtitle,
            Place {
                x,
                y: wizard.scale(56),
                w,
                h: wizard.scale(34),
            },
            wizard.font,
        );
        wizard.body.push(title);
        wizard.body.push(subtitle);
    });
}

fn page_welcome() {
    heading(
        &format!("{APP_NAME} {VERSION}"),
        "Who is in your Discord voice call, shown in the Windows taskbar.",
    );

    with_wizard(|wizard| {
        let area = wizard.body_area();
        let body = concat!(
            "While you are in a voice channel the taskbar shows the server, the channel and ",
            "everyone in the call, with the same green ring around whoever is talking. Your ",
            "own microphone and headphone buttons sit beside them and work as buttons.\r\n",
            "\r\n",
            "What setup will do:\r\n",
            "\r\n",
            "    \u{2022}  Install for your account only, under your own AppData folder.\r\n",
            "    \u{2022}  Ask nothing of Windows that needs administrator rights.\r\n",
            "    \u{2022}  Add an entry to Settings \u{203A} Apps so it uninstalls normally.\r\n",
            "\r\n",
            "What you need:\r\n",
            "\r\n",
            "    \u{2022}  The Discord desktop app, signed in and running. The browser version ",
            "cannot be read.\r\n",
            "    \u{2022}  A Discord application of your own, which the next page walks you ",
            "through creating. It is free and takes about a minute.\r\n",
            "\r\n",
            "Nothing is uploaded anywhere. The widget talks only to the Discord app already ",
            "running on this PC, over a local pipe that Discord provides for the purpose.",
        );

        let text = static_text(
            wizard.hwnd,
            body,
            Place {
                x: area.left,
                y: area.top,
                w: area.right - area.left,
                h: area.bottom - area.top,
            },
            wizard.font,
        );
        wizard.body.push(text);
    });
}

fn page_discord() {
    heading(
        "Create your Discord application",
        "Discord only lets a program read your voice status through an application you own.",
    );

    with_wizard(|wizard| {
        let area = wizard.body_area();
        let x = area.left;
        let width = area.right - area.left;

        let steps = concat!(
            "1.   Press \u{201C}Open the developer portal\u{201D} below and sign in with your ",
            "Discord account.\r\n",
            "\r\n",
            "2.   Press \u{201C}New Application\u{201D}, type any name \u{2014} \u{201C}Taskbar\u{201D} ",
            "will do \u{2014} tick the terms box and press \u{201C}Create\u{201D}.\r\n",
            "\r\n",
            "3.   In the sidebar on the left, choose \u{201C}OAuth2\u{201D}.\r\n",
            "\r\n",
            "4.   Under \u{201C}Redirects\u{201D}, press \u{201C}Add Redirect\u{201D} and enter exactly:\r\n",
            "\r\n",
            "             http://localhost\r\n",
            "\r\n",
            "      then press \u{201C}Save Changes\u{201D} at the bottom of the page. Signing in ",
            "fails without this step.\r\n",
            "\r\n",
            "5.   At the top of that same OAuth2 page, copy \u{201C}Client ID\u{201D} and paste it ",
            "below.\r\n",
            "\r\n",
            "6.   Beside \u{201C}Client Secret\u{201D}, press \u{201C}Reset Secret\u{201D} and confirm, ",
            "then press \u{201C}Copy\u{201D} and paste it below.",
        );

        // The portal button sits to the right of the list, so the steps wrap
        // in a narrower column than the rest of the page.
        let steps_w = width - wizard.scale(200);
        let steps_h = measure(wizard.hwnd, wizard.font, steps, steps_w) + wizard.scale(6);
        let text = static_text(
            wizard.hwnd,
            steps,
            Place {
                x,
                y: area.top,
                w: steps_w,
                h: steps_h,
            },
            wizard.font,
        );
        let portal = button(
            wizard.hwnd,
            "Open the developer portal",
            ID_PORTAL,
            Place {
                x: area.right - wizard.scale(186),
                y: area.top,
                w: wizard.scale(186),
                h: wizard.scale(32),
            },
            wizard.font,
        );
        wizard.body.push(text);
        wizard.body.push(portal);

        // The two fields.
        let label_w = wizard.scale(110);
        let field_w = width - label_w;
        let mut y = area.top + steps_h + wizard.scale(18);

        for (label, id, value) in [
            ("Client ID", ID_CLIENT_ID, wizard.client_id.clone()),
            (
                "Client Secret",
                ID_CLIENT_SECRET,
                wizard.client_secret.clone(),
            ),
        ] {
            let caption = static_text(
                wizard.hwnd,
                label,
                Place {
                    x,
                    y: y + wizard.scale(5),
                    w: label_w,
                    h: wizard.scale(20),
                },
                wizard.font,
            );
            let field = edit(
                wizard.hwnd,
                &value,
                id,
                Place {
                    x: x + label_w,
                    y,
                    w: field_w,
                    h: wizard.scale(24),
                },
                wizard.font,
            );
            wizard.body.push(caption);
            wizard.body.push(field);
            y += wizard.scale(32);
        }

        let footnote = concat!(
            "Sign in as the account that owns the application \u{2014} Discord restricts this kind ",
            "of access to an app's own owner, so somebody else's client id will not work.\r\n",
            "The secret is written only to your config file on this PC and is used only to talk to ",
            "the Discord app running beside it. You can leave both boxes empty and fill them in ",
            "later from the taskbar icon \u{203A} Settings.",
        );
        let note = static_text(
            wizard.hwnd,
            footnote,
            Place {
                x,
                y: y + wizard.scale(8),
                w: width,
                h: area.bottom - (y + wizard.scale(8)),
            },
            wizard.font,
        );
        wizard.body.push(note);
    });
}

fn page_options() {
    heading(
        "Choose how it is installed",
        "The defaults are fine for most people.",
    );

    with_wizard(|wizard| {
        let area = wizard.body_area();
        let x = area.left;
        let width = area.right - area.left;

        let label = static_text(
            wizard.hwnd,
            "Install folder",
            Place {
                x,
                y: area.top,
                w: width,
                h: wizard.scale(20),
            },
            wizard.bold,
        );

        let browse_w = wizard.scale(90);
        let folder = edit(
            wizard.hwnd,
            &wizard.directory.clone(),
            ID_DIRECTORY,
            Place {
                x,
                y: area.top + wizard.scale(24),
                w: width - browse_w - wizard.scale(8),
                h: wizard.scale(24),
            },
            wizard.font,
        );
        let browse = button(
            wizard.hwnd,
            "Browse\u{2026}",
            ID_BROWSE,
            Place {
                x: area.right - browse_w,
                y: area.top + wizard.scale(23),
                w: browse_w,
                h: wizard.scale(26),
            },
            wizard.font,
        );

        let hint = static_text(
            wizard.hwnd,
            concat!(
                "A per-user folder, so no administrator prompt appears. Installing somewhere ",
                "like Program Files would need one.",
            ),
            Place {
                x,
                y: area.top + wizard.scale(54),
                w: width,
                h: wizard.scale(34),
            },
            wizard.font,
        );

        wizard.body.push(label);
        wizard.body.push(folder);
        wizard.body.push(browse);
        wizard.body.push(hint);

        let mut y = area.top + wizard.scale(102);
        let section = static_text(
            wizard.hwnd,
            "Options",
            Place {
                x,
                y,
                w: width,
                h: wizard.scale(20),
            },
            wizard.bold,
        );
        wizard.body.push(section);
        y += wizard.scale(28);

        for (text, id, checked) in [
            (
                "Start Discord Taskbar when I sign in to Windows",
                ID_RUN_AT_SIGNIN,
                wizard.run_at_signin,
            ),
            ("Add a Start menu shortcut", ID_START_MENU, wizard.start_menu),
            ("Add a desktop shortcut", ID_DESKTOP, wizard.desktop),
        ] {
            let box_hwnd = checkbox(
                wizard.hwnd,
                text,
                id,
                Place {
                    x,
                    y,
                    w: width,
                    h: wizard.scale(22),
                },
                wizard.font,
            );
            set_checked(box_hwnd, checked);
            wizard.body.push(box_hwnd);
            y += wizard.scale(28);
        }

        let closing = static_text(
            wizard.hwnd,
            concat!(
                "The first time it connects, Discord will ask you to authorise the application. ",
                "That prompt appears once; the token is remembered afterwards.\r\n",
                "\r\n",
                "If a copy is already running from the folder above, setup closes it first.",
            ),
            Place {
                x,
                y: y + wizard.scale(14),
                w: width,
                h: area.bottom - (y + wizard.scale(14)),
            },
            wizard.font,
        );
        wizard.body.push(closing);
    });
}

fn page_confirm() {
    heading(
        &format!("Remove {APP_NAME}"),
        "This removes the program, its shortcuts and its entry in Installed apps.",
    );

    with_wizard(|wizard| {
        let area = wizard.body_area();
        let location = match &wizard.mode {
            Mode::Uninstall(directory) => directory.display().to_string(),
            Mode::Install => String::new(),
        };

        let text = static_text(
            wizard.hwnd,
            &format!("Installed at:\r\n    {location}"),
            Place {
                x: area.left,
                y: area.top,
                w: area.right - area.left,
                h: wizard.scale(50),
            },
            wizard.font,
        );
        let purge = checkbox(
            wizard.hwnd,
            "Also delete my settings and cached avatars",
            ID_PURGE,
            Place {
                x: area.left,
                y: area.top + wizard.scale(62),
                w: area.right - area.left,
                h: wizard.scale(22),
            },
            wizard.font,
        );
        set_checked(purge, wizard.purge);

        let hint = static_text(
            wizard.hwnd,
            concat!(
                "Left unticked, your Discord application details and appearance settings stay ",
                "where they are, so reinstalling picks up exactly where you left off.",
            ),
            Place {
                x: area.left + wizard.scale(20),
                y: area.top + wizard.scale(88),
                w: area.right - area.left - wizard.scale(20),
                h: wizard.scale(40),
            },
            wizard.font,
        );

        wizard.body.push(text);
        wizard.body.push(purge);
        wizard.body.push(hint);
    });
}

fn page_done() {
    let (installing, failed) = with_wizard(|wizard| {
        (
            wizard.installing(),
            wizard.report.as_ref().map(|r| r.failed).unwrap_or(false),
        )
    })
    .unwrap_or((true, false));

    let title = match (installing, failed) {
        (true, false) => "Installed".to_string(),
        (true, true) => "Finished with problems".to_string(),
        (false, false) => "Removed".to_string(),
        (false, true) => "Finished with problems".to_string(),
    };
    let subtitle = match (installing, failed) {
        (true, false) => {
            "Start Discord, then join a voice channel and look at your taskbar.".to_string()
        }
        (false, false) => format!("{APP_NAME} is no longer installed.").to_string(),
        _ => "Some steps did not complete. The details are below.".to_string(),
    };
    heading(&title, &subtitle);

    with_wizard(|wizard| {
        let area = wizard.body_area();
        let width = area.right - area.left;
        let log_h = area.bottom - area.top - wizard.scale(if wizard.installing() { 96 } else { 0 });

        // A read-only edit rather than a static: this is the one text on the
        // screen somebody might want to copy into a bug report.
        let log = child(
            wizard.hwnd,
            w!("EDIT"),
            &wizard
                .report
                .as_ref()
                .map(|r| r.text())
                .unwrap_or_default(),
            WINDOW_STYLE(
                WS_BORDER.0
                    | WS_VSCROLL.0
                    | WS_TABSTOP.0
                    | ES_MULTILINE as u32
                    | ES_READONLY as u32,
            ),
            0,
            Place {
                x: area.left,
                y: area.top,
                w: width,
                h: log_h,
            },
            wizard.font,
        );
        wizard.body.push(log);

        if wizard.installing() {
            let next = static_text(
                wizard.hwnd,
                concat!(
                    "The widget stays out of the way until you are in a voice call. Right-click ",
                    "its taskbar icon for Settings, or find it in the notification area.",
                ),
                Place {
                    x: area.left,
                    y: area.top + log_h + wizard.scale(12),
                    w: width,
                    h: wizard.scale(36),
                },
                wizard.font,
            );
            let launch = checkbox(
                wizard.hwnd,
                &format!("Start {APP_NAME} now"),
                ID_LAUNCH,
                Place {
                    x: area.left,
                    y: area.top + log_h + wizard.scale(54),
                    w: width,
                    h: wizard.scale(22),
                },
                wizard.font,
            );
            set_checked(launch, !failed);
            wizard.body.push(next);
            wizard.body.push(launch);
        }
    });
}

/// Launch what was just installed, if the box is ticked.
fn finish() {
    let launch = with_wizard(|wizard| {
        if !wizard.installing() {
            return None;
        }
        let ticked = unsafe { GetDlgItem(Some(wizard.hwnd), ID_LAUNCH as i32) }
            .ok()
            .map(is_checked)
            .unwrap_or(false);
        if !ticked {
            return None;
        }
        let exe = PathBuf::from(&wizard.directory).join(EXE_NAME);
        // Nothing to show until they are in a call, so with no credentials
        // yet the useful thing to open is the settings window.
        let arguments = if wizard.client_id.is_empty() || wizard.client_secret.is_empty() {
            "--settings"
        } else {
            ""
        };
        Some((exe, arguments))
    })
    .flatten();

    if let Some((exe, arguments)) = launch {
        unsafe {
            let file = wide(&exe.to_string_lossy());
            let arguments = wide(arguments);
            let _ = ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(file.as_ptr()),
                PCWSTR(arguments.as_ptr()),
                None,
                SW_SHOWNORMAL,
            );
        }
    }

    with_wizard(|wizard| unsafe {
        let _ = DestroyWindow(wizard.hwnd);
    });
}

/// Pick the install folder with the shell's own folder browser.
fn browse() {
    let picked = with_wizard(|wizard| unsafe {
        let title = wide("Where should Discord Taskbar be installed?");
        let info = BROWSEINFOW {
            hwndOwner: wizard.hwnd,
            lpszTitle: PCWSTR(title.as_ptr()),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            ..Default::default()
        };

        let list = SHBrowseForFolderW(&info);
        if list.is_null() {
            return None;
        }

        let mut buffer = [0u16; 260];
        let ok = SHGetPathFromIDListW(list, &mut buffer).as_bool();
        windows::Win32::System::Com::CoTaskMemFree(Some(list as *const _));
        if !ok {
            return None;
        }

        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        let chosen = PathBuf::from(String::from_utf16_lossy(&buffer[..end]));

        // Picking a folder means "put it in here", not "scatter files here",
        // so append the product name unless they already did.
        let chosen = if chosen.file_name().map(|n| n == APP_NAME).unwrap_or(false) {
            chosen
        } else {
            chosen.join(APP_NAME)
        };
        Some(chosen.to_string_lossy().into_owned())
    })
    .flatten();

    if let Some(path) = picked {
        with_wizard(|wizard| unsafe {
            wizard.directory = path.clone();
            if let Ok(hwnd) = GetDlgItem(Some(wizard.hwnd), ID_DIRECTORY as i32) {
                let text = wide(&path);
                let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
            }
        });
    }
}

fn size_window(hwnd: HWND, dpi: u32) {
    unsafe {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: scale_by(dpi, 660),
            bottom: scale_by(dpi, 600),
        };
        let _ = AdjustWindowRectEx(
            &mut rect,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            false,
            WS_EX_APPWINDOW,
        );
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;

        // Centred on the work area, not the screen, so the taskbar does not
        // clip the footer on a short display.
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let x = work.left + ((work.right - work.left) - width) / 2;
        let y = work.top + ((work.bottom - work.top) - height) / 2;

        let _ = SetWindowPos(hwnd, None, x.max(0), y.max(0), width, height, SWP_NOZORDER);
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
        // Static text sits on the window background, not on a grey slab.
        WM_CTLCOLORSTATIC => LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize),

        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }

        WM_COMMAND => {
            let id = wparam.0 & 0xFFFF;
            match id {
                ID_NEXT => {
                    let done = with_wizard(|w| w.page == Page::Done) == Some(true);
                    if done {
                        finish();
                    } else if let Some(page) = with_wizard(|w| w.next_page()).flatten() {
                        go(page);
                    }
                }
                ID_BACK => {
                    if let Some(page) = with_wizard(|w| w.previous_page()).flatten() {
                        go(page);
                    }
                }
                ID_CANCEL => {
                    let _ = DestroyWindow(hwnd);
                }
                ID_PORTAL => {
                    let url = wide(PORTAL_URL);
                    let _ = ShellExecuteW(
                        None,
                        w!("open"),
                        PCWSTR(url.as_ptr()),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
                ID_BROWSE => browse(),
                _ => {}
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }

        WM_DESTROY => {
            WIZARD.with(|cell| {
                if let Some(wizard) = cell.borrow_mut().take() {
                    let _ = DeleteObject(wizard.font.into());
                    let _ = DeleteObject(wizard.bold.into());
                    let _ = DeleteObject(wizard.heading.into());
                }
            });
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// The two hairlines and the grey footer band, which is all the chrome there
/// is — everything else on the window is a real control.
fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        with_wizard(|wizard| {
            let (width, height) = wizard.size();
            let footer = wizard.footer_top();

            let band = RECT {
                left: 0,
                top: footer,
                right: width,
                bottom: height,
            };
            FillRect(hdc, &band, GetSysColorBrush(COLOR_BTNFACE));

            // A soft rule rather than the 3D edge Windows used to draw.
            let rule = CreateSolidBrush(COLORREF(0x00D0_D0D0));
            for y in [wizard.scale(94), footer] {
                let line = RECT {
                    left: 0,
                    top: y,
                    right: width,
                    bottom: y + 1,
                };
                FillRect(hdc, &line, rule);
            }
            let _ = DeleteObject(rule.into());
        });

        let _ = EndPaint(hwnd, &ps);
    }
}
