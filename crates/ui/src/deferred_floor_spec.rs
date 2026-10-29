//! A deferred draw nested in another never paints below it (gpui-pre's priority floor), as a
//! popover inside a modal stays over the modal.

use std::cell::RefCell;

use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, Window, canvas,
    deferred, div,
};

thread_local! {
    static PAINTED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

fn mark(name: &'static str) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, _, _| PAINTED.with(|p| p.borrow_mut().push(name)),
    )
    .size_full()
}

struct Page;

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(deferred(div().size_full().child(mark("handle"))).with_priority(50))
            .child(
                deferred(
                    div()
                        .size_full()
                        .child(mark("modal"))
                        .child(deferred(mark("popover")).with_priority(5)),
                )
                .with_priority(100),
            )
    }
}

#[gpui_kit::test]
fn a_nested_deferred_paints_over_its_parent(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Page);
    PAINTED.with(|p| p.borrow_mut().clear());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let order = PAINTED.with(|p| p.borrow().clone());
    assert_eq!(order, ["handle", "modal", "popover"]);
}
