//! Diagnostic: why will the shell not adopt our window?
//!
//! Tries each parenting strategy in turn and reports the exact Win32 error,
//! plus the integrity levels involved (UIPI blocks a lower-integrity process
//! from parenting into a higher-integrity one).

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TokenIntegrityLevel, TOKEN_QUERY};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    unsafe {
        let tray = match FindWindowW(w!("Shell_TrayWnd"), None) {
            Ok(h) if !h.is_invalid() => h,
            _ => {
                println!("no Shell_TrayWnd");
                return;
            }
        };

        let mut explorer_pid = 0u32;
        let explorer_thread = GetWindowThreadProcessId(tray, Some(&mut explorer_pid));
        println!("Shell_TrayWnd = {tray:?}  explorer pid={explorer_pid} thread={explorer_thread}");
        println!("our pid={}  thread={}", std::process::id(), GetCurrentThreadId());
        println!();

        println!("our integrity      = {}", integrity_self());
        println!("explorer integrity = {}", integrity_of(explorer_pid));
        println!();

        // Strategy 1: create as a popup, then SetParent.
        let a = make_window(WS_POPUP, None);
        println!("[1] popup + SetParent");
        match SetParent(a, Some(tray)) {
            Ok(previous) => println!("    OK (previous parent = {previous:?})"),
            Err(e) => println!("    FAILED: {e}  (GetLastError={:?})", GetLastError()),
        }
        println!("    GetParent -> {:?}", GetParent(a));
        let _ = DestroyWindow(a);

        // Strategy 2: attach to explorer's input queue first.
        let b = make_window(WS_POPUP, None);
        println!("[2] AttachThreadInput + SetParent");
        let attached = AttachThreadInput(GetCurrentThreadId(), explorer_thread, true).as_bool();
        println!("    AttachThreadInput -> {attached}");
        match SetParent(b, Some(tray)) {
            Ok(_) => println!("    OK"),
            Err(e) => println!("    FAILED: {e}  (GetLastError={:?})", GetLastError()),
        }
        println!("    GetParent -> {:?}", GetParent(b));
        let _ = AttachThreadInput(GetCurrentThreadId(), explorer_thread, false);
        let _ = DestroyWindow(b);

        // Strategy 3: WS_CHILD with the tray passed straight to CreateWindowExW.
        println!("[3] CreateWindowExW with WS_CHILD and hWndParent=tray");
        let c = make_window(WS_CHILD | WS_CLIPSIBLINGS, Some(tray));
        if c.is_invalid() {
            println!("    FAILED to create: GetLastError={:?}", GetLastError());
        } else {
            println!("    created {c:?}, GetParent -> {:?}", GetParent(c));
            let _ = DestroyWindow(c);
        }

        // Strategy 4: parent onto the rebar instead of the tray itself.
        if let Ok(rebar) = FindWindowExW(Some(tray), None, w!("ReBarWindow32"), None) {
            println!("[4] popup + SetParent onto ReBarWindow32 ({rebar:?})");
            let d = make_window(WS_POPUP, None);
            match SetParent(d, Some(rebar)) {
                Ok(_) => println!("    OK"),
                Err(e) => println!("    FAILED: {e}  (GetLastError={:?})", GetLastError()),
            }
            println!("    GetParent -> {:?}", GetParent(d));
            let _ = DestroyWindow(d);
        }
    }
}

extern "system" fn probe_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe fn make_window(style: WINDOW_STYLE, parent: Option<HWND>) -> HWND {
    let instance = GetModuleHandleW(None).unwrap();
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(probe_wndproc),
        hInstance: instance.into(),
        lpszClassName: w!("ParentProbeClass"),
        ..Default::default()
    };
    RegisterClassExW(&class);

    CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        w!("ParentProbeClass"),
        w!("probe"),
        style,
        0,
        0,
        10,
        10,
        parent,
        None,
        Some(instance.into()),
        None,
    )
    .unwrap_or_default()
}

fn integrity_self() -> String {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return "<unknown>".into();
        }
        let level = integrity_from_token(token);
        let _ = CloseHandle(token);
        level
    }
}

fn integrity_of(pid: u32) -> String {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return "<cannot open process>".into();
        };
        let mut token = HANDLE::default();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
            let _ = CloseHandle(process);
            return "<cannot open token>".into();
        }
        let level = integrity_from_token(token);
        let _ = CloseHandle(token);
        let _ = CloseHandle(process);
        level
    }
}

unsafe fn integrity_from_token(token: HANDLE) -> String {
    let mut needed = 0u32;
    let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
    if needed == 0 {
        return "<unknown>".into();
    }

    let mut buffer = vec![0u8; needed as usize];
    if GetTokenInformation(
        token,
        TokenIntegrityLevel,
        Some(buffer.as_mut_ptr() as *mut _),
        needed,
        &mut needed,
    )
    .is_err()
    {
        return "<unknown>".into();
    }

    let label = &*(buffer.as_ptr() as *const windows::Win32::Security::TOKEN_MANDATORY_LABEL);
    let sid = label.Label.Sid;
    let count = *windows::Win32::Security::GetSidSubAuthorityCount(sid);
    let rid = *windows::Win32::Security::GetSidSubAuthority(sid, (count - 1) as u32);

    match rid {
        0x0000 => "untrusted".into(),
        0x1000 => "low".into(),
        0x2000 => "medium".into(),
        0x2100 => "medium-plus".into(),
        0x3000 => "high".into(),
        0x4000 => "system".into(),
        other => format!("0x{other:X}"),
    }
}
