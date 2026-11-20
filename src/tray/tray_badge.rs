//! Renders the desktop number into the tray glyph's middle cell.
//!
//! The digits are drawn from a 3x5 pixel font instead of a system font: at the
//! 16 px tray size a hinted font turns to mush (7 px tall glyphs, smeared by the
//! anti-aliasing), while block digits stay readable and match the artwork, which is
//! itself drawn on a 16-unit grid with 1 px strokes. The ICO entry supplies the
//! glyph; the digit is stamped into its pixels before the HICON is built.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

use super::tray_icon;

/// 3x5 pixel digits, one string per row.
const DIGITS: [[&str; 5]; 10] = [
    ["111", "101", "101", "101", "111"], // 0
    ["010", "110", "010", "010", "111"], // 1
    ["111", "001", "111", "100", "111"], // 2
    ["111", "001", "111", "001", "111"], // 3
    ["101", "101", "111", "001", "001"], // 4
    ["111", "100", "111", "001", "111"], // 5
    ["111", "100", "111", "101", "111"], // 6
    ["111", "001", "001", "001", "001"], // 7
    ["111", "101", "111", "101", "111"], // 8
    ["111", "101", "111", "001", "111"], // 9
];

/// `+` for the `9+` overflow label.
const PLUS: [&str; 5] = ["000", "010", "111", "010", "000"];

/// Glyphs to draw: one digit, or `9+` past nine desktops.
fn glyphs(number: usize) -> Vec<[&'static str; 5]> {
    if number > 9 {
        vec![DIGITS[9], PLUS]
    } else {
        vec![DIGITS[number.max(1)]]
    }
}

/// Decodes one ICO entry into top-down RGBA pixels.
///
/// The generator writes classic 32bpp bottom-up DIB entries whose alpha channel
/// carries the shape (the AND mask is all zero), so the colour bytes are all that
/// is needed. Returns `None` for any entry shape this renderer does not expect.
fn decode_entry(bytes: &[u8]) -> Option<(i32, i32, Vec<u8>)> {
    if bytes.len() < 40 {
        return None;
    }
    let width = i32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let double_height = i32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let bpp = u16::from_le_bytes(bytes[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(bytes[16..20].try_into().ok()?);
    let height = double_height / 2;
    if width <= 0 || height <= 0 || bpp != 32 || compression != 0 {
        return None;
    }

    let pixels = (width * height) as usize;
    let source = bytes.get(40..40 + pixels * 4)?;
    let mut rgba = vec![0u8; pixels * 4];
    for y in 0..height as usize {
        let src = (height as usize - 1 - y) * width as usize * 4;
        let dst = y * width as usize * 4;
        for x in 0..width as usize {
            let s = src + x * 4;
            let d = dst + x * 4;
            rgba[d] = source[s + 2]; // R
            rgba[d + 1] = source[s + 1]; // G
            rgba[d + 2] = source[s]; // B
            rgba[d + 3] = source[s + 3]; // A
        }
    }
    Some((width, height, rgba))
}

/// Stamps the digit block into `rgba`, centred in the glyph's middle cell.
fn draw_digits(rgba: &mut [u8], width: i32, height: i32, number: usize, light: bool) {
    let shapes = glyphs(number);
    let columns = shapes.len() * 4 - 1; // 3 wide + 1 gap between glyphs

    // Free space inside the middle cell: the cell is 9x13 of the 16-unit grid
    // and the 1-unit stroke eats one unit on each axis, leaving 8x12 units.
    let inner_w = width / 2;
    let inner_h = height * 3 / 4;

    // 2 px per font dot at 16 px, scaled with the icon; shrink when the label
    // (`9+`) needs more columns than the cell can give.
    let mut dot = (width / 8).max(1);
    dot = dot.min((inner_w / columns as i32).max(1));
    dot = dot.min((inner_h / 5).max(1));

    let block_w = columns as i32 * dot;
    let block_h = 5 * dot;
    let left = width / 2 - block_w / 2;
    let top = height / 2 - block_h / 2;
    let color: [u8; 3] = if light { [0, 0, 0] } else { [255, 255, 255] };

    for (index, shape) in shapes.iter().enumerate() {
        for (row, line) in shape.iter().enumerate() {
            for (col, bit) in line.bytes().enumerate() {
                if bit != b'1' {
                    continue;
                }
                let x0 = left + (index as i32 * 4 + col as i32) * dot;
                let y0 = top + row as i32 * dot;
                for y in y0..y0 + dot {
                    for x in x0..x0 + dot {
                        if x < 0 || y < 0 || x >= width || y >= height {
                            continue;
                        }
                        let at = ((y * width + x) * 4) as usize;
                        rgba[at..at + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
                    }
                }
            }
        }
    }
}

/// Builds a `size`-px HICON with `number` inside the glyph's middle cell.
///
/// Returns `None` when the artwork is unavailable; the caller then keeps the
/// plain glyph.
pub(super) fn build(size: i32, light: bool, number: usize) -> Option<HICON> {
    let (offset, length) = tray_icon::TrayIcon::artwork_entry(size, light)?;
    let entry = &tray_icon::TrayIcon::artwork_bytes(light)[offset..offset + length];
    let (width, height, mut rgba) = decode_entry(entry)?;
    draw_digits(&mut rgba, width, height, number, light);

    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = null_mut();
        let Ok(color) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            tracing::warn!("badge: CreateDIBSection failed");
            return None;
        };
        std::ptr::copy_nonoverlapping(rgba.as_ptr(), bits as *mut u8, rgba.len());

        // The alpha channel carries the shape; the monochrome mask stays empty.
        let mask_row = ((width + 15) / 16) * 2;
        let mask_bits = vec![0u8; (mask_row * height) as usize];
        let mask = CreateBitmap(
            width,
            height,
            1,
            1,
            Some(mask_bits.as_ptr() as *const c_void),
        );

        let info = ICONINFO {
            fIcon: TRUE,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).ok();

        let _ = DeleteObject(HGDIOBJ(color.0 as _));
        let _ = DeleteObject(HGDIOBJ(mask.0 as _));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        icon
    }
}