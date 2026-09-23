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

use crate::assets::icons::IconFonts;
use crate::assets::images::{ImageCache, ImageRef};
use crate::config::Config;

use super::block::{self, BlockId, Context};
use super::integration::{
    self, Event, EventSink, Gesture, Integration, Interaction, Meter, TrayItem, Ui,
    TRAY_ID_BASE,
};
use super::menu;
use super::popup;
use super::render::{Canvas, Color, Font};
use super::settings;
use super::taskbar::{self, Anchor, Edge, TaskbarInfo, DEFAULT_DPI};
use super::theme::Theme;
use super::tray::{self, Tray};
use super::widget;
use super::{
    take_widget_input, Notifier, TIMER_ANCHOR, TIMER_ANCHOR_MS, WM_APP_ASSET_READY,
    WM_APP_INTEGRATION, WM_APP_TRAY,
    WM_APP_WIDGET_CLICK, WM_APP_WIDGET_CONTEXT, WM_APP_WIDGET_CURSOR, WM_APP_WIDGET_HOVER,
    WM_APP_WIDGET_LEAVE, WM_APP_WIDGET_MIDDLE, WM_APP_WIDGET_PAINT, WM_APP_WIDGET_WHEEL,
};

const CLASS_NAME: windows::core::PCWSTR = w!("DiscordTaskbarHost");

/// The widget's own tray commands all sit below
/// [`super::integration::TRAY_ID_BASE`]; anything above belongs to the
/// integration.

/// Keep this much clear of the taskbar edges and the clock.
const EDGE_MARGIN: i32 = 8;

/// How long a transient message (a refused command) stays on screen.
const NOTICE_LINGER: std::time::Duration = std::time::Duration::from_secs(4);

/// The one-line readout. It lives for as long as the pointer stays on the
/// block it describes, rather than for a fixed time: tying it to the pointer
/// is what makes it feel attached to what you are doing.
struct MeterState {
    about: BlockId,
    title: String,
    value_text: String,
    fraction: f32,
    fill: Color,
    image: Option<ImageRef>,
    image_size: i32,
    /// Not dismissed before this instant, whatever the pointer is doing.
    hold_until: std::time::Instant,
}


thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// One widget, on one taskbar.
///
/// Everything here is per-display; the rest of the state is shared. Splitting
/// it this way is what lets the widget appear on several monitors at once
/// without duplicating fonts, avatars or the status itself.
struct Surface {
    hwnd: HWND,
    /// False when the shell would not adopt us and we fell back to a
    /// free-floating top-most window.
    parented: bool,
    taskbar: TaskbarInfo,
    canvas: Option<Canvas>,
    /// Where everything was last drawn, so a click can be routed back to
    /// whichever block owns that pixel.
    layout: Option<block::Layout>,
}

struct App {
    surfaces: Vec<Surface>,
    /// One font per DPI: displays can differ, and a font built for 96 looks
    /// wrong on a 144 DPI panel.
    fonts: HashMap<u32, Font>,
    theme: Theme,
    taskbar_created: u32,

    /// What the widget is showing. Taken out of `self` while it runs, so it
    /// can be handed the `Ui` that borrows the rest.
    integration: Option<Box<dyn Integration>>,
    images: ImageCache,
    icon_fonts: IconFonts,
    /// Shown instead of the status when something needs the user's attention.
    notice: Option<String>,
    /// When set, `notice` clears itself at this time.
    notice_expires: Option<std::time::Instant>,
    /// Lives in its own window above the taskbar so the widget — and the
    /// block it is about — stays exactly where it is.
    meter: Option<MeterState>,
    popup: Option<HWND>,
    popup_canvas: Option<Canvas>,
    tray: Option<Tray>,
}

impl App {
    fn new(
        host: HWND,
        theme: Theme,
        taskbar_created: u32,
        integration: Box<dyn Integration>,
    ) -> Self {
        let (glyph, colour) = integration.tray_icon();
        let tooltip = integration.tooltip();
        let tray = Tray::new(host, WM_APP_TRAY, glyph, colour);
        if let Some(tray) = &tray {
            tray.set_tooltip(&tooltip);
        }

        App {
            surfaces: Vec::new(),
            fonts: HashMap::new(),
            theme,
            taskbar_created,
            integration: Some(integration),
            images: ImageCache::new(Notifier::new(host, WM_APP_ASSET_READY)),
            icon_fonts: IconFonts::new(),
            notice: None,
            notice_expires: None,
            meter: None,
            popup: None,
            popup_canvas: None,
            tray,
        }
    }

    /// Run `action` with the integration and a `Ui` onto this app.
    ///
    /// The integration is a field of `App` and the `Ui` borrows `App`, so the
    /// two cannot be held at once. Taking it out for the duration is what lets
    /// an integration act on the widget while it is deciding what to do.
    fn dispatch(
        &mut self,
        widget: Option<HWND>,
        action: impl FnOnce(&mut dyn Integration, &mut dyn Ui),
    ) {
        let Some(mut integration) = self.integration.take() else {
            return;
        };
        {
            let mut ui = HostUi { app: self, widget };
            action(integration.as_mut(), &mut ui);
        }
        self.integration = Some(integration);
    }

    /// What the integration wants drawn right now.
    fn blocks(&self) -> Vec<block::Block> {
        match &self.integration {
            Some(integration) => integration.blocks(&self.theme),
            None => Vec::new(),
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
        self.notice.is_none() && self.blocks().is_empty()
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
                    // A taskbar recreated by explorer has a new window, so the
                    // widget has to be re-parented to it.
                    if surface.taskbar.tray != bar.tray {
                        surface.parented = widget::attach(surface.hwnd, bar.tray);
                        if !surface.parented {
                            widget::make_topmost(surface.hwnd);
                        }
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
                        layout: None,
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
        self.meter = None;
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

        let radius = self.scale_at(dpi, self.theme.corner_radius);
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

            // Outside the rounded rect stays transparent and the taskbar shows
            // through for real.
            //
            // Except that it cannot be *quite* transparent. A layered window
            // passes mouse messages straight through any pixel with zero
            // alpha, so a fully transparent background would hand the gaps
            // between avatars back to the taskbar, and with them the moves
            // that drive hover, the wheel and WM_MOUSELEAVE. An alpha of 1 is
            // invisible and keeps the whole rectangle hit-testable.
            //
            // TrafficMonitor carries the same workaround for a different
            // reason: it nudges a pure black colour key to 1, because Windows
            // 11 mishandles the fully transparent case there too.
            canvas.fill_rect(full, Color::rgba(0, 0, 0, 1));
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

        // One call moves, resizes and presents, so there is no WM_PAINT and no
        // moment where the window is the right size but still holds the
        // previous frame.
        widget::show(hwnd);
        if let Some(canvas) = self.surfaces[index].canvas.as_ref() {
            canvas.present_layered_at(hwnd, x, y);
        }
    }

    /// Lay the blocks out for one surface, optionally drawing them.
    ///
    /// Takes the canvas and font out of `self` for the duration so the block
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
            self.surfaces[index].layout = None;
            text_width + padding * 2
        } else {
            let blocks = self.blocks();
            let mut ctx = Context {
                theme: &self.theme,
                font: &font,
                icon_fonts: &mut self.icon_fonts,
                images: &mut self.images,
                backdrop: self.theme.background,
                dpi,
            };
            let layout = block::layout(&mut canvas, &mut ctx, &blocks, height, draw);
            let width = layout.width;
            self.surfaces[index].layout = Some(layout);
            width
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

    /// Hand a gesture to the integration, with whatever block it landed on.
    fn dispatch_gesture(&mut self, widget: HWND, gesture: Gesture, point: POINT) {
        let target = self.block_at(widget, point);
        let screen = client_to_screen(widget, point);
        self.dispatch(Some(widget), |integration, ui| {
            integration.on_interaction(
                Interaction {
                    gesture,
                    target,
                    screen,
                },
                ui,
            );
        });
    }

    /// Whether a point is worth showing a hand cursor over.
    fn is_interactive(&mut self, widget: HWND, point: POINT) -> bool {
        self.block_at(widget, point).is_some()
    }

    /// Which block is under a point in a widget's client area.
    fn block_at(&self, widget: HWND, point: POINT) -> Option<BlockId> {
        let index = self.surface_index(widget)?;
        self.surfaces[index].layout.as_ref()?.hit(point)
    }

    /// Which block is under a screen position, for any of our widgets.
    fn block_at_screen(&self, screen: POINT) -> Option<BlockId> {
        let (widget, point) = surface_at(&self.surfaces, screen)?;
        self.block_at(widget, point)
    }

    /// Dismiss the readout once the pointer is no longer on what it describes.
    fn dismiss_meter_if_unhovered(&mut self, widget: Option<HWND>, point: Option<POINT>) {
        let Some(meter) = &self.meter else {
            return;
        };
        if std::time::Instant::now() < meter.hold_until {
            return;
        }
        let about = meter.about.clone();

        let located = match (widget, point) {
            (Some(widget), Some(point)) => Some((widget, point)),
            _ => self.cursor_over_surface(),
        };

        let still_there = located
            .and_then(|(widget, point)| self.block_at(widget, point))
            .is_some_and(|hovered| hovered == about);

        if !still_there {
            self.hide_popup();
        }
    }

    /// The widget under the pointer and the pointer within it, if any.
    fn cursor_over_surface(&self) -> Option<(HWND, POINT)> {
        let screen = unsafe {
            let mut screen = POINT::default();
            GetCursorPos(&mut screen).ok()?;
            screen
        };
        surface_at(&self.surfaces, screen)
    }
}

/// Which of `surfaces` is under a screen position, and where within it.
fn surface_at(surfaces: &[Surface], screen: POINT) -> Option<(HWND, POINT)> {
    unsafe {
        {
            for surface in surfaces {
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
    }
    None
}

impl App {

    /// Draw and position the readout, centred on the display whose widget was
    /// interacted with, just clear of that taskbar.
    fn show_meter_popup(&mut self, widget: Option<HWND>) {
        let index = widget
            .and_then(|widget| self.surface_index(widget))
            .or(if self.surfaces.is_empty() { None } else { Some(0) });
        let Some(index) = index else { return };
        self.draw_meter_popup(index);
    }

    fn draw_meter_popup(&mut self, index: usize) {
        // Copy what the readout says up front: drawing needs `&mut self` for
        // the font and image caches.
        let Some((title, value_text, fraction, fill, image, image_size)) =
            self.meter.as_ref().map(|m| {
                (
                    m.title.clone(),
                    m.value_text.clone(),
                    m.fraction,
                    m.fill,
                    m.image.clone(),
                    m.image_size,
                )
            })
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

        if self.popup_canvas.is_none() {
            self.popup_canvas = Canvas::new(1, 1);
        }
        let Some(canvas) = self.popup_canvas.as_mut() else {
            self.return_font(dpi, font);
            return;
        };

        let mut view = popup::Meter {
            title: &title,
            value_text,
            fraction,
            fill,
            image,
            image_size,
            theme: &self.theme,
            font: &font,
            images: &mut self.images,
            dpi,
        };

        let size = popup::draw_meter(canvas, &mut view);
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

    fn on_integration_event(&mut self, event: Event) {
        let mut redraw = false;
        self.dispatch(None, |integration, ui| {
            redraw = integration.on_event(event, ui);
        });
        if redraw {
            self.update_tooltip();
            self.refresh();
        }
    }

    fn update_tooltip(&self) {
        let (Some(tray), Some(integration)) = (&self.tray, &self.integration) else {
            return;
        };
        tray.set_tooltip(&integration.tooltip());
    }

    fn on_menu_command(&mut self, command: usize) {
        match command {
            tray::CMD_RESTART => tray::restart(),
            tray::CMD_SETTINGS => self.open_settings(),
            tray::CMD_OPEN_CONFIG => tray::open_folder(&crate::config::config_dir()),
            // Out of process, so the checks see the machine as a fresh
            // launch would and a hung provider thread cannot block them.
            tray::CMD_DOCTOR => self.open_doctor(),
            tray::CMD_QUIT => unsafe {
                PostQuitMessage(0);
            },
            tray::CMD_MONITOR_ALL => self.show_on_all_monitors(),
            command if command >= TRAY_ID_BASE => {
                self.dispatch(None, |integration, ui| {
                    integration.on_tray_command(command, ui);
                });
            }
            command if command >= tray::CMD_MONITOR_BASE => {
                self.toggle_monitor(command - tray::CMD_MONITOR_BASE)
            }
            _ => {}
        }
    }

    /// The notification-area menu: the integration's rows, then the widget's.
    fn show_context_menu(&mut self) {
        let mut items: Vec<TrayItem> = match &self.integration {
            Some(integration) => integration.tray_items(),
            None => Vec::new(),
        };
        if !items.is_empty() {
            items.push(TrayItem::Separator);
        }

        // Offer every taskbar the system has, ticking the ones in use.
        let bars = taskbar::find_all();
        if !bars.is_empty() {
            let shown: Vec<bool> = bars
                .iter()
                .map(|bar| {
                    self.theme
                        .wants_monitor(bar.monitor_index, &bar.monitor, bar.is_primary)
                })
                .collect();

            let mut displays = vec![
                TrayItem::Command {
                    id: tray::CMD_MONITOR_ALL,
                    label: "All displays".to_string(),
                    checked: shown.iter().all(|s| *s),
                    enabled: true,
                },
                TrayItem::Separator,
            ];

            for (bar, shown) in bars.iter().zip(shown.iter()) {
                displays.push(TrayItem::Command {
                    id: tray::CMD_MONITOR_BASE + bar.monitor_index,
                    // Device names like \.\DISPLAY1 mean nothing to most
                    // people, so lead with the position.
                    label: format!(
                        "Display {}{}   {}",
                        bar.monitor_index + 1,
                        if bar.is_primary { " (primary)" } else { "" },
                        bar.monitor.trim_start_matches(r"\.\")
                    ),
                    checked: *shown,
                    enabled: true,
                });
            }

            items.push(TrayItem::Submenu {
                label: "Show on".to_string(),
                items: displays,
            });
            items.push(TrayItem::Separator);
        }

        items.extend([
            TrayItem::command(tray::CMD_SETTINGS, "Settings..."),
            TrayItem::command(tray::CMD_OPEN_CONFIG, "Open config folder"),
            TrayItem::command(tray::CMD_DOCTOR, "Diagnostics..."),
            TrayItem::command(tray::CMD_RESTART, "Restart (reload config)"),
            TrayItem::Separator,
            TrayItem::command(tray::CMD_QUIT, "Quit"),
        ]);

        let choice = self.tray.as_ref().and_then(|t| t.show_menu(&items));
        if let Some(command) = choice {
            self.on_menu_command(command);
        }
    }

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

    /// Open the settings window, applying anything it saves immediately.
    ///
    /// The window is modeless and lives on this thread, so the widget carries
    /// on working while it is open — and a change can be seen the moment it is
    /// saved rather than after a restart.
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
        // Safety net for the readout. Hover and leave events normally dismiss
        // it, but a missed WM_MOUSELEAVE would otherwise strand it on screen,
        // so the real pointer position is checked once a second too.
        self.dismiss_meter_if_unhovered(None, None);

        // Retire a transient message once it has had its moment.
        if let Some(expiry) = self.notice_expires {
            if std::time::Instant::now() >= expiry {
                self.notice = None;
                self.notice_expires = None;
            }
        }

        // Only the bars we actually want a widget on. Comparing against every
        // taskbar on the system meant that showing on one display out of two
        // left the counts permanently unequal, so `changed` was always true
        // and the widget re-laid out and re-presented itself once a second
        // for the life of the call.
        let bars: Vec<TaskbarInfo> = taskbar::find_all()
            .into_iter()
            .filter(|bar| {
                self.theme
                    .wants_monitor(bar.monitor_index, &bar.monitor, bar.is_primary)
            })
            .collect();

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

        if changed || !all_alive {
            self.refresh();
        }
    }
}

fn rects_equal(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

/// Create the windows, start the integration, and pump messages.
///
/// `setup_notice` is shown on the widget until something replaces it, for the
/// case where the integration cannot do anything yet — no credentials, say —
/// and `needs_setup` opens the settings window for the same reason.
pub fn run(
    config: Config,
    integration: Box<dyn Integration>,
    warning: Option<String>,
    setup_notice: Option<String>,
    needs_setup: bool,
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

        widget::set_host(host);
        APP.with(|cell| {
            let mut app = App::new(
                host,
                Theme::from(&config.appearance),
                taskbar_created,
                integration,
            );
            app.rebuild_fonts();
            if let Some(warning) = warning {
                // Shown until the user does something about it, since a bad
                // config means the app is not doing what they asked.
                app.notice = Some(warning);
            }
            app.refresh();
            *cell.borrow_mut() = Some(app);
        });

        // Only now: the integration posts to the host window, which has to
        // exist and have an `App` behind it before the first event lands.
        with_app(|app| {
            app.dispatch(None, |integration, _| {
                integration.start(EventSink::new(host));
            });
        });

        if let Some(message) = setup_notice {
            with_app(|app| {
                app.notice = Some(message);
                app.refresh();
            });
        }
        if open_settings || needs_setup {
            with_app(|app| app.open_settings());
        }

        SetTimer(Some(host), TIMER_ANCHOR, TIMER_ANCHOR_MS, None);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            // The settings window is a modeless child of this thread, so this
            // loop is the only place its keyboard can be handled. Without
            // this its controls have WS_TABSTOP and nothing to honour it:
            // Tab, Escape and the arrow keys all do nothing. The diagnostics
            // window and the installer each pump their own loop and already
            // do this.
            if settings::handle_dialog_key(&message) {
                continue;
            }
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

/// The widget, as an integration is allowed to see it.
///
/// Holds the app for the length of one call, plus the widget the gesture came
/// from so a menu or a readout lands on the right display.
struct HostUi<'a> {
    app: &'a mut App,
    widget: Option<HWND>,
}

impl Ui for HostUi<'_> {
    fn show_menu(
        &mut self,
        items: &[menu::Item],
        on_slide: Option<&dyn Fn(f32)>,
    ) -> menu::Outcome {
        let index = self
            .widget
            .and_then(|widget| self.app.surface_index(widget))
            .or(if self.app.surfaces.is_empty() {
                None
            } else {
                Some(0)
            });
        let Some(index) = index else {
            return menu::Outcome::default();
        };

        let info = self.app.surfaces[index].taskbar.clone();
        let widget = self.app.surfaces[index].hwnd;
        let dpi = info.dpi;
        let Some(font) = self.app.font_for(dpi) else {
            return menu::Outcome::default();
        };

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

        let outcome = {
            let mut style = menu::Style {
                theme: &self.app.theme,
                font: &font,
                icon_fonts: &mut self.app.icon_fonts,
                images: &mut self.app.images,
                dpi,
                on_slide,
            };
            menu::show(items, &mut style, anchor, info.edge != Edge::Top, info.rect)
        };
        self.app.return_font(dpi, font);
        outcome
    }

    fn show_meter(&mut self, meter: Meter) {
        self.app.meter = Some(MeterState {
            about: meter.about,
            title: meter.title,
            value_text: meter.value_text,
            fraction: meter.fraction,
            fill: meter.fill,
            image: meter.image,
            image_size: meter.image_size,
            hold_until: std::time::Instant::now() + meter.hold,
        });
        self.app.show_meter_popup(self.widget);
    }

    fn hide_meter(&mut self) {
        self.app.hide_popup();
    }

    fn notice(&mut self, text: String, linger: Option<std::time::Duration>) {
        self.app.notice = Some(text);
        self.app.notice_expires = linger.map(|d| std::time::Instant::now() + d);
    }

    fn clear_notice(&mut self) {
        // A transient message outlives a status update; a real one does not,
        // so only clear notices that are not on a timer.
        if self.app.notice_expires.is_none() {
            self.app.notice = None;
        }
    }

    fn block_at(&mut self, screen: POINT) -> Option<BlockId> {
        self.app.block_at_screen(screen)
    }

    fn redraw(&mut self) {
        self.app.refresh();
    }

    fn theme(&self) -> &Theme {
        &self.app.theme
    }
}

/// A point in a widget's client area, in screen coordinates.
fn client_to_screen(widget: HWND, point: POINT) -> POINT {
    unsafe {
        let mut screen = point;
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(widget, &mut screen);
        screen
    }
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
            WM_APP_INTEGRATION => {
                if let Some(event) = integration::take_event(wparam.0) {
                    with_app(|app| app.on_integration_event(event));
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
                    WM_LBUTTONDBLCLK => with_app(|app| {
                        // Whatever the integration offers first is its primary
                        // action; double-click is a shortcut to it.
                        let first = app.integration.as_ref().and_then(|i| {
                            i.tray_items().into_iter().find_map(|item| match item {
                                TrayItem::Command { id, enabled: true, .. } => Some(id),
                                _ => None,
                            })
                        });
                        if let Some(id) = first {
                            app.on_menu_command(id);
                        }
                    }),
                    _ => {}
                }
                LRESULT(0)
            }

            WM_APP_WIDGET_CLICK => {
                if let Some(input) = take_widget_input(wparam.0) {
                    with_app(|app| {
                        app.dispatch_gesture(input.widget, Gesture::Click, input.point)
                    });
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
                        app.dismiss_meter_if_unhovered(Some(input.widget), Some(input.point));
                        app.dispatch_gesture(input.widget, Gesture::Hover, input.point);
                    });
                }
                LRESULT(0)
            }

            WM_APP_WIDGET_LEAVE => {
                let input = take_widget_input(wparam.0);
                with_app(|app| {
                    app.hide_popup();
                    if let Some(input) = input {
                        app.dispatch_gesture(input.widget, Gesture::Leave, input.point);
                    }
                });
                LRESULT(0)
            }

            WM_APP_WIDGET_MIDDLE => {
                if let Some(input) = take_widget_input(wparam.0) {
                    with_app(|app| {
                        app.dispatch_gesture(input.widget, Gesture::Middle, input.point)
                    });
                }
                LRESULT(0)
            }

            WM_APP_WIDGET_WHEEL => {
                if let Some(input) = take_widget_input(wparam.0) {
                    with_app(|app| {
                        app.dispatch_gesture(
                            input.widget,
                            Gesture::Wheel(input.notches),
                            input.point,
                        )
                    });
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
