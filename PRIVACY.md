# Privacy Policy

This policy applies to the WinGlide application for Windows 11 ("the Application"), operated by congchuahiep ("the Service Provider").

Effective date: 2026-11-23

## Data collection

The Application does not collect, store, transmit, or share any personal data. There are no analytics, no telemetry, no advertising, and no third-party tracking libraries in the Application.

## Local data

The Application stores its settings locally on your device in `%APPDATA%\WinGlide` (a `config.json` file) and writes log files to `%APPDATA%\WinGlide\logs`. This data never leaves your device and is removed when you delete that folder.

## Network access

The Application's behaviour differs by the way it was installed:

- **Microsoft Store version** (MSIX package): makes no network requests at all. Updates are delivered by the Microsoft Store.
- **Installer or portable version** (MSI/ZIP): contains an optional update check. It runs only when you press "Check for updates" in the Settings window, performs a single request to the public GitHub Releases API (`api.github.com`) to read the latest release version, and sends no personal or device-identifying information. Opening the release page in your browser is a normal web request made by your browser.

## Bundled components

The Application is built on Microsoft's Windows App SDK (WinUI 3) and ships that runtime next to its executable. These Microsoft components do not send telemetry on the Application's behalf.

## Permissions and system access

The Application uses Windows APIs on your device to provide its features: UI Automation to read taskbar buttons, a low-level keyboard hook for hotkeys that involve the Windows key (for example `Win+1`..`Win+9`), the virtual desktop API, and a few registry reads (for example the taskbar alignment and the Windows accent colour). All of this happens locally on your device; no data is transmitted.

## Start with Windows

Starting with Windows is off by default. When you enable it, the Application either uses the startup task declared in its package manifest (Microsoft Store version) or writes a single value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (installer and portable versions). Turning the setting off removes it again.

## Children

The Application is not directed at children and does not knowingly collect any information from anyone, including children.

## Changes

If this policy changes, the updated version will be published at this page with a new effective date.

## Contact

If you have questions about this policy, contact the Service Provider at congchuahiep@gmail.com.

