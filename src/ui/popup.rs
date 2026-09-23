//! A small transient panel that floats above the taskbar.
//!
//! Used for the volume readout. It deliberately does *not* live inside the
//! widget: drawing the readout in place would cover the avatar being adjusted,
//! and the moment that avatar is gone the next scroll notch has nothing to hit.
//! Keeping the widget untouched is what makes repeated scrolling work.
//!
//! Being a top-level window, this one *can* use `UpdateLayeredWindow` — the
//! limitation that forced the widget itself to paint opaquely applies only to
//! child windows. So the popup gets real per-pixel alpha, and with it properly
//! anti-aliased rounded corners over whatever is behind it.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::assets::images::{ImageCache, ImageRef};

use super::render::{Canvas, Color, Font};
use super::theme::Theme;

const CLASS_NAME: windows::core::PCWSTR = w!("DiscordTaskbarPopup");

static REGISTERED: AtomicBool = AtomicBool::new(false);

fn register_class() -> bool {
    if REGISTERED.load(Ordering::Relaxed) {
        return true;
    }

    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return false;
        };

        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };

        let ok = RegisterClassExW(&class) != 0
            || windows::Win32::Foundation::GetLastError()
                == windows::Win32::Foundation::WIN32_ERROR(1410);

        REGISTERED.store(ok, Ordering::Relaxed);
        ok
    }
}

/// Create the popup, hidden.
pub fn create() -> Option<HWND> {
    if !register_class() {
        return None;
    }

    unsafe {
        let instance = GetModuleHandleW(None).ok()?;

        let hwnd = CreateWindowExW(
            // Layered for per-pixel alpha, topmost so the taskbar cannot cover
            // it, no-activate so it never takes focus from what you are doing,
            // and tool-window so it stays out of Alt-Tab.
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("Discord Taskbar Popup"),
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

        (!hwnd.is_invalid()).then_some(hwnd)
    }
}

/// Place the popup at `(x, y)` and push `canvas` to it.
///
/// Position, size and content go in one `UpdateLayeredWindow` call, which a
/// top-level window is free to do. `SetWindowPos` only handles z-order and
/// visibility; it deliberately does not move or resize, because doing that
/// separately leaves a frame showing the old content at the new size.
pub fn present(hwnd: HWND, canvas: &Canvas, x: i32, y: i32) -> bool {
    if !canvas.present_layered_at(hwnd, x, y) {
        return false;
    }

    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }

    true
}

pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

pub fn destroy(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // Transparent to the mouse: the popup sits between the cursor and
            // nothing it should ever intercept, and swallowing clicks near the
            // taskbar would be worse than useless.
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// A one-line readout: a picture, a name, a number and a bar.
///
/// Says nothing about what is being measured. The caller has already decided
/// what the number reads as and how full the bar is, which is what keeps a
/// volume, a download and a battery the same window.
pub struct Meter<'a> {
    pub title: &'a str,
    /// The number on the right, already formatted.
    pub value_text: String,
    /// How full the bar is, 0 to 1.
    pub fraction: f32,
    /// The bar's fill. The caller picks it, so "past normal" can be coloured
    /// differently without this knowing what normal is.
    pub fill: Color,
    /// Shown beside the title; `None` for a readout with no picture.
    pub image: Option<ImageRef>,
    /// Size of that picture at 96 DPI. Worth matching a size the widget
    /// already draws: that bitmap is then cached, so the readout is populated
    /// the instant it appears rather than after a fresh download at a size
    /// nothing else uses.
    pub image_size: i32,
    pub theme: &'a Theme,
    pub font: &'a Font,
    pub images: &'a mut ImageCache,
    pub dpi: u32,
}

impl Meter<'_> {
    fn scale(&self, value: i32) -> i32 {
        (value as i64 * self.dpi as i64 / 96) as i32
    }
}

/// Render the readout, sizing `canvas` to fit. Returns its size.
pub fn draw_meter(canvas: &mut Canvas, view: &mut Meter) -> Option<(i32, i32)> {
    let padding = view.scale(10);
    let avatar = view.scale(view.image_size);
    let gap = view.scale(8);
    let bar_height = view.scale(5);
    let width = view.scale(210);
    let height = view.scale(46);
    let radius = view.scale(8);
    let name_lift = view.scale(1);
    let bar_lift = view.scale(3);

    if !canvas.resize(width, height) {
        return None;
    }

    // Real transparency: this is a top-level window, so unlike the widget the
    // corners can be properly rounded against whatever is behind them.
    canvas.clear();
    canvas.fill_round_rect(
        windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
        radius,
        view.theme.background.with_alpha(0xF2),
    );

    let centre_y = height / 2;

    if let Some(image) = view.image.clone() {
        let cx = padding as f32 + avatar as f32 / 2.0;
        match view.images.image(&image, avatar as u32) {
            Some(bitmap) => {
                let bitmap = bitmap.clone();
                canvas.draw_circular_bitmap(&bitmap, cx, centre_y as f32, avatar as f32, 1.0);
            }
            None => canvas.fill_circle(
                cx,
                centre_y as f32,
                avatar as f32 / 2.0,
                view.theme.placeholder,
            ),
        }
    }

    let text_x = padding + avatar + gap;
    let text_width = width - text_x - padding;

    // Title on the left, value right-aligned on the same line.
    let value_width = canvas.measure_text(&view.value_text, view.font).0;
    let title_width = (text_width - value_width - gap).max(0);
    let text_y = centre_y - view.font.height - name_lift;

    canvas.draw_text_ellipsised(
        view.title,
        view.font,
        text_x,
        text_y,
        title_width,
        view.theme.text,
    );
    canvas.draw_text(
        &view.value_text,
        view.font,
        text_x + text_width - value_width,
        text_y,
        view.theme.text_dim,
    );

    // Track, then fill. What the full width means is the caller's business:
    // for volume a 200% ceiling puts normal at the midpoint, so boosting past
    // it is visibly past halfway rather than hidden at the end of the bar.
    let bar_y = centre_y + bar_lift;
    canvas.fill_round_rect(
        windows::Win32::Foundation::RECT {
            left: text_x,
            top: bar_y,
            right: text_x + text_width,
            bottom: bar_y + bar_height,
        },
        bar_height / 2,
        view.theme.divider,
    );

    let filled = (text_width as f32 * view.fraction.clamp(0.0, 1.0)).round() as i32;
    if filled > 0 {
        canvas.fill_round_rect(
            windows::Win32::Foundation::RECT {
                left: text_x,
                top: bar_y,
                right: text_x + filled.min(text_width),
                bottom: bar_y + bar_height,
            },
            bar_height / 2,
            view.fill,
        );
    }

    Some((width, height))
}
