//! A chat code fence scrolls at a capped height, and a vertical wheel over a wide one scrolls
//! it down, not sideways.

use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, point, px};
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
    cx.simulate_event(ScrollWheelEvent {
        position: before.origin + point(px(40.), px(40.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-50.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    let after = cx.debug_bounds("md-pre-content").unwrap();
    assert_eq!(
        after.origin.x, before.origin.x,
        "the fence scrolled sideways"
    );
    assert_eq!(after.origin.y, before.origin.y - px(50.));
}
