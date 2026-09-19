//! Discord RPC IPC transport.
//!
//! Discord's desktop client listens on named pipes `\\?\pipe\discord-ipc-0`
//! through `-9`. The wire format is a stream of frames:
//!
//! ```text
//! [ opcode: u32 LE ][ length: u32 LE ][ payload: `length` bytes of JSON ]
//! ```
//!
//! # Why this uses overlapped I/O
//!
//! The obvious implementation opens the pipe as a `std::fs::File` and calls
//! `try_clone` for a second handle to write through. That does not work here,
//! and the way it fails is subtle: Windows serialises I/O on a *file object*,
//! and `try_clone` duplicates the handle without creating a new file object.
//! The session thread spends nearly all its time parked in a blocking
//! `ReadFile`, so a write issued from the UI thread could not begin until that
//! read returned — which only happened when Discord next sent an event.
//!
//! The symptom was that clicking mute did nothing until somebody in the call
//! started or stopped speaking. `examples/pipe_concurrency_probe.rs`
//! reproduces it.
//!
//! Opening with `FILE_FLAG_OVERLAPPED` and giving each direction its own
//! `OVERLAPPED` and event lets a read and a write be in flight at once.

use std::io;
use std::sync::{Arc, Mutex};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

pub const OP_HANDSHAKE: u32 = 0;
pub const OP_FRAME: u32 = 1;
pub const OP_CLOSE: u32 = 2;
pub const OP_PING: u32 = 3;
pub const OP_PONG: u32 = 4;

/// Guards against a desynced stream turning into a huge allocation.
const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

pub struct Frame {
    pub opcode: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn json(&self) -> io::Result<serde_json::Value> {
        serde_json::from_slice(&self.payload)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// A Win32 handle that closes itself.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// The pipe handle is used from the session thread (reads) and the UI thread
// (writes). Overlapped I/O is precisely what makes that safe.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

/// One direction of the pipe, with the event its operations wait on.
///
/// Reads and writes each get their own instance so their `OVERLAPPED`
/// structures and events never collide.
struct PipeEnd {
    pipe: Arc<OwnedHandle>,
    event: OwnedHandle,
}

impl PipeEnd {
    fn new(pipe: Arc<OwnedHandle>) -> io::Result<Self> {
        // Manual-reset, initially unsignalled.
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|e| io::Error::other(format!("CreateEventW failed: {e}")))?;
        Ok(PipeEnd {
            pipe,
            event: OwnedHandle(event),
        })
    }

    fn handle(&self) -> HANDLE {
        self.pipe.0
    }

    /// Issue one overlapped operation and wait for it to complete.
    fn run<F>(&self, start: F) -> io::Result<u32>
    where
        F: FnOnce(HANDLE, *mut OVERLAPPED) -> windows::core::Result<()>,
    {
        unsafe {
            let _ = ResetEvent(self.event.0);

            let mut overlapped = OVERLAPPED {
                hEvent: self.event.0,
                ..Default::default()
            };

            match start(self.handle(), &mut overlapped) {
                // Completed inline, or queued — either way the result is in
                // the OVERLAPPED, and waiting on it is correct for both.
                Ok(()) => {}
                Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
                Err(error) => return Err(io::Error::other(format!("pipe i/o failed: {error}"))),
            }

            let mut transferred = 0u32;
            GetOverlappedResult(self.handle(), &overlapped, &mut transferred, true)
                .map_err(|e| io::Error::other(format!("pipe i/o failed: {e}")))?;
            Ok(transferred)
        }
    }

    fn read_exact(&self, buffer: &mut [u8]) -> io::Result<()> {
        let mut filled = 0;
        while filled < buffer.len() {
            let slice = &mut buffer[filled..];
            let read = self.run(|handle, overlapped| unsafe {
                ReadFile(handle, Some(slice), None, Some(overlapped))
            })?;

            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "discord closed the pipe",
                ));
            }
            filled += read as usize;
        }
        Ok(())
    }

    fn write_all(&self, buffer: &[u8]) -> io::Result<()> {
        let mut sent = 0;
        while sent < buffer.len() {
            let slice = &buffer[sent..];
            let written = self.run(|handle, overlapped| unsafe {
                WriteFile(handle, Some(slice), None, Some(overlapped))
            })?;

            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "pipe accepted no bytes",
                ));
            }
            sent += written as usize;
        }
        Ok(())
    }
}

/// The write half, shared so the UI thread can issue a command while the
/// session thread is blocked reading. The mutex keeps two frames from
/// interleaving on the wire; overlapped I/O keeps a write from having to wait
/// on the read.
pub struct PipeWriter {
    end: PipeEnd,
}

impl PipeWriter {
    pub fn write_frame(&self, opcode: u32, payload: &[u8]) -> io::Result<()> {
        self.end.write_all(&encode_frame(opcode, payload))
    }
}

pub type SharedWriter = Arc<Mutex<PipeWriter>>;

pub struct IpcConnection {
    reader: PipeEnd,
    writer: SharedWriter,
    pipe_index: u32,
}

impl IpcConnection {
    /// Try each pipe in turn and return the first that accepts a connection.
    pub fn connect() -> io::Result<Self> {
        let mut last_err = None;

        for index in 0..10u32 {
            let path: Vec<u16> = format!(r"\\?\pipe\discord-ipc-{index}")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            let handle = unsafe {
                CreateFileW(
                    PCWSTR(path.as_ptr()),
                    FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            };

            match handle {
                Ok(handle) if !handle.is_invalid() => {
                    let pipe = Arc::new(OwnedHandle(handle));
                    let reader = PipeEnd::new(Arc::clone(&pipe))?;
                    let writer = PipeWriter {
                        end: PipeEnd::new(pipe)?,
                    };

                    return Ok(Self {
                        reader,
                        writer: Arc::new(Mutex::new(writer)),
                        pipe_index: index,
                    });
                }
                Ok(_) => {}
                Err(e) => last_err = Some(io::Error::other(e.to_string())),
            }
        }

        Err(last_err
            .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no discord-ipc pipe found")))
    }

    pub fn pipe_index(&self) -> u32 {
        self.pipe_index
    }

    /// The OS handle reads are issued on, so another thread can cancel a
    /// pending read with `CancelIoEx` and force a reconnect.
    pub fn raw_handle(&self) -> isize {
        self.reader.handle().0 as isize
    }

    /// A handle the UI thread can use to send commands of its own.
    pub fn shared_writer(&self) -> SharedWriter {
        Arc::clone(&self.writer)
    }

    pub fn send(&mut self, opcode: u32, payload: &[u8]) -> io::Result<()> {
        let writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("ipc writer poisoned"))?;
        writer.write_frame(opcode, payload)
    }

    pub fn send_json(&mut self, opcode: u32, value: &serde_json::Value) -> io::Result<()> {
        let payload =
            serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        self.send(opcode, &payload)
    }

    pub fn recv(&mut self) -> io::Result<Frame> {
        let mut header = [0u8; 8];
        self.reader.read_exact(&mut header)?;

        let opcode = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;

        if length > MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame length {length} exceeds sane maximum"),
            ));
        }

        let mut payload = vec![0u8; length];
        if length > 0 {
            self.reader.read_exact(&mut payload)?;
        }

        Ok(Frame { opcode, payload })
    }
}

fn encode_frame(opcode: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + payload.len());
    buf.extend_from_slice(&opcode.to_le_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Monotonic-ish nonce. Discord only requires uniqueness within a connection,
/// so a counter is enough and avoids pulling in a UUID dependency.
pub fn next_nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("dtb-{}-{}", std::process::id(), n)
}
