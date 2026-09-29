//! Paints the virtual-desktop dots and their hover highlights onto a [`Canvas`].
//!
//! Everything here works in the canvas' logical coordinates, so the same code
//! draws a horizontal row and a vertical column. Dots are anti-aliased with a
//! signed-distance circle so they stay smooth without GDI.

use super::canvas::Canvas;
use super::indicator_geometry::{
    DotLayout, IndicatorGeometry, BACKGROUND_ALONG_MARGIN, BACKGROUND_GROW_ALONG,
};
use super::utils::count as get_desktop_count;
use crate::shared::system_theme::{accent_color, is_light_theme};

const DOTS_ON_LIGHT_THEME: (u8, u8, u8) = (20, 20, 20);
const DOTS_ON_DARK_THEME: (u8, u8, u8) = (235, 235, 235);

const INACTIVE_DESATURATION: f32 = 0.6;

const LIGHT_ACCENT_CONTRAST_BLEND: f32 = 0.3;
const DARK_ACCENT_CONTRAST_BLEND: f32 = 0.4;

const BACKGROUND_ALPHA: f32 = 0.25;
const CORNER_RADIUS: f32 = 6.0;

/// Draws a full set of desktop dots for one indicator frame.
pub(super) struct DotPainter {
    layout: DotLayout,
    length: i32,
    thickness: i32,
    /// Active / hovered dot color.
    active_color: (u8, u8, u8),
    /// Inactive dot color (a desaturated `active_color`).
    inactive_color: (u8, u8, u8),
    /// Neutral color for hover highlights and the row background.
    hitbox_color: (u8, u8, u8),
    /// Half the background pill's extent across the dots (also the hover
    /// highlight's half-height).
    pill_half_across: f32,
    background: bool,
    light_mode: bool,
}

impl DotPainter {
    /// Builds a painter for one frame. When `accent` (from config) is set the
    /// dots use the Windows accent color, falling back to the theme-neutral
    /// color if the accent can't be read. `background` draws a translucent
    /// rounded pill behind the whole row.
    pub(super) fn new(geometry: IndicatorGeometry, accent: bool, background: bool) -> Self {
        let light_mode = is_light_theme();
        let theme_dots = if light_mode {
            DOTS_ON_LIGHT_THEME
        } else {
            DOTS_ON_DARK_THEME
        };
        let active_color = if accent {
            accent_color().map_or(theme_dots, |rgb| blend_for_theme(rgb, light_mode))
        } else {
            theme_dots
        };
        let hitbox_color = if light_mode {
            (255, 255, 255)
        } else {
            (100, 100, 100)
        };
        Self {
            layout: geometry.dot_layout(),
            length: geometry.length(),
            thickness: geometry.cross(),
            active_color,
            inactive_color: desaturate(active_color, INACTIVE_DESATURATION),
            hitbox_color,
            pill_half_across: geometry.pill_half_across(),
            background,
            light_mode,
        }
    }

    /// Paints every dot. `hover` and `current` are desktop indices; `move_mode`
    /// tints the hover highlight with the "Alt held" accent color.
    pub(super) fn paint(
        &self,
        canvas: &mut Canvas<'_>,
        hover: Option<usize>,
        current: usize,
        move_mode: bool,
    ) {
        if self.background {
            self.draw_row_background(canvas);
        }

        let cy = self.thickness as f32 / 2.0;
        for i in 0..get_desktop_count() {
            let cx = self.layout.center(i);
            let is_hovered = hover == Some(i);
            let is_active = i == current;

            // Invisible hit-box (plus a rounded highlight when hovered).
            self.draw_hitbox_and_bg(canvas, cx, is_hovered, move_mode);

            let mut radius = self.layout.radius;
            let mut alpha = if is_active {
                radius *= 1.25; // Active dot is as big as a hovered one
                1.0
            } else {
                0.5
            };
            if is_hovered {
                radius = self.layout.radius * 1.25;
                if alpha < 0.8 {
                    alpha = 0.8; // Brighten up
                }
            }

            let color = if is_active || is_hovered {
                self.active_color
            } else {
                self.inactive_color
            };
            self.draw_aa_circle(canvas, cx, cy, radius, color, alpha);
        }
    }

    /// Translucent rounded pill behind the whole dot row, so the dots read as a
    /// single control instead of specks on the taskbar.
    fn draw_row_background(&self, canvas: &mut Canvas<'_>) {
        let count = get_desktop_count();
        let (first, last) = self.layout.extent(count);
        let cx = (first + last) / 2.0;
        let cy = self.thickness as f32 / 2.0;
        let half_w = (last - first) / 2.0 + BACKGROUND_ALONG_MARGIN + BACKGROUND_GROW_ALONG / 2.0;
        let half_h = self.pill_half_across;
        let corner_radius = CORNER_RADIUS.min(half_w).min(half_h).max(0.0);

        let min_x = (cx - half_w).floor().max(0.0) as i32;
        let max_x = (cx + half_w).ceil().min((self.length - 1) as f32) as i32;
        let max_y = self.thickness - 1;

        for y in 0..=max_y {
            for x in min_x..=max_x {
                let alpha = rounded_rect_alpha(x, y, cx, cy, half_w, half_h, corner_radius);
                if alpha > 0.0 {
                    write_pixel(canvas, x, y, self.hitbox_color, alpha * BACKGROUND_ALPHA);
                }
            }
        }
    }

    /// Invisible rectangular hit-box, plus a blurred rounded background on hover.
    fn draw_hitbox_and_bg(
        &self,
        canvas: &mut Canvas<'_>,
        cx: f32,
        is_hovered: bool,
        move_mode: bool,
    ) {
        let spacing = self.layout.spacing;
        let thickness = self.thickness;
        let half_spacing = spacing / 2.0;
        let min_x = (cx - half_spacing).floor().max(0.0) as i32;
        let max_x = (cx + half_spacing).ceil().min((self.length - 1) as f32) as i32;
        let max_y = thickness - 1;

        let cy = thickness as f32 / 2.0;

        // Radius along the taskbar. Left at `spacing / 2` so hover backgrounds
        // touch continuously (no gap).
        let bg_rw = spacing / 2.0;
        // Radius across the taskbar: the hover highlight is exactly the height of
        // the background pill, so hovering never spills outside it.
        let bg_rh = self.pill_half_across;
        let corner_radius = CORNER_RADIUS.min(bg_rh * 0.5).max(0.0);

        // In "move" mode (Alt held) tint the hover background with an accent
        // color to signal that a click will move the current window there.
        let color = if move_mode {
            if self.light_mode {
                (0, 92, 175)
            } else {
                (110, 170, 255)
            }
        } else {
            self.hitbox_color
        };

        for y in 0..=max_y {
            for x in min_x..=max_x {
                if is_hovered {
                    let alpha = rounded_rect_alpha(x, y, cx, cy, bg_rw, bg_rh, corner_radius) * 0.3;
                    if alpha > 0.0 {
                        write_pixel(canvas, x, y, color, alpha);
                        continue;
                    }
                }

                // Invisible hitbox
                if canvas.get(x, y) == 0 {
                    canvas.set(x, y, 0x01000000);
                }
            }
        }
    }

    /// Anti-aliased SDF circle.
    fn draw_aa_circle(
        &self,
        canvas: &mut Canvas<'_>,
        cx: f32,
        cy: f32,
        radius: f32,
        color: (u8, u8, u8),
        base_alpha: f32,
    ) {
        let thickness = self.thickness;

        let min_x = (cx - radius - 1.0).floor().max(0.0) as i32;
        let max_x = (cx + radius + 1.0).ceil().min((self.length - 1) as f32) as i32;
        let min_y = (cy - radius - 1.0).floor().max(0.0) as i32;
        let max_y = (cy + radius + 1.0).ceil().min((thickness - 1) as f32) as i32;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let dx = px - cx;
                let dy = py - cy;
                let distance = (dx * dx + dy * dy).sqrt();

                let alpha = smooth_edge(distance, radius) * base_alpha;
                if alpha > 0.0 {
                    write_pixel(canvas, x, y, color, alpha);
                }
            }
        }
    }
}

/// Anti-aliased coverage of a signed distance `dist` from an edge: fully inside
/// at `<= -0.5`, fully outside at `>= +0.5`.
fn coverage(dist: f32) -> f32 {
    (0.5 - dist).clamp(0.0, 1.0)
}

/// Signed distance to a circle of `radius` for a sample `distance` from center.
fn smooth_edge(distance: f32, radius: f32) -> f32 {
    coverage(distance - radius)
}

/// Anti-aliased coverage of a rounded rectangle centered on `(cx, cy)`.
fn rounded_rect_alpha(
    x: i32,
    y: i32,
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
    corner_radius: f32,
) -> f32 {
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let dx = (px - cx).abs() - (half_w - corner_radius);
    let dy = (py - cy).abs() - (half_h - corner_radius);
    let dist = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - corner_radius;
    coverage(dist)
}

/// Writes `color` at `alpha` (premultiplied) into the canvas.
fn write_pixel(canvas: &mut Canvas<'_>, x: i32, y: i32, color: (u8, u8, u8), alpha: f32) {
    let (r, g, b) = color;
    let a = (alpha * 255.0) as u32;
    let pr = (r as f32 * alpha) as u32;
    let pg = (g as f32 * alpha) as u32;
    let pb = (b as f32 * alpha) as u32;
    canvas.set(x, y, (a << 24) | (pr << 16) | (pg << 8) | pb);
}

/// Pulls `color` toward its grayscale equivalent by `amount` (0 = unchanged,
/// 1 = fully gray), muting accent dots that are not active.
fn desaturate(color: (u8, u8, u8), amount: f32) -> (u8, u8, u8) {
    let (r, g, b) = color;
    let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32)
        .round()
        .clamp(0.0, 255.0) as u8;
    mix(color, (luma, luma, luma), amount)
}

/// Mixes `color` toward the theme's contrast color: black on a light taskbar,
/// white on a dark one, so the accent keeps contrast in both themes.
fn blend_for_theme(color: (u8, u8, u8), light_mode: bool) -> (u8, u8, u8) {
    let target_blend = match light_mode {
        true => DOTS_ON_LIGHT_THEME,
        false => DOTS_ON_DARK_THEME,
    };

    let target_constrast = match light_mode {
        true => LIGHT_ACCENT_CONTRAST_BLEND,
        false => DARK_ACCENT_CONTRAST_BLEND,
    };

    mix(color, target_blend, target_constrast)
}

/// Linear mix per channel: `t = 0` keeps `from`, `t = 1` gives `to`.
fn mix(from: (u8, u8, u8), to: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let blend = |a: u8, b: u8| {
        (a as f32 + (b as f32 - a as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (
        blend(from.0, to.0),
        blend(from.1, to.1),
        blend(from.2, to.2),
    )
}
