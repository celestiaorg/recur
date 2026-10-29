//! A scroller inside another takes the wheel while it can move and passes it on at its edge
//! (gpui-pre's scroll listener), as a tool's output scrolls inside the chat transcript.

use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Render,
    ScrollDelta, ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, TouchPhase, VisualTestContext, Window, div, point, px,
};

struct Page {
    outer: ScrollHandle,
    inner: ScrollHandle,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("outer")
                .h(px(200.))
                .overflow_y_scroll()
                .track_scroll(&self.outer)
                .child(div().h(px(100.)))
                .child(
                    div()
                        .id("inner")
                        .h(px(100.))
                        .overflow_y_scroll()
                        .track_scroll(&self.inner)
                        .child(div().h(px(400.))),
                )
                .child(div().h(px(400.))),
        )
    }
}

fn wheel(dy: f32, cx: &mut VisualTestContext) {
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(10.), px(150.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_inner_scroller_takes_the_wheel_until_its_edge(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let (outer, inner) = (ScrollHandle::new(), ScrollHandle::new());
    let (o, i) = (outer.clone(), inner.clone());
    let (_, cx) = cx.add_window_view(|_, _| Page { outer: o, inner: i });
    cx.run_until_parked();
    let at = || (f32::from(outer.offset().y), f32::from(inner.offset().y));

    wheel(-50., cx);
    assert_eq!(at(), (0., -50.));
    wheel(-1000., cx);
    assert_eq!(at(), (0., -300.), "the inner stops at its end");
    wheel(-50., cx);
    assert_eq!(at(), (-50., -300.), "at its edge the outer scrolls");
}
