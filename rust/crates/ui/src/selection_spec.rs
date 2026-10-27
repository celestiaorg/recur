//! Selection across text elements in a window, against what WebKit copies for the same rows
//! (`.hline` grid rows: two line-number cells and a code cell, measured in Playwright's WebKit).

use gpui_kit::{
    Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, point, px,
};

use crate::fonts::Family;
use crate::selection::{self, State};
use crate::text::{LineHeight, TextStyled as _, text};

const ROWS: [(&str, &str, &str); 4] = [
    ("1", "1", "  let a = 1;"),
    ("2", "", "- let b = 2;"),
    ("", "2", "+ let b = 3;"),
    ("3", "3", "    println!(\"{a}\");"),
];
const LH: f32 = 12. * 1.6;
const CODE_X: f32 = 46. + 46. + 14.;

struct Root {
    state: Option<Entity<State>>,
    disabled: bool,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = selection::state("sel", window, cx);
        self.state = Some(state.clone());
        // `.hline .cd { white-space: pre-wrap }`, as the diff draws code.
        let cell = |s: &str, key: u64| {
            text(s.to_string())
                .family(Family::Mono)
                .size(12.)
                .line_height(LineHeight::Scale(1.6))
                .pre_wrap()
                .selectable(key, "\n")
        };
        let rows = ROWS.iter().enumerate().map(|(i, (o, n, c))| {
            let k = i as u64 * 3;
            div()
                .flex()
                .w_full()
                .child(div().w(px(46.)).child(cell(o, k)))
                .child(div().w(px(46.)).child(cell(n, k + 1)))
                .child(div().flex_1().px(px(14.)).child(cell(c, k + 2)))
        });
        let content = div().flex().flex_col().w(px(400.)).children(rows);
        div()
            .size_full()
            .child(selection::scope(&state, self.disabled, content, cx))
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Root>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let (root, vcx) = cx.add_window_view(|_, _| Root {
        state: None,
        disabled: false,
    });
    vcx.run_until_parked();
    (root, vcx)
}

/// The middle of the gap after `chars` characters of row `row`'s code.
fn at(row: usize, chars: f32, cx: &mut VisualTestContext) -> gpui_kit::Point<gpui_kit::Pixels> {
    let adv = cx.update(|_, _| crate::font_metrics::zero_advance(&Family::Mono, 12.));
    point(px(CODE_X + chars * adv), px(row as f32 * LH + LH / 2.))
}

fn copied(root: &Entity<Root>, cx: &mut VisualTestContext) -> String {
    let state = root.read_with(cx, |r, _| r.state.clone()).expect("scope");
    cx.simulate_keystrokes("secondary-c");
    let clip = cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    let held = cx.update(|_, cx| selection::selected_text(&state, cx));
    assert_eq!(
        clip.as_deref(),
        Some(held.as_str()),
        "⌘C copies the selection"
    );
    held
}

fn drag(
    from: gpui_kit::Point<gpui_kit::Pixels>,
    to: gpui_kit::Point<gpui_kit::Pixels>,
    cx: &mut VisualTestContext,
) {
    cx.simulate_mouse_move(from, None, gpui_kit::Modifiers::none());
    cx.simulate_mouse_down(
        from,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::none(),
    );
    cx.run_until_parked();
    for i in 1..=4 {
        let p = from + (to - from) * (i as f32 / 4.);
        cx.simulate_mouse_move(
            p,
            Some(gpui_kit::MouseButton::Left),
            gpui_kit::Modifiers::none(),
        );
        cx.run_until_parked();
    }
    cx.simulate_mouse_up(to, gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_drag_across_rows_copies_what_webkit_copies(cx: &mut TestAppContext) {
    let (root, cx) = open(cx);
    // WebKit: "t a = 1;\n2\n- let b = 2;\n2\n+ let " (number cells copy; empty ones add nothing).
    let (a, b) = (at(0, 4., cx), at(2, 6., cx));
    drag(a, b, cx);
    assert_eq!(copied(&root, cx), "t a = 1;\n2\n- let b = 2;\n2\n+ let ");
}

#[gpui_kit::test]
fn select_all_copies_every_row_like_webkits_whole_row_drag(cx: &mut TestAppContext) {
    let (root, cx) = open(cx);
    let p = at(1, 3., cx);
    cx.simulate_click(p, gpui_kit::Modifiers::none());
    cx.simulate_keystrokes("secondary-a");
    assert_eq!(
        copied(&root, cx),
        "1\n1\n  let a = 1;\n2\n- let b = 2;\n2\n+ let b = 3;\n3\n3\n    println!(\"{a}\");"
    );
}

#[gpui_kit::test]
fn double_click_selects_a_word_and_triple_click_the_cell(cx: &mut TestAppContext) {
    let (root, cx) = open(cx);
    let p = at(3, 6., cx);
    cx.simulate_event(gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Left,
        position: p,
        modifiers: gpui_kit::Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_mouse_up(p, gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(copied(&root, cx), "println");
    cx.simulate_event(gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Left,
        position: p,
        modifiers: gpui_kit::Modifiers::none(),
        click_count: 3,
        first_mouse: false,
    });
    cx.simulate_mouse_up(p, gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(copied(&root, cx), "    println!(\"{a}\");");
}

#[gpui_kit::test]
fn disabled_scopes_do_not_start_a_selection_and_outside_presses_clear(cx: &mut TestAppContext) {
    let (root, cx) = open(cx);
    let (a, b) = (at(0, 2., cx), at(1, 4., cx));
    drag(a, b, cx);
    let state = root.read_with(cx, |r, _| r.state.clone()).expect("scope");
    assert!(state.read_with(cx, |s, _| s.has_selection()));
    cx.simulate_click(point(px(600.), px(300.)), gpui_kit::Modifiers::none());
    assert!(
        !state.read_with(cx, |s, _| s.has_selection()),
        "a press outside clears"
    );

    root.update(cx, |r, cx| {
        r.disabled = true;
        cx.notify();
    });
    cx.run_until_parked();
    drag(a, b, cx);
    assert!(
        !state.read_with(cx, |s, _| s.has_selection()),
        "`.diff-file.selecting`"
    );
}
