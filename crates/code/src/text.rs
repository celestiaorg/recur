//! Display text helpers.

/// Whitespace runs collapsed to single spaces, trimmed.
pub(crate) fn collapse_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// [`collapse_space`], then text longer than `n` bytes keeps its first
/// `n - 1` bytes (on a char boundary) plus `…`.
pub(crate) fn truncate_fields(s: &str, n: usize) -> String {
    let s = collapse_space(s);
    if s.len() <= n {
        return s;
    }
    format!("{}…", &s[..s.floor_char_boundary(n.saturating_sub(1))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate_fields(" a  b ", 10), "a b");
        assert_eq!(truncate_fields("abcdef", 4), "abc…");
        assert_eq!(truncate_fields("a世b", 3), "a…");
    }
}
