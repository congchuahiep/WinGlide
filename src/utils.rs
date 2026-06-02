/// Cắt chuỗi `s` thành `max_len` ký tự, thêm dấu `...` nếu chuỗi dài hơn.
pub fn truncate(s: &str, max_len: usize) -> String {
    match s.char_indices().nth(max_len) {
        Some((idx, _)) => format!("{}...", &s[..idx]),
        None => s.to_string(),
    }
}
