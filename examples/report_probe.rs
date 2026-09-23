//! Print the diagnostics report as text, without opening a window.
//!
//! Shows both halves in the order the window assembles them, so the
//! "checking..." placeholder the probe replaces is visible here.
fn main() {
    use discord_taskbar::integration::discord;
    let report = discord::doctor::report();
    print!("{}", discord_taskbar::ui::doctor::gather_local(&report));
    print!("{}", (report.probe)());
}
