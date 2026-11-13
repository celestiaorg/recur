//! Chat code fences scroll at a capped height, a vertical wheel over a wide one scrolls it
//! down, not sideways, and fences and tables keep their scroll while off screen.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Modifiers, Pixels, Point, ScrollDelta, ScrollEvent, TestAppContext,
    TouchPhase, point, px,
};
use recur_content::{MarkdownClass, MarkdownView};

#[gpui::test]
fn a_vertical_wheel_scrolls_a_wide_fence_down(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
    let lines: Vec<String> = (0..60)
        .map(|i| format!("{i} {}", "x".repeat(400)))
        .collect();
    let md = format!("```\n{}\n```\n", lines.join("\n"));
    let (_, cx) =
        cx.add_window_view(|_, cx| MarkdownView::new(md, MarkdownClass::ChatCanvas, false, cx));
    cx.run_until_parked();
    let before = cx
        .debug_bounds("md-pre-content")
        .expect("the fence is drawn");
    cx.simulate_event(ScrollEvent {
        position: before.origin + point(px(40.), px(40.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-50.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    });
    cx.run_until_parked();
    let after = cx.debug_bounds("md-pre-content").unwrap();
    assert_eq!(
        after.origin.x, before.origin.x,
        "the fence scrolled sideways"
    );
    assert_eq!(after.origin.y, before.origin.y - px(50.));
}

struct Toggle(gpui::Entity<MarkdownView>, bool);

impl gpui::Render for Toggle {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::ParentElement as _;
        gpui::div().when(self.1, |d| d.child(self.0.clone()))
    }
}

/// Scrolls the element `selector` draws by `delta`, renders a frame without the document,
/// and returns how far it moved once drawn again.
fn scroll_after_hiding(
    md: String,
    selector: &'static str,
    delta: Point<Pixels>,
    cx: &mut TestAppContext,
) -> Point<Pixels> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
    let (root, cx) = cx.add_window_view(|_, cx| {
        Toggle(
            cx.new(|cx| MarkdownView::new(md, MarkdownClass::ChatCanvas, false, cx)),
            true,
        )
    });
    cx.run_until_parked();
    let before = cx.debug_bounds(selector).expect("drawn");
    cx.simulate_event(ScrollEvent {
        position: before.origin + point(px(20.), px(20.)),
        delta: ScrollDelta::Pixels(delta),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    });
    cx.run_until_parked();
    for show in [false, true] {
        root.update(cx, |r, cx| {
            r.1 = show;
            cx.notify();
        });
        cx.run_until_parked();
    }
    cx.debug_bounds(selector).expect("drawn again").origin - before.origin
}

#[gpui::test]
fn a_fence_keeps_its_scroll_while_off_screen(cx: &mut TestAppContext) {
    let lines: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
    let md = format!("```\n{}\n```\n", lines.join("\n"));
    let moved = scroll_after_hiding(md, "md-pre-content", point(px(0.), px(-50.)), cx);
    assert_eq!(moved, point(px(0.), px(-50.)));
}

#[gpui::test]
fn a_wide_table_keeps_its_scroll_while_off_screen(cx: &mut TestAppContext) {
    let cells = vec!["x"; 120].join(" | ");
    let md = format!("| {cells} |\n|{}\n| {cells} |\n", "---|".repeat(120));
    let moved = scroll_after_hiding(md, "md-table-content", point(px(-50.), px(0.)), cx);
    assert_eq!(moved, point(px(-50.), px(0.)));
}
