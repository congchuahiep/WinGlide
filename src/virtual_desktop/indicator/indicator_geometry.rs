//! Dot sizing and window placement, resolved from the taskbar snapshot.
//!
//! [`IndicatorGeometry`] is the single source of truth for "how big is each dot,
//! how long is the row, and where does the window go". Both rendering and mouse
//! hit-testing go through it, so the hover zones always line up with the dots.

use super::indicator_position::IndicatorPosition;
use super::utils;
use super::INDICATOR_MARGIN;
use crate::taskbar::{Taskbar, TaskbarEdge};

/// Dot radius as a fraction of the sizing reference (a standard taskbar
/// thickness).
const DOT_RADIUS_RATIO: f32 = 0.07;
/// Height of the background pill / hover highlight, as a fraction of the dot
/// scale. Half of a standard taskbar, so the pill is the same height (24px at
/// 100% DPI) in every taskbar layout - a label-mode row is exactly this tall.
const PILL_HEIGHT_RATIO: f32 = 0.5;
/// Padding (px) kept between the background pill and the dot row, along the row.
pub(super) const BACKGROUND_ALONG_MARGIN: f32 = 6.0;
/// Extra length (px) the background pill adds on top of the dot row.
pub(super) const BACKGROUND_GROW_ALONG: f32 = 4.0;
/// Slack (px) added to the window along the dots so the background pill, which
/// is wider than the dot row, is never clipped.
const BACKGROUND_ROOM_ALONG: i32 = 4;

/// Shared geometry of the virtual-desktop dots, in logical coordinates: `x` runs
/// along the dots, `y` across them.
#[derive(Clone, Copy)]
pub(super) struct DotLayout {
    pub(super) radius: f32,
    pub(super) spacing: f32,
    start_x: f32,
}

impl DotLayout {
    /// Taskbar at 1080p is usually 48px thick; binding the dot size to that
    /// reference keeps the dots consistent on every screen and DPI.
    fn new(scale: i32) -> Self {
        let radius = scale as f32 * DOT_RADIUS_RATIO; // ~3.36px at 48px thickness
        let spacing = radius * 5.0;
        Self {
            radius,
            spacing,
            start_x: 10.0 + radius,
        }
    }

    /// Center of dot `index` along the dots' axis.
    pub(super) fn center(&self, index: usize) -> f32 {
        self.start_x + index as f32 * self.spacing
    }

    /// First and last pixel covered by the dot row (centers ± radius) along the
    /// dots' axis. Used to size the indicator background.
    pub(super) fn extent(&self, count: usize) -> (f32, f32) {
        (
            self.center(0) - self.radius,
            self.center(count.saturating_sub(1)) + self.radius,
        )
    }

    /// Total length needed so the last dot and its hitbox are fully visible.
    fn length(&self, count: usize) -> i32 {
        (self.center(count.saturating_sub(1)) + self.spacing / 2.0).ceil() as i32 + 1
    }
}

/// Everything the indicator needs to render / place itself, resolved from the
/// taskbar edge and (for a vertical taskbar) whether labels are shown.
///
/// * horizontal taskbar -> dots run along X, cross-axis is the bar's height;
/// * vertical, icons only -> dots run along Y inside the narrow column;
/// * vertical, with labels -> the column is wide, so the dots run along X as a
///   short horizontal row (a vertical stack would look crude).
#[derive(Clone, Copy)]
pub(super) struct IndicatorGeometry {
    edge: TaskbarEdge,
    dots_vertical: bool,
    /// Dot sizing reference (drives radius and spacing).
    scale: i32,
    /// Window extent across the dots.
    cross: i32,
    /// Window extent along the dots.
    length: i32,
}

impl IndicatorGeometry {
    /// Resolves the geometry for the given taskbar, or `None` when the bar has
    /// no usable thickness.
    pub(super) fn for_taskbar(taskbar: &Taskbar) -> Option<Self> {
        let edge = taskbar.edge();
        let thickness = taskbar.thickness();
        if thickness <= 0 {
            return None;
        }

        let count = utils::count();
        let label_mode = taskbar.is_label_mode();
        let dots_vertical = edge.is_vertical() && !label_mode;

        // Dot size reference: a standard taskbar thickness. In label mode the
        // column is much wider than that, so the dots keep a normal size (shrunk
        // only if the row would not fit the column's width).
        let scale = if label_mode {
            fit_scale(taskbar.icon_scale(), thickness, count)
        } else {
            thickness
        };

        // Short dimension of the window. A horizontal bar and a vertical
        // icons-only column use the bar's thickness; a label-mode row only needs
        // room for the (hover-sized) dots and the background pill around them, so it
        // hugs the taskbar edge instead of floating inside a full-thickness band.
        let cross = if label_mode {
            compact_row_height(scale)
        } else {
            thickness
        };

        Some(Self {
            edge,
            dots_vertical,
            scale,
            cross,
            length: DotLayout::new(scale).length(count) + BACKGROUND_ROOM_ALONG,
        })
    }

    pub(super) fn dots_vertical(&self) -> bool {
        self.dots_vertical
    }

    /// Half the background pill's extent across the dots. The pill is the same
    /// height in every layout, so it never grows beyond the window.
    pub(super) fn pill_half_across(&self) -> f32 {
        self.scale as f32 * PILL_HEIGHT_RATIO / 2.0
    }

    pub(super) fn cross(&self) -> i32 {
        self.cross
    }

    pub(super) fn length(&self) -> i32 {
        self.length
    }

    pub(super) fn dot_layout(&self) -> DotLayout {
        DotLayout::new(self.scale)
    }

    /// Physical bitmap / window size `(width, height)`.
    pub(super) fn physical_size(&self) -> (i32, i32) {
        if self.dots_vertical {
            (self.cross, self.length)
        } else {
            (self.length, self.cross)
        }
    }

    /// Desktop dot under the client point `(x, y)`, using the coordinate along
    /// the dots' axis.
    pub(super) fn hit_test(&self, x: i32, y: i32) -> Option<usize> {
        let layout = self.dot_layout();
        let half_spacing = layout.spacing / 2.0;
        let along = (if self.dots_vertical { y } else { x }) as f32;
        (0..utils::count()).find(|&i| {
            let cx = layout.center(i);
            // Rectangular hit-box, connected continuously without gaps.
            along >= cx - half_spacing && along <= cx + half_spacing
        })
    }

    /// Screen coordinates (top-left) at which the indicator window is placed.
    ///
    /// For a horizontal taskbar the indicator is offset along X. For a vertical
    /// taskbar it lies inside the column: offset along Y by the position preset,
    /// and centered across the column for the horizontal (label-mode) dot row.
    pub(super) fn origin(&self, taskbar: &Taskbar, position: IndicatorPosition) -> (i32, i32) {
        let rect = taskbar.rect();
        if !self.edge.is_vertical() {
            return (self.horizontal_left(taskbar, position), rect.top);
        }

        let thickness = rect.right - rect.left;
        let x = if self.dots_vertical {
            rect.left
        } else {
            rect.left + (thickness - self.length).max(0) / 2
        };
        (x, self.vertical_anchor(taskbar, position))
    }

    fn horizontal_left(&self, taskbar: &Taskbar, position: IndicatorPosition) -> i32 {
        let rect = taskbar.rect();
        let left_of_tray = match taskbar.tray_left() {
            Some(l) => (l - INDICATOR_MARGIN - self.length).max(rect.left),
            None => rect.right - INDICATOR_MARGIN - self.length,
        };
        match position {
            IndicatorPosition::Left => rect.left + 10,
            IndicatorPosition::Right => left_of_tray,
            IndicatorPosition::Auto => {
                if taskbar.alignment_left() {
                    left_of_tray
                } else {
                    rect.left + 10
                }
            }
        }
    }

    /// `Left` pins the indicator to the top (start), `Right` to just above the
    /// system tray (end), and `Auto` mirrors the horizontal alignment rule
    /// instead of centering on the taskbar.
    fn vertical_anchor(&self, taskbar: &Taskbar, position: IndicatorPosition) -> i32 {
        let rect = taskbar.rect();
        // Offset along the bar by the indicator's *height*: in label mode that is
        // `cross`, not `length` (the row's width).
        let height = self.physical_size().1;
        let above_tray = match taskbar.tray_top() {
            Some(top) => (top - INDICATOR_MARGIN - height).max(rect.top),
            None => rect.bottom - INDICATOR_MARGIN - height,
        };
        match position {
            IndicatorPosition::Left => rect.top + 10,
            IndicatorPosition::Right => above_tray,
            IndicatorPosition::Auto => {
                if taskbar.alignment_left() {
                    above_tray
                } else {
                    rect.top + 10
                }
            }
        }
    }
}

/// Height of the compact horizontal dot row used in label mode. It is exactly
/// the background pill's height, so the pill fills the row without clipping.
fn compact_row_height(scale: i32) -> i32 {
    (scale as f32 * PILL_HEIGHT_RATIO).round() as i32
}

/// Largest dot scale no bigger than `preferred` for which the whole indicator
/// still fits inside `available` px along the dots' axis.
fn fit_scale(preferred: i32, available: i32, count: usize) -> i32 {
    let row_length = |scale: i32| DotLayout::new(scale).length(count) + BACKGROUND_ROOM_ALONG;
    if row_length(preferred) <= available {
        return preferred.max(1);
    }
    // DotLayout::length is affine in the scale: length ≈ 11 + k * scale.
    let k = 0.245 + 0.35 * (count.saturating_sub(1) as f32);
    if k <= 0.0 {
        return 1;
    }
    (((available - 11) as f32 / k).floor() as i32).max(1)
}

/// Desktop dot under the client point `(x, y)` of the indicator window, if any.
pub(super) fn hovered_dot(x: i32, y: i32) -> Option<usize> {
    let taskbar = Taskbar::primary()?;
    IndicatorGeometry::for_taskbar(&taskbar)?.hit_test(x, y)
}
