//! Sources of Discord state.
//!
//! The UI consumes `ProviderEvent`s and never learns which provider produced
//! them. Today that is the RPC provider; a Vencord plugin could be added as a
//! second source (for things RPC does not expose, such as a real unread count)
//! without the taskbar widget changing at all.

pub mod rpc;

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::Credentials;
use crate::model::StatusSnapshot;
use crate::ui::WM_APP_STATUS;

/// Everything a provider can tell the UI.
#[derive(Debug, Clone)]
pub enum ProviderEvent {
    /// A new view of the world. Replaces whatever came before.
    Status(StatusSnapshot),
    /// Waiting on the user to accept the Discord consent dialog.
    AwaitingAuthorization,
    /// A command was rejected - most usefully, a mute that did not take.
    CommandFailed(String),
    /// Lost contact, with a human-readable reason.
    Offline(String),
}

/// Posts provider events to the UI thread's message queue.
///
/// Cloneable and `Send`: the provider thread owns one, the UI thread owns the
/// window it targets.
#[derive(Clone)]
pub struct ProviderSink {
    target: Arc<AtomicIsize>,
}

impl ProviderSink {
    pub fn new(target: HWND) -> Self {
        ProviderSink {
            target: Arc::new(AtomicIsize::new(target.0 as isize)),
        }
    }

    /// Hand an event to the UI thread.
    ///
    /// The event is boxed and leaked into the message; the UI side reclaims it.
    /// If posting fails the box is reclaimed here instead, so nothing leaks.
    pub fn send(&self, event: ProviderEvent) {
        let raw = self.target.load(Ordering::Relaxed);
        if raw == 0 {
            return;
        }

        let hwnd = HWND(raw as *mut std::ffi::c_void);
        let boxed = Box::into_raw(Box::new(event));

        let posted = unsafe {
            PostMessageW(
                Some(hwnd),
                WM_APP_STATUS,
                WPARAM(boxed as usize),
                LPARAM(0),
            )
            .is_ok()
        };

        if !posted {
            drop(unsafe { Box::from_raw(boxed) });
        }
    }
}

/// Reclaim an event posted by `ProviderSink::send`.
///
/// # Safety
/// `wparam` must be the value from a `WM_APP_STATUS` message and must not have
/// been taken already.
pub unsafe fn take_event(wparam: usize) -> Option<ProviderEvent> {
    if wparam == 0 {
        return None;
    }
    Some(*Box::from_raw(wparam as *mut ProviderEvent))
}

/// Lets the UI thread poke a provider that is blocked reading the pipe.
///
/// The session thread spends nearly all its time inside a blocking read, so
/// "reconnect" cannot be a flag it polls. Instead we keep the pipe handle here
/// and cancel the read outright, which makes the read fail and the session loop
/// fall through to its normal reconnect path.
#[derive(Clone, Default)]
pub struct ProviderControl {
    handle: Arc<AtomicIsize>,
    reauthorize: Arc<AtomicBool>,
    /// Write half of the live pipe, so commands can be sent while the session
    /// thread sits blocked in a read.
    writer: Arc<Mutex<Option<rpc::pipe::SharedWriter>>>,
    /// In demo mode commands are swallowed and reported as sent, so the UI
    /// behaves exactly as it would against a real client.
    demo: Arc<AtomicBool>,
}

impl ProviderControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept and discard every command, for the synthetic provider.
    pub fn demo() -> Self {
        let control = Self::default();
        control.demo.store(true, Ordering::SeqCst);
        control
    }

    /// Called by the session once its pipe is up.
    pub fn attach(&self, handle: isize, writer: rpc::pipe::SharedWriter) {
        self.handle.store(handle, Ordering::SeqCst);
        if let Ok(mut slot) = self.writer.lock() {
            *slot = Some(writer);
        }
    }

    pub fn detach(&self) {
        self.handle.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = self.writer.lock() {
            *slot = None;
        }
    }

    /// Fire a command at Discord without waiting for its reply.
    ///
    /// The response comes back on the session thread's reader, which ignores
    /// frames carrying no `evt` — so it is discarded harmlessly. Anything we
    /// actually need to know arrives as an event anyway: setting mute produces
    /// a `VOICE_SETTINGS_UPDATE`, which is what refreshes the widget.
    pub fn send_command(&self, cmd: &str, args: Value) -> bool {
        if self.demo.load(Ordering::SeqCst) {
            let _ = (cmd, args);
            return true;
        }

        let Ok(slot) = self.writer.lock() else {
            return false;
        };
        let Some(writer) = slot.as_ref() else {
            return false;
        };

        let request = json!({ "cmd": cmd, "args": args, "nonce": rpc::pipe::next_nonce() });
        let Ok(payload) = serde_json::to_vec(&request) else {
            return false;
        };

        let Ok(file) = writer.lock() else {
            return false;
        };
        file.write_frame(rpc::pipe::OP_FRAME, &payload).is_ok()
    }

    /// Set mute and/or deafen on the Discord client.
    pub fn set_voice(&self, mute: Option<bool>, deaf: Option<bool>) -> bool {
        let mut args = json!({});
        if let Some(mute) = mute {
            args["mute"] = Value::Bool(mute);
        }
        if let Some(deaf) = deaf {
            args["deaf"] = Value::Bool(deaf);
        }
        self.send_command("SET_VOICE_SETTINGS", args)
    }

    /// Local volume and/or local mute for one other participant.
    ///
    /// The bound here is a sanity check, not the scale: Discord's documented
    /// maximum is 200, but client plugins raise it, and clamping to 200 here
    /// would quietly drag such a user's volume down every time anything else
    /// about them was set. The meaningful ceiling belongs to the caller, which
    /// knows what scale is in use.
    pub fn set_user_voice(&self, user_id: &str, volume: Option<f32>, mute: Option<bool>) -> bool {
        let mut args = json!({ "user_id": user_id });
        if let Some(volume) = volume {
            args["volume"] = json!(volume.clamp(0.0, 1000.0).round() as i64);
        }
        if let Some(mute) = mute {
            args["mute"] = Value::Bool(mute);
        }
        self.send_command("SET_USER_VOICE_SETTINGS", args)
    }

    /// Leave the current voice channel.
    pub fn leave_voice(&self) -> bool {
        self.send_command("SELECT_VOICE_CHANNEL", json!({ "channel_id": Value::Null }))
    }

    /// Break the current connection; the session loop reconnects by itself.
    pub fn reconnect(&self) {
        let handle = self.handle.swap(0, Ordering::SeqCst);
        if handle == 0 {
            return;
        }
        unsafe {
            let _ = windows::Win32::System::IO::CancelIoEx(
                windows::Win32::Foundation::HANDLE(handle as *mut std::ffi::c_void),
                None,
            );
        }
    }

    /// Drop the cached token and reconnect, forcing a fresh consent dialog.
    pub fn reauthorize(&self) {
        self.reauthorize.store(true, Ordering::SeqCst);
        self.reconnect();
    }

    /// Consumed by the session before it authenticates.
    pub fn take_reauthorize(&self) -> bool {
        self.reauthorize.swap(false, Ordering::SeqCst)
    }
}

/// A fake provider, for working on the interface without being in a call.
///
/// Every UI change until now needed a live voice channel to look at, which
/// made iterating slow and meant some paths could only be reasoned about
/// rather than seen. This emits a plausible call and keeps it moving.
pub fn spawn_demo(sink: ProviderSink) -> std::thread::JoinHandle<()> {
    use crate::model::{ConnectionState, Participant, VoiceStatus};

    std::thread::Builder::new()
        .name("demo-provider".to_string())
        .spawn(move || {
            // Synthetic ids with no avatar hash, so these resolve to
            // Discord's public default avatars — real ids and hashes would be
            // someone's personal data sitting in the repository. The fetch,
            // decode and cache paths are exercised either way.
            let people = [
                ("100000000000000000", "Ada"),
                ("100000000004194304", "Bram"),
                ("100000000008388608", "Cleo"),
            ];

            let mut status = VoiceStatus {
                channel_id: Some("demo".to_string()),
                channel_name: Some("Conclave".to_string()),
                guild_id: Some("demo".to_string()),
                guild_name: Some("Demo Server".to_string()),
                guild_icon_url: Some(
                    "https://cdn.discordapp.com/embed/avatars/2.png".to_string(),
                ),
                connection: ConnectionState::Connected,
                ..Default::default()
            };

            status.participants = people
                .iter()
                .enumerate()
                .map(|(index, (id, name))| {
                    let mut p = Participant::new(id.to_string(), name.to_string());
                    p.is_self = index == 0;
                    p
                })
                .collect();

            let mut tick = 0usize;
            loop {
                // Keep somebody talking so the speaking ring is exercised.
                let speaker = tick % (status.participants.len() + 1);
                for (index, participant) in status.participants.iter_mut().enumerate() {
                    participant.speaking = index == speaker;
                }

                sink.send(ProviderEvent::Status(status.clone().into()));
                tick += 1;
                std::thread::sleep(std::time::Duration::from_millis(1500));
            }
        })
        .expect("spawn demo provider thread")
}

/// Start the RPC provider on its own thread.
pub fn spawn_rpc(
    creds: Credentials,
    sink: ProviderSink,
    control: ProviderControl,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("discord-rpc".to_string())
        .spawn(move || rpc::session::run(creds, sink, control))
        .expect("spawn rpc provider thread")
}
