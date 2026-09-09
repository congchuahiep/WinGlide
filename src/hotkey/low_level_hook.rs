//! Low-level keyboard hook for hotkeys that `RegisterHotKey` cannot claim.
//!
//! Some Windows-native shortcuts (notably `Win+<number>` taskbar app switching, but
//! also reserved combos like `Win+L`, `Win+E`, `Win+R`) are already owned by Windows
//! and cannot be re-registered with `RegisterHotKey`. A `WH_KEYBOARD_LL` hook intercepts
//! and swallows the key press before the shell can react, then posts a `WM_HOTKEY`
//! message so *any* hotkey is dispatched through the same
//! [`crate::app::App::handle_hotkey`] path used by `RegisterHotKey`.
//!
//! The hook is generic: [`LowLevelKeyHook::set_hotkeys`] accepts an arbitrary set of
//! `(id, modifiers, vk)` combinations. To make a new hotkey low-level, classify it as
//! [`HotkeyDispatch::LowLevelHook`] in the manager and it flows through automatically
//! (see `HotkeyManager::low_level_hotkeys`).
//!
//! The hook must be installed/uninstalled and its hotkey set updated from the thread
//! that pumps the message loop (the main thread in [`crate::app::App::run`]).

use std::cell::RefCell;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_KEYDOWN: u32 = 0x0100;
const WM_SYSKEYDOWN: u32 = 0x0104;

/// A hotkey handled by the low-level keyboard hook instead of `RegisterHotKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LowLevelHotkey {
    /// Hotkey ID. Must match an ID known to `HotkeyManager` so the posted
    /// `WM_HOTKEY` message is mapped back to the right action.
    pub id: i32,
    /// `MOD_*` bitmask of the required modifiers (Win/Ctrl/Alt/Shift).
    pub modifiers: u32,
    /// Virtual key code of the main key.
    pub vk: u32,
}

struct HookState {
    hook: Option<HHOOK>,
    hotkeys: Vec<LowLevelHotkey>,
}

// Registry shared between the manager and the static hook procedure. A `WH_KEYBOARD_LL`
// hook is global, but its procedure is invoked only on the thread that installed it
// (the main message-loop thread), so the state lives in a thread-local.
thread_local! {
    static STATE: RefCell<HookState> = RefCell::new(HookState {
        hook: None,
        hotkeys: Vec::new(),
    });
}

/// Owns the global low-level keyboard hook. A single instance is held by
/// [`crate::app::App`]; the underlying hook is global, so state lives in a thread-local.
pub struct LowLevelKeyHook;

impl LowLevelKeyHook {
    pub fn new() -> Self {
        Self
    }

    /// Replaces the set of low-level hotkeys, installing the `WH_KEYBOARD_LL` hook
    /// when the set is non-empty and removing it when empty.
    ///
    /// Must be called from the main (message-loop) thread.
    pub fn set_hotkeys(&self, hotkeys: &[LowLevelHotkey]) {
        STATE.with(|s| {
            let mut state = s.borrow_mut();
            state.hotkeys = hotkeys.to_vec();

            let install = !state.hotkeys.is_empty();
            if install && state.hook.is_none() {
                state.hook = Self::install_hook();
            } else if !install {
                if let Some(h) = state.hook.take() {
                    unsafe {
                        let _ = UnhookWindowsHookEx(h);
                    }
                    tracing::info!("Low-level hotkey hook removed");
                }
            }
        });
    }

    fn install_hook() -> Option<HHOOK> {
        let hmod = unsafe {
            GetModuleHandleW(None)
                .ok()
                .map(|h| windows::Win32::Foundation::HINSTANCE(h.0))
        };
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hmod, 0) } {
            Ok(h) => {
                tracing::info!("Low-level hotkey hook installed");
                Some(h)
            }
            Err(e) => {
                tracing::error!("Failed to install low-level hotkey hook: {}", e);
                None
            }
        }
    }
}

impl Drop for LowLevelKeyHook {
    fn drop(&mut self) {
        STATE.with(|s| {
            let mut state = s.borrow_mut();
            state.hotkeys.clear();
            if let Some(h) = state.hook.take() {
                unsafe {
                    let _ = UnhookWindowsHookEx(h);
                }
            }
        });
    }
}

/// Hook callback: matches key presses against the registered low-level hotkeys,
/// swallows matches (preventing the native Windows behavior) and posts a `WM_HOTKEY`
/// message so `App::handle_hotkey` dispatches the action.
unsafe extern "system" fn hook_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code >= 0 {
        let msg = w_param.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let vk = (*(l_param.0 as *const KBDLLHOOKSTRUCT)).vkCode;
            let matched_id = STATE.with(|s| {
                let state = s.borrow();
                state
                    .hotkeys
                    .iter()
                    .find(|e| e.vk == vk && modifiers_match(e.modifiers))
                    .map(|e| e.id)
            });
            if let Some(id) = matched_id {
                // The hook runs on the installing (main) thread, so posting to the
                // current thread routes through App::dispatch_thread_message.
                let _ = PostThreadMessageW(
                    GetCurrentThreadId(),
                    WM_HOTKEY,
                    WPARAM(id as usize),
                    LPARAM(0),
                );
                return LRESULT(1); // swallow the key: prevent the native shortcut
            }
        }
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}

/// Returns true when exactly the modifiers in `required` (and none of the other three
/// standard modifiers) are currently held down — mirroring `RegisterHotKey` semantics.
unsafe fn modifiers_match(required: u32) -> bool {
    let win = is_key_down(VK_LWIN.0 as u16) || is_key_down(VK_RWIN.0 as u16);
    let ctrl = is_key_down(VK_LCONTROL.0 as u16) || is_key_down(VK_RCONTROL.0 as u16);
    let alt = is_key_down(VK_LMENU.0 as u16) || is_key_down(VK_RMENU.0 as u16);
    let shift = is_key_down(VK_LSHIFT.0 as u16) || is_key_down(VK_RSHIFT.0 as u16);

    (required & MOD_WIN.0 != 0) == win
        && (required & MOD_CONTROL.0 != 0) == ctrl
        && (required & MOD_ALT.0 != 0) == alt
        && (required & MOD_SHIFT.0 != 0) == shift
}

unsafe fn is_key_down(vk: u16) -> bool {
    (GetAsyncKeyState(vk as i32) as u16) & 0x8000 != 0
}
