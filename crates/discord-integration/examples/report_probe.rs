//! Print the diagnostics report as text, without opening a window.
//!
//! Shows both halves in the order the window assembles them, so the
//! "checking..." placeholder the probe replaces is visible here.
fn main() {
    let report = discord_integration::doctor::report();
    print!("{}", taskbar_widget::ui::doctor::gather_local(&report));
    print!("{}", (report.probe)());
}
