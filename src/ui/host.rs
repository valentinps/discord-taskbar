//! The hidden owner window: all state, the re-anchor timer, and recovery from
//! explorer restarts live here.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ScreenToClient, HDC};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::assets::icons::{Icon, IconFonts};
use crate::assets::images::ImageCache;
use crate::config::Config;
use crate::model::VoiceStatus;
use crate::provider::{self, ProviderControl, ProviderEvent, ProviderSink};

use super::menu;
use super::elements::{
    self, Action, AvatarRow, ChannelLabel, Context, Divider, Element, GuildIcon, LeaveButton,
    SelfStatusIcons, Separator,
};
use super::render::{Canvas, Color, Font};
use super::taskbar::{self, Anchor, Edge, TaskbarInfo, DEFAULT_DPI};
use super::settings;
use super::theme::{MiddleClick, Theme};
use super::tray::{self, Tray};
use super::popup;
use super::widget;
use super::{
    take_widget_input, Notifier, TIMER_ANCHOR, TIMER_ANCHOR_MS, WM_APP_ASSET_READY,
    WM_APP_STATUS, WM_APP_TRAY,
    WM_APP_WIDGET_CLICK, WM_APP_WIDGET_CONTEXT, WM_APP_WIDGET_CURSOR, WM_APP_WIDGET_HOVER,
    WM_APP_WIDGET_LEAVE, WM_APP_WIDGET_MIDDLE, WM_APP_WIDGET_PAINT, WM_APP_WIDGET_WHEEL,
};

const CLASS_NAME: windows::core::PCWSTR = w!("DiscordTaskbarHost");

/// Keep this much clear of the taskbar edges and the clock.
const EDGE_MARGIN: i32 = 8;

/// How long a transient message (a refused command) stays on screen.
const NOTICE_LINGER: std::time::Duration = std::time::Duration::from_secs(4);

/// Minimum time the volume readout stays up when it was not opened by
/// hovering — after a menu choice the pointer is nowhere near the avatar, so
/// hiding the instant we check would make it flash and vanish.
const VOLUME_OVERLAY_HOLD: std::time::Duration = std::time::Duration::from_millis(1200);

/// The volume readout. It lives for as long as the pointer stays on the person
/// it describes, rather than for a fixed time: tying it to the pointer is what
/// makes it feel attached to what you are doing.
struct VolumeOverlay {
    user_id: String,
    name: String,
    volume: f32,
    /// Not dismissed before this instant, whatever the pointer is doing.
    hold_until: std::time::Instant,
}

/// How often to re-check the taskbar's colour while the widget is on screen.
///
/// The sample is cached because taking it is relatively expensive, but a
/// cached *wrong* answer would otherwise persist forever — which is exactly
/// what happened when a transient red window over the bar tinted the whole
/// widget.
const BACKDROP_RECHECK: std::time::Duration = std::time::Duration::from_secs(10);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// One widget, on one taskbar.
///
/// Everything here is per-display; therest of the state is shared. Splitting
/// it this way is what lets the widget appear on several monitors at once
/// without duplicating fonts, avatars or the status itself.
struct Surface {
    hwnd: HWND,
    /// False when the shell would not adopt us and we fell back to a
    /// free-floating top-most window.
    parented: bool,
    taskbar: TaskbarInfo,
    canvas: Option<Canvas>,
    /// The taskbar's own colour, sampled from the screen so the widget's
    /// opaque background is indistinguishable from the bar around it.
    backdrop: Color,
    /// Sampling costs ~16 ms because reading the screen DC forces the
    /// compositor to read back from the GPU. Far too expensive to do on every
    /// repaint, so it is cached and only redone when the taskbar changes.
    backdrop_stale: bool,
    backdrop_checked: Option<std::time::Instant>,
    /// Where each element landed in the last layout, for routing clicks.
    element_bounds: Vec<RECT>,
}

struct App {
    surfaces: Vec<Surface>,
    /// One font per DPI: displays can differ, and a font built for 96 looks
    /// wrong on a 144 DPI panel.
    fonts: HashMap<u32, Font>,
    theme: Theme,
    taskbar_created: u32,

    status: VoiceStatus,
    images: ImageCache,
    icon_fonts: IconFonts,
    elements: Vec<Box<dyn Element>>,
    /// Shown instead of the status when something needs the user's attention.
    notice: Option<String>,
    /// When set, `notice` clears itself at this time.
    notice_expires: Option<std::time::Instant>,
    /// Shown while the wheel is adjusting somebody's volume. Lives in its own
    /// window above the taskbar so the widget — and the avatar being scrolled
    /// over — stays exactly where it is.
    volume_overlay: Option<VolumeOverlay>,
    popup: Option<HWND>,
    popup_canvas: Option<Canvas>,
    tray: Option<Tray>,
    control: ProviderControl,
}

impl App {
    fn new(host: HWND, theme: Theme, taskbar_created: u32, control: ProviderControl) -> Self {
        let _ = host;
        App {
            surfaces: Vec::new(),
            fonts: HashMap::new(),
            theme,
            taskbar_created,
            status: VoiceStatus::default(),
            images: ImageCache::new(Notifier::new(host, WM_APP_ASSET_READY)),
            icon_fonts: IconFonts::new(),
            elements: vec![
                Box::new(GuildIcon),
                Box::new(Separator),
                Box::new(ChannelLabel),
                Box::new(AvatarRow),
                Box::new(Divider),
                Box::new(SelfStatusIcons),
                Box::new(LeaveButton),
            ],
            notice: None,
            notice_expires: None,
            volume_overlay: None,
            popup: None,
            popup_canvas: None,
            tray: Tray::new(host, WM_APP_TRAY),
            control,
        }
    }

    /// Scale a 96-DPI design value for a particular display.
    fn scale_at(&self, dpi: u32, value: i32) -> i32 {
        (value as i64 * dpi as i64 / DEFAULT_DPI as i64) as i32
    }

    /// The UI font for a display, built on first use.
    fn font_for(&mut self, dpi: u32) -> Option<Font> {
        if let Some(font) = self.fonts.remove(&dpi) {
            return Some(font);
        }
        Font::system_ui(self.scale_at(dpi, self.theme.font_size), false)
    }

    fn return_font(&mut self, dpi: u32, font: Font) {
        self.fonts.insert(dpi, font);
    }

    fn rebuild_fonts(&mut self) {
        self.fonts.clear();
    }

    /// Nothing to show: not in a call and nothing to report.
    fn is_idle(&self) -> bool {
        self.notice.is_none() && !self.status.is_connected()
    }

    /// Reconcile the set of widgets against the taskbars the config asks for.
    ///
    /// Returns false when there is nothing to draw on, which happens while
    /// explorer is restarting.
    fn ensure_surfaces(&mut self) -> bool {
        let bars = taskbar::find_all();
        if bars.is_empty() {
            // Explorer is down. Drop the widgets; the host survives and
            // rebuilds when TaskbarCreated arrives.
            self.drop_surfaces();
            return false;
        }

        let wanted: Vec<TaskbarInfo> = bars
            .into_iter()
            .filter(|bar| {
                self.theme
                    .wants_monitor(bar.monitor_index, &bar.monitor, bar.is_primary)
            })
            .collect();

        // Retire widgets whose taskbar is gone or no longer selected.
        let keep: Vec<String> = wanted.iter().map(|bar| bar.monitor.clone()).collect();
        self.surfaces.retain(|surface| {
            let alive = unsafe { IsWindow(Some(surface.hwnd)).as_bool() };
            let still_wanted = keep.contains(&surface.taskbar.monitor);
            if !alive || !still_wanted {
                widget::destroy(surface.hwnd);
                return false;
            }
            true
        });

        for bar in wanted {
            match self
                .surfaces
                .iter_mut()
                .find(|surface| surface.taskbar.monitor == bar.monitor)
            {
                Some(surface) => {
                    // The bar may have moved, resized or changed DPI.
                    if surface.taskbar.dpi != bar.dpi {
                        surface.canvas = None;
                    }
                    if !rects_equal(&surface.taskbar.rect, &bar.rect) {
                        surface.backdrop_stale = true;
                    }
                    // A taskbar recreated by explorer has a new window, so the
                    // widget has to be re-parented to it.
                    if surface.taskbar.tray != bar.tray {
                        surface.parented = widget::attach(surface.hwnd, bar.tray);
                        surface.backdrop_stale = true;
                    }
                    surface.taskbar = bar;
                }
                None => {
                    let Some(hwnd) = widget::create() else {
                        continue;
                    };
                    // Same escape hatch TrafficMonitor keeps: if the shell will
                    // not adopt us, float on top instead of giving up.
                    let parented = widget::attach(hwnd, bar.tray);
                    if !parented {
                        widget::make_topmost(hwnd);
                    }
                    self.surfaces.push(Surface {
                        hwnd,
                        parented,
                        taskbar: bar,
                        canvas: None,
                        backdrop: Color::rgb(0, 0, 0),
                        backdrop_stale: true,
                        backdrop_checked: None,
                        element_bounds: Vec::new(),
                    });
                }
            }
        }

        self.surfaces
            .sort_by_key(|surface| surface.taskbar.monitor_index);
        !self.surfaces.is_empty()
    }

    fn drop_surfaces(&mut self) {
        for surface in self.surfaces.drain(..) {
            widget::destroy(surface.hwnd);
        }
        self.hide_popup();
    }

    fn hide_popup(&mut self) {
        self.volume_overlay = None;
        if let Some(hwnd) = self.popup {
            popup::hide(hwnd);
        }
    }

    fn surface_index(&self, hwnd: HWND) -> Option<usize> {
        self.surfaces.iter().position(|s| s.hwnd == hwnd)
    }

    /// Redraw every widget.
    fn refresh(&mut self) {
        if !self.ensure_surfaces() {
            return;
        }

        // When there is nothing to say, take up no space at all.
        if self.is_idle() {
            for surface in &self.surfaces {
                widget::hide(surface.hwnd);
            }
            return;
        }

        for index in 0..self.surfaces.len() {
            self.refresh_surface(index);
        }
    }

    /// Measure, position, draw and present one widget.
    ///
    /// Order matters: the widget's screen rect has to be known before the
    /// taskbar background can be sampled around it.
    fn refresh_surface(&mut self, index: usize) {
        let Some(surface) = self.surfaces.get(index) else {
            return;
        };
        let info = surface.taskbar.clone();
        let hwnd = surface.hwnd;
        let parented = surface.parented;
        let dpi = info.dpi;

        // `height: 0` means fill the bar. Otherwise leave a margin so the
        // widget reads as a panel sitting in the taskbar.
        let height = if self.theme.height == 0 {
            info.height()
        } else {
            self.scale_at(dpi, self.theme.height)
                .min(info.height() - self.scale_at(dpi, EDGE_MARGIN))
                .max(self.scale_at(dpi, 16))
        };

        if self.surfaces[index].canvas.is_none() {
            self.surfaces[index].canvas = Canvas::new(1, height);
        }

        // Measure with the current content, then size the window to match.
        let Some(width) = self.run_layout(index, height, false) else {
            return;
        };

        let margin = self.scale_at(dpi, EDGE_MARGIN);
        let (screen_x, screen_y) = taskbar::place(
            &info,
            (width, height),
            Anchor::Center,
            (self.theme.x_offset, self.theme.y_offset),
            margin,
        );

        // Sample the taskbar's own colour beside us. A layered child window is
        // not composited by the shell, so the widget paints this backdrop
        // itself and blends in without needing transparency.
        //
        // Only when something could have changed it: this is by far the most
        // expensive thing a refresh does.
        if self.surfaces[index].backdrop_stale {
            // An explicit colour wins: sampling cannot succeed on a
            // translucent bar, and being able to say "it is this colour" is
            // the difference between a widget that blends in and a black box.
            if let Some(fixed) = self.theme.taskbar_background {
                self.surfaces[index].backdrop = fixed;
            } else if let Some(sampled) =
                taskbar::sample_background(&info, (screen_x, screen_y, width, height))
            {
                self.surfaces[index].backdrop = Color::from_colorref(sampled.color());
            }
            self.surfaces[index].backdrop_stale = false;
            self.surfaces[index].backdrop_checked = Some(std::time::Instant::now());
        }

        let radius = self.scale_at(dpi, self.theme.corner_radius);
        let backdrop = self.surfaces[index].backdrop;
        let background = self.theme.background;

        if let Some(canvas) = self.surfaces[index].canvas.as_mut() {
            if !canvas.resize(width, height) {
                return;
            }
            let full = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            canvas.clear();
            canvas.fill_rect(full, backdrop);
            canvas.fill_round_rect(full, radius, background);
        }

        self.run_layout(index, height, true);

        // Position is screen-space; translate into the parent's client space
        // when the taskbar has adopted us.
        let (x, y) = if parented {
            taskbar::screen_to_parent(info.tray, screen_x, screen_y)
        } else {
            (screen_x, screen_y)
        };

        widget::set_geometry(hwnd, x, y, width, height);
        widget::show(hwnd);
        widget::invalidate(hwnd);
    }

    /// Run the element layout for one surface, optionally drawing.
    ///
    /// Takes the canvas and font out of `self` for the duration so the element
    /// context can borrow the rest mutably.
    fn run_layout(&mut self, index: usize, height: i32, draw: bool) -> Option<i32> {
        let dpi = self.surfaces.get(index)?.taskbar.dpi;
        let mut canvas = self.surfaces.get_mut(index)?.canvas.take()?;
        let font = match self.font_for(dpi) {
            Some(font) => font,
            None => {
                self.surfaces[index].canvas = Some(canvas);
                return None;
            }
        };

        // A notice replaces the normal contents entirely.
        let width = if let Some(notice) = self.notice.clone() {
            let padding = self.scale_at(dpi, self.theme.padding);
            let text_width = canvas
                .measure_text(&notice, &font)
                .0
                .min(self.scale_at(dpi, self.theme.max_label_width));
            if draw {
                let y = (height - font.height) / 2;
                canvas.draw_text_ellipsised(
                    &notice,
                    &font,
                    padding,
                    y,
                    text_width,
                    self.theme.text_dim,
                );
            }
            self.surfaces[index].element_bounds.clear();
            text_width + padding * 2
        } else {
            let mut ctx = Context {
                status: &self.status,
                theme: &self.theme,
                font: &font,
                icon_fonts: &mut self.icon_fonts,
                images: &mut self.images,
                backdrop: self.theme.background,
                dpi,
            };
            let layout = elements::layout(&mut canvas, &mut ctx, &self.elements, height, draw);
            self.surfaces[index].element_bounds = layout.bounds;
            layout.width
        };

        self.surfaces[index].canvas = Some(canvas);
        self.return_font(dpi, font);
        Some(width.max(self.scale_at(dpi, 24)))
    }

    /// Blit a widget's canvas into its paint DC.
    fn paint(&mut self, widget: HWND, dc: HDC) {
        let Some(index) = self.surface_index(widget) else {
            return;
        };
        if let Some(canvas) = &self.surfaces[index].canvas {
            canvas.blit(dc);
        }
    }

    /// Route a click in a widget to whichever element owns that pixel.
    fn on_widget_click(&mut self, widget: HWND, point: POINT) {
        match self.action_at(widget, point) {
            Some(Action::ToggleMute) => self.toggle_mute(),
            Some(Action::ToggleDeafen) => self.toggle_deafen(),
            Some(Action::LeaveVoice) => {
                self.control.leave_voice();
            }
            Some(Action::UserMenu(user_id)) => self.show_user_menu(widget, &user_id),
            // Only the server icon and the channel name jump to Discord;
            // clicking a participant is for acting on that participant.
            Some(Action::FocusDiscord) => {
                tray::focus_discord();
            }
            None => {}
        }
    }

    /// Whether a point is worth showing a hand cursor over.
    fn is_interactive(&mut self, widget: HWND, point: POINT) -> bool {
        self.action_at(widget, point).is_some()
    }

    fn action_at(&mut self, widget: HWND, point: POINT) -> Option<Action> {
        let index = self.surface_index(widget)?;
        let dpi = self.surfaces[index].taskbar.dpi;
        let bounds = self.surfaces[index].element_bounds.clone();
        let font = self.font_for(dpi)?;

        let action = {
            let ctx = Context {
                status: &self.status,
                theme: &self.theme,
                font: &font,
                icon_fonts: &mut self.icon_fonts,
                images: &mut self.images,
                backdrop: self.theme.background,
                dpi,
            };
            elements::hit_test(&ctx, &self.elements, &bounds, point)
        };
        self.return_font(dpi, font);
        action
    }

    fn user_at(&mut self, widget: HWND, point: POINT) -> Option<String> {
        let index = self.surface_index(widget)?;
        let dpi = self.surfaces[index].taskbar.dpi;
        let bounds = self.surfaces[index].element_bounds.clone();
        let font = self.font_for(dpi)?;

        let user = {
            let ctx = Context {
                status: &self.status,
                theme: &self.theme,
                font: &font,
                icon_fonts: &mut self.icon_fonts,
                images: &mut self.images,
                backdrop: self.theme.background,
                dpi,
            };
            self.elements
                .iter()
                .zip(bounds.iter())
                .filter(|(_, rect)| rect.right > rect.left)
                .find_map(|(element, rect)| element.user_at(&ctx, *rect, point))
        };
        self.return_font(dpi, font);
        user
    }

    /// Wheel over a participant adjusts how loud they are, locally.
    fn on_wheel(&mut self, widget: HWND, notches: i32, point: POINT) {
        let Some(user_id) = self.user_at(widget, point) else {
            return;
        };
        let Some(participant) = self
            .status
            .participants
            .iter()
            .find(|p| p.user_id == user_id)
        else {
            return;
        };

        // Your own volume is not a thing Discord lets you set.
        if participant.is_self {
            return;
        }

        let step = self.theme.scroll_volume_step as f32;
        let volume = (participant.volume + notches as f32 * step).clamp(0.0, 200.0);
        let name = participant.display_name.clone();

        if !self.control.set_user_voice(&user_id, Some(volume), None) {
            return;
        }

        // Reflect it now; Discord's own event will confirm.
        if let Some(participant) = self.status.participant_mut(&user_id) {
            participant.volume = volume;
        }

        self.volume_overlay = Some(VolumeOverlay {
            user_id: user_id.clone(),
            name,
            volume,
            // Opened by the wheel, so the pointer is already on the avatar and
            // moving off it should dismiss immediately.
            hold_until: std::time::Instant::now(),
        });

        // Redraw the widgets too: dropping to zero dims the avatar. The layout
        // is unchanged, so the pointer stays over the same person and the next
        // notch lands where this one did.
        self.refresh();
        self.show_volume_popup(widget);
    }

    fn on_middle_click(&mut self, widget: HWND, point: POINT) {
        let Some(user_id) = self.user_at(widget, point) else {
            return;
        };
        match self.theme.middle_click {
            MiddleClick::None => {}
            MiddleClick::LocalMute => self.toggle_local_mute(&user_id),
            MiddleClick::VolumeReset => self.set_user_volume(&user_id, 100.0, widget),
        }
    }

    fn toggle_local_mute(&mut self, user_id: &str) {
        let Some(participant) = self
            .status
            .participants
            .iter()
            .find(|p| p.user_id == user_id)
        else {
            return;
        };
        let muted = !participant.local_mute;

        if self.control.set_user_voice(user_id, None, Some(muted)) {
            if let Some(participant) = self.status.participant_mut(user_id) {
                participant.local_mute = muted;
            }
            self.refresh();
        }
    }

    fn set_user_volume(&mut self, user_id: &str, volume: f32, widget: HWND) {
        if !self.control.set_user_voice(user_id, Some(volume), None) {
            return;
        }
        let name = self
            .status
            .participants
            .iter()
            .find(|p| p.user_id == user_id)
            .map(|p| p.display_name.clone())
            .unwrap_or_default();

        if let Some(participant) = self.status.participant_mut(user_id) {
            participant.volume = volume;
        }
        self.volume_overlay = Some(VolumeOverlay {
            user_id: user_id.to_string(),
            name,
            volume,
            hold_until: std::time::Instant::now() + VOLUME_OVERLAY_HOLD,
        });
        self.refresh();
        self.show_volume_popup(widget);
    }

    /// Dismiss the volume readout unless the pointer is still on that person.
    ///
    /// `point` is the pointer in widget client coordinates when a message
    /// supplied one; otherwise it is read from the system, which is what makes
    /// this usable as a periodic check.
    fn dismiss_volume_if_unhovered(&mut self, widget: Option<HWND>, point: Option<POINT>) {
        let Some(overlay) = self.volume_overlay.as_ref() else {
            return;
        };
        if std::time::Instant::now() < overlay.hold_until {
            return;
        }
        let user_id = overlay.user_id.clone();

        let located = match (widget, point) {
            (Some(widget), Some(point)) => Some((widget, point)),
            _ => self.cursor_over_surface(),
        };

        let still_there = located
            .and_then(|(widget, point)| self.user_at(widget, point))
            .is_some_and(|hovered| hovered == user_id);

        if !still_there {
            self.hide_popup();
        }
    }

    /// The widget under the pointer and the pointer within it, if any.
    fn cursor_over_surface(&self) -> Option<(HWND, POINT)> {
        unsafe {
            let mut screen = POINT::default();
            GetCursorPos(&mut screen).ok()?;

            for surface in &self.surfaces {
                let mut rect = RECT::default();
                if GetWindowRect(surface.hwnd, &mut rect).is_err() {
                    continue;
                }
                if screen.x < rect.left
                    || screen.x >= rect.right
                    || screen.y < rect.top
                    || screen.y >= rect.bottom
                {
                    continue;
                }

                let mut point = screen;
                let _ = ScreenToClient(surface.hwnd, &mut point);
                return Some((surface.hwnd, point));
            }
        }
        None
    }

    /// Draw and position the volume popup, centred on the display whose widget
    /// was interacted with, just clear of that taskbar.
    fn show_volume_popup(&mut self, widget: HWND) {
        let Some(index) = self.surface_index(widget) else {
            return;
        };
        self.draw_volume_popup(index);
    }

    fn draw_volume_popup(&mut self, index: usize) {
        // Copy what the overlay says up front: drawing needs `&mut self` for
        // the font and image caches.
        let Some((name, volume, user_id)) = self
            .volume_overlay
            .as_ref()
            .map(|o| (o.name.clone(), o.volume, o.user_id.clone()))
        else {
            return;
        };
        let Some(surface) = self.surfaces.get(index) else {
            return;
        };
        let info = surface.taskbar.clone();
        let dpi = info.dpi;
        let Some(font) = self.font_for(dpi) else {
            return;
        };
        let participant = self
            .status
            .participants
            .iter()
            .find(|p| p.user_id == user_id)
            .cloned();

        if self.popup_canvas.is_none() {
            self.popup_canvas = Canvas::new(1, 1);
        }
        let Some(canvas) = self.popup_canvas.as_mut() else {
            self.return_font(dpi, font);
            return;
        };

        let mut view = popup::VolumeView {
            name: &name,
            volume,
            participant: participant.as_ref(),
            theme: &self.theme,
            font: &font,
            images: &mut self.images,
            dpi,
        };

        let size = popup::draw_volume(canvas, &mut view);
        self.return_font(dpi, font);

        let Some((width, height)) = size else {
            return;
        };

        // Centred on the bar, just clear of it.
        let margin = self.scale_at(dpi, 6);
        let centre = (info.rect.left + info.rect.right) / 2;
        let x = (centre - width / 2)
            .clamp(info.rect.left + margin, info.rect.right - width - margin);

        // Above a bottom-docked bar, below a top-docked one — otherwise the
        // popup would be off-screen for anyone with the taskbar at the top.
        let y = if info.edge == Edge::Top {
            info.rect.bottom + margin
        } else {
            info.rect.top - height - margin
        };

        if self.popup.is_none() {
            self.popup = popup::create();
        }
        if let (Some(hwnd), Some(canvas)) = (self.popup, self.popup_canvas.as_ref()) {
            popup::present(hwnd, canvas, x, y);
        }
    }

    /// Mirror the Discord client's coupling: clicking the microphone while
    /// deafened lifts both, because a muted-but-not-deafened state is what the
    /// user is actually asking for.
    fn toggle_mute(&mut self) {
        let state = self.status.self_state;
        let (mute, deaf) = if state.deaf {
            (false, false)
        } else {
            (!state.mute, state.deaf)
        };
        self.apply_voice(mute, deaf);
    }

    fn toggle_deafen(&mut self) {
        let state = self.status.self_state;
        let deaf = !state.deaf;
        // Deafening mutes; undeafening restores an unmuted mic, which is what
        // the Discord client does.
        self.apply_voice(deaf, deaf);
    }

    /// Send the change and reflect it immediately.
    ///
    /// Discord acknowledges in tens of milliseconds, but waiting for the
    /// round trip before redrawing makes the button feel broken. The optimistic
    /// state is replaced by whatever `VOICE_SETTINGS_UPDATE` reports, so if the
    /// write is refused the widget snaps back rather than lying.
    fn apply_voice(&mut self, mute: bool, deaf: bool) {
        let sent = self.control.set_voice(Some(mute), Some(deaf));

        if sent {
            self.status.self_state.mute = mute;
            self.status.self_state.deaf = deaf;
            if let Some(me) = self.status.participants.iter_mut().find(|p| p.is_self) {
                me.self_mute = mute;
                me.self_deaf = deaf;
            }
        } else {
            self.notice = Some("Not connected to Discord".to_string());
            self.notice_expires = Some(std::time::Instant::now() + NOTICE_LINGER);
        }
        self.refresh();
    }

    /// Per-participant menu, drawn in the widget's own style.
    ///
    /// Only local actions appear. Server mute, server deafen and disconnecting
    /// somebody else have no Discord RPC command — see the README.
    ///
    /// Loops rather than returning after one menu: dismissing by clicking
    /// another participant should move the panel to them, and clicking the
    /// same one again should just close it.
    fn show_user_menu(&mut self, widget: HWND, user_id: &str) {
        const ID_LOCAL_MUTE: usize = 1;
        const ID_FOCUS: usize = 2;

        // The panel supersedes the small readout; leaving both up would show
        // the same number twice.
        self.hide_popup();

        let mut target = user_id.to_string();

        loop {
            let Some(participant) = self
                .status
                .participants
                .iter()
                .find(|p| p.user_id == target)
                .cloned()
            else {
                return;
            };
            let Some(index) = self.surface_index(widget) else {
                return;
            };

            let info = self.surfaces[index].taskbar.clone();
            let dpi = info.dpi;
            let Some(font) = self.font_for(dpi) else {
                return;
            };

            let items = vec![
                menu::Item::Header {
                    name: participant.display_name.clone(),
                },
                menu::Item::Separator,
                menu::Item::Action {
                    id: ID_LOCAL_MUTE,
                    label: if participant.local_mute {
                        "Unmute for me".to_string()
                    } else {
                        "Mute for me".to_string()
                    },
                    icon: Some(if participant.local_mute {
                        Icon::VolumeMuted
                    } else {
                        Icon::Volume
                    }),
                    checked: participant.local_mute,
                    danger: participant.local_mute,
                },
                menu::Item::Volume {
                    value: participant.volume,
                },
                menu::Item::VolumePreset {
                    label: "Reset volume".to_string(),
                    value: 100.0,
                },
                menu::Item::Separator,
                menu::Item::Action {
                    id: ID_FOCUS,
                    label: "Focus Discord".to_string(),
                    icon: None,
                    checked: false,
                    danger: false,
                },
            ];

            // Anchored over the pointer, opening away from the taskbar.
            let anchor = unsafe {
                let mut rect = RECT::default();
                let _ = GetWindowRect(widget, &mut rect);
                let mut cursor = POINT::default();
                let _ = GetCursorPos(&mut cursor);
                POINT {
                    x: cursor.x.clamp(rect.left, rect.right),
                    y: if info.edge == Edge::Top {
                        info.rect.bottom
                    } else {
                        info.rect.top
                    },
                }
            };

            // Volume is applied from inside the menu while the bar is dragged,
            // so it is audible immediately and the menu stays open. The
            // control is cheap to clone and borrows nothing else here.
            let control = self.control.clone();
            let applying_to = target.clone();
            let apply = move |volume: f32| {
                control.set_user_voice(&applying_to, Some(volume), None);
            };

            let outcome = {
                let mut style = menu::Style {
                    theme: &self.theme,
                    font: &font,
                    icon_fonts: &mut self.icon_fonts,
                    images: &mut self.images,
                    participant: Some(&participant),
                    dpi,
                    on_volume: Some(&apply),
                };
                menu::show(&items, &mut style, anchor, info.edge != Edge::Top, info.rect)
            };
            self.return_font(dpi, font);

            // Reconcile our own copy with whatever the bar was left at;
            // Discord's own event will confirm it shortly.
            if let Some(volume) = outcome.volume {
                if let Some(participant) = self.status.participant_mut(&target) {
                    participant.volume = volume;
                }
                self.refresh();
            }

            match outcome.choice {
                Some(menu::Choice::Action(ID_LOCAL_MUTE)) => {
                    self.toggle_local_mute(&target);
                    return;
                }
                Some(menu::Choice::Action(ID_FOCUS)) => {
                    tray::focus_discord();
                    return;
                }
                Some(_) => return,
                None => {}
            }

            // Dismissed by clicking outside. If that click was on a different
            // participant, show theirs; on the same one, it was a toggle.
            let Some(screen) = outcome.dismissed_at else {
                return;
            };
            let Some(next) = self.user_at_screen(widget, screen) else {
                return;
            };
            if next == target {
                return;
            }
            target = next;
        }
    }

    /// Which participant is under a screen position, for a widget.
    fn user_at_screen(&mut self, widget: HWND, screen: POINT) -> Option<String> {
        let mut point = screen;
        unsafe {
            let mut rect = RECT::default();
            GetWindowRect(widget, &mut rect).ok()?;
            if point.x < rect.left
                || point.x >= rect.right
                || point.y < rect.top
                || point.y >= rect.bottom
            {
                return None;
            }
            let _ = ScreenToClient(widget, &mut point);
        }
        self.user_at(widget, point)
    }

    fn on_provider_event(&mut self, event: ProviderEvent) {
        match event {
            ProviderEvent::Status(status) => {
                // A transient message outlives a status update; a real one does
                // not, so only clear notices that are not on a timer.
                if self.notice_expires.is_none() {
                    self.notice = None;
                }
                self.status = (*status).clone();
                self.update_tooltip();
            }
            ProviderEvent::AwaitingAuthorization => {
                self.notice = Some("Authorize in Discord".to_string());
            }
            ProviderEvent::CommandFailed(message) => {
                // Most likely another RPC client holds Discord's voice-settings
                // lock. Show it briefly rather than appearing to ignore a click.
                self.notice = Some(message);
                self.notice_expires = Some(std::time::Instant::now() + NOTICE_LINGER);
            }
            ProviderEvent::Offline(reason) => {
                // Discord simply not running is the normal idle case, not
                // something worth putting on the taskbar.
                self.notice_expires = None;
                self.notice = if reason.contains("not running") {
                    None
                } else {
                    Some(reason)
                };
            }
        }
        self.refresh();
    }

    fn update_tooltip(&self) {
        let Some(tray) = &self.tray else { return };
        let text = if self.status.is_connected() {
            format!("Discord Taskbar — {}", self.status.location_label())
        } else {
            "Discord Taskbar — not in voice".to_string()
        };
        tray.set_tooltip(&text);
    }

    fn on_menu_command(&mut self, command: usize) {
        match command {
            tray::CMD_FOCUS_DISCORD => {
                tray::focus_discord();
            }
            tray::CMD_RECONNECT => self.control.reconnect(),
            tray::CMD_RESTART => tray::restart(),
            tray::CMD_REAUTHORIZE => self.control.reauthorize(),
            tray::CMD_SETTINGS => self.open_settings(),
            tray::CMD_OPEN_CONFIG => tray::open_folder(&crate::config::config_dir()),
            // Out of process, so the checks see the machine as a fresh
            // launch would and a hung provider thread cannot block them.
            tray::CMD_DOCTOR => self.open_doctor(),
            tray::CMD_QUIT => unsafe {
                PostQuitMessage(0);
            },
            tray::CMD_MONITOR_ALL => self.show_on_all_monitors(),
            command if command >= tray::CMD_MONITOR_BASE => {
                self.toggle_monitor(command - tray::CMD_MONITOR_BASE)
            }
            _ => {}
        }
    }

    fn show_context_menu(&mut self) {
        let connected = self.status.is_connected();

        // Offer every taskbar the system has, ticking the ones in use.
        let monitors: Vec<(usize, String, bool, bool)> = taskbar::find_all()
            .into_iter()
            .map(|bar| {
                let shown =
                    self.theme
                        .wants_monitor(bar.monitor_index, &bar.monitor, bar.is_primary);
                (bar.monitor_index, bar.monitor, shown, bar.is_primary)
            })
            .collect();

        let choice = self
            .tray
            .as_ref()
            .and_then(|t| t.show_menu(connected, &monitors));

        if let Some(command) = choice {
            self.on_menu_command(command);
        }
    }

    /// Open the settings window, applying anything it saves immediately.
    ///
    /// The window is modeless and lives on this thread, so the widget carries
    /// on working while it is open — and a change can be seen the moment it is
    /// saved rather than after a restart.
    /// Launch a second copy with `--doctor`.
    ///
    /// A separate process on purpose: the report should describe what a fresh
    /// launch would find, and running it here would block the message loop
    /// for as long as the Discord handshake takes.
    fn open_doctor(&mut self) {
        let Ok(exe) = std::env::current_exe() else {
            self.notice = Some("Could not locate the app".to_string());
            return;
        };
        let _ = std::process::Command::new(exe).arg("--doctor").spawn();
    }

    fn open_settings(&mut self) {
        let (config, _) = Config::load_or_create();
        settings::open(
            &config,
            Box::new(|edited| {
                // Runs on the UI thread from the settings window's handler,
                // where the app is not already borrowed.
                with_app(|app| app.apply_config(edited));
            }),
        );
    }

    /// Adopt an edited config: persist it, then redraw with it.
    fn apply_config(&mut self, config: Config) {
        if let Err(error) = config.save() {
            self.notice = Some(format!("Could not save config: {error}"));
            self.notice_expires = Some(std::time::Instant::now() + NOTICE_LINGER);
        }

        self.theme = Theme::from(&config.appearance);

        // Metrics and colours may all have moved.
        self.rebuild_fonts();
        self.icon_fonts.clear();
        self.images.clear();
        for surface in &mut self.surfaces {
            surface.canvas = None;
            surface.backdrop_stale = true;
        }

        self.refresh();
    }

    /// Turn a display on or off and persist the choice.
    fn toggle_monitor(&mut self, index: usize) {
        let bars = taskbar::find_all();
        let Some(bar) = bars.iter().find(|b| b.monitor_index == index) else {
            return;
        };

        let shown = self
            .theme
            .wants_monitor(bar.monitor_index, &bar.monitor, bar.is_primary);

        // Selectors can be `primary`, `all`, indices or device names, so
        // toggling one has to resolve the lot into an explicit index list
        // first — otherwise turning off "all" would have nothing to remove.
        let mut selected: Vec<usize> = bars
            .iter()
            .filter(|b| {
                self.theme
                    .wants_monitor(b.monitor_index, &b.monitor, b.is_primary)
            })
            .map(|b| b.monitor_index)
            .collect();

        if shown {
            selected.retain(|i| *i != index);
        } else if !selected.contains(&index) {
            selected.push(index);
        }

        // Never end up with nothing showing anywhere.
        if selected.is_empty() {
            selected.push(index);
        }

        selected.sort_unstable();
        self.set_monitors(selected.iter().map(|i| i.to_string()).collect());
    }

    fn show_on_all_monitors(&mut self) {
        self.set_monitors(vec!["all".to_string()]);
    }

    fn set_monitors(&mut self, monitors: Vec<String>) {
        self.theme.monitors = monitors.clone();

        // Persist, so the choice survives a restart. A failure here is worth
        // saying out loud: the widget will move now and revert later.
        let (mut config, _) = Config::load_or_create();
        config.appearance.monitors = monitors;
        if let Err(error) = config.save() {
            self.notice = Some(format!("Could not save config: {error}"));
            self.notice_expires = Some(std::time::Instant::now() + NOTICE_LINGER);
        }

        self.refresh();
    }

    /// Cheap periodic check: have the taskbars moved, resized, or died?
    fn on_timer(&mut self) {
        // Re-check taskbar colours now and then, so a sample taken while
        // something was covering a bar cannot stick permanently.
        if !self.is_idle() {
            for surface in &mut self.surfaces {
                let due = surface
                    .backdrop_checked
                    .is_none_or(|at| at.elapsed() >= BACKDROP_RECHECK);
                if due {
                    surface.backdrop_stale = true;
                }
            }
        }

        // Safety net for the readout. Hover and leave events normally dismiss
        // it, but a missed WM_MOUSELEAVE would otherwise strand it on screen,
        // so the real pointer position is checked once a second too.
        self.dismiss_volume_if_unhovered(None, None);

        // Retire a transient message once it has had its moment.
        if let Some(expiry) = self.notice_expires {
            if std::time::Instant::now() >= expiry {
                self.notice = None;
                self.notice_expires = None;
            }
        }

        let bars = taskbar::find_all();
        let changed = bars.len() != self.surfaces.len()
            || bars.iter().any(|bar| {
                self.surfaces
                    .iter()
                    .find(|s| s.taskbar.monitor == bar.monitor)
                    .is_none_or(|s| {
                        !rects_equal(&s.taskbar.rect, &bar.rect)
                            || s.taskbar.dpi != bar.dpi
                            || s.taskbar.tray != bar.tray
                            || s.taskbar.notify_rect.map(|r| r.left)
                                != bar.notify_rect.map(|r| r.left)
                    })
            });

        let all_alive = !self.surfaces.is_empty()
            && self
                .surfaces
                .iter()
                .all(|s| unsafe { IsWindow(Some(s.hwnd)).as_bool() });

        let wants_redraw = self.surfaces.iter().any(|s| s.backdrop_stale);

        if changed || !all_alive || wants_redraw {
            self.refresh();
        }
    }
}

fn rects_equal(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

/// Create the windows, start the provider, and pump messages.
pub fn run(
    config: Config,
    warning: Option<String>,
    demo: bool,
    open_settings: bool,
) -> Result<(), String> {
    unsafe {
        // Match explorer's awareness so our coordinates agree with the tray's.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        if !widget::register_class() {
            return Err("could not register the widget window class".to_string());
        }
        register_host_class()?;

        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;

        // Top-level, not message-only: HWND_MESSAGE windows do not receive the
        // TaskbarCreated broadcast, which is how we survive explorer restarts.
        let host = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("Discord Taskbar"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())?;

        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));

        let control = if demo {
            ProviderControl::demo()
        } else {
            ProviderControl::new()
        };

        widget::set_host(host);
        APP.with(|cell| {
            let mut app = App::new(
                host,
                Theme::from(&config.appearance),
                taskbar_created,
                control.clone(),
            );
            app.rebuild_fonts();
            app.update_tooltip();
            if let Some(warning) = warning {
                // Shown until the user does something about it, since a bad
                // config means the app is not doing what they asked.
                app.notice = Some(warning);
            }
            app.refresh();
            *cell.borrow_mut() = Some(app);
        });

        if demo {
            provider::spawn_demo(ProviderSink::new(host));
        } else {
            provider::spawn_rpc(config.discord.clone(), ProviderSink::new(host), control);
        }

        // Nothing works without a Discord application, so say so rather than
        // sitting there doing nothing. The settings window explains how.
        let needs_setup = !demo && !config.discord.is_complete();
        if open_settings || needs_setup {
            with_app(|app| {
                if needs_setup {
                    app.notice = Some("Set up Discord - see Settings".to_string());
                    app.refresh();
                }
                app.open_settings();
            });
        }

        SetTimer(Some(host), TIMER_ANCHOR, TIMER_ANCHOR_MS, None);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        // Drop the whole App so the tray icon is removed cleanly.
        APP.with(|cell| {
            if let Some(mut app) = cell.borrow_mut().take() {
                app.drop_surfaces();
            }
        });

        Ok(())
    }
}

unsafe fn register_host_class() -> Result<(), String> {
    let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;

    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: instance.into(),
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };

    if RegisterClassExW(&class) == 0 {
        let error = windows::Win32::Foundation::GetLastError();
        if error != windows::Win32::Foundation::WIN32_ERROR(1410) {
            return Err(format!("RegisterClassExW failed: {error:?}"));
        }
    }
    Ok(())
}

fn with_app(action: impl FnOnce(&mut App)) {
    let _ = try_with_app(action);
}

/// Returns false when the app was already borrowed — i.e. this call arrived
/// reentrantly, from inside a refresh.
fn try_with_app(action: impl FnOnce(&mut App)) -> bool {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut borrow) => match borrow.as_mut() {
            Some(app) => {
                action(app);
                true
            }
            None => false,
        },
        Err(_) => false,
    })
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // TaskbarCreated is registered at runtime, so it cannot be a match arm.
        let taskbar_created = APP.with(|cell| {
            cell.try_borrow()
                .ok()
                .and_then(|b| b.as_ref().map(|a| a.taskbar_created))
                .unwrap_or(0)
        });

        if msg != 0 && msg == taskbar_created {
            // Explorer restarted and took our child window with it.
            with_app(|app| {
                app.drop_surfaces();
                app.refresh();
            });
            return LRESULT(0);
        }

        match msg {
            WM_APP_STATUS => {
                if let Some(event) = provider::take_event(wparam.0) {
                    with_app(|app| app.on_provider_event(event));
                }
                LRESULT(0)
            }

            WM_APP_ASSET_READY => {
                with_app(|app| {
                    if app.images.collect() {
                        app.refresh();
                    }
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == TIMER_ANCHOR => {
                with_app(|app| app.on_timer());
                LRESULT(0)
            }

            // Anything that can move the taskbar or change its metrics.
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED | WM_THEMECHANGED => {
                with_app(|app| {
                    for surface in &mut app.surfaces {
                        surface.canvas = None;
                        surface.backdrop_stale = true;
                    }
                    app.rebuild_fonts();
                    app.refresh();
                });
                LRESULT(0)
            }

            WM_APP_WIDGET_PAINT => {
                // wparam is the DC, lparam the widget that is painting.
                let widget = HWND(lparam.0 as *mut std::ffi::c_void);
                let dc = HDC(wparam.0 as *mut std::ffi::c_void);
                let painted = try_with_app(|app| app.paint(widget, dc));
                // Non-zero tells the widget the paint actually happened.
                LRESULT(painted as isize)
            }

            WM_APP_TRAY => {
                // The low word of lParam is the mouse event.
                match (lparam.0 as u32) & 0xFFFF {
                    WM_RBUTTONUP | WM_CONTEXTMENU => with_app(|app| app.show_context_menu()),
                    WM_LBUTTONDBLCLK => {
                        tray::focus_discord();
                    }
                    _ => {}
                }
                LRESULT(0)
            }

            WM_APP_WIDGET_CLICK => {
                if let Some(input) = take_widget_input(wparam.0) {
                    with_app(|app| app.on_widget_click(input.widget, input.point));
                }
                LRESULT(0)
            }

            WM_APP_WIDGET_CONTEXT => {
                let _ = take_widget_input(wparam.0);
                with_app(|app| app.show_context_menu());
                LRESULT(0)
            }

            WM_APP_WIDGET_HOVER => {
                if let Some(input) = take_widget_input(wparam.0) {
                    with_app(|app| {
                        app.dismiss_volume_if_unhovered(Some(input.widget), Some(input.point))
                    });
                }
                LRESULT(0)
            }

            WM_APP_WIDGET_LEAVE => {
                let _ = take_widget_input(wparam.0);
                with_app(|app| app.hide_popup());
                LRESULT(0)
            }

            WM_APP_WIDGET_MIDDLE => {
                if let Some(input) = take_widget_input(wparam.0) {
                    with_app(|app| app.on_middle_click(input.widget, input.point));
                }
                LRESULT(0)
            }

            WM_APP_WIDGET_WHEEL => {
                if let Some(input) = take_widget_input(wparam.0) {
                    with_app(|app| app.on_wheel(input.widget, input.notches, input.point));
                }
                LRESULT(0)
            }

            WM_APP_WIDGET_CURSOR => {
                let mut interactive = false;
                if let Some(input) = take_widget_input(wparam.0) {
                    try_with_app(|app| {
                        interactive = app.is_interactive(input.widget, input.point)
                    });
                }
                LRESULT(interactive as isize)
            }

            WM_DESTROY => {
                KillTimer(Some(hwnd), TIMER_ANCHOR).ok();
                PostQuitMessage(0);
                LRESULT(0)
            }

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
