mod hotkey;
mod logging;
mod switcher;
mod taskbar;
mod uncombine;
mod utils;
mod winevent;

use crate::hotkey::{HotkeyAction, HotkeyManager};
use crate::utils::truncate;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use taskbar::TaskbarEnumerator;
use tracing::{debug, error, info, instrument, warn};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

enum CycleDirection {
    Forward,
    Backward,
}

#[instrument(level = "debug", skip_all)]
fn cycle_taskbar(
    enumerator: &TaskbarEnumerator,
    combine_enabled: bool,
    direction: CycleDirection,
) -> anyhow::Result<()> {
    let entries = enumerator.build_cycle_entries(combine_enabled)?;

    if entries.is_empty() {
        warn!("No cycle entries found");
        return Ok(());
    }

    let foreground = unsafe { GetForegroundWindow() };

    let active_index = entries
        .iter()
        .position(|e| e.hwnd == foreground)
        .unwrap_or(0);

    let target_index = match direction {
        CycleDirection::Forward if active_index + 1 >= entries.len() => 0,
        CycleDirection::Forward => active_index + 1,
        CycleDirection::Backward if active_index == 0 => entries.len() - 1,
        CycleDirection::Backward => active_index - 1,
    };

    let target = &entries[target_index];

    debug!(
        "Cycling {} from [{}] '{}' -> [{}] '{}' (grouped={})",
        match direction {
            CycleDirection::Forward => "[Forward]",
            CycleDirection::Backward => "[Backward]",
        },
        active_index,
        truncate(&entries[active_index].name, 30),
        target_index,
        truncate(&target.name, 30),
        target.is_grouped,
    );

    let ok = unsafe { switcher::force_activate(target.hwnd) };
    if !ok {
        warn!("force_activate returned false (foreground lock may be active)");
    }

    Ok(())
}

fn print_help(verbose: bool, uncombine_enabled: bool) {
    let mut info = String::from(
        "\nTaskbar Switcher started:\
        \n\tAlt+[  : cycle left\
        \n\tAlt+]  : cycle right\
        \n\tCtrl-C : quit\
        \n\
        \n\t-v/--verbose: enable debug logging\
        \n\t--uncombine: enable uncombine mode",
    );

    if verbose {
        info.push_str("\nEnable verbose logging");
    }

    if uncombine_enabled {
        info.push_str("\nEnable uncombine mode");
    }

    println!("{}\n", info);
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let verbose = args.iter().any(|arg| arg == "-v" || arg == "--verbose");
    let combine_enabled = args.iter().any(|a| a == "--combine-mode");

    print_help(verbose, combine_enabled);

    let _file_graud = logging::setup_logger(verbose);
    let enumerator = TaskbarEnumerator::new()?;
    let hotkey_manager = HotkeyManager::new()?;

    let main_thread_id = unsafe { GetCurrentThreadId() };
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        info!("Ctrl-C received, restoring AppUserModelIDs...");
        r.store(false, Ordering::SeqCst);
        unsafe {
            let _ = PostThreadMessageW(
                main_thread_id,
                WM_QUIT,
                WPARAM::default(),
                LPARAM::default(),
            );
        }
    })
    .unwrap_or_else(|e| error!("Ctrl-C handler failed: {e}"));

    unsafe {
        winevent::install_hook()?;
        let mut msg = std::mem::zeroed();

        if !combine_enabled {
            uncombine::uncombine_all_windows();
        }

        while running.load(Ordering::SeqCst) {
            let result = GetMessageW(&mut msg, None, 0, 0);

            if result.0 == 0 {
                winevent::uninstall_hook();
                hotkey_manager.unregister_all();

                if !combine_enabled {
                    uncombine::restore_all_app_user_model_ids();
                }

                break;
            }

            if result.0 == -1 {
                break;
            }

            match msg.message {
                WM_HOTKEY => match hotkey_manager.action_from_id(msg.wParam.0 as i32) {
                    Some(HotkeyAction::Left) => {
                        if let Err(e) =
                            cycle_taskbar(&enumerator, combine_enabled, CycleDirection::Backward)
                        {
                            error!("Error cycling taskbar: {e}");
                        }
                    }
                    Some(HotkeyAction::Right) => {
                        if let Err(e) =
                            cycle_taskbar(&enumerator, combine_enabled, CycleDirection::Forward)
                        {
                            error!("Error cycling taskbar: {e}");
                        }
                    }
                    None => continue,
                },
                winevent::WM_APP_UNCOMBINE if !combine_enabled => {
                    let hwnd = HWND(msg.wParam.0 as *mut _);
                    uncombine::uncombine_new_window(hwnd);
                }
                _ => {}
            }
        }
    }

    info!("Taskbar Switcher stopped");
    Ok(())
}
