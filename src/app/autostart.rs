//! Starts WinGlide when the user logs in.
//!
//! Two mechanisms, picked by whether the process runs inside an MSIX package
//! ([`crate::shared::package::is_packaged`]):
//!
//! * **Packaged** - the `StartupTask` declared in `packaging/AppxManifest.xml`.
//!   Windows stores the state and lists the app in Task Manager's Startup tab.
//!   The Run key is not an option there: a packaged process writes to a private
//!   registry hive that the shell never reads at login.
//! * **Unpackaged** (MSI, ZIP) - the per-user
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` key, as before.

use crate::shared::package::is_packaged;

/// Checks whether WinGlide currently starts with Windows.
pub fn is_autostart_enabled() -> bool {
    if is_packaged() {
        packaged::is_enabled()
    } else {
        registry::is_enabled()
    }
}

/// Turns starting with Windows on or off, using whichever mechanism applies.
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    if is_packaged() {
        packaged::set_enabled(enabled)
    } else {
        registry::set_enabled(enabled)
    }
}

/// Package identity: the startup task is declared in the manifest, Windows owns
/// its state, and the user can override it from Task Manager.
mod packaged {
    use windows::core::HSTRING;
    use windows::ApplicationModel::{StartupTask, StartupTaskState};

    /// Must match `uap5:StartupTask@TaskId` in `packaging/AppxManifest.xml`.
    const TASK_ID: &str = "WinGlideStartup";

    fn task() -> anyhow::Result<StartupTask> {
        Ok(StartupTask::GetAsync(&HSTRING::from(TASK_ID))?.get()?)
    }

    pub fn is_enabled() -> bool {
        task()
            .and_then(|task| Ok(task.State()?))
            .map(|state| {
                matches!(
                    state,
                    StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy
                )
            })
            .unwrap_or(false)
    }

    pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
        let task = task()?;
        if !enabled {
            return Ok(task.Disable()?);
        }

        // The user can veto this from Task Manager; Windows then answers
        // `DisabledByUser` and ignores later requests, so report the refusal
        // instead of leaving the toggle on.
        match task.RequestEnableAsync()?.get()? {
            StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy => Ok(()),
            state => anyhow::bail!("Windows declined to enable the startup task: {state:?}"),
        }
    }
}

/// No package identity: the classic per-user Run key.
mod registry {
    use std::env;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_ALL_ACCESS, KEY_READ, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
        RRF_RT_REG_SZ,
    };

    const APP_NAME: PCWSTR = w!("WinGlide");
    const REG_RUN_PATH: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

    pub fn is_enabled() -> bool {
        unsafe {
            let mut hkey = HKEY::default();
            let status = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                REG_RUN_PATH,
                Some(0),
                REG_SAM_FLAGS(KEY_READ.0),
                &mut hkey,
            );

            if status.is_err() {
                return false;
            }

            let mut data_type = REG_VALUE_TYPE(0);
            let mut data_size: u32 = 0;

            let res = RegGetValueW(
                hkey,
                PCWSTR::null(),
                APP_NAME,
                RRF_RT_REG_SZ,
                Some(&mut data_type as *mut REG_VALUE_TYPE),
                None,
                Some(&mut data_size as *mut u32),
            );

            let _ = RegCloseKey(hkey);
            res.is_ok()
        }
    }

    pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
        unsafe {
            let mut hkey = HKEY::default();
            let status = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                REG_RUN_PATH,
                Some(0),
                REG_SAM_FLAGS(KEY_ALL_ACCESS.0),
                &mut hkey,
            );
            status.ok()?;

            if enabled {
                let exe_path = env::current_exe()?;
                let exe_path_u16: Vec<u16> = exe_path.into_os_string().encode_wide().collect();
                // Wrap the path in quotes in case there are spaces
                let mut val_u16 = vec![b'"' as u16];
                val_u16.extend(exe_path_u16);
                val_u16.push(b'"' as u16);
                val_u16.push(0);

                let size = (val_u16.len() * 2) as u32;

                RegSetValueExW(
                    hkey,
                    APP_NAME,
                    Some(0),
                    REG_SZ,
                    Some(std::slice::from_raw_parts(
                        val_u16.as_ptr() as *const u8,
                        size as usize,
                    )),
                )
                .ok()?;
            } else {
                let _ = RegDeleteValueW(hkey, APP_NAME);
            }

            let _ = RegCloseKey(hkey);
            Ok(())
        }
    }
}
