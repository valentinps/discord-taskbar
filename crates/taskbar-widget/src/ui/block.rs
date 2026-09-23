//! What the widget draws, as data.
//!
//! This is the boundary between the widget and whatever it is showing. An
//! integration turns its own state into a list of `Block`s; the widget
//! measures, lays out, draws and hit-tests them without ever learning what
//! they mean. A block that carries an `id` is interactive, and that id comes
//! straight back to the integration when it is clicked, scrolled or hovered.
//!
//! Sizes here are at 96 DPI — the integration does not know which display it
//! is about to be drawn on, and there may be several with different DPI, so
//! scaling is the widget's job.

use windows::Win32::Foundation::{POINT, RECT};

use crate::assets::icons::{self, Glyph, IconFonts};
use crate::assets::images::{ImageCache, ImageRef};

use super::render::{Canvas, Color, Font};
use super::theme::Theme;

/// Identifies a block, so an interaction can be routed back to whatever put it
/// there.
///
/// Opaque to the widget: an integration can use whatever scheme it likes, and
/// the convention is a namespaced string such as `user:123` or `self:mute`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockId(pub String);

impl BlockId {
    pub fn new(id: impl Into<String>) -> Self {
        BlockId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The part after the first `:`, for ids of the form `kind:value`.
    pub fn suffix(&self, kind: &str) -> Option<&str> {
        self.0.strip_prefix(kind)?.strip_prefix(':')
    }
}

impl From<&str> for BlockId {
    fn from(id: &str) -> Self {
        BlockId(id.to_string())
    }
}

impl From<String> for BlockId {
    fn from(id: String) -> Self {
        BlockId(id)
    }
}

/// A ring drawn just inside an image's edge — someone talking, a live badge.
#[derive(Debug, Clone, Copy)]
pub struct Ring {
    pub color: Color,
    /// Stroke width at 96 DPI.
    pub thickness: i32,
}

impl Ring {
    pub fn new(color: Color) -> Self {
        Ring {
            color,
            thickness: 2,
        }
    }
}

/// A small marker over an image's bottom-right corner.
#[derive(Debug, Clone, Copy)]
pub struct Badge {
    pub glyph: Glyph,
    pub fill: Color,
}

/// A run of text in one colour.
#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub color: Color,
}

impl Span {
    pub fn new(text: impl Into<String>, color: Color) -> Self {
        Span {
            text: text.into(),
            color,
        }
    }
}

/// How square an image's corners are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Album art, thumbnails.
    Square,
    /// Corner radius at 96 DPI.
    Rounded(i32),
    /// Avatars, server icons.
    Circle,
}

#[derive(Debug, Clone)]
pub enum Content {
    /// A picture, or a plain disc of `placeholder` until one arrives.
    Image {
        image: Option<ImageRef>,
        /// Width and height at 96 DPI; images are always square.
        size: i32,
        shape: Shape,
        /// 0..1. Dimming is how "silenced on our end" reads, as distinct from
        /// the badge, which means the other end did it.
        opacity: f32,
        ring: Option<Ring>,
        badge: Option<Badge>,
        placeholder: Color,
    },

    /// One or more runs of text on a single line.
    ///
    /// The first span has priority when there is not enough room: it is
    /// ellipsised and the rest are dropped, rather than every span shrinking
    /// equally. For a channel name followed by a server name that is the
    /// difference between losing the server and losing where you are sitting.
    Text {
        spans: Vec<Span>,
        /// Widest it may get at 96 DPI before being ellipsised.
        max_width: Option<i32>,
    },

    /// One glyph from the icon font.
    Icon {
        glyph: Glyph,
        /// Box size at 96 DPI; the glyph is centred in it.
        size: i32,
        color: Color,
    },

    /// A thin vertical rule, for separating groups.
    Rule { color: Color },

    /// Several blocks packed tighter than the normal spacing.
    ///
    /// A positive `overlap` makes them overlap, each cutting a gap out of the
    /// one to its right so the stack stays legible; a negative one leaves a
    /// gap instead. Items are expected to share a size.
    Cluster { items: Vec<Block>, overlap: i32 },
}

/// One thing to draw.
#[derive(Debug, Clone)]
pub struct Block {
    /// Set to make the block interactive. A block with an id gets the hand
    /// cursor and reports clicks, wheel notches and hovers under that id.
    pub id: Option<BlockId>,
    pub content: Content,
}

impl Block {
    pub fn new(content: Content) -> Self {
        Block { id: None, content }
    }

    pub fn with_id(mut self, id: impl Into<BlockId>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// Everything drawing a block may read.
pub struct Context<'a> {
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

fn contains(bounds: RECT, point: POINT) -> bool {
    point.x >= bounds.left
        && point.x < bounds.right
        && point.y >= bounds.top
        && point.y < bounds.bottom
}

// ---------------------------------------------------------------------------
// Measuring

/// Width in device pixels, or 0 for a block that should be skipped entirely.
fn measure(block: &Block, canvas: &Canvas, ctx: &mut Context) -> i32 {
    match &block.content {
        Content::Image { size, .. } => ctx.scale(*size),

        Content::Text { spans, max_width } => {
            let width: i32 = spans
                .iter()
                .filter(|span| !span.text.is_empty())
                .map(|span| canvas.measure_text(&span.text, ctx.font).0)
                .sum();
            match max_width {
                Some(limit) => width.min(ctx.scale(*limit)),
                None => width,
            }
        }

        Content::Icon { size, .. } => ctx.scale(*size),

        Content::Rule { .. } => ctx.scale(1).max(1),

        Content::Cluster { items, overlap } => {
            let widths: Vec<i32> = items.iter().map(|item| measure(item, canvas, ctx)).collect();
            let visible: Vec<i32> = widths.into_iter().filter(|w| *w > 0).collect();
            let Some(last) = visible.last() else {
                return 0;
            };
            // Each item but the last advances by its own width less the
            // overlap, which is exactly how `place_cluster` walks them.
            let step: i32 = visible
                .iter()
                .take(visible.len() - 1)
                .map(|w| w - ctx.scale(*overlap))
                .sum();
            step + last
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing

fn draw(block: &Block, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
    match &block.content {
        Content::Image { .. } => draw_image(block, canvas, ctx, bounds),

        Content::Text { spans, .. } => {
            let available = bounds.right - bounds.left;
            let y = bounds.top + (bounds.bottom - bounds.top - ctx.font.height) / 2;
            let mut x = bounds.left;

            for (index, span) in spans.iter().enumerate() {
                if span.text.is_empty() {
                    continue;
                }
                let remaining = bounds.left + available - x;
                if remaining <= 0 {
                    return;
                }

                let width = canvas.measure_text(&span.text, ctx.font).0;

                // The first span takes the whole width if it needs it;
                // everything after it lives on what is left.
                if index == 0 && width >= available {
                    canvas.draw_text_ellipsised(
                        &span.text,
                        ctx.font,
                        x,
                        y,
                        available,
                        span.color,
                    );
                    return;
                }

                // Below this there is no room for anything but an ellipsis,
                // and an ellipsis on its own says nothing.
                if index > 0 && remaining < ctx.scale(24) {
                    return;
                }

                if width <= remaining {
                    canvas.draw_text(&span.text, ctx.font, x, y, span.color);
                    x += width;
                } else {
                    canvas.draw_text_ellipsised(
                        &span.text,
                        ctx.font,
                        x,
                        y,
                        remaining,
                        span.color,
                    );
                    return;
                }
            }
        }

        Content::Icon { glyph, size, color } => {
            let size = ctx.scale(*size);
            let y = bounds.top + (bounds.bottom - bounds.top - size) / 2;
            let backdrop = ctx.theme.background;
            ctx.icon_fonts
                .draw(canvas, *glyph, bounds.left, y, size, *color, backdrop);
        }

        Content::Rule { color } => {
            // Short of full height, so it reads as a separator rather than a
            // wall.
            let inset = (bounds.bottom - bounds.top) / 4;
            canvas.fill_rect(
                RECT {
                    left: bounds.left,
                    top: bounds.top + inset,
                    right: bounds.right,
                    bottom: bounds.bottom - inset,
                },
                *color,
            );
        }

        Content::Cluster { items, overlap } => {
            let placed = place_cluster(items, canvas, ctx, bounds, *overlap);
            let cut = *overlap > 0;

            // Right to left, so earlier items overlap later ones — which puts
            // the ones the integration listed first on top.
            for (index, (item, rect)) in placed.iter().enumerate().rev() {
                // Cut a slightly larger hole at our own position first,
                // carving a clean gap out of the neighbour already drawn to
                // our right.
                if cut && index + 1 < placed.len() {
                    cut_backdrop(item, canvas, ctx, *rect);
                }
                draw(item, canvas, ctx, *rect);
            }
        }
    }
}

/// Erase the shape a block is about to occupy, so an overlapping neighbour
/// does not run into it.
fn cut_backdrop(block: &Block, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
    let Content::Image { size, shape, .. } = &block.content else {
        return;
    };
    let size = ctx.scale(*size) as f32;
    let slack = ctx.scale(2).max(1) as f32;
    let centre_x = bounds.left as f32 + size / 2.0;
    let centre_y = (bounds.top + bounds.bottom) as f32 / 2.0;
    let backdrop = ctx.backdrop;

    match shape {
        Shape::Circle => canvas.fill_circle(centre_x, centre_y, size / 2.0 + slack, backdrop),
        Shape::Square | Shape::Rounded(_) => {
            let radius = match shape {
                Shape::Rounded(r) => ctx.scale(*r),
                _ => 0,
            };
            canvas.fill_round_rect(
                RECT {
                    left: (centre_x - size / 2.0 - slack) as i32,
                    top: (centre_y - size / 2.0 - slack) as i32,
                    right: (centre_x + size / 2.0 + slack) as i32,
                    bottom: (centre_y + size / 2.0 + slack) as i32,
                },
                radius,
                backdrop,
            );
        }
    }
}

fn draw_image(block: &Block, canvas: &mut Canvas, ctx: &mut Context, bounds: RECT) {
    let Content::Image {
        image,
        size,
        shape,
        opacity,
        ring,
        badge,
        placeholder,
    } = &block.content
    else {
        return;
    };

    let size = ctx.scale(*size);
    let radius = size as f32 / 2.0;
    let centre_x = bounds.left as f32 + radius;
    let centre_y = (bounds.top + bounds.bottom) as f32 / 2.0;
    let left = centre_x - radius;
    let top = centre_y - radius;

    let corner = match shape {
        Shape::Circle => radius,
        Shape::Rounded(r) => ctx.scale(*r) as f32,
        Shape::Square => 0.0,
    };

    let bitmap = image
        .as_ref()
        .and_then(|image| ctx.images.image(image, size as u32))
        .cloned();

    match bitmap {
        Some(bitmap) => canvas.draw_bitmap_shaped(
            &bitmap,
            left,
            top,
            size as f32,
            size as f32,
            corner,
            *opacity,
        ),
        // Nothing has arrived yet; hold the space rather than reflowing the
        // widget when it does.
        None => match shape {
            Shape::Circle => canvas.fill_circle(centre_x, centre_y, radius, *placeholder),
            _ => canvas.fill_round_rect(
                RECT {
                    left: left as i32,
                    top: top as i32,
                    right: (left + size as f32) as i32,
                    bottom: (top + size as f32) as i32,
                },
                corner as i32,
                *placeholder,
            ),
        },
    }

    if let Some(ring) = ring {
        let thickness = ctx.scale(ring.thickness).max(1) as f32;
        // Inset by half the stroke so the ring sits inside the image's edge
        // rather than straddling it.
        canvas.stroke_circle(
            centre_x,
            centre_y,
            radius - thickness / 2.0,
            thickness,
            ring.color,
        );
    }

    if let Some(badge) = badge {
        let badge_radius = radius * 0.46;
        let badge_x = centre_x + radius - badge_radius * 0.6;
        let badge_y = centre_y + radius - badge_radius * 0.6;
        icons::badge(
            canvas,
            ctx.icon_fonts,
            badge_x,
            badge_y,
            badge_radius,
            badge.glyph,
            badge.fill,
            ctx.backdrop,
        );
    }
}

// ---------------------------------------------------------------------------
// Layout and hit-testing

/// Where each block ended up, so an interaction can be routed back to it.
///
/// Hit-testing works off this rather than measuring a second time: the drawn
/// geometry is the only geometry that can be right, and a re-measure would be
/// a second guess at it. It also means a click costs no text measurement and
/// needs no canvas.
pub struct Layout {
    pub width: i32,
    /// Top-level bounds, one per block, empty for the ones that were skipped.
    pub bounds: Vec<RECT>,
    /// Interactive regions, in the order a hit should be resolved.
    spots: Vec<(BlockId, RECT)>,
    /// Fallback regions covering the gaps inside a cluster, each given to the
    /// item whose centre is nearest.
    gaps: Vec<(BlockId, RECT)>,
}

impl Layout {
    /// Which block is under `point`, in widget client coordinates.
    ///
    /// Only blocks carrying an id are reported: an id is what makes a block
    /// interactive, so anything without one is scenery.
    pub fn hit(&self, point: POINT) -> Option<BlockId> {
        for (id, rect) in &self.spots {
            if contains(*rect, point) {
                return Some(id.clone());
            }
        }
        // Inside a cluster but between its items: give it to the nearest one,
        // so a deliberate click never lands on nothing.
        for (id, rect) in &self.gaps {
            if contains(*rect, point) {
                return Some(id.clone());
            }
        }
        None
    }

    /// Whether `point` is over something worth a hand cursor.
    pub fn is_interactive(&self, point: POINT) -> bool {
        self.hit(point).is_some()
    }
}

/// Place a cluster's items and return each with its own bounds.
fn place_cluster<'a>(
    items: &'a [Block],
    canvas: &Canvas,
    ctx: &mut Context,
    bounds: RECT,
    overlap: i32,
) -> Vec<(&'a Block, RECT)> {
    let gap = ctx.scale(overlap);
    let mut placed = Vec::with_capacity(items.len());
    let mut x = bounds.left;

    for item in items {
        let width = measure(item, canvas, ctx);
        if width <= 0 {
            continue;
        }
        placed.push((
            item,
            RECT {
                left: x,
                top: bounds.top,
                right: x + width,
                bottom: bounds.bottom,
            },
        ));
        x += width - gap;
    }

    placed
}

/// Record the interactive regions of one placed block.
fn collect_spots(
    block: &Block,
    canvas: &Canvas,
    ctx: &mut Context,
    bounds: RECT,
    spots: &mut Vec<(BlockId, RECT)>,
    gaps: &mut Vec<(BlockId, RECT)>,
) {
    if let Content::Cluster { items, overlap } = &block.content {
        let placed = place_cluster(items, canvas, ctx, bounds, *overlap);

        // Items are drawn right to left, so where two overlap it is the
        // earlier one that ends up on top. Listing them forwards means the
        // first match is the one the eye sees.
        for (item, rect) in &placed {
            collect_spots(item, canvas, ctx, *rect, spots, gaps);
        }

        // Partition the cluster's whole width between its items at the
        // midpoints of their centres — nearest-centre, as a plain rectangle
        // test.
        let centres: Vec<i32> = placed
            .iter()
            .map(|(_, rect)| (rect.left + rect.right) / 2)
            .collect();

        for (index, (item, _)) in placed.iter().enumerate() {
            let Some(id) = fallback_id(item) else { continue };
            let left = match index {
                0 => bounds.left,
                _ => (centres[index - 1] + centres[index]) / 2,
            };
            let right = if index + 1 == centres.len() {
                bounds.right
            } else {
                (centres[index] + centres[index + 1]) / 2
            };
            gaps.push((
                id,
                RECT {
                    left,
                    top: bounds.top,
                    right,
                    bottom: bounds.bottom,
                },
            ));
        }
        return;
    }

    if let Some(id) = block.id.clone() {
        spots.push((id, bounds));
    }
}

/// The id a cluster item answers to, for the nearest-item fallback.
fn fallback_id(block: &Block) -> Option<BlockId> {
    if let Content::Cluster { items, .. } = &block.content {
        // A nested cluster falls back to its first identifiable item.
        return items.iter().find_map(fallback_id).or_else(|| block.id.clone());
    }
    block.id.clone()
}

/// Measure every block, then place them left to right with `spacing` between
/// those that asked for room.
pub fn layout(
    canvas: &mut Canvas,
    ctx: &mut Context,
    blocks: &[Block],
    height: i32,
    draw_them: bool,
) -> Layout {
    let padding = ctx.scale(ctx.theme.padding);
    let spacing = ctx.scale(ctx.theme.spacing);

    let widths: Vec<i32> = blocks.iter().map(|b| measure(b, canvas, ctx)).collect();
    let visible: Vec<usize> = (0..blocks.len()).filter(|i| widths[*i] > 0).collect();

    let mut bounds = vec![RECT::default(); blocks.len()];
    let mut spots = Vec::new();
    let mut gaps = Vec::new();

    if visible.is_empty() {
        return Layout {
            width: 0,
            bounds,
            spots,
            gaps,
        };
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

        if draw_them {
            draw(&blocks[*index], canvas, ctx, rect);
        }
        collect_spots(&blocks[*index], canvas, ctx, rect, &mut spots, &mut gaps);

        x += widths[*index];
    }

    Layout {
        width: content + padding * 2,
        bounds,
        spots,
        gaps,
    }
}
