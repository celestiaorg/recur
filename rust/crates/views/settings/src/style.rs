//! `_settings.scss` and the shared rules Settings uses (`.seg`, `.tbtn`, `.inp`, the theme
//! cards of `_overlays.scss`): rows, copy, controls, notes and fields.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Div, ElementId, Entity, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div, px, relative,
};
use recur_ui::fonts::{Family, weight};
use recur_ui::style::{card, text_field};
use recur_ui::text::{InlineBox, LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::{Color, IconName, Theme, Tokens, icon, rgba};

pub fn hsla(c: Color) -> Hsla {
    c.into()
}

pub fn tokens(cx: &App) -> Tokens {
    Theme::of(cx).tokens
}

/// `@media (max-width: 860px)`.
pub fn narrow(window: &Window) -> bool {
    f32::from(window.viewport_size().width) <= 860.
}

// ---- copy ----

/// `.set-title`: 13px, 600, `--text`.
pub fn title(label: impl Into<SharedString>, cx: &App) -> Text {
    text(label)
        .size(13.)
        .weight(weight::SEMIBOLD)
        .leading(1.45)
        .color(tokens(cx).text)
}

/// `.set-desc` (and `.set-lead`'s) inline `<code>`: mono 11px amber on an amber wash.
pub fn code(c: impl Into<SharedString>, cx: &App) -> Span {
    span(c).mono().size(11.).color(tokens(cx).amber).boxed(InlineBox {
        pad_top: 1.,
        pad_bottom: 1.,
        pad_left: 6.,
        pad_right: 6.,
        background: Some(hsla(rgba(242, 179, 61, 0.1))),
        radius: 4.,
        ..Default::default()
    })
}

/// `.set-desc` text (no margin): 12px `--faint`, line-height 1.4.
pub fn desc_text(id: impl Into<ElementId>, spans: Vec<Span>, cx: &App) -> Text {
    Text::from_spans(spans)
        .id(id)
        .size(12.)
        .leading(1.4)
        .color(tokens(cx).faint)
}

/// `.set-desc`: [`desc_text`] 3px below the title.
pub fn desc(id: impl Into<ElementId>, spans: Vec<Span>, cx: &App) -> Div {
    div().mt(px(3.)).child(desc_text(id, spans, cx))
}

/// `.set-copy` with a title and an optional description.
pub fn copy(id: &str, label: impl Into<SharedString>, d: Option<Vec<Span>>, cx: &App) -> Div {
    let mut c = div().flex_1().min_w_0().child(title(label, cx));
    if let Some(spans) = d {
        c = c.child(desc(SharedString::from(format!("{id}:desc")), spans, cx));
    }
    c
}

/// `.set-ctrl`.
pub fn ctrl() -> Div {
    div().flex_none().flex().items_center().justify_end()
}

/// `.set-ctrl.set-ctrl-text`: 12.5px `--muted`, at most 48% of the line wide (42% narrow) and
/// at its start, the text right-aligned inside.
pub fn ctrl_text(id: impl Into<ElementId>, spans: Vec<Span>, window: &Window, cx: &App) -> Div {
    let cap = if narrow(window) { 0.42 } else { 0.48 };
    div()
        .flex_none()
        .w_full()
        .max_w(relative(cap))
        .flex()
        .items_center()
        .justify_end()
        .child(
            Text::from_spans(spans)
                .id(id)
                .size(12.5)
                .leading(1.45)
                .color(tokens(cx).muted)
                .align(recur_ui::text::Align::Right),
        )
}

/// `.set-lead`: 12px `--faint`, line-height 1.4, padding 2px 0 4px.
pub fn lead(id: impl Into<ElementId>, spans: Vec<Span>, cx: &App) -> Div {
    div().pt(px(2.)).pb(px(4.)).child(
        Text::from_spans(spans)
            .id(id)
            .size(12.)
            .leading(1.4)
            .color(tokens(cx).faint),
    )
}

// ---- rows ----

/// Where a `.set-rows` list sits, which decides its borders and paddings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowsCtx {
    /// `.code-panel-sides`: tighter rows, no outer rules.
    pub sides: bool,
    /// A direct child of `.set-subsec-body`.
    pub in_group: bool,
    /// Other children follow the rows in their list, so the last row is no `:last-child`.
    pub more: bool,
}

/// One `.set-row`.
pub struct Row {
    pub stack: bool,
    pub danger: bool,
    pub id: Option<SharedString>,
    pub children: Vec<AnyElement>,
}

impl Row {
    pub fn new() -> Row {
        Row {
            stack: false,
            danger: false,
            id: None,
            children: Vec::new(),
        }
    }
    /// `.set-row-stack`.
    pub fn stack() -> Row {
        Row {
            stack: true,
            ..Row::new()
        }
    }
    /// `.set-row-danger`.
    pub fn danger(mut self) -> Row {
        self.danger = true;
        self
    }
    pub fn id(mut self, id: impl Into<SharedString>) -> Row {
        self.id = Some(id.into());
        self
    }
    pub fn child(mut self, c: impl IntoElement) -> Row {
        self.children.push(c.into_any_element());
        self
    }
}

impl Default for Row {
    fn default() -> Self {
        Row::new()
    }
}

/// `.set-rows` with its rows: the cascade of `.set-row`, `.code-panel-sides .set-row`
/// (`:first-child` / `:last-child`) and `.set-subsec-body` (no outer rules).
pub fn rows(ctx: RowsCtx, list: Vec<Row>, window: &Window, cx: &App) -> Div {
    let t = tokens(cx);
    let n = list.len();
    let wrap = narrow(window);
    let mut out = div().flex().flex_col();
    if !ctx.sides && !ctx.in_group {
        out = out.border_t(px(1.)).border_color(hsla(t.line_soft));
    }
    for (i, r) in list.into_iter().enumerate() {
        let (first, last) = (i == 0, i + 1 == n && !ctx.more);
        let (mut top, mut bottom) = if ctx.sides { (10., 10.) } else { (16., 16.) };
        if ctx.sides && first {
            // `.set-subsec-body .code-panel-sides .set-row:first-child` outranks the 0.
            top = if ctx.in_group { 10. } else { 0. };
        }
        let mut rule = true;
        if ctx.sides && last {
            bottom = 0.;
            rule = false;
        }
        if ctx.in_group && last {
            rule = false;
        }
        let mut row = div().flex().pt(px(top)).pb(px(bottom));
        if let Some(id) = r.id.clone() {
            row = row.debug_selector(move || format!("set-row:{id}"));
        }
        row = if r.stack {
            row.flex_col().gap(px(14.))
        } else {
            row.flex_row()
                .gap(px(16.))
                .when(wrap, |d| d.flex_wrap())
                .map(|d| if r.danger { d.items_start() } else { d.items_center() })
        };
        if rule {
            row = row.border_b(px(1.)).border_color(hsla(t.line_soft));
        }
        out = out.child(row.children(r.children));
    }
    out
}

/// `.set-rows:has(.merge-method-radios)`: a grid of rows whose second column is as wide as the
/// widest control (`minmax(0, 1fr) auto`, subgridded rows with no gap), radios hanging 6px up.
/// `ctrl_w` is that width; under 860px the rows stack.
pub fn radio_rows(
    rows_list: Vec<(AnyElement, AnyElement, bool)>,
    ctrl_w: f32,
    window: &Window,
    cx: &App,
) -> Div {
    let t = tokens(cx);
    let n = rows_list.len();
    let stacked = narrow(window);
    let mut out = div()
        .flex()
        .flex_col()
        .border_t(px(1.))
        .border_color(hsla(t.line_soft));
    for (i, (copy, control, radios)) in rows_list.into_iter().enumerate() {
        let _ = i + 1 == n;
        let row = div()
            .flex()
            .py(px(16.))
            .border_b(px(1.))
            .border_color(hsla(t.line_soft));
        let row = if stacked {
            row.flex_col().gap(px(10.)).child(copy).child(control)
        } else {
            row.flex_row()
                .map(|d| if radios { d.items_start() } else { d.items_center() })
                .child(div().flex_1().min_w_0().flex().child(copy))
                .child(
                    div()
                        .flex_none()
                        .w(px(ctrl_w))
                        .flex()
                        .when(!radios, |d| d.justify_end())
                        .child(control),
                )
        };
        out = out.child(row);
    }
    out
}

// ---- controls ----

/// `.seg`.
pub fn seg(cx: &App) -> Div {
    let t = tokens(cx);
    card(t.pane, t.line, 6.).flex().p(px(2.))
}

/// `.seg button` (`padding: 4px 11px`, or 4px 8px in `.indent-lang-rows`).
pub fn seg_btn(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    disabled: bool,
    pad_x: f32,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let (mut bg, mut fg) = if on {
        (Some(t.pane_2), t.text)
    } else {
        (None, t.muted)
    };
    if disabled {
        // `.seg button:disabled { opacity: .45 }` over the segment's `--pane`.
        fg = recur_ui::style::faded(fg, bg.unwrap_or(t.pane), 0.45, t.pane);
        bg = bg.map(|b| recur_ui::style::faded(b, b, 0.45, t.pane));
    }
    div()
        .id(id)
        .flex()
        .items_center()
        .py(px(4.))
        .px(px(pad_x))
        .rounded(px(4.))
        .when_some(bg, |d, b| d.bg(hsla(b)))
        .text_color(hsla(fg))
        .map(|d| {
            if disabled {
                d.cursor_not_allowed()
            } else {
                d.cursor_pointer()
            }
        })
        .child(recur_ui::style::button_label(label, 11.5, weight::MEDIUM))
}

/// `.mm-radio`: the merge-method style radio row with its `.mm-dot`.
pub fn mm_radio(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let dot = div()
        .flex_none()
        .size(px(14.))
        .rounded_full()
        .border(px(2.))
        .border_color(hsla(if on { t.emerald } else { t.line_bright }))
        .flex()
        .items_center()
        .justify_center()
        .when(on, |d| {
            d.child(div().size(px(7.)).rounded_full().bg(hsla(t.emerald)))
        });
    let hover = if Theme::of(cx).is_light() {
        rgba(0, 0, 0, 0.04)
    } else {
        rgba(255, 255, 255, 0.03)
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(9.))
        .py(px(6.))
        .px(px(10.))
        .rounded(px(6.))
        .text_color(hsla(if on { t.text } else { t.muted }))
        .cursor_pointer()
        .hover(move |s| s.bg(hsla(hover)).text_color(hsla(t.text)))
        .child(dot)
        .child(recur_ui::style::button_label(label, 12.5, weight::REGULAR))
}

/// The width a `.merge-method-radios` column takes: its widest `.mm-radio`.
pub fn radios_width(labels: &[&str], window: &Window) -> f32 {
    labels
        .iter()
        .map(|l| {
            let (w, _, _) = recur_ui::style::button_label(l.to_string(), 12.5, weight::REGULAR)
                .measure(None, window);
            10. + 14. + 9. + w + 10.
        })
        .fold(0., f32::max)
}

/// `.tbtn` (base size).
pub fn tbtn(id: impl Into<ElementId>, disabled: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    recur_ui::style::tbtn(
        id,
        recur_ui::style::Tbtn::Default,
        recur_ui::style::TbtnSize::BASE,
        disabled.then_some(t.bg),
        cx,
    )
}

/// `.tbtn` with a text label.
pub fn tbtn_label(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
    cx: &App,
) -> Stateful<Div> {
    tbtn(id, disabled, cx).child(recur_ui::style::button_label(label, 12., weight::MEDIUM))
}

/// `.tbtn.set-danger-btn` (`.solid` fills it coral).
pub fn danger_btn(id: impl Into<ElementId>, solid: bool, disabled: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    let (bg, border, fg) = if solid {
        (t.coral, t.coral, recur_ui::hex(0x0a0c10))
    } else {
        (t.pane, rgba(235, 122, 92, 0.32), t.coral)
    };
    let mut b = card(bg, border, 6.)
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(11.))
        .text_color(hsla(fg))
        .text_size(px(12.));
    b = if disabled {
        b.cursor_not_allowed()
    } else {
        b.cursor_pointer()
    };
    b
}

/// A 13px icon inside a `.tbtn`.
pub fn tbtn_icon(name: IconName) -> impl IntoElement {
    icon(name).size(px(13.))
}

/// `.exclude-rm`: the 28px trash / × button.
pub fn rm_btn(id: impl Into<ElementId>, name: IconName, disabled: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    div()
        .id(id)
        .flex_none()
        .size(px(28.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(hsla(t.faint))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hsla(t.pane_2)).text_color(hsla(t.text)))
        })
        .child(icon(name).size(px(14.)))
}

// ---- fields ----

/// The look of an `.inp` field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InpStyle {
    pub pad_y: f32,
    pub pad_x: f32,
    pub width: Option<f32>,
    pub min_h: Option<f32>,
    pub family_ui: bool,
    pub weight: f32,
}

impl InpStyle {
    /// `.inp`: mono 12px, padding 9px 12px.
    pub const BASE: InpStyle = InpStyle {
        pad_y: 9.,
        pad_x: 12.,
        width: None,
        min_h: None,
        family_ui: false,
        weight: weight::REGULAR,
    };
    /// `.set-num`: 72px, padding 7px 10px.
    pub const NUM: InpStyle = InpStyle {
        pad_y: 7.,
        pad_x: 10.,
        width: Some(72.),
        ..InpStyle::BASE
    };
    /// `.exclude-inp`: padding 7px 10px.
    pub const EXCLUDE: InpStyle = InpStyle {
        pad_y: 7.,
        pad_x: 10.,
        ..InpStyle::BASE
    };
    /// `.compat-models .inp`: at least 36px tall.
    pub const COMPAT: InpStyle = InpStyle {
        min_h: Some(36.),
        ..InpStyle::BASE
    };
    /// `.compat-models .compat-flag-card .inp`: at least 30px, padding 5px 12px.
    pub const FLAG: InpStyle = InpStyle {
        pad_y: 5.,
        min_h: Some(30.),
        ..InpStyle::BASE
    };
}

/// `.inp` around a field: `--pane` on `--line` (amber 50% while focused), radius 6.
pub fn inp(
    input: &Entity<gpui_kit::base::input::InputState>,
    placeholder: &SharedString,
    style: InpStyle,
    window: &Window,
    cx: &App,
) -> Div {
    let t = tokens(cx);
    let focused = gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window);
    let border = if focused {
        rgba(242, 179, 61, 0.5)
    } else {
        t.line
    };
    let family = if style.family_ui {
        Family::Ui
    } else {
        Family::Mono
    };
    card(t.pane, border, 6.)
        .flex()
        .items_center()
        .py(px(style.pad_y))
        .px(px(style.pad_x))
        .when_some(style.width, |d, w| d.flex_none().w(px(w)))
        .when(style.width.is_none(), |d| d.flex_1().min_w_0())
        .when_some(style.min_h, |d, h| d.min_h(px(h)))
        .font_weight(gpui_kit::FontWeight(style.weight))
        .child(text_field(
            input,
            placeholder,
            recur_ui::style::UA_PLACEHOLDER,
            family,
            12.,
            window,
            cx,
        ))
}

/// `.set-search`: the magnifier and a borderless Inter 12px field on a `--pane` pill, 32px tall
/// but shrinking (in a column that overflows) down to the field's own 21px.
pub fn search_box(
    input: &Entity<gpui_kit::base::input::InputState>,
    placeholder: &SharedString,
    window: &Window,
    cx: &App,
) -> Div {
    let t = tokens(cx);
    let focused = gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window);
    let (border, ico) = if focused {
        (rgba(242, 179, 61, 0.45), t.muted)
    } else {
        (t.line, t.faint)
    };
    card(t.pane, border, 8.)
        .flex()
        .flex_shrink(1.)
        .items_center()
        .gap(px(8.))
        .h(px(32.))
        .min_h(px(23.))
        .px(px(10.))
        .child(
            div()
                .flex_none()
                .child(icon(IconName::Search).size(px(13.)).color(ico)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h(px(21.))
                .flex()
                .items_center()
                .child(text_field(
                    input,
                    placeholder,
                    t.faint,
                    Family::Ui,
                    12.,
                    window,
                    cx,
                )),
        )
}

// ---- notes and messages ----

/// `.set-field-msg` (`.ok` emerald, `.bad` coral, else muted).
pub fn field_msg(id: impl Into<ElementId>, msg: &str, ok: Option<bool>, cx: &App) -> Div {
    let t = tokens(cx);
    let c = match ok {
        Some(true) => t.emerald,
        Some(false) => t.coral,
        None => t.muted,
    };
    div().mt(px(2.)).child(
        text(msg.to_string())
            .id(id)
            .size(12.)
            .leading(1.35)
            .color(c),
    )
}

/// `.acct-status-head` (`muted` / `ok` / `bad` / `warn`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    Muted,
    Ok,
    Bad,
    Warn,
}

pub fn status_head(kind: Head, label: impl Into<SharedString>, cx: &App) -> Div {
    let t = tokens(cx);
    let (color, wt) = match kind {
        Head::Muted => (t.muted, weight::MEDIUM),
        Head::Bad => (t.muted, weight::MEDIUM),
        Head::Ok | Head::Warn => (t.text, weight::SEMIBOLD),
    };
    let mut head = div().flex().items_center().gap(px(8.));
    if kind != Head::Muted {
        let (fill, glow) = match kind {
            Head::Ok => (t.emerald, Some(t.emerald)),
            Head::Warn => (t.amber, Some(t.amber)),
            _ => (t.coral, None),
        };
        let dot = div().size(px(8.)).rounded_full().bg(hsla(fill));
        head = head.child(match glow {
            Some(g) => recur_ui::shadow::with_shadow(
                dot,
                &[recur_ui::shadow::css_box_shadow(
                    0.,
                    0.,
                    0.,
                    3.,
                    g.mix(0.22, recur_ui::color::TRANSPARENT),
                )],
                4.,
            )
            .flex_none()
            .into_any_element(),
            None => dot.flex_none().into_any_element(),
        });
    }
    head.child(
        text(label)
            .size(13.)
            .weight(wt)
            .line_height(LineHeight::Scale(1.2))
            .color(color)
            .nowrap(),
    )
}

/// `.acct-status-table` with `(dt, dd, titled)` rows; a titled `dd` tooltips its full value.
pub fn status_table(id: &str, items: Vec<(&str, String, bool)>, cx: &App) -> Div {
    let t = tokens(cx);
    let mut table = card(t.pane, t.line, 8.).flex().flex_col().overflow_hidden();
    let n = items.len();
    for (i, (dt, dd, titled)) in items.into_iter().enumerate() {
        let cell = |c: Div| c.py(px(9.)).px(px(12.)).min_w_0();
        let dt_cell = cell(div())
            .flex_none()
            .w(relative(0.34))
            .min_w(px(110.))
            .bg(hsla(t.pane_2.mix(0.7, recur_ui::color::TRANSPARENT)))
            .when(i == 0, |d| d.rounded_tl(px(7.)))
            .when(i + 1 == n, |d| d.rounded_bl(px(7.)))
            .child(
                text(dt.to_string())
                    .size(12.5)
                    .leading(1.35)
                    .color(t.muted),
            );
        let dd_text = text(dd.clone())
            .size(12.5)
            .leading(1.35)
            .color(t.text)
            .truncate();
        let dd_inner = div().min_w_0().child(dd_text);
        let dd_cell = cell(div()).flex_1().child(if titled {
            recur_ui::tooltip::titled(SharedString::from(format!("{id}:dd:{i}")), dd, dd_inner)
                .into_any_element()
        } else {
            dd_inner.into_any_element()
        });
        table = table.child(
            div()
                .flex()
                .when(i > 0, |d| d.border_t(px(1.)).border_color(hsla(t.line)))
                .child(dt_cell)
                .child(dd_cell),
        );
    }
    table
}

/// `.set-note` (`.warn` coral wash): an icon, the message and optional trailing controls.
pub fn note(warn: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (bg, border) = if warn {
        (rgba(231, 111, 81, 0.06), rgba(231, 111, 81, 0.28))
    } else {
        (t.pane, t.line)
    };
    card(bg, border, 8.)
        .flex()
        .gap(px(10.))
        .items_start()
        .mt(px(18.))
        .py(px(12.))
        .px(px(13.))
        .text_color(hsla(t.muted))
}

// ---- theme cards (`_overlays.scss`) ----

/// `.thauto-l`: the Light mode / Dark mode label over a grid.
pub fn thauto_label(label: &str, cx: &App) -> Div {
    div().mt(px(10.)).mb(px(6.)).child(
        text(label.to_string())
            .size(11.)
            .leading(1.45)
            .tracking(0.06)
            .uppercase()
            .color(tokens(cx).muted),
    )
}

/// `.edth-swatch`: a 44×22 chip on the theme background with three dots.
pub fn swatch(bg: Color, dots: [Color; 3], cx: &App) -> Div {
    let t = tokens(cx);
    let mut s = card(bg, t.line.mix(0.7, recur_ui::color::TRANSPARENT), 5.)
        .flex_none()
        .w(px(44.))
        .h(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(4.));
    for c in dots {
        s = s.child(div().size(px(7.)).rounded_full().bg(hsla(c)));
    }
    s
}

/// A CSS color string from theme metadata, or `fallback` when it does not parse.
pub fn css_color(s: &str, fallback: Color) -> Color {
    Color::parse(s).unwrap_or(fallback)
}

/// `.tog` with its accessible name and spec handle (`tog:{label}:{on}`).
pub fn tog(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    disabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    let l = label.clone();
    recur_ui::style::toggle(id, on, disabled, cx)
        .role(gpui_kit::accesskit::Role::Switch)
        .aria_label(label)
        .debug_selector(move || format!("tog:{l}:{on}"))
}

/// `.skill-copy`: a mono name, an optional description and an optional `title`d path.
pub fn skill_copy(
    id: &str,
    name: impl Into<SharedString>,
    desc: Option<String>,
    path: Option<(String, String)>,
    cx: &App,
) -> Div {
    let t = tokens(cx);
    let mut c = div().flex_1().min_w_0().child(
        text(name)
            .mono()
            .size(13.)
            .weight(weight::SEMIBOLD)
            .leading(1.45)
            .color(t.text),
    );
    if let Some(d) = desc {
        c = c.child(
            div().mt(px(3.)).child(
                text(d)
                    .id(SharedString::from(format!("{id}:desc")))
                    .size(12.)
                    .leading(1.4)
                    .color(t.faint),
            ),
        );
    }
    if let Some((loc, full)) = path {
        c = c.child(div().mt(px(5.)).child(recur_ui::tooltip::titled(
            SharedString::from(format!("{id}:path-tip")),
            full,
            text(loc)
                .id(SharedString::from(format!("{id}:path")))
                .mono()
                .size(11.)
                .leading(1.35)
                .color(t.muted)
                .break_anywhere(),
        )));
    }
    c
}

/// `.skill-list` of `.skill-row`s: each a copy column and trailing controls (a `.tog` sits
/// 2px down).
pub fn skill_list(rows: Vec<(Div, Vec<AnyElement>)>, cx: &App) -> Div {
    let t = tokens(cx);
    let n = rows.len();
    let mut list = div().flex().flex_col();
    for (i, (copy, trailing)) in rows.into_iter().enumerate() {
        let last = i + 1 == n;
        list = list.child(
            div()
                .flex()
                .items_start()
                .gap(px(16.))
                .pt(px(12.))
                .pb(px(if last { 2. } else { 12. }))
                .when(!last, |d| d.border_b(px(1.)).border_color(hsla(t.line_soft)))
                .child(copy)
                .children(trailing),
        );
    }
    list
}
