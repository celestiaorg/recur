//! Generic styles from `_base.scss`, `_forms.scss`, `_overlays.scss` (and the `.tbtn` / `.rbtn`
//! buttons they build on), as GPUI style builders. Each function documents the class it
//! ports; hover, active and keyboard-focus states are attached the same way the CSS cascades
//! them. View-specific rules stay with their views.

use crate::color::{Color, rgba};
use crate::fonts::{MONO, UI, weight};
use crate::text::{InlineBox, LineHeight, Span, Text, TextStyled, span, text};
use crate::theme::{Theme, Tokens};
use gpui_kit::{
    App, BoxShadow, Div, ElementId, Fill, FocusHandle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, Stateful, Styled, Window, div, point, prelude::FluentBuilder, px,
    relative,
};

impl From<Color> for Fill {
    fn from(c: Color) -> Fill {
        Hsla::from(c).into()
    }
}

fn tokens(cx: &App) -> Tokens {
    Theme::of(cx).tokens
}

/// CSS `opacity` fades an element and its content as one layer. GPUI applies opacity to each
/// primitive, which lets a label or border show through its own fill, so faded controls
/// flatten their colors instead: each color as composited inside the element (over `fill`),
/// then mixed with the `backdrop` the control sits on. Exact for opaque backdrops, glyph
/// anti-aliasing included.
pub fn faded(color: Color, fill: Color, opacity: f32, backdrop: Color) -> Color {
    color.over(fill.over(backdrop)).mix(opacity, backdrop)
}

/// A border the same color as its opaque fill paints nothing visible; drop it so GPUI's
/// per-primitive `opacity` (CSS fades the element as one layer) does not double it up.
fn border_over(border: Color, bg: Color) -> Hsla {
    if border == bg {
        Hsla::transparent_black()
    } else {
        border.into()
    }
}

/// CSS `linear-gradient(<angle>, from, to)` for a box of `size`, as a GPUI background. GPUI
/// stretches a diagonal direction by the box's aspect ratio and measures along its longer side;
/// this picks the GPUI angle whose stretched direction is the CSS one and rescales the stops from
/// CSS's gradient line (`|w sin a| + |h cos a|`) to GPUI's. Exact for any angle.
pub fn css_linear_gradient(
    angle: f32,
    from: (Color, f32),
    to: (Color, f32),
    size: gpui_kit::Size<Pixels>,
) -> gpui_kit::Background {
    let (w, h) = (
        f32::from(size.width).max(1e-3),
        f32::from(size.height).max(1e-3),
    );
    let a = angle.to_radians();
    let (sx, sy) = (a.sin(), -a.cos());
    let (gx, gy) = if w > h {
        (sx, sy * w / h)
    } else {
        (sx * h / w, sy)
    };
    let gpui_angle = (gy.atan2(gx).to_degrees() + 90.).rem_euclid(360.);
    let along = if sx.abs() > sy.abs() { w } else { h };
    let line = (w * sx).abs() + (h * sy).abs();
    let stop = |s: f32| 0.5 + (s - 0.5) * line / along;
    gpui_kit::linear_gradient(
        gpui_angle,
        gpui_kit::linear_color_stop(from.0, stop(from.1)),
        gpui_kit::linear_color_stop(to.0, stop(to.1)),
    )
}

/// A [`css_linear_gradient`] background for its relatively positioned parent with a `border` of
/// `border_color` and `radius`. See [`inset_background`] for its extent.
pub fn css_gradient(
    angle: f32,
    from: (Color, f32),
    to: (Color, f32),
    radius: f32,
    border: f32,
    border_color: Color,
) -> impl IntoElement {
    let opaque = border_color.a >= 1.;
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let gradient_box = bounds.dilate(px(border));
            let (bounds, radius) = background_box(bounds, radius, border, opaque, window);
            window.paint_quad(gpui_kit::quad(
                bounds,
                px(radius),
                css_linear_gradient(angle, from, to, gradient_box.size),
                px(0.),
                Hsla::transparent_black(),
                gpui_kit::BorderStyle::Solid,
            ));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The background of a relatively positioned parent with a `border` of `border_color` and
/// `radius`, in place of `.bg()`. GPUI paints a div's background and border as two anti-aliased
/// quads, so the background bleeds into the border's outer edge; under an opaque border WebKit
/// shrinks the background one device pixel inside the border box instead
/// (`BackgroundBleedShrinkBackground`). Put it first.
pub fn inset_background(
    color: Color,
    radius: f32,
    border: f32,
    border_color: Color,
) -> impl IntoElement {
    let opaque = border_color.a >= 1.;
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (bounds, radius) = background_box(bounds, radius, border, opaque, window);
            window.paint_quad(gpui_kit::fill(bounds, Hsla::from(color)).corner_radii(px(radius)));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The border box from a canvas filling the padding box, shrunk by a device pixel under an
/// opaque border.
fn background_box(
    padding_box: gpui_kit::Bounds<Pixels>,
    radius: f32,
    border: f32,
    opaque: bool,
    window: &Window,
) -> (gpui_kit::Bounds<Pixels>, f32) {
    let shrink = if border > 0. && opaque {
        1. / window.scale_factor()
    } else {
        0.
    };
    (
        padding_box.dilate(px(border - shrink)),
        (radius - shrink).max(0.),
    )
}

/// A CSS `box-shadow` layer. CSS blur is twice the Gaussian sigma GPUI takes.
pub fn css_shadow(x: f32, y: f32, blur: f32, spread: f32, color: Color) -> BoxShadow {
    BoxShadow {
        color: color.into(),
        offset: point(px(x), px(y)),
        blur_radius: px(blur / 2.),
        spread_radius: px(spread),
        inset: false,
    }
}

/// `body`: `--bg` background, `--text` color, Inter 13px, line-height 1.45.
pub fn body(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .bg(t.bg)
        .text_color(t.text)
        .font_family(UI)
        .text_size(px(13.))
        .line_height(relative(1.45))
}

/// The root every window renders its content in. WebKit snaps a box edge that lands exactly on
/// a half device pixel down and to the right; GPUI rounds halves toward zero. Shifting the whole
/// tree by 1/4096 px (through `aspect_ratio` spacers, since authored lengths round to whole
/// device pixels) makes those ties round as WebKit does and moves nothing else.
pub fn window_root(content: impl IntoElement) -> Div {
    const NUDGE: f32 = 4096.;
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().flex_none().w(px(1.)).aspect_ratio(NUDGE))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().flex_none().h(px(1.)).aspect_ratio(1. / NUDGE))
                .child(div().flex_1().min_w_0().size_full().child(content)),
        )
}

/// Button label text: `<button>` resets `font` (13.333px, line-height normal) and the class
/// then sets size and weight.
pub fn button_label(label: impl Into<gpui_kit::SharedString>, size: f32, weight: f32) -> Text {
    text(label)
        .size(size)
        .weight(weight)
        .line_height(LineHeight::Normal)
        .nowrap()
}

/// Whether `handle` should paint its `:focus-visible` treatment: focused, and reached by
/// keyboard (GPUI's input-modality tracking, the same rule browsers use).
pub fn is_focus_visible(handle: &FocusHandle, window: &Window) -> bool {
    handle.is_focused(window) && window.last_input_was_keyboard()
}

/// `:focus-visible { outline: 2px solid color-mix(amber 70%, transparent); outline-offset:
/// 1px; border-radius: 4px }`. Add as the last child of a `relative()` element while
/// [`is_focus_visible`] holds.
pub fn focus_ring(cx: &App) -> Div {
    outline(2., 1., 4., tokens(cx).amber.fade(0.7))
}

/// A CSS outline of `width` at `offset` around a box with corner `radius`, as an absolutely
/// positioned overlay.
pub fn outline(width: f32, offset: f32, radius: f32, color: Color) -> Div {
    let inset = -(offset + width);
    div()
        .absolute()
        .top(px(inset))
        .left(px(inset))
        .right(px(inset))
        .bottom(px(inset))
        .border(px(width))
        .border_color(Hsla::from(color))
        .rounded(px(radius + offset + width))
}

/// `.tbtn` variants (`_main.scss`, `_observatory.scss`, `_overlays.scss`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tbtn {
    #[default]
    Default,
    /// `.tbtn.amber`
    Amber,
    /// `.tbtn.solid`
    Solid,
    /// `.tbtn.on` (the `_observatory.scss` rule, which cascades last)
    On,
    /// `.tbtn.danger`
    Danger,
    /// `.tbtn.danger.armed`
    Armed,
}

/// Metrics of a `.tbtn`: the base class, or `.tb-right .tbtn` in the titlebar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TbtnSize {
    pub height: f32,
    pub pad_x: f32,
    pub font_size: f32,
}

impl TbtnSize {
    pub const BASE: TbtnSize = TbtnSize {
        height: 28.,
        pad_x: 11.,
        font_size: 12.,
    };
    /// `.tb-right .tbtn`.
    pub const TITLEBAR: TbtnSize = TbtnSize {
        height: 26.,
        pad_x: 9.,
        font_size: 11.5,
    };
}

/// `.tbtn`: inline-flex, gap 6, radius 6, 1px border, weight 500. Children: icons at 13px and
/// a [`button_label`]. `disabled_over` disables it; titlebar buttons also fade over that
/// backdrop.
pub fn tbtn(
    id: impl Into<ElementId>,
    kind: Tbtn,
    size: TbtnSize,
    disabled_over: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let (mut bg, mut border, mut fg) = match kind {
        Tbtn::Default => (t.pane, t.line, t.text),
        Tbtn::Amber => (t.amber, t.amber, t.on_amber),
        Tbtn::Solid => (t.text, t.text, t.bg),
        Tbtn::On => (t.pane, t.amber, t.amber),
        Tbtn::Danger => (t.pane, rgba(235, 122, 92, 0.32), t.coral),
        Tbtn::Armed => (t.coral, t.coral, crate::color::hex(0x0a0c10)),
    };
    // `.tb-right .tbtn:disabled { opacity: .45; cursor: not-allowed }`; elsewhere a disabled
    // `.tbtn` looks enabled.
    if let Some(back) = disabled_over
        && size == TbtnSize::TITLEBAR
    {
        (bg, border, fg) = (
            faded(bg, bg, 0.45, back),
            faded(border, bg, 0.45, back),
            faded(fg, bg, 0.45, back),
        );
    }
    let disabled = disabled_over.is_some();
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(size.height))
        .px(px(size.pad_x))
        .rounded(px(6.))
        .border(px(1.))
        .border_color(border_over(border, bg))
        .bg(bg)
        .text_color(fg)
        .text_size(px(size.font_size))
        .font_weight(gpui_kit::FontWeight(weight::MEDIUM));
    if disabled {
        b = b.cursor_not_allowed();
    } else {
        b = b.cursor_pointer();
        match kind {
            // `.tbtn:hover` only changes the border; danger/armed/amber keep theirs (later or
            // more specific rules win).
            Tbtn::Default => b = b.hover(|s| s.border_color(Hsla::from(t.line_bright))),
            // `.tbtn.solid:hover { filter: brightness(.92) }` dims fill, border and label.
            Tbtn::Solid => {
                let (dim, ink) = (t.text.brightness(0.92), t.bg.brightness(0.92));
                b = b.hover(move |s| s.bg(dim).border_color(Hsla::from(dim)).text_color(ink))
            }
            _ => {}
        }
    }
    b
}

/// `.rbtn` variants (`_prs.scss`, `_observatory.scss`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Rbtn {
    #[default]
    Default,
    /// `.rbtn.merge`
    Merge,
    /// `.rbtn.primary`
    Primary,
    /// `.rbtn.armed`
    Armed,
}

/// `.rbtn`: height 30, padding 0 13, radius 7, 12.5px weight 550, gap 7.
pub fn rbtn(
    id: impl Into<ElementId>,
    kind: Rbtn,
    disabled_over: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let (mut bg, mut border, mut fg) = match kind {
        Rbtn::Default => (t.pane, t.line, t.text),
        Rbtn::Merge => (t.emerald, t.emerald, crate::color::hex(0x0a0c10)),
        Rbtn::Primary => (t.text, t.text, t.bg),
        Rbtn::Armed => (t.coral, t.coral, crate::color::hex(0x0a0c10)),
    };
    // `.rbtn.primary:disabled { opacity: .35 }`; other rbtns fade like titlebar buttons.
    if let Some(back) = disabled_over {
        let o = if kind == Rbtn::Primary { 0.35 } else { 0.45 };
        (bg, border, fg) = (
            faded(bg, bg, o, back),
            faded(border, bg, o, back),
            faded(fg, bg, o, back),
        );
    }
    let disabled = disabled_over.is_some();
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(7.))
        .h(px(30.))
        .px(px(13.))
        .rounded(px(7.))
        .border(px(1.))
        .border_color(border_over(border, bg))
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.5))
        .font_weight(gpui_kit::FontWeight(weight::MEDIUM_PLUS));
    if !disabled {
        b = b.cursor_pointer();
        if kind == Rbtn::Default {
            b = b.hover(|s| s.border_color(Hsla::from(t.line_bright)));
        }
    }
    b
}

/// `.nt-go`: the solid primary action (Create, Approve). Text-colored fill, 32px, radius 8,
/// pushed to the end of its row (`margin-left: auto`).
pub fn primary_button(
    id: impl Into<ElementId>,
    disabled_over: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    // `.nt-go:disabled { opacity: .35 }`.
    let (bg, fg) = match disabled_over {
        Some(back) => (
            faded(t.text, t.text, 0.35, back),
            faded(t.bg, t.text, 0.35, back),
        ),
        None => (t.text, t.bg),
    };
    let mut b = div()
        .id(id)
        .ml_auto()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(7.))
        .h(px(32.))
        .px(px(14.))
        .rounded(px(8.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(13.))
        .font_weight(gpui_kit::FontWeight(weight::SEMIBOLD));
    if disabled_over.is_none() {
        // `filter: brightness(1.05)` brightens the fill and the label.
        let (lit, ink) = (t.text.brightness(1.05), t.bg.brightness(1.05));
        b = b.cursor_pointer().hover(move |s| s.bg(lit).text_color(ink));
    }
    b
}

/// `.nt-link`: a borderless 30px text action, amber when `on`.
pub fn link_button(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(8.))
        .rounded(px(6.))
        .text_color(if on { t.amber } else { t.faint })
        .text_size(px(12.))
        .cursor_pointer()
        .hover(move |s| {
            s.bg(t.pane_2)
                .text_color(Hsla::from(if on { t.amber } else { t.text }))
        })
}

/// `.lnk`: mono 11px amber text link (underline on hover is the label's job).
pub fn lnk(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Text {
    text(label)
        .mono()
        .size(11.)
        .color(tokens(cx).amber)
        .nowrap()
}

/// `.tb-icon-btn`: 26px-high square-ish titlebar icon button, faint until hover or `on`.
pub fn titlebar_icon_button(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(1.))
        .h(px(26.))
        .px(px(6.))
        .rounded(px(6.))
        .text_color(t.faint)
        .cursor_pointer()
        .hover(move |s| s.bg(t.pane_2).text_color(Hsla::from(t.text)));
    if on {
        b = b.bg(t.pane_2).text_color(t.text);
    }
    b
}

/// `.tb-rail`: the 28px sidebar toggle.
pub fn rail_toggle(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .ml(px(2.))
        .rounded(px(6.))
        .text_color(if on { t.text } else { t.faint })
        .cursor_pointer()
        .hover(move |s| s.bg(t.pane_2).text_color(Hsla::from(t.text)))
}

/// `.tog` (36×20 switch): knob 14px at left 2, amber track and dark knob at left 18 when on.
pub fn toggle(id: impl Into<ElementId>, on: bool, disabled: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    let (track, border, knob, left) = if on {
        (t.amber, t.amber, crate::color::hex(0x0a0c10), 18.)
    } else {
        (t.pane_2, t.line, t.muted, 2.)
    };
    let mut d = div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(36.))
        .h(px(20.))
        .rounded(px(12.))
        .bg(track)
        .border(px(1.))
        .border_color(border_over(border, track))
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(left))
                .size(px(14.))
                .rounded_full()
                .bg(knob),
        );
    if !disabled {
        d = d.cursor_pointer();
    }
    d
}

/// `.panel`.
pub fn panel(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .bg(t.pane)
        .border(px(1.))
        .border_color(Hsla::from(t.line))
        .rounded(px(8.))
}

/// `.nt-bg` / `.palette-bg`: the modal scrim, `rgba(5, 7, 10, .55)` with a 2px backdrop blur.
/// GPUI has no backdrop filter; see `ui-parity.md`.
pub fn modal_scrim() -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(rgba(5, 7, 10, 0.55))
        .flex()
        .justify_center()
        .items_start()
}

/// `.nt-modal`: 640px card, `--line-bright` border, radius 16, deep shadow.
pub fn modal_panel(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .w(px(640.))
        .max_w_full()
        .bg(t.pane)
        .border(px(1.))
        .border_color(Hsla::from(t.line_bright))
        .rounded(px(16.))
        .shadow(vec![css_shadow(0., 32., 90., -20., rgba(0, 0, 0, 0.8))])
        .flex()
        .flex_col()
}

/// `.palette`: 580px command palette card, radius 13.
pub fn palette_panel(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .w(px(580.))
        .max_w(relative(0.92))
        .bg(t.pane)
        .border(px(1.))
        .border_color(Hsla::from(t.line_bright))
        .rounded(px(13.))
        .shadow(vec![css_shadow(0., 28., 80., -18., rgba(0, 0, 0, 0.75))])
        .overflow_hidden()
        .flex()
        .flex_col()
}

/// `.cmd-palette`: the denser 480px Run Command card, radius 10.
pub fn command_palette_panel(cx: &App) -> Div {
    palette_panel(cx).w(px(480.)).rounded(px(10.))
}

/// `.pal-in`: the palette's input row (14px 16px, gap 11, bottom border).
pub fn palette_input_row(dense: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (py, pxx, gap) = if dense {
        (10., 12., 9.)
    } else {
        (14., 16., 11.)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(gap))
        .py(px(py))
        .px(px(pxx))
        .border_b(px(1.))
        .border_color(Hsla::from(t.line))
}

/// `.pal-g`: mono 9px uppercase group label with .12em tracking.
pub fn palette_group(label: impl Into<gpui_kit::SharedString>, dense: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (pad, size, track) = if dense {
        ((6., 8., 2.), 8.5, 0.14)
    } else {
        ((10., 10., 5.), 9., 0.12)
    };
    div().pt(px(pad.0)).px(px(pad.1)).pb(px(pad.2)).child(
        text(label)
            .mono()
            .size(size)
            .tracking(track)
            .uppercase()
            .color(t.faint)
            .nowrap(),
    )
}

/// `.pal-row`: palette row, `--pane-2` when selected.
pub fn palette_row(
    id: impl Into<ElementId>,
    selected: bool,
    dense: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let mut d = div().id(id).flex().items_center().cursor_pointer();
    d = if dense {
        d.py(px(3.)).px(px(8.)).rounded(px(5.)).min_h(px(24.))
    } else {
        d.gap(px(11.)).py(px(8.)).px(px(10.)).rounded(px(7.))
    };
    if selected {
        d = d.bg(t.pane_2);
    }
    d
}

/// `.pal-row .pk` / `.sc-row .pk`: a keycap chip on a palette row.
pub fn palette_key(label: impl Into<gpui_kit::SharedString>, selected: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (bg, border, fg) = if selected {
        (t.pane, t.line_bright, t.text)
    } else {
        (t.pane_2, t.line, t.muted)
    };
    div()
        .ml_auto()
        .flex_none()
        .bg(bg)
        .border(px(1.))
        .border_color(Hsla::from(border))
        .rounded(px(4.))
        .py(px(2.))
        .px(px(6.))
        .child(crate::kbd::kbd(label).size(12.).color(fg))
}

/// `.pal-empty`.
pub fn palette_empty(label: impl Into<gpui_kit::SharedString>, dense: bool, cx: &App) -> Div {
    let t = tokens(cx);
    div().p(px(if dense { 22. } else { 34. })).child(
        text(label)
            .mono()
            .size(if dense { 11. } else { 12. })
            .color(t.faint)
            .align(crate::text::Align::Center),
    )
}

/// `.pal-foot`: the hint footer on `--rail`, mono 9.5px.
pub fn palette_footer(dense: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (py, pxx, gap) = if dense {
        (6., 12., 14.)
    } else {
        (8., 14., 16.)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(gap))
        .py(px(py))
        .px(px(pxx))
        .border_t(px(1.))
        .border_color(Hsla::from(t.line))
        .bg(t.rail)
        .font_family(MONO)
        .text_size(px(9.5))
        .text_color(t.faint)
}

/// `.pal-foot .k`: an inline keycap span inside the palette footer (bordered inline box,
/// 5px right margin), followed in the same text by its label.
pub fn footer_key(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Span {
    let t = tokens(cx);
    span(label)
        .system()
        .weight(weight::MEDIUM)
        .tracking(0.02)
        .boxed(InlineBox {
            margin_right: 5.,
            border: 1.,
            border_color: t.line.into(),
            pad_top: 1.,
            pad_right: 5.,
            pad_bottom: 1.,
            pad_left: 5.,
            background: Some(t.pane_2.into()),
            radius: 3.,
            ..Default::default()
        })
}

/// `.pal-in .esc` / `.nt-x .esc`: the bordered `esc` hint.
pub fn esc_key(hovered: bool, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .flex_none()
        .border(px(1.))
        .border_color(Hsla::from(if hovered { t.line_bright } else { t.line }))
        .rounded(px(4.))
        .py(px(2.))
        .px(px(6.))
        .child(
            crate::kbd::kbd("esc")
                .size(9.5)
                .color(if hovered { t.muted } else { t.faint }),
        )
}

/// `.nt-tip-pop`: the hover help popover, `--pane-2` with `--line-bright`, radius 9.
pub fn popover(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .w(px(280.))
        .py(px(11.))
        .px(px(12.))
        .bg(t.pane_2)
        .border(px(1.))
        .border_color(Hsla::from(t.line_bright))
        .rounded(px(9.))
        .shadow(vec![css_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))])
        .text_color(t.muted)
        .text_size(px(12.))
        .line_height(relative(1.45))
}

/// `.emoji-suggest` / `.user-mention`: the floating suggestion list.
pub fn suggest_panel(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .p(px(4.))
        .bg(t.pane)
        .border(px(1.))
        .border_color(Hsla::from(t.line_bright))
        .rounded(px(10.))
        .shadow(vec![css_shadow(0., 14., 36., -12., rgba(0, 0, 0, 0.7))])
}

/// `.sel-menu`: the dropdown menu shell shared by Select and IconPicker.
pub fn menu_panel(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(t.pane_2)
        .border(px(1.))
        .border_color(Hsla::from(t.line_bright))
        .rounded(px(8.))
        .p(px(4.))
        .shadow(vec![css_shadow(0., 10., 28., 0., rgba(0, 0, 0, 0.45))])
}

/// `.prog` / `.prog-fill`: a 4px progress bar.
pub fn progress(fraction: f32, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .mt(px(10.))
        .h(px(4.))
        .rounded(px(2.))
        .bg(t.line)
        .overflow_hidden()
        // `overflow: hidden` on the 2px-round track clips the fill's ends; GPUI masks are
        // rectangular, so the fill carries the track's radius itself.
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0., 1.)))
                .rounded_l(px(2.))
                .when(fraction >= 1., |d| d.rounded_r(px(2.)))
                .bg(t.amber),
        )
}

/// `.hint`: centered mono footnote.
pub fn hint(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Div {
    div().pt(px(22.)).pb(px(30.)).child(
        text(label)
            .mono()
            .size(10.5)
            .color(tokens(cx).faint)
            .align(crate::text::Align::Center),
    )
}

/// Count chip (`.nav .ct`, `.tproj-h .ct`, …): coral with dark ink when it needs attention,
/// `--pane-2` / `--faint` when it is a plain or zero count.
pub fn count_chip(count: u64, attention: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (bg, fg) = if attention {
        (t.coral, crate::color::hex(0x0a0c10))
    } else {
        (t.pane_2, t.faint)
    };
    div().flex_none().rounded(px(10.)).px(px(6.)).bg(bg).child(
        text(count.to_string())
            .weight(weight::SEMIBOLD)
            .color(fg)
            .nowrap(),
    )
}

/// `.skel`: a loading placeholder with the shimmer sweep (`::after`, 1.4s, `translateX`
/// from -100% to 100%). Every skeleton sweeps in step, from the first one drawn.
pub fn skeleton(window: &mut Window, cx: &App) -> Div {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t = tokens(cx);
    let skel = div()
        .relative()
        .overflow_hidden()
        .bg(t.pane_2)
        .rounded(px(5.))
        .min_h(px(12.));
    if crate::motion::reduced(cx) {
        return skel;
    }
    if !crate::motion::is_frozen(cx) {
        window.request_animation_frame();
    }
    let period = 1.4;
    let elapsed = crate::motion::elapsed(*START.get_or_init(std::time::Instant::now), cx);
    let p = (elapsed.as_secs_f32() % period) / period;
    // `ease`, the animation's default timing function.
    let x = -1. + 2. * crate::motion::Easing::EASE.apply(p);
    let sheen = if Theme::of(cx).id.is_light() {
        rgba(0, 0, 0, 0.06)
    } else {
        rgba(255, 255, 255, 0.045)
    };
    // GPUI gradients take two stops, so each half of the three-stop sweep is its own box.
    let half = |from: Color, to: Color| {
        div()
            .h_full()
            .w(relative(0.5))
            .bg(gpui_kit::linear_gradient(
                90.,
                gpui_kit::linear_color_stop(from, 0.),
                gpui_kit::linear_color_stop(to, 1.),
            ))
    };
    skel.child(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(x))
            .w_full()
            .flex()
            .child(half(sheen.fade(0.), sheen))
            .child(half(sheen, sheen.fade(0.))),
    )
}

/// `.skelrow`.
pub fn skeleton_row(cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(12.))
        .px(px(16.))
        .border_b(px(1.))
        .border_color(Hsla::from(tokens(cx).line_soft))
}
