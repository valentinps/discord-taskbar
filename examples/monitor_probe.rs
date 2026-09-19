//! Diagnostic: list every taskbar the app can attach to.
//!
//! Multi-monitor support hinges on finding the secondary bars, which use a
//! different window class from the primary one.

use discord_taskbar::ui::taskbar;

fn main() {
    println!("monitor enumeration order:");
    for (index, device) in taskbar::monitor_order().iter().enumerate() {
        println!("  [{index}] {device}");
    }
    println!();

    let bars = taskbar::find_all();
    println!("{} taskbar(s):", bars.len());
    for bar in &bars {
        println!(
            "  [{}] {:<14} primary={:<5} rect=({},{})-({},{}) {}x{} dpi={} edge={:?}",
            bar.monitor_index,
            bar.monitor,
            bar.is_primary,
            bar.rect.left,
            bar.rect.top,
            bar.rect.right,
            bar.rect.bottom,
            bar.width(),
            bar.height(),
            bar.dpi,
            bar.edge,
        );
    }
}
