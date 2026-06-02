use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, VK_OEM_4, VK_OEM_6,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    Left,
    Right,
}

struct Hotkey {
    id: i32,
    action: HotkeyAction,
    modifiers: HOT_KEY_MODIFIERS,
    vk: u32,
}

impl Hotkey {
    fn register(&self) -> windows::core::Result<()> {
        unsafe { RegisterHotKey(None, self.id, self.modifiers, self.vk) }
    }
    fn unregister(&self) {
        unsafe {
            let _ = UnregisterHotKey(None, self.id);
        }
    }
}

pub struct HotkeyManager {
    hotkeys: Vec<Hotkey>,
}

impl HotkeyManager {
    pub fn new() -> anyhow::Result<Self> {
        let this = Self {
            hotkeys: vec![
                Hotkey {
                    id: 1,
                    action: HotkeyAction::Left,
                    modifiers: MOD_ALT,
                    vk: VK_OEM_4.0 as u32,
                },
                Hotkey {
                    id: 2,
                    action: HotkeyAction::Right,
                    modifiers: MOD_ALT,
                    vk: VK_OEM_6.0 as u32,
                },
            ],
        };

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

    pub fn unregister_all(&self) {
        for hotkey in &self.hotkeys {
            hotkey.unregister();
        }
    }

    pub fn action_from_id(&self, id: i32) -> Option<HotkeyAction> {
        self.hotkeys.iter().find(|h| h.id == id).map(|h| h.action)
    }
}
