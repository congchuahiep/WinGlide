//! Event module - quản lý WinEvent hook và UIA StructureChanged event.
//!
//! WinEvent: theo dõi EVENT_OBJECT_SHOW để uncombine cửa sổ mới.
//! UIA: theo dõi StructureChanged để invalidate button cache.

mod uia;
mod winevent;

pub use uia::*;
pub use winevent::*;
pub const WM_APP_RELOAD_CONFIG: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 0x102;
pub const WM_APP_RESTART_AS_ADMIN: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 0x103;
