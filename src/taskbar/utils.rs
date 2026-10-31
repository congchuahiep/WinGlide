/// Strips the suffix `" pinned"` from a taskbar button name, then applies
/// [`clean_button_name`].
///
/// Windows 11 appends `" pinned"` to buttons for pinned apps, so the raw name can be
/// `"File Explorer - 1 running window pinned"`. This is only used by the fallback
/// matching path; the primary path uses the button's exact AUMID instead.
pub fn normalize_button_label(name: &str) -> String {
    let trimmed = name.trim();
    let without_pinned = trimmed
        .strip_suffix(" pinned")
        .or_else(|| trimmed.strip_suffix(" Pinned"))
        .unwrap_or(trimmed);

    clean_button_name(without_pinned).trim().to_string()
}

/// Strips the suffix " - N running window(s)" from the taskbar button name.
///
/// Format on Win11:
/// - Single app: `"Notepad"` -> `"Notepad"`
/// - Multiple windows: `"Chrome - 3 running windows"` -> `"Chrome"`
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_running_window_suffix() {
        assert_eq!(clean_button_name("Chrome - 3 running windows"), "Chrome");
        assert_eq!(
            clean_button_name("VS Code - main.rs - 1 running window"),
            "VS Code - main.rs"
        );
    }

    #[test]
    fn normalize_strips_the_pinned_suffix_before_the_count() {
        assert_eq!(
            normalize_button_label("File Explorer - 1 running window pinned"),
            "File Explorer"
        );
        assert_eq!(normalize_button_label("Notepad"), "Notepad");
    }
}
