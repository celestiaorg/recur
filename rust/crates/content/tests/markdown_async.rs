//! A large markdown document parses on a background thread: `set_text` returns at once and
//! the blocks land on a later frame (a 200 KB file preview froze the window for seconds).

use gpui::TestAppContext;
use recur_content::markdown::peek_markdown_cache;
use recur_content::{MarkdownClass, MarkdownView};

#[gpui::test]
fn a_large_document_renders_off_the_ui_thread(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
    let mut big = String::new();
    for i in 0..20 {
        big.push_str(&format!("Paragraph {i} {}\n\n", "word ".repeat(700)));
    }
    assert!(big.len() > 64 * 1024);
    big.push_str("The last paragraph.\n");
    let (view, cx) = cx.add_window_view(|_, cx| {
        MarkdownView::new("small first", MarkdownClass::ChatCanvas, false, cx)
    });
    cx.run_until_parked();
    let blocks = |cx: &mut gpui::VisualTestContext| {
        view.read_with(cx, |v, _| {
            v.text_blocks().into_iter().map(|b| b.1).collect::<Vec<_>>()
        })
    };
    assert_eq!(blocks(cx), ["small first"]);
    view.update(cx, |v, cx| v.set_text(big.clone(), cx));
    // Not parsed inside set_text: the markdown host has not seen it yet.
    assert!(peek_markdown_cache(&big).is_none());
    assert_eq!(blocks(cx), ["small first"]);
    cx.run_until_parked();
    let after = blocks(cx);
    assert_eq!(after.len(), 21);
    assert_eq!(
        after.last().map(String::as_str),
        Some("The last paragraph.")
    );
}
