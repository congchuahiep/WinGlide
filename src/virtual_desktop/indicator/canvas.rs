//! A 32-bit ARGB drawing surface addressed in *logical* coordinates.
//!
//! All dot layout math is written against one axis (the dots' axis) and one
//! cross axis. [`Canvas`] maps that logical space onto the physical bitmap,
//! transposing the axes for a vertical dot column, so no drawing code has to
//! care which way the taskbar runs.

/// Logical `x` runs along the dots (their length), `y` runs across them
/// (thickness). For a vertical dot column the pixels are transposed while
/// writing.
pub(super) struct Canvas<'a> {
    buffer: &'a mut [u32],
    phys_width: i32,
    phys_height: i32,
    vertical: bool,
}

impl<'a> Canvas<'a> {
    pub(super) fn new(
        buffer: &'a mut [u32],
        phys_width: i32,
        phys_height: i32,
        vertical: bool,
    ) -> Self {
        Self {
            buffer,
            phys_width,
            phys_height,
            vertical,
        }
    }

    /// Maps logical `(x, y)` to a buffer index, or `None` when out of bounds.
    #[inline]
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 {
            return None;
        }
        let (px, py) = if self.vertical { (y, x) } else { (x, y) };
        if px >= self.phys_width || py >= self.phys_height {
            return None;
        }
        Some(py as usize * self.phys_width as usize + px as usize)
    }

    #[inline]
    pub(super) fn get(&self, x: i32, y: i32) -> u32 {
        self.index(x, y).map(|i| self.buffer[i]).unwrap_or(0)
    }

    #[inline]
    pub(super) fn set(&mut self, x: i32, y: i32, value: u32) {
        if let Some(i) = self.index(x, y) {
            self.buffer[i] = value;
        }
    }
}
