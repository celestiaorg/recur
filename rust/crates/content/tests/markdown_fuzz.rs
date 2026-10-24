//! Seeded random markdown through the full pipeline (marked, DOMPurify, the HTML parser): the
//! lexer and sanitizer are JavaScript ports that slice by byte offset where JS sliced by UTF-16
//! unit, so mixed multi-byte text around markdown syntax must never panic.

use recur_content::markdown::{ParseOptions, render_markdown};
use recur_content::render::doc::Doc;

const PIECES: &[&str] = &[
    "*",
    "**",
    "_",
    "__",
    "`",
    "``",
    "```",
    "~~",
    "~",
    "[",
    "]",
    "(",
    ")",
    "<",
    ">",
    "#",
    "# ",
    "## ",
    "- ",
    "* ",
    "+ ",
    "1. ",
    "2) ",
    "> ",
    "|",
    "| --- |",
    ":-:",
    "\\",
    "\n",
    "\n\n",
    "  ",
    "\t",
    "    ",
    "---",
    "===",
    "&amp;",
    "&#x1F600;",
    "&",
    "!",
    "![",
    "](",
    "\"",
    "'",
    ":",
    "/",
    "http://a.b/c",
    "www.x.y",
    "a@b.c",
    "<br>",
    "<div>",
    "</div>",
    "<img src=x>",
    "<a href=\"",
    "[^1]",
    "- [ ] ",
    "- [x] ",
    "é",
    "漢字",
    "😀",
    "👍🏽",
    "\u{200b}",
    "\u{fe0f}",
    "\u{301}",
    "İ",
    "ß",
    "ﬁ",
    "\r\n",
    "x",
    "word",
    " ",
    "$",
    "{",
    "}",
    "=",
    ".",
    ",",
    "@",
    "^",
    "%",
    "```mermaid\n",
    "```rust\n",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn case(rng: &mut Rng) -> String {
    let n = 1 + rng.below(48);
    (0..n).map(|_| PIECES[rng.below(PIECES.len())]).collect()
}

#[test]
fn random_markdown_never_panics() {
    let iterations: usize = std::env::var("RECUR_MD_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for i in 0..iterations {
        let src = case(&mut rng);
        let out = std::panic::catch_unwind(|| {
            for demote_images in [false, true] {
                if let Ok(html) = render_markdown(&src, ParseOptions { demote_images }) {
                    Doc::parse(&html);
                }
            }
        });
        assert!(out.is_ok(), "case {i} panicked: {src:?}");
    }
}

/// Random markdown laid out and painted at several widths, through the inline flow, line
/// breaking, inline code boxes, tables and the find mirror. Panics inside layout are contained
/// (text lays out empty), so a hook counts them.
#[gpui::test]
fn random_markdown_lays_out_at_any_width(cx: &mut gpui::TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static PANICS: AtomicUsize = AtomicUsize::new(0);
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICS.fetch_add(1, Ordering::SeqCst);
        prev(info);
    }));
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
    let iterations: usize = std::env::var("RECUR_MD_LAYOUT_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    let (view, cx) = cx.add_window_view(|_, cx| {
        recur_content::MarkdownView::new("", recur_content::MarkdownClass::ChatCanvas, false, cx)
    });
    let mut rng = Rng(0x9e37_79b9_7f4b_dbf5);
    for i in 0..iterations {
        let src = case(&mut rng);
        let width = [24., 80., 190., 420., 900.][rng.below(5)];
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
        view.update(cx, |v, cx| v.set_text(src.clone(), cx));
        cx.run_until_parked();
        assert_eq!(
            PANICS.load(Ordering::SeqCst),
            0,
            "case {i} at {width}px: {src:?}"
        );
        assert!(
            cx.debug_bounds("guard-failed").is_none(),
            "case {i}: {src:?}"
        );
    }
}
