//! Minimal HTTPS client over WinHTTP.
//!
//! We use WinHTTP rather than a Rust HTTP crate so the binary carries no TLS
//! stack of its own: the OS already has one, and it keeps the working set down.
//! Only what this app needs is implemented — a form POST for the OAuth token
//! exchange and a GET for avatar bitmaps.

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::*;

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Debug)]
pub struct HttpError(pub String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for HttpError {}

type Result<T> = std::result::Result<T, HttpError>;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error(context: &str) -> HttpError {
    HttpError(format!(
        "{context} failed: {}",
        windows::core::Error::from_thread()
    ))
}

/// RAII wrapper so early returns can't leak WinHTTP handles.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// URL-encode a string for `application/x-www-form-urlencoded` bodies.
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build a form body from key/value pairs.
pub fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn request(
    method: &str,
    host: &str,
    path: &str,
    headers: Option<&str>,
    body: Option<&[u8]>,
) -> Result<Response> {
    unsafe {
        let agent = wide("discord-taskbar/0.1");
        let session = Handle(WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("WinHttpOpen"));
        }

        let host_w = wide(host);
        let connection = Handle(WinHttpConnect(
            session.0,
            PCWSTR(host_w.as_ptr()),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if connection.0.is_null() {
            return Err(last_error("WinHttpConnect"));
        }

        let method_w = wide(method);
        let path_w = wide(path);
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            PCWSTR(method_w.as_ptr()),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null_mut(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(last_error("WinHttpOpenRequest"));
        }

        // WinHttpSendRequest takes headers as a counted slice, so no
        // terminating NUL.
        let headers_w: Option<Vec<u16>> = headers.map(|h| h.encode_utf16().collect());

        let (body_ptr, body_len) = match body {
            Some(b) => (Some(b.as_ptr() as *const c_void), b.len() as u32),
            None => (None, 0),
        };

        WinHttpSendRequest(
            request.0,
            headers_w.as_deref(),
            body_ptr,
            body_len,
            body_len,
            0,
        )
        .map_err(|_| last_error("WinHttpSendRequest"))?;

        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|_| last_error("WinHttpReceiveResponse"))?;

        // Status code comes back as a DWORD thanks to WINHTTP_QUERY_FLAG_NUMBER.
        let mut status: u32 = 0;
        let mut status_len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut status_len,
            std::ptr::null_mut(),
        )
        .map_err(|_| last_error("WinHttpQueryHeaders"))?;

        let mut out = Vec::new();
        loop {
            let mut available: u32 = 0;
            WinHttpQueryDataAvailable(request.0, &mut available)
                .map_err(|_| last_error("WinHttpQueryDataAvailable"))?;
            if available == 0 {
                break;
            }

            let mut chunk = vec![0u8; available as usize];
            let mut read: u32 = 0;
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr() as *mut c_void,
                available,
                &mut read,
            )
            .map_err(|_| last_error("WinHttpReadData"))?;
            if read == 0 {
                break;
            }
            chunk.truncate(read as usize);
            out.extend_from_slice(&chunk);
        }

        Ok(Response { status, body: out })
    }
}

/// POST an `application/x-www-form-urlencoded` body.
pub fn post_form(host: &str, path: &str, pairs: &[(&str, &str)]) -> Result<Response> {
    let body = form_body(pairs);
    request(
        "POST",
        host,
        path,
        Some("Content-Type: application/x-www-form-urlencoded\r\n"),
        Some(body.as_bytes()),
    )
}

/// GET raw bytes (used for avatar images).
pub fn get(host: &str, path: &str) -> Result<Response> {
    request("GET", host, path, None, None)
}
