//! The Observatory's shared chrome: pane toggles
//! with badges, `.seg` text chips, side heads, status bars, `.linkish` buttons and empty
//! states.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    Stateful, Styled, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::style::faded;
use recur_ui::text::{LineHeight, Text, TextStyled, text};
use recur_ui::{Color, IconName, Theme, Tokens, icon};
use recur_view_review::style::hsla;

pub use recur_view_widgets::chrome::pane_toggle;

pub use recur_ui::style::CHROME_H;
/// `--chrome-foot-h`.
pub const FOOT_H: f32 = 26.;
/// `--win-corner-pad`.
pub const WIN_CORNER_PAD: f32 = 24.;
/// The media query that stacks the Observatory into one column.
pub const NARROW_MAX_W: f32 = 960.;

pub fn tokens(cx: &App) -> Tokens {
    Theme::of(cx).tokens
}

/// `.obs-pane-badge`: amber 14px pill, mono 9px bold, `9+` past nine.
pub fn pane_badge(n: usize, cx: &App) -> AnyElement {
    let t = tokens(cx);
    let label = if n > 9 {
        "9+".to_string()
    } else {
        n.to_string()
    };
    div()
        .absolute()
        .top(px(1.))
        .right(px(1.))
        .min_w(px(14.))
        .h(px(14.))
        .px(px(3.))
        .rounded(px(7.))
        .bg(hsla(t.amber))
        .flex()
        .justify_center()
        .child(
            text(label)
                .mono()
                .size(9.)
                .weight(700.)
                .line_height(LineHeight::Px(14.))
                .color(t.on_amber)
                .nowrap(),
        )
        .into_any_element()
}

/// `.seg` container: `--pane` fill, 1px line border, radius 6, padding 2.
pub fn seg(cx: &App) -> Div {
    let t = tokens(cx);
    recur_ui::style::card(t.pane, t.line, 6.)
        .flex()
        .flex_none()
        .p(px(2.))
}

/// One text `.seg button`: radius 4, muted 11.5px weight 500 (sizes vary by host);
/// `.on` fills `--pane-2` in the text color.
pub fn seg_text_btn(
    id: impl Into<ElementId>,
    on: bool,
    disabled: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let (mut fg, mut bg) = if on {
        (t.text, t.pane_2)
    } else {
        (t.muted, recur_ui::color::TRANSPARENT)
    };
    if let Some(back) = disabled {
        fg = faded(fg, bg, 0.45, back);
        bg = faded(bg, bg, 0.45, back);
    }
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(4.))
        .bg(hsla(bg))
        .text_color(hsla(fg))
        .when(disabled.is_none(), |d| d.cursor_pointer())
}

/// `.obs-side-head`: the 40px pane title row (padding 0 8 0 12, gap 8, `--pane`).
pub fn side_head(tabbed: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let d = div()
        .flex()
        .flex_none()
        .h(px(CHROME_H))
        .min_w_0()
        .bg(hsla(t.pane));
    if tabbed {
        d.relative()
            .items_stretch()
            .pr(px(4.))
            .child(recur_ui::widgets::tab_bar_rule(cx))
    } else {
        d.items_center()
            .gap(px(8.))
            .pl(px(12.))
            .pr(px(8.))
            .border_b_1()
            .border_color(hsla(t.line))
    }
}

/// `.obs-side-title`: mono 11px 600, tracking .02em.
pub fn side_title(label: impl Into<SharedString>, cx: &App) -> Text {
    let t = tokens(cx);
    text(label.into())
        .mono()
        .size(11.)
        .weight(600.)
        .tracking(0.02)
        .color(t.text)
        .nowrap()
}

/// `.obs-statusbar`: the 26px footer, mono 10.5px, line-height 1, faint.
pub fn statusbar(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(FOOT_H))
        .px(px(10.))
        .border_t_1()
        .border_color(hsla(t.line))
        .bg(hsla(t.pane))
        .text_color(hsla(t.faint))
}

/// Status bar text (mono 10.5px, line-height 1). It wraps when the bar is too narrow, as
/// `.obs-statusbar` items shrink to their min-content width.
pub fn foot_text(s: impl Into<SharedString>) -> Text {
    text(s.into())
        .family(Family::Mono)
        .size(10.5)
        .line_height(LineHeight::Scale(1.))
        .nowrap()
}

/// A button label in the status bar: wrapped lines center (`button { text-align: center }`).
pub fn link_text(s: impl Into<SharedString>) -> Text {
    foot_text(s).align(recur_ui::text::Align::Center)
}

/// `.obs-statusbar` items shrink to their min-content width once the bar overflows. Nowrap
/// text keeps each item's exact width until the bar's `.sp` spacer is gone; then items wrap
/// until the bar is wider than where that began.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatusWrap(Option<f32>);

impl StatusWrap {
    /// Follows the bar's and its spacer's last widths; true when the mode changed.
    pub fn update(&mut self, bar: f32, spacer: f32) -> bool {
        match self.0 {
            None if bar > 0. && spacer < 0.25 => {
                self.0 = Some(bar);
                true
            }
            Some(w0) if bar > w0 + 0.5 => {
                self.0 = None;
                true
            }
            _ => false,
        }
    }

    pub fn on(&self) -> bool {
        self.0.is_some()
    }

    /// An item's text: wrapping while the bar overflows.
    pub fn text(&self, t: Text) -> Text {
        if self.on() {
            t.white_space(recur_ui::text::WhiteSpace::Normal)
        } else {
            t
        }
    }
}

/// `.obs-statusbar .sep`: a dot at half opacity.
pub fn foot_sep(cx: &App) -> AnyElement {
    let t = tokens(cx);
    foot_text("·").color(t.faint.fade(0.5)).into_any_element()
}

/// `.linkish`: borderless text button, muted until hover (or `color` with a text hover).
pub fn linkish(id: impl Into<ElementId>, color: Option<Color>, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    let fg = color.unwrap_or(t.muted);
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(hsla(fg))
        .hover(move |s| s.text_color(hsla(t.text)))
}

/// `.obs-empty`: 16px 12px faint 12px (`.pad`: 48px 24px centered).
pub fn obs_empty(s: impl Into<SharedString>, pad: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let label = text(s.into()).size(12.).color(t.faint);
    if pad {
        div()
            .px(px(24.))
            .py(px(48.))
            .flex()
            .justify_center()
            .child(label.align(recur_ui::text::Align::Center))
    } else {
        div().px(px(12.)).py(px(16.)).child(label)
    }
}

/// `.obs-icon-btn` with an optional amber `.on` (the crumb bar's find toggle).
pub fn icon_btn(
    id: impl Into<ElementId>,
    name: IconName,
    on: bool,
    disabled: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    match disabled {
        Some(back) => recur_view_review::style::icon_btn_disabled(id, name, 26., 15., back, cx),
        None if on => div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(5.))
            .cursor_pointer()
            .bg(hsla(t.amber_d))
            .text_color(hsla(t.amber))
            .child(icon(name).size(px(15.))),
        None => recur_view_review::style::icon_btn(id, name, 26., 15., None, cx),
    }
}

/// Git status colors of the explorer (`--git-*`).
pub fn git_color(st: &str, t: &Tokens) -> Option<Color> {
    match st {
        "m" => Some(t.git_m),
        "a" | "u" => Some(t.git_a),
        "d" => Some(t.git_d),
        _ => None,
    }
}

/// WebKit's native `<input type=checkbox>` (macOS, light control): a 12px rounded box with a
/// 3px 3px 3px 4px margin; checked fills with the system accent and a white check.
pub fn native_checkbox(on: bool) -> AnyElement {
    const MARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="#ffffff" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 6.1 5 8.6 9.7 2.7"/></svg>"##;
    let accent = recur_ui::hex(0x007aff);
    let edge = recur_ui::rgba(0, 0, 0, 0.25);
    div()
        .mt(px(3.))
        .mb(px(3.))
        .mr(px(3.))
        .ml(px(4.))
        .size(px(12.))
        .rounded(px(3.))
        .border(px(0.5))
        .border_color(hsla(if on { accent } else { edge }))
        .bg(hsla(if on { accent } else { recur_ui::hex(0xffffff) }))
        .when(on, |d| {
            d.child(recur_ui::svg_markup(MARK.to_string()).size(px(12.)))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::StatusWrap;

    #[test]
    fn status_items_wrap_once_the_spacer_is_gone_until_the_bar_widens() {
        let mut w = StatusWrap::default();
        assert!(!w.update(600., 40.), "room to spare");
        assert!(w.update(420., 0.), "the spacer is gone: wrap");
        assert!(w.on());
        assert!(!w.update(420.4, 0.), "a sub-pixel change keeps wrapping");
        assert!(!w.update(380., 0.));
        assert!(w.update(421., 0.), "wider than where wrapping began");
        assert!(!w.on());
        assert!(!w.update(0., 0.), "an unmeasured bar never wraps");
    }
}
