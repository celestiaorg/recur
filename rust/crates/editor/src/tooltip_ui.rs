//! Editor tooltips: the completion list and its info pane
//! (`@codemirror/autocomplete` restyled by `_codemirror.scss`), the hover
//! host (LSP hover + lint diagnostics) and LSP signature help, positioned by
//! CodeMirror's tooltip placement (`writeMeasure`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Div, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, ScrollWheelEvent, SharedString, Size, StatefulInteractiveElement,
    Styled, Window, deferred, div, point, px, size,
};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::anchored_at;
use recur_ui::text::Align;
use recur_ui::{Text, TextStyled, Theme, Tokens, rgba, span, text};

use crate::autocomplete::{CompletionDialog, CompletionOption, range_around_selected};
use crate::lint::Diagnostic;
use crate::lsp::{HoverInfo, HoverPart};
use crate::markdown::{DocStyle, render_code, render_doc};
use crate::state::{EditorState, Transaction};
use crate::view::EditorView;

/// `maxRenderedOptions`.
const MAX_RENDERED_OPTIONS: usize = 100;
/// `li` box height: `min-height: 26px`.
const OPTION_HEIGHT: f32 = 26.0;
/// `Info.Width` / `Info.Margin`.
const INFO_WIDTH: f32 = 400.0;
const INFO_MARGIN: f32 = 30.0;
/// The completion list's font size (`ul { font-size: 12.5px }`).
const LIST_FONT: f32 = 12.5;
/// The editor's inherited `line-height` (`body`).
const BODY_LEADING: f32 = 1.45;

fn h(c: impl Into<Hsla>) -> Hsla {
    c.into()
}

/// One hover source's tooltip (`HoverTooltip`).
#[derive(Clone, Debug)]
pub(crate) struct HoverSection {
    pub pos: usize,
    pub end: usize,
    pub above: bool,
    pub body: HoverBody,
}

#[derive(Clone, Debug)]
pub(crate) enum HoverBody {
    Lint(Vec<Rc<Diagnostic>>),
    Lsp(Vec<HoverPart>),
}

/// Tooltip state the view keeps between frames.
#[derive(Default)]
pub(crate) struct Tooltips {
    pub lsp_hover: Option<HoverSection>,
    pub lint_hover: Option<HoverSection>,
    pub pointer_in_hover: bool,
    pub lsp_hover_pending: Option<(u64, usize)>,
    /// Completion list `scrollTop`.
    pub list_scroll: f32,
    list_for: Option<(usize, usize)>,
    /// Laid-out sizes from the previous frame (`readMeasure`).
    sizes: Rc<RefCell<HashMap<&'static str, Size<Pixels>>>>,
    /// Sides chosen last time (`this.above`).
    above: HashMap<&'static str, bool>,
}

impl Tooltips {
    pub fn hide_hover(&mut self) {
        self.lsp_hover = None;
        self.lint_hover = None;
        self.lsp_hover_pending = None;
        self.pointer_in_hover = false;
    }

    pub fn has_hover(&self) -> bool {
        self.lsp_hover.is_some() || self.lint_hover.is_some()
    }

    /// `lintTooltip`: diagnostics touching the hovered position.
    pub fn set_lint_hover(
        &mut self,
        _pos: usize,
        found: Option<(usize, usize, Vec<Rc<Diagnostic>>)>,
        state: &EditorState,
    ) {
        self.lint_hover = found.map(|(from, to, diags)| HoverSection {
            pos: from,
            end: to,
            above: state.doc.line_at(from).to < to,
            body: HoverBody::Lint(diags),
        });
    }

    pub fn set_lsp_hover(&mut self, info: HoverInfo) {
        self.lsp_hover = Some(HoverSection {
            pos: info.from,
            end: info.to,
            above: true,
            body: HoverBody::Lsp(info.parts),
        });
    }

    /// Hover tooltips after a transaction: LSP hovers hide on edits
    /// (`hideOn: docChanged`), lint hovers on `hideLintTooltip`, the rest
    /// map through the changes.
    pub fn after_transaction(&mut self, tr: &Transaction, _state: &EditorState) {
        if tr.doc_changed() {
            self.lsp_hover = None;
            self.lsp_hover_pending = None;
        }
        if let Some(l) = self.lint_hover.as_ref()
            && crate::lint::hide_lint_tooltip(tr, l.pos, l.end)
        {
            self.lint_hover = None;
        }
        if tr.doc_changed()
            && let Some(l) = self.lint_hover.as_mut()
        {
            l.pos = tr.changes.map_pos(l.pos, 1);
            l.end = tr.changes.map_pos(l.end, -1);
        }
        if !self.has_hover() {
            self.pointer_in_hover = false;
        }
    }

    pub fn signature_changed(&mut self) {
        self.above.remove("signature");
    }

    /// Options a page step moves (`offsetHeight / li height - 1`).
    pub fn completion_page(&self) -> usize {
        let h = self
            .sizes
            .borrow()
            .get("completion")
            .map_or(0.0, |s| f32::from(s.height));
        ((h / OPTION_HEIGHT).floor() as usize).saturating_sub(1)
    }

    fn measured(&self, key: &'static str) -> Option<Size<Pixels>> {
        self.sizes.borrow().get(key).copied()
    }

    /// Keep the selected option in view (`scrollIntoView` on the list).
    fn scroll_to_selected(
        &mut self,
        dialog: &CompletionDialog,
        range: (usize, usize),
        list_h: f32,
        top_marker: f32,
    ) {
        let Some(sel) = dialog.selected else { return };
        if self.list_for == Some((sel, dialog.options.len())) {
            return;
        }
        self.list_for = Some((sel, dialog.options.len()));
        let top = top_marker + (sel.saturating_sub(range.0)) as f32 * OPTION_HEIGHT;
        let bottom = top + OPTION_HEIGHT;
        if top < self.list_scroll {
            self.list_scroll = top;
        } else if bottom > self.list_scroll + list_h {
            self.list_scroll = bottom - list_h;
        }
    }
}

/// Where a tooltip goes (`writeMeasure`): left/top in window coordinates,
/// side and a clamped height.
fn place(
    anchor: (f32, f32, f32),
    size: Size<Pixels>,
    space: Bounds<Pixels>,
    above_pref: bool,
    line_height: f32,
) -> Option<(f32, f32, bool, Option<f32>)> {
    let (x, top, bottom) = anchor;
    let (width, mut height) = (f32::from(size.width), f32::from(size.height));
    let (sl, st) = (f32::from(space.origin.x), f32::from(space.origin.y));
    let (sr, sb) = (
        sl + f32::from(space.size.width),
        st + f32::from(space.size.height),
    );
    let left = if width > sr - sl {
        sl
    } else {
        sl.max(x.min(sr - width))
    };
    let mut above = above_pref;
    if (if above {
        top - height < st
    } else {
        bottom + height > sb
    }) && above == (sb - bottom > top - st)
    {
        above = !above;
    }
    let space_vert = if above { top - st } else { sb - bottom };
    let mut clamp = None;
    if space_vert < height {
        if space_vert < line_height {
            return None;
        }
        height = space_vert;
        clamp = Some(height);
    }
    let y = if above { top - height } else { bottom };
    Some((left, y, above, clamp))
}

/// `.cm-tooltip`: pane card, `--line-bright` border, radius 8, and its
/// shadow (lighter on light app themes).
fn card(theme: &Theme) -> Div {
    let t = &theme.tokens;
    recur_ui::style::card(t.pane, t.line_bright, 8.).text_color(h(t.text))
}

fn tooltip_shadow(theme: &Theme) -> [recur_ui::shadow::CssShadow; 1] {
    if theme.is_light() {
        [css_box_shadow(0., 12., 28., 0., rgba(0, 0, 0, 0.12))]
    } else {
        [css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))]
    }
}

/// A card with its shadow behind it.
fn shadowed(theme: &Theme, body: impl IntoElement) -> Div {
    with_shadow(body, &tooltip_shadow(theme), 8.)
}

/// `.cm-completionIcon-*:after` letters.
fn icon_letters(kind: Option<&str>) -> &'static str {
    let Some(kind) = kind else { return "" };
    for k in kind.split_whitespace() {
        let l = match k {
            "function" | "method" => "fn",
            "class" => "C",
            "interface" => "I",
            "variable" => "v",
            "constant" => "k",
            "type" => "T",
            "enum" => "E",
            "property" => "p",
            "keyword" => "kw",
            "namespace" => "ns",
            "text" => "W",
            "snippet" => "S",
            _ => continue,
        };
        return l;
    }
    ""
}

impl EditorView {
    /// The tooltip space (`tooltipSpace`): the window, or with LSP attached
    /// the scroller inset by `TOOLTIP_EDGE_MARGIN`.
    fn tooltip_space(&self, window: &Window) -> Bounds<Pixels> {
        let f = self.frame.as_ref();
        match (self.lsp.is_some(), f) {
            (true, Some(f)) => {
                let m = crate::lsp::TOOLTIP_EDGE_MARGIN;
                Bounds::new(
                    point(f.bounds.origin.x + px(m), px(f.scroller_top + m)),
                    size(
                        px(f.scroller_width - 2.0 * m),
                        px(f.scroller_height - 2.0 * m),
                    ),
                )
            }
            _ => Bounds::new(point(px(0.), px(0.)), window.viewport_size()),
        }
    }

    /// `coordsAtPos`: x and the text box top/bottom, in window coordinates;
    /// `None` outside the visible scroller.
    pub(crate) fn pos_coords(
        &self,
        pos: usize,
        side: i32,
        window: &Window,
    ) -> Option<(f32, f32, f32)> {
        let f = self.frame.as_ref()?;
        let tb = self.text_box(window);
        let pos = pos.min(self.state.doc.len());
        let (top, _) = self.with_layout_view(window, |lv| lv.row_box(pos, side));
        let x = self.with_layout_view(window, |lv| lv.x_at(pos, side));
        let left = f32::from(f.bounds.origin.x) + f.gutter_width - self.scroll_left
            + self.metrics.line_padding_left
            + x;
        let y = f.scroller_top - self.scroll_top + top + tb.top();
        let (vt, vb) = (f.scroller_top, f.scroller_top + f.scroller_height);
        let (vl, vr) = (
            f32::from(f.bounds.origin.x),
            f32::from(f.bounds.origin.x) + f.scroller_width,
        );
        if y + tb.height() <= vt || y >= vb || left < vl - 0.1 || left > vr + 0.1 {
            return None;
        }
        Some((left, y, y + tb.height()))
    }

    /// Wrap a tooltip body: measure it for next frame and place it.
    fn positioned(
        &mut self,
        key: &'static str,
        anchor: (f32, f32, f32),
        above_pref: bool,
        body: Div,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, Bounds<Pixels>)> {
        let space = self.tooltip_space(window);
        let known = self.tooltips.measured(key);
        let lh = self.metrics.line_height();
        let sizes = self.tooltips.sizes.clone();
        let ent = cx.entity();
        let measure = move |bounds: Vec<Bounds<Pixels>>, _: &mut Window, cx: &mut App| {
            let Some(b) = bounds.first() else { return };
            let prev = sizes.borrow_mut().insert(key, b.size);
            if prev.is_none_or(|p| {
                (f32::from(p.width) - f32::from(b.size.width)).abs() > 0.5
                    || (f32::from(p.height) - f32::from(b.size.height)).abs() > 0.5
            }) {
                ent.update(cx, |_, cx| cx.notify());
            }
        };
        let Some(sz) = known else {
            // First frame: lay out off-screen to measure, like `Outside`.
            let el = deferred(anchored_at(
                -10000.,
                -10000.,
                div().on_children_prepainted(measure).child(body),
            ));
            return Some((el.into_any_element(), Bounds::default()));
        };
        let prev_above = self.tooltips.above.get(key).copied().unwrap_or(above_pref);
        let (left, top, above, clamp) = place(anchor, sz, space, prev_above, lh)?;
        self.tooltips.above.insert(key, above);
        let wrapper = div()
            .on_children_prepainted(measure)
            .when_some(clamp, |d, c| d.max_h(px(c)))
            .child(body);
        let el = deferred(anchored_at(left, top, wrapper)).with_priority(1);
        Some((
            el.into_any_element(),
            Bounds::new(point(px(left), px(top)), sz),
        ))
    }

    /// All visible tooltips, as overlay elements.
    pub(crate) fn render_tooltips(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let focused = self.focus.is_focused(window);
        if let Some(open) = self.state.completion.open.clone()
            && focused
            && let Some(el) = self.render_completion(&open, window, cx)
        {
            out.extend(el);
        }
        if let Some(el) = self.render_hover(window, cx) {
            out.push(el);
        }
        if let Some(el) = self.render_signature(window, cx) {
            out.push(el);
        }
        out
    }

    fn render_completion(
        &mut self,
        open: &Rc<CompletionDialog>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        let theme = Theme::of(cx).clone();
        let t = theme.tokens;
        let anchor = self.pos_coords(open.pos, 1, window)?;
        let range = range_around_selected(open.options.len(), open.selected, MAX_RENDERED_OPTIONS);
        let vh = f32::from(window.viewport_size().height);
        let list_max = (16.0 * LIST_FONT).min(0.4 * vh);
        // `:before` / `:after` "···": a block line at the inherited 1.45,
        // padded 4px above and below.
        let marker_h = 4.0 + 4.0 + LIST_FONT * BODY_LEADING;
        let top_marker = if range.0 > 0 { marker_h } else { 0.0 };
        let bottom_marker = if range.1 < open.options.len() {
            marker_h
        } else {
            0.0
        };
        let content_h = top_marker + (range.1 - range.0) as f32 * OPTION_HEIGHT + bottom_marker;
        let list_h = content_h.min(list_max);
        self.tooltips
            .scroll_to_selected(open, range, list_h, top_marker);
        self.tooltips.list_scroll = self
            .tooltips
            .list_scroll
            .clamp(0.0, (content_h - list_h).max(0.0));
        let scroll = self.tooltips.list_scroll;
        let ent = cx.entity();
        let marker = || {
            div().w_full().py(px(4.)).child(
                text("···")
                    .mono()
                    .size(LIST_FONT)
                    .leading(BODY_LEADING)
                    .color(h(t.faint))
                    .align(Align::Center)
                    .nowrap(),
            )
        };
        let mut items = div().flex().flex_col().relative().top(px(-scroll));
        if range.0 > 0 {
            items = items.child(marker());
        }
        for i in range.0..range.1 {
            let option = &open.options[i];
            let selected = open.selected == Some(i);
            let ent = ent.clone();
            items = items.child(
                self.option_row(i, option, selected, open.disabled, &t)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        ent.update(cx, |v, cx| v.click_option(i, window, cx));
                    }),
            );
        }
        if range.1 < open.options.len() {
            items = items.child(marker());
        }
        let ent2 = cx.entity();
        let list = div()
            .id("cm-completion-list")
            .min_w(px(220.))
            .max_w(px(
                700.0f32.min(0.95 * f32::from(window.viewport_size().width))
            ))
            .h(px(list_h))
            .overflow_hidden()
            .on_scroll_wheel(move |e: &ScrollWheelEvent, _, cx| {
                cx.stop_propagation();
                let d = f32::from(e.delta.pixel_delta(px(OPTION_HEIGHT)).y);
                ent2.update(cx, |v, cx| {
                    v.tooltips.list_scroll =
                        (v.tooltips.list_scroll - d).clamp(0.0, (content_h - list_h).max(0.0));
                    cx.notify();
                });
            })
            .child(items);
        let body = shadowed(
            &theme,
            card(&theme)
                .p(px(4.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(list),
        );
        let (el, bounds) = self.positioned("completion", anchor, false, body, window, cx)?;
        let mut els = vec![el];
        if let Some(sel) = open.selected
            && let Some(info) = open.options[sel].completion.info.clone()
            && bounds.size.width > px(0.)
        {
            let row_top = f32::from(bounds.origin.y)
                + 5.0
                + top_marker
                + (sel - range.0) as f32 * OPTION_HEIGHT
                - scroll;
            if let Some(info_el) = self.render_info(
                &info,
                bounds,
                (row_top, row_top + OPTION_HEIGHT),
                window,
                cx,
            ) {
                els.push(info_el);
            }
        }
        Some(els)
    }

    fn option_row(
        &self,
        i: usize,
        option: &CompletionOption,
        selected: bool,
        disabled: bool,
        t: &Tokens,
    ) -> gpui_kit::Stateful<Div> {
        let c = &option.completion;
        let label = c.display_label.clone().unwrap_or_else(|| c.label.clone());
        let mut spans = Vec::new();
        let mut off = 0;
        let piece = |from: usize, to: usize, matched: bool| {
            let sp = span(label[from..to].to_string());
            if matched {
                sp.color(h(t.amber)).weight(600.)
            } else {
                sp
            }
        };
        for &(from, to) in &option.matched {
            let (from, to) = (from.min(label.len()), to.min(label.len()));
            if from > off {
                spans.push(piece(off, from, false));
            }
            if to > from.max(off) {
                spans.push(piece(from.max(off), to, true));
            }
            off = to.max(off);
        }
        if off < label.len() {
            spans.push(piece(off, label.len(), false));
        }
        let ink = if selected && disabled {
            t.muted
        } else {
            t.text
        };
        let icon_on = selected && !disabled;
        let row = div()
            .id(SharedString::from(format!("cm-completion-{i}")))
            .flex()
            .items_center()
            .min_h(px(OPTION_HEIGHT))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .when(selected, |d| d.bg(h(t.pane_2)))
            .hover(|s| s.bg(h(t.pane_2)))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .mr(px(8.))
                    .rounded(px(5.))
                    .bg(h(if icon_on { t.amber_d } else { t.pane_2 }))
                    .child(
                        text(icon_letters(c.kind.as_deref()))
                            .mono()
                            .size(9.)
                            .weight(700.)
                            .leading(1.3)
                            .color(h(if icon_on { t.amber } else { t.muted }))
                            .nowrap(),
                    ),
            )
            .child(
                Text::from_spans(spans)
                    .mono()
                    .size(LIST_FONT)
                    .leading(1.3)
                    .color(h(ink))
                    .nowrap(),
            );
        match c.detail.clone() {
            Some(detail) => row.child(
                div().ml_auto().pl(px(10.)).child(
                    text(detail)
                        .ui()
                        .size(10.5)
                        .leading(1.3)
                        .color(h(t.faint))
                        .nowrap(),
                ),
            ),
            None => row,
        }
    }

    fn click_option(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.state.completion.open.clone() else {
            return;
        };
        let Some(option) = open.options.get(i).cloned() else {
            return;
        };
        self.with_dispatcher(window, cx, |d| {
            crate::autocomplete::apply_completion(d, &option);
        });
    }

    /// The info pane beside the list (`defaultPositionInfo`): an LSP
    /// `.cm-lsp-completion-documentation` box in `.cm-completionInfo`.
    fn render_info(
        &mut self,
        info: &crate::autocomplete::Doc,
        list: Bounds<Pixels>,
        option: (f32, f32),
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        let space = self.tooltip_space(window);
        let (ll, lt) = (f32::from(list.origin.x), f32::from(list.origin.y));
        let (lr, lb) = (
            ll + f32::from(list.size.width),
            lt + f32::from(list.size.height),
        );
        let (sl, st) = (f32::from(space.origin.x), f32::from(space.origin.y));
        let (sr, sb) = (
            sl + f32::from(space.size.width),
            st + f32::from(space.size.height),
        );
        let size = self.tooltips.measured("info").unwrap_or_default();
        let (iw, ih) = (f32::from(size.width), f32::from(size.height));
        let (space_left, space_right) = (ll - sl, sr - lr);
        let mut left = false;
        if space_right < iw.min(space_left) {
            left = true;
        }
        let (x, y, max_w) = if iw <= if left { space_left } else { space_right } {
            let off = st.max(option.0.min(sb - ih)) - lt;
            let max_w = INFO_WIDTH.min(if left { space_left } else { space_right });
            (if left { ll - iw } else { lr }, lt + off, max_w)
        } else {
            let max_w = INFO_WIDTH.min(sr - ll - INFO_MARGIN);
            let below = sb - lb;
            let y = if below >= ih || below > lt {
                option.1
            } else {
                option.0 - ih
            };
            (ll + INFO_MARGIN, y, max_w)
        };
        let mut style = DocStyle::from_theme(&theme, 12.5);
        style.pre_line = true;
        let body = shadowed(
            &theme,
            card(&theme).max_w(px(max_w)).py(px(8.)).px(px(10.)).child(
                div()
                    .py(px(10.))
                    .px(px(12.))
                    .child(render_doc(info, &style)),
            ),
        );
        let sizes = self.tooltips.sizes.clone();
        let ent = cx.entity();
        let measure = move |bounds: Vec<Bounds<Pixels>>, _: &mut Window, cx: &mut App| {
            let Some(b) = bounds.first() else { return };
            if sizes.borrow_mut().insert("info", b.size) != Some(b.size) {
                ent.update(cx, |_, cx| cx.notify());
            }
        };
        Some(
            deferred(anchored_at(
                x,
                y,
                div().on_children_prepainted(measure).child(body),
            ))
            .with_priority(2)
            .into_any_element(),
        )
    }

    /// The hover host: LSP hover, then lint diagnostics.
    fn render_hover(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sections: Vec<HoverSection> = [
            self.tooltips.lsp_hover.clone(),
            self.tooltips.lint_hover.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let first = sections.first()?.clone();
        let pos = sections.iter().map(|s| s.pos).min().unwrap_or(first.pos);
        let anchor = self.pos_coords(pos, 1, window)?;
        let theme = Theme::of(cx).clone();
        let t = theme.tokens;
        let base_size = if self.lsp.is_some() {
            13.0
        } else {
            self.metrics.font_size
        };
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let mut host = card(&theme)
            .id("cm-tooltip-hover")
            .flex()
            .flex_col()
            .min_w(px(352.0f32.min(vw - 48.0)))
            .max_w(px(640.0f32.min(vw - 48.0)))
            .max_h(px(352.0f32.min(0.45 * vh)))
            .overflow_y_scroll();
        let (action_bg, action_fg) = crate::theme::diagnostic_action();
        for (i, s) in sections.iter().enumerate() {
            let sep = i > 0 && !self.theme.dark;
            let body: AnyElement = match &s.body {
                HoverBody::Lsp(parts) => {
                    let style = DocStyle::from_theme(&theme, 12.5);
                    let mut d = div().py(px(10.)).px(px(12.)).flex().flex_col();
                    for (k, part) in parts.iter().enumerate() {
                        if k > 0 {
                            d = d.child(div().flex_none().h(px(12.5 * BODY_LEADING)));
                        }
                        d = d.child(match part {
                            HoverPart::Doc(doc) => render_doc(doc, &style),
                            HoverPart::Code { language, value } => {
                                render_code(language, value, &style)
                            }
                        });
                    }
                    d.into_any_element()
                }
                HoverBody::Lint(diags) => {
                    let mut ul = div().flex().flex_col();
                    for d in diags {
                        let mut line = div().flex().flex_wrap().items_baseline().child(
                            text(d.message.clone())
                                .ui()
                                .size(base_size)
                                .leading(BODY_LEADING)
                                .color(h(t.text))
                                .pre_wrap(),
                        );
                        for a in &d.actions {
                            line = line.child(
                                div()
                                    .ml(px(8.))
                                    .px(px(4.))
                                    .py(px(2.))
                                    .rounded(px(3.))
                                    .bg(h(action_bg))
                                    .child(
                                        text(a.name.clone())
                                            .ui()
                                            .size(base_size)
                                            .leading(BODY_LEADING)
                                            .color(h(action_fg))
                                            .nowrap(),
                                    ),
                            );
                        }
                        let mut li = div()
                            .ml(px(-1.))
                            .pt(px(3.))
                            .pb(px(3.))
                            .pr(px(6.))
                            .pl(px(8.))
                            .border_l(px(5.))
                            .border_color(h(self.theme.diagnostic_border(d.severity)))
                            .child(line);
                        if let Some(src) = d.source.clone() {
                            li = li.child(
                                text(src)
                                    .ui()
                                    .size(base_size * 0.7)
                                    .leading(BODY_LEADING)
                                    .color(h(t.text))
                                    .opacity(0.7),
                            );
                        }
                        ul = ul.child(li);
                    }
                    ul.into_any_element()
                }
            };
            host = host.child(
                div()
                    .when(sep, |d| {
                        d.border_t_1()
                            .border_color(h(crate::theme::tooltip_section_border()))
                    })
                    .child(body),
            );
        }
        let ent = cx.entity();
        let host = host.on_hover(move |hovered: &bool, _, cx| {
            let hovered = *hovered;
            ent.update(cx, |v, cx| {
                v.tooltips.pointer_in_hover = hovered;
                if !hovered {
                    v.hover_left_tooltip(cx);
                }
            })
        });
        let body = shadowed(&theme, host);
        self.positioned("hover", anchor, first.above, body, window, cx)
            .map(|(e, _)| e)
    }

    /// The signature help tooltip (`drawSignatureTooltip`).
    /// simplify: `.cm-lsp-signature`'s `text-indent: 1em hanging` is not
    /// applied (recur-ui text has no text-indent); wrapped signature lines
    /// start flush.
    fn render_signature(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sig = self.lsp.as_ref()?.signature.clone()?;
        let view = sig.view()?;
        let anchor = self.pos_coords(sig.pos, 1, window)?;
        let theme = Theme::of(cx).clone();
        let t = theme.tokens;
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let label = view.label.clone();
        let spans = match view.active_param {
            Some((f, to)) => vec![
                span(label[..f].to_string()),
                span(label[f..to].to_string())
                    .color(h(t.amber))
                    .weight(700.),
                span(label[to..].to_string()),
            ],
            None => vec![span(label)],
        };
        let style = DocStyle::from_theme(&theme, 10.0);
        let mut body = card(&theme)
            .id("cm-lsp-signature")
            .relative()
            .py(px(10.))
            .px(px(12.))
            .max_w(px(576.0f32.min(0.7 * vw)))
            .max_h(px(224.0f32.min(0.4 * vh)))
            .overflow_y_scroll();
        if let Some(counter) = view.counter.clone() {
            body = body.child(
                div().absolute().left(px(2.)).top(px(4.)).child(
                    text(counter)
                        .family(crate::vim::prompt::generic_monospace())
                        .size(12.5 * 0.7)
                        .leading(1.3)
                        .color(h(t.text))
                        .nowrap(),
                ),
            );
        }
        body = body.child(
            Text::from_spans(spans)
                .mono()
                .size(12.5)
                .leading(BODY_LEADING)
                .color(h(t.text)),
        );
        if let Some(doc) = view.doc.as_ref() {
            body = body.child(render_doc(doc, &style));
        }
        let body = shadowed(&theme, body);
        self.positioned("signature", anchor, true, body, window, cx)
            .map(|(e, _)| e)
    }

    /// The pointer left the hover tooltip: hide unless it went back over
    /// the hovered range.
    fn hover_left_tooltip(&mut self, cx: &mut Context<Self>) {
        let p = self.last_hover_point;
        if p.is_none() {
            self.tooltips.hide_hover();
            cx.notify();
        }
    }
}

/// Visible for tests: `place` keeps tooltips inside the space and flips.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_flips_and_clamps() {
        let space = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
        let sz = size(px(200.), px(100.));
        let below = place((450.0, 50.0, 64.0), sz, space, false, 18.0).unwrap();
        assert_eq!((below.0, below.1, below.2), (300.0, 64.0, false));
        let flipped = place((10.0, 250.0, 264.0), sz, space, false, 18.0).unwrap();
        assert_eq!((flipped.1, flipped.2), (150.0, true));
        let squeezed = place(
            (10.0, 60.0, 74.0),
            size(px(200.), px(400.)),
            space,
            false,
            18.0,
        )
        .unwrap();
        assert_eq!(squeezed.3, Some(226.0));
    }
}
