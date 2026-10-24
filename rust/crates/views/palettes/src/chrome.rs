//! What the palettes share: `.palette-bg` / `.palette` and its `.cmd-palette` and
//! `.open-in-palette` densities (`_overlays.scss`), and the keys they read (`lib/shortcut.ts`).

use gpui_kit::base::input::InputState;
use gpui_kit::{
    AnyElement, App, Div, ElementId, Entity, InteractiveElement, IntoElement, Keystroke,
    MouseButton, ParentElement, ScrollHandle, SharedString, Stateful,
    Styled, Window, div, prelude::FluentBuilder, px,
};
use recur_ui::fonts::Family;
use recur_ui::kbd::kbd;
use recur_ui::shadow::with_shadow;
use recur_ui::style::{
    PALETTE_SHADOW, card, footer_key, palette_footer, palette_input_row, palette_scrim_capped,
    text_field,
};
use recur_ui::text::{LineHeight, Text, TextStyled, span, text};
use recur_ui::widgets::scroll_area;
use recur_ui::{IconName, Theme, icon};

/// Which palette card: `.palette` (⌘K, ⌘⇧/), `.cmd-palette` (⌘⇧P, the language picker) or
/// `.cmd-palette.open-in-palette` (⌘O, ⌘R).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Palette,
    Command,
    OpenIn,
}

impl Kind {
    pub fn dense(self) -> bool {
        self != Kind::Palette
    }

    fn width(self) -> f32 {
        match self {
            Kind::Palette => 580.,
            Kind::Command => 480.,
            Kind::OpenIn => 420.,
        }
    }

    fn radius(self) -> f32 {
        if self.dense() { 10. } else { 13. }
    }

    /// `max-height` in vh (`.shortcuts-panel` raises it to 72).
    pub fn max_vh(self) -> f32 {
        if self.dense() { 56. } else { 64. }
    }
}

/// `.pal-in`: the leading icon, the query field and the trailing hint keycap.
pub fn head(
    kind: Kind,
    name: IconName,
    input: &Entity<InputState>,
    placeholder: &SharedString,
    hint: &str,
    window: &Window,
    cx: &App,
) -> Div {
    let t = Theme::of(cx).tokens;
    let (icon_px, font) = if kind.dense() { (14., 13.) } else { (16., 14.5) };
    palette_input_row(kind.dense(), cx)
        .child(icon(name).size(px(icon_px)).color(t.faint))
        // WebKit keeps an input's default 1px padding; only the border is reset.
        .child(div().flex_1().min_w_0().flex().p(px(1.)).child(text_field(
            input,
            placeholder,
            t.faint,
            Family::Ui,
            font,
            window,
            cx,
        )))
        .child(hint_key(hint, cx))
}

/// `.pal-in .esc`: the bordered hint (`esc`, `⌘O`, `⌘⇧P`, …).
pub fn hint_key(label: &str, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    div()
        .flex_none()
        .border(px(1.))
        .border_color(t.line)
        .rounded(px(4.))
        .py(px(2.))
        .px(px(6.))
        .child(kbd(label.to_string()).size(9.5).color(t.faint))
}

/// `.pal-list`: the padded scroller. Its children are the group labels and rows, so a row's
/// child index is what [`ScrollHandle::scroll_to_item`] reveals (`scrollIntoView({ block:
/// 'nearest' })`).
pub fn list(
    id: impl Into<ElementId>,
    kind: Kind,
    scroll: &ScrollHandle,
    children: Vec<AnyElement>,
) -> Div {
    let pad = if kind.dense() { 4. } else { 6. };
    div().flex_auto().min_h_0().flex().flex_col().child(
        scroll_area(id)
            .track(scroll)
            .scroller(move |d| d.p(px(pad)).flex().flex_col())
            .children(children),
    )
}

/// `.pal-foot`: `<Kbd class="k">` hints with their labels. GPUI clips rectangles, so the
/// footer rounds its own bottom corners inside the card.
pub fn footer(kind: Kind, hints: &[(&str, &str)], cx: &App) -> Div {
    let mut f = palette_footer(kind.dense(), cx).rounded_b(px(kind.radius() - 1.));
    if kind == Kind::OpenIn {
        f = f.flex_wrap().gap_x(px(14.)).gap_y(px(8.));
    }
    f.children(hints.iter().map(|(k, label)| {
        Text::from_spans(vec![footer_key(k.to_string(), cx), span(label.to_string())]).nowrap()
    }))
}

/// The card on its scrim, `kind.width()` wide (at most 92vw) and `max_vh` tall. A press on the
/// scrim outside the card calls `close`.
pub fn frame(
    kind: Kind,
    max_vh: f32,
    id: &'static str,
    children: Vec<AnyElement>,
    close: impl Fn(&mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let panel = card(t.pane, t.line_bright, kind.radius())
        .w_full()
        .overflow_hidden()
        .flex()
        .flex_col()
        .children(children);
    let width = kind
        .width()
        .min(f32::from(window.viewport_size().width) * 0.92);
    let panel = with_shadow(panel, &PALETTE_SHADOW, kind.radius())
        .id(id)
        .debug_selector(move || id.to_string())
        .w(px(width))
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    palette_scrim_capped(max_vh, window, panel)
        .id(SharedString::from(format!("{id}-bg")))
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| close(window, cx))
        .into_any_element()
}

/// `.pal-row .pl`: 13px and `flex: 1` in `.palette`, 12.5px at line-height 1.3 in
/// `.cmd-palette`. Labels wrap where the row is too narrow, as the span does.
pub fn label(id: impl Into<ElementId>, s: impl Into<SharedString>, kind: Kind, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    if kind.dense() {
        text(s)
            .id(id)
            .size(12.5)
            .line_height(LineHeight::Scale(1.3))
            .color(t.text)
            .into_any_element()
    } else {
        div()
            .flex_1()
            .min_w_0()
            .child(text(s).id(id).size(13.).color(t.text))
            .into_any_element()
    }
}

/// `.open-in-palette .pal-row .pk`: the 1–9 row digit, mono 11px at the keycap's weight and
/// tracking, at least 18px wide. After the pin it sits flush (`.open-in-pin + .pk`).
pub fn digit_key(n: usize, selected: bool, after_pin: bool, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    let (bg, border, fg) = if selected {
        (t.pane, t.line_bright, t.text)
    } else {
        (t.pane_2, t.line, t.muted)
    };
    let d = div();
    let d = if after_pin { d } else { d.ml_auto() };
    d.flex_none()
        .min_w(px(18.))
        .flex()
        .justify_center()
        .bg(bg)
        .border(px(1.))
        .border_color(border)
        .rounded(px(4.))
        .py(px(1.))
        .px(px(5.))
        .child(
            text(n.to_string())
                .mono()
                .size(11.)
                .weight(recur_ui::fonts::weight::MEDIUM)
                .tracking(0.02)
                .color(fg)
                .nowrap(),
        )
}

/// `.open-in-palette .pal-row .pic`: the 22px tile (transparent behind an app icon).
pub fn app_tile(selected: bool, os: bool, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    let d = div()
        .flex_none()
        .size(px(22.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_center();
    if os {
        d
    } else if selected {
        d.bg(recur_ui::rgba(242, 179, 61, 0.15))
    } else {
        d.bg(t.pane_2)
    }
}

/// `.open-in-pin`: the 22px Set as default button, shown on the selected row and on the
/// default (`.on`); amber while hovered or on. Its `title` is the tooltip.
pub fn pin(id: SharedString, on: bool, row_selected: bool, hovered: bool, cx: &App) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    let tip = if on { "Default" } else { "Set as default" };
    let visible = on || row_selected;
    let color = if on || hovered { t.amber } else { t.faint };
    let inner = div()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .when(visible, |d| d.child(icon(IconName::Pin).size(px(12.)).color(color)));
    div()
        .id(id.clone())
        .flex_none()
        .ml_auto()
        .cursor_pointer()
        .child(recur_ui::tooltip::titled(
            SharedString::from(format!("{id}-tip")),
            tip,
            inner,
        ))
}

/// No ⌘, Ctrl or ⌥ (a palette's own keys skip chords).
pub fn plain(k: &Keystroke) -> bool {
    !(k.modifiers.platform || k.modifiers.control || k.modifiers.alt)
}

/// `isCmdOrCtrl`.
pub fn cmd_or_ctrl(k: &Keystroke) -> bool {
    k.modifiers.secondary()
}

/// `paletteRowDigit(e)`: 1–9 without modifiers (Shift turns the key into a symbol).
pub fn row_digit(k: &Keystroke) -> Option<usize> {
    if !plain(k) || k.modifiers.shift {
        return None;
    }
    match k.key.as_str() {
        d @ ("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => d.parse().ok(),
        _ => None,
    }
}

/// `sel = Math.min(len - 1, sel + 1)` / `Math.max(0, sel - 1)`.
pub fn step(sel: usize, len: usize, down: bool) -> usize {
    if down {
        (sel + 1).min(len.saturating_sub(1))
    } else {
        sel.saturating_sub(1)
    }
}

/// JS `String.prototype.length` (UTF-16 units).
pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_skip_chords_and_shift() {
        let k = |s: &str| Keystroke::parse(s).unwrap();
        assert_eq!(row_digit(&k("1")), Some(1));
        assert_eq!(row_digit(&k("9")), Some(9));
        assert_eq!(row_digit(&k("0")), None);
        assert_eq!(row_digit(&k("cmd-1")), None);
        assert_eq!(row_digit(&k("shift-1")), None);
        assert_eq!(row_digit(&k("a")), None);
    }

    #[test]
    fn steps_clamp() {
        assert_eq!(step(0, 3, true), 1);
        assert_eq!(step(2, 3, true), 2);
        assert_eq!(step(0, 3, false), 0);
        assert_eq!(step(0, 0, true), 0);
        assert_eq!(js_len("é😀"), 3);
    }
}
