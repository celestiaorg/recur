//! The diff's trusted markup: `codeHtml` (escaped text, with token spans in rows older builds
//! highlighted on the backend) and the hunk header labels, parsed as `{@html}` parses them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::LazyLock;

use recur_content::render::doc::{Doc, El, Node};
use regex::Regex;

/// `parseInt(s, 10) || 0`.
pub fn js_int(s: &str) -> i64 {
    let t = recur_engine::view::js::trim(s);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let n = digits.bytes().take_while(u8::is_ascii_digit).count();
    let v: i64 = digits[..n].parse().unwrap_or(0);
    if neg { -v } else { v }
}

/// A token class from the review palette (`.kw .str .fn .cm .ty .nm`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Kw,
    Str,
    Fn,
    Cm,
    Ty,
    Nm,
}

impl Token {
    fn of(class: &str) -> Option<Token> {
        class.split_ascii_whitespace().rev().find_map(|c| {
            Some(match c {
                "kw" => Token::Kw,
                "str" => Token::Str,
                "fn" => Token::Fn,
                "cm" => Token::Cm,
                "ty" => Token::Ty,
                "nm" => Token::Nm,
                _ => return None,
            })
        })
    }
}

/// A run of code text and the innermost token class around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeRun {
    pub text: String,
    pub token: Option<Token>,
}

/// The text runs of a `codeHtml` cell.
/// Parsed cells kept across frames: every row a stack redraws (each scroll step) parsed its
/// markup again. Two generations bounded by bytes: a hit in the old one moves to the new, and a
/// full new one becomes the old.
#[derive(Default)]
struct RunsCache {
    now: HashMap<String, Rc<Vec<CodeRun>>>,
    old: HashMap<String, Rc<Vec<CodeRun>>>,
    bytes: usize,
}

const CODE_RUNS_MAX_BYTES: usize = 16 * 1024 * 1024;

thread_local! {
    static RUNS: RefCell<RunsCache> = RefCell::default();
}

/// [`parse_code_runs`], memoized by the cell's markup.
pub fn code_runs(html: &str) -> Rc<Vec<CodeRun>> {
    RUNS.with_borrow_mut(|c| {
        if let Some(r) = c.now.get(html) {
            return r.clone();
        }
        let runs = c
            .old
            .remove(html)
            .unwrap_or_else(|| Rc::new(parse_code_runs(html)));
        c.bytes += 2 * html.len() + 64;
        c.now.insert(html.to_string(), runs.clone());
        if c.bytes > CODE_RUNS_MAX_BYTES {
            c.old = std::mem::take(&mut c.now);
            c.bytes = 0;
        }
        runs
    })
}

pub fn parse_code_runs(html: &str) -> Vec<CodeRun> {
    fn walk(nodes: &[Node], token: Option<Token>, out: &mut Vec<CodeRun>) {
        for n in nodes {
            match n {
                Node::Text(t) => match out.last_mut() {
                    Some(last) if last.token == token => last.text.push_str(t),
                    _ => out.push(CodeRun {
                        text: t.clone(),
                        token,
                    }),
                },
                Node::El(e) => {
                    let inner = e.attr("class").and_then(Token::of).or(token);
                    walk(&e.children, inner, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&Doc::parse(html).nodes, None, &mut out);
    out
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").expect("tag pattern"));
static HUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^@@\s+-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s+@@\s*(.*)$").expect("hunk pattern")
});

/// `hunkLabel`: `@@ -a,b +c,d @@ ctx` as `Lines c–(c+d-1) · ctx` (escaped HTML); anything else
/// is returned as is.
pub fn hunk_label(html: &str) -> String {
    let raw = TAG.replace_all(html, "");
    let Some(m) = HUNK.captures(&raw) else {
        return html.to_string();
    };
    let num = |i: usize| m.get(i).map(|g| g.as_str().parse::<i64>().unwrap_or(0));
    let old_start = num(1).unwrap_or(0);
    let old_count = num(2).unwrap_or(1);
    let new_start = num(3).unwrap_or(0);
    let new_count = num(4).unwrap_or(1);
    let ctx = recur_engine::view::js::trim(m.get(5).map_or("", |g| g.as_str()));
    let (start, count) = if new_count == 0 {
        (old_start, old_count)
    } else {
        (new_start, new_count)
    };
    let range = if count <= 1 {
        format!("Line {start}")
    } else {
        format!("Lines {start}–{}", start + count - 1)
    };
    let label = if ctx.is_empty() {
        range
    } else {
        format!("{range} · {ctx}")
    };
    label
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `textContent` of trusted markup.
pub fn text_content(html: &str) -> String {
    let doc = Doc::parse(html);
    let mut out = String::new();
    for n in &doc.nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::El(e) => out.push_str(&e.text_content()),
        }
    }
    out
}

/// The parsed nodes of trusted markup (`extraHtml`).
pub fn parse(html: &str) -> Vec<Node> {
    Doc::parse(html).nodes
}

/// An element's declarations from its `style` attribute, in order.
pub fn inline_style(e: &El) -> Vec<(String, String)> {
    e.attr("style")
        .unwrap_or_default()
        .split(';')
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            let k = k.trim().to_ascii_lowercase();
            (!k.is_empty()).then(|| (k, v.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_int_reads_leading_digits() {
        assert_eq!(js_int("12"), 12);
        assert_eq!(js_int(" 7x"), 7);
        assert_eq!(js_int(""), 0);
        assert_eq!(js_int("abc"), 0);
        assert_eq!(js_int("-3"), -3);
    }

    #[test]
    fn code_runs_keep_token_classes_and_decode_entities() {
        let runs = code_runs(
            r#"<span class="kw">const</span> a = <span class="str">'&lt;b&gt;'</span> &amp;&amp; x"#,
        );
        assert_eq!(
            *runs,
            vec![
                CodeRun {
                    text: "const".into(),
                    token: Some(Token::Kw)
                },
                CodeRun {
                    text: " a = ".into(),
                    token: None
                },
                CodeRun {
                    text: "'<b>'".into(),
                    token: Some(Token::Str)
                },
                CodeRun {
                    text: " && x".into(),
                    token: None
                },
            ]
        );
        assert!(code_runs("").is_empty());
    }

    #[test]
    fn hunk_labels() {
        assert_eq!(
            hunk_label("@@ -1,6 +1,9 @@ provider registry"),
            "Lines 1–9 · provider registry"
        );
        assert_eq!(hunk_label("@@ -1,1 +1,2 @@"), "Lines 1–2");
        assert_eq!(hunk_label("@@ -4 +4 @@"), "Line 4");
        assert_eq!(hunk_label("@@ -5,3 +4,0 @@"), "Lines 5–7");
        assert_eq!(hunk_label("@@ -0,0 +1,1 @@ a<b"), "Line 1 · a&lt;b");
        assert_eq!(hunk_label("Lines 1–3"), "Lines 1–3");
    }
}
