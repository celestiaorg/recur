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

/// The decoded text of `codeHtml` cells, kept across frames: every row a stack redraws (each
/// scroll step) decoded its markup again. Two generations bounded by bytes: a hit in the old one
/// moves to the new, and a full new one becomes the old.
#[derive(Default)]
struct TextCache {
    now: HashMap<String, Rc<str>>,
    old: HashMap<String, Rc<str>>,
    bytes: usize,
}

const CELL_TEXT_MAX_BYTES: usize = 16 * 1024 * 1024;

thread_local! {
    static CELLS: RefCell<TextCache> = RefCell::default();
}

/// [`text_content`] of a `codeHtml` cell, memoized by its markup.
pub fn cell_text(html: &str) -> Rc<str> {
    CELLS.with_borrow_mut(|c| {
        if let Some(r) = c.now.get(html) {
            return r.clone();
        }
        let text = c
            .old
            .remove(html)
            .unwrap_or_else(|| text_content(html).into());
        c.bytes += 2 * html.len() + 64;
        c.now.insert(html.to_string(), text.clone());
        if c.bytes > CELL_TEXT_MAX_BYTES {
            c.old = std::mem::take(&mut c.now);
            c.bytes = 0;
        }
        text
    })
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
    fn cell_text_drops_token_spans_and_decodes_entities() {
        assert_eq!(
            &*cell_text(
                r#"<span class="kw">const</span> a = <span class="str">'&lt;b&gt;'</span> &amp;&amp; x"#,
            ),
            "const a = '<b>' && x"
        );
        assert!(cell_text("").is_empty());
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
