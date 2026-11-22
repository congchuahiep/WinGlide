//! The Settings window: a `Component` that edits [`AppConfig`] row by row.
//!
//! Every change takes the same path - [`Settings::apply`]: re-read the config from
//! disk, apply the change, save it, then poke the background app so it reloads.
//! Re-reading instead of mutating a copy captured at render time is what the old
//! handlers did by hand; here it is one helper, so a change can never be computed
//! from a stale `AppConfig`.
//!
//! This module also owns the IPC helper that pokes the background app and the job
//! object that ties the UI process to it.

use crate::app_setting::hotkey_button;
use crate::app_setting::setting_item::SettingItem;
use crate::app_setting::AppConfig;
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_WIN};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows_core::w;
use windows_reactor::*;

/// Sends a signal to reload the configuration to the background process
fn send_reload_signal() {
    unsafe {
        if let Ok(hwnd) = windows::Win32::UI::WindowsAndMessaging::FindWindowW(
            windows::core::w!("WinGlideTray"),
            windows::core::PCWSTR::null(),
        ) {
            if !hwnd.is_invalid() {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(hwnd),
                    crate::app::messages::WM_APP_RELOAD_CONFIG,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
    }
}

/// The two hotkey slots, so a capture can be attributed to one of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CycleHotkey {
    Left,
    Right,
}

/// State of the update check. Compiled out of the Store build, which updates
/// through Windows and may not offer a GitHub download.
#[cfg(feature = "updater")]
#[derive(Default)]
struct UpdateCheck {
    checking: bool,
    /// Version of the newest release, once a check has found one.
    available: Option<String>,
}

#[derive(Clone)]
enum Message {
    UncombineChanged(bool),
    CycleTaskbarChanged(bool),
    StartCapture(CycleHotkey),
    Captured(CycleHotkey, Option<(u32, u32)>),
    DesktopIndicatorChanged(bool),
    IndicatorPositionChanged(Option<usize>),
    AccentColorChanged(bool),
    BackgroundChanged(bool),
    TrayBadgeChanged(bool),
    JumpModifierChanged(u32, bool),
    AutostartChanged(bool),
    #[cfg(feature = "updater")]
    CheckForUpdates,
    #[cfg(feature = "updater")]
    UpdateChecked(Option<String>),
}

struct Settings {
    config: AppConfig,
    autostart: bool,
    /// Which cycle hotkey is being captured, if any. Owned here because a
    /// `WH_KEYBOARD_LL` hook is a process-wide singleton: one capture at a time,
    /// and the buttons only reflect it.
    listening: Option<CycleHotkey>,
    #[cfg(feature = "updater")]
    update: UpdateCheck,
}

impl Settings {
    /// Applies one change to the persisted config and tells the background app.
    ///
    /// Loading first (rather than mutating the copy rendered last) keeps this window
    /// from overwriting an edit the file received in the meantime.
    fn apply(&mut self, change: impl FnOnce(&mut AppConfig)) {
        let mut config = AppConfig::load();
        change(&mut config);
        config.save();
        self.config = config;
        send_reload_signal();
    }

    fn taskbar_section(&self, context: &ViewContext<Self>) -> View {
        // One cycle button: the combination it holds, and a capture while it is the
        // slot being listened to.
        let cycle = |slot: CycleHotkey, title: &str| {
            let (modifiers, vk) = match slot {
                CycleHotkey::Left => (
                    self.config.hotkey_left_modifiers,
                    self.config.hotkey_left_vk,
                ),
                CycleHotkey::Right => (
                    self.config.hotkey_right_modifiers,
                    self.config.hotkey_right_vk,
                ),
            };
            SettingItem::new(title)
                .enabled(self.config.cycle_taskbar_based)
                .action(hotkey_button::hotkey_button(
                    (modifiers, vk),
                    self.config.cycle_taskbar_based,
                    self.listening == Some(slot),
                    context.message(Message::StartCapture(slot)),
                ))
        };

        StackPanel::new().spacing(4.0).children((
            section_title("Taskbar"),
            SettingItem::new("Uncombine taskbar buttons")
                .icon('\u{E7C4}')
                .description("Disallow group windows on the taskbar")
                .action(toggle(
                    self.config.uncombine_mode,
                    // Cycling forces uncombining, so the switch is not the
                    // user's to move while cycling is on.
                    !self.config.cycle_taskbar_based,
                    Message::UncombineChanged,
                    context,
                )),
            SettingItem::new("Cycle taskbar buttons")
                .icon('\u{E8AB}')
                .description("Switch windows based on taskbar buttons")
                .action(toggle(
                    self.config.cycle_taskbar_based,
                    true,
                    Message::CycleTaskbarChanged,
                    context,
                ))
                .children([
                    SettingItem::note("By default, 'Cycle taskbar buttons' automatically uncombines buttons because it handles button groups poorly")
                        .enabled(self.config.cycle_taskbar_based),
                    cycle(CycleHotkey::Left, "Cycle left"),
                    cycle(CycleHotkey::Right, "Cycle right"),
                ]),
        ))
    }

    fn virtual_desktop_section(&self, context: &ViewContext<Self>) -> View {
        let indicator = self.config.desktop_indicator;
        // A config written by hand can hold anything; the old window showed such a
        // value as Auto, so keep that instead of an empty picker.
        let position = match self.config.indicator_position {
            1 => 1,
            2 => 2,
            _ => 0,
        };
        // Ctrl / Alt / Win: the jump hotkey is whatever combination of these bits is
        // set.
        let modifier = |label: &str, flag: u32| {
            ToggleButton::new()
                .is_checked(self.config.jump_desktop_modifiers & flag != 0)
                .on_is_checked_changed(
                    context.callback(move |checked| Message::JumpModifierChanged(flag, checked)),
                )
                .content(label)
        };

        StackPanel::new().spacing(4.0).children((
            section_title("Virtual Desktop"),
            SettingItem::new("Desktop Indicator")
                .icon('\u{E712}')
                .description("Show the virtual desktop indicator on the taskbar")
                .action(toggle(
                    self.config.desktop_indicator,
                    true,
                    Message::DesktopIndicatorChanged,
                    context,
                ))
                .children([
                    SettingItem::note("Click a dot to switch to that desktop. Hold Alt + click to move the current window there. Right-click a dot to open a 'Move to Desktop' menu")
                        .enabled(indicator),
                    SettingItem::new("Position")
                        .description("Placement of the indicator on the taskbar")
                        .enabled(indicator)
                        .action(
                            ComboBox::new()
                                .items_source(["Auto", "Left", "Right"])
                                .selected_index(position)
                                .is_enabled(indicator)
                                .width(120.0)
                                .margin(Thickness::new(0.0, 0.0, 10.0, 0.0))
                                .on_selection_changed(
                                    context.callback(Message::IndicatorPositionChanged),
                                ),
                        ),
                    SettingItem::new("Accent Color")
                        .description("Use the Windows accent color for the dots")
                        .enabled(indicator)
                        .action(toggle(
                            self.config.indicator_accent_color,
                            indicator,
                            Message::AccentColorChanged,
                            context,
                        )),
                    SettingItem::new("Background")
                        .description("Draw a translucent background behind the dots")
                        .enabled(indicator)
                        .action(toggle(
                            self.config.indicator_background,
                            indicator,
                            Message::BackgroundChanged,
                            context,
                        )),
                ]),
            SettingItem::new("Tray Icon Badge")
                .icon('\u{EC1B}')
                .description("Draw the current desktop number on the tray icon")
                // Independent of the dots: the badge needs the indicator's
                // desktop thread, not the indicator itself.
                .action(toggle(
                    self.config.tray_badge,
                    true,
                    Message::TrayBadgeChanged,
                    context,
                )),
            SettingItem::new("Jump to Desktop")
                .icon('\u{E7B5}')
                .description("Change the desktop by index. e.g Alt+1, Alt+2")
                .action(
                    StackPanel::new()
                        .orientation(Orientation::Horizontal)
                        .spacing(2.0)
                        .margin(Thickness::new(0.0, 0.0, 10.0, 0.0))
                        .children((
                            modifier("Ctrl", MOD_CONTROL.0),
                            modifier("Alt", MOD_ALT.0),
                            modifier("Win", MOD_WIN.0),
                        )),
                ),
        ))
    }

    fn system_section(&self, context: &ViewContext<Self>) -> View {
        StackPanel::new().spacing(4.0).children((
            section_title("System"),
            SettingItem::new("Start with Windows")
                .icon('\u{E7E8}')
                .description("Automatically run this app when you log in")
                .action(toggle(
                    self.autostart,
                    true,
                    Message::AutostartChanged,
                    context,
                )),
        ))
    }

    #[cfg(feature = "updater")]
    fn update_section(&self, context: &ViewContext<Self>) -> View {
        let (description, action) = if let Some(version) = &self.update.available {
            (
                format!("New version {version} is available!"),
                Button::new().on_click(open_releases).content("Update Now"),
            )
        } else if self.update.checking {
            (
                "Checking...".to_string(),
                Button::new().is_enabled(false).content("Check"),
            )
        } else {
            (
                format!("Current version: {}", env!("CARGO_PKG_VERSION")),
                Button::new()
                    .is_enabled(true)
                    .on_click(context.message(Message::CheckForUpdates))
                    .content("Check"),
            )
        };

        StackPanel::new().spacing(4.0).children((
            section_title("Updates"),
            SettingItem::new("Check for updates")
                .icon('\u{E895}')
                .description(description)
                .action(action),
        ))
    }
}

impl Component for Settings {
    type Message = Message;
    type Input = ();

    fn create(_input: &(), _context: &ComponentContext<Self>) -> Self {
        Self {
            config: AppConfig::load(),
            autostart: crate::app::autostart::is_autostart_enabled(),
            listening: None,
            #[cfg(feature = "updater")]
            update: UpdateCheck::default(),
        }
    }

    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        match message {
            Message::UncombineChanged(on) => {
                self.apply(|config| config.uncombine_mode = on);
            }
            Message::CycleTaskbarChanged(on) => {
                self.apply(|config| {
                    config.cycle_taskbar_based = on;
                    // Cycling walks taskbar buttons; grouped buttons have none to
                    // walk, so turning cycling on uncombines.
                    if on {
                        config.uncombine_mode = true;
                    }
                });
            }
            Message::StartCapture(slot) => {
                self.listening = Some(slot);
                let captured = context
                    .sender()
                    .callback(move |result: Option<(u32, u32)>| Message::Captured(slot, result));
                hotkey_button::capture_hotkey(move |result| {
                    // The hook reports on this thread; the callback only queues.
                    let _ = captured.call(result);
                });
            }
            Message::Captured(slot, result) => {
                self.listening = None;
                if let Some((modifiers, vk)) = result {
                    // Capturing what the other slot already uses swaps the two, so the
                    // cycle hotkeys can never end up identical.
                    self.apply(|config| match slot {
                        CycleHotkey::Left => {
                            if config.hotkey_right_modifiers == modifiers
                                && config.hotkey_right_vk == vk
                            {
                                config.hotkey_right_modifiers = config.hotkey_left_modifiers;
                                config.hotkey_right_vk = config.hotkey_left_vk;
                            }
                            config.hotkey_left_modifiers = modifiers;
                            config.hotkey_left_vk = vk;
                        }
                        CycleHotkey::Right => {
                            if config.hotkey_left_modifiers == modifiers
                                && config.hotkey_left_vk == vk
                            {
                                config.hotkey_left_modifiers = config.hotkey_right_modifiers;
                                config.hotkey_left_vk = config.hotkey_right_vk;
                            }
                            config.hotkey_right_modifiers = modifiers;
                            config.hotkey_right_vk = vk;
                        }
                    });
                }
            }
            Message::DesktopIndicatorChanged(on) => {
                self.apply(|config| config.desktop_indicator = on);
            }
            // `None` means the picker was cleared, which the three fixed presets
            // never offer; ignore it rather than saving a meaningless position.
            Message::IndicatorPositionChanged(Some(position)) => {
                self.apply(|config| config.indicator_position = position as u8);
            }
            Message::IndicatorPositionChanged(None) => {}
            Message::AccentColorChanged(on) => {
                self.apply(|config| config.indicator_accent_color = on);
            }
            Message::BackgroundChanged(on) => {
                self.apply(|config| config.indicator_background = on);
            }
            Message::TrayBadgeChanged(on) => {
                self.apply(|config| config.tray_badge = on);
            }
            Message::JumpModifierChanged(flag, checked) => {
                self.apply(|config| {
                    if checked {
                        config.jump_desktop_modifiers |= flag;
                    } else {
                        config.jump_desktop_modifiers &= !flag;
                    }
                });
            }
            Message::AutostartChanged(on) => match crate::app::autostart::set_autostart(on) {
                Ok(()) => self.autostart = on,
                // Windows can refuse (a packaged app the user disabled in Task
                // Manager); log it instead of showing a switch that lies.
                Err(e) => tracing::warn!("autostart: {e}"),
            },
            #[cfg(feature = "updater")]
            Message::CheckForUpdates => {
                self.update.checking = true;
                let _ = context.spawn_background(|_cancellation| {
                    // reqwest blocking: off the UI thread, or the window stops
                    // responding for the length of the request.
                    let available = match crate::app::update_checker::check_for_updates() {
                        Ok(info) => info.map(|info| info.latest_version),
                        Err(e) => {
                            tracing::warn!("update check failed: {e}");
                            None
                        }
                    };
                    Message::UpdateChecked(available)
                });
            }
            #[cfg(feature = "updater")]
            Message::UpdateChecked(available) => {
                self.update.checking = false;
                self.update.available = available;
            }
        }
    }

    fn view(&self, _input: &(), context: &mut ViewContext<Self>) -> View {
        context.window_title("WinGlide");
        context.window_visuals(
            WindowVisuals::new()
                .backdrop(WindowBackdrop::Mica)
                .client_size(540.0, 720.0)
                .constraints(WindowConstraints {
                    min_width: Some(540.0),
                    min_height: Some(540.0),
                    max_width: None,
                    max_height: None,
                }),
        );

        // The Store build compiles the updater out entirely: Windows updates the
        // package, and offering a GitHub download inside a Store app is against
        // Store policy.
        #[cfg(feature = "updater")]
        let sections = (
            header(),
            self.taskbar_section(context),
            self.virtual_desktop_section(context),
            self.system_section(context),
            self.update_section(context),
            footer(),
        );
        #[cfg(not(feature = "updater"))]
        let sections = (
            header(),
            self.taskbar_section(context),
            self.virtual_desktop_section(context),
            self.system_section(context),
            footer(),
        );

        ScrollViewer::new().content(
            StackPanel::new()
                .spacing(32.0)
                .margin(Thickness::uniform(24.0))
                .children(sections),
        )
    }
}

/// A toggle switch wired to `message`, sized down to the 42px the rows reserve.
fn toggle(
    value: bool,
    enabled: bool,
    message: fn(bool) -> Message,
    context: &ViewContext<Settings>,
) -> View {
    ToggleSwitch::new()
        .is_on(value)
        .is_enabled(enabled)
        .width(42.0)
        .min_width(0.0)
        .on_toggled(context.callback(message))
        // The switch's own "On"/"Off" text would push the row wider than it is.
        .slots([
            SlotView::new(ToggleSwitchSlot::OnContent, ""),
            SlotView::new(ToggleSwitchSlot::OffContent, ""),
        ])
}

fn header() -> View {
    TextBlock::new()
        .text("Settings")
        .font_size(28.0)
        .font_weight(FontWeight::SEMI_BOLD)
        .into()
}

fn section_title(text: &str) -> View {
    TextBlock::new()
        .text(text)
        .font_size(14.0)
        .font_weight(FontWeight::SEMI_BOLD)
        .margin(Thickness::new(0.0, 0.0, 0.0, 10.0))
        .into()
}

fn footer() -> View {
    StackPanel::new().spacing(8.0).children((
        section_title("About"),
        TextBlock::new().text("Author: congchuahiep"),
        TextBlock::new().text(format!("Version: {}", env!("CARGO_PKG_VERSION"))),
        Button::new()
            .on_click(open_repository)
            .content("GitHub Repository"),
    ))
}

fn open_repository() {
    open_url(env!("CARGO_PKG_REPOSITORY"));
}

#[cfg(feature = "updater")]
fn open_releases() {
    open_url(crate::app::update_checker::RELEASES_URL);
}

fn open_url(url: &str) {
    unsafe {
        let url = windows::core::HSTRING::from(url);
        ShellExecuteW(None, w!("open"), &url, None, None, SW_SHOWNORMAL);
    }
}

/// Runs the settings UI application. Only run it in the main thread.
pub fn run() -> anyhow::Result<()> {
    // WinUI 3 windows do not inherit the executable's icon; give the title bar one.
    super::window_icon::apply();

    App::run_component::<Settings>(())?;
    Ok(())
}

/// Opens the settings UI by spawning a new process.
pub fn show_ui() {
    let exe_path = std::env::current_exe().expect("Failed to get executable path");

    let child = std::process::Command::new(exe_path)
        .arg("--settings-ui")
        .spawn()
        .expect("Failed to spawn settings UI process");

    static UI_JOB: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

    let job_handle = UI_JOB.get_or_init(|| unsafe {
        let job =
            CreateJobObjectW(None, windows::core::PCWSTR::null()).unwrap_or(HANDLE::default());
        if !job.is_invalid() {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let _ = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
        }
        job.0 as isize
    });

    unsafe {
        let job = HANDLE(*job_handle as *mut _);
        if !job.is_invalid() {
            let _ = AssignProcessToJobObject(job, HANDLE(child.as_raw_handle() as _));
        }
    }
}
