//! The tiny bash highlighter for terminal tool cards: one pass over the
//! command, producing classed runs (`sh-kw` command words, `sh-str` strings, `sh-com`
//! comments, `sh-flag` flags, `sh-var` variables, `sh-op` operators).

use std::sync::LazyLock;

use regex::Regex;

const OPS: [&str; 9] = ["&&", "||", "|", ";", ">>", ">", "<<", "<", "2>&1"];

static VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\$(\{[^}]*\}|[A-Za-z_]\w*)").expect("var"));
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r##"^[^\s'"#$|&;<>]+"##).expect("word"));
static FLAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^--?[\w][\w-]*=?$").expect("flag"));
static ASSIGN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_]\w*=").expect("assign"));

/// A run of the highlighted command: its text and `sh-*` class ("" for plain).
pub type ShRun = (String, &'static str);

fn push(out: &mut Vec<ShRun>, text: &str, class: &'static str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.1 == class
        && class.is_empty()
    {
        last.0.push_str(text);
        return;
    }
    out.push((text.to_string(), class));
}

/// `highlightBash`.
pub fn highlight_bash(src: &str) -> Vec<ShRun> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut at_cmd_start = true;
    let n = src.len();
    while i < n {
        let rest = &src[i..];
        let c = rest.chars().next().unwrap_or(' ');
        let prev_space = i == 0
            || src[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if c == '#' && prev_space {
            let j = rest.find('\n').map_or(n, |k| i + k);
            push(&mut out, &src[i..j], "sh-com");
            i = j;
            continue;
        }
        if c == '\'' || c == '"' {
            let bytes = src.as_bytes();
            let q = bytes[i];
            let mut j = i + 1;
            while j < n && bytes[j] != q {
                if q == b'"' && bytes[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let j = (j + 1).min(n);
            let j = (j..=n).find(|&k| src.is_char_boundary(k)).unwrap_or(n);
            push(&mut out, &src[i..j], "sh-str");
            i = j;
            at_cmd_start = false;
            continue;
        }
        if c == '$'
            && rest[1..]
                .chars()
                .next()
                .is_some_and(|d| d.is_ascii_alphabetic() || d == '_' || d == '{')
            && let Some(m) = VAR.find(rest)
        {
            push(&mut out, m.as_str(), "sh-var");
            i += m.end();
            at_cmd_start = false;
            continue;
        }
        if let Some(op) = OPS.iter().find(|o| rest.starts_with(**o)) {
            push(&mut out, op, "sh-op");
            i += op.len();
            at_cmd_start = true;
            continue;
        }
        if c == '\n' {
            push(&mut out, "\n", "");
            i += 1;
            at_cmd_start = true;
            continue;
        }
        if c.is_whitespace() {
            push(&mut out, &rest[..c.len_utf8()], "");
            i += c.len_utf8();
            continue;
        }
        let word = WORD
            .find(rest)
            .map_or(&rest[..c.len_utf8()], |m| m.as_str());
        if FLAG.is_match(word) {
            push(&mut out, word, "sh-flag");
        } else if at_cmd_start {
            if ASSIGN.is_match(word) {
                push(&mut out, word, "sh-var");
            } else {
                push(&mut out, word, "sh-kw");
                at_cmd_start = false;
            }
        } else {
            push(&mut out, word, "");
        }
        i += word.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(src: &str) -> Vec<(&'static str, String)> {
        highlight_bash(src)
            .into_iter()
            .map(|(t, c)| (c, t))
            .collect()
    }

    #[test]
    fn command_words_flags_strings_and_ops() {
        let got = classes("FOO=1 go test -run 'TestX' ./... && echo \"$HOME\" # done");
        assert_eq!(got[0], ("sh-var", "FOO=1".to_string()));
        assert_eq!(got[2], ("sh-kw", "go".to_string()));
        assert_eq!(got[3], ("", " test ".to_string()));
        assert!(got.contains(&("sh-flag", "-run".to_string())));
        assert!(got.contains(&("sh-str", "'TestX'".to_string())));
        assert!(got.contains(&("sh-op", "&&".to_string())));
        assert!(got.contains(&("sh-kw", "echo".to_string())));
        assert!(got.contains(&("sh-str", "\"$HOME\"".to_string())));
        assert_eq!(got.last(), Some(&("sh-com", "# done".to_string())));
        let text: String = highlight_bash("a | b\nc $X")
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(text, "a | b\nc $X");
        assert!(classes("cat ${DIR}/x").contains(&("sh-var", "${DIR}".to_string())));
        assert!(classes("ls 2>&1").contains(&("sh-op", "2>&1".to_string())));
    }
}
