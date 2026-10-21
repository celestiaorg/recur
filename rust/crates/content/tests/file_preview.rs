//! `components/FilePreview.test.ts`: the preview find bar opens, counts, follows its toggles,
//! closes on Escape, stays out of inactive and SVG previews, recollects when the content
//! changes, and paints highlights only for the active preview.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
use recur_content::preview::PreviewKind;
use recur_content::preview::scroll::PreviewScrollStore;
use recur_content::preview::view::FilePreview;

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
}

fn mount<'a>(
    cx: &'a mut TestAppContext,
    kind: PreviewKind,
    content: &'static str,
    path: &'static str,
) -> (Entity<FilePreview>, &'a mut VisualTestContext) {
    let scroll = Rc::new(RefCell::new(PreviewScrollStore::new()));
    let (view, cx) = cx.add_window_view(move |window, cx| {
        FilePreview::new(kind, content, path, scroll, window, cx)
    });
    cx.run_until_parked();
    (view, cx)
}

fn focus_preview(view: &Entity<FilePreview>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let h = gpui::Focusable::focus_handle(view.read(cx), cx);
        window.focus(&h, cx);
    });
    cx.run_until_parked();
}

/// `typeFind`: replace the query.
fn type_find(cx: &mut VisualTestContext, q: &str) {
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input(q);
    cx.run_until_parked();
}

fn count(view: &Entity<FilePreview>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| view.read(cx).find_count_label(cx))
}

fn open(view: &Entity<FilePreview>, cx: &mut VisualTestContext) -> bool {
    cx.update(|_, cx| view.read(cx).is_search_open())
}

#[gpui::test]
fn opens_a_find_bar_and_counts_markdown_matches(cx: &mut TestAppContext) {
    init(cx);
    let (view, cx) = mount(
        cx,
        PreviewKind::Markdown,
        "Hello world. Hello again.",
        "README.md",
    );
    assert!(!open(&view, cx));
    cx.update(|window, cx| view.update(cx, |v, cx| assert!(v.toggle_search(window, cx))));
    cx.run_until_parked();
    type_find(cx, "hello");
    assert_eq!(count(&view, cx), "1 / 2");
    // The regexp toggle (the second of three).
    cx.simulate_keystrokes("alt-cmd-r");
    cx.run_until_parked();
    type_find(cx, "H.llo");
    assert_eq!(count(&view, cx), "1 / 2");
}

#[gpui::test]
fn opens_on_cmd_f_and_closes_on_escape(cx: &mut TestAppContext) {
    init(cx);
    let (view, cx) = mount(
        cx,
        PreviewKind::Markdown,
        "needle in a haystack",
        "notes.md",
    );
    focus_preview(&view, cx);
    cx.simulate_keystrokes("cmd-f");
    cx.run_until_parked();
    assert!(open(&view, cx));
    type_find(cx, "needle");
    assert_eq!(count(&view, cx), "1 / 1");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!open(&view, cx));
}

#[gpui::test]
fn does_not_intercept_cmd_f_when_inactive(cx: &mut TestAppContext) {
    init(cx);
    let (view, cx) = mount(cx, PreviewKind::Markdown, "needle", "notes.md");
    cx.update(|window, cx| view.update(cx, |v, cx| v.set_active(false, window, cx)));
    focus_preview(&view, cx);
    cx.simulate_keystrokes("cmd-f");
    cx.run_until_parked();
    assert!(!open(&view, cx));
}

#[gpui::test]
fn does_not_open_find_for_svg_previews(cx: &mut TestAppContext) {
    init(cx);
    let (view, cx) = mount(
        cx,
        PreviewKind::Svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text>needle</text></svg>"#,
        "icon.svg",
    );
    cx.update(|window, cx| view.update(cx, |v, cx| assert!(!v.toggle_search(window, cx))));
    assert!(!open(&view, cx));
    focus_preview(&view, cx);
    cx.simulate_keystrokes("cmd-f");
    cx.run_until_parked();
    assert!(!open(&view, cx));
}

#[gpui::test]
fn recollects_matches_after_the_content_changes(cx: &mut TestAppContext) {
    init(cx);
    let (view, cx) = mount(cx, PreviewKind::Markdown, "nothing yet", "notes.md");
    cx.update(|window, cx| view.update(cx, |v, cx| v.toggle_search(window, cx)));
    cx.run_until_parked();
    type_find(cx, "hello");
    assert_eq!(count(&view, cx), "No results");
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.set_content("nothing yet\n\nhello", window, cx)
        })
    });
    cx.run_until_parked();
    assert_eq!(count(&view, cx), "1 / 1");
}

/// Two previews in one window, as two mounted `FilePreview`s.
struct Pair(Entity<FilePreview>, Entity<FilePreview>);

impl gpui::Render for Pair {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled};
        gpui::div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.0.clone())
            .child(self.1.clone())
    }
}

#[gpui::test]
fn restores_highlights_for_the_active_preview_and_ignores_inactive_paints(cx: &mut TestAppContext) {
    init(cx);
    let (pair, cx) = cx.add_window_view(|window, cx| {
        let make = |content: &'static str,
                    path: &'static str,
                    window: &mut gpui::Window,
                    cx: &mut gpui::Context<Pair>| {
            let scroll = Rc::new(RefCell::new(PreviewScrollStore::new()));
            cx.new(|cx| FilePreview::new(PreviewKind::Markdown, content, path, scroll, window, cx))
        };
        let a = make("alpha needle", "a.md", window, cx);
        let b = make("beta needle", "b.md", window, cx);
        b.update(cx, |v, cx| v.set_active(false, window, cx));
        Pair(a, b)
    });
    cx.run_until_parked();
    let (a, b) = cx.update(|_, cx| (pair.read(cx).0.clone(), pair.read(cx).1.clone()));
    let painted = |v: &Entity<FilePreview>, cx: &mut VisualTestContext| {
        cx.update(|_, cx| v.read(cx).painted_highlights(cx))
    };
    let active = |v: &Entity<FilePreview>, on: bool, cx: &mut VisualTestContext| {
        cx.update(|window, cx| v.update(cx, |v, cx| v.set_active(on, window, cx)));
        cx.run_until_parked();
    };

    cx.update(|window, cx| a.update(cx, |v, cx| v.toggle_search(window, cx)));
    cx.run_until_parked();
    type_find(cx, "needle");
    assert_eq!(painted(&a, cx), ["needle"]);

    active(&a, false, cx);
    active(&b, true, cx);
    assert!(painted(&a, cx).is_empty());
    cx.update(|window, cx| b.update(cx, |v, cx| v.toggle_search(window, cx)));
    cx.run_until_parked();
    type_find(cx, "beta");
    assert_eq!(painted(&b, cx), ["beta"]);
    assert!(painted(&a, cx).is_empty());

    active(&b, false, cx);
    active(&a, true, cx);
    assert_eq!(painted(&a, cx), ["needle"]);
    assert!(painted(&b, cx).is_empty());
}
