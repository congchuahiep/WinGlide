//! Liệt kê các nút (buttons) trên Windows 11 taskbar theo đúng thứ tự từ trái sang phải.
//!
//! # Tại sao không dùng `FindWindow` trực tiếp?
//!
//! Trên Windows 10, taskbar buttons là các `ToolbarWindow32` — một control tiêu chuẩn của Windows.
//! Ta có thể dùng `TB_GETBUTTON` message để lấy thông tin trực tiếp. Nhưng trên **Windows 11**,
//! Microsoft viết lại taskbar bằng **XAML** (UWP/WinRT). Các nút không còn là `HWND` riêng biệt
//! nữa — chúng là **XAML elements** bên trong `Windows.UI.Composition.DesktopWindowContentBridge`.
//!
//! Do đó ta phải dùng **UI Automation (UIAutomation)**, một COM-based API cho phép truy cập UI
//! elements bất kể underlying technology (Win32, XAML, WebView, etc.).
//!
//! # Khái niệm quan trọng: IUIAutomation
//!
//! **IUIAutomation** giống như một "máy quét màn hình" cho người khiếm thị. Nó mô tả mọi thứ trên màn hình thành một **cây phân cấp** (tree):
//!
//! ```text
//! Root (Desktop)
//!  └── Shell_TrayWnd (Taskbar)
//!       └── Windows.UI.Composition.DesktopWindowContentBridge
//!            └── Taskbar.TaskListButtonAutomationPeer  ← đây là các nút!
//!            └── Taskbar.TaskListButtonAutomationPeer
//!            └── ...
//! ```
//!
//! Mỗi **element** có các **properties**:
//! - `CurrentClassName`: loại element (VD: "Taskbar.TaskListButtonAutomationPeer")
//! - `CurrentName`: tên hiển thị (VD: "Chrome - 3 running windows")
//! - `CurrentBoundingRectangle`: vị trí + kích thước trên màn hình
//! - `CurrentProcessId`: PID của process sở hữu (thường là explorer.exe trên Win11)
//! - `CurrentAutomationId`: ID duy nhất của element
//!
//! # Luồng hoạt động
//!
//! ```rust
//! // 1. Tạo IUIAutomation instance
//! let automation = CoCreateInstance(&CUIAutomation)?;
//!
//! // 2. Tìm Shell_TrayWnd (taskbar window)
//! let taskbar = FindWindowW("Shell_TrayWnd", None)?;
//!
//! // 3. Lấy element gốc của taskbar
//! let root = automation.ElementFromHandle(taskbar)?;
//!
//! // 4. Tìm tất cả descendants là TaskListButtonAutomationPeer
//! let items = root.FindAll(TreeScope_Descendants, true_condition)?;
//!
//! // 5. Lọc, lấy thông tin, sort theo vị trí trái -> phải
//! buttons.sort_by_key(|b| b.rect.left);
//! ```

use tracing::{debug, instrument};
use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElementArray,
    TreeScope_Descendants,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW};

use crate::switcher::{find_window_for_button, find_windows_for_button};

const TASKBAR_BUTTON_CLASS: &str = "Taskbar.TaskListButtonAutomationPeer";

/// Taskbar button trên Windows 11. Chứa thông tin để xác định vị trí và thứ tự của nút trên
/// taskbar.
///
/// **Không chứa HWND** vì Win11 XAML taskbar buttons không có HWND riêng
#[derive(Debug, Clone)]
pub struct TaskbarButton {
    /// Tên hiển thị của nút.
    ///
    /// Format trên Win11:
    /// - App đơn: `"Chrome"`
    /// - App có nhiều window: `"Chrome - 3 running windows"`
    /// - App đã pin: `"Notepad - Pinned"`
    ///
    /// Dùng [`clean_button_name()`] để strip suffix.
    pub name: String,

    /// Vị trí và kích thước trên màn hình (pixel).
    ///
    /// Dùng `rect.left` để sắp xếp các nút theo thứ tự trái -> phải.
    pub rect: windows::Win32::Foundation::RECT,

    /// Process ID của ứng dụng sở hữu nút này.
    ///
    /// ⚠️ **Quan trọng**: Trên Win11, giá trị này THƯỜNG trả về PID của `explorer.exe`,
    /// không phải PID của ứng dụng thực. Lý do: XAML taskbar chạy trong explorer process.
    ///
    /// Do đó, ta KHÔNG thể dùng PID này trực tiếp để `SetForegroundWindow`.
    /// Phải dùng [`super::switcher::find_window_for_button()`] để tìm HWND thực.
    pub process_id: i32,

    /// Automation ID của button từ UI Automation.
    ///
    /// Trên Win11, đây có thể chứa AppUserModelID, giúp matching windows chính xác hơn.
    pub automation_id: Option<String>,
}

/// Một window target trong danh sách cycle.
/// Mỗi entry tương ứng với 1 window cụ thể (HWND),
/// không phải 1 taskbar button.
/// Dùng cho flat list cycling — grouped buttons được expand.
#[derive(Debug, Clone)]
pub struct CycleEntry {
    /// Tên hiển thị (window title)
    pub name: String,
    /// HWND của window cần activate
    pub hwnd: HWND,
    /// Vị trí trái của taskbar button gốc (để sort theo thứ tự trái→phải)
    pub taskbar_left: i32,
    /// Có thuộc grouped button không
    pub is_grouped: bool,
    /// Vị trí window trên màn hình (dùng để sort windows trong group)
    pub window_rect: RECT,
}

/// Result của việc enumerate taskbar buttons.
pub struct TaskbarEnumerator {
    /// COM interface IUIAutomation, "máy quét" UI.
    ///
    /// Không implements `Send`/`Sync` vì COM objects không an toàn khi share cross-thread.
    automation: IUIAutomation,

    /// Flag: đã tự init COM chưa.
    ///
    /// Nếu `true`, ta phải `CoUninitialize()` khi drop.
    /// Nếu `false`, có thể COM đã được init sẵn bởi thread khác.
    com_initialized: bool,
}

impl TaskbarEnumerator {
    /// Tạo enumerator mới và init COM (STA apartment).
    ///
    /// # COM Apartments
    ///
    /// Windows COM có 2 loại apartment:
    /// - **STA (Single-Threaded Apartment)**: Mỗi thread sở hữu message queue riêng, dùng
    /// `GetMessageW`.
    /// - **MTA (Multi-Threaded Apartment)**: Không có message queue, dùng
    /// `CoWaitForMultipleObjects`.
    ///
    /// IUIAutomation hoạt động tốt với cả 2, nhưng STA được khuyến nghị cho đơn giản.
    ///
    /// # Ví dụ
    ///
    /// ```rust,ignore
    /// let enumerator = TaskbarEnumerator::new()?;
    /// let buttons = enumerator.enumerate_primary_buttons()?;
    /// ```
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let com_initialized = hr.is_ok();

            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

            Ok(Self {
                automation,
                com_initialized,
            })
        }
    }

    /// Liệt kê tất cả taskbar buttons trên **primary monitor** (taskbar chính).
    ///
    /// # Luồng
    ///
    /// ```text
    /// 1. Tìm Shell_TrayWnd (FindWindowW)
    /// 2. Quét descendants từ root element (ElementFromHandle + FindAll)
    /// 3. Nếu không thấy -> thử qua DesktopWindowContentBridge (Win11 fallback)
    /// 4. Sort theo rect.left (trái -> phải)
    /// ```
    ///
    /// # Tại sao phải thử 2 lần?
    ///
    /// Win11 có 2 cấu trúc taskbar:
    /// 1. **DirectUI** (cũ): XAML buttons nằm trực tiếp trong Shell_TrayWnd tree
    /// 2. **ContentBridge** (mới): XAML buttons nằm trong
    /// `Windows.UI.Composition.DesktopWindowContentBridge`
    ///
    /// Code thử cả 2 path để đảm bảo tìm thấy buttons.
    pub fn enumerate_primary_buttons(&self) -> anyhow::Result<Vec<TaskbarButton>> {
        let taskbar_hwnd = self.find_primary_taskbar_hwnd()?;

        unsafe { self.enumerate_buttons_for_hwnd(taskbar_hwnd) }
    }

    /// Build danh sách cycle entries, mỗi entry là 1 window cụ thể.
    ///
    /// Grouped buttons (Combine mode ON) được expand thành nhiều entries, mỗi entry tương ứng với
    /// 1 window riêng lẻ. _(WARN: Cơ chế cho group button hiện tại không chính xác, xem phần bên
    /// dưới để biết lý do)_
    ///
    /// # Luồng
    ///
    /// 1. enumerate_primary_buttons() → lấy các taskbar buttons
    /// 2. Với mỗi button → find_all_windows_for_button() → tìm windows
    /// 3. Nếu button có 1 window → 1 CycleEntry
    /// 4. Nếu button có N>1 windows (grouped) → N CycleEntries, sort theo window_rect.left _(WARN:
    /// Cơ chế hiện tại không chính xác, xem phần bên dưới để biết lý do)_
    /// 5. Sort tất cả entries theo taskbar_left (trái → phải)
    ///
    /// # Ví dụ output
    ///
    /// Với taskbar: [Settings] [Chrome(group: 3 windows)] [VScode] [Explorer]
    ///
    /// Output flat list:
    /// ```text
    /// [Settings#1, Chrome#1, Chrome#2, Chrome#3, VScode#1, Explorer#1]
    /// ```
    ///
    /// # Thứ tự trong group
    ///
    /// Cơ chế hiện tại không chính xác: các cửa sổ trong một nhóm taskbar button đang được
    /// sắp xếp theo ID của window (HWND). Vì vậy, thứ tự cửa sổ không khớp với thứ tự hiển thị trên
    /// taskbar, mà chỉ theo ID nội bộ. Chi tiết hơn tại [`find_all_windows_for_button`].
    #[instrument(level = "debug", skip_all)]
    pub fn build_cycle_entries(&self, combine_enabled: bool) -> anyhow::Result<Vec<CycleEntry>> {
        let buttons = self.enumerate_primary_buttons()?;

        let mut entries = Vec::new();

        for button in &buttons {
            match combine_enabled {
                // Nếu combine_enabled là true, tìm các cửa sổ trong group button/không phải group
                // button và thêm vào entries
                true => {
                    let windows = find_windows_for_button(
                        &button.name,
                        button.process_id,
                        button.automation_id.as_deref(),
                    );

                    let is_grouped = windows.len() > 1;

                    for w in windows {
                        entries.push(CycleEntry {
                            name: w.title.clone(),
                            hwnd: w.hwnd,
                            taskbar_left: button.rect.left,
                            is_grouped,
                            window_rect: w.rect,
                        });
                    }
                }
                // Nếu combine_enabled là false, chỉ tìm cửa sổ duy nhất và thêm vào entries
                false => {
                    let window = find_window_for_button(
                        &button.name,
                        button.process_id,
                        button.automation_id.as_deref(),
                    );

                    match window {
                        Some(w) => {
                            entries.push(CycleEntry {
                                name: w.title.clone(),
                                hwnd: w.hwnd,
                                taskbar_left: button.rect.left,
                                is_grouped: false,
                                window_rect: w.rect,
                            });
                        }
                        None => {}
                    }
                }
            }
        }

        entries.sort_by(|a, b| {
            a.taskbar_left
                .cmp(&b.taskbar_left)
                .then_with(|| a.window_rect.left.cmp(&b.window_rect.left))
                .then_with(|| a.hwnd.0.cmp(&b.hwnd.0))
        });

        for (i, e) in entries.iter().enumerate() {
            debug!(
                "Entry[{}]: name='{}', grouped={}, left={}",
                i, e.name, e.is_grouped, e.taskbar_left
            );
        }

        Ok(entries)
    }

    /// Core enumeration logic — tìm tất cả TaskListButtonAutomationPeer.
    ///
    /// # Chi tiết từng bước
    ///
    /// ```rust,ignore
    /// // Tạo condition "lấy tất cả" (không lọc gì)
    /// let true_condition = automation.CreateTrueCondition()?;
    ///
    /// // Lấy element gốc của taskbar (Shell_TrayWnd)
    /// let root = automation.ElementFromHandle(taskbar_hwnd)?;
    ///
    /// // FindAll với TreeScope_Descendants = tìm TẤT CẢ con cháu
    /// let items = root.FindAll(TreeScope_Descendants, &true_condition)?;
    ///
    /// // Duyệt từng element, lọc class_name == TASKBAR_BUTTON_CLASS
    /// for i in 0..count {
    ///     let item = items.GetElement(i)?;
    ///     if item.CurrentClassName() == "Taskbar.TaskListButtonAutomationPeer" {
    ///         buttons.push(extract_info(item));
    ///     }
    /// }
    ///
    /// // Sort theo vị trí trái → phải (thứ tự taskbar)
    /// buttons.sort_by_key(|b| b.rect.left);
    /// ```
    unsafe fn enumerate_buttons_for_hwnd(
        &self,
        root_hwnd: HWND,
    ) -> anyhow::Result<Vec<TaskbarButton>> {
        let true_condition = self.automation.CreateTrueCondition()?;
        let mut all_buttons = Vec::new();
        let root_element = self.automation.ElementFromHandle(root_hwnd)?;
        let items = root_element.FindAll(TreeScope_Descendants, &true_condition)?;

        self.collect_buttons(&items, &mut all_buttons)?;

        if all_buttons.is_empty() {
            self.enumerate_via_bridge_windows(root_hwnd, &true_condition, &mut all_buttons)?;
        }

        // Sort theo vị trí trái -> phải
        //
        // rect.left = tọa độ x của cạnh trái button
        // Taskbar có thể ở trên/dưới/trái/phải màn hình,
        // nhưng với taskbar ngang, left tăng dần từ trái → phải.
        all_buttons.sort_by_key(|b| b.rect.left);

        Ok(all_buttons)
    }

    /// Lọc và trích xuất thông tin từ UIA element array.
    ///
    /// # Tại sao phải lọc?
    ///
    /// `FindAll(TreeScope_Descendants)` trả về **MỌI** element con của taskbar:
    /// - TaskListButtonAutomationPeer (các nút app)
    /// - StartButton
    /// - SearchButton
    /// - ClockButton
    /// - NotificationIcon
    /// - v.v.
    ///
    /// Ta chỉ quan tâm `Taskbar.TaskListButtonAutomationPeer`.
    ///
    /// # Mỗi button cung cấp gì?
    ///
    /// | Property | Ý nghĩa | Ví dụ |
    /// |----------|---------|---------|
    /// | `CurrentClassName` | Loại element | `"Taskbar.TaskListButtonAutomationPeer"` |
    /// | `CurrentName` | Tên hiển thị | `"Chrome - 3 running windows"` |
    /// | `CurrentBoundingRectangle` | Vị trí | `RECT { left: 100, top: 1060, ... }` |
    /// | `CurrentProcessId` | PID | `1234` (thường là explorer.exe) |
    unsafe fn collect_buttons(
        &self,
        items: &IUIAutomationElementArray,
        buttons: &mut Vec<TaskbarButton>,
    ) -> anyhow::Result<()> {
        let count = items.Length()?;

        for i in 0..count {
            let item = items.GetElement(i)?;

            // CurrentClassName: lọc chỉ lấy button
            //
            // Taskbar.TaskListButtonAutomationPeer = nút app trên taskbar Win11
            // StartButton = nút Start
            // SearchButton = nút Search
            // ClockButton = đồng hồ
            let class_name = item
                .CurrentClassName()
                .ok()
                .map(|b| b.to_string())
                .unwrap_or_default();

            if class_name == TASKBAR_BUTTON_CLASS {
                let name = item
                    .CurrentName()
                    .ok()
                    .map(|b| b.to_string())
                    .unwrap_or_default();

                // Lấy vị trí (để sắp xếp thứ tự)
                let rect = match item.CurrentBoundingRectangle() {
                    Ok(r) => r,
                    Err(_) => continue, // Button không có rect hợp lệ -> bỏ qua
                };

                // Lấy PID (để match với window thực)
                // ⚠️ Thường là explorer.exe PID, không phải app PID!
                let process_id = item.CurrentProcessId().unwrap_or(0);

                // Lấy AutomationID (có thể chứa AppUserModelID hoặc dùng để match)
                let automation_id = item.CurrentAutomationId().ok().map(|s| s.to_string());

                buttons.push(TaskbarButton {
                    name,
                    rect,
                    process_id,
                    automation_id,
                });
            }
        }

        Ok(())
    }

    /// Win11 fallback: Tìm buttons qua DesktopWindowContentBridge.
    ///
    /// Windows 11 có thể render taskbar buttons bên trong một
    /// `DesktopWindowContentBridge` window con của Shell_TrayWnd.
    ///
    /// # Luồng
    ///
    /// ```text
    /// 1. FindWindowEx tìm child window có class "Windows.UI.Composition.DesktopWindowContentBridge"
    /// 2. Gọi ElementFromHandle trên bridge window đó
    /// 3. FindAll từ bridge element
    /// 4. collect_buttons lọc TaskListButtonAutomationPeer
    /// ```
    ///
    /// # Tại sao cần vòng lặp?
    ///
    /// Có thể có NHIỀU bridge windows (một số ẩn hoặc không chứa buttons).
    /// Code duyệt đến khi tìm thấy buttons HOẶC hết child windows.
    unsafe fn enumerate_via_bridge_windows(
        &self,
        root_hwnd: HWND,
        condition: &IUIAutomationCondition,
        buttons: &mut Vec<TaskbarButton>,
    ) -> anyhow::Result<()> {
        // HWND::default() = null pointer
        let mut child_hwnd = HWND::default();

        // FindWindowEx: tìm child window của root_hwnd
        //
        // FindWindowExW(parent, child_after, class_name, window_name)
        // - parent = Shell_TrayWnd
        // - child_after = null (tìm từ đầu)
        // - class_name = "Windows.UI.Composition.DesktopWindowContentBridge"
        // - window_name = null (bất kỳ)
        //
        // Vòng lặp để tìm TẤT CẢ child windows cùng class
        loop {
            child_hwnd = FindWindowExW(
                Some(root_hwnd),
                Some(child_hwnd),
                w!("Windows.UI.Composition.DesktopWindowContentBridge"),
                None,
            )
            .unwrap_or_default();

            // .0.is_null() = kiểm tra HWND có null không
            if child_hwnd.0.is_null() {
                break; // Hết windows
            }

            // Lấy UIA element từ bridge HWND
            if let Ok(bridge_element) = self.automation.ElementFromHandle(child_hwnd) {
                // Tìm descendants (các nút bên trong bridge)
                if let Ok(items) = bridge_element.FindAll(TreeScope_Descendants, condition) {
                    self.collect_buttons(&items, buttons)?;
                }
            }

            // Nếu đã tìm thấy buttons → dừng
            // Tránh duyệt thêm các bridge không cần thiết
            if !buttons.is_empty() {
                break;
            }
        }

        Ok(())
    }

    /// Tìm HWND của primary taskbar (`Shell_TrayWnd`).
    ///
    /// # Shell_TrayWnd là gì?
    ///
    /// `Shell_TrayWnd` (Shell Tray Window) là **top-level window** của taskbar.
    /// Đây là window class tiêu chuẩn của Windows từ Windows 95 đến Win11.
    ///
    /// Các class windows liên quan:
    /// - `Shell_TrayWnd` — taskbar chính
    /// - `Shell_SecondaryTrayWnd` — taskbar trên monitor phụ
    /// - `ReBarWindow32` — container chứa taskbar items (Win10)
    /// - `MSTaskSwWClass` — taskbar switcher (Win10)
    /// - `MSTaskListWClass` — danh sách nhiệm vụ (Win10)
    ///
    /// # Win11 thay đổi gì?
    ///
    /// Win11 ẩn `ReBarWindow32` và dùng XAML-based taskbar.
    /// Nhưng `Shell_TrayWnd` vẫn tồn tại (legacy compatibility).
    fn find_primary_taskbar_hwnd(&self) -> anyhow::Result<HWND> {
        unsafe {
            let hwnd = FindWindowW(w!("Shell_TrayWnd"), None).unwrap_or_default();

            if hwnd.0.is_null() {
                anyhow::bail!("Shell_TrayWnd not found — có thể đang chạy portable mode hoặc taskbar bị disabled");
            }

            Ok(hwnd)
        }
    }

    /// Tìm index của taskbar button đang "active" (focused window thuộc app đó).
    ///
    /// # Active button là gì?
    ///
    /// Active button = nút trên taskbar tương ứng với cửa sổ đang ở foreground.
    ///
    /// Ví dụ: Đang focus vào VS Code window:
    /// - Active button = nút "VS Code - 2 running windows"
    ///
    /// # Thuật toán
    ///
    /// 1. Lấy foreground window (window đang được focus)
    /// 2. Từ foreground HWND → lấy UIA element
    /// 3. Từ element → lấy PID và Name
    /// 4. So khớp với danh sách buttons:
    ///    - Ưu tiên 1: PID khớp (nếu không phải explorer.exe)
    ///    - Ưu tiên 2: Name chứa nhau (fuzzy match)
    ///
    /// # Tại sao không dùng UIA Selection?
    ///
    /// IUIAutomation có `CurrentIsSelected` property, nhưng trên Win11 taskbar,
    /// property này không always accurate hoặc available.
    /// Nên ta fallback sang so khớp PID/Name.
    pub fn find_active_button_index(
        &self,
        buttons: &[TaskbarButton],
        foreground_hwnd: HWND,
    ) -> Option<usize> {
        unsafe {
            // Lấy UIA element của foreground window
            let fg_element = self.automation.ElementFromHandle(foreground_hwnd).ok()?;

            // Properties của foreground window
            let fg_pid = fg_element.CurrentProcessId().ok().unwrap_or(-1);
            let fg_name = fg_element
                .CurrentName()
                .ok()
                .map(|b| b.to_string())
                .unwrap_or_default();

            // Ưu tiên 1: PID khớp (nếu > 0 và không phải explorer)
            //
            // ⚠️ PID từ taskbar button thường = explorer.exe PID
            // Nên check này thường fail trên Win11
            for (i, button) in buttons.iter().enumerate() {
                if button.process_id == fg_pid && fg_pid > 0 {
                    return Some(i);
                }
            }

            // Ưu tiên 2: Name fuzzy match
            //
            // Strip suffix trước khi so:
            // "Chrome - 3 running windows" → "Chrome"
            // "VS Code - main.rs" → "VS Code - main.rs"
            let fg_clean = clean_button_name(&fg_name);

            for (i, button) in buttons.iter().enumerate() {
                let btn_clean = clean_button_name(&button.name);

                // fg_clean chứa btn_clean HOẶC ngược lại
                // Ví dụ:
                // - fg_name = "main.rs - VS Code"
                // - btn_name = "VS Code - 1 running window"
                // → clean: "main.rs - VS Code" contains "VS Code" ✓
                if !btn_clean.is_empty()
                    && (fg_clean.contains(&btn_clean) || btn_clean.contains(&fg_clean))
                {
                    return Some(i);
                }
            }

            None
        }
    }
}

/// Strip suffix " - N running window(s)" từ taskbar button name.
///
/// Win11 taskbar button name format:
///
/// | Loại | Format | After clean |
/// |------|--------|------------|
/// | App đơn | `"Notepad"` | `"Notepad"` |
/// | Nhiều windows | `"Chrome - 3 running windows"` | `"Chrome"` |
/// | Pinned | `"Notepad - Pinned"` | `"Notepad - Pinned"` |
/// | VS Code split | `"VS Code - main.rs - 1 running window"` | `"VS Code - main.rs"` |
///
/// # Algorithm
///
/// 1. Tìm `" running window"` từ cuối chuỗi
/// 2. Lấy phần trước đó
/// 3. Tìm `" - "` hoặc `" — "` (em dash) làm delimiter cuối
/// 4. Trả về phần trước delimiter
///
/// # Ví dụ
///
/// ```rust
/// assert_eq!(clean_button_name("Chrome - 3 running windows"), "Chrome");
/// assert_eq!(clean_button_name("VS Code - main.rs - 1 running window"), "VS Code - main.rs");
/// assert_eq!(clean_button_name("Notepad"), "Notepad"); // không đổi
/// ```
pub fn clean_button_name(name: &str) -> String {
    // rfind: tìm từ cuối về đầu
    if let Some(pos) = name.rfind(" running window") {
        // Lấy phần trước " running window"
        let before = &name[..pos];

        // Thử dash thường: " - "
        if let Some(dash_pos) = before.rfind(" - ") {
            return before[..dash_pos].to_string();
        }

        // Thử em dash: " — " (Unicode U+2014)
        if let Some(dash_pos) = before.rfind(" \u{2014} ") {
            return before[..dash_pos].to_string();
        }

        // Không có dash → trả về toàn bộ phần trước
        return before.to_string();
    }

    // Không có suffix → trả về nguyên name
    name.to_string()
}

// /// Destructor — giải phóng COM khi TaskbarEnumerator bị drop.
// ///
// /// Nếu `CoInitializeEx` được gọi thành công trong `new()`,
// /// ta phải gọi `CoUninitialize()` để "rút phích cắm COM".
// ///
// /// ⚠️ **Quan trọng**: Chỉ uninitialize nếu chính ta đã init.
// /// Nếu COM đã được init sẵn bởi thread khác, việc uninitialize
// /// có thể gây crash hoặc lỗi cho ứng dụng khác.
// impl Drop for TaskbarEnumerator {
//     fn drop(&mut self) {
//         if self.com_initialized {
//             unsafe {
//                 CoUninitialize();
//             }
//         }
//     }
// }
