//! The visible window that lives inside the taskbar.
//!
//! It holds no state of its own: the host owns everything, and this window
//! just forwards input. That keeps recreation after an explorer restart cheap
//! — we throw the `HWND` away and make another.

use std::collections::HashSet;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, ScreenToClient, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
// WM_MOUSELEAVE lives in UI::Controls, not WindowsAndMessaging — without
// this import it silently becomes a catch-all binding in the match below.
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{
    WidgetInput, WM_APP_WIDGET_CLICK, WM_APP_WIDGET_CONTEXT, WM_APP_WIDGET_CURSOR,
    WM_APP_WIDGET_HOVER, WM_APP_WIDGET_LEAVE, WM_APP_WIDGET_MIDDLE, WM_APP_WIDGET_PAINT,
    WM_APP_WIDGET_WHEEL,
};

const CLASS_NAME: windows::core::PCWSTR = w!("DiscordTaskbarWidget");

/// The host window, so input can be forwarded to whoever owns the state.
static HOST: AtomicIsize = AtomicIsize::new(0);

/// Which widgets `TrackMouseEvent` is currently armed for.
///
/// Per window, not a single flag: `TrackMouseEvent` tracks one `HWND`, and
/// with one widget per taskbar a shared flag meant the second widget the
/// pointer entered saw the first one's arming and skipped its own. It then
/// never received `WM_MOUSELEAVE`, so the volume readout could only be
/// dismissed by the host's once-a-second safety net.
static TRACKING: Mutex<Option<HashSet<isize>>> = Mutex::new(None);

/// Arm leave-tracking for `hwnd` unless it already is. Returns whether the
/// caller now needs to call `TrackMouseEvent`.
fn arm_tracking(hwnd: HWND) -> bool {
    match TRACKING.lock() {
        Ok(mut set) => set.get_or_insert_with(HashSet::new).insert(hwnd.0 as isize),
        // A poisoned lock is not a reason to stop tracking the mouse; arming
        // twice is harmless, since Windows ignores a redundant request.
        Err(_) => true,
    }
}

fn disarm_tracking(hwnd: HWND) {
    if let Ok(mut set) = TRACKING.lock() {
        if let Some(set) = set.as_mut() {
            set.remove(&(hwnd.0 as isize));
        }
    }
}

pub fn set_host(hwnd: HWND) {
    HOST.store(hwnd.0 as isize, Ordering::Relaxed);
}

fn host() -> Option<HWND> {
    match HOST.load(Ordering::Relaxed) {
        0 => None,
        raw => Some(HWND(raw as *mut std::ffi::c_void)),
    }
}

pub fn register_class() -> bool {
    unsafe {
        let instance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => return false,
        };

        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };

        // A non-zero atom means success; a duplicate registration is harmless.
        RegisterClassExW(&class) != 0 || class_already_registered()
    }
}

/// `ERROR_CLASS_ALREADY_EXISTS` — harmless, and expected if we re-register
/// after an explorer restart.
unsafe fn class_already_registered() -> bool {
    windows::Win32::Foundation::GetLastError()
        == windows::Win32::Foundation::WIN32_ERROR(1410)
}

/// Create the widget as a top-level window, to be adopted by the taskbar.
///
/// It is deliberately *not* created with the taskbar as its parent: passing a
/// cross-process `HWND` to `CreateWindowExW` with `WS_CHILD` fails here. The
/// two-step create-then-`SetParent` dance is what TrafficMonitor does, and it
/// is what actually works.
///
/// Always layered: the widget is presented with `UpdateLayeredWindow` so the
/// taskbar shows through it.
///
/// That works on a child window since Windows 8, which is what lets the widget
/// be genuinely transparent while still living inside `Shell_TrayWnd` and
/// being clipped, hidden and z-ordered by the shell along with it. See
/// `examples/layered_child_probe.rs`, which checks it on the machine it runs
/// on.
pub fn create() -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Discord Status"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;

        if hwnd.is_invalid() {
            return None;
        }

        Some(hwnd)
    }
}

/// Adopt the taskbar as our parent. Returns false if the shell refused, in
/// which case the caller should leave the widget floating on top instead.
pub fn attach(hwnd: HWND, parent: HWND) -> bool {
    unsafe {
        if SetParent(hwnd, Some(parent)).is_err() {
            return false;
        }

        // SetParent alone does not change the style bits; without WS_CHILD the
        // window keeps being treated as a popup and clips against the desktop.
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            (WS_CHILD | WS_CLIPSIBLINGS | WS_VISIBLE).0 as isize,
        );

        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );

        true
    }
}

/// Keep the floating fallback above the taskbar.
pub fn make_topmost(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Show without stealing focus from whatever the user is doing.
pub fn show(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
}

/// Hide entirely, so an idle widget occupies no taskbar space and paints
/// nothing at all.
pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

pub fn destroy(hwnd: HWND) {
    // Drop the tracking entry first: handles are recycled, so leaving a dead
    // one behind could make a future widget at the same address skip arming.
    disarm_tracking(hwnd);
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// A mouse position packed into an `LPARAM`, sign-extended — a widget can be
/// hit at negative client coordinates. The packing is the same whether the
/// message reports client or screen coordinates.
fn client_point(lparam: LPARAM) -> POINT {
    let raw = lparam.0 as i32;
    POINT {
        x: ((raw & 0xFFFF) << 16) >> 16,
        y: (((raw >> 16) & 0xFFFF) << 16) >> 16,
    }
}

/// Hand an input event to the host.
///
/// The payload is boxed and the pointer travels in `wparam`; the host reclaims
/// it. Packing a position into an `LPARAM` was fine with one widget, but with
/// one per monitor the source window has to travel too, and that does not fit.
fn post_input(hwnd: HWND, message: u32, point: POINT, notches: i32) {
    let Some(host) = host() else {
        return;
    };

    let boxed = Box::into_raw(Box::new(WidgetInput {
        widget: hwnd,
        point,
        notches,
    }));

    let posted =
        unsafe { PostMessageW(Some(host), message, WPARAM(boxed as usize), LPARAM(0)).is_ok() };

    if !posted {
        drop(unsafe { Box::from_raw(boxed) });
    }
}

/// Same, but synchronous, for the questions that need an answer back.
fn send_input(hwnd: HWND, message: u32, point: POINT, notches: i32) -> isize {
    let Some(host) = host() else {
        return 0;
    };

    let boxed = Box::into_raw(Box::new(WidgetInput {
        widget: hwnd,
        point,
        notches,
    }));

    unsafe {
        SendMessageW(host, message, Some(WPARAM(boxed as usize)), Some(LPARAM(0))).0
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // Report as client area so clicks reach us rather than the taskbar.
            WM_NCHITTEST => LRESULT(HTCLIENT as isize),

            WM_LBUTTONUP => {
                post_input(hwnd, WM_APP_WIDGET_CLICK, client_point(lparam), 0);
                LRESULT(0)
            }

            WM_RBUTTONUP => {
                post_input(hwnd, WM_APP_WIDGET_CONTEXT, client_point(lparam), 0);
                LRESULT(0)
            }

            // Tracking ends when WM_MOUSELEAVE fires, so it is armed once per
            // entry rather than on every move.
            WM_MOUSEMOVE => {
                if arm_tracking(hwnd) {
                    let mut tracking = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tracking);
                }

                post_input(hwnd, WM_APP_WIDGET_HOVER, client_point(lparam), 0);
                LRESULT(0)
            }

            WM_MOUSELEAVE => {
                disarm_tracking(hwnd);
                post_input(hwnd, WM_APP_WIDGET_LEAVE, POINT::default(), 0);
                LRESULT(0)
            }

            WM_MBUTTONUP => {
                post_input(hwnd, WM_APP_WIDGET_MIDDLE, client_point(lparam), 0);
                LRESULT(0)
            }

            // The wheel reports in screen coordinates, unlike the button
            // messages, so convert before handing it on.
            WM_MOUSEWHEEL => {
                let notches = (wparam.0 >> 16) as i16 as i32 / WHEEL_DELTA as i32;
                let mut point = client_point(lparam);
                let _ = ScreenToClient(hwnd, &mut point);
                post_input(hwnd, WM_APP_WIDGET_WHEEL, point, notches);
                LRESULT(0)
            }

            // A hand over anything clickable, so the interactive parts of the
            // widget advertise themselves.
            WM_SETCURSOR => {
                let mut point = POINT::default();
                let interactive = if GetCursorPos(&mut point).is_ok() {
                    let _ = ScreenToClient(hwnd, &mut point);
                    send_input(hwnd, WM_APP_WIDGET_CURSOR, point, 0) != 0
                } else {
                    false
                };

                if interactive {
                    if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(Some(hand));
                        return LRESULT(1);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            // The shell does not composite a layered *child* window, so the
            // widget paints itself opaquely over the taskbar's own colour.
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);

                let mut painted = false;
                if !dc.is_invalid() {
                    if let Some(host) = host() {
                        // Synchronous: the DC is only valid until EndPaint.
                        painted = SendMessageW(
                            host,
                            WM_APP_WIDGET_PAINT,
                            Some(WPARAM(dc.0 as usize)),
                            // Which widget is painting: there is one per
                            // taskbar once multiple monitors are in play.
                            Some(LPARAM(hwnd.0 as isize)),
                        )
                        .0 != 0;
                    }
                }
                let _ = EndPaint(hwnd, &ps);

                // The host refuses to paint while it is mid-refresh, which
                // happens because SetWindowPos delivers WM_PAINT synchronously.
                // EndPaint has already validated the region, so without this the
                // frame would simply be lost, leaving the widget showing stale
                // state until something else happened to invalidate it.
                if !painted {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(
                        Some(hwnd),
                        None,
                        false,
                    );
                }

                LRESULT(0)
            }

            WM_ERASEBKGND => LRESULT(1),

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
