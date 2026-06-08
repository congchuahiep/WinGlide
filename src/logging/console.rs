//! Quản lý cửa sổ Debug Console. Dùng để logging realtime
//!
//! ## Kiến trúc: "Self-Invoking Console Worker"
//!
//! Vì ứng dụng chính chạy với `#![windows_subsystem = "windows"]` (GUI App), nó không có stdout.
//! Khi gọi subprocess bằng lệnh `Command` kèm `Stdio::piped()`, stdout mặc định sẽ là `NULL`.
//!
//! Giải pháp: Ứng dụng **tự gọi lại chính nó** với tham số `--console-worker`.
//! Tiến trình con:
//! 1. Gọi `AllocConsole()` để tự tạo ra một Console Window mới (GUI App không tự có).
//! 2. Mở file đặc biệt `CONOUT$` (ghi thẳng vào bộ đệm của Console hiện tại, bỏ qua `stdout` đã bị NULL).
//! 3. Bật cờ `ENABLE_VIRTUAL_TERMINAL_PROCESSING` để hiển thị màu ANSI.
//! 4. Đọc luồng `stdin` liên tục và in ra màn hình. Khi `stdin` bị đóng (app chính tắt) -> tiến trình con thoát.

use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

extern "system" {
    fn AllocConsole() -> i32;
    fn GetConsoleMode(hConsoleHandle: isize, lpMode: *mut u32) -> i32;
    fn SetConsoleMode(hConsoleHandle: isize, dwMode: u32) -> i32;
    fn SetConsoleTitleW(lpConsoleTitle: *const u16) -> i32;
}

/// Cờ đồng bộ trạng thái hiển thị Console.
pub static CONSOLE_VISIBLE: AtomicBool = AtomicBool::new(false);

/// Cờ báo hiệu ứng dụng đang chạy ở chế độ CLI debug (in log ra stdout).
pub static DEBUG_CLI_MODE: AtomicBool = AtomicBool::new(false);

/// Handle tiến trình con Console, bảo vệ bằng Mutex.
static CHILD_PROCESS: Mutex<Option<Child>> = Mutex::new(None);

/// Pipe stdin dùng chung cho `MakeWriter`, bảo vệ bằng Mutex.
pub static CONSOLE_PIPE: Mutex<Option<std::process::ChildStdin>> = Mutex::new(None);

/// Bật/tắt cửa sổ Debug Console.
pub fn toggle() {
    if DEBUG_CLI_MODE.load(Ordering::SeqCst) {
        return;
    }

    let was_visible = CONSOLE_VISIBLE.load(Ordering::SeqCst);
    let new_visible = !was_visible;
    CONSOLE_VISIBLE.store(new_visible, Ordering::SeqCst);

    match new_visible {
        true => spawn_console(),
        false => kill_console(),
    }
}

/// Chạy vòng lặp console worker (chỉ gọi từ tiến trình con)
pub fn run_worker() {
    unsafe {
        AllocConsole();
    }

    // Cài đặt tiêu đề cửa sổ
    let title = "Debug Console - Taskbar Switcher";
    let mut title_u16: Vec<u16> = title.encode_utf16().collect();
    title_u16.push(0);
    unsafe {
        SetConsoleTitleW(title_u16.as_ptr());
    }

    // Mở CONOUT$ để ghi trực tiếp (vì stdout của gui app truyền qua đã bị NULL)
    // Phải mở với quyền đọc/ghi để GetConsoleMode hoạt động
    let out_res = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$");
    if out_res.is_err() {
        return;
    }
    let mut out = out_res.unwrap();

    // Bật hỗ trợ màu sắc ANSI
    let handle = out.as_raw_handle() as isize;
    let mut mode = 0;
    unsafe {
        if GetConsoleMode(handle, &mut mode) != 0 {
            SetConsoleMode(handle, mode | 0x0004);
        }
    }

    // Tắt khả năng select (Quick Edit Mode) trên CONIN$
    if let Ok(in_file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONIN$")
    {
        let in_handle = in_file.as_raw_handle() as isize;
        let mut in_mode = 0;
        unsafe {
            if GetConsoleMode(in_handle, &mut in_mode) != 0 {
                SetConsoleMode(in_handle, (in_mode & !0x0040) | 0x0080);
            }
        }
    }

    // Đọc liên tục từ Stdin và ghi ra màn hình
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 1024];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break, // EOF -> app chính đã đóng
            Ok(n) => {
                if out.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Spawn tiến trình con `--console-worker`
fn spawn_console() {
    kill_console();

    let exe_path = std::env::current_exe().expect("Failed to get executable path");
    let result = Command::new(exe_path)
        .arg("--console-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_PROCESS_GROUP) // Nhóm tiến trình mới để khi ấn Ctrl+C không chết app chính
        .spawn();

    match result {
        Ok(mut child) => {
            let stdin = child.stdin.take();
            *CHILD_PROCESS.lock().unwrap() = Some(child);
            *CONSOLE_PIPE.lock().unwrap() = stdin;
        }
        Err(e) => {
            CONSOLE_VISIBLE.store(false, Ordering::SeqCst);
            eprintln!("Failed to spawn debug console worker: {e}");
        }
    }
}

/// Tắt tiến trình PowerShell con.
fn kill_console() {
    // Đóng pipe trước để tiến trình con nhận EOF
    let _ = CONSOLE_PIPE.lock().unwrap().take();

    if let Some(mut child) = CHILD_PROCESS.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Windows process creation flags.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
