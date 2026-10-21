//! The editor's GPUI element: lays out the visible lines and paints them
//! in CodeMirror's stacking order (scroller fill, selection layer, line
//! backgrounds, mark backgrounds, text, cursors, gutters, scrollbars).

use gpui_kit::{
    App, BorderStyle, Bounds, Corners, CursorStyle, DispatchPhase, Edges, Element, ElementId,
    ElementInputHandler, Entity, FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior,
    Hsla, InspectorElementId, IntoElement, LayoutId, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Path, PathBuilder, Pixels, Point, Rgba, ScrollWheelEvent, ShapedLine, SharedString,
    StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, point, px, quad,
    relative, size,
};

use crate::display::TextMeasure;
use crate::display::{BlockWidget, MeasuredLine, SegKind};
use crate::icons::{IconDef, lint_icon};
use crate::merge::{COLLAPSE_MIN_SIZE, Chunk, chunk_decorations, deleted_chunk_lines};
use crate::prefs::EditorWhitespace;
use crate::tokens::SCROLLBAR_SIZE;
use crate::view::{EditorKind, EditorView, Frame, GpuiMeasure};
use crate::view_queries::ViewQueries;

/// A gutter column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GutterKind {
    Findings,
    Lint,
    LineNumbers,
    Fold,
    Diff,
    MergeChange,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GutterColumn {
    pub kind: GutterKind,
    /// Left edge relative to the element.
    pub x: f32,
    pub width: f32,
}

/// The collapsed-region widget's height (padding 5px, 10.5px × 1.4 text,
/// 1px borders).
pub const COLLAPSED_HEIGHT: f32 = 5.0 + 10.5 * 1.4 + 5.0 + 2.0;
/// `.cm-deletedChunk` left padding.
pub const DELETED_CHUNK_PADDING: f32 = 6.0;

fn h(c: Rgba) -> Hsla {
    c.into()
}

/// Display rows of one deleted-chunk line: its display text (tabs
/// expanded) and the wrap row starts.
fn deleted_line_rows(
    view: &EditorView,
    text: &str,
    measure: &dyn TextMeasure,
) -> (crate::display::DisplayLine, Vec<f32>, Vec<usize>) {
    let l = view.layout.borrow();
    let display =
        crate::display::display_line(text, l.config.tab_size, l.config.special_chars, false);
    let xs = measure.char_xs(&display.text);
    // `.cm-deletedLine` spans the content box less the chunk's 6px padding.
    let width =
        l.config.wrap_width + view.metrics.line_padding_left + view.metrics.line_padding_right
            - DELETED_CHUNK_PADDING;
    let rows = if l.config.wrap {
        crate::display::wrap_rows(&display.text, &xs, width)
    } else {
        vec![0]
    };
    (display, xs, rows)
}

/// Block widgets for the unified merge view.
pub(crate) fn merge_widgets(view: &EditorView, measure: &dyn TextMeasure) -> Vec<BlockWidget> {
    let Some(merge) = view.state.merge.as_ref() else {
        return Vec::new();
    };
    let doc = &view.state.doc;
    let lh = view.layout.borrow().config.line_height;
    let mut out = Vec::new();
    for (i, chunk) in merge.chunks.iter().enumerate() {
        let rows: usize = deleted_chunk_lines(&merge.original, chunk)
            .iter()
            .map(|spans| {
                let text: String = spans.iter().map(|s| s.text.as_str()).collect();
                deleted_line_rows(view, &text, measure).2.len()
            })
            .sum();
        out.push(BlockWidget {
            line: doc.line_at(chunk.from_b.min(doc.len())).number,
            height: rows as f32 * lh,
            replace_to: None,
            id: i,
        });
    }
    for (i, c) in merge.collapsed.iter().enumerate() {
        if c.lines < COLLAPSE_MIN_SIZE {
            continue;
        }
        out.push(BlockWidget {
            line: doc.line_at(c.from).number,
            height: COLLAPSED_HEIGHT,
            replace_to: Some(doc.line_at(c.to).number),
            id: 1_000_000 + i,
        });
    }
    out
}

/// Text with CSS `letter-spacing` (spacing after every character): glyphs
/// shaped one by one. Returns the ops and the advance.
#[allow(clippy::too_many_arguments)]
fn spaced_text(
    text: &str,
    font: &gpui_kit::Font,
    size: f32,
    color: Hsla,
    spacing: f32,
    origin: (f32, f32),
    line_height: f32,
    window: &Window,
) -> (Vec<Op>, f32) {
    let mut ops = Vec::new();
    let mut x = origin.0;
    for ch in text.chars() {
        let s = ch.to_string();
        let shaped = window.text_system().shape_line(
            SharedString::from(s.clone()),
            px(size),
            &[TextRun {
                len: s.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let w = f32::from(shaped.width);
        ops.push(Op::Text {
            line: Box::new(shaped),
            origin: point(px(x), px(origin.1)),
            line_height: px(line_height),
        });
        x += w + spacing;
    }
    (ops, x - origin.0)
}

/// One painted primitive.
pub enum Op {
    Quad(PaintQuad),
    Text {
        line: Box<ShapedLine>,
        origin: Point<Pixels>,
        line_height: Pixels,
    },
    Path(Path<Pixels>, Hsla),
    Icon(IconDef, Bounds<Pixels>, Hsla),
}

/// Everything prepaint computed for paint.
#[derive(Default)]
pub struct Plan {
    pub scroller: Vec<Op>,
    pub selection: Vec<Op>,
    pub line_bgs: Vec<Op>,
    pub mark_bgs: Vec<Op>,
    pub text: Vec<Op>,
    pub overlay: Vec<Op>,
    pub chip: Vec<Op>,
    pub gutter: Vec<Op>,
    pub scrollbars: Vec<Op>,
    pub content_bounds: Option<Bounds<Pixels>>,
    pub gutter_bounds: Option<Bounds<Pixels>>,
    pub hitbox: Option<Hitbox>,
    pub crosshair: bool,
}

pub struct EditorElement {
    pub view: Entity<EditorView>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn rect(x: f32, y: f32, w: f32, hgt: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w.max(0.0)), px(hgt.max(0.0))))
}

/// Font metrics for the text content box (`coordsAtPos` heights).
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextBox {
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
}

impl TextBox {
    /// Top of the font content area within a row.
    pub fn top(&self) -> f32 {
        (self.line_height - (self.ascent + self.descent)) / 2.0
    }

    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }
}

/// A styled run of display text before shaping.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StyleRun {
    len: usize,
    color: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

fn run_of(base: &gpui_kit::Font, s: &StyleRun) -> TextRun {
    let mut font = base.clone();
    if s.bold {
        font.weight = FontWeight::BOLD;
    }
    if s.italic {
        font.style = FontStyle::Italic;
    }
    TextRun {
        len: s.len,
        font,
        color: s.color,
        background_color: None,
        underline: s.underline.then_some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(s.color),
            wavy: false,
        }),
        strikethrough: s.strike.then_some(StrikethroughStyle {
            thickness: px(1.0),
            color: Some(s.color),
        }),
    }
}

/// Per-display-byte styles for a line: base color, TextMate tokens,
/// special chars and the fold placeholder.
fn line_styles(view: &EditorView, n: usize, m: &MeasuredLine, line_from: usize) -> Vec<StyleRun> {
    let theme = &view.theme;
    let base = h(theme.foreground);
    let len = m.display.text.len();
    let mut styles: Vec<(Hsla, bool, bool, bool, bool)> =
        vec![(base, false, false, false, false); len];
    if let (Some(hl), Some(tm)) = (view.highlighter.as_ref(), view.tm.as_ref()) {
        let map = tm.borrow().catalog.color_map().to_vec();
        for span in hl.spans(n) {
            let (from, to) = (
                m.display.disp_of(span.from - line_from, 1),
                m.display.disp_of(span.to - line_from, -1),
            );
            let fg = span
                .paint
                .foreground
                .and_then(|i| map.get(i as usize))
                .and_then(|c| crate::color::parse_css_color(c))
                .map(h);
            let fs = span.paint.font_style;
            for s in styles.iter_mut().take(to.min(len)).skip(from) {
                if let Some(c) = fg {
                    s.0 = c;
                }
                s.1 = fs & recur_textmate::tm_font::BOLD != 0;
                s.2 = fs & recur_textmate::tm_font::ITALIC != 0;
                s.3 = fs & recur_textmate::tm_font::UNDERLINE != 0;
                s.4 = fs & recur_textmate::tm_font::STRIKETHROUGH != 0;
            }
        }
    }
    for seg in &m.display.segs {
        let color = match seg.kind {
            SegKind::Special(_) => Some(h(theme.special_char)),
            SegKind::FoldPlaceholder => Some(h(theme.fold_placeholder_fg)),
            _ => None,
        };
        if let Some(c) = color {
            for s in styles.iter_mut().take(seg.disp_to).skip(seg.disp_from) {
                *s = (c, false, false, false, false);
            }
        }
    }
    let mut runs: Vec<StyleRun> = Vec::new();
    let text = &m.display.text;
    for (i, ch) in text.char_indices() {
        let st = styles[i];
        let l = ch.len_utf8();
        match runs.last_mut() {
            Some(r) if (r.color, r.bold, r.italic, r.underline, r.strike) == st => r.len += l,
            _ => runs.push(StyleRun {
                len: l,
                color: st.0,
                bold: st.1,
                italic: st.2,
                underline: st.3,
                strike: st.4,
            }),
        }
    }
    runs
}

/// Slice runs to a row's display range.
fn runs_for(runs: &[StyleRun], from: usize, to: usize, font: &gpui_kit::Font) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut pos = 0;
    for r in runs {
        let (a, b) = (pos, pos + r.len);
        pos = b;
        let (s, e) = (a.max(from), b.min(to));
        if s < e {
            out.push(run_of(font, &StyleRun { len: e - s, ..*r }));
        }
    }
    out
}

/// `@codemirror/lint`'s underline: a 6×3 tile (`m0 2.5 l2 -1.5 l1 0 l2 1.5
/// l1 0`, stroke .7) repeated from `x0` and clipped at `x1`, its box
/// bottom at `bottom`.
fn zigzag(x0: f32, x1: f32, bottom: f32, color: Hsla) -> Option<Op> {
    if x1 <= x0 {
        return None;
    }
    const TILE: [(f32, f32); 5] = [(0.0, 2.5), (2.0, 1.0), (3.0, 1.0), (5.0, 2.5), (6.0, 2.5)];
    let top = bottom - 3.0;
    let mut b = PathBuilder::stroke(px(0.7));
    b.move_to(point(px(x0), px(top + TILE[0].1)));
    let mut base = x0;
    'tiles: while base < x1 {
        for w in TILE.windows(2) {
            let ((ax, ay), (bx, by)) = (w[0], w[1]);
            if base + bx >= x1 {
                let t = (x1 - base - ax) / (bx - ax);
                b.line_to(point(px(x1), px(top + ay + (by - ay) * t)));
                break 'tiles;
            }
            b.line_to(point(px(base + bx), px(top + by)));
        }
        base += 6.0;
    }
    b.build().ok().map(|p| Op::Path(p, color))
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Plan;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        style.flex_grow = 1.0;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Plan {
        self.view.update(cx, |view, cx| {
            let mut plan = view.plan(bounds, window, cx);
            plan.hitbox = Some(window.insert_hitbox(bounds, HitboxBehavior::Normal));
            plan
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        plan: &mut Plan,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        if let Some(hitbox) = plan.hitbox.as_ref() {
            let over_gutter = plan
                .gutter_bounds
                .is_some_and(|g| g.contains(&window.mouse_position()));
            let style = if plan.crosshair {
                CursorStyle::Crosshair
            } else if over_gutter {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            };
            window.set_cursor_style(style, hitbox);
        }
        let paint_ops = |ops: &mut Vec<Op>, window: &mut Window, cx: &mut App| {
            for op in ops.drain(..) {
                match op {
                    Op::Quad(q) => window.paint_quad(q),
                    Op::Text {
                        line,
                        origin,
                        line_height,
                    } => {
                        let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
                    }
                    Op::Path(p, c) => window.paint_path(p, c),
                    Op::Icon(icon, b, c) => icon.paint(b, c, window, cx),
                }
            }
        };
        window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
            paint_ops(&mut plan.scroller, window, cx);
            let content = plan.content_bounds.unwrap_or(bounds);
            window.with_content_mask(Some(gpui_kit::ContentMask { bounds: content }), |window| {
                paint_ops(&mut plan.selection, window, cx);
                paint_ops(&mut plan.line_bgs, window, cx);
                paint_ops(&mut plan.mark_bgs, window, cx);
                paint_ops(&mut plan.text, window, cx);
                paint_ops(&mut plan.overlay, window, cx);
                paint_ops(&mut plan.chip, window, cx);
            });
            paint_ops(&mut plan.gutter, window, cx);
            paint_ops(&mut plan.scrollbars, window, cx);
        });
        let view = self.view.clone();
        window.on_mouse_event({
            let view = view.clone();
            move |e: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && bounds.contains(&e.position) {
                    view.update(cx, |v, cx| v.mouse_down(e, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |e: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    view.update(cx, |v, cx| v.mouse_move(e, bounds, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |e: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    view.update(cx, |v, cx| v.mouse_up(e, window, cx));
                }
            }
        });
        window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && bounds.contains(&e.position) {
                view.update(cx, |v, cx| v.scroll_wheel(e, window, cx));
            }
        });
    }
}

impl EditorView {
    pub(crate) fn text_box(&self, window: &Window) -> TextBox {
        let ts = window.text_system();
        let id = ts.resolve_font(&self.font);
        let size = px(self.metrics.font_size);
        TextBox {
            ascent: f32::from(ts.ascent(id, size)),
            descent: f32::from(ts.descent(id, size)),
            line_height: self.metrics.line_height(),
        }
    }

    fn text_width(&self, text: &str, window: &Window) -> f32 {
        let m = GpuiMeasure {
            ts: window.text_system(),
            font: self.font.clone(),
            size: px(self.metrics.font_size),
        };
        crate::display::TextMeasure::char_xs(&m, text)
            .last()
            .copied()
            .unwrap_or(0.0)
    }

    /// The gutter columns for this editor, left to right.
    pub(crate) fn gutter_columns(&self, window: &Window) -> Vec<GutterColumn> {
        let mut kinds: Vec<(GutterKind, f32)> = Vec::new();
        let marker_w = |visible: bool| if visible { 12.0 + 1.0 + 4.0 } else { 0.0 };
        match self.kind {
            EditorKind::Code => {
                kinds.push((
                    GutterKind::Findings,
                    marker_w(self.state.gutters.findings_visible()),
                ));
                kinds.push((GutterKind::Lint, marker_w(self.state.lint.gutter_visible())));
            }
            EditorKind::Merge { .. } => {}
        }
        if self.prefs.line_numbers {
            let mut max = 9usize;
            while max < self.state.doc.lines() {
                max = max * 10 + 9;
            }
            let digits = max.to_string();
            let w = self.text_width(&digits, window);
            kinds.push((GutterKind::LineNumbers, (w + 8.0).max(20.0)));
        }
        if self.prefs.fold_gutter {
            kinds.push((GutterKind::Fold, self.text_width("\u{203a}", window) + 2.0));
        }
        match self.kind {
            EditorKind::Code if self.prefs.diff_gutter => kinds.push((GutterKind::Diff, 3.0)),
            EditorKind::Merge { .. } => kinds.push((GutterKind::MergeChange, 3.0)),
            _ => {}
        }
        let mut x = 0.0;
        kinds
            .into_iter()
            .map(|(kind, width)| {
                let c = GutterColumn { kind, x, width };
                x += width;
                c
            })
            .collect()
    }

    /// Lay out and build the paint plan for `bounds`.
    pub(crate) fn plan(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Plan {
        let mut plan = Plan::default();
        let theme = self.theme.clone();
        let tb = self.text_box(window);
        let lh = tb.line_height;
        let (bx, by) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let gutters = self.gutter_columns(window);
        let gutters_w: f32 = gutters.iter().map(|g| g.width).sum();
        let gutter_width = if gutters.is_empty() {
            0.0
        } else {
            gutters_w + 1.0
        };

        // Layout config follows the box; wrap width changes re-estimate.
        let content_height_guess = self.layout.borrow().content_height() as f32;
        let vbar = content_height_guess > bh;
        let scroller_width = bw - if vbar { SCROLLBAR_SIZE } else { 0.0 };
        let new_frame_dims = (scroller_width, bh, gutter_width);
        let old_dims = self
            .frame
            .as_ref()
            .map(|f| (f.scroller_width, f.scroller_height, f.gutter_width));
        if old_dims != Some(new_frame_dims) {
            let mut f = self.frame.clone().unwrap_or_default();
            f.scroller_width = scroller_width;
            f.scroller_height = bh;
            f.gutter_width = gutter_width;
            self.frame = Some(f);
            let config = self.layout_config(window);
            let rebuild = self.layout.borrow().config != config;
            if rebuild {
                self.rebuild_layout(window);
            }
        }
        let past_end = if self.prefs.scroll_past_end {
            (bh - lh - self.metrics.content_padding_y).max(0.0)
        } else {
            0.0
        };
        let content_h = self.layout.borrow().content_height() as f32 + past_end;
        let text_area_w = scroller_width - gutter_width;
        let max_line_w = self.layout.borrow().max_measured_width();
        let content_w = if self.prefs.line_wrapping {
            text_area_w
        } else {
            (max_line_w + self.metrics.line_padding_left + self.metrics.line_padding_right)
                .max(text_area_w)
        };
        let hbar = !self.prefs.line_wrapping && content_w > text_area_w;
        let view_h = bh - if hbar { SCROLLBAR_SIZE } else { 0.0 };
        {
            let f = self.frame.get_or_insert_with(Frame::default);
            f.bounds = bounds;
            f.scroller_top = by;
            f.scroller_height = view_h;
            f.max_scroll_top = (content_h - view_h).max(0.0);
            f.max_scroll_left = (content_w - text_area_w).max(0.0);
            f.content_width = content_w;
            f.gutters = gutters.clone();
        }
        self.resolve_pending_scroll(window);
        self.scroll_top = self.scroll_top.clamp(0.0, (content_h - view_h).max(0.0));
        self.scroll_left = self
            .scroll_left
            .clamp(0.0, (content_w - text_area_w).max(0.0));

        // Measure the visible lines, anchoring scroll on height changes
        // above the viewport.
        let measure = GpuiMeasure {
            ts: window.text_system(),
            font: self.font.clone(),
            size: px(self.metrics.font_size),
        };
        let (first, last) = {
            let mut tries = 0;
            loop {
                let (first, last) = {
                    let l = self.layout.borrow();
                    (
                        l.line_at_y(self.scroll_top as f64),
                        l.line_at_y((self.scroll_top + view_h) as f64),
                    )
                };
                let mut changed_above = 0.0;
                let mut changed = false;
                for n in first..=last {
                    let hidden = self.layout.borrow().is_hidden(n);
                    if hidden {
                        continue;
                    }
                    let already = self.layout.borrow().measured(n).is_some();
                    if already {
                        continue;
                    }
                    let top_before = self.layout.borrow().line_top(n) as f32;
                    let d = self.layout.borrow_mut().measure(&self.state, n, &measure) as f32;
                    if d != 0.0 {
                        changed = true;
                        if top_before < self.scroll_top {
                            changed_above += d;
                        }
                    }
                }
                self.scroll_top += changed_above;
                tries += 1;
                if !changed || tries > 4 {
                    break (first, last);
                }
            }
        };
        let rendered = {
            let l = self.layout.borrow();
            (
                l.line_at_y((self.scroll_top - 1000.0).max(0.0) as f64),
                l.line_at_y((self.scroll_top + view_h + 1000.0) as f64),
            )
        };
        let prev_rendered = self.frame.as_ref().map(|f| f.rendered_lines);
        if let Some(f) = self.frame.as_mut() {
            f.visible_lines = (first, last);
            f.rendered_lines = rendered;
            f.content_left = bx + gutter_width;
        }
        if prev_rendered != Some(rendered) {
            let win = self.highlight_window(window);
            let state = self.state.clone();
            if let Some(hl) = self.highlighter.as_mut() {
                let needs = hl.viewport_changed(&state, win);
                self.handle_highlight_needs(needs, window, cx);
            }
        }

        let content_left = bx + gutter_width - self.scroll_left;
        let text_left = content_left + self.metrics.line_padding_left;
        let doc_top = by - self.scroll_top;
        let content_bounds = rect(bx + gutter_width, by, text_area_w, view_h);
        plan.content_bounds = Some(content_bounds);
        let focused = self.focus.is_focused(window);
        let scope_bg = theme.content_background(&self.scope);
        plan.scroller
            .push(Op::Quad(fill(rect(bx, by, bw, bh), h(theme.background))));
        plan.scroller
            .push(Op::Quad(fill(rect(bx, by, bw, bh), h(scope_bg))));

        let doc = self.state.doc.clone();
        let lines_total = doc.lines();
        let row_box = |n: usize, row: usize, l: &crate::display::Layout| -> f32 {
            doc_top + l.line_top(n) as f32 + row as f32 * lh
        };

        // Selection layer (drawSelection).
        let sel_color = h(theme.selection_for(self.lit));
        let left_side = content_left + self.metrics.line_padding_left;
        let right_side = content_left + content_w - self.metrics.line_padding_right;
        for range in self.state.selection.ranges.clone() {
            if range.is_empty() {
                continue;
            }
            for r in self.selection_rects(
                range.from, range.to, left_side, right_side, doc_top, &tb, window,
            ) {
                plan.selection.push(Op::Quad(fill(r, sel_color)));
            }
        }

        let l = self.layout.borrow();
        // Active line(s): line blocks holding a cursor head.
        let mut active_lines: Vec<usize> = Vec::new();
        for r in &self.state.selection.ranges {
            let n = l.block_start_line(doc.line_at(r.head()).number);
            if !active_lines.contains(&n) {
                active_lines.push(n);
            }
        }
        let merge_colors = theme.merge();
        let merge_decos: Vec<(Chunk, crate::merge::ChunkDecorations)> = self
            .state
            .merge
            .as_ref()
            .map(|m| {
                m.chunks
                    .iter()
                    .map(|c| (c.clone(), chunk_decorations(&doc, c)))
                    .collect()
            })
            .unwrap_or_default();

        let full_line = |from: usize| merge_decos.iter().any(|(_, d)| d.full_line.contains(&from));
        let changed_line_set: std::collections::HashSet<usize> = merge_decos
            .iter()
            .flat_map(|(_, d)| d.changed_lines.iter().copied())
            .collect();

        let search_hl = crate::search::search_highlights(
            &self.state,
            &[(
                doc.line(rendered.0).from,
                doc.line(rendered.1.min(lines_total)).to,
            )],
        );
        let sel_matches = crate::search::selection_match_highlights(
            &self.state,
            &[(doc.line(first).from, doc.line(last.min(lines_total)).to)],
        );
        let brackets = if self.prefs.bracket_matching && focused {
            crate::brackets::bracket_highlights(&self.state)
        } else {
            Vec::new()
        };

        let mut n = first;
        while n <= last.min(lines_total) {
            for w in l
                .widgets()
                .iter()
                .filter(|w| w.line == n && w.replace_to.is_some())
            {
                let wtop = doc_top + l.block_top(n) as f32;
                self.plan_widget(
                    &mut plan,
                    w,
                    wtop,
                    content_left,
                    content_w,
                    &tb,
                    &merge_colors,
                    window,
                );
            }
            if l.is_hidden(n) {
                n += 1;
                continue;
            }
            let block_end = l.block_end_line(n);
            let line = doc.line(n);
            let Some(m) = l.measured(n) else {
                n = block_end + 1;
                continue;
            };
            let rows = m.rows.len();
            let top = row_box(n, 0, &l);
            let height = rows as f32 * lh;
            // Block widgets above the line.
            for w in l
                .widgets()
                .iter()
                .filter(|w| w.line == n && w.replace_to.is_none())
            {
                let wtop = doc_top + l.block_top(n) as f32;
                self.plan_widget(
                    &mut plan,
                    w,
                    wtop,
                    content_left,
                    content_w,
                    &tb,
                    &merge_colors,
                    window,
                );
            }
            // Line backgrounds.
            let line_bg_rect = rect(content_left, top, content_w, height);
            let active = self.prefs.active_line && active_lines.contains(&n);
            if active {
                let c = if changed_line_set.contains(&line.from) {
                    merge_colors.changed_line_active
                } else {
                    theme.active_line
                };
                plan.line_bgs.push(Op::Quad(fill(line_bg_rect, h(c))));
            } else if changed_line_set.contains(&line.from) {
                plan.line_bgs
                    .push(Op::Quad(fill(line_bg_rect, h(merge_colors.changed_line))));
            }
            if !active
                && self.highlighter.is_some()
                && (crate::prefs::wants_diff_line_chrome(&self.path)
                    || self.scope == "source.diff"
                    || self.scope == "text.diff")
            {
                let text = doc.slice(line.from, line.to);
                if let Some(kind) = crate::prefs::diff_line_kind(&text) {
                    let c = match kind {
                        crate::prefs::DiffLineKind::Ins => {
                            crate::color::rgba_f(63.0 / 255.0, 185.0 / 255.0, 80.0 / 255.0, 0.14)
                        }
                        crate::prefs::DiffLineKind::Del => {
                            crate::color::rgba_f(224.0 / 255.0, 101.0 / 255.0, 95.0 / 255.0, 0.14)
                        }
                        crate::prefs::DiffLineKind::Meta => {
                            crate::color::rgba_f(210.0 / 255.0, 168.0 / 255.0, 1.0, 0.10)
                        }
                    };
                    plan.line_bgs.push(Op::Quad(fill(line_bg_rect, h(c))));
                }
            }
            // Mark backgrounds and text, row by row.
            let runs = line_styles(self, n, m, line.from);
            let x_of = |doc_pos: usize, assoc: i32| -> (usize, f32) {
                let d = m.display.disp_of(doc_pos - line.from, assoc);
                let row = m.row_of(d, assoc);
                (row, text_left + m.row_x(d, row))
            };
            let mark = |plan: &mut Vec<Op>,
                        from: usize,
                        to: usize,
                        color: Rgba,
                        pad_x: f32,
                        pad_y: f32,
                        radius: f32| {
                if to <= from || from > line.to || to < line.from {
                    return;
                }
                let (f, t) = (from.max(line.from), to.min(line.to));
                let (rf, _) = x_of(f, 1);
                let (rt, _) = x_of(t, -1);
                for row in rf..=rt {
                    let (rs, re) = m.row_range(row);
                    let ds = m.display.disp_of(f - line.from, 1).max(rs);
                    let de = m.display.disp_of(t - line.from, -1).min(re);
                    if de < ds {
                        continue;
                    }
                    let x0 = text_left + m.row_x(ds, row) - pad_x;
                    let x1 = text_left + m.row_x(de, row) + pad_x;
                    let y = row_box(n, row, &l) + tb.top() - pad_y;
                    plan.push(Op::Quad(
                        fill(rect(x0, y, x1 - x0, tb.height() + 2.0 * pad_y), h(color))
                            .corner_radii(px(radius)),
                    ));
                }
            };
            let em = self.metrics.font_size;
            if let Some(hl) = self.highlighter.as_ref()
                && let Some(tm) = self.tm.as_ref()
            {
                let map = tm.borrow().catalog.color_map().to_vec();
                for span in hl.spans(n) {
                    if let Some(bg) = span
                        .paint
                        .background
                        .and_then(|i| map.get(i as usize))
                        .and_then(|c| crate::color::parse_css_color(c))
                    {
                        mark(
                            &mut plan.mark_bgs,
                            span.from,
                            span.to,
                            bg,
                            0.12 * em,
                            0.05 * em,
                            2.0,
                        );
                    }
                }
            }
            for (f, t, selected) in search_hl.iter().map(|m| (m.from, m.to, m.selected)) {
                mark(
                    &mut plan.mark_bgs,
                    f,
                    t,
                    if selected {
                        theme.search_match_selected
                    } else {
                        theme.search_match
                    },
                    0.0,
                    0.0,
                    0.0,
                );
            }
            for &(f, t) in &sel_matches {
                mark(
                    &mut plan.mark_bgs,
                    f,
                    t,
                    theme.selection_match,
                    0.0,
                    0.0,
                    0.0,
                );
            }
            for &(f, t, matched) in &brackets {
                mark(
                    &mut plan.mark_bgs,
                    f,
                    t,
                    if matched {
                        theme.matching_bracket
                    } else {
                        theme.nonmatching_bracket
                    },
                    0.0,
                    0.0,
                    0.0,
                );
            }
            for (chunk, d) in &merge_decos {
                let _ = chunk;
                for &(f, t) in &d.changed_text {
                    if full_line(doc.line_at(f).from) {
                        continue;
                    }
                    mark(
                        &mut plan.mark_bgs,
                        f,
                        t,
                        merge_colors.changed_text,
                        0.0,
                        0.05 * em,
                        2.0,
                    );
                }
            }
            if let Some(snip) = self.state.snippet.as_ref() {
                for (f, t, empty) in snip.decorations() {
                    if empty {
                        if f >= line.from && f <= line.to {
                            let (row, x) = x_of(f, 1);
                            let y = row_box(n, row, &l) + tb.top();
                            plan.overlay.push(Op::Quad(quad(
                                rect(x - 0.7, y, 1.4, 1.15 * em),
                                Corners::default(),
                                gpui_kit::transparent_black(),
                                Edges {
                                    left: px(1.4),
                                    ..Default::default()
                                },
                                h(theme.snippet_field_position),
                                BorderStyle::Dashed,
                            )));
                        }
                    } else {
                        mark(&mut plan.mark_bgs, f, t, theme.snippet_field, 0.0, 0.0, 0.0);
                    }
                }
            }
            // Whitespace marks.
            let text = doc.slice(line.from, line.to);
            let trailing_from = line.from + text.trim_end_matches(crate::state::is_js_space).len();
            match self.prefs.whitespace {
                EditorWhitespace::Trailing => mark(
                    &mut plan.mark_bgs,
                    trailing_from,
                    line.to,
                    theme.trailing_space,
                    0.0,
                    0.0,
                    0.0,
                ),
                EditorWhitespace::All => {
                    for (i, c) in text.char_indices() {
                        let pos = line.from + i;
                        if c == ' ' {
                            let (row, x0) = x_of(pos, 1);
                            let (_, x1) = x_of(pos + 1, -1);
                            let y = row_box(n, row, &l) + tb.top();
                            let (cx_, cy) = ((x0 + x1) / 2.0, y + tb.height() * 0.55);
                            let r = 0.2 * ((x1 - x0) / 2.0).hypot(tb.height() * 0.55);
                            let color = runs_color_at(&runs, m.display.disp_of(i, 1))
                                .unwrap_or(h(theme.foreground));
                            let mut b = PathBuilder::fill();
                            b.move_to(point(px(cx_ + r), px(cy)));
                            for k in 1..=16 {
                                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                                b.line_to(point(px(cx_ + r * a.cos()), px(cy + r * a.sin())));
                            }
                            if let Ok(p) = b.build() {
                                plan.mark_bgs
                                    .push(Op::Path(p, h(theme.whitespace_dot(color.into()))));
                            }
                        } else if c == '\t' {
                            let (row, x0) = x_of(pos, 1);
                            let (_, x1) = x_of(pos + 1, -1);
                            let y = row_box(n, row, &l) + tb.top();
                            let hh = tb.height();
                            let s = hh / 20.0;
                            let right = x1;
                            let y0 = y;
                            let mut b = PathBuilder::stroke(px(s.max(0.5)));
                            let xs = |v: f32| (right - (197.0 - v) * s).max(x0);
                            b.move_to(point(px(xs(1.0)), px(y0 + 10.0 * s)));
                            b.line_to(point(px(xs(196.0)), px(y0 + 10.0 * s)));
                            b.line_to(point(px(xs(190.0)), px(y0 + 5.0 * s)));
                            b.move_to(point(px(xs(190.0)), px(y0 + 15.0 * s)));
                            b.line_to(point(px(xs(196.0)), px(y0 + 10.0 * s)));
                            b.move_to(point(px(xs(197.0)), px(y0 + 4.0 * s)));
                            b.line_to(point(px(xs(197.0)), px(y0 + 16.0 * s)));
                            if let Ok(p) = b.build() {
                                plan.overlay.push(Op::Path(p, h(theme.tab_arrow)));
                            }
                        }
                    }
                }
                EditorWhitespace::Off => {}
            }
            // Lint underlines and points.
            for lr in self.state.lint.ranges.iter() {
                if lr.to < line.from || lr.from > line.to {
                    continue;
                }
                if lr.widget {
                    if lr.from >= line.from && lr.from <= line.to {
                        let (row, x) = x_of(lr.from, -1);
                        let bottom = row_box(n, row, &l) + tb.top() + tb.height();
                        let mut b = PathBuilder::fill();
                        b.move_to(point(px(x - 2.0), px(bottom)));
                        b.line_to(point(px(x + 4.0), px(bottom)));
                        b.line_to(point(px(x + 1.0), px(bottom - 4.0)));
                        if let Ok(p) = b.build() {
                            plan.overlay
                                .push(Op::Path(p, h(theme.lint_point_color(lr.severity))));
                        }
                    }
                    continue;
                }
                let (f, t) = (lr.from.max(line.from), lr.to.min(line.to));
                let (rf, _) = x_of(f, 1);
                let (rt, _) = x_of(t, -1);
                for row in rf..=rt {
                    let (rs, re) = m.row_range(row);
                    let ds = m.display.disp_of(f - line.from, 1).max(rs);
                    let de = m.display.disp_of(t - line.from, -1).min(re);
                    let x0 = text_left + m.row_x(ds, row);
                    let x1 = text_left + m.row_x(de, row);
                    let bottom = row_box(n, row, &l) + tb.top() + tb.height() + 0.7;
                    if let Some(op) = zigzag(x0, x1, bottom, h(theme.lint_color(lr.severity))) {
                        plan.overlay.push(op);
                    }
                }
            }
            // Text rows.
            for row in 0..rows {
                let (rs, re) = m.row_range(row);
                let (ph, _) = m.display.placeholder().unwrap_or((usize::MAX, 0));
                let text_end = re.min(ph);
                if text_end > rs {
                    let row_text = m.display.text[rs..text_end].to_string();
                    let text_runs = runs_for(&runs, rs, text_end, &self.font);
                    let shaped = window.text_system().shape_line(
                        SharedString::from(row_text),
                        px(self.metrics.font_size),
                        &text_runs,
                        None,
                    );
                    let x = text_left;
                    plan.text.push(Op::Text {
                        line: Box::new(shaped),
                        origin: point(px(x), px(row_box(n, row, &l))),
                        line_height: px(lh),
                    });
                }
                if ph != usize::MAX && ph >= rs && ph < re.max(rs + 1) {
                    let x = text_left + m.row_x(ph, row);
                    let w = self.text_width("\u{2026}", window);
                    let y = row_box(n, row, &l) + tb.top() - 1.0;
                    plan.text.push(Op::Quad(quad(
                        rect(x + 1.0, y, w + 4.0, tb.height() + 2.0),
                        Corners::all(px(0.2 * self.metrics.font_size)),
                        h(theme.fold_placeholder_bg),
                        Edges::all(px(1.0)),
                        h(theme.fold_placeholder_border),
                        BorderStyle::Solid,
                    )));
                    let shaped = window.text_system().shape_line(
                        SharedString::from("\u{2026}"),
                        px(self.metrics.font_size),
                        &[run_of(
                            &self.font,
                            &StyleRun {
                                len: "\u{2026}".len(),
                                color: h(theme.fold_placeholder_fg),
                                bold: false,
                                italic: false,
                                underline: false,
                                strike: false,
                            },
                        )],
                        None,
                    );
                    plan.text.push(Op::Text {
                        line: Box::new(shaped),
                        origin: point(px(x + 3.0), px(row_box(n, row, &l))),
                        line_height: px(lh),
                    });
                }
            }
            n = block_end + 1;
        }
        // Trailing widgets (after the last line).
        let lines = doc.lines();
        for w in l.widgets().iter().filter(|w| w.line > lines) {
            let top = doc_top + l.content_height() as f32 - l.config.padding_bottom - w.height;
            self.plan_widget(
                &mut plan,
                w,
                top,
                content_left,
                content_w,
                &tb,
                &merge_colors,
                window,
            );
        }
        drop(l);

        // Cursors (drawSelection cursor layer, or the modal block cursor)
        // and the drop cursor.
        let blink_visible = self.blink_on || self.metrics.cursor_blink_ms == 0;
        if let Some(fat) = self.fat_cursor_mode() {
            if !focused || blink_visible {
                self.plan_fat_cursors(&mut plan, fat, focused, text_left, doc_top, &tb, window);
            }
        } else if focused && blink_visible {
            let caret = h(theme.caret);
            let heads: Vec<(usize, i32)> = self
                .state
                .selection
                .ranges
                .iter()
                .map(|r| {
                    let assoc = if r.is_empty() {
                        r.assoc()
                    } else if r.head() > r.anchor() {
                        -1
                    } else {
                        1
                    };
                    (r.head(), if assoc == 0 { 1 } else { assoc })
                })
                .collect();
            for (head, assoc) in heads {
                if let Some(r) = self.caret_rect(head, assoc, text_left, doc_top, &tb, window) {
                    plan.overlay.push(Op::Quad(fill(r, caret)));
                }
            }
        }
        if let Some(p) = self.drop_cursor
            && let Some(r) = self.caret_rect(p, 1, text_left, doc_top, &tb, window)
        {
            plan.overlay.push(Op::Quad(fill(r, h(theme.caret))));
        }
        // Rulers.
        if self.prefs.ruler_column > 0 {
            let cw = self.layout.borrow().config.char_width;
            let x = (content_left + self.prefs.ruler_column as f32 * cw).round();
            plan.overlay
                .push(Op::Quad(fill(rect(x, by, 1.0, view_h), h(theme.ruler))));
        }
        // Revert chip.
        self.plan_chip(
            &mut plan,
            bx,
            by,
            gutter_width + content_w,
            scroller_width,
            window,
        );

        // Gutters.
        if gutter_width > 0.0 {
            let gb = rect(bx, by, gutter_width, bh);
            plan.gutter_bounds = Some(gb);
            plan.gutter
                .push(Op::Quad(fill(gb, h(theme.gutter_background))));
            plan.gutter.push(Op::Quad(fill(
                rect(bx + gutters_w, by, 1.0, bh),
                h(theme.gutter_border),
            )));
            self.plan_gutters(
                &mut plan,
                &gutters,
                bx,
                doc_top,
                first,
                last.min(lines_total),
                &tb,
                &changed_line_set,
                window,
            );
        }
        // Scrollbars.
        let tokens = theme.tokens;
        if vbar || content_h > view_h {
            let track = view_h;
            let thumb = (track * view_h / content_h.max(1.0)).max(20.0).min(track);
            let y = by + (track - thumb) * (self.scroll_top / (content_h - view_h).max(1.0));
            plan.scrollbars.push(Op::Quad(
                fill(
                    rect(bx + bw - SCROLLBAR_SIZE, y, SCROLLBAR_SIZE, thumb),
                    h(tokens.scrollbar_thumb),
                )
                .corner_radii(px(8.0)),
            ));
        }
        if hbar {
            let track = scroller_width;
            let thumb = (track * scroller_width / (gutter_width + content_w).max(1.0))
                .max(20.0)
                .min(track);
            let x = bx + (track - thumb) * (self.scroll_left / (content_w - text_area_w).max(1.0));
            plan.scrollbars.push(Op::Quad(
                fill(
                    rect(x, by + bh - SCROLLBAR_SIZE, thumb, SCROLLBAR_SIZE),
                    h(tokens.scrollbar_thumb),
                )
                .corner_radii(px(8.0)),
            ));
        }
        plan.crosshair = self.prefs.rectangular_select && window.modifiers().alt;
        plan
    }

    /// The modal keymap's block cursor, when it replaces the caret: `Some(true)`
    /// for the half-height pending form.
    pub(crate) fn fat_cursor_mode(&self) -> Option<bool> {
        self.emacs.as_ref().map(crate::emacs::Emacs::pending)
    }

    /// `BlockCursorPlugin`: a `.cm-fat-cursor` per range, the character
    /// under it drawn on top in its own style.
    #[allow(clippy::too_many_arguments)]
    fn plan_fat_cursors(
        &self,
        plan: &mut Plan,
        partial: bool,
        focused: bool,
        text_left: f32,
        doc_top: f32,
        tb: &TextBox,
        window: &Window,
    ) {
        let fat = h(gpui_kit::rgb(0xff9696));
        let doc = &self.state.doc;
        let h_coeff = if partial { 0.5 } else { 1.0 };
        for r in self.state.selection.ranges.clone() {
            let head = r.head();
            let mut letter: Option<String> = (head < doc.len()).then(|| {
                doc.slice(
                    head,
                    doc.clamp_to_char(head + 1).max(head + 1).min(doc.len()),
                )
            });
            if letter.as_deref().is_some_and(|l| l.is_empty()) {
                letter = None;
            }
            let (top, _) = self.with_layout_view(window, |lv| lv.row_box(head, 1));
            let mut left = text_left + self.with_layout_view(window, |lv| lv.x_at(head, 1));
            let letter = match letter.as_deref() {
                None | Some("\n") | Some("\r") => "\u{a0}".to_string(),
                Some("\t") => {
                    if head < doc.len() {
                        let next =
                            text_left + self.with_layout_view(window, |lv| lv.x_at(head + 1, -1));
                        left = next - (next - left) / self.state.tab_size().max(1) as f32;
                    }
                    "\u{a0}".to_string()
                }
                Some(l) => l.to_string(),
            };
            let line = doc.line_at(head);
            let (color, bold, italic) = self
                .layout
                .borrow()
                .measured(line.number)
                .map(|m| {
                    let runs = line_styles(self, line.number, m, line.from);
                    let d = m.display.disp_of(head - line.from, 1);
                    let mut pos = 0;
                    for run in &runs {
                        if d >= pos && d < pos + run.len {
                            return (run.color, run.bold, run.italic);
                        }
                        pos += run.len;
                    }
                    (h(self.theme.foreground), false, false)
                })
                .unwrap_or((h(self.theme.foreground), false, false));
            let hh = tb.height();
            let y = doc_top + top + tb.top() + hh * (1.0 - h_coeff);
            let box_h = hh * h_coeff;
            let style = StyleRun {
                len: letter.len(),
                color,
                bold,
                italic,
                underline: false,
                strike: false,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(letter.clone()),
                px(self.metrics.font_size),
                &[run_of(&self.font, &style)],
                None,
            );
            let w = f32::from(shaped.width);
            if focused {
                plan.overlay
                    .push(Op::Quad(fill(rect(left, y, w, box_h), fat)));
            } else {
                plan.overlay.push(Op::Quad(quad(
                    rect(left - 1.0, y - 1.0, w + 2.0, box_h + 2.0),
                    Corners::default(),
                    gpui_kit::transparent_black(),
                    Edges::all(px(1.0)),
                    fat,
                    BorderStyle::Solid,
                )));
            }
            if focused && !partial {
                plan.overlay.push(Op::Text {
                    line: Box::new(shaped),
                    origin: point(px(left), px(y)),
                    line_height: px(box_h),
                });
            }
        }
    }

    /// Caret rectangle for a position (`coordsAtPos` + caret width/nudge).
    pub(crate) fn caret_rect(
        &self,
        pos: usize,
        assoc: i32,
        text_left: f32,
        doc_top: f32,
        tb: &TextBox,
        window: &Window,
    ) -> Option<Bounds<Pixels>> {
        let (top, _) = self.with_layout_view(window, |lv| {
            lv.row_box(pos.min(self.state.doc.len()), assoc)
        });
        let x = self.with_layout_view(window, |lv| lv.x_at(pos.min(self.state.doc.len()), assoc));
        Some(rect(
            text_left + x + self.metrics.caret_nudge,
            doc_top + top + tb.top(),
            self.metrics.caret_width,
            tb.height(),
        ))
    }

    /// `rectanglesForRange` for one selection range.
    #[allow(clippy::too_many_arguments)]
    fn selection_rects(
        &self,
        from: usize,
        to: usize,
        left: f32,
        right: f32,
        doc_top: f32,
        tb: &TextBox,
        window: &Window,
    ) -> Vec<Bounds<Pixels>> {
        let text_left = left;
        self.with_layout_view(window, |lv| {
            let (f_top, _) = lv.row_box(from, 1);
            let (t_top, _) = lv.row_box(to, -1);
            let xf = text_left + lv.x_at(from, 1);
            let xt = text_left + lv.x_at(to, -1);
            let content_top = |row_top: f32| doc_top + row_top + tb.top();
            if (f_top - t_top).abs() < 0.5 {
                let y = content_top(f_top);
                return vec![rect(xf, y, xt - xf, tb.height())];
            }
            let mut top = (content_top(f_top), content_top(f_top) + tb.height());
            let mut bottom = (content_top(t_top), content_top(t_top) + tb.height());
            let mut out = Vec::new();
            let lh = tb.line_height;
            let gap_rows = (t_top - f_top) / lh;
            if gap_rows > 1.5 {
                out.push(rect(left, top.1, right - left, bottom.0 - top.1));
            } else if top.1 < bottom.0 {
                let mid = (top.1 + bottom.0) / 2.0;
                top.1 = mid;
                bottom.0 = mid;
            }
            out.insert(0, rect(xf, top.0, right - xf, top.1 - top.0));
            out.push(rect(left, bottom.0, xt - left, bottom.1 - bottom.0));
            out
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_widget(
        &self,
        plan: &mut Plan,
        w: &BlockWidget,
        top: f32,
        content_left: f32,
        content_w: f32,
        tb: &TextBox,
        mc: &crate::theme::MergeColors,
        window: &Window,
    ) {
        let lh = tb.line_height;
        let Some(merge) = self.state.merge.as_ref() else {
            return;
        };
        if w.replace_to.is_some() {
            let idx = w.id - 1_000_000;
            let Some(c) = merge.collapsed.get(idx) else {
                return;
            };
            let hover = self.collapsed_hover == Some(idx);
            plan.line_bgs.push(Op::Quad(fill(
                rect(content_left, top, content_w, w.height),
                h(if hover {
                    mc.collapsed_bg_hover
                } else {
                    mc.collapsed_bg
                }),
            )));
            plan.line_bgs.push(Op::Quad(fill(
                rect(content_left, top, content_w, 1.0),
                h(mc.collapsed_border),
            )));
            plan.line_bgs.push(Op::Quad(fill(
                rect(content_left, top + w.height - 1.0, content_w, 1.0),
                h(mc.collapsed_border),
            )));
            // `.cm-collapsedLines`: padding 5px 14px, mono 10.5px/500,
            // letter-spacing .02em, "·" 8px label 8px "·".
            let mut font = crate::tokens::mono_font();
            font.weight = FontWeight::MEDIUM;
            let color = h(if hover {
                mc.collapsed_fg_hover
            } else {
                mc.collapsed_fg
            });
            let spacing = 0.02 * 10.5;
            let y = top + 1.0 + 5.0;
            let mut x = content_left + 14.0;
            for (i, part) in [
                "\u{b7}".to_string(),
                format!("{} unchanged lines", c.lines),
                "\u{b7}".to_string(),
            ]
            .iter()
            .enumerate()
            {
                let (ops, w) = spaced_text(
                    part,
                    &font,
                    10.5,
                    color,
                    spacing,
                    (x, y),
                    10.5 * 1.4,
                    window,
                );
                plan.text.extend(ops);
                x += w + if i < 2 { 8.0 } else { 0.0 };
            }
            return;
        }
        let Some(chunk) = merge.chunks.get(w.id) else {
            return;
        };
        if chunk.from_a >= chunk.to_a {
            return;
        }
        plan.line_bgs.push(Op::Quad(fill(
            rect(content_left, top, content_w, w.height),
            h(mc.deleted_chunk),
        )));
        let measure = GpuiMeasure {
            ts: window.text_system(),
            font: self.font.clone(),
            size: px(self.metrics.font_size),
        };
        let x0 = content_left + DELETED_CHUNK_PADDING;
        let fg = h(self.theme.foreground);
        let mut y = top;
        for spans in deleted_chunk_lines(&merge.original, chunk) {
            let text: String = spans.iter().map(|s| s.text.as_str()).collect();
            let (display, xs, rows) = deleted_line_rows(self, &text, &measure);
            let all_changed = !spans.is_empty() && spans.iter().all(|s| s.changed);
            let bounds: Vec<usize> = display
                .text
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(display.text.len()))
                .collect();
            let x_of = |d: usize| xs[bounds.partition_point(|b| *b < d).min(xs.len() - 1)];
            let row_end = |r: usize| rows.get(r + 1).copied().unwrap_or(display.text.len());
            let mut doc_off = 0;
            for s in &spans {
                let (df, dt) = (
                    display.disp_of(doc_off, 1),
                    display.disp_of(doc_off + s.text.len(), -1),
                );
                doc_off += s.text.len();
                if !s.changed || all_changed {
                    continue;
                }
                for (r, &rs) in rows.iter().enumerate() {
                    let (a, b) = (df.max(rs), dt.min(row_end(r)));
                    if a >= b {
                        continue;
                    }
                    let base = x_of(rs);
                    plan.mark_bgs.push(Op::Quad(
                        fill(
                            rect(
                                x0 + x_of(a) - base,
                                y + r as f32 * lh + tb.top(),
                                x_of(b) - x_of(a),
                                tb.height(),
                            ),
                            h(mc.deleted_text),
                        )
                        .corner_radii(px(2.0)),
                    ));
                }
            }
            for (r, &rs) in rows.iter().enumerate() {
                let re = row_end(r);
                if re > rs {
                    let row = display.text[rs..re].to_string();
                    let shaped = window.text_system().shape_line(
                        SharedString::from(row.clone()),
                        px(self.metrics.font_size),
                        &[TextRun {
                            len: row.len(),
                            font: self.font.clone(),
                            color: fg,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    );
                    plan.text.push(Op::Text {
                        line: Box::new(shaped),
                        origin: point(px(x0), px(y + r as f32 * lh)),
                        line_height: px(lh),
                    });
                }
            }
            y += rows.len() as f32 * lh;
        }
    }

    /// The revert chip (`mergeRevertHoverPlugin.place`): below the hunk's
    /// box, right-aligned to the nearer of the content edge and the visible
    /// scroller edge. Positions are scroller-relative like the plugin's
    /// absolute `style.top/left`.
    #[allow(clippy::too_many_arguments)]
    fn plan_chip(
        &mut self,
        plan: &mut Plan,
        bx: f32,
        by: f32,
        content_right: f32,
        client_w: f32,
        window: &Window,
    ) {
        self.chip_bounds = None;
        if self.read_only {
            return;
        }
        let Some(from_b) = self.revert.from_b else {
            return;
        };
        let Some(merge) = self.state.merge.clone() else {
            return;
        };
        let Some(chunk) = merge.chunks.iter().find(|c| c.from_b == from_b) else {
            return;
        };
        let Some((_, bottom)) = self.hunk_box(chunk, window) else {
            return;
        };
        let label = "Revert";
        let key = crate::merge::revert_hunk_key_display();
        let mut ui = crate::tokens::ui_font();
        ui.weight = FontWeight::MEDIUM;
        let spacing = |size: f32| 0.02 * size;
        let measure = |t: &str, size: f32| {
            t.chars()
                .map(|ch| self.ui_width(&ch.to_string(), &ui, size, window) + spacing(size))
                .sum::<f32>()
        };
        let width = 1.0 + 8.0 + measure(label, 10.5) + 6.0 + measure(&key, 10.0) + 8.0 + 1.0;
        let left = crate::merge::hunk_button_left(content_right, self.scroll_left, client_w, width);
        let x = bx + left.round() - self.scroll_left;
        let y = by + (bottom + crate::merge::REVERT_OUTSIDE_GAP).round() - self.scroll_top;
        let hgt = 16.0 + 2.0 + 2.0;
        let mc = self.theme.merge();
        let hover = self.chip_hover;
        plan.chip.push(Op::Quad(quad(
            rect(x, y, width, hgt),
            Corners::all(px(5.0)),
            h(if hover {
                mc.revert_bg_hover
            } else {
                mc.revert_bg
            }),
            Edges::all(px(1.0)),
            h(mc.revert_border),
            BorderStyle::Solid,
        )));
        let fg = h(if hover {
            mc.revert_fg_hover
        } else {
            mc.revert_fg
        });
        let (ops, lw) = spaced_text(
            label,
            &ui,
            10.5,
            fg,
            spacing(10.5),
            (x + 9.0, y + 2.0),
            16.0,
            window,
        );
        plan.chip.extend(ops);
        let kcolor = Hsla {
            a: fg.a * 0.62,
            ..fg
        };
        let (ops, _) = spaced_text(
            &key,
            &ui,
            10.0,
            kcolor,
            spacing(10.0),
            (x + 9.0 + lw + 6.0, y + 2.0),
            16.0,
            window,
        );
        plan.chip.extend(ops);
        self.chip_bounds = Some(rect(x, y, width, hgt));
    }

    fn ui_width(&self, text: &str, font: &gpui_kit::Font, size: f32, window: &Window) -> f32 {
        let shaped = window.text_system().shape_line(
            SharedString::from(text.to_string()),
            px(size),
            &[TextRun {
                len: text.len(),
                font: font.clone(),
                color: gpui_kit::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        f32::from(shaped.width)
    }

    /// `hunkContentBox` in scroller content coordinates. Like the plugin,
    /// the B-side lines come from `lineBlockAt` (document coordinates,
    /// without the content's top padding) and the deleted widget from its
    /// painted box.
    pub(crate) fn hunk_box(&self, chunk: &Chunk, window: &Window) -> Option<(f32, f32)> {
        let doc = &self.state.doc;
        let mut top = f32::INFINITY;
        let mut bottom = f32::NEG_INFINITY;
        let pad = self.layout.borrow().config.padding_top;
        if chunk.from_b < chunk.to_b && !doc.is_empty() {
            let from = chunk.from_b.min(doc.len());
            let last = chunk.to_b.saturating_sub(1).max(from).min(doc.len());
            let (lt, lb) = self.with_layout_view(window, |lv| {
                (lv.line_block_at(from).top, lv.line_block_at(last).bottom())
            });
            top = top.min(lt - pad);
            bottom = bottom.max(lb - pad);
        }
        let l = self.layout.borrow();
        let n = doc.line_at(chunk.from_b.min(doc.len())).number;
        let idx = self
            .state
            .merge
            .as_ref()?
            .chunks
            .iter()
            .position(|c| c == chunk)?;
        if chunk.from_a < chunk.to_a
            && let Some(w) = l
                .widgets()
                .iter()
                .find(|w| w.replace_to.is_none() && w.id == idx && w.line == n)
        {
            let wtop = l.block_top(n) as f32;
            top = top.min(wtop);
            bottom = bottom.max(wtop + w.height);
        }
        (bottom.is_finite() && bottom >= top).then_some((top, bottom))
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_gutters(
        &self,
        plan: &mut Plan,
        gutters: &[GutterColumn],
        bx: f32,
        doc_top: f32,
        first: usize,
        last: usize,
        tb: &TextBox,
        changed_lines: &std::collections::HashSet<usize>,
        window: &Window,
    ) {
        let theme = &self.theme;
        let lh = tb.line_height;
        let doc = &self.state.doc;
        let l = self.layout.borrow();
        let mut active: Vec<usize> = Vec::new();
        if self.prefs.active_line {
            for r in &self.state.selection.ranges {
                let from = doc.line_at(r.head()).from;
                if !active.contains(&from) {
                    active.push(from);
                }
            }
        }
        let leftmost_marked = gutters.iter().find(|g| match g.kind {
            GutterKind::Findings => self.state.gutters.findings_visible(),
            GutterKind::Lint => self.state.lint.gutter_visible(),
            GutterKind::LineNumbers => true,
            _ => false,
        });
        let bar = h(theme.active_gutter_bar(self.lit));
        let mut n = first;
        while n <= last {
            if l.is_hidden(n) {
                n += 1;
                continue;
            }
            let block_end = l.block_end_line(n);
            let line = doc.line(n);
            let top = doc_top + l.line_top(n) as f32;
            let rows = l.measured(n).map_or(1, |m| m.rows.len());
            let height = rows as f32 * lh;
            // Merge widgets get a gutter marker for deleted chunks.
            for w in l
                .widgets()
                .iter()
                .filter(|w| w.line == n && w.replace_to.is_none())
            {
                if let Some(c) = gutters.iter().find(|g| g.kind == GutterKind::MergeChange) {
                    let wtop = doc_top + l.block_top(n) as f32;
                    plan.gutter.push(Op::Quad(fill(
                        rect(bx + c.x + c.width - 2.0, wtop, 2.0, w.height),
                        h(theme.merge().gutter_deleted),
                    )));
                }
            }
            let is_active = active.contains(&line.from);
            for g in gutters {
                let gx = bx + g.x;
                if is_active && g.width > 0.0 {
                    plan.gutter.push(Op::Quad(fill(
                        rect(gx, top, g.width, height),
                        h(theme.active_line_gutter),
                    )));
                    if leftmost_marked.is_some_and(|lm| lm.kind == g.kind) {
                        plan.gutter
                            .push(Op::Quad(fill(rect(gx, top, 2.0, height), bar)));
                    }
                }
                match g.kind {
                    GutterKind::LineNumbers => {
                        let label = n.to_string();
                        let shaped = window.text_system().shape_line(
                            SharedString::from(label.clone()),
                            px(self.metrics.font_size),
                            &[TextRun {
                                len: label.len(),
                                font: self.font.clone(),
                                color: h(theme.gutter_foreground),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            None,
                        );
                        let x = gx + g.width - 3.0 - f32::from(shaped.width);
                        plan.gutter.push(Op::Text {
                            line: Box::new(shaped),
                            origin: point(px(x), px(top)),
                            line_height: px(lh),
                        });
                    }
                    GutterKind::Fold => {
                        let block = crate::view_queries::LineBlock {
                            from: line.from,
                            to: doc.line(block_end).to,
                            top: 0.0,
                            height,
                        };
                        if let Some(marker) = crate::fold::fold_marker(&self.state, block) {
                            let t = marker.text();
                            let shaped = window.text_system().shape_line(
                                SharedString::from(t),
                                px(self.metrics.font_size),
                                &[TextRun {
                                    len: t.len(),
                                    font: self.font.clone(),
                                    color: h(theme.gutter_foreground),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                }],
                                None,
                            );
                            plan.gutter.push(Op::Text {
                                line: Box::new(shaped),
                                origin: point(px(gx + 1.0), px(top)),
                                line_height: px(lh),
                            });
                        }
                    }
                    GutterKind::Diff => {
                        let mark = self
                            .state
                            .gutters
                            .diff_marks
                            .iter()
                            .find(|(p, _)| doc.line_at((*p).min(doc.len())).from == line.from)
                            .map(|(_, k)| *k);
                        if let Some(kind) = mark {
                            let c = match kind {
                                crate::gutters::DiffHintKind::Add => theme.gutter_added,
                                crate::gutters::DiffHintKind::Mod => theme.gutter_modified,
                                crate::gutters::DiffHintKind::Del => theme.gutter_deleted,
                            };
                            plan.gutter.push(Op::Quad(fill(
                                rect(gx + g.width - 2.0, top, 2.0, height),
                                h(c),
                            )));
                        }
                    }
                    GutterKind::MergeChange => {
                        if changed_lines.contains(&line.from) {
                            plan.gutter.push(Op::Quad(fill(
                                rect(gx + g.width - 2.0, top, 2.0, height),
                                h(theme.merge().gutter_changed),
                            )));
                        }
                    }
                    GutterKind::Lint => {
                        if let Some(hit) = self.state.lint.gutter.get(&line.from) {
                            let icon = lint_icon(hit.severity);
                            let cw = g.width - 5.0;
                            let r = rect(
                                gx + 4.0 + (cw - 12.0) / 2.0,
                                top + (height - 12.0) / 2.0,
                                12.0,
                                12.0,
                            );
                            plan.gutter.push(Op::Icon(
                                icon,
                                r,
                                h(theme.lint_gutter_color(hit.severity)),
                            ));
                        }
                    }
                    GutterKind::Findings => {
                        if let Some(hit) = self.state.gutters.finding_hits.get(&line.from) {
                            let icon = crate::icons::by_name(crate::gutters::finding_sev_icon(
                                &hit.severity,
                            ))
                            .unwrap_or(crate::icons::SEV_LOW);
                            let cw = g.width - 5.0;
                            let r = rect(
                                gx + 4.0 + (cw - 12.0) / 2.0,
                                top + (height - 12.0) / 2.0,
                                12.0,
                                12.0,
                            );
                            plan.gutter.push(Op::Icon(
                                icon,
                                r,
                                h(theme.finding_color(&hit.severity)),
                            ));
                        }
                    }
                }
            }
            n = block_end + 1;
        }
    }

    /// Hit test a screen point: the gutter column and line, or content.
    pub(crate) fn hit_gutter(
        &self,
        p: Point<Pixels>,
        window: &Window,
    ) -> Option<(GutterKind, usize)> {
        let frame = self.frame.as_ref()?;
        let x = f32::from(p.x) - f32::from(frame.bounds.origin.x);
        if x >= frame.gutter_width {
            return None;
        }
        let col = frame
            .gutters
            .iter()
            .find(|g| x >= g.x && x < g.x + g.width)?;
        let y = f32::from(p.y) - frame.scroller_top + self.scroll_top;
        let n = self.layout.borrow().line_at_y(y as f64);
        let _ = window;
        Some((col.kind, n))
    }
}

fn runs_color_at(runs: &[StyleRun], disp: usize) -> Option<Hsla> {
    let mut pos = 0;
    for r in runs {
        if disp >= pos && disp < pos + r.len {
            return Some(r.color);
        }
        pos += r.len;
    }
    None
}
