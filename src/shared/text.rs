//! String helpers shared across features.

/// Truncates `s` to `max_len` characters, appending `...` when it is longer.
pub fn truncate(s: &str, max_len: usize) -> String {
    match s.char_indices().nth(max_len) {
        Some((idx, _)) => format!("{}...", &s[..idx]),
        None => s.to_string(),
    }
}
