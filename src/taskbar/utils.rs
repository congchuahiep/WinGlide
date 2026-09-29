/// Strips the suffix " - N running window(s)" from the taskbar button name.
///
/// Format on Win11:
/// - Single app: `"Notepad"` -> `"Notepad"`
/// - Multiple windows: `"Chrome - 3 running windows"` -> `"Chrome"`
/// - Pinned: `"Notepad - Pinned"` -> `"Notepad - Pinned"` (unchanged)
/// - VS Code: `"VS Code - main.rs - 1 running window"` -> `"VS Code - main.rs"`
pub fn clean_button_name(name: &str) -> String {
    if let Some(pos) = name.rfind(" running window") {
        let before = &name[..pos];

        if let Some(dash_pos) = before.rfind(" - ") {
            return before[..dash_pos].to_string();
        }

        if let Some(dash_pos) = before.rfind(" \u{2014} ") {
            return before[..dash_pos].to_string();
        }

        return before.to_string();
    }

    name.to_string()
}
