//! The window's UI zoom (gpui-pre's `Window::set_ui_zoom`) lays out in zoomed logical pixels
//! and maps platform input into them, as CSS `zoom` on the root element does.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton, ParentElement as _,
    Pixels, Point, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchPhase, Window, div, point,
    px,
};

struct Page {
    hit: Rc<Cell<Option<Point<Pixels>>>>,
    scroll: ScrollHandle,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let hit = self.hit.clone();
        div()
            .size_full()
            .child(
                div()
                    .absolute()
                    .left(px(50.))
                    .top(px(50.))
                    .size(px(50.))
                    .on_mouse_down(MouseButton::Left, move |e, _, _| hit.set(Some(e.position))),
            )
            .child(
                div()
                    .id("scroller")
                    .absolute()
                    .left(px(0.))
                    .top(px(120.))
                    .size(px(100.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().h(px(1000.))),
            )
    }
}

#[gpui_kit::test]
fn zoom_scales_the_layout_and_maps_input(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let hit = Rc::new(Cell::new(None));
    let scroll = ScrollHandle::new();
    let (h, s) = (hit.clone(), scroll.clone());
    let (_, cx) = cx.add_window_view(|_, _| Page { hit: h, scroll: s });
    cx.run_until_parked();
    let (viewport, scale) = cx.update(|window, _| (window.viewport_size(), window.scale_factor()));

    cx.update(|window, cx| window.set_ui_zoom(2., cx));
    cx.run_until_parked();
    cx.update(|window, _| {
        assert_eq!(window.viewport_size(), viewport.map(|v| v / 2.));
        assert_eq!(window.scale_factor(), scale * 2.);
        let b = gpui_kit::Bounds::new(point(px(50.), px(50.)), gpui_kit::size(px(50.), px(50.)));
        assert_eq!(
            window.zoom_to_platform(b),
            gpui_kit::Bounds::new(
                point(px(100.), px(100.)),
                gpui_kit::size(px(100.), px(100.))
            )
        );
    });

    cx.simulate_mouse_down(
        point(px(80.), px(80.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(
        hit.get(),
        None,
        "platform (80, 80) is (40, 40), outside the box"
    );
    cx.simulate_mouse_down(
        point(px(150.), px(150.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(hit.get(), Some(point(px(75.), px(75.))));

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(100.), px(300.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-50.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    assert_eq!(
        f32::from(scroll.offset().y),
        -25.,
        "the content follows the finger"
    );
}
