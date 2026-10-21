//! Editor tooltips: the completion list and its info pane
//! (`@codemirror/autocomplete` restyled by `_codemirror.scss`), the hover
//! host (LSP hover + lint diagnostics) and LSP signature help, positioned by
//! CodeMirror's tooltip placement (`writeMeasure`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Bounds, BoxShadow, Context, Div, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Rgba, ScrollWheelEvent, SharedString, Size,
    StatefulInteractiveElement, Styled, StyledText, TextRun, Window, anchored, deferred, div,
    point, px, size,
};

use crate::autocomplete::{CompletionDialog, CompletionOption, range_around_selected};
use crate::lint::Diagnostic;
use crate::lsp::{HoverInfo, HoverPart};
use crate::markdown::{DocStyle, render_code, render_doc};
use crate::state::{EditorState, Transaction};
use crate::tokens::{AppTokens, mono_font, ui_font, with_font};
use crate::view::EditorView;

/// `maxRenderedOptions`.
const MAX_RENDERED_OPTIONS: usize = 100;
/// `li` box height: `min-height: 26px`.
const OPTION_HEIGHT: f32 = 26.0;
/// `Info.Width` / `Info.Margin`.
const INFO_WIDTH: f32 = 400.0;
const INFO_MARGIN: f32 = 30.0;

fn h(c: Rgba) -> Hsla {
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

fn card(t: &AppTokens) -> Div {
    div()
        .bg(h(t.pane))
        .text_color(h(t.text))
        .border_1()
        .border_color(h(t.line_bright))
        .rounded(px(8.))
        .shadow(vec![BoxShadow {
            color: gpui_kit::hsla(0., 0., 0., if t.light { 0.12 } else { 0.45 }),
            offset: point(px(0.), px(12.)),
            blur_radius: px(if t.light { 28. } else { 32. }),
            spread_radius: px(0.),
            inset: false,
        }])
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
            let el = deferred(
                anchored()
                    .position(point(px(-10000.), px(-10000.)))
                    .child(div().on_children_prepainted(measure).child(body)),
            );
            return Some((el.into_any_element(), Bounds::default()));
        };
        let prev_above = self.tooltips.above.get(key).copied().unwrap_or(above_pref);
        let (left, top, above, clamp) = place(anchor, sz, space, prev_above, lh)?;
        self.tooltips.above.insert(key, above);
        let wrapper = div()
            .on_children_prepainted(measure)
            .when_some(clamp, |d, c| d.max_h(px(c)))
            .child(body);
        let el =
            deferred(anchored().position(point(px(left), px(top))).child(wrapper)).with_priority(1);
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
        let mut placed: Vec<Bounds<Pixels>> = Vec::new();
        if let Some(open) = self.state.completion.open.clone()
            && focused
            && let Some((el, b)) = self.render_completion(&open, window, cx)
        {
            placed.push(b);
            out.extend(el);
        }
        if let Some(el) = self.render_hover(window, cx) {
            out.push(el);
        }
        if let Some(el) = self.render_signature(window, cx) {
            out.push(el);
        }
        let _ = placed;
        out
    }

    fn render_completion(
        &mut self,
        open: &Rc<CompletionDialog>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(Vec<AnyElement>, Bounds<Pixels>)> {
        let t = self.theme.tokens;
        let anchor = self.pos_coords(open.pos, 1, window)?;
        let range = range_around_selected(open.options.len(), open.selected, MAX_RENDERED_OPTIONS);
        let vh = f32::from(window.viewport_size().height);
        let list_max = (16.0 * 12.5f32).min(0.4 * vh);
        let marker_h = 4.0 + 4.0 + 12.5 * 1.3;
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
        let mono = mono_font();
        let marker = |t: &AppTokens| {
            div()
                .w_full()
                .flex()
                .justify_center()
                .py(px(4.))
                .text_color(h(t.faint))
                .child("···")
        };
        let mut items = div().flex().flex_col().relative().top(px(-scroll));
        if range.0 > 0 {
            items = items.child(marker(&t));
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
            items = items.child(marker(&t));
        }
        let ent2 = cx.entity();
        let list = with_font(div(), &mono)
            .id("cm-completion-list")
            .min_w(px(220.))
            .max_w(px(
                700.0f32.min(0.95 * f32::from(window.viewport_size().width))
            ))
            .h(px(list_h))
            .overflow_hidden()
            .text_size(px(12.5))
            .whitespace_nowrap()
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
        let body = card(&t)
            .p(px(4.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(list);
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
        Some((els, bounds))
    }

    fn option_row(
        &self,
        _i: usize,
        option: &CompletionOption,
        selected: bool,
        disabled: bool,
        t: &AppTokens,
    ) -> gpui_kit::Stateful<Div> {
        let c = &option.completion;
        let label = c.display_label.clone().unwrap_or_else(|| c.label.clone());
        let mono = mono_font();
        let mut runs = Vec::new();
        let mut off = 0;
        let base_color = if selected && disabled {
            h(t.muted)
        } else {
            h(t.text)
        };
        let run = |len: usize, matched: bool| {
            let mut font = mono.clone();
            if matched {
                font.weight = FontWeight::SEMIBOLD;
            }
            TextRun {
                len,
                font,
                color: if matched { h(t.amber) } else { base_color },
                background_color: None,
                underline: None,
                strikethrough: None,
            }
        };
        for &(from, to) in &option.matched {
            let (from, to) = (from.min(label.len()), to.min(label.len()));
            if from > off {
                runs.push(run(from - off, false));
            }
            if to > from {
                runs.push(run(to - from, true));
            }
            off = to.max(off);
        }
        if off < label.len() {
            runs.push(run(label.len() - off, false));
        }
        let pressed = selected;
        let row = div()
            .id(SharedString::from(format!("cm-completion-{}", _i)))
            .flex()
            .items_center()
            .min_h(px(OPTION_HEIGHT))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .line_height(gpui_kit::relative(1.3))
            .when(pressed, |d| d.bg(h(t.pane_2)))
            .hover(|s| s.bg(h(t.pane_2)))
            .child(
                with_font(div(), &mono)
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .mr(px(8.))
                    .rounded(px(5.))
                    .bg(h(if pressed && !disabled {
                        t.amber_d
                    } else {
                        t.pane_2
                    }))
                    .text_color(h(if pressed && !disabled {
                        t.amber
                    } else {
                        t.muted
                    }))
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .child(icon_letters(c.kind.as_deref())),
            )
            .child(StyledText::new(SharedString::from(label)).with_runs(runs));
        match c.detail.clone() {
            Some(detail) => row.child(
                with_font(div(), &ui_font())
                    .ml_auto()
                    .pl(px(10.))
                    .text_size(px(10.5))
                    .text_color(h(t.faint))
                    .child(detail),
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

    /// The info pane beside the list (`defaultPositionInfo`).
    fn render_info(
        &mut self,
        info: &crate::autocomplete::Doc,
        list: Bounds<Pixels>,
        option: (f32, f32),
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = self.theme.tokens;
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
        let style = DocStyle::from_tokens(&t, 12.0);
        let body = card(&t)
            .max_w(px(max_w))
            .py(px(8.))
            .px(px(10.))
            .text_color(h(t.muted))
            .text_size(px(12.))
            .child(with_font(div(), &ui_font()).child(render_doc(info, &style)));
        let sizes = self.tooltips.sizes.clone();
        let ent = cx.entity();
        let measure = move |bounds: Vec<Bounds<Pixels>>, _: &mut Window, cx: &mut App| {
            let Some(b) = bounds.first() else { return };
            if sizes.borrow_mut().insert("info", b.size) != Some(b.size) {
                ent.update(cx, |_, cx| cx.notify());
            }
        };
        Some(
            deferred(
                anchored()
                    .position(point(px(x), px(y)))
                    .child(div().on_children_prepainted(measure).child(body)),
            )
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
        let t = self.theme.tokens;
        let base_size = if self.lsp.is_some() {
            13.0
        } else {
            self.metrics.font_size
        };
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let mut host = with_font(card(&t), &ui_font())
            .id("cm-tooltip-hover")
            .flex()
            .flex_col()
            .text_size(px(base_size))
            .line_height(gpui_kit::relative(1.45))
            .min_w(px(352.0f32.min(vw - 48.0)))
            .max_w(px(640.0f32.min(vw - 48.0)))
            .max_h(px(352.0f32.min(0.45 * vh)))
            .overflow_y_scroll();
        for (i, s) in sections.iter().enumerate() {
            let sep = i > 0 && !self.theme.dark;
            let body: AnyElement = match &s.body {
                HoverBody::Lsp(parts) => {
                    let style = DocStyle::from_tokens(&t, 12.5);
                    let mut d = with_font(div(), &ui_font())
                        .py(px(10.))
                        .px(px(12.))
                        .text_size(px(12.5))
                        .line_height(gpui_kit::relative(1.45))
                        .text_color(h(t.text));
                    for (k, part) in parts.iter().enumerate() {
                        if k > 0 {
                            d = d.child(div().h(px(12.5 * 1.45)));
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
                        let mut li = div()
                            .ml(px(-1.))
                            .pt(px(3.))
                            .pb(px(3.))
                            .pr(px(6.))
                            .pl(px(8.))
                            .border_l(px(5.))
                            .border_color(h(self.theme.diagnostic_border(d.severity)))
                            .whitespace_normal()
                            .child(d.message.clone());
                        for a in &d.actions {
                            li = li.child(
                                div()
                                    .ml(px(8.))
                                    .px(px(4.))
                                    .py(px(2.))
                                    .rounded(px(3.))
                                    .bg(gpui_kit::rgb(0x444444))
                                    .text_color(gpui_kit::white())
                                    .child(a.name.clone()),
                            );
                        }
                        if let Some(src) = d.source.clone() {
                            li = li.child(
                                div().text_size(px(base_size * 0.7)).opacity(0.7).child(src),
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
                        d.border_t_1().border_color(gpui_kit::rgb(0xbbbbbb))
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
        let body = div().child(host);
        self.positioned("hover", anchor, first.above, body, window, cx)
            .map(|(e, _)| e)
    }

    /// The signature help tooltip (`drawSignatureTooltip`).
    fn render_signature(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sig = self.lsp.as_ref()?.signature.clone()?;
        let view = sig.view()?;
        let anchor = self.pos_coords(sig.pos, 1, window)?;
        let t = self.theme.tokens;
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let mono = mono_font();
        let mut runs = Vec::new();
        let plain = |len: usize| TextRun {
            len,
            font: mono.clone(),
            color: h(t.text),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        match view.active_param {
            Some((f, to)) => {
                if f > 0 {
                    runs.push(plain(f));
                }
                let mut font = mono.clone();
                font.weight = FontWeight::BOLD;
                runs.push(TextRun {
                    len: to - f,
                    font,
                    color: h(t.amber),
                    ..plain(0)
                });
                if to < view.label.len() {
                    runs.push(plain(view.label.len() - to));
                }
            }
            None => runs.push(plain(view.label.len())),
        }
        let style = DocStyle::from_tokens(&t, 10.0);
        let mut body = with_font(card(&t), &ui_font())
            .id("cm-lsp-signature")
            .relative()
            .py(px(10.))
            .px(px(12.))
            .text_size(px(12.5))
            .line_height(gpui_kit::relative(1.45))
            .max_w(px(576.0f32.min(0.7 * vw)))
            .max_h(px(224.0f32.min(0.4 * vh)))
            .overflow_y_scroll();
        if let Some(counter) = view.counter.clone() {
            body = body.child(
                with_font(div(), &mono)
                    .absolute()
                    .left(px(2.))
                    .top(px(4.))
                    .text_size(px(12.5 * 0.7))
                    .line_height(gpui_kit::relative(1.3))
                    .child(counter),
            );
        }
        body = body.child(
            with_font(div(), &mono)
                .child(StyledText::new(SharedString::from(view.label.clone())).with_runs(runs)),
        );
        if let Some(doc) = view.doc.as_ref() {
            body = body.child(div().text_size(px(10.)).child(render_doc(doc, &style)));
        }
        self.positioned("signature", anchor, true, div().child(body), window, cx)
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
