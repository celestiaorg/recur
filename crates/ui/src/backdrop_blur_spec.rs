//! Backdrop blurs (`style::backdrop_blur`): a blur ends the scene's batches at its draw order,
//! and replayed frames (an isolated view drawn again, a cached view copied) keep it in order.

use std::cell::Cell;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px,
};

use crate::color::rgba;
use crate::style::{backdrop_blur, cached_view, isolated_view};

thread_local! {
    static RENDERS: Cell<u32> = const { Cell::new(0) };
}

struct Field;

impl Render for Field {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        RENDERS.with(|r| r.set(r.get() + 1));
        div().size_full().bg(gpui_kit::blue())
    }
}

/// A scrim over the page with a panel on it.
struct Overlay;

impl Render for Overlay {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(backdrop_blur(2., rgba(5, 7, 10, 0.55)))
            .child(div().w(px(40.)).h(px(20.)).bg(gpui_kit::red()))
    }
}

struct Host {
    field: Entity<Field>,
    overlay: Entity<Overlay>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .bg(gpui_kit::white())
            .child(isolated_view(&self.field, page_style()))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(cached_view(&self.overlay, page_style())),
            )
    }
}

fn page_style() -> gpui_kit::StyleRefinement {
    let mut s = div().size_full();
    s.style().clone()
}

fn labels(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| window.scene_batch_labels())
}

#[gpui_kit::test]
fn a_blur_ends_the_batches_and_survives_replays(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let mut parts = None;
    let (host, cx) = cx.add_window_view(|_, cx| {
        let field = cx.new(|_| Field);
        let overlay = cx.new(|_| Overlay);
        parts = Some(field.clone());
        Host { field, overlay }
    });
    cx.run_until_parked();
    let field = parts.unwrap();

    // The page and the field under the blur, the scrim's fill and the panel over it.
    let frame = labels(cx);
    assert_eq!(
        frame,
        ["quads (2)", "backdrop blurs (1)", "quads (2)"],
        "the blur splits the quads"
    );

    for _ in 0..3 {
        RENDERS.with(|r| r.set(0));
        cx.update(|_, cx| field.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert_eq!(RENDERS.with(Cell::get), 1, "the field draws again alone");
        assert_eq!(labels(cx), frame, "a replayed root keeps the blur in order");

        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert_eq!(
            labels(cx),
            frame,
            "a replayed cached view keeps the blur in order"
        );
    }
}
