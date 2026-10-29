//! Go RE2 regex dialect for bundle patterns.

use regex::Regex;

/// Compiles a Go RE2 pattern with Go semantics: `\d \s \w \b` and their
/// negations are ASCII-only in Go but Unicode in Rust, `\Q…\E` quotes, and
/// `[`, `&`, `~` are literal inside a Go character class.
pub fn compile_regexp(pattern: &str) -> Result<Regex, String> {
    Regex::new(&translate_regexp(pattern)).map_err(|e| {
        let text = e.to_string();
        let msg = text
            .lines()
            .rev()
            .find_map(|l| l.trim().strip_prefix("error: "))
            .unwrap_or(text.trim());
        format!("error parsing regexp: {msg}: `{pattern}`")
    })
}

fn translate_regexp(pattern: &str) -> String {
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
                    // POSIX class `[:alpha:]`: copy through the closing `:]`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexp_uses_go_classes() {
        let re = compile_regexp(r"^\d+\s\w$").unwrap();
        assert!(re.is_match("12 a"));
        assert!(!re.is_match("\u{0663} a"));
        assert!(!re.is_match("1\u{a0}a"));
        assert!(compile_regexp(r"[\d.]+").unwrap().is_match("1.2"));
        assert!(compile_regexp(r"a\Q.*\E").unwrap().is_match("a.*"));
        assert!(compile_regexp(r"[]a]").unwrap().is_match("]"));
        assert!(compile_regexp(r"[a&&b]").unwrap().is_match("&"));
        assert!(compile_regexp(r"(?i)^eng-\d+").unwrap().is_match("ENG-12"));
        let err = compile_regexp("(").unwrap_err();
        assert!(err.starts_with("error parsing regexp: "), "{err}");
    }
}
