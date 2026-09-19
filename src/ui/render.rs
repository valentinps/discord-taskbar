//! Software compositing onto a 32-bit DIB, presented with `UpdateLayeredWindow`.
//!
//! Two things drive this design:
//!
//! * The widget sits on the taskbar, so it needs real per-pixel alpha —
//!   anti-aliased avatar circles and speaking rings look wrong against a
//!   colour-keyed background.
//! * GDI does not write the alpha channel. Text is therefore rasterised into a
//!   scratch DIB as a white-on-black coverage mask and composited by hand,
//!   which also gives us correctly anti-aliased edges. The font is created with
//!   `ANTIALIASED_QUALITY` rather than ClearType, because sub-pixel
//!   anti-aliasing produces colour fringes over a transparent surface.
//!
//! Pixels are stored premultiplied BGRA, which is what `UpdateLayeredWindow`
//! expects with `AC_SRC_ALPHA`.

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r, g, b, a }
    }

    /// Parse `#RRGGBB` or `#RRGGBBAA`. Returns `None` on anything else so the
    /// caller can fall back to a default rather than panic on a typo.
    pub fn from_hex(text: &str) -> Option<Self> {
        let hex = text.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            6 => Some(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    /// Build from a GDI `COLORREF` (0x00BBGGRR), fully opaque.
    pub fn from_colorref(value: u32) -> Self {
        Color::rgb(
            (value & 0xFF) as u8,
            ((value >> 8) & 0xFF) as u8,
            ((value >> 16) & 0xFF) as u8,
        )
    }

    pub fn with_alpha(self, a: u8) -> Self {
        Color { a, ..self }
    }

    /// Premultiplied 0xAARRGGBB, matching the DIB's memory layout.
    fn premultiplied(self) -> u32 {
        let a = self.a as u32;
        let scale = |c: u8| ((c as u32 * a + 127) / 255) & 0xFF;
        (a << 24) | (scale(self.r) << 16) | (scale(self.g) << 8) | scale(self.b)
    }
}

/// Source-over composite of a premultiplied source onto a premultiplied dest.
#[inline]
fn src_over(dst: u32, src: u32) -> u32 {
    let sa = (src >> 24) & 0xFF;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }

    let inv = 255 - sa;
    let blend = |shift: u32| {
        let s = (src >> shift) & 0xFF;
        let d = (dst >> shift) & 0xFF;
        (s + (d * inv + 127) / 255).min(255)
    };

    (blend(24) << 24) | (blend(16) << 16) | (blend(8) << 8) | blend(0)
}

/// A decoded image in premultiplied BGRA, ready to composite.
#[derive(Clone)]
pub struct Bitmap {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
}

impl Bitmap {
    pub fn new(width: i32, height: i32) -> Self {
        Bitmap {
            width,
            height,
            pixels: vec![0; (width.max(0) * height.max(0)) as usize],
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        self.pixels[(y * self.width + x) as usize]
    }
}

/// A GDI font handle plus its metrics.
pub struct Font {
    handle: HFONT,
    pub height: i32,
    pub ascent: i32,
}

impl Font {
    pub fn new(family: &str, height_px: i32, bold: bool) -> Option<Self> {
        unsafe {
            let name: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();

            let handle = CreateFontW(
                -height_px,
                0,
                0,
                0,
                if bold { FW_SEMIBOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                // Grayscale AA: ClearType's sub-pixel output would fringe on a
                // transparent surface.
                ANTIALIASED_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                PCWSTR(name.as_ptr()),
            );

            if handle.is_invalid() {
                return None;
            }

            let dc = CreateCompatibleDC(None);
            let previous = SelectObject(dc, handle.into());
            let mut metrics = TEXTMETRICW::default();
            let _ = GetTextMetricsW(dc, &mut metrics);
            SelectObject(dc, previous);
            let _ = DeleteDC(dc);

            Some(Font {
                handle,
                height: metrics.tmHeight,
                ascent: metrics.tmAscent,
            })
        }
    }

    /// The taskbar's own UI font, so the widget matches its surroundings.
    pub fn system_ui(height_px: i32, bold: bool) -> Option<Self> {
        Font::new("Segoe UI", height_px, bold)
    }

    /// Whether a font family is installed.
    ///
    /// GDI silently substitutes a missing family, so asking up front is the
    /// only way to know whether e.g. Segoe Fluent Icons is really available.
    pub fn family_exists(family: &str) -> bool {
        unsafe extern "system" fn callback(
            _font: *const LOGFONTW,
            _metrics: *const TEXTMETRICW,
            _kind: u32,
            found: LPARAM,
        ) -> i32 {
            unsafe {
                *(found.0 as *mut bool) = true;
            }
            0
        }

        unsafe {
            let name: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
            let mut logfont = LOGFONTW {
                lfCharSet: DEFAULT_CHARSET,
                ..Default::default()
            };
            let copy = name.len().min(logfont.lfFaceName.len());
            logfont.lfFaceName[..copy].copy_from_slice(&name[..copy]);

            let mut found = false;
            let dc = CreateCompatibleDC(None);
            EnumFontFamiliesExW(
                dc,
                &logfont,
                Some(callback),
                LPARAM(&mut found as *mut bool as isize),
                0,
            );
            let _ = DeleteDC(dc);
            found
        }
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.handle.into());
        }
    }
}

/// An off-screen 32-bit surface backed by a DIB section.
pub struct Canvas {
    hdc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    width: i32,
    height: i32,
}

impl Canvas {
    pub fn new(width: i32, height: i32) -> Option<Self> {
        let width = width.max(1);
        let height = height.max(1);

        unsafe {
            let hdc = CreateCompatibleDC(None);
            if hdc.is_invalid() {
                return None;
            }

            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative height gives a top-down bitmap, so row 0 is the
                    // top and indexing is the obvious `y * width + x`.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0);

            let bitmap = match bitmap {
                Ok(b) if !b.is_invalid() && !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(hdc);
                    return None;
                }
            };

            let previous = SelectObject(hdc, bitmap.into());
            SetBkMode(hdc, TRANSPARENT);

            Some(Canvas {
                hdc,
                bitmap,
                previous,
                bits: bits as *mut u32,
                width,
                height,
            })
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn hdc(&self) -> HDC {
        self.hdc
    }

    /// Resize in place, reallocating only when the size actually changes.
    pub fn resize(&mut self, width: i32, height: i32) -> bool {
        if self.width == width.max(1) && self.height == height.max(1) {
            return true;
        }
        match Canvas::new(width, height) {
            Some(replacement) => {
                *self = replacement;
                true
            }
            None => false,
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.width * self.height) as usize) }
    }

    /// Copy the surface out, for tests and diagnostics.
    pub fn snapshot(&mut self) -> Vec<u32> {
        self.pixels().to_vec()
    }

    pub fn clear(&mut self) {
        self.pixels().fill(0);
    }

    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, premultiplied: u32) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let index = (y * self.width + x) as usize;
        let pixels = self.pixels();
        pixels[index] = src_over(pixels[index], premultiplied);
    }

    pub fn fill_rect(&mut self, rect: RECT, color: Color) {
        let src = color.premultiplied();
        let x0 = rect.left.max(0);
        let y0 = rect.top.max(0);
        let x1 = rect.right.min(self.width);
        let y1 = rect.bottom.min(self.height);

        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, src);
            }
        }
    }

    /// Rounded rectangle, used for the widget's own background pill.
    pub fn fill_round_rect(&mut self, rect: RECT, radius: i32, color: Color) {
        let radius = radius
            .min((rect.right - rect.left) / 2)
            .min((rect.bottom - rect.top) / 2)
            .max(0);

        if radius == 0 {
            self.fill_rect(rect, color);
            return;
        }

        let src = color.premultiplied();
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            for x in rect.left.max(0)..rect.right.min(self.width) {
                // Distance from the nearest corner centre, if we are in a corner.
                let cx = if x < rect.left + radius {
                    rect.left + radius
                } else if x >= rect.right - radius {
                    rect.right - radius - 1
                } else {
                    x
                };
                let cy = if y < rect.top + radius {
                    rect.top + radius
                } else if y >= rect.bottom - radius {
                    rect.bottom - radius - 1
                } else {
                    y
                };

                let coverage = if cx == x && cy == y {
                    1.0
                } else {
                    let dx = (x - cx) as f32;
                    let dy = (y - cy) as f32;
                    edge_coverage(radius as f32 - (dx * dx + dy * dy).sqrt())
                };

                if coverage > 0.0 {
                    self.blend(x, y, scale_premultiplied(src, coverage));
                }
            }
        }
    }

    /// Anti-aliased filled disc.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        let src = color.premultiplied();
        let x0 = (cx - radius - 1.0).floor() as i32;
        let x1 = (cx + radius + 1.0).ceil() as i32;
        let y0 = (cy - radius - 1.0).floor() as i32;
        let y1 = (cy + radius + 1.0).ceil() as i32;

        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let coverage = edge_coverage(radius - (dx * dx + dy * dy).sqrt());
                if coverage > 0.0 {
                    self.blend(x, y, scale_premultiplied(src, coverage));
                }
            }
        }
    }

    /// Anti-aliased ring — this is the Discord speaking outline.
    pub fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, thickness: f32, color: Color) {
        let src = color.premultiplied();
        let outer = radius + thickness / 2.0;
        let inner = radius - thickness / 2.0;

        let x0 = (cx - outer - 1.0).floor() as i32;
        let x1 = (cx + outer + 1.0).ceil() as i32;
        let y0 = (cy - outer - 1.0).floor() as i32;
        let y1 = (cy + outer + 1.0).ceil() as i32;

        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let distance = (dx * dx + dy * dy).sqrt();
                let coverage =
                    edge_coverage(outer - distance).min(edge_coverage(distance - inner));
                if coverage > 0.0 {
                    self.blend(x, y, scale_premultiplied(src, coverage));
                }
            }
        }
    }

    /// Anti-aliased thick line with rounded ends.
    pub fn stroke_line(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        thickness: f32,
        color: Color,
    ) {
        let src = color.premultiplied();
        let radius = thickness / 2.0;

        let min_x = (x0.min(x1) - radius - 1.0).floor() as i32;
        let max_x = (x0.max(x1) + radius + 1.0).ceil() as i32;
        let min_y = (y0.min(y1) - radius - 1.0).floor() as i32;
        let max_y = (y0.max(y1) + radius + 1.0).ceil() as i32;

        let dx = x1 - x0;
        let dy = y1 - y0;
        let length_squared = dx * dx + dy * dy;

        for y in min_y..max_y {
            for x in min_x..max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;

                // Distance from the pixel to the segment.
                let t = if length_squared <= f32::EPSILON {
                    0.0
                } else {
                    (((px - x0) * dx + (py - y0) * dy) / length_squared).clamp(0.0, 1.0)
                };
                let nearest_x = x0 + t * dx;
                let nearest_y = y0 + t * dy;
                let distance =
                    ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt();

                let coverage = edge_coverage(radius - distance);
                if coverage > 0.0 {
                    self.blend(x, y, scale_premultiplied(src, coverage));
                }
            }
        }
    }

    /// Draw `bitmap` clipped to a circle, scaling it to `diameter`.
    /// Nearest-neighbour is fine because avatars are fetched at roughly the
    /// size they are drawn.
    pub fn draw_circular_bitmap(
        &mut self,
        bitmap: &Bitmap,
        cx: f32,
        cy: f32,
        diameter: f32,
        opacity: f32,
    ) {
        if bitmap.width <= 0 || bitmap.height <= 0 {
            return;
        }

        let radius = diameter / 2.0;
        let x0 = (cx - radius - 1.0).floor() as i32;
        let x1 = (cx + radius + 1.0).ceil() as i32;
        let y0 = (cy - radius - 1.0).floor() as i32;
        let y1 = (cy + radius + 1.0).ceil() as i32;

        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let coverage = edge_coverage(radius - (dx * dx + dy * dy).sqrt());
                if coverage <= 0.0 {
                    continue;
                }

                let u = ((dx + radius) / diameter * bitmap.width as f32) as i32;
                let v = ((dy + radius) / diameter * bitmap.height as f32) as i32;
                let texel = bitmap.get(u.clamp(0, bitmap.width - 1), v.clamp(0, bitmap.height - 1));

                self.blend(x, y, scale_premultiplied(texel, coverage * opacity));
            }
        }
    }

    pub fn measure_text(&self, text: &str, font: &Font) -> (i32, i32) {
        if text.is_empty() {
            return (0, font.height);
        }
        unsafe {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let previous = SelectObject(self.hdc, font.handle.into());
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(self.hdc, &wide, &mut size);
            SelectObject(self.hdc, previous);
            (size.cx, size.cy.max(font.height))
        }
    }

    /// Draw text at `(x, y)` (top-left) and return its advance width.
    ///
    /// Rasterises into a scratch DIB and composites the result as a coverage
    /// mask, because GDI leaves the alpha channel untouched.
    pub fn draw_text(&mut self, text: &str, font: &Font, x: i32, y: i32, color: Color) -> i32 {
        if text.is_empty() {
            return 0;
        }

        let (text_width, text_height) = self.measure_text(text, font);
        if text_width <= 0 || text_height <= 0 {
            return 0;
        }

        let Some(mut mask) = Canvas::new(text_width, text_height) else {
            return text_width;
        };

        unsafe {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let previous = SelectObject(mask.hdc, font.handle.into());
            SetBkMode(mask.hdc, TRANSPARENT);
            // White on the zeroed DIB, so each channel ends up holding the
            // glyph's anti-aliasing coverage.
            SetTextColor(mask.hdc, COLORREF(0x00FF_FFFF));
            let _ = TextOutW(mask.hdc, 0, 0, &wide);
            SelectObject(mask.hdc, previous);
            let _ = GdiFlush();
        }

        let base = color.premultiplied();
        let mask_width = mask.width;
        let mask_height = mask.height;
        let mask_pixels: Vec<u32> = mask.pixels().to_vec();

        for my in 0..mask_height {
            for mx in 0..mask_width {
                let pixel = mask_pixels[(my * mask_width + mx) as usize];
                let r = (pixel >> 16) & 0xFF;
                let g = (pixel >> 8) & 0xFF;
                let b = pixel & 0xFF;
                let coverage = r.max(g).max(b);
                if coverage == 0 {
                    continue;
                }
                self.blend(
                    x + mx,
                    y + my,
                    scale_premultiplied(base, coverage as f32 / 255.0),
                );
            }
        }

        text_width
    }

    /// Truncate with an ellipsis so long channel names cannot push the widget
    /// into the clock.
    pub fn draw_text_ellipsised(
        &mut self,
        text: &str,
        font: &Font,
        x: i32,
        y: i32,
        max_width: i32,
        color: Color,
    ) -> i32 {
        let (width, _) = self.measure_text(text, font);
        if width <= max_width {
            return self.draw_text(text, font, x, y, color);
        }

        let chars: Vec<char> = text.chars().collect();
        let mut best = String::new();
        for count in (1..chars.len()).rev() {
            let candidate: String = chars[..count].iter().collect::<String>() + "…";
            if self.measure_text(&candidate, font).0 <= max_width {
                best = candidate;
                break;
            }
        }

        if best.is_empty() {
            return 0;
        }
        self.draw_text(&best, font, x, y, color)
    }

    /// Surrender the underlying bitmap, for APIs that need to own an
    /// `HBITMAP` (such as `CreateIconIndirect`). The canvas is consumed and
    /// will not delete the bitmap.
    pub fn take_bitmap(mut self) -> HBITMAP {
        unsafe {
            SelectObject(self.hdc, self.previous);
        }
        let bitmap = self.bitmap;
        self.bitmap = HBITMAP::default();
        bitmap
    }

    /// Push the surface to a layered window, setting its position and size at
    /// the same time.
    ///
    /// `UpdateLayeredWindow` documents `psize` as mandatory whenever `hdcSrc`
    /// is given, so the position-and-size-less form is not usable for a window
    /// that has not been sized by a previous call. Passing all three is the
    /// correct usage and is atomic, which also avoids a frame where the window
    /// is the right size but still holds the old content.
    pub fn present_layered_at(&self, hwnd: HWND, x: i32, y: i32) -> bool {
        unsafe {
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let source = POINT { x: 0, y: 0 };
            let destination = POINT { x, y };
            let size = SIZE {
                cx: self.width,
                cy: self.height,
            };

            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&destination),
                Some(&size),
                Some(self.hdc),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
            .is_ok()
        }
    }

    /// Opaque fallback path for when layering is unavailable.
    pub fn blit(&self, target: HDC) {
        unsafe {
            let _ = BitBlt(
                target,
                0,
                0,
                self.width,
                self.height,
                Some(self.hdc),
                0,
                0,
                SRCCOPY,
            );
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.hdc, self.previous);
            // Invalid when `take_bitmap` handed ownership to someone else.
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
            let _ = DeleteDC(self.hdc);
        }
    }
}

/// Map a signed distance (in pixels, positive = inside) to edge coverage.
#[inline]
fn edge_coverage(distance: f32) -> f32 {
    (distance + 0.5).clamp(0.0, 1.0)
}

/// Scale a premultiplied pixel's contribution by `factor`.
#[inline]
fn scale_premultiplied(pixel: u32, factor: f32) -> u32 {
    if factor >= 1.0 {
        return pixel;
    }
    if factor <= 0.0 {
        return 0;
    }
    let scale = |shift: u32| (((pixel >> shift) & 0xFF) as f32 * factor).round() as u32 & 0xFF;
    (scale(24) << 24) | (scale(16) << 16) | (scale(8) << 8) | scale(0)
}
