# AGENTS.md - WinGlide

## Project summary

Rust (edition 2021) app for **Windows 11 only**, version 0.0.9. Single binary (`WinGlide.exe`), no workspace/monorepo. Features:

- Cycle through taskbar buttons via global hotkeys (`Alt+[` / `Alt+]`)
- Uncombine taskbar buttons (unique AUMID per window)
- On-screen virtual desktop indicator (drawn on the taskbar)
- Jump to virtual desktop via `Alt+1`..`Alt+9`
- System tray icon + Settings GUI
- Update check against GitHub Releases (MSI/ZIP builds; the Store updates the package itself)

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
cd tools/icons && bun install && bun run gen   # regenerate the .ico assets from the SVG sources
cargo build --release --no-default-features --features store   # Store flavour (no GitHub updater)
powershell -ExecutionPolicy Bypass -File packaging/build-msix.ps1   # MSIX -> target\msix\WinGlide_<version>_x64.msix
```

No lint/formatter config exists in the repo - only `cargo check` / `cargo build` are available.

### Icon assets

`assets/tray-light.ico`, `assets/tray-dark.ico` and `assets/app.ico` are generated
by `tools/icons/gen.ts` and committed, so building the app never needs Bun. Each
tray ICO holds seven entries (16/20/24/28/32/40/48 px); `TrayIcon` parses the
ICOs directory itself and hands the shell an HICON that already matches the
taskbar's DPI, instead of letting Windows downscale one large bitmap (the
`LookupIconIdFromDirectoryEx` helper returns the first entry for in-memory ICO
data, so it is deliberately not used). `TrayIcon::resync` rebuilds the icon when
the taskbar DPI or the system theme changes - it runs on `WM_SETTINGCHANGE`,
`WM_DPICHANGED`, `WM_DISPLAYCHANGE`, `WM_THEMECHANGED` and after
`TaskbarCreated`, and no-ops when nothing relevant changed. Sizes tuned by hand
live in `assets/overrides/tray-<size>.png`; the rest are rasterized from
`assets/tray-icon.svg`. The dark variant keeps the alpha channel and paints it
white, so only one set of source files exists. `tools/icons/README.md` documents
the size table and the override rules.

`assets/app.ico` holds ten entries (16..256) and is the icon Windows shows for the
exe itself. `build.rs` embeds it plus the VERSIONINFO block through `winresource`;
the resource script deliberately adds no manifest, because `windows-reactor-setup`
already links the WinRT SxS manifest via `/MANIFESTINPUT` and Windows rejects a
second one.

The same generator also writes `packaging/Assets/` (47 PNGs: the plated tiles plus
`Square44x44Logo.targetsize-<n>` in normal, `_altform-unplated` and
`_altform-lightunplated` forms). Those are committed too, so a package build needs
no Bun either.

### MSIX packaging

`packaging/` builds the Microsoft Store package; the MSI and ZIP channels are
unchanged and `packaging/README.md` is the user-facing guide. What an agent needs
to know:

- `packaging/AppxManifest.xml` is a template - `build-msix.ps1` substitutes
  `{{IDENTITY_NAME}}`, `{{PUBLISHER}}`, `{{PUBLISHER_DISPLAY_NAME}}` and
  `{{VERSION}}` (from `Cargo.toml`, padded to four parts). The three identity
  values come from `MSIX_IDENTITY_NAME` / `MSIX_PUBLISHER` /
  `MSIX_PUBLISHER_DISPLAY_NAME` in the environment, falling back to a gitignored
  root `.env`; `packaging/identity.example.env` is the template. They are public
  data, but a fork must build under its own publisher.
- The script must rewrite `resources.pri`: Windows resolves the
  `_altform-unplated` logos through the index, so without it Start and the taskbar
  paint a coloured plate behind the icon. Indexing the whole stage fails on
  duplicate `Microsoft.UI.Xaml/Assets` entries, so the index is built from a
  temporary directory containing only `Assets/` and merged into the SDK one.
- `uap:VisualElements BackgroundColor` must be a real colour: `transparent` makes
  Windows fill the tile with the system accent colour.
- Runtime differences are behind `shared::package::is_packaged()`:
  `app::autostart` uses `StartupTask::GetAsync` with the manifest's `TaskId`
  when packaged - a packaged process writes the Run key into a private registry
  hive the shell never reads - and the plain Run key otherwise.
- Build-time differences are behind the `updater` / `store` Cargo features. They
  are mutually exclusive and `default = ["updater"]`: plain `cargo build` gives
  the MSI/ZIP flavour with the GitHub updater, `--no-default-features --features
  store` compiles the updater and `reqwest` out for the Store package, whose
  updates come from Windows instead. `main.rs` fails the build with a
  `compile_error!` when neither or both are selected.
- `release.yml` builds the package after the MSI and ZIP steps and uploads it as
  the `WinGlide-msix` workflow artifact - never to the GitHub release, because an
  unsigned `.msix` only installs in developer mode. The step is skipped with a
  warning when the `MSIX_IDENTITY_*` secrets are missing.
- `build-msix.ps1` builds into `target\msix\cargo` (`CARGO_TARGET_DIR`), so the
  Store flavour never overwrites the MSI/ZIP build in `target\release`.
- The package is signed by default with a self-signed certificate kept in
  `target\msix\` - `Add-AppxPackage -Path` refuses an unsigned one with
  `0x800B0100` - and CI passes `-NoSign` because the Store re-signs the upload.
- `-Register` is the no-admin local loop (`Add-AppxPackage -Register` on
  `target/msix/stage/AppxManifest.xml`, Developer Mode required). A signed install
  additionally needs the dev certificate in `LocalMachine\TrustedPeople` -
  `CurrentUser` does not work (`0x800B0109`). Reinstalling the same version needs
  `Remove-AppxPackage` first (`0x80073CFB`).

## CLI args (manual parsing in `cli/args.rs`, no clap)

- `-v` / `--verbose` - enable debug-level logging
- `--debug` - attach/alloc console for debug logging (also enables console worker)
- `--console-worker` - run as standalone debug console worker process
- `--settings-ui` - launch only the Settings GUI (XAML)

`Args.combine_enabled` field exists but is never set by any flag (dead).

## Architecture

The crate is **feature-based**: only `main.rs` lives at `src/`, and every feature is a
directory whose `mod.rs` is a pure table of contents (no definitions). Files are named
after what they contain - a feature's primary type lives in a file named after that type,
shared data structs live in `types.rs`, small helpers in `utils.rs`.

```
main.rs                     -> panic hook -> dispatch: cli::parse_args -> app (single-instance, DPI, debug console) -> mode routing
app/
├── mod.rs                  -> re-export: App, ensure_single_instance, InstanceType, setup_dpi_awareness
├── orchestrator.rs         -> App: wires hotkey_manager + key_hook + enumerator + uncombine_manager + tray + indicator + hidden window
├── autostart.rs            -> enable/disable start at login: StartupTask when packaged, HKCU\...\Run otherwise
├── update_checker.rs       -> GitHub Releases API check (reqwest blocking); MSI/ZIP builds only
├── messages.rs             -> custom message IDs: WM_APP_UNCOMBINE/INVALIDATE_CACHE/RELOAD_CONFIG, WM_USER_TRAYICON
├── single_instance.rs      -> ensure_single_instance (named mutex) + InstanceType
└── dpi_awareness.rs        -> setup_dpi_awareness (Per-Monitor V2)
app_setting/                -> user settings: what is stored + the window that edits it
├── mod.rs                  -> re-export: AppConfig, run, show_ui
├── app_config.rs           -> AppConfig: serde JSON at %APPDATA%/WinGlide/config.json (see "Config")
├── settings_window.rs      -> windows-reactor native GUI + IPC to the background app
├── hotkey_button.rs        -> hotkey-capture control
├── setting_item.rs         -> reusable settings row widget
└── window_icon.rs          -> WM_SETICON for the WinUI title bar (which ignores the exe icon)
cli/
├── mod.rs                  -> re-export: parse_args, print_help, RunMode
└── args.rs                 -> manual arg parsing (Args / RunMode: ConsoleWorker / SettingsUi / BackgroundApp)
hotkey/
├── mod.rs                  -> re-export: HotkeyManager, HotkeyAction, LowLevelKeyHook
├── manager.rs              -> HotkeyManager: RegisterHotKey + dispatch classification (CycleLeft/CycleRight/SwitchVirtualDesktop)
└── low_level_hook.rs       -> generic WH_KEYBOARD_LL hook: overrides Windows-owned Win+key combos; posts WM_HOTKEY with the same IDs
event/
├── mod.rs                  -> re-export: WinEventHook, UiaEventHook, InvalidateSource, reset_cache_invalidated_flag
├── uia.rs                  -> UIA StructureChanged hook -> WM_APP_INVALIDATE_CACHE
└── winevent.rs             -> WinEvent EVENT_OBJECT_SHOW hook -> WM_APP_UNCOMBINE
taskbar/
├── mod.rs                  -> re-export: TaskbarEnumerator, CycleDirection, UncombineManager, Taskbar, TaskbarEdge, TaskbarButton
├── snapshot.rs             -> Taskbar/TaskbarEdge: edge, thickness, DPI, labels mode of Shell_TrayWnd
├── enumerator.rs           -> IUIAutomation: enumerate buttons, per-monitor 1s TTL cache, cycle_to_neighbor
├── slot.rs                 -> SlotPlan: exact button identity (`Appid: <AUMID>` / `TaskbarSwitcher_<hwnd>`) -> windows
├── cycle_state.rs          -> CycleState: cursor, flattened cycle order, blocked-window bookkeeping
├── uncombine.rs            -> UncombineManager: sets unique AppUserModelID per window
├── explorer_process.rs     -> explorer.exe PID cache (distinguishes shell buttons from app buttons)
├── types.rs                -> TaskbarButton
└── utils.rs                -> clean_button_name, normalize_button_label
window/
├── mod.rs                  -> re-export: WindowInfo, LogicalWindow
├── types.rs                -> WindowInfo (+ UWP_FRAME_CLASS / UWP_CONTENT_CLASS)
├── enumerate.rs            -> EnumWindows: find_visible_windows, find_window_by_hwnd, get_process_name
├── logical_window.rs       -> LogicalWindow: groups a UWP frame with its content window; group_logical_windows
├── activate.rs             -> activate (verified activation ladder) + force_activate (plain HWND path for VD jumps)
├── aumid.rs                -> get/set window AUMID (SHGetPropertyStoreForWindow)
├── context.rs              -> WindowContext::current_state(): foreground window + its monitor + virtual desktop
└── system_class.rs         -> is_system_class (shell/OS classes to ignore)
virtual_desktop/
├── mod.rs                  -> re-export: indicator
└── indicator/              -> IndicatorWindow: layered window drawing desktop dots on the taskbar (winvd)
                              left-click = switch desktop, right-click = "Move to Desktop" menu, Alt+click = move window there
                              (IndicatorSettings mirrors the config; also DotPainter / IndicatorGeometry / Canvas / DesktopMover)
tray/
├── mod.rs                  -> re-export: TrayIcon, IDM_EXIT, IDM_SETTINGS, IDM_SHOW_CONSOLE
└── tray_icon.rs          -> Shell_NotifyIconW tray icon + DPI-aware HICON selection + context menu
logging/
├── mod.rs                  -> re-export: setup_logger, CleanFormatter, ConsoleWriter
├── logger.rs               -> tracing-subscriber stack: rolling file + forest console
├── formatter.rs            -> console formatting + pipe writer
├── console.rs              -> detached console window over a named pipe
└── debug_console.rs        -> attach_debug_console (--debug)
shared/                     -> cross-cutting helpers used by more than one feature
├── package.rs              -> is_packaged() via GetCurrentPackageFullName (MSIX vs MSI/ZIP)
├── system_theme.rs         -> is_light_theme
└── text.rs                 -> truncate
```

## Config

`AppConfig` (`app_setting/app_config.rs`) is serialized to `%APPDATA%/WinGlide/config.json` (via `dirs::config_dir()`). Loaded at startup in `main.rs`, reloaded by `WM_APP_RELOAD_CONFIG`. Fields:

- `uncombine_mode: bool` (default true)
- `cycle_taskbar_based: bool` (default true)
- `hotkey_left_vk` / `hotkey_left_modifiers`, `hotkey_right_vk` / `hotkey_right_modifiers` (default Alt+`[` / Alt+`]`)
- `desktop_indicator: bool` (default true)
- `indicator_position: u8` (0=Auto, 1=Left, 2=Right; serde default 0 so old configs keep parsing)
- `indicator_accent_color: bool` (default true; paints the dots with `HKCU\Software\Microsoft\Windows\DWM\AccentColor` (ABGR DWORD), falling back to the theme color when that value is absent)
- `indicator_background: bool` (default true; draws a translucent rounded pill behind the dot row)
- `jump_desktop_modifiers: u32` (default Alt)

**Invariant:** in `load()`, if `cycle_taskbar_based` is true then `uncombine_mode` is forced to true.

## Key facts agents will miss

### COM threading

- App runs **STA apartment** (`CoInitializeEx(COINIT_APARTMENTTHREADED)`). The Windows message loop (`GetMessageW` in `App::run`) must run on the same thread that initialized COM.

### UncombineManager lifetime

- `UncombineManager` is `Box::leak`'d in `App::new()` to get a `&'static` reference. This is intentional - the WinEvent callback thread accesses it via `AtomicPtr<UncombineManager>`.

### Cache invalidation

- Button cache (1s TTL) is invalidated by UIA `StructureChanged` events (not WinEvent). A `CACHE_INVALIDATED` AtomicBool prevents posting duplicate `WM_APP_INVALIDATE_CACHE` messages when multiple UIA events fire in rapid succession.
- The cache records the monitor it was built for; with a taskbar on several monitors, a cache that only stored buttons would serve another monitor's buttons for up to a second.

### Explorer restart recovery

- `TaskbarEnumerator::enumerate_buttons()` catches `EVENT_E_ALL_SUBSCRIBERS_FAILED (0x80040201)` and auto-recovers via `refresh_taskbar_hwnd()` - re-finds Shell_TrayWnd, re-subscribes UIA hooks, invalidates explorer PID cache.

### Cycle model (taskbar/slot.rs + taskbar/cycle_state.rs)

The cycle is **identity based and stateful**. There is no fuzzy "guess which window belongs to this button" pass on the hot path, and no stale index fallback.

1. `window/logical_window.rs` turns raw top-level windows into `LogicalWindow`s. A UWP/packaged app is **one** logical window: `ApplicationFrameWindow` (the primary, the only handle that can be activated) plus its `Windows.UI.Core.CoreWindow` content window, which is normally a child of the frame and only sometimes enumerated as a top-level sibling. `LogicalWindow::contains` resolves child handles through `GetAncestor(.., GA_ROOT)` and owned dialogs through `GA_ROOTOWNER`, so either handle counts as the same app window.
2. `taskbar/slot.rs` builds one `Slot` per UIA button. Resolution order:
   - `Appid: TaskbarSwitcher_<hwnd>` - the exact handle written by the uncombine feature (checked against the window's AUMID);
   - `Window: 0x<hwnd>` - the exact handle the shell exposes on a **labelled** taskbar (Windows 11 "never combine" with labels);
   - `Appid: <AUMID>` - exact case-insensitive AUMID equality with any member window (never `contains`);
   - no usable id - PID/title/process fallback, restricted to windows no exact slot claimed.
   A window belongs to at most one slot. A slot with several windows therefore means the shell itself groups them (same AUMID).
3. `taskbar/cycle_state.rs` flattens slots left-to-right, expanding a grouped button in place by ascending handle (stable; never z-order, which would make a group oscillate). The source is the focused window when it resolves to a slot, otherwise the stored `cursor`. The cursor records the last target (successful or not) and windows that failed to activate are blocked for 10s, so an app that refuses activation can never trap the cycle.
4. `window/activate.rs` tries, in order, verifying after each step: restore if minimized -> `SetForegroundWindow` -> attach input to the foreground/target thread + `SetActiveWindow`/`SetFocus` -> `SwitchToThisWindow` -> ALT tap (documented to unlock `SetForegroundWindow`) -> click the taskbar button. The click is only passed for single-window slots, because clicking a grouped button opens the thumbnail flyout instead of switching. Verification compares the logical window's members and top-level ancestors (`GA_ROOT` / `GA_ROOTOWNER`), never raw `HWND` equality.

Things that look like details but matter:

- Windows 11's button `automation_id` is `Appid: <AUMID>` for combined buttons, or `Window: 0x<hwnd>` on a labelled taskbar; `slot.rs` normalizes and parses both. Never treat a `Window:` id as an AUMID: doing so silently drops the exact handle and forces the fuzzy fallback, which is what made labelled layouts skip buttons and swap same-titled instances.
- Button names are **localized**. They are only used by the fallback path and diagnostics; never build new logic on the English `"running window"` text.
- The monitor is taken from the **foreground window**, not the cursor (`window/context.rs`); the cursor only decides when there is no foreground window.
- `AllowSetForegroundWindow` was removed from activation: per its documentation it only works for a process that can already set the foreground window, so calling it from a background app is a no-op.
- Debugging the mapping: `cargo test print_slot_plan -- --ignored --nocapture` prints the live slots (button name, identity key, resolved windows) without touching any window.

### Logging output

- Logs go to a **rolling file** (`./logs/WinGlide*.log` via `tracing-appender`) and - in debug/console-worker mode - a detached console window via named pipes (`logging/console.rs`). Use `tracing_forest` for tree-structured output (custom formatter in `logging/formatter.rs`).

### Dependencies

- `windows` 0.61 (features from `Win32_UI_Accessibility`, `Win32_UI_Shell_PropertiesSystem`, `Win32_UI_WindowsAndMessaging`, etc., plus `ApplicationModel` for the packaged startup task)
- `windows-reactor` (git from microsoft/windows-rs master) - native GUI for the Settings UI
- `winvd` - virtual desktop API (detect/switch desktops, DesktopEventThread)
- `serde`/`serde_json` - config persistence
- `reqwest` (blocking, optional, feature `updater`) - GitHub update check; the
  `store` build is compiled without it
- `anyhow`, `tracing` stack, `once_cell`, `dirs`, `nu-ansi-term`

### Windows 11 only

- Relies on XAML taskbar class `Taskbar.TaskListButtonAutomationPeer`. Will not work on Windows 10 (which uses `ToolbarWindow32`). Supports both **primary** (`Shell_TrayWnd`) and **secondary** monitors (`Shell_SecondaryTrayWnd`).

### Hotkey IDs

- ID 1 = Left cycle, ID 2 = Right cycle (both only registered when `cycle_taskbar_based` is true)
- IDs 11..=19 = `SwitchVirtualDesktop(0..8)` (`Alt+1`..`Alt+9`), only registered when `jump_desktop_modifiers != 0`
- All configurable via Settings GUI; `HotkeyManager::reload()` re-registers on `WM_APP_RELOAD_CONFIG`

### Low-level hotkey dispatch (hotkey/low_level_hook.rs)

- `HotkeyManager` classifies every hotkey via `dispatch_for(modifiers)`: any combo **involving Win** gets `HotkeyDispatch::LowLevelHook`, everything else stays on `RegisterHotKey` (which cannot claim `Win+1..Win+9` because they are Windows-native taskbar shortcuts, nor reserved combos like `Win+L`).
- `HotkeyManager::low_level_hotkeys()` hands the Win combos to `LowLevelKeyHook::set_hotkeys()` (called from `App::new` and `handle_reload_config`, both on the main message-loop thread).
- The generic `WH_KEYBOARD_LL` proc matches `(modifiers, vk)` with exact modifier semantics (mirrors `RegisterHotKey`), swallows the key (`LRESULT(1)`) so the native Windows behavior never fires, and posts `WM_HOTKEY` with the same hotkey ID, so `App::handle_hotkey` / `action_from_id` work unchanged for both mechanisms (including auto-repeat drain).
- Hook state is a `thread_local` (the hook proc only runs on the installing thread); add a future low-level hotkey by assigning it `LowLevelHook` in `dispatch_for`/registration - no other wiring needed.
- The hook ignores `LLKHF_INJECTED` events. The activation ladder taps ALT and can click the taskbar; reacting to our own synthetic input would re-enter hotkey dispatch.

### Custom window messages

All IDs are defined in `app/messages.rs`; the event hooks and the Settings UI import them from there.

- `WM_APP_UNCOMBINE = WM_USER + 0x100` - uncombine a new window (posted to the main thread)
- `WM_APP_INVALIDATE_CACHE = WM_USER + 0x101` - invalidate button cache (posted to the main thread)
- `WM_APP_RELOAD_CONFIG = WM_USER + 0x102` - signal background process to reload configuration (posted to hidden window `WinGlideTray`)
- `WM_USER_TRAYICON = WM_USER + 0x200` - tray icon callback message

### Single-instance & IPC

- Two named mutexes: `Global\WinGlide_BackgroundMutex` and `Global\WinGlide_SettingsUIMutex`.
- The Settings UI and tray actions talk to the background app by `PostMessageW` to the hidden window class `WinGlideTray` (reload config, restart as admin, tray commands).
- Hidden window has `WS_EX_LAYERED | WS_EX_TOOLWINDOW`, class `WinGlideTray`; the `App` pointer is stashed in `GWLP_USERDATA` for the static `window_proc`.

### Hotkey auto-repeat protection

- Holding a key floods the queue with `WM_HOTKEY`. After handling a hotkey, `App::handle_hotkey` drains queued `WM_HOTKEY` messages with `PeekMessageW(PM_REMOVE)` to prevent runaway cycling.

### Virtual desktops (winvd)

- `switch_desktop` + `WindowContext::current_state()` are used to re-activate a window on the target desktop after switching (`App::handle_hotkey` -> `SwitchVirtualDesktop`).
- `IndicatorWindow` is an owned layered window of `Shell_TrayWnd`; it gets cloaked by DWM when Task View (`Win+Tab`) opens - known limitation.
- **Move window to desktop via indicator**: right-clicking a desktop dot opens a menu for exactly that dot's desktop N, with the window title in a disabled header and two short items: "Move to Desktop N" (move only) and "Move and jump to Desktop N" (move + switch + re-activate); the move-only item is grayed when the window is already on that desktop. The target window is captured before the menu temporarily drops `WS_EX_NOACTIVATE` (stored in `MOVE_TARGET_HWND` + `MOVE_TARGET_INDEX`); `force_activate` hands focus back to the app window after the menu closes, and the indicator's own class (`TaskbarSwitcherIndicator`) is filtered out of move targets so it can never be chosen as the window to move. `Alt`+click a dot is the move+jump keyboard shortcut. Requires the `transmute` of the HWND into winvd's `windows`-0.58 type. Pinned (all-desktops) windows are ignored.
- **Indicator placement** (`AppConfig.indicator_position`: 0 Auto / 1 Left / 2 Right): in **Auto** the indicator stays clear of the system tray when the taskbar is Left-aligned (`HKCU\\...\\Explorer\\Advanced\\TaskbarAl` = 0) by anchoring just left of `TrayNotifyWnd`, otherwise it sits at the left edge. **Left**/**Right** pin it to a fixed side (Right anchors just left of / above the tray). Changed from the Settings GUI (Desktop Indicator -> Position / Accent Color / Background), applied on `WM_APP_RELOAD_CONFIG` via `IndicatorWindow::set_config(IndicatorSettings)`. Position is decided at render time (no polling).
- **Indicator inset invariant**: on a vertical taskbar the offset along the bar is computed from the indicator's *height* (`IndicatorGeometry::physical_size().1`), **not** from its length along the dots. In label mode the dots form a horizontal row, so the height is `cross` while `length` is the row width; using `length` would push the `Right` anchor far above the tray.
- **Label-mode row**: a vertical taskbar showing labels gets a compact horizontal dot row, so `IndicatorGeometry` separates the dot *size reference* (`scale`, a standard taskbar thickness) from the window's short dimension (`cross`, = `compact_row_height(scale)` = the pill height). `Right` keeps the usual `INDICATOR_MARGIN` from the tray.
- **Dot color** (`AppConfig.indicator_accent_color`): when on, `DotPainter` reads the Windows accent color (`shared::system_theme::accent_color()`, from `HKCU\\...\\DWM\\AccentColor`) and blends it toward the theme's contrast color (`blend_for_theme`: `ACCENT_CONTRAST_BLEND` 0.35 toward black on a light taskbar, toward white on a dark one) so the dots stay legible in both themes; when off, or when the accent can't be read, it falls back to the theme-neutral color (near-black on light, white on dark). Inactive dots are desaturated (`desaturate`, 0.6) so the accent marks the current desktop instead of tinting the whole row.
- **Indicator background** (`AppConfig.indicator_background`): draws a translucent rounded pill (neutral theme color, alpha 0.25) behind the whole dot row. Its height is `PILL_HEIGHT_RATIO * scale` (0.5 => 24px at 100% DPI) in **every** layout, and the hover highlight uses the same half-height, so hovering can never spill outside the pill. Along the row the pill is the dot row plus `BACKGROUND_ALONG_MARGIN` on each side and `BACKGROUND_GROW_ALONG` of growth; the window reserves `BACKGROUND_ROOM_ALONG` so it is never clipped. The window's invisible hit-box still spans the full row either way.
- Hotkey auto-repeat protection
