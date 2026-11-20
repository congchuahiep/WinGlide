//! Process-integrity queries used to avoid injecting input into elevated windows.
//!
//! Windows blocks a medium-integrity process from injecting keyboard or mouse input
//! into a high-integrity (elevated) window. The activation ladder therefore checks the
//! foreground's integrity before trying the ALT tap, so that a fallback step can never
//! leak a keystroke into an elevated terminal.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

/// Returns `Some(true)` when the process is elevated, `Some(false)` when it is not, and
/// `None` when its token cannot be queried (protected process, invalid PID, access denied).
pub fn is_process_elevated(pid: u32) -> Option<bool> {
    if pid == 0 {
        return None;
    }

    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut token = HANDLE::default();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
            let _ = CloseHandle(process);
            return None;
        }

        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            size,
            &mut size,
        );

        let _ = CloseHandle(token);
        let _ = CloseHandle(process);

        result.ok().map(|_| elevation.TokenIsElevated != 0)
    }
}

/// Elevation of the process that owns `hwnd`.
pub fn is_window_elevated(hwnd: HWND) -> Option<bool> {
    if hwnd.0.is_null() {
        return None;
    }

    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    is_process_elevated(pid)
}