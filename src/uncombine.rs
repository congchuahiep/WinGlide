//! Quản lý việc tách (uncombine) các taskbar button.
//!
//! # Bối cảnh
//!
//! Trên Windows, taskbar mặc định gộp (`combine`) nhiều cửa sổ cùng app vào
//! một button duy nhất. Cơ chế này dựa trên **AppUserModelID (AUMID)**:
//! các cửa sổ có cùng AUMID được Windows gộp vào chung một group.
//!
//! `UncombineManager` gán cho mỗi cửa sổ một AUMID **duy nhất** dạng
//! `TaskbarSwitcher_<HWND>`, khiến Windows coi mỗi cửa sổ là một app
//! riêng biệt → mỗi cửa sổ có taskbar button riêng.
//!
//! AUMID gốc của mỗi cửa sổ được lưu lại để có thể **khôi phục** khi
//! thoát app (tránh làm hỏng trạng thái hệ thống).
//!
//! # Luồng hoạt động
//!
//! ```text
//! 1. App start → uncombine_all() duyệt tất cả window, set AUMID riêng
//! 2. WinEvent hook → phát hiện window mới → uncombine_one(hwnd)
//! 3. App exit (Ctrl+C) → restore_all() khôi phục AUMID gốc
//! ```

use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{debug, error, instrument};
use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::InitPropVariantFromStringAsVector;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};

use crate::utils::truncate;

/// Quản lý uncombine: lưu AUMID gốc và set AUMID riêng cho từng cửa sổ.
///
/// # Thread safety
///
/// `original_aumids` được bảo vệ bởi `Mutex` vì:
/// - Main thread gọi `uncombine_all()` / `uncombine_one()` / `restore_all()`
/// - WinEvent callback thread gọi `is_tracked()` để filter trước khi post message
pub struct UncombineManager {
    /// `hwnd_val (isize) → AUMID gốc`.
    ///
    /// - `Some("App.Aumid")` — cửa sổ có AUMID gốc
    /// - `None` — cửa sổ vốn không có AUMID (VD: console window)
    original_aumids: Mutex<HashMap<isize, Option<String>>>,
}

impl UncombineManager {
    /// Tạo instance mới với map rỗng.
    pub fn new() -> Self {
        Self {
            original_aumids: Mutex::new(HashMap::new()),
        }
    }

    /// Kiểm tra cửa sổ đã được theo dõi bởi uncombine chưa.
    ///
    /// Dùng tromg WinEvent callback để filter nhanh trước khi post message
    /// — tránh gửi message vô ích cho cửa sổ đã uncombine.
    ///
    /// # Ví dụ
    ///
    /// ```rust,ignore
    /// if uncombine.is_tracked(hwnd) {
    ///     return; // đã uncombine rồi, bỏ qua
    /// }
    /// ```
    pub fn is_tracked(&self, hwnd: HWND) -> bool {
        self.original_aumids
            .lock()
            .unwrap()
            .contains_key(&(hwnd.0 as isize))
    }

    /// Uncombine **tất cả** cửa sổ đang visible trên desktop.
    ///
    /// Duyệt `find_visible_windows()`, skip window đã track,
    /// gán AUMID mới `TaskbarSwitcher_<HWND>` cho từng cửa sổ.
    ///
    /// Gọi **một lần** khi app start.
    ///
    /// # Luồng
    ///
    /// ```text
    /// 1. find_visible_windows() → tất cả cửa sổ có title, visible
    /// 2. Với mỗi cửa sổ:
    ///    a. Kiểm tra đã track chưa → skip
    ///    b. Lấy AUMID gốc → lưu vào map
    ///    c. Set AUMID mới = "TaskbarSwitcher_<hwnd_val>"
    /// ```
    ///
    /// # Ví dụ
    ///
    /// ```text
    /// Chrome window A (hwnd=1234, aumid="Chrome.App")
    ///   → map[1234] = Some("Chrome.App")
    ///   → set aumid = "TaskbarSwitcher_1234"
    ///
    /// Console window (hwnd=5678, no aumid)
    ///   → map[5678] = None
    ///   → set aumid = "TaskbarSwitcher_5678"
    /// ```
    #[instrument(level = "debug", skip_all)]
    pub fn uncombine_all(&self) {
        let windows = crate::switcher::find_visible_windows();
        let mut map = self.original_aumids.lock().unwrap();

        for w in &windows {
            let hwnd_val = w.hwnd.0 as isize;

            if map.contains_key(&hwnd_val) {
                continue;
            }

            let original = get_aumid(w.hwnd);
            let new_aumid = format!("TaskbarSwitcher_{}", hwnd_val);
            map.insert(hwnd_val, original.clone());

            match set_aumid(w.hwnd, &new_aumid) {
                Ok(()) => debug!("'{}' has been uncombined", truncate(&w.title, 30),),
                Err(e) => error!(
                    "Failed to set AUMID for {}: {:?}",
                    truncate(&w.title, 30),
                    e,
                ),
            }
        }
    }

    /// Uncombine **một** cửa sổ mới xuất hiện.
    ///
    /// Được gọi từ WinEvent callback (qua WM_APP_UNCOMBINE message).
    /// Nếu cửa sổ đã được track → bỏ qua.
    ///
    /// # Tham số
    ///
    /// * `hwnd` — HWND của cửa sổ mới cần uncombine
    ///
    /// # Ví dụ
    ///
    /// ```rust,ignore
    /// // Trong message loop:
    /// winevent::WM_APP_UNCOMBINE => {
    ///     let hwnd = HWND(msg.wParam.0 as *mut _);
    ///     uncombine.uncombine_one(hwnd);
    /// }
    /// ```
    #[instrument(level = "debug", skip_all)]
    pub fn uncombine_one(&self, hwnd: HWND, on_success: impl FnOnce()) {
        let hwnd_val = hwnd.0 as isize;
        let mut map = self.original_aumids.lock().unwrap();

        if map.contains_key(&hwnd_val) {
            return;
        }

        let original = get_aumid(hwnd);
        let new_aumid = format!("TaskbarSwitcher_{}", hwnd_val);
        map.insert(hwnd_val, original.clone());

        match set_aumid(hwnd, &new_aumid) {
            Ok(()) => {
                debug!("New window {:?} has been uncombined", hwnd);
                on_success();
            }
            Err(e) => error!("Failed to set AUMID for {:?}: {:?}", hwnd, e),
        }
    }

    /// Khôi phục **tất cả** AUMID gốc.
    ///
    /// Duyệt toàn bộ map, với mỗi cửa sổ:
    /// - Nếu có AUMID gốc: set lại AUMID gốc
    /// - Nếu `None` (vốn không có AUMID): bỏ qua
    ///
    /// Gọi **khi thoát app** (Ctrl+C hoặc kết thúc bình thường).
    /// Sau khi gọi hàm này, các cửa sổ sẽ được Windows group lại bình thường.
    ///
    /// # Ví dụ
    ///
    /// ```json
    /// map: { 1234: Some("Chrome.App"), 5678: None }
    ///
    /// - Chrome (1234): set aumid về "Chrome.App"  ← khôi phục
    /// - Console (5678): bỏ qua                     ← không cần làm gì
    /// ```
    #[instrument(level = "debug", skip_all)]
    pub fn restore_all(&self) {
        debug!("Restoring original AppUserModelIDs");

        let map = self.original_aumids.lock().unwrap();

        for (&hwnd_val, original) in map.iter() {
            let hwnd = HWND(hwnd_val as *mut _);

            if let Some(aumid) = original {
                debug!("  Restoring {:?} -> '{}'", hwnd, aumid);
                let _ = set_aumid(hwnd, aumid);
            }
        }
    }
}

// ── Helper functions (private) ──

/// Set AppUserModelID cho một cửa sổ.
///
/// # Cơ chế
///
/// `SHGetPropertyStoreForWindow` → lấy `IPropertyStore`
/// → `SetValue(PKEY_AppUserModel_ID, prop)` → gán AUMID mới
///
/// # Lỗi thường gặp
///
/// - **Access Denied (0x80070005)**: cửa sổ system (VD: Program Manager)
/// - **Invalid window handle**: cửa sổ đã bị destroy
fn set_aumid(hwnd: HWND, aumid: &str) -> Result<(), windows::core::Error> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd)?;
        let prop = InitPropVariantFromStringAsVector(&HSTRING::from(aumid))?;
        store.SetValue(&PKEY_AppUserModel_ID, &prop)
    }
}

/// Lấy AppUserModelID hiện tại của một cửa sổ.
///
/// Trả về `None` nếu:
/// - Cửa sổ không có `IPropertyStore`
/// - `PKEY_AppUserModel_ID` không tồn tại hoặc rỗng
/// - Giá trị là `VT_EMPTY`
///
/// # Tại sao cần?
///
/// Để lưu AUMID gốc trước khi ghi đè bằng AUMID riêng.
/// Khi thoát app, ta khôi phục AUMID gốc để Windows hoạt động bình thường.
fn get_aumid(hwnd: HWND) -> Option<String> {
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let variant: PROPVARIANT = store.GetValue(&PKEY_AppUserModel_ID).ok()?;

        if variant.is_empty() {
            return None;
        }

        let bstr: windows::core::BSTR = (&variant).try_into().ok()?;
        let s = bstr.to_string();

        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}
