//! Go RE2 patterns on the `regex` crate: `\d \s \w \b` and their negations
//! are ASCII-only in Go but Unicode in Rust, `\Q…\E` quotes, and `[`, `&`, `~`
//! are literal inside a Go character class.

use regex::Regex;

/// Go `regexp.Compile`, with Go's `error parsing regexp: …` error text.
pub(crate) fn compile(pattern: &str) -> Result<Regex, String> {
    Regex::new(&translate(pattern)).map_err(|e| {
        let text = e.to_string();
        let msg = text
            .lines()
            .rev()
            .find_map(|l| l.trim().strip_prefix("error: "))
            .unwrap_or(text.trim());
        format!("error parsing regexp: {msg}: `{pattern}`")
    })
}

/// Go `regexp.MustCompile` for patterns written in this crate.
pub(crate) fn must(pattern: &str) -> Regex {
    match compile(pattern) {
        Ok(re) => re,
        Err(e) => panic!("{e}"),
    }
}

fn translate(pattern: &str) -> String {
    const SPACE: &str = r"\t\n\x0C\r ";
    const WORD: &str = "0-9A-Za-z_";
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut in_class = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let d = chars[i + 1];
            i += 2;
            match (d, in_class) {
                ('d', true) => out.push_str("0-9"),
                ('d', false) => out.push_str("[0-9]"),
                ('D', _) => out.push_str("[^0-9]"),
                ('s', true) => out.push_str(SPACE),
                ('s', false) => out.push_str(&format!("[{SPACE}]")),
                ('S', _) => out.push_str(&format!("[^{SPACE}]")),
                ('w', true) => out.push_str(WORD),
                ('w', false) => out.push_str(&format!("[{WORD}]")),
                ('W', _) => out.push_str(&format!("[^{WORD}]")),
                ('b', false) => out.push_str(r"(?-u:\b)"),
                ('B', false) => out.push_str(r"(?-u:\B)"),
                ('Q', _) => {
                    let mut lit = String::new();
                    while i < chars.len() {
                        if chars[i] == '\\' && chars.get(i + 1) == Some(&'E') {
                            i += 2;
                            break;
                        }
                        lit.push(chars[i]);
                        i += 1;
                    }
                    if in_class {
                        for ch in lit.chars() {
                            out.push_str(&regex::escape(&ch.to_string()));
                        }
                    } else {
                        out.push_str(&regex::escape(&lit));
                    }
                }
                _ => {
                    out.push('\\');
                    out.push(d);
                }
            }
            continue;
        }
        if in_class {
            match c {
                ']' => {
                    in_class = false;
                    out.push(']');
                }
                '[' if chars.get(i + 1) == Some(&':') => {
                    let end = (i + 2..chars.len().saturating_sub(1))
                        .find(|&j| chars[j] == ':' && chars[j + 1] == ']');
                    match end {
                        Some(j) => {
                            out.extend(&chars[i..j + 2]);
                            i = j + 2;
                            continue;
                        }
                        None => out.push_str(r"\["),
                    }
                }
                '[' | '&' | '~' => {
                    out.push('\\');
                    out.push(c);
                }
                _ => out.push(c),
            }
            i += 1;
            continue;
        }
        if c == '[' {
            in_class = true;
            out.push('[');
            i += 1;
            if chars.get(i) == Some(&'^') {
                out.push('^');
                i += 1;
            }
            if chars.get(i) == Some(&']') {
                out.push_str(r"\]");
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Go `regexp.QuoteMeta`.
pub(crate) fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_classes_like_go() {
        let re = must(r"\b\w+\b");
        assert_eq!(re.find("é abc").map(|m| m.as_str()), Some("abc"));
        assert!(!must(r"^\s$").is_match("\u{a0}"));
        assert!(
            compile("(")
                .unwrap_err()
                .starts_with("error parsing regexp: ")
        );
    }
}
