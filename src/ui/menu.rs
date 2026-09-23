//! A small drawn menu, in the same style as the rest of the widget.
//!
//! The native `TrackPopupMenu` works, but it looks like Windows 95 sitting
//! under a panel that does not. This draws its own, reusing the canvas, the
//! icon font and the theme, so a menu opened from the widget belongs to it.
//!
//! The rows are generic: a header, a separator, a command, a slider and a
//! preset that jumps the slider somewhere. Nothing here knows what a slider
//! is measuring.
//!
//! Like the volume readout it is a top-level layered window, so it gets real
//! per-pixel alpha and properly rounded corners. Unlike the readout it has to
//! take input, which means a nested message loop with the mouse captured —
//! the same shape as `TrackPopupMenu`'s own contract: show, block, return
//! what was chosen.

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::assets::icons::{Glyph, IconFonts};
use crate::assets::images::{ImageCache, ImageRef};

use super::render::{Canvas, Color, Font};
use super::theme::Theme;

const CLASS_NAME: windows::core::PCWSTR = w!("DiscordTaskbarMenu");

/// Polls for artwork that arrives after the menu is already up.
const ASSET_TIMER: usize = 1;

/// One row.
pub enum Item {
    /// A picture and a name, not selectable.
    Header {
        name: String,
        image: Option<ImageRef>,
        /// Size of the picture at 96 DPI. Worth matching a size the widget
        /// already draws: that bitmap is then cached, and the header is
        /// populated the instant the menu opens rather than after a fresh
        /// download at a size nothing else uses.
        image_size: i32,
    },
    Separator,
    Action {
        id: usize,
        label: String,
        icon: Option<Glyph>,
        checked: bool,
        danger: bool,
    },
    /// A click-anywhere bar, which is how most volume sliders behave.
    Slider {
        label: String,
        value: f32,
        /// Lowest and highest the bar can be dragged to.
        range: (f32, f32),
        /// Turns a value into the number shown on the right.
        ///
        /// A plain function pointer, not a closure: it covers percentages,
        /// decibels and durations, and it keeps `Item` free of lifetimes.
        format: fn(f32) -> String,
        /// Above this the fill turns to the danger colour — a volume being
        /// boosted past normal, a meter into the red.
        warn_above: Option<f32>,
    },
    /// Jumps the slider to a fixed value. Like the bar and unlike a command,
    /// it leaves the menu open — it is the same control by another route.
    SliderPreset { label: String, value: f32 },
}

impl Item {
    fn selectable(&self) -> bool {
        matches!(
            self,
            Item::Action { .. } | Item::Slider { .. } | Item::SliderPreset { .. }
        )
    }
}

/// What the user picked.
///
/// There is no slider variant. Dragging the bar is a continuous control, and
/// closing the menu the moment it is released would be like a slider that
/// dismisses its own dialog — the change is applied through `on_slide` while
/// the menu stays open. Only discrete commands close it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choice {
    Action(usize),
}

/// Everything drawing needs.
pub struct Style<'a> {
    pub theme: &'a Theme,
    pub font: &'a Font,
    pub icon_fonts: &'a mut IconFonts,
    pub images: &'a mut ImageCache,
    pub dpi: u32,
    /// Applies a value as the bar is dragged, so the change takes effect
    /// while adjusting rather than only once the menu closes.
    ///
    /// A callback rather than a return value because this has to happen mid-
    /// loop, and because it lets the caller hand over something cheap to
    /// clone that does not borrow the rest of its state.
    pub on_slide: Option<&'a dyn Fn(f32)>,
}

impl Style<'_> {
    fn scale(&self, value: i32) -> i32 {
        (value as i64 * self.dpi as i64 / 96) as i32
    }
}

struct Metrics {
    width: i32,
    padding: i32,
    row: i32,
    header: i32,
    separator: i32,
    radius: i32,
    icon: i32,
    gap: i32,
}

fn metrics(style: &Style) -> Metrics {
    Metrics {
        width: style.scale(212),
        padding: style.scale(6),
        row: style.scale(28),
        header: style.scale(40),
        separator: style.scale(7),
        radius: style.scale(8),
        icon: style.scale(15),
        gap: style.scale(9),
    }
}

fn item_height(item: &Item, m: &Metrics) -> i32 {
    match item {
        Item::Header { .. } => m.header,
        Item::Separator => m.separator,
        Item::Action { .. } | Item::Slider { .. } | Item::SliderPreset { .. } => m.row,
    }
}

/// Where everything ended up, so hit-testing can use the drawn geometry
/// rather than a second guess at it.
pub struct Layout {
    /// Vertical extent of each row.
    rows: Vec<(i32, i32)>,
    /// Horizontal extent of the slider bar, if there is one.
    slider_bar: Option<(i32, i32)>,
}

/// Draw the whole menu and report its layout.
fn render(
    canvas: &mut Canvas,
    items: &[Item],
    style: &mut Style,
    hovered: Option<usize>,
    slider_override: Option<f32>,
) -> Option<Layout> {
    let m = metrics(style);

    let height: i32 = items.iter().map(|i| item_height(i, &m)).sum::<i32>() + m.padding * 2;
    if !canvas.resize(m.width, height) {
        return None;
    }

    canvas.clear();
    canvas.fill_round_rect(
        RECT {
            left: 0,
            top: 0,
            right: m.width,
            bottom: height,
        },
        m.radius,
        // A touch more opaque than the widget: a menu should feel solid.
        style.theme.background.with_alpha(0xF7),
    );

    let mut rows = Vec::with_capacity(items.len());
    let mut slider_bar = None;
    let mut y = m.padding;

    for (index, item) in items.iter().enumerate() {
        let h = item_height(item, &m);
        rows.push((y, y + h));

        let inner_left = m.padding + style.scale(4);
        let inner_right = m.width - m.padding - style.scale(4);

        // Hover highlight, behind everything else in the row.
        if hovered == Some(index) && item.selectable() {
            canvas.fill_round_rect(
                RECT {
                    left: m.padding,
                    top: y + style.scale(1),
                    right: m.width - m.padding,
                    bottom: y + h - style.scale(1),
                },
                style.scale(4),
                Color::rgba(0xFF, 0xFF, 0xFF, 0x1E),
            );
        }

        match item {
            Item::Header {
                name,
                image,
                image_size,
            } => {
                let picture = style.scale(*image_size);
                let cx = inner_left as f32 + picture as f32 / 2.0;
                let cy = (y + h / 2) as f32;

                if let Some(image) = image {
                    match style.images.image(image, picture as u32) {
                        Some(bitmap) => {
                            let bitmap = bitmap.clone();
                            canvas.draw_circular_bitmap(&bitmap, cx, cy, picture as f32, 1.0);
                        }
                        None => canvas.fill_circle(
                            cx,
                            cy,
                            picture as f32 / 2.0,
                            style.theme.placeholder,
                        ),
                    }
                }

                let text_x = inner_left + picture + m.gap;
                canvas.draw_text_ellipsised(
                    name,
                    style.font,
                    text_x,
                    y + (h - style.font.height) / 2,
                    inner_right - text_x,
                    style.theme.text,
                );
            }

            Item::Separator => {
                let mid = y + h / 2;
                canvas.fill_rect(
                    RECT {
                        left: m.padding,
                        top: mid,
                        right: m.width - m.padding,
                        bottom: mid + style.scale(1).max(1),
                    },
                    style.theme.divider.with_alpha(0x66),
                );
            }

            Item::Action {
                label,
                icon,
                checked,
                danger,
                ..
            } => {
                let colour = if *danger {
                    style.theme.danger
                } else {
                    style.theme.text
                };

                let mut text_x = inner_left;
                if let Some(icon) = icon {
                    let iy = y + (h - m.icon) / 2;
                    style.icon_fonts.draw(
                        canvas,
                        *icon,
                        inner_left,
                        iy,
                        m.icon,
                        colour,
                        style.theme.background,
                    );
                    text_x += m.icon + m.gap;
                }

                canvas.draw_text_ellipsised(
                    label,
                    style.font,
                    text_x,
                    y + (h - style.font.height) / 2,
                    inner_right - text_x - style.scale(14),
                    colour,
                );

                if *checked {
                    // A dot rather than a tick: less visual noise at this size.
                    canvas.fill_circle(
                        (inner_right - style.scale(4)) as f32,
                        (y + h / 2) as f32,
                        style.scale(3) as f32,
                        style.theme.accent,
                    );
                }
            }

            Item::SliderPreset { label, value } => {
                canvas.draw_text(
                    label,
                    style.font,
                    inner_left,
                    y + (h - style.font.height) / 2,
                    style.theme.text,
                );

                let target = preset_text(items, *value);
                let target_width = canvas.measure_text(&target, style.font).0;
                canvas.draw_text(
                    &target,
                    style.font,
                    inner_right - target_width,
                    y + (h - style.font.height) / 2,
                    style.theme.text_dim,
                );
            }

            Item::Slider {
                label,
                value,
                range,
                format,
                warn_above,
            } => {
                // While dragging, show where the pointer is rather than the
                // value the source last confirmed.
                let value = slider_override.unwrap_or(*value);

                let label_width = canvas.measure_text(label, style.font).0;
                canvas.draw_text(
                    label,
                    style.font,
                    inner_left,
                    y + (h - style.font.height) / 2,
                    style.theme.text_dim,
                );

                let shown = format(value);
                let shown_width = canvas.measure_text(&shown, style.font).0;
                canvas.draw_text(
                    &shown,
                    style.font,
                    inner_right - shown_width,
                    y + (h - style.font.height) / 2,
                    style.theme.text_dim,
                );

                let bar_left = inner_left + label_width + m.gap;
                let bar_right = inner_right - shown_width - m.gap;
                let bar_height = style.scale(4);
                let bar_y = y + (h - bar_height) / 2;

                slider_bar = Some((bar_left, bar_right));

                if bar_right > bar_left {
                    canvas.fill_round_rect(
                        RECT {
                            left: bar_left,
                            top: bar_y,
                            right: bar_right,
                            bottom: bar_y + bar_height,
                        },
                        bar_height / 2,
                        style.theme.divider,
                    );

                    let (low, high) = *range;
                    let span = bar_right - bar_left;
                    let fraction =
                        ((value - low) / (high - low).max(f32::EPSILON)).clamp(0.0, 1.0);
                    let filled = (span as f32 * fraction).round() as i32;
                    if filled > 0 {
                        canvas.fill_round_rect(
                            RECT {
                                left: bar_left,
                                top: bar_y,
                                right: bar_left + filled.min(span),
                                bottom: bar_y + bar_height,
                            },
                            bar_height / 2,
                            match warn_above {
                                Some(limit) if value > *limit => style.theme.danger,
                                _ => style.theme.accent,
                            },
                        );
                    }
                }
            }
        }

        y += h;
    }

    Some(Layout { rows, slider_bar })
}

/// The header's picture and the size it is drawn at, if it has one.
fn header_image(items: &[Item]) -> Option<(ImageRef, i32)> {
    items.iter().find_map(|item| match item {
        Item::Header {
            image: Some(image),
            image_size,
            ..
        } => Some((image.clone(), *image_size)),
        _ => None,
    })
}

/// The slider's range, and the text one of its presets should show.
fn slider_range(items: &[Item]) -> (f32, f32) {
    items
        .iter()
        .find_map(|item| match item {
            Item::Slider { range, .. } => Some(*range),
            _ => None,
        })
        .unwrap_or((0.0, 1.0))
}

/// A preset shows its target in the slider's own units, so the two agree.
fn preset_text(items: &[Item], value: f32) -> String {
    items
        .iter()
        .find_map(|item| match item {
            Item::Slider { format, .. } => Some(format(value)),
            _ => None,
        })
        .unwrap_or_else(|| value.round().to_string())
}

/// Map a click to a value, using the bar's real extent.
///
/// The first version estimated where the bar was from the metrics, which did
/// not match where it had actually been drawn — the label and the readout are
/// measured text, so their widths are not knowable in advance. Clicking
/// therefore set a value some distance from the one under the pointer.
fn value_at(x: i32, bar: (i32, i32), range: (f32, f32)) -> f32 {
    let (left, right) = bar;
    let (low, high) = range;
    let span = (right - left).max(1);
    (low + ((x - left) as f32 / span as f32) * (high - low)).clamp(low, high)
}

fn register_class() -> bool {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return false;
        };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&class) != 0
            || windows::Win32::Foundation::GetLastError()
                == windows::Win32::Foundation::WIN32_ERROR(1410)
    }
}

/// What the menu ended with.
#[derive(Debug, Default, Clone, Copy)]
pub struct Outcome {
    /// A discrete command, if one was chosen.
    pub choice: Option<Choice>,
    /// The value the bar was left at, if it was touched. Already applied.
    pub slider: Option<f32>,
    /// Where the click that dismissed the menu landed, in screen coordinates.
    ///
    /// The menu swallows that click whole, so the caller is the only thing
    /// that can act on it — which is what lets clicking another participant
    /// switch panels rather than merely closing this one.
    pub dismissed_at: Option<POINT>,
}

/// Show the menu near `anchor` and block until something is chosen or it is
/// dismissed.
///
/// `above` places the menu's bottom edge at the anchor, which is what you want
/// over a bottom-docked taskbar.
pub fn show(
    items: &[Item],
    style: &mut Style,
    anchor: POINT,
    above: bool,
    bounds_limit: RECT,
) -> Outcome {
    if !register_class() {
        return Outcome::default();
    }

    // Kick the header fetch off before the first draw, so it has the whole
    // window-creation round trip to arrive in.
    if let Some((image, size)) = header_image(items) {
        let size = style.scale(size) as u32;
        style.images.image(&image, size);
    }

    let Some(mut canvas) = Canvas::new(1, 1) else {
        return Outcome::default();
    };
    let Some(layout) = render(&mut canvas, items, style, None, None) else {
        return Outcome::default();
    };
    let (width, height) = (canvas.width(), canvas.height());

    let hwnd = unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return Outcome::default();
        };
        let created = CreateWindowExW(
            // Activatable, unlike the widget and the readout. A menu that
            // cannot take focus cannot reliably learn that the user clicked
            // somewhere else, and mouse capture alone is not enough when the
            // owning thread is not in the foreground.
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("Taskbar widget menu"),
            WS_POPUP,
            0,
            0,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        );
        match created {
            Ok(hwnd) => hwnd,
            Err(_) => return Outcome::default(),
        }
    };

    let margin = style.scale(6);
    let x = (anchor.x - width / 2).clamp(
        bounds_limit.left + margin,
        (bounds_limit.right - width - margin).max(bounds_limit.left + margin),
    );
    let y = if above {
        anchor.y - height - margin
    } else {
        anchor.y + margin
    };

    canvas.present_layered_at(hwnd, x, y);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let _ = SetForegroundWindow(hwnd);
        SetCapture(hwnd);
        // The header avatar may still be downloading. Poll for it rather than
        // leaving a grey disc for the life of the menu.
        SetTimer(Some(hwnd), ASSET_TIMER, 120, None);
    }

    let outcome = run_loop(hwnd, &mut canvas, items, style, layout, POINT { x, y });

    unsafe {
        let _ = KillTimer(Some(hwnd), ASSET_TIMER);
        let _ = ReleaseCapture();
        let _ = DestroyWindow(hwnd);
    }

    outcome
}

/// The value the slider is showing and has applied.
///
/// `dragging` is the live pointer position while the button is held;
/// `settled` outlives it so the bar keeps showing where it was left rather
/// than reverting to the value the menu opened with.
#[derive(Default)]
struct SliderState {
    dragging: Option<f32>,
    settled: Option<f32>,
    /// Last whole value handed to `on_slide`. Mouse movement produces far
    /// more updates than any source needs, and each one is likely a request
    /// somewhere.
    last_sent: Option<i32>,
}

impl SliderState {
    /// What the bar should display, or `None` to use the item's own value.
    fn shown(&self) -> Option<f32> {
        self.dragging.or(self.settled)
    }

    /// Record a new value and apply it, unless it rounds to the last one.
    fn set(&mut self, style: &Style, value: f32, dragging: bool) {
        if dragging {
            self.dragging = Some(value);
        }
        self.settled = Some(value);

        let rounded = value.round() as i32;
        if self.last_sent == Some(rounded) {
            return;
        }
        self.last_sent = Some(rounded);

        if let Some(apply) = style.on_slide {
            apply(value);
        }
    }
}

/// The modal pump. Mouse input is captured, so everything arrives here.
fn run_loop(
    hwnd: HWND,
    canvas: &mut Canvas,
    items: &[Item],
    style: &mut Style,
    mut layout: Layout,
    origin: POINT,
) -> Outcome {
    let width = canvas.width();
    let height = canvas.height();

    let mut hovered: Option<usize> = None;
    let mut slider = SliderState::default();
    let mut dismissed_at: Option<POINT> = None;

    let slider_row = items
        .iter()
        .position(|item| matches!(item, Item::Slider { .. }));

    let choice = unsafe {
        loop {
            let mut message = MSG::default();
            if !GetMessageW(&mut message, None, 0, 0).as_bool() {
                // WM_QUIT: put it back for the main loop and give up.
                PostQuitMessage(0);
                break None;
            }

            // Everything below wants the pointer in menu-local coordinates.
            let local = cursor_local(origin);
            let inside = local.is_some_and(|p| {
                p.x >= 0 && p.x < width && p.y >= 0 && p.y < height
            });

            let row_at = |local: POINT| {
                layout
                    .rows
                    .iter()
                    .position(|(top, bottom)| local.y >= *top && local.y < *bottom)
            };

            let repaint = |canvas: &mut Canvas,
                               style: &mut Style,
                               hovered: Option<usize>,
                               slider: &SliderState,
                               layout: &mut Layout| {
                if let Some(next) = render(canvas, items, style, hovered, slider.shown()) {
                    *layout = next;
                    canvas.present_layered_at(hwnd, origin.x, origin.y);
                }
            };

            match message.message {
                WM_MOUSEMOVE => {
                    let Some(local) = local else { continue };

                    // A drag tracks the pointer even outside the row, which is
                    // what makes reaching either end of the scale possible.
                    if slider.dragging.is_some() {
                        if let Some(bar) = layout.slider_bar {
                            let value = value_at(local.x, bar, slider_range(items));
                            slider.set(style, value, true);
                            repaint(canvas, style, hovered, &slider, &mut layout);
                        }
                        continue;
                    }

                    let next = inside
                        .then(|| row_at(local))
                        .flatten()
                        .filter(|index| items[*index].selectable());

                    if next != hovered {
                        hovered = next;
                        repaint(canvas, style, hovered, &slider, &mut layout);
                    }
                }

                WM_RBUTTONDOWN if !inside => break None,

                WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                    // Deliberately no dismissal here. Closing on button-down
                    // would leave the matching button-up to fall through to
                    // whatever is underneath — which, when that is the avatar
                    // the menu was opened from, immediately reopened it. The
                    // menu keeps capture until the release so the click is
                    // consumed as one gesture.
                    let Some(local) = local else { continue };
                    if !inside {
                        continue;
                    }

                    // Grabbing the bar starts a drag and applies immediately,
                    // so the press itself is the first adjustment.
                    if message.message == WM_LBUTTONDOWN && row_at(local) == slider_row {
                        if let Some(bar) = layout.slider_bar {
                            let value = value_at(local.x, bar, slider_range(items));
                            slider.set(style, value, true);
                            repaint(canvas, style, hovered, &slider, &mut layout);
                        }
                    }
                }

                WM_LBUTTONUP => {
                    // Ending a drag leaves the menu open: the value is
                    // already applied, and dismissing here would make the
                    // slider feel like it had cancelled itself.
                    if let Some(value) = slider.dragging.take() {
                        slider.set(style, value, false);
                        repaint(canvas, style, hovered, &slider, &mut layout);
                        continue;
                    }

                    if !inside {
                        // Report where it landed; the caller decides whether
                        // that click meant anything.
                        dismissed_at = cursor_screen();
                        break None;
                    }
                    let Some(local) = local else { continue };
                    let Some(index) = row_at(local) else { continue };

                    match &items[index] {
                        // A command closes the menu.
                        Item::Action { id, .. } => break Some(Choice::Action(*id)),
                        // A preset is the slider by another route, so it
                        // behaves like the bar and stays open.
                        Item::SliderPreset { value, .. } => {
                            slider.set(style, *value, false);
                            repaint(canvas, style, hovered, &slider, &mut layout);
                        }
                        _ => continue,
                    }
                }

                WM_TIMER if message.wParam.0 == ASSET_TIMER => {
                    // Fall back to the capture, not the foreground.
                    //
                    // `SetForegroundWindow` is refused when the calling
                    // process did not receive the input that led here, and
                    // testing foreground would then dismiss the menu within a
                    // tick of opening it. Capture is what actually determines
                    // whether the menu can still see the mouse.
                    if GetCapture() != hwnd {
                        break None;
                    }
                    if style.images.collect() {
                        repaint(canvas, style, hovered, &slider, &mut layout);
                    }
                }

                WM_KEYDOWN if message.wParam.0 as u32 == VK_ESCAPE.0 as u32 => break None,

                // Losing capture or focus means something else took over;
                // treat either as a dismissal rather than stranding a menu.
                WM_CANCELMODE | WM_CAPTURECHANGED | WM_KILLFOCUS => break None,

                WM_ACTIVATE if message.wParam.0 as u32 == WA_INACTIVE => break None,

                _ => {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    };

    Outcome {
        choice,
        slider: slider.settled,
        dismissed_at,
    }
}

/// The pointer in screen coordinates.
fn cursor_screen() -> Option<POINT> {
    unsafe {
        let mut point = POINT::default();
        GetCursorPos(&mut point).ok()?;
        Some(point)
    }
}

/// The pointer relative to the menu's top-left corner.
fn cursor_local(origin: POINT) -> Option<POINT> {
    unsafe {
        let mut point = POINT::default();
        GetCursorPos(&mut point).ok()?;
        Some(POINT {
            x: point.x - origin.x,
            y: point.y - origin.y,
        })
    }
}

/// The menu does its own input handling in the modal loop; this only needs to
/// keep Windows happy and never erase, since every pixel comes from
/// `UpdateLayeredWindow`.
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
