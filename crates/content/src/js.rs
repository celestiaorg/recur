//! JavaScript string and RegExp semantics the ported libraries rely on: ECMAScript regexes
//! (through `regress`, so the sources port verbatim), `String.prototype.replace` with
//! `GetSubstitution`, `trim`, UTF-16 lengths, `encodeURI` and `encodeURIComponent`.

use std::ops::Range;

/// A compiled ECMAScript regex. `global` mirrors the `g` flag for [`Re::replace`].
pub struct Re {
    re: regress::Regex,
    global: bool,
}

/// One match: the haystack and the group ranges (byte offsets).
pub struct Caps<'t> {
    text: &'t str,
    m: regress::Match,
}

impl<'t> Caps<'t> {
    /// Group `i` (0 is the whole match), `None` when it did not participate.
    pub fn get(&self, i: usize) -> Option<&'t str> {
        self.m.group(i).map(|r| &self.text[r])
    }
    /// Group `i`, or `""` when it did not participate (JS `cap[i] || ''`).
    pub fn str(&self, i: usize) -> &'t str {
        self.get(i).unwrap_or("")
    }
    /// Whether group `i` matched a non-empty string (JS truthiness).
    pub fn truthy(&self, i: usize) -> bool {
        !self.str(i).is_empty()
    }
    pub fn range(&self) -> Range<usize> {
        self.m.range()
    }
    pub fn index(&self) -> usize {
        self.m.start()
    }
    pub fn full(&self) -> &'t str {
        &self.text[self.m.range()]
    }
    pub fn len(&self) -> usize {
        self.m.captures.len() + 1
    }
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl Re {
    /// Compile `source` with JS `flags` (`g`, `i`, `m`, `s`, `u`). The sources are the
    /// libraries' own literals, so a failure is a porting bug.
    pub fn new(source: &str, flags: &str) -> Re {
        let re = regress::Regex::with_flags(source, flags)
            .unwrap_or_else(|e| panic!("regex /{source}/{flags}: {e}"));
        Re {
            re,
            global: flags.contains('g'),
        }
    }

    /// `RegExp.prototype.exec` from index 0.
    pub fn exec<'t>(&self, text: &'t str) -> Option<Caps<'t>> {
        self.exec_at(text, 0)
    }

    /// `exec` with `lastIndex = at` (a global or sticky regex).
    pub fn exec_at<'t>(&self, text: &'t str, at: usize) -> Option<Caps<'t>> {
        if at > text.len() {
            return None;
        }
        self.re.find_from(text, at).next().map(|m| Caps { text, m })
    }

    pub fn test(&self, text: &str) -> bool {
        self.re.find(text).is_some()
    }

    /// `String.prototype.search`: byte index of the first match, or `None` (-1).
    pub fn search(&self, text: &str) -> Option<usize> {
        self.re.find(text).map(|m| m.start())
    }

    /// Every match, advancing past empty ones as `matchAll` does.
    pub fn matches<'t>(&self, text: &'t str) -> Vec<Caps<'t>> {
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(c) = self.exec_at(text, at) {
            let r = c.range();
            at = if r.is_empty() {
                next_char(text, r.end)
            } else {
                r.end
            };
            out.push(c);
            if at > text.len() {
                break;
            }
        }
        out
    }

    /// `String.prototype.replace(regex, fn)`: every match when global, else the first.
    pub fn replace_with(&self, text: &str, mut f: impl FnMut(&Caps) -> String) -> String {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        let mut at = 0;
        while let Some(c) = self.exec_at(text, at) {
            let r = c.range();
            out.push_str(&text[last..r.start]);
            out.push_str(&f(&c));
            last = r.end;
            if !self.global {
                break;
            }
            at = if r.is_empty() {
                next_char(text, r.end)
            } else {
                r.end
            };
            if at > text.len() {
                break;
            }
        }
        out.push_str(&text[last.min(text.len())..]);
        out
    }

    /// `String.prototype.replace(regex, string)` with `$`-substitutions.
    pub fn replace(&self, text: &str, replacement: &str) -> String {
        self.replace_with(text, |c| substitute(replacement, c))
    }

    /// `String.prototype.split(regex)` with captured separators spliced in, as JS does.
    pub fn split<'t>(&self, text: &'t str) -> Vec<Option<&'t str>> {
        let mut out = Vec::new();
        let mut last = 0;
        let mut at = 0;
        while at < text.len() {
            let Some(c) = self.exec_at(text, at) else {
                break;
            };
            let r = c.range();
            if r.end == r.start && r.start >= text.len() {
                break;
            }
            if r.is_empty() && r.start == last {
                at = next_char(text, r.start);
                continue;
            }
            out.push(Some(&text[last..r.start]));
            for i in 1..c.len() {
                out.push(c.get(i));
            }
            last = r.end;
            at = if r.is_empty() {
                next_char(text, r.end)
            } else {
                r.end
            };
        }
        out.push(Some(&text[last..]));
        out
    }
}

fn next_char(text: &str, i: usize) -> usize {
    i + text[i.min(text.len())..]
        .chars()
        .next()
        .map_or(1, char::len_utf8)
}

/// `GetSubstitution` for a replacement string: `$$`, `$&`, `` $` ``, `$'`, `$n`, `$nn`.
pub fn substitute(replacement: &str, c: &Caps) -> String {
    let groups = c.len() - 1;
    let b = replacement.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' || i + 1 >= b.len() {
            let ch = replacement[i..].chars().next().unwrap_or('$');
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        match b[i + 1] {
            b'$' => {
                out.push('$');
                i += 2;
            }
            b'&' => {
                out.push_str(c.full());
                i += 2;
            }
            b'`' => {
                out.push_str(&c.text[..c.index()]);
                i += 2;
            }
            b'\'' => {
                out.push_str(&c.text[c.range().end..]);
                i += 2;
            }
            d if d.is_ascii_digit() => {
                let one = (d - b'0') as usize;
                let two = b
                    .get(i + 2)
                    .filter(|x| x.is_ascii_digit())
                    .map(|x| one * 10 + (x - b'0') as usize);
                match two {
                    Some(n) if n >= 1 && n <= groups => {
                        out.push_str(c.str(n));
                        i += 3;
                    }
                    _ if one >= 1 && one <= groups => {
                        out.push_str(c.str(one));
                        i += 2;
                    }
                    _ => {
                        out.push('$');
                        i += 1;
                    }
                }
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    out
}

/// `String.prototype.replace(string, string)`: the first occurrence only, with `$`-patterns
/// (`$$`, `$&`, `` $` ``, `$'`) in the replacement.
pub fn replace_first(text: &str, pattern: &str, replacement: &str) -> String {
    let Some(at) = text.find(pattern) else {
        return text.to_string();
    };
    let b = replacement.as_bytes();
    let mut sub = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() {
            match b[i + 1] {
                b'$' => {
                    sub.push('$');
                    i += 2;
                    continue;
                }
                b'&' => {
                    sub.push_str(pattern);
                    i += 2;
                    continue;
                }
                b'`' => {
                    sub.push_str(&text[..at]);
                    i += 2;
                    continue;
                }
                b'\'' => {
                    sub.push_str(&text[at + pattern.len()..]);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        let ch = replacement[i..].chars().next().unwrap_or('$');
        sub.push(ch);
        i += ch.len_utf8();
    }
    let mut out = String::with_capacity(text.len() + sub.len());
    out.push_str(&text[..at]);
    out.push_str(&sub);
    out.push_str(&text[at + pattern.len()..]);
    out
}

/// ECMAScript `WhiteSpace` and `LineTerminator` (what `trim` removes).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

pub fn trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_space)
}

pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_space)
}

/// `String.prototype.length`: UTF-16 code units.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn encode(s: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for ch in s.chars() {
        if ch.is_ascii() && keep(ch as u8) {
            out.push(ch);
            continue;
        }
        for b in ch.encode_utf8(&mut buf).bytes() {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `encodeURIComponent`.
pub fn encode_uri_component(s: &str) -> String {
    encode(s, |b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
    })
}

/// `encodeURI`.
pub fn encode_uri(s: &str) -> String {
    encode(s, |b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_'
                    | b'.'
                    | b'!'
                    | b'~'
                    | b'*'
                    | b'\''
                    | b'('
                    | b')'
                    | b';'
                    | b','
                    | b'/'
                    | b'?'
                    | b':'
                    | b'@'
                    | b'&'
                    | b'='
                    | b'+'
                    | b'$'
                    | b'#'
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_substitutes_groups_like_js() {
        let re = Re::new(r"(a)(b)?", "g");
        assert_eq!(re.replace("xaabx", "[$1|$2|$&|$$]"), "x[a||a|$][a|b|ab|$]x");
        assert_eq!(replace_first("a-b-b", "b", "<$&>"), "a-<b>-b");
    }

    #[test]
    fn split_keeps_captured_separators() {
        let re = Re::new(r"(```[\s\S]*?```|`[^`]+`)", "");
        let parts: Vec<_> = re
            .split("a `b` c")
            .into_iter()
            .map(|p| p.unwrap_or(""))
            .collect();
        assert_eq!(parts, vec!["a ", "`b`", " c"]);
    }

    #[test]
    fn uri_encoders_match_js() {
        assert_eq!(
            encode_uri_component("https://a/b?c=d e"),
            "https%3A%2F%2Fa%2Fb%3Fc%3Dd%20e"
        );
        assert_eq!(encode_uri("https://a/b?c=d e%"), "https://a/b?c=d%20e%25");
        assert_eq!(encode_uri("é"), "%C3%A9");
    }
}
