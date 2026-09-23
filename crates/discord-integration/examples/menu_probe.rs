//! Diagnostic: show the drawn user menu on its own.
//!
//! Separates "does the menu build and render" from "does a click on an avatar
//! reach it", which are two very different failures.

use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::{HWND, RECT};

use taskbar_widget::assets::icons::IconFonts;
use discord_integration::icons as glyphs;
use discord_integration::view;
use taskbar_widget::assets::images::ImageCache;
use discord_integration::model::Participant;
use taskbar_widget::ui::menu::{self, Item};
use taskbar_widget::ui::render::Font;
use taskbar_widget::ui::taskbar;
use discord_integration::settings::Settings;
use taskbar_widget::ui::theme::{Appearance, Theme};
use taskbar_widget::ui::Notifier;

fn main() {
    let theme = Theme::from(&Appearance::default());
    let settings = Settings::default();
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
            image: Some(participant.avatar_ref(settings.avatar_size as u32)),
            image_size: settings.avatar_size,
        },
        Item::Separator,
        Item::Action {
            id: 1,
            label: "Mute for me".to_string(),
            icon: Some(glyphs::VOLUME),
            checked: false,
            danger: false,
        },
        Item::Slider {
            label: "Volume".to_string(),
            value: participant.volume,
            range: (0.0, settings.volume_ceiling(participant.volume)),
            format: view::percent,
            warn_above: Some(100.5),
        },
        Item::SliderPreset {
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
        on_slide: Some(&applied),
    };

    let outcome = menu::show(&items, &mut style, anchor, true, bar.rect);
    println!("chose: {:?}", outcome.choice);
    println!("volume left at: {:?}", outcome.slider);
    let _ = RECT::default();
}
