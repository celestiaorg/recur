//! JavaScript `RegExp` semantics on Oniguruma.
//!
//! Bundle metadata (`[[languageData]]` indent patterns, `[[symbol]]`
//! transforms, grammar fold markers, `firstLineMatch`) is compiled with
//! `new RegExp(...)` in the web UI. This compiles the same sources the same
//! way: constructs JS rejects are rejected here too (possessive quantifiers,
//! inline flags, atomic groups), unknown letter escapes are literal letters
//! (Annex B), `\d` / `\w` / `\b` are ASCII, and `^` / `$` anchor to the whole
//! input unless the `m` flag is set.
//!
//! simplify: JS-only details that never occur in bundle patterns are not
//! emulated (`.` excluding `\r`/U+2028, `[^]`, variable-length lookbehind).
//! Add them here if a bundle starts relying on one.

use onig::{Regex, RegexOptions, Region, SearchOptions, Syntax};

const WORD_IS_ASCII: u32 = 1 << 16;
const DIGIT_IS_ASCII: u32 = 1 << 17;

pub struct JsRegex {
    re: Regex,
    global: bool,
    has_named_groups: bool,
}

/// Escapes JS understands in non-unicode mode; any other escaped letter is
/// the letter itself.
fn js_escape_letter(c: char) -> bool {
    matches!(
        c,
        'd' | 'D'
            | 'w'
            | 'W'
            | 's'
            | 'S'
            | 'b'
            | 'B'
            | 'n'
            | 'r'
            | 't'
            | 'v'
            | 'f'
            | 'x'
            | 'u'
            | 'c'
            | 'k'
    )
}

/// Rewrite a JS source for Oniguruma, or `None` where `new RegExp` throws.
fn translate(src: &str) -> Option<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_class = false;
    let mut prev_quantifier = false;
    while i < chars.len() {
        let c = chars[i];
        let mut is_quantifier = false;
        match c {
            '\\' => {
                let &n = chars.get(i + 1)?;
                let hex4: String = chars[i + 2..].iter().take(4).collect();
                if n.is_ascii_alphabetic() && !js_escape_letter(n) {
                    out.push(n);
                } else if n == 'v' {
                    out.push('\u{0B}');
                } else if n == 'u' && hex4.len() == 4 && hex4.chars().all(|h| h.is_ascii_hexdigit())
                {
                    out.push_str(&format!("\\x{{{hex4}}}"));
                    i += 6;
                    prev_quantifier = false;
                    continue;
                } else {
                    out.push('\\');
                    out.push(n);
                }
                i += 2;
                prev_quantifier = false;
                continue;
            }
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class && chars.get(i + 1) == Some(&'?') => {
                let rest: String = chars[i + 2..].iter().take(2).collect();
                let ok = rest.starts_with(':')
                    || rest.starts_with('=')
                    || rest.starts_with('!')
                    || rest == "<="
                    || rest == "<!"
                    || (rest.starts_with('<')
                        && rest
                            .chars()
                            .nth(1)
                            .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$'));
                if !ok {
                    return None;
                }
            }
            '*' | '+' | '?' if !in_class => {
                if prev_quantifier && c == '+' {
                    return None;
                }
                is_quantifier = c != '?' || !prev_quantifier;
                if c == '?' && prev_quantifier {
                    // Lazy modifier: `+?` / `*?` / `??`.
                    is_quantifier = false;
                }
            }
            '{' if !in_class => {
                let close = chars[i + 1..].iter().position(|&c| c == '}');
                let body: Option<String> = close.map(|n| chars[i + 1..i + 1 + n].iter().collect());
                let interval = body.as_deref().is_some_and(|b| {
                    let (lo, hi) = b.split_once(',').unwrap_or((b, ""));
                    !lo.is_empty()
                        && lo.bytes().all(|c| c.is_ascii_digit())
                        && hi.bytes().all(|c| c.is_ascii_digit())
                });
                if let (true, Some(body)) = (interval, body) {
                    if prev_quantifier {
                        return None;
                    }
                    out.push('{');
                    out.push_str(&body);
                    out.push('}');
                    i += body.chars().count() + 2;
                    prev_quantifier = true;
                    continue;
                }
                out.push_str("\\{");
                i += 1;
                prev_quantifier = false;
                continue;
            }
            '}' if !in_class => {
                out.push_str("\\}");
                i += 1;
                prev_quantifier = false;
                continue;
            }
            _ => {}
        }
        out.push(c);
        prev_quantifier = is_quantifier;
        i += 1;
    }
    if in_class {
        return None;
    }
    Some(out)
}

impl JsRegex {
    /// `new RegExp(source, flags)`; `None` where JS would throw.
    pub fn new(source: &str, flags: &str) -> Option<JsRegex> {
        let mut opts = RegexOptions::from_bits_retain(WORD_IS_ASCII | DIGIT_IS_ASCII)
            | RegexOptions::REGEX_OPTION_CAPTURE_GROUP;
        let mut global = false;
        for f in flags.chars() {
            match f {
                'g' => global = true,
                'i' => opts |= RegexOptions::REGEX_OPTION_IGNORECASE,
                'm' => opts |= RegexOptions::REGEX_OPTION_NEGATE_SINGLELINE,
                's' => opts |= RegexOptions::REGEX_OPTION_MULTILINE,
                'u' | 'y' | 'd' => {}
                _ => return None,
            }
        }
        let translated = translate(source)?;
        let re = Regex::with_options(&translated, opts, Syntax::perl_ng()).ok()?;
        Some(JsRegex {
            has_named_groups: re.capture_names_len() > 0,
            re,
            global,
        })
    }

    /// Group spans of the first match at or after `start`.
    pub fn exec_at(&self, text: &str, start: usize) -> Option<Vec<Option<(usize, usize)>>> {
        let mut region = Region::new();
        self.re.search_with_options(
            text,
            start,
            text.len(),
            SearchOptions::SEARCH_OPTION_NONE,
            Some(&mut region),
        )?;
        Some((0..region.len()).map(|g| region.pos(g)).collect())
    }

    pub fn test(&self, text: &str) -> bool {
        self.exec_at(text, 0).is_some()
    }

    fn group_index(&self, name: &str) -> Option<usize> {
        let mut found = None;
        self.re.foreach_name(|n, groups| {
            if n == name {
                found = groups.first().map(|g| *g as usize);
                false
            } else {
                true
            }
        });
        found
    }

    /// `String.prototype.replace(regex, replacement)` with JS `$` patterns.
    pub fn replace(&self, text: &str, replacement: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        let mut pos = 0;
        while pos <= text.len() {
            let Some(groups) = self.exec_at(text, pos) else {
                break;
            };
            let Some((ms, me)) = groups[0] else { break };
            out.push_str(&text[last..ms]);
            self.expand(text, &groups, replacement, &mut out);
            last = me;
            if !self.global {
                break;
            }
            pos = if me == ms {
                me + text[me..].chars().next().map_or(1, char::len_utf8)
            } else {
                me
            };
        }
        out.push_str(&text[last.min(text.len())..]);
        out
    }

    fn expand(&self, text: &str, groups: &[Option<(usize, usize)>], repl: &str, out: &mut String) {
        let group = |i: usize| {
            groups
                .get(i)
                .copied()
                .flatten()
                .map_or("", |(s, e)| &text[s..e])
        };
        let (ms, me) = groups[0].unwrap_or((0, 0));
        let b = repl.as_bytes();
        let ngroups = groups.len() - 1;
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'$' || i + 1 >= b.len() {
                let ch = repl[i..].chars().next().unwrap_or('$');
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
                    out.push_str(&text[ms..me]);
                    i += 2;
                }
                b'`' => {
                    out.push_str(&text[..ms]);
                    i += 2;
                }
                b'\'' => {
                    out.push_str(&text[me..]);
                    i += 2;
                }
                d @ b'0'..=b'9' => {
                    let one = usize::from(d - b'0');
                    let two = b
                        .get(i + 2)
                        .filter(|c| c.is_ascii_digit())
                        .map(|c| one * 10 + usize::from(c - b'0'));
                    match two {
                        Some(n) if n >= 1 && n <= ngroups => {
                            out.push_str(group(n));
                            i += 3;
                        }
                        _ if one >= 1 && one <= ngroups => {
                            out.push_str(group(one));
                            i += 2;
                        }
                        _ => {
                            out.push('$');
                            i += 1;
                        }
                    }
                }
                b'<' if self.has_named_groups => match repl[i + 2..].find('>') {
                    Some(close) => {
                        let name = &repl[i + 2..i + 2 + close];
                        if let Some(g) = self.group_index(name) {
                            out.push_str(group(g));
                        }
                        i += 3 + close;
                    }
                    None => {
                        out.push('$');
                        i += 1;
                    }
                },
                _ => {
                    out.push('$');
                    i += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_what_js_rejects() {
        assert!(JsRegex::new("(?:^(\\s*+(module|class))", "").is_none());
        assert!(JsRegex::new("(?i)-\\*- mode -\\*-", "").is_none());
        assert!(JsRegex::new("(?>a)", "").is_none());
        assert!(JsRegex::new("[abc", "").is_none());
        assert!(JsRegex::new("a", "q").is_none());
        assert!(JsRegex::new("a+?b*?", "").is_some());
        assert!(JsRegex::new("(?<name>x)(?<=y)(?<!z)(?=a)(?!b)", "").is_some());
    }

    #[test]
    fn anchors_and_classes_follow_js() {
        let re = JsRegex::new("^\\s*(?:[})\\]]|(?:case|default)\\b)", "").unwrap();
        assert!(re.test("  }"));
        assert!(re.test("case x:"));
        assert!(!re.test("x\ncase"));
        let lead = JsRegex::new("^\\d", "").unwrap();
        assert!(!lead.test("\u{0663}"));
        let multi = JsRegex::new("^b", "m").unwrap();
        assert!(multi.test("a\nb"));
        assert!(JsRegex::new("\\A", "").unwrap().test("A"));
    }

    #[test]
    fn replace_matches_string_prototype_replace() {
        let re = JsRegex::new("^[\"']|[\"']$", "g").unwrap();
        assert_eq!(re.replace("\"main\"", ""), "main");
        let pre = JsRegex::new("^", "").unwrap();
        assert_eq!(pre.replace("main", "ID: "), "ID: main");
        let swap = JsRegex::new("(\\w+) (\\w+)", "").unwrap();
        assert_eq!(swap.replace("a b", "$2 $1 $$ $& $3"), "b a $ a b $3");
        let named = JsRegex::new("(?<x>o)", "g").unwrap();
        assert_eq!(named.replace("foo", "[$<x>]"), "f[o][o]");
        let empty = JsRegex::new("x*", "g").unwrap();
        assert_eq!(empty.replace("ab", "-"), "-a-b-");
    }
}
