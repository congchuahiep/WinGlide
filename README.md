# WinGlide

![Windows 11 Only](https://img.shields.io/badge/OS-Windows%2011-blue?logo=windows)
![Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)
![License](https://img.shields.io/badge/License-MIT-green)
![GitHub Release](https://img.shields.io/github/v/release/congchuahiep/WinGlide)
![GitHub Downloads (all assets, all releases)](https://img.shields.io/github/downloads/congchuahiep/WinGlide/total)

A powerful, lightweight utility built in Rust designed to enhance navigation and multitasking on Windows 11. It provides seamless keyboard-driven taskbar navigation, uncombines taskbar buttons, and lets you switch and move windows across virtual desktops.

[📥 Download the latest](https://github.com/congchuahiep/WinGlide/releases/latest)

## Motivation

I built WinGlide to supercharge productivity by keeping your hands on the keyboard. While native Windows shortcuts like `Alt + Tab` or `Win + Ctrl + D` exist, they can feel disconnected or visually cluttered when juggling many applications. WinGlide provides a fast, lightweight, keyboard-first approach to interact with your workspace.

## Features

| Feature                       | Description                                                                                                                                        | Demo                                                                                                                                          |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| **Cycle taskbar windows**     | Use <kbd>Alt</kbd> + <kbd>[</kbd> / <kbd>Alt</kbd> + <kbd>]</kbd> to instantly cycle through open applications on your taskbar (Left/Right).       | <video src="https://github.com/user-attachments/assets/46faf7d0-0227-45b4-b0c4-01e6a4e78797" controls preload="metadata" width="320"></video> |
| **Uncombine taskbar buttons** | Prevents taskbar buttons from being grouped together, giving you an individual button for each window.                                             | <video src="https://github.com/user-attachments/assets/c2d74899-5966-47dc-a678-f8df698e1485" controls preload="metadata" width="320"></video> |
| **Virtual desktop indicator** | A compact indicator on the taskbar shows which virtual desktop you're on; click a dot to switch.                                                   | <video src="https://github.com/user-attachments/assets/1d41a096-5139-45ff-8675-7cea09de6e1a" controls preload="metadata" width="320"></video> |
| **Jump to desktop**           | Quickly jump to a specific virtual desktop using <kbd>Alt</kbd> + <kbd>&lt;index&gt;</kbd> (starting from 1).                                      | <video src="https://github.com/user-attachments/assets/6f77acad-0fb5-42c3-adc2-1f8916c1f351" controls preload="metadata" width="320"></video> |
| **Move window to desktop**    | Right-click a desktop dot to move the current window there ("Move" or "Move and jump"), or <kbd>Alt</kbd> + click a dot for a quick move-and-jump. |                                                                                                                                               |
| **System Tray Integration**   | Easily manage the app via a convenient system tray menu.                                                                                           |                                                                                                                                               |
| **Lightweight & Fast**        | Built with Rust for maximum performance and minimal resource usage (<10Mb ram usage).                                                              |                                                                                                                                               |
| **Free**                      | Why not?                                                                                                                                           |                                                                                                                                               |

## Installation

You can download the latest version of WinGlide from our [GitHub Releases](https://github.com/congchuahiep/WinGlide/releases/latest) page.

We offer two ways to run the application:

- **Installer (`.msi`)**: The standard installation experience. Download the file, run it, and follow the setup wizard to install WinGlide on your system.
- **Portable (`.zip`)**: A standalone version requiring no installation. Simply download, extract the contents to your preferred folder, and run the executable directly.

> [!WARNING]
>
> WinGlide is currently not code-signed with a paid developer certificate. Because of this, Windows Defender SmartScreen or your antivirus software might flag the application as "unrecognized" or potentially malicious. This is a common false positive for new, open-source executables.
>
> If you trust the source code, you can bypass Windows SmartScreen by clicking **"More info"** and then **"Run anyway"**.
>
> **If you are unable to run or install the `.msi` file due to strict system policies or antivirus blocks, we recommend using the portable `.zip` version.**

## Configuration

WinGlide runs quietly in the background, but you can easily customize its behavior by right-clicking the **System Tray** icon:

<p align="center">
<img width="428" height="584" alt="image" src="https://github.com/user-attachments/assets/1ed6098f-a992-47e6-af19-0447ef9bb5b8" />
</p>

## Development

> "To build from source, make sure you have Rust and Cargo installed https://rustup.rs/."

```bash
# Build the application
cargo build

# Build release version
cargo build --release

# Run normally
cargo run --release

# Run with debug
cargo run -- --debug --verbose
```

## Technology Stack

| Component               | Technology                                  |
| ----------------------- | ------------------------------------------- |
| **Windows API**         | `windows-rs` 0.61                           |
| **Taskbar Enumeration** | IUIAutomation (UIA)                         |
| **Window Matching**     | `EnumWindows` + `GetWindowTextW`            |
| **Window Activation**   | `SetForegroundWindow` + `AttachThreadInput` |
| **Global Hotkeys**      | `RegisterHotKey` + `GetMessageW`            |
| **Virtual Desktops**    | `winvd`                                     |
| **UI**                  | `windows-reactor`                           |

## Contributing & Support

- **Found a bug or have a feature request?** Please [open an issue](https://github.com/congchuahiep/WinGlide/issues) to let me know!
- **Want to contribute?** Pull requests are always welcome.

## License

This project is open-source and licensed under the MIT License.

## Limitations & Requirements

- **Windows 11 Only**: Relies on the modern Windows 11 XAML taskbar implementation (`Taskbar.TaskListButtonAutomationPeer`).
