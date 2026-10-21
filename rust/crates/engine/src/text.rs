//! Go text helpers the engine's user-visible strings depend on.

pub use recur_agent::display::{hard_truncate_runes, truncate};

/// Go `html.EscapeString`.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '"' => out.push_str("&#34;"),
            _ => out.push(c),
        }
    }
    out
}

/// Go `time.Now().UnixNano()`.
pub fn unix_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// Go `time.Now().UnixMilli()`.
pub fn unix_millis() -> i64 {
    unix_nanos() / 1_000_000
}

/// Go `time.Time.Format(time.RFC3339)` in local time.
pub fn rfc3339(t: chrono::DateTime<chrono::Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `n one` / `n many` (Go `plural`).
pub fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_like_go() {
        assert_eq!(
            html_escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&#34;x&#34;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn plural_forms() {
        assert_eq!(plural(1, "finding", "findings"), "1 finding");
        assert_eq!(plural(0, "finding", "findings"), "0 findings");
    }
}
