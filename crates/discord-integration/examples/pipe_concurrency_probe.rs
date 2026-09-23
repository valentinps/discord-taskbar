//! Diagnostic: can we write to the pipe while another thread is blocked reading?
//!
//! Mute was reported as only taking effect when somebody started or stopped
//! speaking. That is a very specific symptom: it means the command sits
//! unwritten until an event arrives, i.e. the write is queued behind the
//! blocking read.
//!
//! Windows serialises I/O on a *synchronous* file object, and `try_clone`
//! duplicates the handle without making a new file object — so a blocking
//! `ReadFile` may well hold the write off. This measures whether that is
//! actually happening.
//!
//! Expected if the theory holds: the write takes as long as it takes for
//! Discord to send something unprompted, rather than microseconds.

use std::time::Instant;

use serde_json::json;

use taskbar_widget::config::Config;
use discord_integration::provider::rpc::pipe::{self, IpcConnection, OP_FRAME, OP_HANDSHAKE};

fn main() {
    let (config, _) = Config::load_or_create();
    let creds = discord_integration::settings::credentials(&config);
    let client_id = creds.client_id.clone();
    if client_id.is_empty() {
        eprintln!("client_id not configured");
        return;
    }

    let mut conn = match IpcConnection::connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not reach Discord: {e}");
            return;
        }
    };
    println!("connected on discord-ipc-{}", conn.pipe_index());

    // Handshake, and read the READY frame, on this thread.
    conn.send_json(OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))
        .expect("handshake");
    let ready = conn.recv().expect("ready");
    println!("handshake ok ({} byte READY)", ready.payload.len());
    println!();

    let writer = conn.shared_writer();

    // Park a thread in a blocking read. Nothing is subscribed, so Discord has
    // no reason to send anything — the read should simply sit there.
    let reader = std::thread::spawn(move || loop {
        match conn.recv() {
            Ok(frame) => println!("    [reader] frame opcode={} len={}", frame.opcode, frame.payload.len()),
            Err(e) => {
                println!("    [reader] read ended: {e}");
                return;
            }
        }
    });

    // Let the reader actually get into ReadFile.
    std::thread::sleep(std::time::Duration::from_millis(500));

    println!("reader is now blocked in ReadFile; attempting writes from this thread\n");

    for attempt in 1..=3 {
        let request = json!({
            "cmd": "GET_VOICE_SETTINGS",
            "nonce": pipe::next_nonce(),
        });
        let payload = serde_json::to_vec(&request).expect("encode");

        let start = Instant::now();
        let result = {
            let file = writer.lock().expect("writer lock");
            file.write_frame(OP_FRAME, &payload)
        };
        let elapsed = start.elapsed();

        println!(
            "write {attempt}: {:>9.2} ms  {}",
            elapsed.as_secs_f64() * 1000.0,
            match &result {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("error: {e}"),
            }
        );

        if elapsed.as_millis() > 200 {
            println!("        ^ blocked. The write waited on the reader.");
        }

        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    println!("\nIf every write returned in well under a millisecond, reads and writes");
    println!("do not serialise and the delay is somewhere else.");

    // Do not wait for the reader; it is parked on purpose.
    drop(reader);
    std::process::exit(0);
}
