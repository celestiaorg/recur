//! String semantics that std does not spell the same way.

/// Equal under simple Unicode case folding.
pub(crate) fn eq_fold(a: &str, b: &str) -> bool {
    let fold = |c: char| {
        let mut lower = c.to_lowercase();
        match (lower.next(), lower.next()) {
            (Some(l), None) => l,
            _ => c,
        }
    };
    a.chars().count() == b.chars().count()
        && a.chars()
            .zip(b.chars())
            .all(|(x, y)| x == y || fold(x) == fold(y))
}

/// HTML-escapes `s`, as diff lines' `code_html` carries code.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            c => out.push(c),
        }
    }
    out
}

/// Matches `^[a-z][a-z0-9-]{0,max_rest}$`.
pub(crate) fn kebab_slug(s: &str, max_rest: usize) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= max_rest + 1
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

#[cfg(test)]
mod tests {
    use super::eq_fold;

    #[test]
    fn eq_fold_cases() {
        assert!(eq_fold("Dev", "dEV"));
        assert!(eq_fold("", ""));
        assert!(eq_fold("\u{212A}", "k")); // Kelvin sign folds to k
        assert!(!eq_fold("dev", "deva"));
        assert!(!eq_fold("dev", "dew"));
    }
}
