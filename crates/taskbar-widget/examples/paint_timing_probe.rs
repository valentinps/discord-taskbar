//! Diagnostic: time the individual steps of a refresh.
//!
//! Toggling mute felt like it took seconds even though Discord acknowledges
//! `SET_VOICE_SETTINGS` in tens of milliseconds, so the cost has to be on this
//! side. This times each thing `App::refresh` does, in isolation.
//!
//! The expensive step it originally caught — sampling the taskbar's colour —
//! is gone entirely now that the widget is transparent.

use std::time::Instant;

use taskbar_widget::ui::taskbar;

const ITERATIONS: usize = 30;

fn main() {
    let Some(info) = taskbar::find() else {
        eprintln!("no taskbar found");
        return;
    };

    println!(
        "taskbar {}x{} at ({}, {}), dpi {}",
        info.width(),
        info.height(),
        info.rect.left,
        info.rect.top,
        info.dpi
    );
    println!();

    // 1. Locating the taskbar: FindWindow + GetWindowRect + GetDpiForWindow.
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = taskbar::find();
    }
    let find_ms = start.elapsed().as_secs_f64() * 1000.0 / ITERATIONS as f64;

    println!("{:<34} {:>9.3} ms/call", "taskbar::find()", find_ms);
    println!();
    println!("cost per refresh from locating the bar: {find_ms:.3} ms");
    println!();
    println!("The taskbar colour used to be sampled here too, and dominated this");
    println!("measurement: GetPixel on the screen DC forces the compositor to read");
    println!("back from the GPU. The widget is presented with UpdateLayeredWindow");
    println!("now and the taskbar shows through it, so nothing needs sampling.");
}
