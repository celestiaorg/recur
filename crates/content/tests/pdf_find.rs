//! Find in the PDF preview: ⌘F opens the bar over the pages' text layer, it counts and steps
//! through matches, and Escape closes it.

use std::sync::Arc;

use gpui::{TestAppContext, VisualTestContext};
use recur_content::PdfPreview;

const PDF: &[u8] = include_bytes!("../../ui/gallery/content/page.pdf");

fn type_find(cx: &mut VisualTestContext, q: &str) {
    cx.simulate_keystrokes("secondary-a backspace");
    cx.simulate_input(q);
    cx.run_until_parked();
}

#[gpui::test]
fn finds_steps_and_closes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
    let (view, cx) =
        cx.add_window_view(|window, cx| PdfPreview::new(Some(Arc::from(PDF)), window, cx));
    cx.run_until_parked();
    let count = |cx: &mut VisualTestContext| cx.update(|_, cx| view.read(cx).find_count_label(cx));
    cx.update(|window, cx| view.update(cx, |p, cx| p.open_search(window, cx)));
    cx.run_until_parked();
    type_find(cx, "the");
    let total = count(cx);
    assert!(total.starts_with("1 / "), "{total}");
    cx.simulate_keystrokes("secondary-g");
    cx.run_until_parked();
    assert!(count(cx).starts_with("2 / "), "{}", count(cx));
    type_find(cx, "quarterly REPORT");
    assert_eq!(count(cx), "1 / 1");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.update(|_, cx| !view.read(cx).is_search_open()));
    // Closing focuses the pages, where ⌘F opens the bar again.
    cx.simulate_keystrokes("secondary-f");
    cx.run_until_parked();
    assert!(cx.update(|_, cx| view.read(cx).is_search_open()));
}
