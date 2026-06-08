#![windows_subsystem = "windows"]

mod app;
mod event;
mod hotkey;
mod indicator;
mod logging;
mod taskbar;
mod tray_icon;
mod types;
mod utils;

use std::sync::atomic::Ordering;

use tracing::info;
use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi;

#[derive(Default)]
struct Args {
    debug: bool,
    verbose: bool,
    combine_enabled: bool,
    console_worker: bool,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().collect();
    Args {
        debug: raw.iter().any(|a| a == "--debug"),
        verbose: raw.iter().any(|a| a == "-v" || a == "--verbose"),
        combine_enabled: raw.iter().any(|a| a == "--combine-mode"),
        console_worker: raw.iter().any(|a| a == "--console-worker"),
    }
}

fn print_help(args: &Args) {
    let mut info = String::from(
        "\nTaskbar Switcher started:\
        \n\tAlt+[  : cycle left\
        \n\tAlt+]  : cycle right\
        \n\tRight-click tray icon : menu\
        \n\
        \n\t-v/--verbose: enable debug logging\
        \n\t--combine-mode: enable combine mode\
        \n\t--debug: attach console for debugging",
    );

    if args.verbose {
        info.push_str("\nVerbose logging enabled");
    }

    if args.combine_enabled {
        info.push_str("\nCombine mode enabled");
    }

    if args.debug {
        info.push_str("\nDebug console enabled");
    }

    println!("{}\n", info);
}

fn main() -> anyhow::Result<()> {
    unsafe {
        // Thông báo cho Windows: "Tôi tự lo được màn hình độ phân giải cao (Per-Monitor v2),
        // đừng tự zoom mờ app của tôi!"
        let _ =
            HiDpi::SetProcessDpiAwarenessContext(HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let args = parse_args();

    if args.debug {
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
                let _ = AllocConsole();
            }
        }
        crate::logging::console::DEBUG_CLI_MODE.store(true, Ordering::SeqCst);
    }

    if args.console_worker {
        logging::console::run_worker();
        return Ok(());
    }

    print_help(&args);

    let _guard = logging::setup_logger(args.verbose);

    let main_thread_id = unsafe { GetCurrentThreadId() };
    let mut app = app::App::new(args.combine_enabled)?;

    unsafe {
        app.run(main_thread_id)?;
    }

    info!("Taskbar Switcher stopped");
    Ok(())
}
