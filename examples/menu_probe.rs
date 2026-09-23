//! Diagnostic: show the drawn user menu on its own.
//!
//! Separates "does the menu build and render" from "does a click on an avatar
//! reach it", which are two very different failures.

use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::{HWND, RECT};

use discord_taskbar::assets::icons::IconFonts;
use discord_taskbar::integration::discord::icons as glyphs;
use discord_taskbar::assets::images::ImageCache;
use discord_taskbar::model::Participant;
use discord_taskbar::ui::menu::{self, Item};
use discord_taskbar::ui::render::Font;
use discord_taskbar::ui::taskbar;
use discord_taskbar::ui::theme::{Appearance, Theme};
use discord_taskbar::ui::Notifier;

fn main() {
    let theme = Theme::from(&Appearance::default());
    let font = Font::system_ui(theme.font_size, false).expect("font");
    let mut images = ImageCache::new(Notifier::new(HWND(std::ptr::null_mut()), 0));
    let mut icon_fonts = IconFonts::new();

    let mut participant =
        Participant::new("100000000008388608".to_string(), "Cleo".to_string());
    participant.volume = 135.0;

    // Let the avatar arrive before the menu is drawn.
    for _ in 0..30 {
        images.image(&participant.avatar_ref(26), 26);
        if images.collect() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let items = vec![
        Item::Header {
            name: participant.display_name.clone(),
            image: Some(participant.avatar_ref(theme.avatar_size as u32)),
        },
        Item::Separator,
        Item::Action {
            id: 1,
            label: "Mute for me".to_string(),
            icon: Some(glyphs::VOLUME),
            checked: false,
            danger: false,
        },
        Item::Volume {
            value: participant.volume,
        },
        Item::VolumePreset {
            label: "Reset volume".to_string(),
            value: 100.0,
        },
        Item::Separator,
        Item::Action {
            id: 2,
            label: "Focus Discord".to_string(),
            icon: None,
            checked: false,
            danger: false,
        },
    ];

    let Some(bar) = taskbar::find() else {
        eprintln!("no taskbar");
        return;
    };

    let anchor = POINT {
        x: (bar.rect.left + bar.rect.right) / 2,
        y: bar.rect.top,
    };

    println!("showing menu at ({}, {}) - click or press Escape", anchor.x, anchor.y);

    // Report every volume the bar is dragged through, so the throttling and
    // the live-application path are both visible.
    let applied = |volume: f32| println!("  applied volume: {volume:.1}");

    let mut style = menu::Style {
        theme: &theme,
        font: &font,
        icon_fonts: &mut icon_fonts,
        images: &mut images,
        dpi: bar.dpi,
        on_volume: Some(&applied),
    };

    let outcome = menu::show(&items, &mut style, anchor, true, bar.rect);
    println!("chose: {:?}", outcome.choice);
    println!("volume left at: {:?}", outcome.volume);
    let _ = RECT::default();
}
