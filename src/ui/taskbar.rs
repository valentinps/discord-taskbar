//! Finding the taskbar and working out where to sit inside it.
//!
//! The approach is TrafficMonitor's: locate `Shell_TrayWnd` and become a child
//! of it. That works with StartAllBack's legacy taskbar because the classic
//! `Shell_TrayWnd` / `ReBarWindow32` / `TrayNotifyWnd` hierarchy is still what
//! gets created — only the painting is replaced.

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, ScreenToClient, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};

use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GetWindowRect, IsWindow,
};

pub const DEFAULT_DPI: u32 = 96;

/// Which screen edge the taskbar is docked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

impl Edge {
    pub fn is_horizontal(self) -> bool {
        matches!(self, Edge::Bottom | Edge::Top)
    }
}

#[derive(Debug, Clone)]
pub struct TaskbarInfo {
    /// `Shell_TrayWnd` on the primary display, `Shell_SecondaryTrayWnd`
    /// elsewhere — whichever we become a child of.
    pub tray: HWND,
    /// The display device this bar belongs to, e.g. `\.\DISPLAY1`. Stable
    /// across restarts, unlike an enumeration index.
    pub monitor: String,
    /// Position in the system's monitor enumeration, for humans picking one.
    pub monitor_index: usize,
    pub is_primary: bool,
    /// Taskbar bounds in screen coordinates.
    pub rect: RECT,
    /// `TrayNotifyWnd` (clock + notification area) in screen coordinates.
    pub notify_rect: Option<RECT>,
    /// `ReBarWindow32` (the task button strip) in screen coordinates.
    pub rebar_rect: Option<RECT>,
    pub edge: Edge,
    pub dpi: u32,
}

impl TaskbarInfo {
    pub fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }

    pub fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }

    /// Scale a 96-DPI design value to the taskbar's DPI.
    pub fn scale(&self, value: i32) -> i32 {
        (value as i64 * self.dpi as i64 / DEFAULT_DPI as i64) as i32
    }
}

/// Locate the primary taskbar. Returns `None` when explorer is not up yet.
pub fn find() -> Option<TaskbarInfo> {
    unsafe {
        let tray = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        if tray.is_invalid() || !IsWindow(Some(tray)).as_bool() {
            return None;
        }

        let mut rect = RECT::default();
        GetWindowRect(tray, &mut rect).ok()?;
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return None;
        }

        let notify_rect = child_rect(tray, w!("TrayNotifyWnd"));
        let rebar_rect = child_rect(tray, w!("ReBarWindow32"));

        let dpi = match GetDpiForWindow(tray) {
            0 => DEFAULT_DPI,
            d => d,
        };

        let (monitor, is_primary) = monitor_of(tray);

        Some(TaskbarInfo {
            tray,
            monitor,
            monitor_index: 0,
            is_primary,
            rect,
            notify_rect,
            rebar_rect,
            edge: detect_edge(tray, &rect),
            dpi,
        })
    }
}

/// Every taskbar on the system: the primary bar plus one per secondary
/// display, in monitor enumeration order.
pub fn find_all() -> Vec<TaskbarInfo> {
    let mut bars = Vec::new();

    unsafe {
        if let Some(primary) = find() {
            bars.push(primary);
        }

        // Secondary bars all share one class, so walk the chain.
        let mut previous: Option<HWND> = None;
        loop {
            let Ok(next) = FindWindowExW(
                None,
                previous.map(Some).unwrap_or(None),
                w!("Shell_SecondaryTrayWnd"),
                None,
            ) else {
                break;
            };
            if next.is_invalid() {
                break;
            }
            previous = Some(next);

            if let Some(info) = describe(next) {
                bars.push(info);
            }
        }
    }

    // Number them by where their monitor sits in the system enumeration, so
    // the indices a user picks in the config match what Windows shows them.
    let order = monitor_order();
    for bar in &mut bars {
        bar.monitor_index = order
            .iter()
            .position(|device| *device == bar.monitor)
            .unwrap_or(usize::MAX);
    }
    bars.sort_by_key(|bar| bar.monitor_index);

    bars
}

/// Build a `TaskbarInfo` for a specific bar window.
fn describe(tray: HWND) -> Option<TaskbarInfo> {
    unsafe {
        if tray.is_invalid() || !IsWindow(Some(tray)).as_bool() {
            return None;
        }

        let mut rect = RECT::default();
        GetWindowRect(tray, &mut rect).ok()?;
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return None;
        }

        let dpi = match GetDpiForWindow(tray) {
            0 => DEFAULT_DPI,
            d => d,
        };
        let (monitor, is_primary) = monitor_of(tray);

        Some(TaskbarInfo {
            tray,
            monitor,
            monitor_index: 0,
            is_primary,
            rect,
            // A secondary bar has its own notification area class; treating it
            // as absent just means the widget may centre across it, which the
            // clamp already handles.
            notify_rect: child_rect(tray, w!("TrayNotifyWnd")),
            rebar_rect: child_rect(tray, w!("ReBarWindow32")),
            edge: detect_edge(tray, &rect),
            dpi,
        })
    }
}

/// The display device a window sits on, and whether it is the primary.
unsafe fn monitor_of(hwnd: HWND) -> (String, bool) {
    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    if !GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool() {
        return (String::new(), false);
    }

    let name = String::from_utf16_lossy(&info.szDevice)
        .trim_end_matches('\0')
        .to_string();
    // MONITORINFOF_PRIMARY is not exposed by the bindings; it is 1.
    (name, info.monitorInfo.dwFlags & 1 != 0)
}

/// Display device names in system enumeration order.
pub fn monitor_order() -> Vec<String> {
    unsafe extern "system" fn visit(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> windows::core::BOOL {
        unsafe {
            let names = &mut *(data.0 as *mut Vec<String>);
            let mut info = MONITORINFOEXW {
                monitorInfo: MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool() {
                names.push(
                    String::from_utf16_lossy(&info.szDevice)
                        .trim_end_matches('\0')
                        .to_string(),
                );
            }
            true.into()
        }
    }

    let mut names: Vec<String> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(visit),
            LPARAM(&mut names as *mut Vec<String> as isize),
        );
    }
    names
}

unsafe fn child_rect(parent: HWND, class: windows::core::PCWSTR) -> Option<RECT> {
    let child = FindWindowExW(Some(parent), None, class, None).ok()?;
    if child.is_invalid() {
        return None;
    }
    let mut rect = RECT::default();
    GetWindowRect(child, &mut rect).ok()?;
    Some(rect)
}

unsafe fn detect_edge(tray: HWND, rect: &RECT) -> Edge {
    let monitor = MonitorFromWindow(tray, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };

    if !GetMonitorInfoW(monitor, &mut info).as_bool() {
        return Edge::Bottom;
    }

    let screen = info.rcMonitor;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    // A horizontal bar spans the monitor's width; a vertical one its height.
    if width >= height {
        if rect.top - screen.top <= screen.bottom - rect.bottom {
            Edge::Top
        } else {
            Edge::Bottom
        }
    } else if rect.left - screen.left <= screen.right - rect.right {
        Edge::Left
    } else {
        Edge::Right
    }
}

/// Horizontal placement rules for the widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    /// Centred on the taskbar, clamped so it never covers the clock.
    #[default]
    Center,
    /// Hard against the notification area.
    RightOfTasks,
    /// Hard against the left edge, after the Start button.
    Left,
}

/// Where the widget should sit, in screen coordinates.
///
/// Only horizontal taskbars are handled for now; a vertical taskbar falls back
/// to centring in the bar's long axis, which is at least not wrong.
pub fn place(
    info: &TaskbarInfo,
    size: (i32, i32),
    anchor: Anchor,
    offset: (i32, i32),
    margin: i32,
) -> (i32, i32) {
    let (w, h) = size;

    if !info.edge.is_horizontal() {
        let x = info.rect.left + (info.width() - w) / 2 + offset.0;
        let y = info.rect.top + (info.height() - h) / 2 + offset.1;
        return (x, y);
    }

    // Right-hand limit is the clock / notification area when we can see it.
    let right_limit = info
        .notify_rect
        .map(|r| r.left)
        .unwrap_or(info.rect.right)
        - margin;
    let left_limit = info.rect.left + margin;

    let mut x = match anchor {
        Anchor::Center => (info.rect.left + info.rect.right) / 2 - w / 2,
        Anchor::RightOfTasks => right_limit - w,
        Anchor::Left => left_limit,
    } + offset.0;

    // Clamp, preferring to stay clear of the clock if space is tight.
    if x + w > right_limit {
        x = right_limit - w;
    }
    if x < left_limit {
        x = left_limit;
    }

    let y = info.rect.top + (info.height() - h) / 2 + offset.1;
    (x, y)
}

/// Convert screen coordinates to the tray window's client space, which is what
/// `SetWindowPos` wants for a child of the taskbar.
pub fn screen_to_parent(parent: HWND, x: i32, y: i32) -> (i32, i32) {
    unsafe {
        let mut point = POINT { x, y };
        if ScreenToClient(parent, &mut point).as_bool() {
            (point.x, point.y)
        } else {
            (x, y)
        }
    }
}


