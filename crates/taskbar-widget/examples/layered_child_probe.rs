//! Diagnostic: can a *child* of the taskbar hold per-pixel alpha?
//!
//! This decides the whole transparency design. If `UpdateLayeredWindow` works
//! on a child of `Shell_TrayWnd`, the widget can be genuinely transparent
//! while still living inside the taskbar — clipped, hidden and z-ordered by
//! the shell for free. If it does not, transparency costs us the parent, and
//! with it everything the shell was doing on our behalf.
//!
//! `WS_EX_LAYERED` has been documented as supported on child windows since
//! Windows 8, but "the style is accepted" and "UpdateLayeredWindow actually
//! composites" are different claims. TrafficMonitor records an
//! `update_layered_window_error_code` and disables its Direct2D path when it
//! is set, which suggests the second one does not hold. This checks directly.
//!
//! Run with the taskbar visible; it prints what happened and leaves the window
//! up for a few seconds so it can be looked at.

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC, GetPixel};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW,
    GetWindowRect, RegisterClassExW, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    UpdateLayeredWindow, GWL_EXSTYLE, GWL_STYLE, HWND_TOP,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOW, ULW_ALPHA, WNDCLASSEXW, WS_CHILD,
    WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

use taskbar_widget::ui::render::Canvas;
use taskbar_widget::ui::taskbar;

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
}

const SIZE_W: i32 = 240;
const SIZE_H: i32 = 30;

fn main() {
    let Some(info) = taskbar::find() else {
        println!("no taskbar found; is explorer running?");
        return;
    };
    println!(
        "taskbar at ({},{}) {}x{}",
        info.rect.left,
        info.rect.top,
        info.width(),
        info.height()
    );

    // Somewhere in the middle of the bar, clear of the Start button.
    let x = info.rect.left + info.width() / 2;
    let y = info.rect.top + (info.height() - SIZE_H) / 2;

    // A canvas that is half-transparent red: if alpha is honoured the taskbar
    // tints through it, and if it is ignored we get a flat red block.
    let mut canvas = Canvas::new(SIZE_W, SIZE_H).expect("canvas");
    for px in 0..SIZE_W {
        for py in 0..SIZE_H {
            // Premultiplied: 50% alpha over pure red.
            canvas.blend(px, py, 0x8080_0000);
        }
    }

    for parented in [true, false] {
        println!(
            "\n--- {} ---",
            if parented {
                "child of Shell_TrayWnd"
            } else {
                "top-level, for comparison"
            }
        );
        probe(&info, &canvas, x, y, parented);
    }
}

fn probe(info: &taskbar::TaskbarInfo, canvas: &Canvas, x: i32, y: i32, parented: bool) {
    unsafe {
        let instance = GetModuleHandleW(None).expect("module");
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("LayeredChildProbe"),
            ..Default::default()
        };
        RegisterClassExW(&class);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("LayeredChildProbe"),
            w!("probe"),
            WS_POPUP,
            x,
            y,
            SIZE_W,
            SIZE_H,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("create");

        let (px, py) = if parented {
            if SetParent(hwnd, Some(info.tray)).is_err() {
                println!("SetParent failed; the shell refused to adopt us");
                let _ = DestroyWindow(hwnd);
                return;
            }
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
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            // A child's position is in the parent's client space.
            taskbar::screen_to_parent(info.tray, x, y)
        } else {
            (x, y)
        };

        // Confirm the style actually stuck; the docs say it is allowed on a
        // child since Windows 8, but that is worth checking rather than
        // assuming.
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        println!(
            "WS_EX_LAYERED present after setup: {}",
            ex & WS_EX_LAYERED.0 != 0
        );

        let _ = ShowWindow(hwnd, SW_SHOW);

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let source = POINT { x: 0, y: 0 };
        let destination = POINT { x: px, y: py };
        let size = SIZE {
            cx: SIZE_W,
            cy: SIZE_H,
        };

        let result = UpdateLayeredWindow(
            hwnd,
            None,
            Some(&destination),
            Some(&size),
            Some(canvas.hdc()),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        match &result {
            Ok(()) => println!("UpdateLayeredWindow: Ok"),
            Err(e) => println!("UpdateLayeredWindow: FAILED {e}"),
        }

        // The call returning Ok is not the same as the pixels being blended.
        // Sample where the window actually ended up rather than where it was
        // asked to go: for a child, UpdateLayeredWindow's destination point is
        // not obviously in the same space as CreateWindowEx's.
        std::thread::sleep(std::time::Duration::from_millis(600));

        let mut placed = RECT::default();
        let _ = GetWindowRect(hwnd, &mut placed);
        println!(
            "window ended up at ({},{}) {}x{}",
            placed.left,
            placed.top,
            placed.right - placed.left,
            placed.bottom - placed.top
        );

        let sx = (placed.left + placed.right) / 2;
        let sy = (placed.top + placed.bottom) / 2;

        let screen = GetDC(None);
        let over = GetPixel(screen, sx, sy);
        // A reference reading well clear of the window, for comparison.
        let bare = GetPixel(screen, info.rect.left + 40, sy);
        ReleaseDC(None, screen);

        let (r, g, b) = (over.0 & 0xFF, (over.0 >> 8) & 0xFF, (over.0 >> 16) & 0xFF);
        let (br, bg, bb) = (bare.0 & 0xFF, (bare.0 >> 8) & 0xFF, (bare.0 >> 16) & 0xFF);
        println!("over the window : R={r} G={g} B={b}");
        println!("bare taskbar    : R={br} G={bg} B={bb}");

        // Premultiplied 50% red contributes 128 to red and nothing to the
        // others, so a real composite reads R >= ~120 and keeps some of the
        // taskbar in green and blue.
        if r >= 200 && g < 40 && b < 40 {
            println!("  -> OPAQUE RED: the style is on but alpha was ignored");
        } else if r >= 120 && r > br + 30 {
            println!("  -> BLENDED: per-pixel alpha WORKS here");
        } else {
            println!("  -> NOT DRAWN: reads like the bare taskbar; no compositing");
        }

        std::thread::sleep(std::time::Duration::from_millis(1200));
        let _ = DestroyWindow(hwnd);
    }
}
