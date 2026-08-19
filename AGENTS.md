# AGENTS.md - WinGlide

## Project summary

Rust (edition 2021) app for **Windows 11 only**, version 0.0.9. Single binary (`WinGlide.exe`), no workspace/monorepo. Features:

- Cycle through taskbar buttons via global hotkeys (`Alt+[` / `Alt+]`)
- Uncombine taskbar buttons (unique AUMID per window)
- On-screen virtual desktop indicator (drawn on the taskbar)
- Jump to virtual desktop via `Alt+1`..`Alt+9`
- System tray icon + Settings GUI + auto-update check

Uses IUIAutomation because the Win11 taskbar is XAML, not Win32 HWNDs.

## Developer commands

```bash
cargo build --release              # binary -> target/release/WinGlide.exe
cargo build                        # debug build
cargo check                        # quick type-check (no codegen needed)
cargo run --release                # run
cargo run -- --debug --verbose     # run with debug console + verbose logging
./target/release/WinGlide.exe            # normal run
./target/release/WinGlide.exe -v         # with debug logging
./target/release/WinGlide.exe --settings-ui   # only open the Settings GUI
```

No lint/formatter config exists in the repo - only `cargo check` / `cargo build` are available.

## CLI args (manual parsing in cli.rs, no clap)

- `-v` / `--verbose` - enable debug-level logging
- `--debug` - attach/alloc console for debug logging (also enables console worker)
- `--console-worker` - run as standalone debug console worker process
- `--settings-ui` - launch only the Settings GUI (XAML)
- `--reopen-ui` - after starting the background app, reopen the Settings UI

`Args.combine_enabled` field exists but is never set by any flag (dead).

## Architecture

```
main.rs                     -> panic hook -> dispatch: cli::parse_args -> bootstrap (single-instance, DPI, debug console) -> mode routing
cli.rs                      -> manual arg parsing (RunMode: ConsoleWorker / SettingsUi / BackgroundApp)
config.rs                   -> AppConfig: serde JSON at %APPDATA%/WinGlide/config.json (see "Config")
bootstrap.rs                -> ensure_single_instance (named mutex), attach_debug_console, setup_dpi_awareness
app.rs                      -> orchestrator: wires hotkey_manager + enumerator + uncombine_manager + tray + indicator + hidden window
hotkey.rs                   -> RegisterHotKey; dispatches HotkeyAction::CycleLeft/CycleRight/SwitchVirtualDesktop(idx)
taskbar/
├── mod.rs                  -> module doc + re-export: TaskbarEnumerator, CycleDirection, UncombineManager
├── enumerator.rs           -> IUIAutomation: enumerate buttons, 1s TTL cache, cycle_to_neighbor
├── button_window.rs        -> ButtonWindowMap: map button ↔ window (AUMID -> PID -> Title -> Process)
└── uncombine.rs            -> UncombineManager: sets unique AppUserModelID per window
win32/
├── mod.rs                  -> re-exports win32 submodules
├── window.rs               -> EnumWindows: find_visible_windows, get_process_name
├── activate.rs             -> force_activate (SetForegroundWindow + AttachThreadInput)
├── aumid.rs                -> get/window AUMID helpers (SHGetPropertyStoreForWindow)
├── explorer.rs             -> get_explorer_pid, invalidate_explorer_pid_cache
└── window_context.rs       -> WindowContext::current_state(): foreground window + monitor + virtual desktop
event/
├── mod.rs                  -> re-exports; defines WM_APP_RELOAD_CONFIG (0x102), WM_APP_RESTART_AS_ADMIN (0x103)
├── uia.rs                  -> UIA StructureChanged hook -> WM_APP_INVALIDATE_CACHE (0x101)
└── winevent.rs             -> WinEvent EVENT_OBJECT_SHOW hook -> WM_APP_UNCOMBINE (0x100)
virtual_desktop/
└── indicator.rs            -> IndicatorWindow: layered window drawing desktop dots on the taskbar (winvd)
tray_icon.rs                -> Shell_NotifyIconW tray icon + context menu (Exit / Settings / Debug Console)
setting/                    -> windows-reactor native GUI settings (hotkey capture, toggles, update check)
logging/                    -> tracing-subscriber: rolling file + detached console via named pipes; tracing-forest format
admin.rs                    -> is_running_as_admin, restart_as_admin (ShellExecuteW "runas")
autostart.rs                -> HKCU\...\Run registry autostart enable/disable
updater.rs                  -> GitHub Releases API check (reqwest blocking), finds .msi asset
types.rs                    -> shared data structs (TaskbarButton, WindowInfo, TargetWindow), no logic, no imports
utils.rs                    -> clean_button_name, truncate, is_system_class, is_light_theme
```

## Config

`AppConfig` is serialized to `%APPDATA%/WinGlide/config.json` (via `dirs::config_dir()`). Loaded at startup in `main.rs`, reloaded by `WM_APP_RELOAD_CONFIG`. Fields:

- `uncombine_mode: bool` (default true)
- `cycle_taskbar_based: bool` (default true)
- `hotkey_left_vk` / `hotkey_left_modifiers`, `hotkey_right_vk` / `hotkey_right_modifiers` (default Alt+`[` / Alt+`]`)
- `desktop_indicator: bool` (default true)
- `jump_desktop_modifiers: u32` (default Alt)

**Invariant:** in `load()`, if `cycle_taskbar_based` is true then `uncombine_mode` is forced to true.

## Key facts agents will miss

### COM threading

- App runs **STA apartment** (`CoInitializeEx(COINIT_APARTMENTTHREADED)`). The Windows message loop (`GetMessageW` in `App::run`) must run on the same thread that initialized COM.

### UncombineManager lifetime

- `UncombineManager` is `Box::leak`'d in `App::new()` to get a `&'static` reference. This is intentional - the WinEvent callback thread accesses it via `AtomicPtr<UncombineManager>`.

### Cache invalidation

- Button cache (1s TTL) is invalidated by UIA `StructureChanged` events (not WinEvent). A `CACHE_INVALIDATED` AtomicBool prevents posting duplicate `WM_APP_INVALIDATE_CACHE` messages when multiple UIA events fire in rapid succession.

### Explorer restart recovery

- `TaskbarEnumerator::enumerate_buttons()` catches `EVENT_E_ALL_SUBSCRIBERS_FAILED (0x80040201)` and auto-recovers via `refresh_taskbar_hwnd()` - re-finds Shell_TrayWnd, re-subscribes UIA hooks, invalidates explorer PID cache.

### Matching strategies (button_window.rs)

Button-to-window matching tries 4 strategies in order:

1. **AppUserModelID** (button `automation_id` vs window `SHGetPropertyStoreForWindow`)
2. **PID** (if button PID ≠ explorer PID)
3. **Title** fuzzy match (after `clean_button_name` stripping)
4. **Process name** (`.exe` stem match, allows windows with empty titles)

### Logging output

- Logs go to a **rolling file** (`./logs/WinGlide*.log` via `tracing-appender`) and - in debug/console-worker mode - a detached console window via named pipes (`logging/console.rs`). Use `tracing_forest` for tree-structured output (custom formatter in `logging/formatter.rs`).

### Dependencies

- `windows` 0.61 (features from `Win32_UI_Accessibility`, `Win32_UI_Shell_PropertiesSystem`, `Win32_UI_WindowsAndMessaging`, etc.)
- `windows-reactor` (git from microsoft/windows-rs master) - native GUI for the Settings UI
- `winvd` - virtual desktop API (detect/switch desktops, DesktopEventThread)
- `serde`/`serde_json` - config persistence
- `reqwest` (blocking) - GitHub update check
- `anyhow`, `tracing` stack, `once_cell`, `dirs`, `nu-ansi-term`

### Windows 11 only

- Relies on XAML taskbar class `Taskbar.TaskListButtonAutomationPeer`. Will not work on Windows 10 (which uses `ToolbarWindow32`). Supports both **primary** (`Shell_TrayWnd`) and **secondary** monitors (`Shell_SecondaryTrayWnd`).

### Hotkey IDs

- ID 1 = Left cycle, ID 2 = Right cycle (both only registered when `cycle_taskbar_based` is true)
- IDs 11..=19 = `SwitchVirtualDesktop(0..8)` (`Alt+1`..`Alt+9`), only registered when `jump_desktop_modifiers != 0`
- All configurable via Settings GUI; `HotkeyManager::reload()` re-registers on `WM_APP_RELOAD_CONFIG`

### Custom window messages

- `WM_APP_UNCOMBINE = WM_USER + 0x100` - uncombine a new window (posted to the main thread)
- `WM_APP_INVALIDATE_CACHE = WM_USER + 0x101` - invalidate button cache (posted to the main thread)
- `WM_APP_RELOAD_CONFIG = WM_USER + 0x102` - signal background process to reload configuration (posted to hidden window `WinGlideTray`)
- `WM_APP_RESTART_AS_ADMIN = WM_USER + 0x103` - signal to restart app with admin privileges (posted to hidden window)
- `WM_USER_TRAYICON = WM_USER + 0x200` - tray icon callback message (in app.rs)

### Single-instance & IPC

- Two named mutexes: `Global\WinGlide_BackgroundMutex` and `Global\WinGlide_SettingsUIMutex`.
- The Settings UI and tray actions talk to the background app by `PostMessageW` to the hidden window class `WinGlideTray` (reload config, restart as admin, tray commands).
- Hidden window has `WS_EX_LAYERED | WS_EX_TOOLWINDOW`, class `WinGlideTray`; the `App` pointer is stashed in `GWLP_USERDATA` for the static `window_proc`.

### Hotkey auto-repeat protection

- Holding a key floods the queue with `WM_HOTKEY`. After handling a hotkey, `App::handle_hotkey` drains queued `WM_HOTKEY` messages with `PeekMessageW(PM_REMOVE)` to prevent runaway cycling.

### Virtual desktops (winvd)

- `switch_desktop` + `WindowContext::current_state()` are used to re-activate a window on the target desktop after switching (`App::handle_hotkey` -> `SwitchVirtualDesktop`).
- `IndicatorWindow` is an owned layered window of `Shell_TrayWnd`; it gets cloaked by DWM when Task View (`Win+Tab`) opens - known limitation.
- Hotkey auto-repeat protection
