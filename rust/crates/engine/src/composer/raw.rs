//! `⟦raw[:nonce]⟧ … ⟦/raw[:nonce]⟧` regions (Go `raw_regions.go`): text the
//! host wrap must not scan for `@path` / `/skill`. Nonce-paired markers let
//! an embedded blob quote the bare syntax without closing early. Markers
//! are removed before storage and the provider.

use std::sync::LazyLock;

use regex::Regex;

static RAW_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"⟦raw(?::([A-Za-z0-9_-]+))?⟧").expect("raw open pattern"));

/// Any leftover open or close (stray / unmatched).
static RAW_MARKER_STRIP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"⟦/?raw(?::[A-Za-z0-9_-]+)?⟧").expect("raw strip pattern"));

/// Replaces raw regions with placeholders; returns the masked text and the
/// region bodies. An unclosed open marker makes the rest raw.
pub fn mask_raw_regions(text: &str) -> (String, Vec<String>) {
    if text.is_empty() || !text.contains("⟦raw") {
        return (text.to_owned(), Vec::new());
    }
    let mut regions: Vec<String> = Vec::new();
    let mut b = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(c) = RAW_OPEN_RE.captures(rest) else {
            b.push_str(rest);
            break;
        };
        let open = c.get(0).expect("whole match");
        b.push_str(&rest[..open.start()]);
        let id = c.get(1).map_or("", |m| m.as_str());
        let after_open = &rest[open.end()..];
        match find_matching_raw_close(after_open, id) {
            None => {
                regions.push(after_open.to_owned());
                b.push_str(&raw_placeholder(regions.len() - 1));
                break;
            }
            Some((idx, len)) => {
                regions.push(after_open[..idx].to_owned());
                b.push_str(&raw_placeholder(regions.len() - 1));
                rest = &after_open[idx + len..];
            }
        }
    }
    (b, regions)
}

/// The close marker matching `id` (empty id: bare `⟦/raw⟧` only, skipping
/// nonced closes).
fn find_matching_raw_close(s: &str, id: &str) -> Option<(usize, usize)> {
    if !id.is_empty() {
        let want = format!("⟦/raw:{id}⟧");
        return s.find(&want).map(|j| (j, want.len()));
    }
    const BARE: &str = "⟦/raw⟧";
    const NONCED: &str = "⟦/raw:";
    let mut i = 0;
    loop {
        let j = i + s[i..].find(BARE)?;
        if s[j..].starts_with(NONCED) {
            i = j + NONCED.len();
            continue;
        }
        return Some((j, BARE.len()));
    }
}

/// Restores each region in place of its placeholder.
pub fn unmask_raw_regions(text: &str, regions: &[String]) -> String {
    let mut out = text.to_owned();
    for (i, body) in regions.iter().enumerate() {
        out = out.replacen(&raw_placeholder(i), body, 1);
    }
    out
}

/// Drops unmatched markers from scannable text.
pub fn strip_stray_raw_markers(text: &str) -> String {
    if text.is_empty() || !text.contains('⟦') {
        return text.to_owned();
    }
    RAW_MARKER_STRIP_RE.replace_all(text, "").into_owned()
}

/// No `@` or `/` after whitespace, so it never looks like a mention.
fn raw_placeholder(i: usize) -> String {
    format!("⟦rawph:{i}⟧")
}
