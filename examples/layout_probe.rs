//! Diagnostic: render the full widget from synthetic state.
//!
//! Exercises every visual case — speaking, muted, deafened, server-muted,
//! overflow — without needing a live call to reproduce them. Writes a BMP so it
//! can be opened directly.

use windows::Win32::Foundation::{HWND, RECT};

use discord_taskbar::assets::icons::IconFonts;
use discord_taskbar::assets::images::ImageCache;
use discord_taskbar::model::{Participant, VoiceStatus};
use discord_taskbar::ui::elements::{
    self, AvatarRow, ChannelLabel, Context, Divider, Element, GuildIcon, LeaveButton,
    SelfStatusIcons, Separator,
};
use discord_taskbar::ui::render::{Canvas, Color, Font};
use discord_taskbar::ui::theme::{Appearance, Theme};
use discord_taskbar::ui::Notifier;

/// Synthetic ids with no avatar hash, so these resolve to Discord's public
/// default avatars. The download, decode and cache paths are exercised the
/// same way, without anyone's real account ending up in the repository.
const PEOPLE: &[(&str, &str)] = &[
    ("100000000000000000", "Ada"),
    ("100000000004194304", "Bram"),
    ("100000000008388608", "Cleo"),
];

fn person(index: usize) -> Participant {
    let (id, name) = PEOPLE[index % PEOPLE.len()];
    Participant::new(id.to_string(), name.to_string())
}

fn main() {
    let theme = Theme::from(&Appearance::default());
    let height = theme.height;

    // One row per visual case.
    let cases: Vec<(&str, VoiceStatus)> = vec![
        ("speaking", {
            let mut s = base();
            s.participants[0].speaking = true;
            s
        }),
        ("self muted", {
            let mut s = base();
            s.self_state.mute = true;
            s.participants[0].self_mute = true;
            s
        }),
        ("self deafened", {
            let mut s = base();
            s.self_state.mute = true;
            s.self_state.deaf = true;
            s.participants[0].self_mute = true;
            s.participants[0].self_deaf = true;
            s
        }),
        ("others muted/deafened", {
            let mut s = base();
            s.participants[1].self_mute = true;
            s.participants[2].self_deaf = true;
            s
        }),
        ("server muted", {
            let mut s = base();
            s.participants[1].server_mute = true;
            s
        }),
        ("someone muted locally", {
            let mut s = base();
            s.participants[1].local_mute = true;
            s.participants[2].volume = 35.0;
            s
        }),
        ("no overlap (avatar_overlap -4)", base()),
        ("integrated look", base()),
    ];

    let mut images = ImageCache::new(Notifier::new(HWND(std::ptr::null_mut()), 0));
    let mut icon_fonts = IconFonts::new();
    let font = Font::system_ui(theme.font_size, false).expect("font");

    // Warm the cache so avatars are present when we draw.
    {
        let status = base();
        let mut probe = Canvas::new(1, 1).expect("probe canvas");
        let mut ctx = Context {
            status: &status,
            theme: &theme,
            font: &font,
            icon_fonts: &mut icon_fonts,
            images: &mut images,
            backdrop: theme.background,
            dpi: 96,
        };
        for element in [
            Box::new(GuildIcon) as Box<dyn Element>,
            Box::new(AvatarRow),
        ] {
            element.measure(&probe, &mut ctx);
            element.draw(
                &mut probe,
                &mut ctx,
                RECT { left: 0, top: 0, right: 1, bottom: 1 },
            );
        }
    }
    // Give the fetch worker a moment; probes are allowed to be patient.
    for _ in 0..50 {
        images.collect();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let elements: Vec<Box<dyn Element>> = vec![
        Box::new(GuildIcon),
        Box::new(Separator),
        Box::new(ChannelLabel),
        Box::new(AvatarRow),
        Box::new(Divider),
        Box::new(SelfStatusIcons),
        Box::new(LeaveButton),
    ];

    // Per-case theme tweaks, so the config options are exercised too.
    let themes: Vec<Theme> = cases
        .iter()
        .map(|(name, _)| {
            let mut appearance = Appearance::default();
            match *name {
                "no overlap (avatar_overlap -4)" => appearance.avatar_overlap = -4,
                "integrated look" => {
                    appearance.background = "#00000000".to_string();
                    appearance.height = 0;
                    appearance.avatar_size = 28;
                    appearance.icon_size = 20;
                }
                _ => {}
            }
            Theme::from(&appearance)
        })
        .collect();

    // Measure the widest case so every row shares one canvas width.
    let mut widths = Vec::new();
    for (index, (_, status)) in cases.iter().enumerate() {
        let mut scratch = Canvas::new(1, height).expect("scratch");
        let mut ctx = Context {
            status,
            theme: &themes[index],
            font: &font,
            icon_fonts: &mut icon_fonts,
            images: &mut images,
            backdrop: theme.background,
            dpi: 96,
        };
        widths.push(
            elements::layout(&mut scratch, &mut ctx, &elements, height, false).width,
        );
    }

    let widest = widths.iter().copied().max().unwrap_or(200);
    let gap = 6;
    let total_height = (height + gap) * cases.len() as i32 + gap;

    let mut sheet = Canvas::new(widest, total_height).expect("sheet");
    sheet.clear();
    sheet.fill_rect(
        RECT {
            left: 0,
            top: 0,
            right: widest,
            bottom: total_height,
        },
        // Same black as the user's taskbar, so contrast matches reality.
        Color::rgb(0, 0, 0),
    );

    for (index, (name, status)) in cases.iter().enumerate() {
        let top = gap + index as i32 * (height + gap);

        let mut row = Canvas::new(widths[index].max(1), height).expect("row");
        row.clear();
        row.fill_rect(
            RECT {
                left: 0,
                top: 0,
                right: widths[index],
                bottom: height,
            },
            Color::rgb(0, 0, 0),
        );
        row.fill_round_rect(
            RECT {
                left: 0,
                top: 0,
                right: widths[index],
                bottom: height,
            },
            themes[index].corner_radius,
            themes[index].background,
        );

        let mut ctx = Context {
            status,
            theme: &themes[index],
            font: &font,
            icon_fonts: &mut icon_fonts,
            images: &mut images,
            backdrop: Color::rgb(0, 0, 0),
            dpi: 96,
        };
        elements::layout(&mut row, &mut ctx, &elements, height, true);

        let pixels = row.snapshot();
        let row_width = row.width();
        for y in 0..height {
            for x in 0..row_width {
                sheet.blend(x, top + y, pixels[(y * row_width + x) as usize]);
            }
        }

        println!("row {index}: {name:<24} width={}", widths[index]);
    }

    write_bmp("layout_probe.bmp", &mut sheet);
    println!("\nwrote layout_probe.bmp ({widest}x{total_height})");
}

fn base() -> VoiceStatus {
    VoiceStatus {
        channel_id: Some("1".to_string()),
        channel_name: Some("Conclave".to_string()),
        guild_id: Some("probe".to_string()),
        guild_name: Some("My Server".to_string()),
        // A real, publicly reachable image so the fetch path is exercised.
        guild_icon_url: Some("https://cdn.discordapp.com/embed/avatars/2.png".to_string()),
        participants: (0..3).map(person).collect(),
        ..Default::default()
    }
}

/// 32-bit BMP with a negative height (top-down), so any viewer opens it.
fn write_bmp(path: &str, canvas: &mut Canvas) {
    let width = canvas.width();
    let height = canvas.height();
    let pixels = canvas.snapshot();

    let pixel_bytes = (width * height * 4) as u32;
    let offset = 14u32 + 40u32;

    let mut out = Vec::with_capacity((offset + pixel_bytes) as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(offset + pixel_bytes).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());

    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&(-height).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&pixel_bytes.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for pixel in &pixels {
        out.extend_from_slice(&pixel.to_le_bytes());
    }

    std::fs::write(path, out).expect("write bmp");
}
