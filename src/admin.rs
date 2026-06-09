use std::env;
use std::os::windows::ffi::OsStrExt;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;

pub fn is_running_as_admin() -> bool {
    unsafe {
        let mut token: HANDLE = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;

        let res = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            size,
            &mut size,
        );

        let _ = windows::Win32::Foundation::CloseHandle(token);

        if res.is_ok() {
            elevation.TokenIsElevated != 0
        } else {
            false
        }
    }
}

pub fn restart_as_admin(reopen_ui: bool) -> anyhow::Result<()> {
    unsafe {
        let exe_path = env::current_exe()?;
        let mut exe_path_u16: Vec<u16> = exe_path.into_os_string().encode_wide().collect();
        exe_path_u16.push(0);

        let mut args_u16: Vec<u16> = if reopen_ui {
            w!("--reopen-ui").as_wide().to_vec()
        } else {
            vec![0]
        };
        args_u16.push(0);

        // Run as admin
        let res = ShellExecuteW(
            None,
            w!("runas"),
            PCWSTR(exe_path_u16.as_ptr()),
            if reopen_ui { PCWSTR(args_u16.as_ptr()) } else { PCWSTR::null() },
            PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOW,
        );

        if res.0 as isize > 32 {
            std::process::exit(0);
        } else {
            anyhow::bail!("Failed to restart as administrator. Error code: {}", res.0 as isize);
        }
    }
}
