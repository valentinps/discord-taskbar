//! The taskbar user interface.
//!
//! Two windows cooperate:
//!
//! * The **host** is a hidden top-level window. It owns all state, the timer
//!   and the tray icon. It must be top-level rather than message-only because
//!   `TaskbarCreated` is a broadcast, and broadcasts only reach top-level
//!   windows — that message is how we recover from an explorer restart.
//! * The **widget** is a `WS_CHILD` of `Shell_TrayWnd`. Explorer destroys it
//!   when it restarts, which is fine: the host simply builds another.

pub mod controls;
pub mod doctor;
pub mod elements;
pub mod host;
pub mod menu;
pub mod popup;
pub mod render;
pub mod settings;
pub mod taskbar;
pub mod theme;
pub mod tray;
pub mod widget;

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// A new status snapshot is ready (`wparam` carries a boxed snapshot).
pub const WM_APP_STATUS: u32 = WM_APP + 1;
/// The widget was left-clicked.
pub const WM_APP_WIDGET_CLICK: u32 = WM_APP + 2;
/// The widget was right-clicked; `lparam` packs the screen coordinates.
pub const WM_APP_WIDGET_CONTEXT: u32 = WM_APP + 3;

/// The widget is painting; `wparam` carries the `HDC` from `BeginPaint`.
pub const WM_APP_WIDGET_PAINT: u32 = WM_APP + 4;

/// A background fetch finished and the widget should repaint.
pub const WM_APP_ASSET_READY: u32 = WM_APP + 5;
/// Notification-area icon callback.
pub const WM_APP_TRAY: u32 = WM_APP + 6;
/// The wheel turned over the widget; `wparam` is the notch count, `lparam` the
/// position in widget client coordinates.
pub const WM_APP_WIDGET_WHEEL: u32 = WM_APP + 7;
/// The middle button was released over the widget.
pub const WM_APP_WIDGET_MIDDLE: u32 = WM_APP + 8;
/// Asks whether the point in `lparam` is interactive, for cursor shaping.
/// Replies non-zero when it is.
pub const WM_APP_WIDGET_CURSOR: u32 = WM_APP + 9;
/// The pointer moved over the widget; `lparam` carries its position.
pub const WM_APP_WIDGET_HOVER: u32 = WM_APP + 10;
/// The pointer left the widget entirely.
pub const WM_APP_WIDGET_LEAVE: u32 = WM_APP + 11;

/// Posts a bare message to a window from any thread.
///
/// Used by worker threads that only need to say "something changed" without
/// carrying a payload.
#[derive(Clone)]
pub struct Notifier {
    target: Arc<AtomicIsize>,
    message: u32,
}

impl Notifier {
    pub fn new(target: HWND, message: u32) -> Self {
        Notifier {
            target: Arc::new(AtomicIsize::new(target.0 as isize)),
            message,
        }
    }

    pub fn notify(&self) {
        let raw = self.target.load(Ordering::Relaxed);
        if raw == 0 {
            return;
        }
        unsafe {
            let _ = PostMessageW(
                Some(HWND(raw as *mut std::ffi::c_void)),
                self.message,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// An input event from a widget.
///
/// Boxed and passed by pointer in `wparam`, because with one widget per
/// monitor the message has to say which window it came from, and a window
/// handle plus a position does not fit in the two parameters available.
pub struct WidgetInput {
    pub widget: HWND,
    pub point: windows::Win32::Foundation::POINT,
    /// Wheel notches; zero for everything else.
    pub notches: i32,
}

/// Reclaim a `WidgetInput` sent with a widget message.
///
/// # Safety
/// `wparam` must come from one of those messages and must not be taken twice.
pub unsafe fn take_widget_input(wparam: usize) -> Option<Box<WidgetInput>> {
    if wparam == 0 {
        return None;
    }
    Some(Box::from_raw(wparam as *mut WidgetInput))
}

/// Re-anchors the widget and notices when the taskbar has gone away.
pub const TIMER_ANCHOR: usize = 1;
pub const TIMER_ANCHOR_MS: u32 = 1000;
