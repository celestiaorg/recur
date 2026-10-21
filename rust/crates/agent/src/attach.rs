//! Chat attachment markers and user content blocks (the pure half of Go
//! `attachments.go`; disk I/O lives in the engine).
//!
//! The composer appends `⟦attachment:<id>⟧` after the text in chip order.
//! Claude splits the prompt into interleaved text/image content blocks;
//! Cursor receives text plus `images[]` in marker order.

use std::collections::HashMap;

use recur_domain::ContentBlock;

use crate::gostr::go_prefix;

pub const ATTACHMENT_MARKER_PREFIX: &str = "⟦attachment:";
pub const ATTACHMENT_MARKER_SUFFIX: &str = "⟧";

/// display_text for a stored wire segment that must not appear in the chat
/// bubble (image-first fork inject). Reconstruct/Retry read the text; the
/// transcript flatten treats this label as used-and-empty.
pub const DISPLAY_HIDDEN_SENTINEL: &str = "\u{2064}";

/// Caps one pasted image (matches the observatory preview).
pub const MAX_CHAT_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

/// Accepted chat paste MIME types and their file extensions.
pub const CHAT_IMAGE_MIME: &[(&str, &str)] = &[
    ("image/png", ".png"),
    ("image/jpeg", ".jpg"),
    ("image/jpg", ".jpg"),
    ("image/gif", ".gif"),
    ("image/webp", ".webp"),
];

/// The file extension for an accepted (lowercase, trimmed) MIME type.
pub fn chat_image_ext(mime: &str) -> Option<&'static str> {
    CHAT_IMAGE_MIME
        .iter()
        .find(|(m, _)| *m == mime)
        .map(|(_, e)| *e)
}

/// The stored MIME type for a lowercase extension (`image/jpg` normalizes to
/// `image/jpeg`).
pub fn chat_image_mime_for_ext(ext: &str) -> Option<&'static str> {
    CHAT_IMAGE_MIME
        .iter()
        .find(|(_, e)| *e == ext)
        .map(|(m, _)| if *m == "image/jpg" { "image/jpeg" } else { *m })
}

/// One image attached to a turn (bytes already validated).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserImage {
    pub id: String,
    pub name: String,
    pub mime: String,
    /// Raw base64.
    pub data: String,
    /// Relative to the data root (`workspaces/…/attachments/…`).
    pub path: String,
}

/// The in-text marker for an attachment id.
pub fn marker(id: &str) -> String {
    format!("{ATTACHMENT_MARKER_PREFIX}{id}{ATTACHMENT_MARKER_SUFFIX}")
}

/// One `⟦attachment:<id>⟧` match: byte offsets of the whole marker and of
/// the id inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerSpan {
    pub start: usize,
    pub end: usize,
    pub id_start: usize,
    pub id_end: usize,
}

impl MarkerSpan {
    pub fn id<'a>(&self, s: &'a str) -> &'a str {
        &s[self.id_start..self.id_end]
    }
}

/// Every marker in `s`, left to right (Go `⟦attachment:([^⟧]+)⟧`).
pub fn parse_markers(s: &str) -> Vec<MarkerSpan> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = s[from..].find(ATTACHMENT_MARKER_PREFIX) {
        let start = from + i;
        let id_start = start + ATTACHMENT_MARKER_PREFIX.len();
        let Some(j) = s[id_start..].find(ATTACHMENT_MARKER_SUFFIX) else {
            break;
        };
        if j == 0 {
            from = start + 1;
            while !s.is_char_boundary(from) {
                from += 1;
            }
            continue;
        }
        let id_end = id_start + j;
        let end = id_end + ATTACHMENT_MARKER_SUFFIX.len();
        out.push(MarkerSpan {
            start,
            end,
            id_start,
            id_end,
        });
        from = end;
    }
    out
}

/// Drops marker runs from titles and loop goals.
pub fn strip_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for m in parse_markers(s) {
        out.push_str(&s[last..m.start]);
        last = m.end;
    }
    out.push_str(&s[last..]);
    out.trim().to_owned()
}

/// Appends any missing markers so the provider can pair `imgs` with the
/// prompt body.
pub fn ensure_markers(text: &str, imgs: &[UserImage]) -> String {
    let mut text = text.to_owned();
    for img in imgs {
        let m = marker(&img.id);
        if text.contains(&m) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&m);
    }
    text
}

/// Keeps letters, digits, `-` and `_`, capped at 64 bytes.
// simplify: non-ASCII letters/digits use Rust's Alphabetic/Numeric
// properties, a superset of Go's L/Nd categories. Composer ids are ASCII.
pub fn sanitize_attachment_id(id: &str) -> String {
    let out: String = id
        .trim()
        .chars()
        .filter(|c| c.is_alphabetic() || c.is_numeric() || *c == '-' || *c == '_')
        .collect();
    if out.len() > 64 {
        return go_prefix(&out, 64).into_owned();
    }
    out
}

/// Images ordered as their markers appear in `text`, then any unreferenced
/// ones in their original order.
pub fn images_in_marker_order(text: &str, imgs: &[UserImage]) -> Vec<UserImage> {
    let by_id = by_id(imgs);
    let mut ordered = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for m in parse_markers(text) {
        let id = m.id(text);
        if let Some(img) = by_id.get(id)
            && seen.insert(id.to_owned())
        {
            ordered.push((*img).clone());
        }
    }
    for img in imgs {
        if !seen.contains(&img.id) {
            ordered.push(img.clone());
        }
    }
    ordered
}

/// "first" … "fifth", then "<n>th".
pub fn ordinal_word(n: usize) -> String {
    match n {
        1 => "first".into(),
        2 => "second".into(),
        3 => "third".into(),
        4 => "fourth".into(),
        5 => "fifth".into(),
        _ => format!("{n}th"),
    }
}

/// Splits prompt text on attachment markers into ordered content blocks
/// (text + image). `display` is UI-only: with no images it lands on the
/// single text block; with images, a matching marker layout pairs
/// per-segment display text, otherwise the whole display goes on the first
/// text block only.
///
/// Only markers whose ids are in `imgs` split. A fork inject's copied
/// transcript often quotes Recur source containing `⟦attachment:…⟧` as
/// literal text; treating those as images broke pairing and leaked the wire
/// into the bubble.
pub fn user_content_blocks(text: &str, display: &str, imgs: &[UserImage]) -> Vec<ContentBlock> {
    let by_id = by_id(imgs);
    let text_locs = image_locs(text, &by_id);
    if imgs.is_empty() || text_locs.is_empty() {
        if text.trim().is_empty() {
            return Vec::new();
        }
        return vec![ContentBlock::text_display(text, display)];
    }
    let text_segs = segments(text, &text_locs);
    let mut disp_segs = None;
    if !display.is_empty() {
        let disp_locs = image_locs(display, &by_id);
        if markers_match(text, &text_locs, display, &disp_locs) {
            disp_segs = Some(segments(display, &disp_locs));
        }
    }
    let mut blocks = Vec::new();
    let mut display_assigned = false;
    for (i, seg) in text_segs.iter().enumerate() {
        if !seg.is_empty() {
            let mut dt = "";
            if let Some(ds) = &disp_segs {
                dt = ds[i];
                // Image-first composer: the paired display segment before the
                // marker is empty while the wire segment is the whole fork
                // inject. Keep the wire for Retry; hide it in the bubble.
                if dt.is_empty() {
                    dt = DISPLAY_HIDDEN_SENTINEL;
                }
            } else if !display_assigned && !display.is_empty() {
                dt = display;
                display_assigned = true;
            }
            blocks.push(ContentBlock::text_display(seg, dt));
        }
        let Some(loc) = text_locs.get(i) else {
            continue;
        };
        match by_id.get(loc.id(text)) {
            Some(img) => blocks.push(ContentBlock::image(
                &img.id, &img.name, &img.mime, &img.path,
            )),
            None => blocks.push(ContentBlock::text(&text[loc.start..loc.end])),
        }
    }
    blocks.retain(|b| b.as_text().is_none_or(|p| !p.text.is_empty()));
    blocks
}

fn by_id(imgs: &[UserImage]) -> HashMap<&str, &UserImage> {
    imgs.iter().map(|i| (i.id.as_str(), i)).collect()
}

fn image_locs(s: &str, by_id: &HashMap<&str, &UserImage>) -> Vec<MarkerSpan> {
    if by_id.is_empty() {
        return Vec::new();
    }
    parse_markers(s)
        .into_iter()
        .filter(|m| by_id.contains_key(m.id(s)))
        .collect()
}

fn segments<'a>(s: &'a str, locs: &[MarkerSpan]) -> Vec<&'a str> {
    let mut segs = Vec::with_capacity(locs.len() + 1);
    let mut last = 0;
    for l in locs {
        segs.push(&s[last..l.start]);
        last = l.end;
    }
    segs.push(&s[last..]);
    segs
}

fn markers_match(a: &str, a_locs: &[MarkerSpan], b: &str, b_locs: &[MarkerSpan]) -> bool {
    a_locs.len() == b_locs.len() && a_locs.iter().zip(b_locs).all(|(x, y)| x.id(a) == y.id(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(id: &str) -> UserImage {
        UserImage {
            id: id.into(),
            name: format!("{id}.png"),
            mime: "image/png".into(),
            path: format!("workspaces/W/attachments/{id}.png"),
            ..Default::default()
        }
    }

    #[test]
    fn strip_markers_trims() {
        let input = format!("look at this\n{}\nplease", marker("img1"));
        assert_eq!(strip_markers(&input), "look at this\n\nplease");
        assert_eq!(strip_markers(&marker("img1")), "");
    }

    #[test]
    fn parse_markers_skips_empty_ids() {
        let s = "a ⟦attachment:⟧ ⟦attachment:x⟧ ⟦attachment:y";
        let ids: Vec<_> = parse_markers(s)
            .iter()
            .map(|m| m.id(s).to_owned())
            .collect();
        assert_eq!(ids, ["x"]);
        let s = "⟦attachment:a\nb⟧";
        assert_eq!(parse_markers(s)[0].id(s), "a\nb");
    }

    #[test]
    fn ensure_markers_appends_missing() {
        let got = ensure_markers("look", &[img("img1")]);
        assert_eq!(got, format!("look\n{}", marker("img1")));
        assert_eq!(ensure_markers(&got, &[img("img1")]), got);
        assert_eq!(ensure_markers("", &[img("a")]), marker("a"));
    }

    #[test]
    fn content_blocks_order() {
        let text = format!("before {} after", marker("a"));
        let blocks = user_content_blocks(&text, "", &[img("a")]);
        assert_eq!(blocks.len(), 3);
        assert!(blocks[0].as_text().unwrap().text.contains("before"));
        assert_eq!(blocks[1].as_image().unwrap().id, "a");
        assert!(blocks[2].as_text().unwrap().text.contains("after"));
    }

    #[test]
    fn content_blocks_display_with_images() {
        let m = marker("a");
        let blocks = user_content_blocks(&format!("W1 {m} W2"), &format!("D1 {m} D2"), &[img("a")]);
        assert_eq!(blocks.len(), 3);
        let p = blocks[0].as_text().unwrap();
        assert_eq!((p.text.as_str(), p.display_text.as_str()), ("W1 ", "D1 "));
        let p = blocks[2].as_text().unwrap();
        assert_eq!((p.text.as_str(), p.display_text.as_str()), (" W2", " D2"));

        let blocks = user_content_blocks(&format!("W1 {m} W2"), "composer label", &[img("a")]);
        assert_eq!(blocks[0].as_text().unwrap().display_text, "composer label");
        assert_eq!(blocks[2].as_text().unwrap().display_text, "");
    }

    #[test]
    fn content_blocks_ignore_foreign_markers() {
        let m = marker("shot");
        let composer = format!("please continue {m} thanks");
        let preamble = "Fork <transcript>\nvar re = \"⟦attachment:other⟧\"\n</transcript>\n";
        let blocks =
            user_content_blocks(&format!("{preamble}{composer}"), &composer, &[img("shot")]);
        assert_eq!(blocks.len(), 3);
        let first = blocks[0].as_text().unwrap();
        assert!(first.text.contains("attachment:other"));
        assert_eq!(first.display_text, "please continue ");
        assert_eq!(blocks[1].as_image().unwrap().id, "shot");
        assert_eq!(blocks[2].as_text().unwrap().display_text, " thanks");
    }

    #[test]
    fn content_blocks_image_first_hides_preamble() {
        let m = marker("shot");
        let preamble = "Fork <transcript>\nold reply\n</transcript>\n";
        let blocks = user_content_blocks(&format!("{preamble}{m}"), &m, &[img("shot")]);
        assert_eq!(blocks.len(), 2);
        let p = blocks[0].as_text().unwrap();
        assert_eq!(
            (p.text.as_str(), p.display_text.as_str()),
            (preamble, DISPLAY_HIDDEN_SENTINEL)
        );
        assert_eq!(blocks[1].as_image().unwrap().id, "shot");

        let composer = format!("{m} cues from the inbox");
        let blocks =
            user_content_blocks(&format!("{preamble}{composer}"), &composer, &[img("shot")]);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0].as_text().unwrap().display_text,
            DISPLAY_HIDDEN_SENTINEL
        );
        assert_eq!(
            blocks[2].as_text().unwrap().display_text,
            " cues from the inbox"
        );
    }

    #[test]
    fn content_blocks_without_images() {
        assert!(user_content_blocks("  ", "x", &[]).is_empty());
        let b = user_content_blocks("hi", "shown", &[]);
        assert_eq!(b, vec![ContentBlock::text_display("hi", "shown")]);
        let text = format!("x {}", marker("gone"));
        assert_eq!(
            user_content_blocks(&text, "", &[img("a")]),
            vec![ContentBlock::text(&text)]
        );
    }

    #[test]
    fn marker_order_and_ordinals() {
        let text = format!("{} mid {}", marker("img_a"), marker("img_b"));
        let ordered = images_in_marker_order(&text, &[img("img_c"), img("img_b"), img("img_a")]);
        let ids: Vec<_> = ordered.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["img_a", "img_b", "img_c"]);
        assert_eq!(ordinal_word(2), "second");
        assert_eq!(ordinal_word(6), "6th");
    }

    #[test]
    fn sanitize_and_mime() {
        assert_eq!(sanitize_attachment_id(" a/b..c-d_e "), "abc-d_e");
        assert_eq!(sanitize_attachment_id(&"x".repeat(70)).len(), 64);
        assert_eq!(chat_image_ext("image/jpg"), Some(".jpg"));
        assert_eq!(chat_image_mime_for_ext(".jpg"), Some("image/jpeg"));
        assert_eq!(chat_image_ext("image/bmp"), None);
    }
}
