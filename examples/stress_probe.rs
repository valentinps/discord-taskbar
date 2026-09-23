//! Diagnostic: hammer the repaint path and watch for growth.
//!
//! A speaking indicator repaints several times a second for as long as a call
//! lasts, so anything the draw path allocates per frame matters. This renders
//! the widget thousands of times and reports process memory and GDI/handle
//! counts as it goes — flat numbers mean the paint path is clean.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Threading::GetCurrentProcess;

use discord_taskbar::assets::icons::IconFonts;
use discord_taskbar::assets::images::ImageCache;
use discord_taskbar::integration::discord::model::{Participant, VoiceStatus};
use discord_taskbar::integration::discord::settings::Settings;
use discord_taskbar::integration::discord::view;
use discord_taskbar::ui::block::{self, Context};
use discord_taskbar::ui::render::{Canvas, Color, Font};
use discord_taskbar::ui::theme::{Appearance, Theme};
use discord_taskbar::ui::Notifier;

const FRAMES: usize = 5_000;
const REPORT_EVERY: usize = 1_000;

fn main() {
    let theme = Theme::from(&Appearance::default());
    let settings = Settings::default();
    let height = theme.height;

    let mut images = ImageCache::new(Notifier::new(HWND(std::ptr::null_mut()), 0));
    let mut icon_fonts = IconFonts::new();
    let font = Font::system_ui(theme.font_size, false).expect("font");

    let mut status = base();
    let mut canvas = Canvas::new(260, height).expect("canvas");

    println!("{:>8}  {:>10}  {:>10}  {:>8}  {:>8}", "frame", "private", "working", "handles", "gdi");
    report(0);

    let start = std::time::Instant::now();

    for frame in 1..=FRAMES {
        // Churn the speaking flags the way a live conversation would.
        let speaker = frame % status.participants.len();
        for (index, participant) in status.participants.iter_mut().enumerate() {
            participant.speaking = index == speaker;
        }
        status.self_state.mute = frame % 7 == 0;
        status.self_state.deaf = frame % 11 == 0;

        canvas.clear();
        canvas.fill_round_rect(
            RECT { left: 0, top: 0, right: 260, bottom: height },
            theme.corner_radius,
            theme.background,
        );

        let blocks = view::blocks(&status, &theme, &settings);
        let mut ctx = Context {
            theme: &theme,
            font: &font,
            icon_fonts: &mut icon_fonts,
            images: &mut images,
            backdrop: Color::rgb(0, 0, 0),
            dpi: 96,
        };
        block::layout(&mut canvas, &mut ctx, &blocks, height, true);

        if frame % REPORT_EVERY == 0 {
            report(frame);
        }
    }

    let elapsed = start.elapsed();
    println!(
        "\n{FRAMES} frames in {:.2}s = {:.3} ms/frame",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / FRAMES as f64
    );
}

fn report(frame: usize) {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetProcessHandleCount;
    use windows::Win32::System::Threading::{GetGuiResources, GR_GDIOBJECTS};

    unsafe {
        let process = GetCurrentProcess();

        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let _ = GetProcessMemoryInfo(
            process,
            &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        );

        let mut handles = 0u32;
        let _ = GetProcessHandleCount(process, &mut handles);
        let gdi = GetGuiResources(process, GR_GDIOBJECTS);

        println!(
            "{frame:>8}  {:>8.2}MB  {:>8.2}MB  {handles:>8}  {gdi:>8}",
            counters.PrivateUsage as f64 / 1_048_576.0,
            counters.WorkingSetSize as f64 / 1_048_576.0,
        );
    }
}

fn base() -> VoiceStatus {
    // Synthetic ids with no avatar hash; see layout_probe for why.
    let people = [
        ("100000000000000000", "Ada"),
        ("100000000004194304", "Bram"),
        ("100000000008388608", "Cleo"),
    ];

    VoiceStatus {
        channel_id: Some("1".to_string()),
        channel_name: Some("Conclave".to_string()),
        guild_id: Some("probe".to_string()),
        guild_name: Some("My Server".to_string()),
        guild_icon_url: Some("https://cdn.discordapp.com/embed/avatars/2.png".to_string()),
        participants: people
            .iter()
            .map(|(id, name)| Participant::new(id.to_string(), name.to_string()))
            .collect(),
        ..Default::default()
    }
}
