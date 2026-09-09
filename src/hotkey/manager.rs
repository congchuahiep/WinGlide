//! Hotkey manager: owns the hotkey set, classifies each hotkey's delivery mechanism,
//! and maps hotkey IDs to actions.
//!
//! By default, the application registers two hotkeys:
//! - **Alt + [**: Move focus to the left Taskbar button ([`HotkeyAction::CycleLeft`])
//! - **Alt + ]**: Move focus to the right Taskbar button ([`HotkeyAction::CycleRight`])
//!
//! Hotkeys whose combination involves the **Win** modifier cannot be claimed with
//! `RegisterHotKey` (Windows owns `Win+<number>` taskbar shortcuts and reserved combos
//! like `Win+L`), so they are dispatched through the low-level keyboard hook instead
//! (see [`crate::hotkey::LowLevelKeyHook`]). Both mechanisms post `WM_HOTKEY` with the
//! same IDs, so [`HotkeyManager::action_from_id`] works uniformly.

use super::low_level_hook::LowLevelHotkey;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_WIN,
};

/// How a hotkey is delivered to the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyDispatch {
    /// Registered with `RegisterHotKey`; Windows posts `WM_HOTKEY` when pressed.
    Registered,
    /// Intercepted by the low-level keyboard hook; we post `WM_HOTKEY` ourselves.
    LowLevelHook,
}

/// Chooses the dispatch mechanism for a hotkey. Any combination involving the Win
/// modifier is routed through the low-level hook, because such combinations are often
/// owned by Windows (native taskbar shortcuts, reserved combos) and would fail to
/// register — and because the hook also suppresses the native behavior.
fn dispatch_for(modifiers: u32) -> HotkeyDispatch {
    if modifiers & MOD_WIN.0 != 0 {
        HotkeyDispatch::LowLevelHook
    } else {
        HotkeyDispatch::Registered
    }
}

/// Actions that can be triggered by global hotkeys.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Cycle focus to the left window on the Taskbar.
    CycleLeft,
    /// Cycle focus to the right window on the Taskbar.
    CycleRight,
    /// Switch to the Virtual Desktop with the specified index (0-based).
    SwitchVirtualDesktop(u32),
}

/// Stores the details and state of a specific hotkey.
struct Hotkey {
    /// Unique identifier (ID) of the hotkey within the application scope.
    id: i32,
    /// The action to be executed when this hotkey is pressed.
    action: HotkeyAction,
    /// Accompanying modifier keys (such as Alt, Ctrl, Shift).
    modifiers: HOT_KEY_MODIFIERS,
    /// Virtual Key Code of the main key.
    vk: u32,
    /// How this hotkey is delivered to the application.
    dispatch: HotkeyDispatch,
}

impl Hotkey {
    /// Registers this hotkey with the Windows system.
    ///
    /// # Errors
    /// Returns an error if the hotkey is already in use by another application.
    fn register(&self) -> windows::core::Result<()> {
        if self.dispatch != HotkeyDispatch::Registered {
            return Ok(());
        }
        unsafe { RegisterHotKey(None, self.id, self.modifiers, self.vk) }
    }

    /// Unregisters this hotkey from the Windows system.
    fn unregister(&self) {
        if self.dispatch != HotkeyDispatch::Registered {
            return;
        }
        unsafe {
            let _ = UnregisterHotKey(None, self.id);
        }
    }
}

/// Manager for the application's global hotkeys.
pub struct HotkeyManager {
    /// List of managed hotkey instances.
    hotkeys: Vec<Hotkey>,
}

impl HotkeyManager {
    /// Initializes the manager and registers the default hotkeys with the system.
    ///
    /// Defaults:
    /// - ID 1: `Alt+[` -> Cycle left ([`HotkeyAction::CycleLeft`]).
    /// - ID 2: `Alt+]` -> Cycle right ([`HotkeyAction::CycleRight`]).
    /// - ID 11-19: `Alt+1` -> `Alt+9` -> Switch to respective VD ([`HotkeyAction::SwitchVirtualDesktop`]).
    ///
    /// TODO: Allow users to customize hotkeys.
    ///
    /// # Errors
    /// Returns an error if it fails to register one or more hotkeys (usually due to a conflict with
    /// another software).
    pub fn new(config: &crate::config::AppConfig) -> anyhow::Result<Self> {
        let mut hotkeys = vec![];

        if config.cycle_taskbar_based {
            hotkeys.push(Hotkey {
                id: 1,
                action: HotkeyAction::CycleLeft,
                modifiers: HOT_KEY_MODIFIERS(config.hotkey_left_modifiers),
                vk: config.hotkey_left_vk,
                dispatch: dispatch_for(config.hotkey_left_modifiers),
            });
            hotkeys.push(Hotkey {
                id: 2,
                action: HotkeyAction::CycleRight,
                modifiers: HOT_KEY_MODIFIERS(config.hotkey_right_modifiers),
                vk: config.hotkey_right_vk,
                dispatch: dispatch_for(config.hotkey_right_modifiers),
            });
        }

        // Jump-to-desktop keys. When the Win modifier is involved, `Win+1..Win+9`
        // are owned by Windows (native taskbar shortcuts) and cannot be registered
        // with RegisterHotKey; the low-level keyboard hook overrides them instead.
        // The entries are always kept so action_from_id maps the IDs the hook posts.
        let dispatch = dispatch_for(config.jump_desktop_modifiers);
        if config.jump_desktop_modifiers != 0 {
            for i in 1..=9 {
                hotkeys.push(Hotkey {
                    id: 10 + i as i32,
                    action: HotkeyAction::SwitchVirtualDesktop(i as u32 - 1),
                    modifiers: HOT_KEY_MODIFIERS(config.jump_desktop_modifiers),
                    vk: 0x30 + i as u32,
                    dispatch,
                });
            }
        }

        let this = Self { hotkeys };

        let mut errs = Vec::new();
        for hotkey in &this.hotkeys {
            if let Err(e) = hotkey.register() {
                errs.push(e);
            }
        }

        if !errs.is_empty() {
            anyhow::bail!("Failed to register hotkeys: {:?}", errs);
        }

        Ok(this)
    }

    /// Unregisters all hotkeys established with Windows.
    ///
    /// This method is called automatically when the `HotkeyManager` object is dropped ([`Drop`]).
    pub fn unregister_all(&self) {
        for hotkey in &self.hotkeys {
            hotkey.unregister();
        }
    }

    /// Returns the hotkeys that must be intercepted by the low-level keyboard hook
    /// (Win-involved combinations that `RegisterHotKey` cannot claim).
    pub fn low_level_hotkeys(&self) -> Vec<LowLevelHotkey> {
        self.hotkeys
            .iter()
            .filter(|h| h.dispatch == HotkeyDispatch::LowLevelHook)
            .map(|h| LowLevelHotkey {
                id: h.id,
                modifiers: h.modifiers.0,
                vk: h.vk,
            })
            .collect()
    }

    /// Looks up the action corresponding to the hotkey ID received from the system message.
    pub fn action_from_id(&self, id: i32) -> Option<HotkeyAction> {
        self.hotkeys.iter().find(|h| h.id == id).map(|h| h.action)
    }

    /// Reloads the hotkey configuration: unregisters old ones, loads new ones, and registers them again.
    pub fn reload(&mut self, config: &crate::config::AppConfig) -> anyhow::Result<()> {
        for hotkey in &self.hotkeys {
            hotkey.unregister();
        }

        self.hotkeys.clear();

        if config.cycle_taskbar_based {
            self.hotkeys.push(Hotkey {
                id: 1,
                action: HotkeyAction::CycleLeft,
                modifiers: HOT_KEY_MODIFIERS(config.hotkey_left_modifiers),
                vk: config.hotkey_left_vk,
                dispatch: dispatch_for(config.hotkey_left_modifiers),
            });

            self.hotkeys.push(Hotkey {
                id: 2,
                action: HotkeyAction::CycleRight,
                modifiers: HOT_KEY_MODIFIERS(config.hotkey_right_modifiers),
                vk: config.hotkey_right_vk,
                dispatch: dispatch_for(config.hotkey_right_modifiers),
            });
        }

        let dispatch = dispatch_for(config.jump_desktop_modifiers);
        if config.jump_desktop_modifiers != 0 {
            for i in 1..=9 {
                self.hotkeys.push(Hotkey {
                    id: 10 + i as i32,
                    action: HotkeyAction::SwitchVirtualDesktop(i as u32 - 1),
                    modifiers: HOT_KEY_MODIFIERS(config.jump_desktop_modifiers),
                    vk: 0x30 + i as u32,
                    dispatch,
                });
            }
        }

        let mut errs = Vec::new();
        for hotkey in &self.hotkeys {
            if let Err(e) = hotkey.register() {
                errs.push(format!("ID {}: {}", hotkey.id, e));
            }
        }

        if !errs.is_empty() {
            tracing::warn!("Failed to reload some hotkeys: {:?}", errs);
        }

        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.unregister_all();
    }
}
