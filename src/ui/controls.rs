//! Thin wrappers over the standard Win32 controls.
//!
//! Shared by the settings window and the installer, which both want an
//! ordinary-looking Windows dialog rather than the drawn interface the widget
//! uses inside the taskbar.
//!
//! Nothing here abstracts much — it exists so that creating a control is one
//! readable line instead of a fourteen-argument `CreateWindowExW`.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, ANSI_CHARSET, CLIP_DEFAULT_PRECIS, DEFAULT_PITCH, FF_DONTCARE, FW_NORMAL,
    FW_SEMIBOLD, HFONT, OUT_TT_PRECIS, PROOF_QUALITY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Where a control goes. Passing the four numbers loose made every helper
/// below read as an unlabelled pile of integers at the call site.
#[derive(Clone, Copy, Debug, Default)]
pub struct Place {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A UTF-16, NUL-terminated copy of `text`, as every `*W` entry point wants.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Segoe UI at `height` device pixels, the system's own dialog face.
pub fn ui_font(height: i32, bold: bool) -> HFONT {
    unsafe {
        let name = wide("Segoe UI");
        CreateFontW(
            -height,
            0,
            0,
            0,
            if bold {
                FW_SEMIBOLD.0 as i32
            } else {
                FW_NORMAL.0 as i32
            },
            0,
            0,
            0,
            ANSI_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            PROOF_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(name.as_ptr()),
        )
    }
}

pub fn child(
    parent: HWND,
    class: PCWSTR,
    text: &str,
    style: WINDOW_STYLE,
    id: usize,
    at: Place,
    font: HFONT,
) -> HWND {
    unsafe {
        let caption = wide(text);
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(caption.as_ptr()),
            style | WS_CHILD | WS_VISIBLE,
            at.x,
            at.y,
            at.w,
            at.h,
            Some(parent),
            Some(HMENU(id as *mut std::ffi::c_void)),
            Some(instance.into()),
            None,
        )
        .unwrap_or_default();

        let _ = SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        hwnd
    }
}

pub fn static_text(p: HWND, t: &str, at: Place, f: HFONT) -> HWND {
    child(p, w!("STATIC"), t, WINDOW_STYLE(0), 0, at, f)
}

pub fn edit(p: HWND, t: &str, id: usize, at: Place, f: HFONT) -> HWND {
    child(
        p,
        w!("EDIT"),
        t,
        WINDOW_STYLE(WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32),
        id,
        at,
        f,
    )
}

pub fn checkbox(p: HWND, t: &str, id: usize, at: Place, f: HFONT) -> HWND {
    child(
        p,
        w!("BUTTON"),
        t,
        WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0),
        id,
        at,
        f,
    )
}

pub fn button(p: HWND, t: &str, id: usize, at: Place, f: HFONT) -> HWND {
    child(
        p,
        w!("BUTTON"),
        t,
        WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
        id,
        at,
        f,
    )
}

pub fn combo(p: HWND, id: usize, at: Place, f: HFONT) -> HWND {
    child(
        p,
        w!("COMBOBOX"),
        "",
        WINDOW_STYLE(CBS_DROPDOWNLIST as u32 | WS_TABSTOP.0 | WS_VSCROLL.0),
        id,
        at,
        f,
    )
}

/// Read a control's text back.
pub fn text_of(hwnd: HWND) -> String {
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..copied as usize])
    }
}

/// Whether a checkbox is ticked.
pub fn is_checked(hwnd: HWND) -> bool {
    // 1 is `BST_CHECKED`, which windows-rs does not re-export here.
    unsafe { SendMessageW(hwnd, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 == 1 }
}

/// Tick or clear a checkbox.
pub fn set_checked(hwnd: HWND, checked: bool) {
    unsafe {
        let _ = SendMessageW(
            hwnd,
            BM_SETCHECK,
            Some(WPARAM(usize::from(checked))),
            Some(LPARAM(0)),
        );
    }
}
