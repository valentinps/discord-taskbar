//! Diagnostic: time the individual steps of a refresh.
//!
//! Toggling mute felt like it took seconds even though Discord acknowledges
//! `SET_VOICE_SETTINGS` in tens of milliseconds, so the cost has to be on this
//! side. This times each thing `App::refresh` does, in isolation.

use std::time::Instant;

use discord_taskbar::ui::taskbar;

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

    // 2. Sampling the taskbar's colour: GetDC(NULL) + three GetPixel calls.
    let avoid = (info.rect.left + 400, info.rect.top, 300, info.height());
    let start = Instant::now();
    let mut sampled = None;
    for _ in 0..ITERATIONS {
        sampled = taskbar::sample_background(&info, avoid);
    }
    let sample_ms = start.elapsed().as_secs_f64() * 1000.0 / ITERATIONS as f64;

    println!("{:<34} {:>9.3} ms/call", "taskbar::find()", find_ms);
    println!(
        "{:<34} {:>9.3} ms/call   -> {:?}",
        "taskbar::sample_background()", sample_ms, sampled
    );
    println!();

    let per_refresh = find_ms + sample_ms;
    println!("cost per refresh from these two alone: {per_refresh:.3} ms");
    if sample_ms > 5.0 {
        println!(
            "\nsample_background dominates. GetPixel on the screen DC forces the\n\
             compositor to read back from the GPU, which is why it is slow."
        );
    }
}
