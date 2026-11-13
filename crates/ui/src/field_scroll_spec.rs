//! A text field's overlay text follows the editor's horizontal scroll in the frame the editor
//! scrolls, not one render later.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::base::input::{Input, InputState};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels, Render, Styled as _,
    TestAppContext, Window, canvas, div, px,
};

use crate::color::hex;
use crate::fonts::Family;
use crate::style::{scroll_follow, text_field};

type Frames = Rc<RefCell<Vec<(Pixels, Pixels)>>>;

struct Root {
    input: Entity<InputState>,
    frames: Frames,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let input = self.input.clone();
        let frames = self.frames.clone();
        let probe = canvas(
            |_, _, _| {},
            move |bounds, _, _, cx| {
                let scroll = input.read(cx).scroll_offset().x;
                frames.borrow_mut().push((bounds.origin.x, scroll));
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size(px(1.));
        div().child(
            div()
                .relative()
                .w(px(40.))
                .h(px(20.))
                .child(Input::new(&self.input))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(scroll_follow(&self.input, probe)),
                ),
        )
    }
}

#[gpui_kit::test]
fn the_overlay_moves_with_the_scroll_in_the_same_frame(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let frames = Frames::default();
    let mut input = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let i = cx.new(|cx| InputState::new(window, cx));
        input = Some(i.clone());
        Root {
            input: i,
            frames: frames.clone(),
        }
    });
    let input = input.expect("input");
    cx.update(|window, cx| input.update(cx, |s, cx| s.focus(window, cx)));
    cx.run_until_parked();
    for _ in 0..30 {
        cx.simulate_input("w");
    }
    let frames = frames.borrow();
    assert!(
        frames.iter().any(|&(_, scroll)| scroll < px(0.)),
        "typing past the edge scrolls the field"
    );
    for &(x, scroll) in frames.iter() {
        assert!(
            (x - scroll).abs() < px(0.5),
            "the overlay at {x:?} sits at the scroll painted with it, {scroll:?}"
        );
    }
}

struct Field {
    input: Entity<InputState>,
}

impl Render for Field {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(40.)).flex().child(text_field(
            &self.input,
            &"".into(),
            hex(0xa9a9a9),
            Family::Mono,
            12.,
            window,
            cx,
        ))
    }
}

#[gpui_kit::test]
fn a_long_paste_scrolls_to_its_end(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let mut input = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let i = cx.new(|cx| InputState::new(window, cx));
        input = Some(i.clone());
        Field { input: i }
    });
    let input = input.expect("input");
    cx.update(|window, cx| input.update(cx, |s, cx| s.focus(window, cx)));
    cx.run_until_parked();
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("w".repeat(30)));
    cx.dispatch_action(gpui_kit::base::input::Paste);
    cx.run_until_parked();
    let scroll = input.read_with(cx, |s, _| s.scroll_offset().x);
    assert!(
        scroll < px(-100.),
        "the field scrolls to the pasted end, {scroll:?}"
    );
}

#[gpui_kit::test]
fn a_paste_that_changes_nothing_keeps_the_selection(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let mut input = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let i = cx.new(|cx| InputState::new(window, cx));
        input = Some(i.clone());
        Field { input: i }
    });
    let input = input.expect("input");
    cx.update(|window, cx| {
        input.update(cx, |s, cx| {
            s.set_value("abc", window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        })
    });
    cx.run_until_parked();
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()));
    cx.dispatch_action(gpui_kit::base::input::Paste);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let range = input.read_with(cx, |s, _| s.selected_range());
    assert_eq!(range, 0..3, "the selection survives an empty paste");
}
