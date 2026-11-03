//! Find-in-page over a preview's rendered text. The haystack joins the
//! rendered text blocks with newlines (inline runs inside a block concatenate, so matches span
//! formatting but never blocks); matches are capped at 999.

use std::ops::Range;

use crate::js::Re;

pub const PREVIEW_FIND_CAP: usize = 999;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub case_sensitive: bool,
    pub regexp: bool,
    pub whole_word: bool,
}

fn escape_regexp(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn compile(query: &str, opts: FindOptions) -> Option<Re> {
    let src = if opts.regexp {
        query.to_string()
    } else {
        escape_regexp(query)
    };
    let flags = if opts.case_sensitive { "g" } else { "gi" };
    regress::Regex::with_flags(&src, flags).ok()?;
    Some(Re::new(&src, flags))
}

/// `WORD_CHAR`: `/[\p{L}\p{N}_]/u`.
fn is_word_char(c: char) -> bool {
    static RE: std::sync::OnceLock<Re> = std::sync::OnceLock::new();
    let mut buf = [0u8; 4];
    RE.get_or_init(|| Re::new(r"^[\p{L}\p{N}_]$", "u"))
        .test(c.encode_utf8(&mut buf))
}

fn whole_word(hay: &str, start: usize, end: usize) -> bool {
    let before = hay[..start].chars().next_back();
    let after = hay[end..].chars().next();
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

fn matches_in(hay: &str, rx: &Re, whole: bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(m) = rx.exec_at(hay, at) {
        let r = m.range();
        if r.is_empty() {
            at = r.end + hay[r.end..].chars().next().map_or(1, char::len_utf8);
            if at > hay.len() {
                break;
            }
            continue;
        }
        at = r.end;
        if !whole || whole_word(hay, r.start, r.end) {
            out.push(r);
        }
        if out.len() >= PREVIEW_FIND_CAP {
            break;
        }
    }
    out
}

/// `findInText`: matches in one string (byte ranges); `None` when the regexp is invalid.
pub fn find_in_text(hay: &str, query: &str, opts: FindOptions) -> Option<Vec<Range<usize>>> {
    if query.is_empty() {
        return Some(Vec::new());
    }
    let rx = compile(query, opts)?;
    Some(matches_in(hay, &rx, opts.whole_word))
}

/// A match located in a rendered block: (block id, byte range in the block).
pub type BlockRange = (usize, Range<usize>);

/// `collectPreviewFindRanges` over rendered blocks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewFindMatch {
    pub ranges: Vec<BlockRange>,
    pub capped: bool,
    pub valid: bool,
}

/// Matches across `blocks` (id, text) joined by newlines, never spanning two blocks.
pub fn collect_preview_find_ranges(
    blocks: &[(usize, String)],
    query: &str,
    opts: FindOptions,
) -> PreviewFindMatch {
    if query.is_empty() {
        return PreviewFindMatch {
            valid: true,
            ..Default::default()
        };
    }
    let Some(rx) = compile(query, opts) else {
        return PreviewFindMatch::default();
    };
    let mut hay = String::new();
    let mut pieces: Vec<(usize, usize, usize)> = Vec::new();
    for (id, text) in blocks {
        if text.is_empty() {
            continue;
        }
        if !hay.is_empty() {
            hay.push('\n');
        }
        pieces.push((*id, hay.len(), text.len()));
        hay.push_str(text);
    }
    let mut out = PreviewFindMatch {
        valid: true,
        ..Default::default()
    };
    for hit in matches_in(&hay, &rx, opts.whole_word) {
        let Some(&(id, from, len)) = pieces
            .iter()
            .find(|(_, from, len)| hit.start >= *from && hit.start < from + len)
        else {
            continue;
        };
        if hit.end > from + len {
            continue;
        }
        out.ranges.push((id, hit.start - from..hit.end - from));
        if out.ranges.len() >= PREVIEW_FIND_CAP {
            out.capped = true;
            break;
        }
    }
    out
}

/// `previewFindCountLabel`.
pub fn preview_find_count_label(index: usize, total: usize, capped: bool) -> String {
    if total == 0 {
        return "No results".into();
    }
    if capped {
        format!("{index} / {PREVIEW_FIND_CAP}+")
    } else {
        format!("{index} / {total}")
    }
}

/// `revealPreviewFindRange` along one axis: the scroll offset that centres `box_mid` in a
/// viewport of `view_len` (box and view in the same coordinates), clamped to the scroll range.
pub fn reveal_offset(scroll: f32, max: f32, box_mid: f32, view_mid: f32) -> f32 {
    let delta = box_mid - view_mid;
    if delta.abs() < 1. {
        return scroll;
    }
    (scroll + delta).clamp(0., max.max(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(b: &[&str]) -> Vec<(usize, String)> {
        b.iter()
            .enumerate()
            .map(|(i, s)| (i, s.to_string()))
            .collect()
    }

    #[test]
    fn finds_case_insensitive_matches_across_inline_runs() {
        let m = collect_preview_find_ranges(
            &blocks(&["Hello hello HELLO"]),
            "hello",
            FindOptions::default(),
        );
        assert!(!m.capped);
        assert_eq!(m.ranges.len(), 3);
        assert_eq!(m.ranges[0], (0, 0..5));
    }

    #[test]
    fn honours_case_sensitive() {
        let b = blocks(&["Hello hello"]);
        let cs = FindOptions {
            case_sensitive: true,
            ..Default::default()
        };
        assert_eq!(collect_preview_find_ranges(&b, "Hello", cs).ranges.len(), 1);
        assert_eq!(
            collect_preview_find_ranges(&b, "Hello", FindOptions::default())
                .ranges
                .len(),
            2
        );
    }

    #[test]
    fn matches_inline_boundaries_but_not_blocks() {
        // `<p>Hello <strong>world</strong></p>` is one block.
        assert_eq!(
            collect_preview_find_ranges(
                &blocks(&["Hello world"]),
                "Hello world",
                FindOptions::default()
            )
            .ranges
            .len(),
            1
        );
        // `<p>Hello</p><p>world</p>`.
        assert!(
            collect_preview_find_ranges(
                &blocks(&["Hello", "world"]),
                "Hello world",
                FindOptions::default()
            )
            .ranges
            .is_empty()
        );
    }

    #[test]
    fn whole_word_applies_to_concatenated_inline_text() {
        let b = blocks(&["hello"]);
        let ww = FindOptions {
            whole_word: true,
            ..Default::default()
        };
        assert!(collect_preview_find_ranges(&b, "lo", ww).ranges.is_empty());
        assert_eq!(collect_preview_find_ranges(&b, "hello", ww).ranges.len(), 1);
    }

    #[test]
    fn empty_query_and_invalid_regexp() {
        let b = blocks(&["hello"]);
        assert!(
            collect_preview_find_ranges(&b, "", FindOptions::default())
                .ranges
                .is_empty()
        );
        let re = FindOptions {
            regexp: true,
            ..Default::default()
        };
        assert!(!collect_preview_find_ranges(&b, "(", re).valid);
    }

    #[test]
    fn find_in_text_regex_whole_word_and_invalid() {
        let re = FindOptions {
            regexp: true,
            ..Default::default()
        };
        assert_eq!(
            find_in_text("hello hallo", "h.llo", re),
            Some(vec![0..5, 6..11])
        );
        assert_eq!(find_in_text("hello", "(", re), None);
        let ww = FindOptions {
            whole_word: true,
            ..Default::default()
        };
        assert_eq!(
            find_in_text("hello sayhello hello.", "hello", ww),
            Some(vec![0..5, 15..20])
        );
        let both = FindOptions {
            regexp: true,
            whole_word: true,
            ..Default::default()
        };
        assert_eq!(
            find_in_text("cat cats cat", "cat", both),
            Some(vec![0..3, 9..12])
        );
    }

    #[test]
    fn count_label() {
        assert_eq!(preview_find_count_label(0, 0, false), "No results");
        assert_eq!(preview_find_count_label(2, 5, false), "2 / 5");
        assert_eq!(
            preview_find_count_label(1, PREVIEW_FIND_CAP, true),
            "1 / 999+"
        );
    }

    #[test]
    fn reveal_centres_and_clamps() {
        // Scroller 80..280 (centre 180), match at 900..924 (centre 912): scrolls down.
        assert!(reveal_offset(0., 1800., 912., 180.) > 0.);
        assert_eq!(reveal_offset(0., 0., 912., 180.), 0.);
        assert_eq!(reveal_offset(40., 100., 180.5, 180.), 40.);
    }
}
