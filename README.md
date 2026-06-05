# Taskbar Switcher

Ứng dụng Rust thuần cho **Windows 11** giúp chuyển đổi qua lại giữa các nút taskbar bằng theo thứ tự trên thanh taskbar (khác với Alt+Tab dùng để chuyển cửa sổ mở gần nhất).

```
Alt + [    ->  Chuyển sang nút taskbar bên trái
Alt + ]    ->  Chuyển sang nút taskbar bên phải
Ctrl + C   ->  Thoát
```

## Tại sao cần ứng dụng này?

Windows 11 mặc định **không có phím tắt** để cycle qua các nút trên taskbar (khác với Alt+Tab dùng để chuyển cửa sổ). Ứng dụng này mang lại trải nghiệm giống như **Mouse Wheel on Taskbar** trên Windhawk, nhưng là phiên bản standalone không cần DLL injection.

## Kiến trúc

```
src/
├── main.rs        # Entry point, hotkey loop, điều phối cycle
├── taskbar.rs     # IUIAutomation - liệt kê buttons từ Win11 taskbar
├── switcher.rs    # EnumWindows - tìm HWND thực của app, force activate
└── hotkey.rs      # RegisterHotKey - đăng ký global hotkeys
```

### Luồng hoạt động (Cycle)

```
1. Alt+] được nhấn
         ↓
2. IUIAutomation -> liệt kê tất cả Taskbar.TaskListButtonAutomationPeer
         ↓
3. Tìm button đang active (window đang focus)
         ↓
4. Tính target index (active + 1 hoặc -1, wrap around)
         ↓
5. EnumWindows -> tìm HWND của target app (match PID / title)
         ↓
6. force_activate(hwnd) -> đưa target lên foreground
```

## Công nghệ sử dụng

| Thành phần | Công nghệ |
|-----------|-----------|
| Ngôn ngữ | Rust (Edition 2021) |
| Windows API | windows-rs 0.61 |
| Taskbar enumeration | IUIAutomation (UIA) |
| Tìm window | EnumWindows + GetWindowTextW |
| Activate window | SetForegroundWindow + AttachThreadInput |
| Global hotkeys | RegisterHotKey + GetMessageW |

## Tại sao dùng IUIAutomation?

Trên **Windows 10**, taskbar buttons là các `ToolbarWindow32` - ta có thể dùng `TB_GETBUTTON` message trực tiếp.

Nhưng trên **Windows 11**, Microsoft viết lại taskbar bằng **XAML** (UWP/WinRT). Các nút không còn là `HWND` riêng biệt - chúng là **XAML elements** bên trong `Windows.UI.Composition.DesktopWindowContentBridge`.

Do đó phải dùng **UI Automation (UIAutomation)** - COM-based API truy cập UI elements bất kể underlying technology.

## Build

```bash
cargo build --release
```

Binary sẽ nằm ở `target/release/taskbar_switcher.exe`.

## Chạy

```bash
./target/release/taskbar_switcher.exe
```

Ứng dụng chạy nền, đăng ký global hotkeys. Tắt bằng `Ctrl+C`.

## Cách thức hoạt động chi tiết

### 1. Enumerate Buttons (`taskbar.rs`)

```
Shell_TrayWnd (FindWindowW)
  └── Windows.UI.Composition.DesktopWindowContentBridge
       └── Taskbar.TaskListButtonAutomationPeer  ← các nút
       └── Taskbar.TaskListButtonAutomationPeer
       └── ...
```

IUIAutomation `FindAll(TreeScope_Descendants)` lấy tất cả descendants, lọc class name = `"Taskbar.TaskListButtonAutomationPeer"`. Sort theo `rect.left` để có thứ tự trái->phải.

### 2. Tìm Active Button (`find_active_button_index`)

Từ `GetForegroundWindow()` -> lấy element -> so sánh PID và name (fuzzy match) với danh sách buttons.

### 3. Tìm Target HWND (`switcher.rs`)

Win11 XAML buttons **không có HWND riêng** - ta phải tìm window HWND của ứng dụng bằng cách:

- EnumWindows duyệt tất cả visible windows
- So khớp PID (nếu button PID != explorer PID)
- Hoặc fuzzy match title (`clean_button_name`)

### 4. Force Activate (`force_activate`)

Windows có **foreground lock** - không phải lúc nào `SetForegroundWindow` cũng hoạt động. Thứ tự ưu tiên:

1. `AllowSetForegroundWindow(ASFW_ANY)` - cho phép foreground switch
2. `SetForegroundWindow(target)` - thử trực tiếp
3. `AttachThreadInput` dance - attach current thread vào foreground thread để bypass lock

## Các vấn đề đã giải quyết

- **Win11 buttons không có HWND** -> Dùng IUIAutomation enumerate + EnumWindows tìm app HWND
- **Button PID luôn = explorer.exe** -> Không dùng PID trực tiếp, thay vào đó fuzzy match title
- **SetForegroundWindow bị chặn** -> AttachThreadInput dance + AllowSetForegroundWindow
- **" - N running window" suffix** -> `clean_button_name()` strip suffix trước khi match

## Giới hạn

- Chỉ hỗ trợ **Windows 11**
- Chỉ enumerate **primary monitor taskbar**
- Hotkeys `Alt+[` và `Alt+]` có thể conflict với ứng dụng khác (WSL, vim, etc.)

## Tham khảo

- Windhawk `taskbar-wheel-cycle` mod
- windows-rs documentation
- Microsoft UI Automation documentation
