//! The widget's contents, as a row of independent elements.
//!
//! This is the extension point. Adding a feature — a ping readout, a
//! screenshare marker, an unread badge — means writing one `Element` and
//! putting it in the list. Layout, measurement, hit-testing and positioning
//! already work for anything that implements the trait.

use windows::Win32::Foundation::{POINT, RECT};

use crate::assets::icons::{self, IconFonts};
use crate::integration::discord::icons as glyphs;
use crate::assets::images::{ImageCache, ImageRef, OVERSAMPLE};
use crate::model::VoiceStatus;

use super::render::{Canvas, Color, Font};
use super::theme::Theme;

/// Something a click on the widget should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    ToggleMute,
    ToggleDeafen,
    FocusDiscord,
    LeaveVoice,
    /// Open the per-participant menu.
    UserMenu(String),
}

/// Everything an element may read while measuring or drawing.
pub struct Context<'a> {
    pub status: &'a VoiceStatus,
    pub theme: &'a Theme,
    pub font: &'a Font,
    pub icon_fonts: &'a mut IconFonts,
    pub images: &'a mut ImageCache,
    /// The taskbar colour behind the widget, for cut-out effects.
    pub backdrop: Color,
    pub dpi: u32,
}

impl Context<'_> {
    /// Scale a 96-DPI design value to device pixels.
    pub fn scale(&self, value: i32) -> i32 {
        (value as i64 * self.dpi as i64 / 96) as i32
    }
}

pub trait Element {
    /// Width in device pixels, or 0 to be skipped entirely.
    fn measure(&self, canvas: &Canvas, ctx: &mut Context) -> i32;

    /// Draw within `bounds`, whose width is what `measure` returned.
    fn draw(&self, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT);

    /// What clicking at `point` (widget client coordinates) should do.
    fn hit_test(&self, _ctx: &Context, _bounds: RECT, _point: POINT) -> Option<Action> {
        None
    }

    /// Which participant is under `point`, for the wheel and middle button.
    fn user_at(&self, _ctx: &Context, _bounds: RECT, _point: POINT) -> Option<String> {
        None
    }
}

fn contains(bounds: RECT, point: POINT) -> bool {
    point.x >= bounds.left
        && point.x < bounds.right
        && point.y >= bounds.top
        && point.y < bounds.bottom
}

// ---------------------------------------------------------------------------

/// The server's icon, as a circle.
///
/// People recognise servers by icon far more than by name, and an icon costs a
/// fraction of the width a name does — which matters on a taskbar.
pub struct GuildIcon;

impl GuildIcon {
    fn shown(ctx: &Context) -> bool {
        ctx.theme.show_guild_icon && ctx.status.guild_icon_url.is_some()
    }

    fn size(ctx: &Context) -> i32 {
        ctx.scale(ctx.theme.guild_icon_size)
    }
}

impl Element for GuildIcon {
    fn measure(&self, _canvas: &Canvas, ctx: &mut Context) -> i32 {
        if !Self::shown(ctx) {
            return 0;
        }
        Self::size(ctx)
    }

    fn draw(&self, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
        if !Self::shown(ctx) {
            return;
        }
        let (Some(url), Some(id)) = (
            ctx.status.guild_icon_url.clone(),
            ctx.status.guild_id.clone(),
        ) else {
            return;
        };

        let size = Self::size(ctx);
        let centre_x = bounds.left as f32 + size as f32 / 2.0;
        let centre_y = (bounds.top + bounds.bottom) as f32 / 2.0;

        let separator = if url.contains('?') { '&' } else { '?' };
        let image = ImageRef::new(
            format!("guild_{id}"),
            format!(
                "{url}{separator}size={}",
                (size as u32 * OVERSAMPLE).next_power_of_two()
            ),
        );

        match ctx.images.image(&image, size as u32) {
            Some(bitmap) => {
                let bitmap = bitmap.clone();
                canvas.draw_circular_bitmap(&bitmap, centre_x, centre_y, size as f32, 1.0);
            }
            None => {
                canvas.fill_circle(
                    centre_x,
                    centre_y,
                    size as f32 / 2.0,
                    Color::rgb(0x4E, 0x50, 0x58),
                );
            }
        }
    }

    fn hit_test(&self, _ctx: &Context, bounds: RECT, point: POINT) -> Option<Action> {
        contains(bounds, point).then_some(Action::FocusDiscord)
    }
}

/// A chevron between the server icon and the channel name.
pub struct Separator;

impl Element for Separator {
    fn measure(&self, canvas: &Canvas, ctx: &mut Context) -> i32 {
        if !GuildIcon::shown(ctx) || ctx.status.channel_name.is_none() {
            return 0;
        }
        canvas.measure_text("›", ctx.font).0
    }

    fn draw(&self, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
        let y = bounds.top + (bounds.bottom - bounds.top - ctx.font.height) / 2;
        canvas.draw_text("›", ctx.font, bounds.left, y, ctx.theme.text_dim);
    }
}

/// Channel name, optionally followed by the server name in a dimmer colour.
///
/// Channel first deliberately: when the label has to be ellipsised it is the
/// server name that should lose characters, not the channel you are sitting in.
pub struct ChannelLabel;

const GUILD_SEPARATOR: &str = "  ·  ";

impl ChannelLabel {
    fn parts(ctx: &Context) -> (String, Option<String>) {
        let channel = ctx.status.channel_name.clone().unwrap_or_default();
        let guild = if ctx.theme.show_guild_name {
            ctx.status.guild_name.clone()
        } else {
            None
        };
        (channel, guild)
    }
}

impl Element for ChannelLabel {
    fn measure(&self, canvas: &Canvas, ctx: &mut Context) -> i32 {
        let (channel, guild) = Self::parts(ctx);
        if channel.is_empty() {
            return 0;
        }

        let mut width = canvas.measure_text(&channel, ctx.font).0;
        if let Some(guild) = &guild {
            width += canvas
                .measure_text(&format!("{GUILD_SEPARATOR}{guild}"), ctx.font)
                .0;
        }
        width.min(ctx.scale(ctx.theme.max_label_width))
    }

    fn draw(&self, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
        let (channel, guild) = Self::parts(ctx);
        if channel.is_empty() {
            return;
        }

        let available = bounds.right - bounds.left;
        let y = bounds.top + (bounds.bottom - bounds.top - ctx.font.height) / 2;
        let channel_width = canvas.measure_text(&channel, ctx.font).0;

        // No room for both: give everything to the channel.
        if channel_width >= available {
            canvas.draw_text_ellipsised(
                &channel,
                ctx.font,
                bounds.left,
                y,
                available,
                ctx.theme.text,
            );
            return;
        }

        canvas.draw_text(&channel, ctx.font, bounds.left, y, ctx.theme.text);

        let Some(guild) = guild else { return };
        let remaining = available - channel_width;
        // Below this there is no room for anything but an ellipsis.
        if remaining < ctx.scale(24) {
            return;
        }

        canvas.draw_text_ellipsised(
            &format!("{GUILD_SEPARATOR}{guild}"),
            ctx.font,
            bounds.left + channel_width,
            y,
            remaining,
            ctx.theme.text_dim,
        );
    }

    fn hit_test(&self, _ctx: &Context, bounds: RECT, point: POINT) -> Option<Action> {
        contains(bounds, point).then_some(Action::FocusDiscord)
    }
}

// ---------------------------------------------------------------------------

/// Circular avatars, ringed while their owner is talking and badged when they
/// are muted or deafened.
pub struct AvatarRow;

impl AvatarRow {
    fn visible_count(ctx: &Context) -> usize {
        ctx.status.participants.len().min(ctx.theme.max_avatars)
    }

    /// Distance between adjacent avatar centres. A negative `avatar_overlap`
    /// turns into a gap instead.
    fn step(ctx: &Context) -> i32 {
        ctx.scale(ctx.theme.avatar_size - ctx.theme.avatar_overlap)
    }

    fn overlapping(ctx: &Context) -> bool {
        ctx.theme.avatar_overlap > 0
    }
}

impl Element for AvatarRow {
    fn measure(&self, _canvas: &Canvas, ctx: &mut Context) -> i32 {
        let count = Self::visible_count(ctx) as i32;
        if count == 0 {
            return 0;
        }
        (count - 1) * Self::step(ctx) + ctx.scale(ctx.theme.avatar_size)
    }

    fn draw(&self, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
        let count = Self::visible_count(ctx);
        if count == 0 {
            return;
        }

        let size = ctx.scale(ctx.theme.avatar_size);
        let step = Self::step(ctx);
        let radius = size as f32 / 2.0;
        let centre_y = (bounds.top + bounds.bottom) as f32 / 2.0;

        // Snapshot what we need: `ctx` is borrowed mutably for the caches.
        let theme_speaking = ctx.theme.speaking;
        let theme_danger = ctx.theme.danger;
        let backdrop = ctx.backdrop;
        let overlapping = Self::overlapping(ctx);
        let ring_thickness = (ctx.scale(2)).max(1) as f32;

        let people: Vec<_> = ctx
            .status
            .sorted_participants(ctx.theme.sort_by_speaking)
            .into_iter()
            .take(count)
            .cloned()
            .collect();

        // Right to left, so earlier avatars overlap later ones the way Discord
        // stacks them.
        for (index, participant) in people.iter().enumerate().rev() {
            let centre_x = bounds.left as f32 + radius + (index as i32 * step) as f32;

            // Cut a slightly larger disc at our own position first, carving a
            // clean gap out of the neighbour already drawn to our right. Only
            // needed when the avatars actually overlap.
            if overlapping && index + 1 < people.len() {
                canvas.fill_circle(centre_x, centre_y, radius + ring_thickness, backdrop);
            }

            // Someone silenced on our end is dimmed, which distinguishes it
            // from the red badge that means *they* muted themselves.
            let opacity = if participant.is_locally_silenced() {
                0.35
            } else {
                1.0
            };

            match ctx
                .images
                .image(&participant.avatar_ref(size as u32), size as u32)
            {
                Some(bitmap) => {
                    let bitmap = bitmap.clone();
                    canvas.draw_circular_bitmap(&bitmap, centre_x, centre_y, size as f32, opacity);
                }
                None => {
                    // Placeholder until the download lands.
                    canvas.fill_circle(centre_x, centre_y, radius, Color::rgb(0x4E, 0x50, 0x58));
                }
            }

            if participant.speaking {
                canvas.stroke_circle(
                    centre_x,
                    centre_y,
                    radius - ring_thickness / 2.0,
                    ring_thickness,
                    theme_speaking,
                );
            }

            if participant.is_deafened() || participant.is_muted() {
                let badge_radius = radius * 0.46;
                let badge_x = centre_x + radius - badge_radius * 0.6;
                let badge_y = centre_y + radius - badge_radius * 0.6;
                icons::badge(
                    canvas,
                    ctx.icon_fonts,
                    badge_x,
                    badge_y,
                    badge_radius,
                    if participant.is_deafened() {
                        glyphs::HEADPHONES
                    } else {
                        glyphs::MICROPHONE
                    },
                    theme_danger,
                    backdrop,
                );
            }
        }
    }

    fn hit_test(&self, ctx: &Context, bounds: RECT, point: POINT) -> Option<Action> {
        self.user_at(ctx, bounds, point).map(Action::UserMenu)
    }

    fn user_at(&self, ctx: &Context, bounds: RECT, point: POINT) -> Option<String> {
        if !contains(bounds, point) {
            return None;
        }

        let count = Self::visible_count(ctx);
        if count == 0 {
            return None;
        }

        let size = ctx.scale(ctx.theme.avatar_size);
        let step = Self::step(ctx).max(1);

        // Avatars are drawn right to left, so the one on top at any overlap is
        // the earlier index. Walking forwards and taking the first hit matches
        // what the eye sees.
        let offset = point.x - bounds.left;
        for index in 0..count {
            let start = index as i32 * step;
            if offset >= start && offset < start + size {
                return ctx
                    .status
                    .sorted_participants(ctx.theme.sort_by_speaking)
                    .get(index)
                    .map(|p| p.user_id.clone());
            }
        }

        // Past the last centre but still inside: the trailing avatar.
        ctx.status
            .sorted_participants(ctx.theme.sort_by_speaking)
            .get(count - 1)
            .map(|p| p.user_id.clone())
    }
}

// ---------------------------------------------------------------------------

/// A thin rule separating the participants from your own controls.
pub struct Divider;

impl Element for Divider {
    fn measure(&self, _canvas: &Canvas, ctx: &mut Context) -> i32 {
        if !ctx.theme.show_divider || ctx.status.participants.is_empty() {
            return 0;
        }
        ctx.scale(1).max(1)
    }

    fn draw(&self, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
        // Short of full height, so it reads as a separator rather than a wall.
        let inset = (bounds.bottom - bounds.top) / 4;
        canvas.fill_rect(
            RECT {
                left: bounds.left,
                top: bounds.top + inset,
                right: bounds.right,
                bottom: bounds.bottom - inset,
            },
            ctx.theme.divider,
        );
    }
}

/// Hang-up button: leaves the voice channel.
pub struct LeaveButton;

impl LeaveButton {
    fn size(ctx: &Context) -> i32 {
        ctx.scale(ctx.theme.icon_size)
    }
}

impl Element for LeaveButton {
    fn measure(&self, _canvas: &Canvas, ctx: &mut Context) -> i32 {
        if !ctx.theme.show_leave_button || !ctx.status.is_connected() {
            return 0;
        }
        Self::size(ctx)
    }

    fn draw(&self, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
        if !ctx.theme.show_leave_button || !ctx.status.is_connected() {
            return;
        }
        let size = Self::size(ctx);
        let y = bounds.top + (bounds.bottom - bounds.top - size) / 2;
        let colour = ctx.theme.danger;
        let backdrop = ctx.theme.background;
        ctx.icon_fonts
            .draw(canvas, glyphs::HANG_UP, bounds.left, y, size, colour, backdrop);
    }

    fn hit_test(&self, ctx: &Context, bounds: RECT, point: POINT) -> Option<Action> {
        if !ctx.theme.show_leave_button || !ctx.status.is_connected() {
            return None;
        }
        contains(bounds, point).then_some(Action::LeaveVoice)
    }
}

// ---------------------------------------------------------------------------

/// Your own microphone and headphone state, optionally clickable to toggle.
pub struct SelfStatusIcons;

impl SelfStatusIcons {
    fn metrics(ctx: &Context) -> (i32, i32) {
        (ctx.scale(ctx.theme.icon_size), ctx.scale(6))
    }
}

impl Element for SelfStatusIcons {
    fn measure(&self, _canvas: &Canvas, ctx: &mut Context) -> i32 {
        if !ctx.theme.show_self_icons {
            return 0;
        }
        let (size, gap) = Self::metrics(ctx);
        size * 2 + gap
    }

    fn draw(&self, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
        if !ctx.theme.show_self_icons {
            return;
        }

        let (size, gap) = Self::metrics(ctx);
        let y = bounds.top + (bounds.bottom - bounds.top - size) / 2;

        let state = ctx.status.self_state;
        let muted = state.is_muted();
        let backdrop = ctx.theme.background;

        let mic_colour = if muted {
            ctx.theme.danger
        } else {
            ctx.theme.text_dim
        };
        let ear_colour = if state.deaf {
            ctx.theme.danger
        } else {
            ctx.theme.text_dim
        };

        let mic = if muted {
            glyphs::MICROPHONE_OFF
        } else {
            glyphs::MICROPHONE
        };
        let ear = if state.deaf {
            glyphs::HEADPHONES_OFF
        } else {
            glyphs::HEADPHONES
        };

        ctx.icon_fonts
            .draw(canvas, mic, bounds.left, y, size, mic_colour, backdrop);
        ctx.icon_fonts.draw(
            canvas,
            ear,
            bounds.left + size + gap,
            y,
            size,
            ear_colour,
            backdrop,
        );
    }

    fn hit_test(&self, ctx: &Context, bounds: RECT, point: POINT) -> Option<Action> {
        if !ctx.theme.show_self_icons || !ctx.theme.clickable_self_icons {
            return None;
        }
        if !contains(bounds, point) {
            return None;
        }

        let (size, gap) = Self::metrics(ctx);
        // Split the gap between the two halves so there is no dead strip.
        let boundary = bounds.left + size + gap / 2;
        Some(if point.x < boundary {
            Action::ToggleMute
        } else {
            Action::ToggleDeafen
        })
    }
}

// ---------------------------------------------------------------------------

/// Where each element ended up, so clicks can be routed back to it.
pub struct Layout {
    pub width: i32,
    pub bounds: Vec<RECT>,
}

/// Measure every element, then place them left to right with `spacing` between
/// those that asked for room.
pub fn layout(
    canvas: &mut Canvas,
    ctx: &mut Context,
    elements: &[Box<dyn Element>],
    height: i32,
    draw: bool,
) -> Layout {
    let padding = ctx.scale(ctx.theme.padding);
    let spacing = ctx.scale(ctx.theme.spacing);

    let widths: Vec<i32> = elements.iter().map(|e| e.measure(canvas, ctx)).collect();
    let visible: Vec<usize> = (0..elements.len()).filter(|i| widths[*i] > 0).collect();

    let mut bounds = vec![RECT::default(); elements.len()];

    if visible.is_empty() {
        return Layout { width: 0, bounds };
    }

    let content: i32 =
        visible.iter().map(|i| widths[*i]).sum::<i32>() + spacing * (visible.len() as i32 - 1);

    let mut x = padding;
    for (position, index) in visible.iter().enumerate() {
        if position > 0 {
            x += spacing;
        }
        let rect = RECT {
            left: x,
            top: 0,
            right: x + widths[*index],
            bottom: height,
        };
        bounds[*index] = rect;

        if draw {
            elements[*index].draw(canvas, ctx, rect);
        }
        x += widths[*index];
    }

    Layout {
        width: content + padding * 2,
        bounds,
    }
}

/// Route a click to whichever element owns that point.
pub fn hit_test(
    ctx: &Context,
    elements: &[Box<dyn Element>],
    bounds: &[RECT],
    point: POINT,
) -> Option<Action> {
    for (element, rect) in elements.iter().zip(bounds.iter()) {
        if rect.right <= rect.left {
            continue;
        }
        if let Some(action) = element.hit_test(ctx, *rect, point) {
            return Some(action);
        }
    }
    None
}
