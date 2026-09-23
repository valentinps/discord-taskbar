//! The settings window.
//!
//! Ordinary Win32 controls rather than the drawn UI the widget uses: a
//! settings dialog should look like the rest of Windows, and text entry,
//! focus, tab order, IME and accessibility all come free with the real
//! controls. The widget's own painting is for living inside the taskbar,
//! which is a different problem.
//!
//! The rows are declared as `Field`s and everything else — creating controls,
//! filling them in, reading them back — is driven from that list. Each field
//! names a dotted path into `config.json` rather than carrying a getter and a
//! setter, which is what lets an integration contribute its own without this
//! module knowing anything about them.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, GetSysColorBrush, COLOR_WINDOW, HFONT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use serde_json::Value;

use crate::config::Config;
use crate::ui::controls::{
    button, checkbox, combo, edit, is_checked, set_checked, static_text, text_of, ui_font, wide,
    Place,
};

const CLASS_NAME: PCWSTR = w!("DiscordTaskbarSettings");

/// Control ids. Field controls start at `ID_FIELD_BASE + index`.
const ID_SAVE: usize = 1;
const ID_CANCEL: usize = 2;
const ID_PORTAL: usize = 3;
const ID_FIELD_BASE: usize = 100;

/// What kind of editor a setting needs.
#[derive(Debug, Clone)]
pub enum Kind {
    /// A JSON boolean.
    Toggle,
    /// A whole number, clamped on the way in.
    Number { min: i32, max: i32 },
    /// A fractional number, clamped on the way in.
    Decimal { min: f32, max: f32 },
    /// Free text — colours, credentials.
    Text,
    /// An array of strings, edited as a comma-separated list.
    List,
    /// One of a fixed set of strings.
    Choice(Vec<String>),
}

/// One row of the settings window.
///
/// `path` is dotted, and addresses a value in the serialised config —
/// `appearance.height`, or `integrations.discord.client_id`. Addressing by
/// path rather than by accessor is what keeps this module free of any
/// particular integration's types.
#[derive(Debug, Clone)]
pub struct Field {
    pub section: String,
    pub label: String,
    pub kind: Kind,
    pub path: String,
}

impl Field {
    fn new(section: &str, label: &str, path: impl Into<String>, kind: Kind) -> Self {
        Field {
            section: section.to_string(),
            label: label.to_string(),
            kind,
            path: path.into(),
        }
    }

    pub fn toggle(section: &str, label: &str, path: impl Into<String>) -> Self {
        Field::new(section, label, path, Kind::Toggle)
    }

    pub fn number(section: &str, label: &str, path: impl Into<String>, min: i32, max: i32) -> Self {
        Field::new(section, label, path, Kind::Number { min, max })
    }

    pub fn decimal(
        section: &str,
        label: &str,
        path: impl Into<String>,
        min: f32,
        max: f32,
    ) -> Self {
        Field::new(section, label, path, Kind::Decimal { min, max })
    }

    pub fn text(section: &str, label: &str, path: impl Into<String>) -> Self {
        Field::new(section, label, path, Kind::Text)
    }

    pub fn list(section: &str, label: &str, path: impl Into<String>) -> Self {
        Field::new(section, label, path, Kind::List)
    }

    pub fn choice<I, T>(section: &str, label: &str, path: impl Into<String>, options: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let options = options.into_iter().map(Into::into).collect();
        Field::new(section, label, path, Kind::Choice(options))
    }
}

/// A note at the top of the window, with a button that opens a link.
///
/// An integration that needs setting up elsewhere — a developer portal, an API
/// key page — says so here rather than the window hard-coding it.
pub struct Intro {
    pub text: String,
    pub button: String,
    pub url: String,
}

/// The widget's own settings, shown above whatever the integration adds.
pub fn core_fields() -> Vec<Field> {
    vec![
        Field::list("Displays", "Show on", "appearance.monitors"),
        Field::number("Displays", "Horizontal nudge", "appearance.x_offset", -4000, 4000),
        Field::number("Displays", "Vertical nudge", "appearance.y_offset", -400, 400),
        Field::number("Size", "Height (0 fills the bar)", "appearance.height", 0, 80),
        Field::number("Size", "Text size", "appearance.font_size", 6, 40),
        Field::number("Size", "Icon size", "appearance.icon_size", 6, 48),
        Field::number("Size", "Corner radius", "appearance.corner_radius", 0, 40),
        Field::number("Size", "Inner padding", "appearance.padding", 0, 40),
        Field::number("Size", "Gap between parts", "appearance.spacing", 0, 40),
        Field::number("Size", "Longest label", "appearance.max_label_width", 40, 2000),
        Field::text("Colours", "Background", "appearance.background"),
        Field::text("Colours", "Text", "appearance.text"),
        Field::text("Colours", "Dimmed text", "appearance.text_dim"),
        Field::text("Colours", "Accent", "appearance.accent"),
        Field::text("Colours", "Muted / deafened", "appearance.danger"),
        Field::text("Colours", "Divider", "appearance.divider"),
        Field::text("Colours", "Image placeholder", "appearance.placeholder"),
    ]
}

/// Read a dotted path out of a serialised config.
fn at<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut node = root;
    for step in path.split('.') {
        node = node.get(step)?;
    }
    Some(node)
}

/// Write a dotted path into a serialised config, creating objects on the way.
fn set_at(root: &mut Value, path: &str, value: Value) {
    let mut node = root;
    let steps: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = steps.split_last() else {
        return;
    };

    for step in parents {
        if !node.is_object() {
            *node = Value::Object(Default::default());
        }
        node = node
            .as_object_mut()
            .expect("just made an object")
            .entry((*step).to_string())
            .or_insert_with(|| Value::Object(Default::default()));
    }

    if !node.is_object() {
        *node = Value::Object(Default::default());
    }
    if let Some(map) = node.as_object_mut() {
        map.insert((*last).to_string(), value);
    }
}

/// What a field's control should show for the config it is editing.
fn display(root: &Value, field: &Field) -> String {
    let value = at(root, &field.path);
    match &field.kind {
        Kind::Toggle => bool_text(value.and_then(Value::as_bool).unwrap_or(false)),
        Kind::Number { .. } => value
            .and_then(Value::as_i64)
            .map(|n| n.to_string())
            .unwrap_or_default(),
        Kind::Decimal { .. } => value
            .and_then(Value::as_f64)
            .map(|n| format!("{n}"))
            .unwrap_or_default(),
        Kind::Text | Kind::Choice(_) => {
            value.and_then(Value::as_str).unwrap_or_default().to_string()
        }
        Kind::List => value
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
    }
}

/// Turn what a control says back into the JSON the config wants.
///
/// A number that will not parse keeps whatever was there, so a half-typed
/// entry cannot blank a setting.
fn parsed(root: &Value, field: &Field, text: &str) -> Value {
    match &field.kind {
        Kind::Toggle => Value::Bool(text == "1"),

        Kind::Number { min, max } => {
            let current = at(root, &field.path).and_then(Value::as_i64).unwrap_or(0);
            let value = text.trim().parse::<i64>().unwrap_or(current);
            Value::from(value.clamp(*min as i64, *max as i64))
        }

        Kind::Decimal { min, max } => {
            let current = at(root, &field.path).and_then(Value::as_f64).unwrap_or(0.0);
            let value = text.trim().parse::<f64>().unwrap_or(current);
            Value::from(value.clamp(*min as f64, *max as f64))
        }

        Kind::Text | Kind::Choice(_) => Value::String(text.trim().to_string()),

        Kind::List => {
            let items: Vec<Value> = text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.to_string()))
                .collect();
            // Never end up with nothing selected; the widget would vanish.
            if items.is_empty() {
                at(root, &field.path)
                    .cloned()
                    .unwrap_or_else(|| Value::Array(vec![Value::String("primary".to_string())]))
            } else {
                Value::Array(items)
            }
        }
    }
}

fn bool_text(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

/// The one settings window, if it is open.
static OPEN: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

struct Window {
    controls: Vec<HWND>,
    /// The rows, in the same order as `controls`.
    fields: Vec<Field>,
    /// The config as it was opened, so a field the window does not show is
    /// carried through untouched.
    base: Value,
    /// Where the intro button points, if there is one.
    portal: Option<String>,
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
/// `fields` is the whole list to show, in order — usually [`core_fields`]
/// followed by whatever the integration adds. `on_save` runs on the UI thread
/// with the edited config, so the caller can apply it immediately rather than
/// making the user restart.
pub fn open(
    config: &Config,
    fields: Vec<Field>,
    intro: Option<Intro>,
    title: &str,
    on_save: Box<dyn Fn(Config)>,
) {
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

        let caption = wide(title);
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

        let dpi = match GetDpiForWindow(hwnd) {
            0 => 96,
            d => d,
        };
        let scale = |v: i32| (v as i64 * dpi as i64 / 96) as i32;

        let font = ui_font(scale(9), false);
        let bold = ui_font(scale(9), true);

        // Everything below works on the serialised form, so a field can
        // address any setting by path without this module knowing its type.
        let base = serde_json::to_value(config).unwrap_or(Value::Null);
        let portal = intro.as_ref().map(|i| i.url.clone());
        let controls = build(hwnd, &base, &fields, intro.as_ref(), scale, font, bold);

        WINDOW.with(|cell| {
            *cell.borrow_mut() = Some(Window {
                controls,
                fields,
                base,
                portal,
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

/// Offer a message to the settings window's dialog-key handling.
///
/// Returns true when the window consumed it, in which case the caller must
/// not translate or dispatch it. False whenever the window is closed, so the
/// host's loop pays a single atomic load for this when it is not open.
pub fn handle_dialog_key(message: &MSG) -> bool {
    use std::sync::atomic::Ordering;

    let open = OPEN.load(Ordering::Relaxed);
    if open == 0 {
        return false;
    }
    unsafe { IsDialogMessageW(HWND(open as *mut std::ffi::c_void), message).as_bool() }
}

/// Create every control and size the window around them.
///
/// Two columns, filled section by section, because thirty settings in one
/// column would be a scrollbar nobody enjoys.
#[allow(clippy::too_many_arguments)]
fn build(
    parent: HWND,
    config: &Value,
    fields: &[Field],
    intro: Option<&Intro>,
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

    let mut controls = vec![HWND::default(); fields.len()];
    let mut extra: Vec<HWND> = Vec::new();

    // Whatever the integration wants said before anything else.
    let mut top = margin;
    if let Some(intro) = intro {
        let button_w = scale(150);
        let intro_h = scale(52);
        extra.push(static_text(
            parent,
            &intro.text,
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
            &intro.button,
            ID_PORTAL,
            Place {
                x: margin + column_w * 2 - button_w,
                y: margin,
                w: button_w,
                h: scale(40),
            },
            font,
        ));
        top = margin + intro_h + scale(8);
    }

    // Group the fields into their sections, keeping declaration order.
    let mut sections: Vec<(&str, Vec<usize>)> = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        match sections.last_mut() {
            Some((name, members)) if *name == field.section => members.push(index),
            _ => sections.push((field.section.as_str(), vec![index])),
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
            let field = &fields[*field_index];
            let value = display(config, field);
            let fy = y[column];

            match &field.kind {
                Kind::Toggle => {
                    let check = checkbox(
                        parent,
                        &field.label,
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
                    extra.push(static_text(parent, &field.label, label_at(x, fy), font));
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
                        for option in options {
                            let text = wide(option);
                            let _ = SendMessageW(
                                combo,
                                CB_ADDSTRING,
                                Some(WPARAM(0)),
                                Some(LPARAM(text.as_ptr() as isize)),
                            );
                        }
                        let selected = options.iter().position(|o| *o == value).unwrap_or(0);
                        let _ = SendMessageW(
                            combo,
                            CB_SETCURSEL,
                            Some(WPARAM(selected)),
                            Some(LPARAM(0)),
                        );
                    }
                    controls[*field_index] = combo;
                }
                Kind::Text | Kind::List | Kind::Number { .. } | Kind::Decimal { .. } => {
                    extra.push(static_text(parent, &field.label, label_at(x, fy), font));
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
/// Read every control back into a config.
///
/// Anything the window does not show survives untouched: the edit happens on
/// the config that was loaded, not on a fresh default.
fn collect(base: &Value, fields: &[Field], controls: &[HWND]) -> Config {
    let mut root = base.clone();

    for (index, field) in fields.iter().enumerate() {
        let Some(hwnd) = controls.get(index) else {
            continue;
        };
        if hwnd.is_invalid() {
            continue;
        }

        let text = match &field.kind {
            Kind::Toggle => bool_text(is_checked(*hwnd)),
            Kind::Choice(options) => unsafe {
                let selected =
                    SendMessageW(*hwnd, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0;
                options
                    .get(selected.max(0) as usize)
                    .cloned()
                    .unwrap_or_default()
            },
            _ => text_of(*hwnd),
        };

        let value = parsed(base, field, &text);
        set_at(&mut root, &field.path, value);
    }

    // A field left in a state that will not deserialise must not throw the
    // whole config away, so fall back to what was opened.
    serde_json::from_value(root)
        .or_else(|_| serde_json::from_value(base.clone()))
        .unwrap_or_default()
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = wparam.0 & 0xFFFF;
                match id {
                    ID_PORTAL => {
                        let Some(portal) =
                            WINDOW.with(|cell| cell.borrow().as_ref().and_then(|w| w.portal.clone()))
                        else {
                            return LRESULT(0);
                        };
                        let url = wide(&portal);
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
                                let (disk, _) = Config::load_or_create();
                                let base = serde_json::to_value(&disk)
                                    .unwrap_or_else(|_| window.base.clone());
                                let edited =
                                    collect(&base, &window.fields, &window.controls);
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
