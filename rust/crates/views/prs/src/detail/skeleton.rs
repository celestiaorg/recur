//! The pending-state skeletons (`convSkel`, `commitsSkel`, `filesSkel`, `checksSkel`).

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Div, InteractiveElement, IntoElement, ParentElement, Styled, Window, div, px,
    relative,
};
use recur_ui::Theme;

use crate::style::hsla;

fn skel(window: &mut Window, cx: &App) -> Div {
    recur_ui::style::skeleton(window, cx)
}

/// `width:{w}%`.
fn pct(w: f32) -> gpui_kit::DefiniteLength {
    relative(w / 100.)
}

pub(crate) fn conv_skel(window: &mut Window, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let ava = |window: &mut Window| skel(window, cx).flex_none().size(px(28.)).rounded(px(6.));
    let desc = recur_ui::style::card(t.pane, t.line, 9.)
        .flex()
        .gap(px(12.))
        .py(px(14.))
        .px(px(15.))
        .mb(px(8.))
        .child(ava(window))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(skel(window, cx).w(px(112.)).h(px(11.)).mb(px(12.)))
                .child(skel(window, cx).w(pct(94.)).h(px(10.)).mb(px(7.)))
                .child(skel(window, cx).w(pct(82.)).h(px(10.)).mb(px(7.)))
                .child(skel(window, cx).w(pct(61.)).h(px(10.))),
        );
    let mut thread = div().pt(px(6.)).flex().flex_col();
    for (i, w) in [88., 64., 76.].into_iter().enumerate() {
        let last = i == 2;
        let head = if i == 1 { 88. } else { 108. };
        thread = thread.child(
            div()
                .flex()
                .gap(px(12.))
                .py(px(14.))
                .when(!last, |d| d.border_b_1().border_color(hsla(t.line_soft)))
                .child(ava(window))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(skel(window, cx).w(px(head)).h(px(11.)).mb(px(9.)))
                        .child(skel(window, cx).w(pct(w)).h(px(10.)).mb(px(6.)))
                        .child(skel(window, cx).w(pct(f32::max(38., w - 22.))).h(px(10.))),
                ),
        );
    }
    div()
        .debug_selector(|| "pr-skel".into())
        .child(desc)
        .child(thread)
        .into_any_element()
}

pub(crate) fn commits_skel(window: &mut Window, cx: &App) -> AnyElement {
    let mut col = div()
        .debug_selector(|| "pr-skel".into())
        .flex()
        .flex_col()
        .gap(px(2.))
        .py(px(8.))
        .px(px(4.));
    for w in [72., 58., 81., 49.] {
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(8.))
                .px(px(2.))
                .child(skel(window, cx).flex_none().size(px(14.)).rounded(px(4.)))
                .child(skel(window, cx).w(pct(w)).h(px(12.)))
                .child(
                    skel(window, cx)
                        .flex_none()
                        .ml_auto()
                        .w(px(51.2))
                        .h(px(10.)),
                ),
        );
    }
    col.into_any_element()
}

pub(crate) fn files_skel(window: &mut Window, cx: &App) -> AnyElement {
    let mut col = div()
        .debug_selector(|| "pr-skel".into())
        .flex()
        .flex_col()
        .gap(px(14.))
        .py(px(16.))
        .px(px(18.));
    for w in [86., 64., 78., 51., 71.] {
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(8.))
                .px(px(2.))
                .child(skel(window, cx).flex_none().size(px(14.)).rounded(px(3.)))
                .child(skel(window, cx).w(pct(w)).h(px(12.)))
                .child(
                    skel(window, cx)
                        .flex_none()
                        .ml_auto()
                        .w(px(41.6))
                        .h(px(10.)),
                ),
        );
    }
    col.into_any_element()
}

pub(crate) fn checks_skel(window: &mut Window, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let mut card = recur_ui::style::card(t.pane, t.line, 9.)
        .debug_selector(|| "pr-skel".into())
        .mb(px(14.))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.))
                .py(px(11.))
                .px(px(15.))
                .border_b_1()
                .border_color(hsla(t.line))
                .child(skel(window, cx).flex_none().size(px(14.)).rounded(px(99.)))
                .child(skel(window, cx).w(px(144.)).h(px(12.))),
        );
    for w in [54., 70., 46.] {
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .pt(px(8.))
                .px(px(12.))
                .pb(px(10.))
                .child(skel(window, cx).flex_none().size(px(12.)).rounded(px(99.)))
                .child(skel(window, cx).w(pct(w)).h(px(11.))),
        );
    }
    card.into_any_element()
}
