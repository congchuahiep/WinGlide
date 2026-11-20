//! Hotkey manager: owns the hotkey set, decides how each hotkey is delivered, and maps
//! hotkey IDs to actions.
//!
//! # Delivery
//!
//! Every hotkey is first offered to `RegisterHotKey`, because a registered system hotkey
//! is consumed by the window manager before it reaches the focused window: it works while
//! an elevated window has focus (for combinations that include a modifier) and it cannot
//! leak the key into that window.
//!
//! Combinations that Windows already owns — `Win+1`..`Win+9`, `Win+L`, `Win+E`, ... —
//! fail with `ERROR_HOTKEY_ALREADY_REGISTERED (1409)`. Those fall back to the low-level
//! keyboard hook ([`crate::hotkey::LowLevelKeyHook`]), which intercepts the key before
//! the shell and posts `WM_HOTKEY` itself. The hook is therefore still the mechanism for
//! overriding Windows shortcuts; it is no longer the mechanism for every Win combination.
//!
//! A hook installed by a non-elevated process does not receive input destined for an
//! elevated window (UIPI), so hook-dispatched hotkeys are best-effort while an elevated
//! window has focus. Registered hotkeys with modifiers do not have that limitation.
//!
//! Both mechanisms post `WM_HOTKEY` with the same IDs, so
//! [`HotkeyManager::action_from_id`] works uniformly.

use tracing::{info, warn};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT,
    MOD_WIN,
};

use super::low_level_hook::LowLevelHotkey;

/// `RegisterHotKey` returns this error when Windows or another application already owns
/// the combination.
const ERROR_HOTKEY_ALREADY_REGISTERED: i32 = 1409;

/// Checks for `ERROR_HOTKEY_ALREADY_REGISTERED` in a Win32 error HRESULT.
fn is_already_registered(code: i32) -> bool {
    (code & 0xFFFF) == ERROR_HOTKEY_ALREADY_REGISTERED
}

/// How a hotkey is delivered to the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyDispatch {
    /// Registered with `RegisterHotKey`; Windows posts `WM_HOTKEY` when pressed.
    Registered,
    /// Intercepted by the low-level keyboard hook; we post `WM_HOTKEY` ourselves.
    LowLevelHook,
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
    fn has_win_modifier(&self) -> bool {
        self.modifiers.0 & MOD_WIN.0 != 0
    }

    /// Human-readable combination, for startup/reload logs.
    fn combo(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.0 & MOD_WIN.0 != 0 {
            parts.push("Win");
        }
        if self.modifiers.0 & MOD_CONTROL.0 != 0 {
            parts.push("Ctrl");
        }
        if self.modifiers.0 & MOD_ALT.0 != 0 {
            parts.push("Alt");
        }
        if self.modifiers.0 & MOD_SHIFT.0 != 0 {
            parts.push("Shift");
        }

        let key = format!("0x{:02X}", self.vk);
        if parts.is_empty() {
            key
        } else {
            format!("{}+{}", parts.join("+"), key)
        }
    }

    /// Tries to claim the combination with [`RegisterHotKey`].
    ///
    /// On success the system owns the combination and posts `WM_HOTKEY`, which also
    /// means the key never reaches the focused window. If the combination has a Win
    /// modifier and registration fails, Windows (or another application) already owns
    /// it and the hotkey falls back to the low-level hook. A non-Win failure is a real
    /// configuration conflict and is returned as an error.
    fn register(&mut self) -> anyhow::Result<()> {
        match unsafe { RegisterHotKey(None, self.id, self.modifiers, self.vk) } {
            Ok(()) => {
                self.dispatch = HotkeyDispatch::Registered;
                info!(id = self.id, combo = %self.combo(), "hotkey registered");
                Ok(())
            }
            Err(error) if self.has_win_modifier() => {
                let code = error.code().0;
                self.dispatch = HotkeyDispatch::LowLevelHook;
                if is_already_registered(code) {
                    warn!(
                        id = self.id,
                        combo = %self.combo(),
                        "hotkey is already owned; using the low-level hook (unavailable while an elevated window has focus)"
                    );
                } else {
                    warn!(
                        id = self.id,
                        combo = %self.combo(),
                        error = %error,
                        "RegisterHotKey failed; using the low-level hook"
                    );
                }
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
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

/// Builds the configured hotkey set.
///
/// Dispatch starts as `Registered` for every hotkey; `Hotkey::register()` decides
/// whether it can stay there or must fall back to the low-level hook.
fn build_hotkeys(config: &crate::app_setting::AppConfig) -> Vec<Hotkey> {
    let mut hotkeys = Vec::new();

    if config.cycle_taskbar_based {
        hotkeys.push(Hotkey {
            id: 1,
            action: HotkeyAction::CycleLeft,
            modifiers: HOT_KEY_MODIFIERS(config.hotkey_left_modifiers),
            vk: config.hotkey_left_vk,
            dispatch: HotkeyDispatch::Registered,
        });
        hotkeys.push(Hotkey {
            id: 2,
            action: HotkeyAction::CycleRight,
            modifiers: HOT_KEY_MODIFIERS(config.hotkey_right_modifiers),
            vk: config.hotkey_right_vk,
            dispatch: HotkeyDispatch::Registered,
        });
    }

    if config.jump_desktop_modifiers != 0 {
        for i in 1..=9 {
            hotkeys.push(Hotkey {
                id: 10 + i as i32,
                action: HotkeyAction::SwitchVirtualDesktop(i as u32 - 1),
                modifiers: HOT_KEY_MODIFIERS(config.jump_desktop_modifiers),
                vk: 0x30 + i as u32,
                dispatch: HotkeyDispatch::Registered,
            });
        }
    }

    hotkeys
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
    pub fn new(config: &crate::app_setting::AppConfig) -> anyhow::Result<Self> {
        let mut this = Self {
            hotkeys: build_hotkeys(config),
        };
        this.register_all(true)?;
        Ok(this)
    }

    /// Registers every hotkey and records which mechanism each one uses.
    ///
    /// `strict` controls whether a failure is fatal. Startup is strict; reload keeps the
    /// existing behaviour of logging a warning and continuing with the hotkeys that did
    /// register.
    fn register_all(&mut self, strict: bool) -> anyhow::Result<()> {
        let mut errors = Vec::new();

        for hotkey in &mut self.hotkeys {
            if let Err(error) = hotkey.register() {
                errors.push(format!("ID {} ({}): {error}", hotkey.id, hotkey.combo()));
            }
        }

        if errors.is_empty() {
            return Ok(());
        }

        if strict {
            anyhow::bail!("Failed to register hotkeys: {}", errors.join("; "));
        }

        warn!("Failed to reload some hotkeys: {:?}", errors);
        Ok(())
    }

    /// Unregisters all hotkeys established with Windows.
    ///
    /// This method is called automatically when the `HotkeyManager` object is dropped ([`Drop`]).
    pub fn unregister_all(&self) {
        for hotkey in &self.hotkeys {
            hotkey.unregister();
        }
    }

    /// Returns the hotkeys that `RegisterHotKey` could not claim and that are therefore
    /// intercepted by the low-level keyboard hook.
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
    pub fn reload(&mut self, config: &crate::app_setting::AppConfig) -> anyhow::Result<()> {
        self.unregister_all();
        self.hotkeys = build_hotkeys(config);
        self.register_all(false)
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.unregister_all();
    }
}

#[cfg(test)]
mod tests {
    use super::is_already_registered;

    #[test]
    fn recognises_the_already_registered_code() {
        // HRESULT_FROM_WIN32(ERROR_HOTKEY_ALREADY_REGISTERED)
        assert!(is_already_registered(0x8007_0581u32 as i32));
        assert!(!is_already_registered(0x8007_0005u32 as i32));
    }
}
