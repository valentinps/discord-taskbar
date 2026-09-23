//! Synchronous Discord RPC client.
//!
//! Commands are request/response keyed by `nonce`; events arrive unsolicited
//! on the same stream. `call` pumps frames until it sees its own nonce and
//! queues anything else, so an event can never be dropped just because it
//! landed while a command was in flight.

pub mod events;
pub mod oauth;
pub mod pipe;
pub mod session;

use std::collections::VecDeque;

use serde_json::{json, Value};

use pipe::{next_nonce, IpcConnection, OP_CLOSE, OP_FRAME, OP_HANDSHAKE, OP_PING, OP_PONG};

pub const RPC_VERSION: u32 = 1;

#[derive(Debug)]
pub enum RpcError {
    /// No `discord-ipc-N` pipe answered — Discord is probably not running.
    NotRunning(std::io::Error),
    Io(std::io::Error),
    /// Discord returned an error frame; `code` maps to its RPC error table.
    Remote { code: i64, message: String },
    Protocol(String),
    Http(taskbar_widget::http::HttpError),
    Config(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::NotRunning(e) => write!(f, "could not reach Discord over IPC: {e}"),
            RpcError::Io(e) => write!(f, "ipc i/o error: {e}"),
            RpcError::Remote { code, message } => {
                write!(f, "discord returned error {code}: {message}")
            }
            RpcError::Protocol(m) => write!(f, "protocol error: {m}"),
            RpcError::Http(e) => write!(f, "http error: {e}"),
            RpcError::Config(m) => write!(f, "configuration error: {m}"),
        }
    }
}

impl std::error::Error for RpcError {}

impl From<std::io::Error> for RpcError {
    fn from(e: std::io::Error) -> Self {
        RpcError::Io(e)
    }
}

impl From<taskbar_widget::http::HttpError> for RpcError {
    fn from(e: taskbar_widget::http::HttpError) -> Self {
        RpcError::Http(e)
    }
}

pub type Result<T> = std::result::Result<T, RpcError>;

/// A dispatched event: `evt` name plus its `data` payload.
#[derive(Debug, Clone)]
pub struct Event {
    pub name: String,
    pub data: Value,
}

pub struct RpcClient {
    conn: IpcConnection,
    queued_events: VecDeque<Event>,
    /// The `user` object from the READY frame — this is the local account.
    pub ready_user: Option<Value>,
}

impl RpcClient {
    /// Connect and complete the handshake, returning once READY arrives.
    pub fn connect(client_id: &str) -> Result<Self> {
        let conn = IpcConnection::connect().map_err(RpcError::NotRunning)?;

        let mut client = Self {
            conn,
            queued_events: VecDeque::new(),
            ready_user: None,
        };

        client.conn.send_json(
            OP_HANDSHAKE,
            &json!({ "v": RPC_VERSION, "client_id": client_id }),
        )?;

        // The client replies with a DISPATCH/READY frame before anything else.
        loop {
            let frame = client.conn.recv()?;
            match frame.opcode {
                OP_FRAME => {
                    let value = frame.json()?;
                    if value.get("evt").and_then(Value::as_str) == Some("READY") {
                        client.ready_user = value
                            .pointer("/data/user")
                            .cloned();
                        return Ok(client);
                    }
                    // Anything before READY is unexpected but harmless; keep it.
                    if let Some(event) = as_event(&value) {
                        client.queued_events.push_back(event);
                    }
                }
                OP_CLOSE => return Err(close_error(&frame.json().unwrap_or(Value::Null))),
                OP_PING => {
                    client.conn.send(OP_PONG, &frame.payload)?;
                }
                _ => {}
            }
        }
    }

    pub fn pipe_index(&self) -> u32 {
        self.conn.pipe_index()
    }

    pub fn raw_handle(&self) -> isize {
        self.conn.raw_handle()
    }

    pub fn shared_writer(&self) -> pipe::SharedWriter {
        self.conn.shared_writer()
    }

    /// Issue a command and return its `data` payload.
    pub fn call(&mut self, cmd: &str, args: Value) -> Result<Value> {
        let nonce = next_nonce();
        let mut request = json!({ "cmd": cmd, "nonce": nonce });
        if !args.is_null() {
            request["args"] = args;
        }
        self.conn.send_json(OP_FRAME, &request)?;
        self.await_nonce(&nonce)
    }

    pub fn subscribe(&mut self, evt: &str, args: Value) -> Result<Value> {
        self.command_with_evt("SUBSCRIBE", evt, args)
    }

    pub fn unsubscribe(&mut self, evt: &str, args: Value) -> Result<Value> {
        self.command_with_evt("UNSUBSCRIBE", evt, args)
    }

    fn command_with_evt(&mut self, cmd: &str, evt: &str, args: Value) -> Result<Value> {
        let nonce = next_nonce();
        let mut request = json!({ "cmd": cmd, "evt": evt, "nonce": nonce });
        if !args.is_null() {
            request["args"] = args;
        }
        self.conn.send_json(OP_FRAME, &request)?;
        self.await_nonce(&nonce)
    }

    /// Read frames until the response matching `nonce` arrives, queueing any
    /// events that overtake it.
    fn await_nonce(&mut self, nonce: &str) -> Result<Value> {
        loop {
            let frame = self.conn.recv()?;
            match frame.opcode {
                OP_FRAME => {
                    let value = frame.json()?;
                    let matches = value.get("nonce").and_then(Value::as_str) == Some(nonce);

                    if matches {
                        if value.get("evt").and_then(Value::as_str) == Some("ERROR") {
                            return Err(remote_error(&value));
                        }
                        return Ok(value.get("data").cloned().unwrap_or(Value::Null));
                    }

                    if let Some(event) = as_event(&value) {
                        self.queued_events.push_back(event);
                    }
                }
                OP_CLOSE => return Err(close_error(&frame.json().unwrap_or(Value::Null))),
                OP_PING => self.conn.send(OP_PONG, &frame.payload)?,
                _ => {}
            }
        }
    }

    /// Block until the next event. Drains the queue first.
    pub fn next_event(&mut self) -> Result<Event> {
        if let Some(event) = self.queued_events.pop_front() {
            return Ok(event);
        }

        loop {
            let frame = self.conn.recv()?;
            match frame.opcode {
                OP_FRAME => {
                    let value = frame.json()?;
                    if let Some(event) = as_event(&value) {
                        return Ok(event);
                    }
                }
                OP_CLOSE => return Err(close_error(&frame.json().unwrap_or(Value::Null))),
                OP_PING => self.conn.send(OP_PONG, &frame.payload)?,
                _ => {}
            }
        }
    }
}

/// A frame is an event when it carries an `evt` name and no pending nonce we
/// are waiting on. DISPATCH frames have `cmd: "DISPATCH"`.
///
/// `ERROR` frames are included deliberately. A command sent without waiting for
/// its reply — as the UI does for `SET_VOICE_SETTINGS` — can only fail here, so
/// dropping these would make a refused mute look like nothing happening at all.
fn as_event(value: &Value) -> Option<Event> {
    let name = value.get("evt").and_then(Value::as_str)?;
    Some(Event {
        name: name.to_string(),
        data: value.get("data").cloned().unwrap_or(Value::Null),
    })
}

fn remote_error(value: &Value) -> RpcError {
    RpcError::Remote {
        code: value
            .pointer("/data/code")
            .and_then(Value::as_i64)
            .unwrap_or(-1),
        message: value
            .pointer("/data/message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string(),
    }
}

fn close_error(value: &Value) -> RpcError {
    RpcError::Remote {
        code: value.get("code").and_then(Value::as_i64).unwrap_or(-1),
        message: value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("connection closed by Discord")
            .to_string(),
    }
}
