//! The settings window.
//!
//! Ordinary Win32 controls rather than the drawn UI the widget uses: a
//! settings dialog should look like the rest of Windows, and text entry,
//! focus, tab order, IME and accessibility all come free with the real
//! controls. The widget's own painting is for living inside the taskbar,
//! which is a different problem.
//!
//! The fields are declared once in `FIELDS` and everything else — creating
//! controls, filling them in, reading them back — is driven from that table.
//! Adding a setting means adding a row, not touching four functions.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, GetSysColorBrush, COLOR_WINDOW, HFONT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::Config;
use crate::ui::controls::{
    button, checkbox, combo, edit, is_checked, set_checked, static_text, text_of, ui_font, wide,
    Place,
};

const CLASS_NAME: PCWSTR = w!("DiscordTaskbarSettings");

/// Where to make the Discord application. Shown in the window and opened by
/// the button beside it.
pub const PORTAL_URL: &str = "https://discord.com/developers/applications";

/// Control ids. Field controls start at `ID_FIELD_BASE + index`.
const ID_SAVE: usize = 1;
const ID_CANCEL: usize = 2;
const ID_PORTAL: usize = 3;
const ID_FIELD_BASE: usize = 100;

/// What kind of editor a setting needs.
enum Kind {
    Toggle,
    /// A whole number, clamped on the way in.
    Number { min: i32, max: i32 },
    /// Free text — colours, monitor lists, credentials.
    Text,
    /// One of a fixed set.
    Choice(&'static [&'static str]),
}

struct Field {
    section: &'static str,
    label: &'static str,
    kind: Kind,
    get: fn(&Config) -> String,
    set: fn(&mut Config, &str),
}

/// Every setting the window offers, in the order it shows them.
///
/// Not every key in `config.json` appears here: the ones left out are either
/// derived or so rarely useful that a text editor is the better tool.
static FIELDS: &[Field] = &[
    Field {
        section: "Discord application",
        label: "Client ID",
        kind: Kind::Text,
        get: |c| c.discord.client_id.clone(),
        set: |c, v| c.discord.client_id = v.trim().to_string(),
    },
    Field {
        section: "Discord application",
        label: "Client secret",
        kind: Kind::Text,
        get: |c| c.discord.client_secret.clone(),
        set: |c, v| c.discord.client_secret = v.trim().to_string(),
    },
    Field {
        section: "Displays",
        label: "Show on",
        kind: Kind::Text,
        get: |c| c.appearance.monitors.join(", "),
        set: |c, v| {
            let list: Vec<String> = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            c.appearance.monitors = if list.is_empty() {
                vec!["primary".to_string()]
            } else {
                list
            };
        },
    },
    Field {
        section: "Displays",
        label: "Horizontal nudge",
        kind: Kind::Number { min: -4000, max: 4000 },
        get: |c| c.appearance.x_offset.to_string(),
        set: |c, v| c.appearance.x_offset = parse_int(v, c.appearance.x_offset),
    },
    Field {
        section: "Displays",
        label: "Vertical nudge",
        kind: Kind::Number { min: -400, max: 400 },
        get: |c| c.appearance.y_offset.to_string(),
        set: |c, v| c.appearance.y_offset = parse_int(v, c.appearance.y_offset),
    },
    Field {
        section: "Size",
        label: "Height (0 fills the bar)",
        kind: Kind::Number { min: 0, max: 80 },
        get: |c| c.appearance.height.to_string(),
        set: |c, v| c.appearance.height = parse_int(v, c.appearance.height),
    },
    Field {
        section: "Size",
        label: "Text size",
        kind: Kind::Number { min: 6, max: 40 },
        get: |c| c.appearance.font_size.to_string(),
        set: |c, v| c.appearance.font_size = parse_int(v, c.appearance.font_size),
    },
    Field {
        section: "Size",
        label: "Icon size",
        kind: Kind::Number { min: 6, max: 48 },
        get: |c| c.appearance.icon_size.to_string(),
        set: |c, v| c.appearance.icon_size = parse_int(v, c.appearance.icon_size),
    },
    Field {
        section: "Size",
        label: "Corner radius",
        kind: Kind::Number { min: 0, max: 40 },
        get: |c| c.appearance.corner_radius.to_string(),
        set: |c, v| c.appearance.corner_radius = parse_int(v, c.appearance.corner_radius),
    },
    Field {
        section: "Size",
        label: "Inner padding",
        kind: Kind::Number { min: 0, max: 40 },
        get: |c| c.appearance.padding.to_string(),
        set: |c, v| c.appearance.padding = parse_int(v, c.appearance.padding),
    },
    Field {
        section: "Size",
        label: "Gap between parts",
        kind: Kind::Number { min: 0, max: 40 },
        get: |c| c.appearance.spacing.to_string(),
        set: |c, v| c.appearance.spacing = parse_int(v, c.appearance.spacing),
    },
    Field {
        section: "Colours",
        label: "Background",
        kind: Kind::Text,
        get: |c| c.appearance.background.clone(),
        set: |c, v| c.appearance.background = v.trim().to_string(),
    },
    Field {
        section: "Colours",
        label: "See-through background",
        kind: Kind::Toggle,
        get: |c| bool_text(c.appearance.transparent),
        set: |c, v| c.appearance.transparent = v == "1",
    },
    Field {
        section: "Colours",
        label: "Behind widget",
        kind: Kind::Text,
        get: |c| c.appearance.taskbar_background.clone(),
        set: |c, v| c.appearance.taskbar_background = v.trim().to_string(),
    },
    Field {
        section: "Colours",
        label: "Text",
        kind: Kind::Text,
        get: |c| c.appearance.text.clone(),
        set: |c, v| c.appearance.text = v.trim().to_string(),
    },
    Field {
        section: "Colours",
        label: "Dimmed text",
        kind: Kind::Text,
        get: |c| c.appearance.text_dim.clone(),
        set: |c, v| c.appearance.text_dim = v.trim().to_string(),
    },
    Field {
        section: "Colours",
        label: "Speaking ring",
        kind: Kind::Text,
        get: |c| c.appearance.speaking.clone(),
        set: |c, v| c.appearance.speaking = v.trim().to_string(),
    },
    Field {
        section: "Colours",
        label: "Muted / deafened",
        kind: Kind::Text,
        get: |c| c.appearance.danger.clone(),
        set: |c, v| c.appearance.danger = v.trim().to_string(),
    },
    Field {
        section: "Colours",
        label: "Divider",
        kind: Kind::Text,
        get: |c| c.appearance.divider.clone(),
        set: |c, v| c.appearance.divider = v.trim().to_string(),
    },
    Field {
        section: "Participants",
        label: "Avatar size",
        kind: Kind::Number { min: 8, max: 64 },
        get: |c| c.appearance.avatar_size.to_string(),
        set: |c, v| c.appearance.avatar_size = parse_int(v, c.appearance.avatar_size),
    },
    Field {
        section: "Participants",
        label: "Overlap (negative gaps)",
        kind: Kind::Number { min: -40, max: 40 },
        get: |c| c.appearance.avatar_overlap.to_string(),
        set: |c, v| c.appearance.avatar_overlap = parse_int(v, c.appearance.avatar_overlap),
    },
    Field {
        section: "Participants",
        label: "Most avatars shown",
        kind: Kind::Number { min: 1, max: 32 },
        get: |c| c.appearance.max_avatars.to_string(),
        set: |c, v| {
            c.appearance.max_avatars = parse_int(v, c.appearance.max_avatars as i32).max(1) as usize
        },
    },
    Field {
        section: "Participants",
        label: "Speakers move to the front",
        kind: Kind::Toggle,
        get: |c| bool_text(c.appearance.sort_by_speaking),
        set: |c, v| c.appearance.sort_by_speaking = v == "1",
    },
    Field {
        section: "Participants",
        label: "Volume per wheel notch",
        kind: Kind::Number { min: 1, max: 50 },
        get: |c| c.appearance.scroll_volume_step.to_string(),
        set: |c, v| {
            c.appearance.scroll_volume_step = parse_int(v, c.appearance.scroll_volume_step)
        },
    },
    Field {
        section: "Participants",
        label: "Middle-click does",
        kind: Kind::Choice(&["local_mute", "volume_reset", "none"]),
        get: |c| c.appearance.middle_click.clone(),
        set: |c, v| c.appearance.middle_click = v.to_string(),
    },
    Field {
        section: "What to show",
        label: "Server icon",
        kind: Kind::Toggle,
        get: |c| bool_text(c.appearance.show_guild_icon),
        set: |c, v| c.appearance.show_guild_icon = v == "1",
    },
    Field {
        section: "What to show",
        label: "Server name",
        kind: Kind::Toggle,
        get: |c| bool_text(c.appearance.show_guild_name),
        set: |c, v| c.appearance.show_guild_name = v == "1",
    },
    Field {
        section: "What to show",
        label: "Your mic and headphones",
        kind: Kind::Toggle,
        get: |c| bool_text(c.appearance.show_self_icons),
        set: |c, v| c.appearance.show_self_icons = v == "1",
    },
    Field {
        section: "What to show",
        label: "Clicking those toggles them",
        kind: Kind::Toggle,
        get: |c| bool_text(c.appearance.clickable_self_icons),
        set: |c, v| c.appearance.clickable_self_icons = v == "1",
    },
    Field {
        section: "What to show",
        label: "Divider",
        kind: Kind::Toggle,
        get: |c| bool_text(c.appearance.show_divider),
        set: |c, v| c.appearance.show_divider = v == "1",
    },
    Field {
        section: "What to show",
        label: "Hang-up button",
        kind: Kind::Toggle,
        get: |c| bool_text(c.appearance.show_leave_button),
        set: |c, v| c.appearance.show_leave_button = v == "1",
    },
];

fn bool_text(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

fn parse_int(text: &str, fallback: i32) -> i32 {
    text.trim().parse().unwrap_or(fallback)
}

/// The one settings window, if it is open.
static OPEN: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

struct Window {
    controls: Vec<HWND>,
    font: HFONT,
    bold: HFONT,
    /// Called with the edited config when Save is pressed.
    on_save: Box<dyn Fn(Config)>,
}

thread_local! {
    static WINDOW: std::cell::RefCell<Option<Window>> = const { std::cell::RefCell::new(None) };
}

/// Open the settings window, or bring the existing one to the front.
///
/// `on_save` runs on the UI thread with the edited config, so the caller can
/// apply it immediately rather than making the user restart.
pub fn open(config: &Config, on_save: Box<dyn Fn(Config)>) {
    use std::sync::atomic::Ordering;

    let existing = OPEN.load(Ordering::Relaxed);
    if existing != 0 {
        unsafe {
            let hwnd = HWND(existing as *mut std::ffi::c_void);
            if IsWindow(Some(hwnd)).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
                return;
            }
        }
    }

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
            w!("Discord Taskbar — Settings"),
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

        let dpi = match GetDpiForWindow(hwnd) {
            0 => 96,
            d => d,
        };
        let scale = |v: i32| (v as i64 * dpi as i64 / 96) as i32;

        let font = ui_font(scale(9), false);
        let bold = ui_font(scale(9), true);

        let controls = build(hwnd, config, scale, font, bold);

        WINDOW.with(|cell| {
            *cell.borrow_mut() = Some(Window {
                controls,
                font,
                bold,
                on_save,
            });
        });

        OPEN.store(hwnd.0 as isize, Ordering::Relaxed);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Create every control and size the window around them.
///
/// Two columns, filled section by section, because thirty settings in one
/// column would be a scrollbar nobody enjoys.
fn build(
    parent: HWND,
    config: &Config,
    scale: impl Fn(i32) -> i32,
    font: HFONT,
    bold: HFONT,
) -> Vec<HWND> {
    let margin = scale(14);
    let row = scale(26);
    let label_w = scale(168);
    let control_w = scale(150);
    let column_w = label_w + control_w + scale(16);
    let header_h = row - scale(4);
    let header_gap = scale(10);

    let mut controls = vec![HWND::default(); FIELDS.len()];
    let mut extra: Vec<HWND> = Vec::new();

    // Explain what this is for before anything else.
    let button_w = scale(150);
    let intro_h = scale(52);
    extra.push(static_text(
        parent,
        concat!(
            "Set up a Discord application of your own, then paste its Client ID ",
            "and Client Secret below. Under OAuth2, add exactly  http://localhost  ",
            "as a redirect URI and save.

",
            "Nothing is sent anywhere except your own Discord client, on this machine.",
        ),
        Place {
            x: margin,
            y: margin,
            w: column_w * 2 - button_w - scale(16),
            h: intro_h,
        },
        font,
    ));
    extra.push(button(
        parent,
        "Open Discord developer portal",
        ID_PORTAL,
        Place {
            x: margin + column_w * 2 - button_w,
            y: margin,
            w: button_w,
            h: scale(40),
        },
        font,
    ));

    let top = margin + intro_h + scale(8);

    // Group the fields into their sections, keeping declaration order.
    let mut sections: Vec<(&'static str, Vec<usize>)> = Vec::new();
    for (index, field) in FIELDS.iter().enumerate() {
        match sections.last_mut() {
            Some((name, members)) if *name == field.section => members.push(index),
            _ => sections.push((field.section, vec![index])),
        }
    }

    // Split into two columns by height rather than by count: a section header
    // costs a row too, so an even split of fields leaves the columns ragged.
    let height_of = |members: &Vec<usize>| header_h + header_gap + members.len() as i32 * row;
    let total: i32 = sections.iter().map(|(_, m)| height_of(m)).sum();

    // Pick the boundary that leaves the two columns closest in height, rather
    // than the first one past the midpoint — a single tall section either side
    // of the midpoint otherwise skews it badly.
    let mut best_split = sections.len();
    let mut best_diff = i32::MAX;
    let mut running = 0;
    for index in 1..sections.len() {
        running += height_of(&sections[index - 1].1);
        let diff = (running - (total - running)).abs();
        if diff < best_diff {
            best_diff = diff;
            best_split = index;
        }
    }

    let column_starts: Vec<usize> = (0..sections.len())
        .map(|index| usize::from(index >= best_split))
        .collect();

    let mut y = [top, top];

    // Labels sit nudged down so their baseline lines up with the control
    // beside them, which every row with a control does identically.
    let label_at = |x: i32, fy: i32| Place {
        x,
        y: fy + scale(4),
        w: label_w,
        h: row,
    };

    for (index, (name, members)) in sections.iter().enumerate() {
        let column = column_starts[index];
        let x = margin + column as i32 * column_w;

        if y[column] > top {
            y[column] += header_gap;
        }
        extra.push(static_text(
            parent,
            name,
            Place {
                x,
                y: y[column],
                w: column_w,
                h: header_h,
            },
            bold,
        ));
        y[column] += header_h;

        for field_index in members {
            let field = &FIELDS[*field_index];
            let value = (field.get)(config);
            let fy = y[column];

            match &field.kind {
                Kind::Toggle => {
                    let check = checkbox(
                        parent,
                        field.label,
                        ID_FIELD_BASE + field_index,
                        Place {
                            x,
                            y: fy,
                            w: label_w + control_w,
                            h: row - scale(4),
                        },
                        font,
                    );
                    set_checked(check, value == "1");
                    controls[*field_index] = check;
                }
                Kind::Choice(options) => {
                    extra.push(static_text(parent, field.label, label_at(x, fy), font));
                    let combo = combo(
                        parent,
                        ID_FIELD_BASE + field_index,
                        Place {
                            x: x + label_w,
                            y: fy,
                            w: control_w,
                            h: row * 8,
                        },
                        font,
                    );
                    unsafe {
                        for option in *options {
                            let text = wide(option);
                            let _ = SendMessageW(
                                combo,
                                CB_ADDSTRING,
                                Some(WPARAM(0)),
                                Some(LPARAM(text.as_ptr() as isize)),
                            );
                        }
                        let selected =
                            options.iter().position(|o| *o == value).unwrap_or(0);
                        let _ = SendMessageW(
                            combo,
                            CB_SETCURSEL,
                            Some(WPARAM(selected)),
                            Some(LPARAM(0)),
                        );
                    }
                    controls[*field_index] = combo;
                }
                Kind::Text | Kind::Number { .. } => {
                    extra.push(static_text(parent, field.label, label_at(x, fy), font));
                    controls[*field_index] = edit(
                        parent,
                        &value,
                        ID_FIELD_BASE + field_index,
                        Place {
                            x: x + label_w,
                            y: fy,
                            w: control_w,
                            h: row - scale(4),
                        },
                        font,
                    );
                }
            }

            y[column] += row;
        }
    }

    // Buttons sit below whichever column ended lower.
    let content_bottom = y[0].max(y[1]);
    let action_w = scale(90);
    let action_h = scale(28);
    let bottom = content_bottom + scale(14);
    let right = margin + column_w * 2;

    extra.push(button(
        parent,
        "Save",
        ID_SAVE,
        Place {
            x: right - action_w * 2 - scale(8),
            y: bottom,
            w: action_w,
            h: action_h,
        },
        font,
    ));
    extra.push(button(
        parent,
        "Close",
        ID_CANCEL,
        Place {
            x: right - action_w,
            y: bottom,
            w: action_w,
            h: action_h,
        },
        font,
    ));

    // Size the frame to fit what was laid out.
    unsafe {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: right + margin,
            bottom: bottom + action_h + margin,
        };
        let _ = AdjustWindowRectEx(
            &mut rect,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            false,
            WS_EX_APPWINDOW,
        );
        let _ = SetWindowPos(
            parent,
            None,
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    controls.extend(extra);
    controls
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
            // COLOR_WINDOW, so the dialog matches the system theme.
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&class) != 0
            || windows::Win32::Foundation::GetLastError()
                == windows::Win32::Foundation::WIN32_ERROR(1410)
    }
}

/// Read every control back into a copy of the config.
fn collect(base: &Config, controls: &[HWND]) -> Config {
    let mut config = base.clone();

    for (index, field) in FIELDS.iter().enumerate() {
        let Some(hwnd) = controls.get(index) else {
            continue;
        };
        if hwnd.is_invalid() {
            continue;
        }

        let value = match &field.kind {
            Kind::Toggle => bool_text(is_checked(*hwnd)),
            Kind::Choice(options) => unsafe {
                let selected =
                    SendMessageW(*hwnd, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0;
                options
                    .get(selected.max(0) as usize)
                    .unwrap_or(&options[0])
                    .to_string()
            },
            Kind::Text => text_of(*hwnd),
            Kind::Number { min, max } => {
                let text = text_of(*hwnd);
                let current = (field.get)(base).parse().unwrap_or(0);
                parse_int(&text, current).clamp(*min, *max).to_string()
            }
        };

        (field.set)(&mut config, &value);
    }

    config
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = wparam.0 & 0xFFFF;
                match id {
                    ID_PORTAL => {
                        let url = wide(PORTAL_URL);
                        ShellExecuteW(
                            None,
                            w!("open"),
                            PCWSTR(url.as_ptr()),
                            PCWSTR::null(),
                            PCWSTR::null(),
                            SW_SHOWNORMAL,
                        );
                    }
                    ID_SAVE => {
                        WINDOW.with(|cell| {
                            if let Some(window) = cell.borrow().as_ref() {
                                // Re-read from disk so a key this window does
                                // not offer is preserved rather than reset.
                                let (base, _) = Config::load_or_create();
                                let edited = collect(&base, &window.controls);
                                (window.on_save)(edited);
                            }
                        });
                    }
                    ID_CANCEL => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }

            // Labels and checkboxes paint on the window's own background.
            WM_CTLCOLORSTATIC => LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize),

            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }

            WM_DESTROY => {
                use std::sync::atomic::Ordering;
                OPEN.store(0, Ordering::Relaxed);
                WINDOW.with(|cell| {
                    if let Some(window) = cell.borrow_mut().take() {
                        let _ = DeleteObject(window.font.into());
                        let _ = DeleteObject(window.bold.into());
                    }
                });
                LRESULT(0)
            }

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
