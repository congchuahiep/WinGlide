use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{debug, error, instrument};
use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::InitPropVariantFromStringAsVector;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};

use crate::utils::truncate;

static ORIGINAL_AUMID: std::sync::OnceLock<Mutex<HashMap<isize, Option<String>>>> =
    std::sync::OnceLock::new();

pub fn get_original_aumid_map() -> &'static Mutex<HashMap<isize, Option<String>>> {
    ORIGINAL_AUMID.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Kiểm tra xem một cửa sổ có được theo dõi bởi uncombine hay không.
pub fn is_window_tracked(hwnd: HWND) -> bool {
    get_original_aumid_map()
        .lock()
        .unwrap()
        .contains_key(&(hwnd.0 as isize))
}

/// AUMID Nhằm xác định các cửa sổ cho một taskbar button group, vì vậy nếu đặt AUMID riêng biệt
/// cho mỗi cửa sổ sẽ chặn việc nhóm các cửa sổ lại thành một group trên taskbar.
fn set_window_app_user_model_id(hwnd: HWND, aumid: &str) -> Result<(), windows::core::Error> {
    unsafe {
        let store: IPropertyStore = match SHGetPropertyStoreForWindow(hwnd) {
            Ok(s) => s,
            Err(e) => return Err(e),
        };

        let prop = match InitPropVariantFromStringAsVector(&HSTRING::from(aumid)) {
            Ok(p) => p,
            Err(e) => return Err(e),
        };

        if let Err(e) = store.SetValue(&PKEY_AppUserModel_ID, &prop) {
            return Err(e);
        }

        Ok(())
    }
}

fn get_window_app_user_model_id(hwnd: HWND) -> Option<String> {
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

    unsafe {
        let store: IPropertyStore = match SHGetPropertyStoreForWindow(hwnd) {
            Ok(s) => s,
            Err(_) => return None,
        };

        let variant: PROPVARIANT = match store.GetValue(&PKEY_AppUserModel_ID) {
            Ok(v) => v,
            Err(_) => return None,
        };

        if variant.is_empty() {
            return None;
        }

        let bstr: windows::core::BSTR = match (&variant).try_into() {
            Ok(b) => b,
            Err(_) => return None,
        };

        let s = bstr.to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

#[instrument(level = "debug", skip_all)]
pub fn restore_all_app_user_model_ids() {
    debug!("Restoring original AppUserModelIDs");

    let map = get_original_aumid_map().lock().unwrap();

    for (&hwnd_val, original_aumid) in map.iter() {
        let hwnd = HWND(hwnd_val as *mut _);

        match original_aumid {
            Some(aumid) => {
                debug!("Restoring {:?} to '{}'", hwnd, aumid);
                let _ = set_window_app_user_model_id(hwnd, aumid);
            }
            None => {}
        }
    }
}

/// Duyệt qua tất cả các window đang hiển thị trên taskbar và tách chúng thành các
/// taskbar button riêng biệt, không group lại với nhau.
#[instrument(level = "debug", skip_all)]
pub fn uncombine_all_windows() {
    let windows = crate::switcher::find_visible_windows();
    let mut map = get_original_aumid_map().lock().unwrap();

    for w in &windows {
        let hwnd_val = w.hwnd.0 as isize;

        if map.contains_key(&hwnd_val) {
            continue;
        }

        let original_aumid = get_window_app_user_model_id(w.hwnd);
        let new_aumid = format!("TaskbarSwitcher_{}", hwnd_val);
        map.insert(hwnd_val, original_aumid.clone());

        match set_window_app_user_model_id(w.hwnd, &new_aumid) {
            Ok(()) => debug!(
                "New window {:?} has been uncombined",
                truncate(&w.title, 30),
            ),
            Err(e) => {
                error!(
                    "Failed to set AUMID for window {:?}: {:?}",
                    truncate(&w.title, 30),
                    e
                );
            }
        }
    }
}

/// Gọi hàm này mỗi lần có window mới xuất hiện trong taskbar, hàm này sẽ set cho window đó một
/// AppUserModelID riêng biệt, nhờ đó window đó sẽ không bị group lại với các window khác trong
/// taskbar
pub fn uncombine_new_window(hwnd: HWND) {
    let hwnd_val = hwnd.0 as isize;
    let mut map = get_original_aumid_map().lock().unwrap();

    // Đã được track rồi, bỏ qua
    if map.contains_key(&hwnd_val) {
        debug!("Window: {:?} is already uncombined, skipping...", hwnd);
        return;
    }

    let original_aumid = get_window_app_user_model_id(hwnd);
    let new_aumid = format!("TaskbarSwitcher_{}", hwnd_val);
    map.insert(hwnd_val, original_aumid.clone());

    match set_window_app_user_model_id(hwnd, &new_aumid) {
        Ok(()) => debug!(
            "New window {:?} original AUMID: {:?} has been uncombined",
            hwnd, original_aumid
        ),
        Err(e) => {
            error!("Failed to set AUMID for window {:?}: {:?}", hwnd, e);
        }
    }
}
