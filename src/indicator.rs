use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct IndicatorWindow {
    pub hwnd: HWND,

    /// Giữ cho luồng lắng nghe sự kiện chuyển Desktop không bị Drop
    _desktop_event_thread: Option<winvd::DesktopEventThread>,
}

impl IndicatorWindow {
    pub unsafe fn new() -> anyhow::Result<Self> {
        let hinstance = GetModuleHandleW(None)?;

        let class_name = w!("TaskbarSwitcherIndicator");

        let wnd_class = WNDCLASSW {
            hInstance: HINSTANCE(hinstance.0),
            lpszClassName: class_name,
            lpfnWndProc: Some(Self::window_proc),
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0 as _),
            ..Default::default()
        };

        let atom = RegisterClassW(&wnd_class);
        if atom == 0 {
            // Might already be registered
        }

        let taskbar_hwnd = FindWindowW(w!("Shell_TrayWnd"), None)?;

        let mut tray_rect = RECT::default();
        unsafe { GetWindowRect(taskbar_hwnd, &mut tray_rect)? };
        let taskbar_height = tray_rect.bottom - tray_rect.top;

        println!("Taskbar height: {}", taskbar_height);

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE,
                class_name,
                w!("Indicator"),
                WS_POPUP | WS_VISIBLE,
                tray_rect.left + 10,
                tray_rect.top,
                128,
                taskbar_height,
                Some(taskbar_hwnd),
                None,
                Some(hinstance.into()),
                None,
            )?
        };

        unsafe {
            // LWA_COLORKEY: Everything black (0,0,0) will be fully transparent.
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0x000000), 0, LWA_COLORKEY);
        }

        Ok(Self {
            hwnd,
            _desktop_event_thread: None,
        })
    }

    pub fn run(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel::<winvd::DesktopEvent>();
        let hwnd_ind_ptr = self.hwnd.0 as isize;

        match winvd::listen_desktop_events(tx) {
            Ok(thread) => {
                std::thread::spawn(move || {
                    while let Ok(_event) = rx.recv() {
                        unsafe {
                            let hwnd_ind = windows::Win32::Foundation::HWND(hwnd_ind_ptr as *mut _);
                            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(
                                Some(hwnd_ind),
                                None,
                                true,
                            );

                            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                Some(hwnd_ind),
                                windows::Win32::UI::WindowsAndMessaging::WM_NULL,
                                windows::Win32::Foundation::WPARAM(0),
                                windows::Win32::Foundation::LPARAM(0),
                            );
                        }
                    }
                });

                self._desktop_event_thread = Some(thread);
            }
            Err(e) => tracing::error!("Failed to start winvd desktop event listener: {:?}", e),
        }
    }

    pub fn redraw(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);

                // Fill background with black (transparent key)
                let rect = ps.rcPaint;
                let hbrush = GetStockObject(BLACK_BRUSH);
                FillRect(hdc, &rect, HBRUSH(hbrush.0 as _));

                let count = winvd::get_desktop_count().unwrap_or(1) as usize;
                let current = winvd::get_current_desktop().ok();
                let desktops = winvd::get_desktops().unwrap_or_default();

                let mut current_idx = 0;
                if let Some(c) = current {
                    for (i, d) in desktops.iter().enumerate() {
                        if *d == c {
                            current_idx = i;
                            break;
                        }
                    }
                }

                let mut rect = RECT::default();
                unsafe {
                    GetClientRect(hwnd, &mut rect).ok();
                };

                let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as i32;
                let scale = dpi as f32 / 96.0;

                let base_radius = 4.5_f32;
                let base_spacing = 16_f32;
                let base_start_x = 10_f32;

                SetBkMode(hdc, TRANSPARENT);
                let radius = (base_radius * scale) as i32;
                let spacing = (base_spacing * scale) as i32;
                let start_x = (base_start_x * scale) as i32;
                let y_center = (rect.bottom - rect.top) / 2;

                unsafe {
                    // ==========================================
                    // BẮT ĐẦU KỸ THUẬT SUPERSAMPLING (Vẽ nháp x4)
                    // ==========================================
                    let scale_factor = 4; // Phóng to gấp 4 lần để khử răng cưa
                    let mem_width = (rect.right - rect.left) * scale_factor;
                    let mem_height = (rect.bottom - rect.top) * scale_factor;

                    // Tạo một tờ giấy nháp trong RAM (Memory DC)
                    let mem_dc = CreateCompatibleDC(Some(hdc));
                    let mem_bmp = CreateCompatibleBitmap(hdc, mem_width, mem_height);
                    let old_bmp = SelectObject(mem_dc, HGDIOBJ(mem_bmp.0 as _));

                    // Tô nền đen (Color Key) cho tờ nháp
                    let bg_brush = GetStockObject(BLACK_BRUSH);
                    let mem_rect = RECT {
                        left: 0,
                        top: 0,
                        right: mem_width,
                        bottom: mem_height,
                    };
                    FillRect(mem_dc, &mem_rect, HBRUSH(bg_brush.0 as _));

                    let old_pen = SelectObject(mem_dc, HGDIOBJ(GetStockObject(NULL_PEN).0 as _));

                    // Tính toán tọa độ GẤP 4 LẦN
                    let big_radius = radius * scale_factor;
                    let big_spacing = spacing * scale_factor;
                    let big_start_x = start_x * scale_factor;
                    let big_y_center = y_center * scale_factor;

                    for i in 0..count {
                        let hbrush = if i == current_idx {
                            CreateSolidBrush(COLORREF(0x00FFFFFF))
                        } else {
                            CreateSolidBrush(COLORREF(0x00666666))
                        };
                        let old_brush = SelectObject(mem_dc, HGDIOBJ(hbrush.0 as _));

                        let big_x = big_start_x + (i as i32) * big_spacing;

                        // Vẽ hình tròn siêu to lên tờ nháp
                        Ellipse(
                            mem_dc,
                            big_x - big_radius,
                            big_y_center - big_radius,
                            big_x + big_radius,
                            big_y_center + big_radius,
                        )
                        .ok()
                        .ok();

                        SelectObject(mem_dc, old_brush);
                        DeleteObject(HGDIOBJ(hbrush.0 as _)).ok().ok();
                    }
                    SelectObject(mem_dc, old_pen);

                    // ==========================================
                    // ÉP NHỎ TỜ NHÁP LÊN MÀN HÌNH THẬT (Anti-Aliasing)
                    // ==========================================

                    // Bật thuật toán làm mịn (HALFTONE)
                    SetStretchBltMode(hdc, HALFTONE);
                    SetBrushOrgEx(hdc, 0, 0, None).ok().ok(); // Lệnh bắt buộc khi dùng HALFTONE

                    // Copy và thu nhỏ 4 lần từ tờ nháp (mem_dc) sang màn hình (hdc)
                    StretchBlt(
                        hdc,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        // Đích
                        Some(mem_dc),
                        0,
                        0,
                        mem_width,
                        mem_height, // Nguồn
                        SRCCOPY,
                    )
                    .ok()
                    .ok();

                    // Dọn dẹp RAM
                    SelectObject(mem_dc, old_bmp);
                    DeleteObject(HGDIOBJ(mem_bmp.0 as _)).ok().ok();
                    DeleteDC(mem_dc).ok().ok();
                }

                let _ = EndPaint(hwnd, &mut ps);
                LRESULT(0)
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                // 1. Đo lại kích thước mới của Taskbar
                let mut tray_rect = RECT::default();
                if let Ok(taskbar_hwnd) = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) } {
                    unsafe { GetWindowRect(taskbar_hwnd, &mut tray_rect).ok() };
                }
                let taskbar_height = tray_rect.bottom - tray_rect.top;

                // 2. Cập nhật lại vị trí và kích thước mới cho Indicator
                unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(HWND(0 as *mut _)),
                        tray_rect.left + 10,
                        tray_rect.top,
                        128,
                        taskbar_height,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                    .ok();

                    // 3. Xóa bộ đệm, ép vẽ lại ngay lập tức
                    InvalidateRect(Some(hwnd), None, true).ok().ok();
                }
                LRESULT(0)
            }
            WM_NCHITTEST => {
                // HTTRANSPARENT = -1
                LRESULT(-1)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

impl Drop for IndicatorWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
