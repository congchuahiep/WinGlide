//! Global hotkey management with two delivery mechanisms.
//!
//! - [`manager::HotkeyManager`] owns the hotkey set, classifies every hotkey into
//!   one of two dispatch kinds, and maps `WM_HOTKEY` IDs back to actions.
//! - [`manager::HotkeyDispatch::Registered`]: the hotkey is registered with
//!   `RegisterHotKey`; the OS watches for it and posts `WM_HOTKEY` — kernel-filtered,
//!   zero per-keystroke cost in this process, but cannot claim combinations another
//!   app (or Windows itself) already owns.
//! - [`manager::HotkeyDispatch::LowLevelHook`]: the hotkey is intercepted by a
//!   low-level keyboard hook ([`low_level_hook::LowLevelKeyHook`]); we match and
//!   swallow the key before the shell reacts and post `WM_HOTKEY` ourselves. Used
//!   for Win-involved combinations that Windows owns (native taskbar shortcuts like
//!   `Win+1..Win+9`, reserved combos).
//!
//! Both mechanisms post `WM_HOTKEY` with the same hotkey IDs, so action dispatch is
//! identical regardless of how a hotkey is delivered.

mod low_level_hook;
mod manager;

pub use low_level_hook::LowLevelKeyHook;
pub use manager::{HotkeyAction, HotkeyManager};
