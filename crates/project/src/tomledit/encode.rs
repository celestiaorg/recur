//! The parts of go-toml v2's encoder that Recur's project.toml output relies
//! on, reproduced byte for byte: key quoting, literal-first strings, basic
//! string escapes, `multiline:"true"` strings and `[[array]]` sections.

use std::fmt::Write as _;

/// A key, bare when it only has `A-Za-z0-9_-`, quoted otherwise.
pub fn key(k: &str) -> String {
    if is_bare_key(k) {
        k.to_owned()
    } else {
        string(k)
    }
}

fn is_bare_key(k: &str) -> bool {
    !k.is_empty()
        && k.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// A string value: a literal string when possible, a basic string otherwise.
pub fn string(s: &str) -> String {
    if can_be_literal(s) {
        format!("'{s}'")
    } else {
        basic_string(s)
    }
}

/// A string field tagged `multiline:"true"`: `"""…"""` only when the value
/// actually contains a newline.
pub fn multiline_field(s: &str) -> String {
    if s.contains('\n') {
        multiline_string(s)
    } else {
        string(s)
    }
}

fn can_be_literal(s: &str) -> bool {
    !s.bytes().any(|c| c == b'\'' || c == 0x7f || c < 0x20)
}

fn push_control(out: &mut String, c: char) {
    let _ = write!(out, "\\u{:04X}", c as u32);
}

fn basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => push_control(&mut out, c),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn multiline_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 7);
    out.push_str("\"\"\"\n");
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                // Runs of three or more quotes must be escaped.
                let mut j = i;
                while j < chars.len() && chars[j] == '"' {
                    j += 1;
                }
                if j - i >= 3 {
                    for _ in i..j {
                        out.push_str("\\\"");
                    }
                } else {
                    for _ in i..j {
                        out.push('"');
                    }
                }
                i = j;
                continue;
            }
            '\\' => out.push_str("\\\\"),
            '\n' => out.push('\n'),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push('\t'),
            c if (c as u32) < 0x20 || c == '\u{7f}' => push_control(&mut out, c),
            c => out.push(c),
        }
        i += 1;
    }
    out.push_str("\"\"\"");
    out
}

/// One `key = value` line of a struct field.
pub fn key_value(name: &str, value: &str) -> String {
    format!("{} = {value}\n", key(name))
}

/// The `[[name]]` blocks for an array of tables, each element given as its
/// rendered `key = value` lines. `preceded` is whether the encoder already
/// wrote anything, which earns the first header a blank line.
pub fn array_tables(name: &str, elems: &[String], preceded: bool) -> String {
    let mut out = String::new();
    let mut wrote = preceded;
    for body in elems {
        if wrote {
            out.push('\n');
        }
        let _ = writeln!(out, "[[{}]]", key(name));
        out.push_str(body);
        wrote = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_match_go_toml() {
        assert_eq!(string("pnpm install"), "'pnpm install'");
        assert_eq!(string("it's"), r#""it's""#);
        assert_eq!(string("a\tb"), r#""a\tb""#);
        assert_eq!(string("q\"\\\u{1}"), r#""q\"\\\u0001""#);
        assert_eq!(string(""), "''");
        assert_eq!(multiline_field("one line"), "'one line'");
        assert_eq!(
            multiline_field("A=1\n\"\"\"x\\\ty\r"),
            "\"\"\"\nA=1\n\\\"\\\"\\\"x\\\\\ty\\r\"\"\""
        );
        assert_eq!(multiline_field("a\"\"\nb"), "\"\"\"\na\"\"\nb\"\"\"");
        assert_eq!(key("dev_cmd"), "dev_cmd");
        assert_eq!(key("a b"), "'a b'");
        assert_eq!(key(""), "''");
    }
}
