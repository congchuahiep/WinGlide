//! Whether this process runs inside an MSIX package.
//!
//! One question, one answer, but two features depend on it: autostart goes
//! through the Windows `StartupTask` API once the app is packaged instead of the
//! registry, and the Microsoft Store owns updates. `GetCurrentPackageFullName`
//! reports `APPMODEL_ERROR_NO_PACKAGE` only when the process has no package
//! identity, which is the case for the MSI and ZIP builds.

use windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

/// True when the running process belongs to an installed MSIX package.
pub fn is_packaged() -> bool {
    let mut length = 0u32;
    let status = unsafe { GetCurrentPackageFullName(&mut length, None) };
    status != APPMODEL_ERROR_NO_PACKAGE
}

#[cfg(test)]
mod tests {
    use super::is_packaged;

    /// The test harness runs unpackaged. If this ever flips, the MSI and ZIP builds
    /// would silently lose autostart: they would ask for a `StartupTask` that does
    /// not exist and report the toggle as broken.
    #[test]
    fn test_binary_has_no_package_identity() {
        assert!(!is_packaged());
    }
}
