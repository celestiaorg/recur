//! Panics GPUI contains (Recur's gpui-pre patch, `rust/vendor/README.md` "Contained panics"):
//! one in an element handler, a nested `entity.update`, a `canvas`, a task or a window update
//! leaves the app running, with every lease and the window back in place.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestAppContext,
    VisualTestContext, Window, canvas, div, px, take_contained_panics,
};

struct Counter(u32);

struct Host {
    outer: Entity<Counter>,
    inner: Entity<Counter>,
    canvas_panics: Rc<Cell<bool>>,
    painted: Rc<Cell<u32>>,
    focus: FocusHandle,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (outer, inner) = (self.outer.clone(), self.inner.clone());
        let ok = self.outer.clone();
        let keys = self.outer.clone();
        let (panics, painted) = (self.canvas_panics.clone(), self.painted.clone());
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(move |ev, _, cx| {
                if ev.keystroke.key == "x" {
                    panic!("key listener");
                }
                keys.update(cx, |c, _| c.0 += 100);
            })
            .child(
                div()
                    .id("boom")
                    .debug_selector(|| "boom".into())
                    .size(px(40.))
                    .on_click(move |_, _, cx| {
                        outer.update(cx, |o, cx| {
                            o.0 += 1;
                            inner.update(cx, |_, _| panic!("nested update"));
                        });
                    }),
            )
            .child(
                div()
                    .id("ok")
                    .debug_selector(|| "ok".into())
                    .size(px(40.))
                    .on_click(move |_, _, cx| ok.update(cx, |c, _| c.0 += 10)),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        window.with_content_mask(None, |_| {
                            if panics.get() {
                                panic!("canvas paint");
                            }
                        });
                        painted.set(painted.get() + 1);
                    },
                )
                .size(px(10.)),
            )
    }
}

fn mount(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
    cx.add_window_view(|window, cx| {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Host {
            outer: cx.new(|_| Counter(0)),
            inner: cx.new(|_| Counter(0)),
            canvas_panics: Rc::default(),
            painted: Rc::default(),
            focus,
        }
    })
}

fn sites(panics: Vec<String>) -> Vec<String> {
    panics
        .into_iter()
        .map(|p| p.split(':').next().unwrap_or_default().to_string())
        .collect()
}

#[gpui_kit::test]
fn a_panic_in_a_click_handler_returns_both_leases(cx: &mut TestAppContext) {
    let (host, cx) = mount(cx);
    let boom = cx.debug_bounds("boom").expect("boom").center();
    cx.simulate_click(boom, Default::default());
    assert_eq!(sites(take_contained_panics()), ["a mouse listener"]);
    let (outer, inner) = host.read_with(cx, |h, _| (h.outer.clone(), h.inner.clone()));
    // Both entities are back in the map: they read and update again.
    assert_eq!(outer.read_with(cx, |c, _| c.0), 1);
    inner.update(cx, |c, _| c.0 += 1);
    let ok = cx.debug_bounds("ok").expect("ok").center();
    cx.simulate_click(ok, Default::default());
    assert_eq!(outer.read_with(cx, |c, _| c.0), 11);
    assert!(take_contained_panics().is_empty());
}

#[gpui_kit::test]
fn a_panic_in_a_key_listener_leaves_the_next_key_working(cx: &mut TestAppContext) {
    let (host, cx) = mount(cx);
    cx.simulate_keystrokes("x");
    assert_eq!(sites(take_contained_panics()), ["a key listener"]);
    cx.simulate_keystrokes("y");
    let outer = host.read_with(cx, |h, _| h.outer.clone());
    assert_eq!(outer.read_with(cx, |c, _| c.0), 100);
}

#[gpui_kit::test]
fn a_panic_in_a_canvas_paints_the_rest_of_the_frame(cx: &mut TestAppContext) {
    let (host, cx) = mount(cx);
    let (panics, painted) = host.read_with(cx, |h, _| (h.canvas_panics.clone(), h.painted.clone()));
    let before = painted.get();
    panics.set(true);
    host.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert!(sites(take_contained_panics()).contains(&"a canvas paint".to_string()));
    assert_eq!(painted.get(), before);
    // The frame was drawn around it: the buttons still hit-test and respond.
    let ok = cx.debug_bounds("ok").expect("ok").center();
    cx.simulate_click(ok, Default::default());
    let outer = host.read_with(cx, |h, _| h.outer.clone());
    assert_eq!(outer.read_with(cx, |c, _| c.0), 10);
    panics.set(false);
    host.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert!(painted.get() > before);
    take_contained_panics();
}

#[gpui_kit::test]
fn a_panicking_task_fails_alone_and_its_awaiter_with_it(cx: &mut TestAppContext) {
    let (host, cx) = mount(cx);
    let outer = host.read_with(cx, |h, _| h.outer.clone());
    let reached = Rc::new(Cell::new(false));
    cx.update(|_, cx| {
        cx.spawn(async move |_| panic!("foreground task")).detach();
        let bg = cx
            .background_executor()
            .spawn(async { panic!("background task") });
        let reached = reached.clone();
        cx.spawn(async move |_| {
            bg.await;
            reached.set(true);
        })
        .detach();
    });
    cx.run_until_parked();
    assert_eq!(
        take_contained_panics().len(),
        3,
        "both tasks and the awaiter"
    );
    assert!(!reached.get());
    // The executor and the entities keep working.
    let o = outer.clone();
    cx.update(|_, cx| {
        cx.spawn(async move |cx| o.update(cx, |c, _| c.0 = 7))
            .detach()
    });
    cx.run_until_parked();
    assert_eq!(outer.read_with(cx, |c, _| c.0), 7);
}

#[gpui_kit::test]
fn an_unwind_out_of_a_window_update_puts_the_window_back(cx: &mut TestAppContext) {
    let (host, cx) = mount(cx);
    let outer = host.read_with(cx, |h, _| h.outer.clone());
    let caught = catch_unwind(AssertUnwindSafe(|| {
        cx.update(|_, cx| outer.update(cx, |_, _| panic!("window update")))
    }));
    assert!(caught.is_err());
    // The window and the entity are back: both update, and the window still draws and hits.
    cx.update(|window, cx| {
        outer.update(cx, |c, _| {
            c.0 = f32::from(window.bounds().size.width) as u32
        })
    });
    assert!(outer.read_with(cx, |c, _| c.0) > 0);
    let ok = cx.debug_bounds("ok").expect("ok").center();
    cx.simulate_click(ok, Default::default());
    assert!(take_contained_panics().is_empty());
}
