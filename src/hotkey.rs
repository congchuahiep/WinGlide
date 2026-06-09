//! Quản lý phím nóng toàn cục (Global Hotkeys) bằng API Win32.
//!
//! Module chịu trách nhiệm đăng ký, hủy đăng ký và ánh xạ các phím nóng toàn cục.
//!
//! Khác với RegisterHotKey, ứng dụng sử dụng Low-Level Keyboard Hook (WH_KEYBOARD_LL)
//! để có thể đè lên các phím tắt hệ thống như Win + Left/Right.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, VK_LMENU, VK_LWIN, VK_RMENU,
    VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK,
    KBDLLHOOKSTRUCT, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_SYSKEYDOWN,
};

/// Các hành động có thể kích hoạt bởi phím nóng toàn cục
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Di chuyển tiêu điểm sang cửa sổ bên trái trên Taskbar.
    CycleLeft,
    /// Di chuyển tiêu điểm sang cửa sổ bên phải trên Taskbar.
    CycleRight,
    /// Chuyển đổi sang Virtual Desktop với index (0-based) chỉ định.
    SwitchVirtualDesktop(u32),
}

/// Lưu trữ thông tin chi tiết và trạng thái của một phím nóng cụ thể.
struct Hotkey {
    /// Mã định danh duy nhất (ID) của phím nóng trong phạm vi ứng dụng.
    id: i32,
    /// Hành động sẽ được thực thi khi phím nóng này được nhấn.
    action: HotkeyAction,
    /// Các phím bổ trợ đi kèm (như phím Alt, Ctrl, Shift).
    modifiers: u32,
    /// Mã phím ảo (Virtual Key Code) của phím chính.
    vk: u32,
}

static HOTKEYS: Mutex<Vec<Hotkey>> = Mutex::new(Vec::new());
static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);
// Wrap HHOOK inside a thread-safe struct because HHOOK is a primitive pointer wrapper which isn't Send/Sync
struct HookWrapper(HHOOK);
unsafe impl Send for HookWrapper {}
unsafe impl Sync for HookWrapper {}
static HOOK_HANDLE: Mutex<Option<HookWrapper>> = Mutex::new(None);

/// Trình quản lý danh sách các phím nóng toàn cục của ứng dụng.
pub struct HotkeyManager;

impl HotkeyManager {
    /// Khởi tạo trình quản lý và đăng ký các phím nóng mặc định với hệ thống.
    pub fn new(config: &crate::config::AppConfig) -> anyhow::Result<Self> {
        unsafe {
            MAIN_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);
        }

        let mut manager = Self;
        manager.reload(config)?;

        let mut hook_guard = HOOK_HANDLE.lock().unwrap();
        if hook_guard.is_none() {
            unsafe {
                let hmod = GetModuleHandleW(None)?;
                let hook =
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(hmod.into()), 0)?;
                *hook_guard = Some(HookWrapper(hook));
            }
        }

        Ok(manager)
    }

    /// Hủy đăng ký toàn bộ các phím nóng.
    #[allow(dead_code)]
    pub fn unregister_all(&self) {
        HOTKEYS.lock().unwrap().clear();
    }

    /// Tìm kiếm hành động tương ứng với ID phím nóng nhận được từ tin nhắn hệ thống
    pub fn action_from_id(&self, id: i32) -> Option<HotkeyAction> {
        HOTKEYS
            .lock()
            .unwrap()
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.action)
    }

    /// Tải lại cấu hình phím tắt
    pub fn reload(&mut self, config: &crate::config::AppConfig) -> anyhow::Result<()> {
        let mut hotkeys = Vec::new();

        if config.cycle_taskbar_based {
            hotkeys.push(Hotkey {
                id: 1,
                action: HotkeyAction::CycleLeft,
                modifiers: config.hotkey_left_modifiers,
                vk: config.hotkey_left_vk,
            });

            hotkeys.push(Hotkey {
                id: 2,
                action: HotkeyAction::CycleRight,
                modifiers: config.hotkey_right_modifiers,
                vk: config.hotkey_right_vk,
            });
        }

        if config.jump_desktop_modifiers != 0 {
            for i in 1..=9 {
                hotkeys.push(Hotkey {
                    id: 10 + i as i32,
                    action: HotkeyAction::SwitchVirtualDesktop(i as u32 - 1),
                    modifiers: config.jump_desktop_modifiers,
                    vk: 0x30 + i as u32,
                });
            }
        }

        *HOTKEYS.lock().unwrap() = hotkeys;

        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        let mut hook_guard = HOOK_HANDLE.lock().unwrap();
        if let Some(hook) = hook_guard.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(hook.0);
            }
        }
    }
}

unsafe extern "system" fn hook_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code >= 0 {
        let msg = w_param.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let kbd = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
            let vk = kbd.vkCode;

            // Lấy trạng thái của các phím modifier
            let mut current_mods = 0;
            if (GetAsyncKeyState(0x11 /* VK_CONTROL */) as u16 & 0x8000) != 0 {
                current_mods |= MOD_CONTROL.0 as u32;
            }
            if (GetAsyncKeyState(0x12 /* VK_MENU */) as u16 & 0x8000) != 0
                || (GetAsyncKeyState(VK_LMENU.0 as _) as u16 & 0x8000) != 0
                || (GetAsyncKeyState(VK_RMENU.0 as _) as u16 & 0x8000) != 0
            {
                current_mods |= MOD_ALT.0 as u32;
            }
            if (GetAsyncKeyState(0x10 /* VK_SHIFT */) as u16 & 0x8000) != 0 {
                current_mods |= MOD_SHIFT.0 as u32;
            }
            if (GetAsyncKeyState(VK_LWIN.0 as _) as u16 & 0x8000) != 0
                || (GetAsyncKeyState(VK_RWIN.0 as _) as u16 & 0x8000) != 0
            {
                current_mods |= MOD_WIN.0 as u32;
            }

            // Nếu phím bấm hiện tại là phím modifier thì không làm gì
            let is_modifier = matches!(
                vk,
                16 | 160 | 161 | 17 | 162 | 163 | 18 | 164 | 165 | 91 | 92
            );

            if !is_modifier {
                // Duyệt qua danh sách hotkey xem có khớp không
                // Tránh panic trong hook callback nếu mutex bị poison
                if let Ok(hotkeys) = HOTKEYS.lock() {
                    for hotkey in hotkeys.iter() {
                        if hotkey.vk == vk && hotkey.modifiers == current_mods {
                            // Gửi message WM_HOTKEY đến main thread
                            let thread_id = MAIN_THREAD_ID.load(Ordering::SeqCst);
                            let _ = PostThreadMessageW(
                                thread_id,
                                WM_HOTKEY,
                                WPARAM(hotkey.id as usize),
                                LPARAM(0),
                            );
                            return LRESULT(1); // Chặn phím
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}
