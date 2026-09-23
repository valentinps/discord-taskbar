//! The contract between the widget and whatever it is showing.
//!
//! The widget owns the windows, the taskbar, the drawing, the tray icon and
//! the settings. An integration owns some state and knows how to describe it
//! as [`Block`]s. Neither knows anything else about the other.
//!
//! Everything crossing this boundary is plain data. Blocks go one way;
//! interactions, identified by the [`BlockId`] the integration chose, come
//! back. Anything the integration wants to *do* to the screen — open a menu,
//! raise a readout, put a message on the widget — it asks for through [`Ui`],
//! so it never touches a window handle.

use std::any::Any;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::assets::images::ImageRef;
use crate::ui::block::{Block, BlockId};
use crate::ui::menu;
use crate::ui::render::Color;
use crate::ui::theme::Theme;

use super::WM_APP_INTEGRATION;

/// Something an integration's background work has to say.
///
/// Opaque to the widget: it travels through the window message queue, so it
/// arrives on the UI thread, in order, without a lock, and the integration
/// downcasts it back to its own type on the way out.
pub type Event = Box<dyn Any + Send>;

/// Posts events to the UI thread's message queue.
///
/// Cloneable and `Send`: a background thread owns one, the UI thread owns the
/// window it targets.
#[derive(Clone)]
pub struct EventSink {
    target: Arc<AtomicIsize>,
}

impl EventSink {
    pub fn new(target: HWND) -> Self {
        EventSink {
            target: Arc::new(AtomicIsize::new(target.0 as isize)),
        }
    }

    /// Hand an event to the UI thread.
    ///
    /// The event is boxed and leaked into the message; the UI side reclaims
    /// it. If posting fails the box is reclaimed here instead, so nothing
    /// leaks.
    pub fn send<T: Any + Send + 'static>(&self, event: T) {
        let raw = self.target.load(Ordering::Relaxed);
        if raw == 0 {
            return;
        }

        let hwnd = HWND(raw as *mut std::ffi::c_void);
        let boxed: *mut Event = Box::into_raw(Box::new(Box::new(event) as Event));

        let posted = unsafe {
            PostMessageW(
                Some(hwnd),
                WM_APP_INTEGRATION,
                WPARAM(boxed as usize),
                LPARAM(0),
            )
            .is_ok()
        };

        if !posted {
            drop(unsafe { Box::from_raw(boxed) });
        }
    }
}

/// Reclaim an event posted by [`EventSink::send`].
///
/// # Safety
/// `wparam` must be the value from a `WM_APP_INTEGRATION` message and must not
/// have been taken already.
pub unsafe fn take_event(wparam: usize) -> Option<Event> {
    if wparam == 0 {
        return None;
    }
    Some(*Box::from_raw(wparam as *mut Event))
}

/// What the pointer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Click,
    Middle,
    /// Wheel notches: positive is away from the user.
    Wheel(i32),
    /// The pointer moved onto, or within, the widget.
    Hover,
    /// The pointer left the widget entirely. `target` is always `None`.
    Leave,
}

/// One thing the user did to the widget.
#[derive(Debug, Clone)]
pub struct Interaction {
    pub gesture: Gesture,
    /// The block under the pointer, if it was over one.
    pub target: Option<BlockId>,
    /// Where it happened, in screen coordinates.
    pub screen: POINT,
}

impl Interaction {
    /// The target's id, when there is one.
    pub fn id(&self) -> Option<&str> {
        self.target.as_ref().map(|id| id.as_str())
    }
}

/// A one-line readout to raise above the taskbar.
pub struct Meter {
    /// The block this is about. The readout stays up while the pointer is on
    /// that block and goes away when it moves off, which is what makes it feel
    /// attached to what you are doing.
    pub about: BlockId,
    pub title: String,
    /// The number on the right, already formatted.
    pub value_text: String,
    /// How full the bar is, 0 to 1.
    pub fraction: f32,
    pub fill: Color,
    pub image: Option<ImageRef>,
    /// Size of that picture at 96 DPI.
    pub image_size: i32,
    /// Stays up at least this long whatever the pointer is doing — after a
    /// menu choice the pointer is nowhere near the block, and dismissing on
    /// the next check would make it flash and vanish.
    pub hold: Duration,
}

/// One row of the notification-area menu.
pub enum TrayItem {
    Command {
        id: usize,
        label: String,
        checked: bool,
        enabled: bool,
    },
    Separator,
    Submenu {
        label: String,
        items: Vec<TrayItem>,
    },
}

impl TrayItem {
    /// A plain, enabled, unticked command.
    pub fn command(id: usize, label: impl Into<String>) -> Self {
        TrayItem::Command {
            id,
            label: label.into(),
            checked: false,
            enabled: true,
        }
    }

    pub fn enabled(self, enabled: bool) -> Self {
        match self {
            TrayItem::Command {
                id, label, checked, ..
            } => TrayItem::Command {
                id,
                label,
                checked,
                enabled,
            },
            other => other,
        }
    }
}

/// What an integration may ask the widget to do.
pub trait Ui {
    /// Show a menu anchored over the pointer, clear of the taskbar, and block
    /// until something is chosen or it is dismissed.
    ///
    /// `on_slide` is applied as a slider is dragged, so the change takes
    /// effect while adjusting rather than only once the menu closes.
    fn show_menu(&mut self, items: &[menu::Item], on_slide: Option<&dyn Fn(f32)>)
        -> menu::Outcome;

    /// Raise the one-line readout, replacing whatever it was showing.
    fn show_meter(&mut self, meter: Meter);

    fn hide_meter(&mut self);

    /// Put a message on the widget in place of its usual contents.
    ///
    /// With a `linger` it clears itself; without one it stays until the next
    /// status says otherwise.
    fn notice(&mut self, text: String, linger: Option<Duration>);

    /// Clear any message.
    fn clear_notice(&mut self);

    /// Which block is at a screen position, if the widget is under it.
    ///
    /// Menus dismiss on a click outside themselves and report where it landed;
    /// this is what lets a click on the next block along move the menu there
    /// rather than merely closing it.
    fn block_at(&mut self, screen: POINT) -> Option<BlockId>;

    /// Redraw every widget now.
    fn redraw(&mut self);

    /// The colours and metrics in force.
    ///
    /// An integration needs these while deciding what a gesture means — what
    /// colour a readout's bar should be, what a wheel notch is worth — not
    /// only while drawing.
    fn theme(&self) -> &Theme;
}

/// A source of things to show on the taskbar.
pub trait Integration {
    /// Stable, short, and safe in a config file — it namespaces this
    /// integration's settings.
    fn id(&self) -> &'static str;

    /// What to call it in the tray tooltip and the settings window.
    fn name(&self) -> &str;

    /// The tray icon's picture and colour.
    fn tray_icon(&self) -> (crate::assets::icons::Glyph, Color);

    /// Start background work. Anything it learns comes back through `events`.
    fn start(&mut self, events: EventSink);

    /// Fold an event into state. Returns whether the widget should redraw.
    fn on_event(&mut self, event: Event, ui: &mut dyn Ui) -> bool;

    /// Adopt this integration's section of the config.
    ///
    /// Called before `start`, and again whenever the settings are saved.
    fn apply_settings(&mut self, _settings: &serde_json::Value) {}

    /// Settings to show below the widget's own, addressed by a dotted path
    /// within the config — `integrations.<id>.<key>`.
    fn settings_fields(&self) -> Vec<crate::ui::settings::Field> {
        Vec::new()
    }

    /// A note at the top of the settings window, for an integration that has
    /// to be set up somewhere else first.
    fn settings_intro(&self) -> Option<crate::ui::settings::Intro> {
        None
    }

    /// What the widget should show right now, left to right.
    fn blocks(&self, theme: &Theme) -> Vec<Block>;

    /// The tray icon's hover text.
    fn tooltip(&self) -> String;

    /// Respond to a click, a wheel notch, a hover or a middle click.
    fn on_interaction(&mut self, interaction: Interaction, ui: &mut dyn Ui);

    /// Extra rows for the top of the notification-area menu.
    ///
    /// Ids below [`TRAY_ID_BASE`] are the widget's own; anything at or above
    /// it comes back to [`Integration::on_tray_command`].
    fn tray_items(&self) -> Vec<TrayItem> {
        Vec::new()
    }

    fn on_tray_command(&mut self, _id: usize, _ui: &mut dyn Ui) {}
}

/// The first tray command id an integration may use.
///
/// Everything below belongs to the widget: the monitor list alone runs from
/// 300 upwards, one per display.
pub const TRAY_ID_BASE: usize = 1000;
