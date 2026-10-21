//! Block layout: the markdown tree as GPUI boxes with CSS margin collapsing, the list
//! markers, code fences and their copy controls, blockquotes, rules, tables, images and
//! Mermaid slots, all at WebKit's LayoutUnit positions.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Bounds, Corners, Div, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderImage, StatefulInteractiveElement, Styled, Window, canvas, div,
    point, px, relative,
};
use recur_ui::fonts::Family;
use recur_ui::style::card;
use recur_ui::text::InlineBox;
use recur_ui::{Color, IconName, Span, Text, TextStyled, Tokens, span};

use super::doc::{El, Node};
use super::exact::{hgap, vgap};
use super::inline::{self, Context as InlineCx, Ink, Inline, Piece};
use super::mirror::{Mirror, RunSpec, TextSpec};
use super::style::{Sheet, copied_green, lu};

/// A laid-out block and its outer vertical margins (they collapse with neighbors).
pub struct Laid {
    pub el: AnyElement,
    pub mt: f32,
    pub mb: f32,
}

/// A decoded image.
#[derive(Clone)]
pub enum ImageSlot {
    Loading,
    Ready {
        image: Arc<RenderImage>,
        width: f32,
        height: f32,
    },
    Failed,
}

/// Draws a diagram at a width no wider than the argument.
pub type DrawDiagram = Rc<dyn Fn(f32, &Window) -> AnyElement>;

/// A paragraph's spans, the mirror's run specs, and link ranges with their href index.
type SpanRuns = (Vec<Span>, Vec<RunSpec>, Vec<(Range<usize>, usize)>);

/// A ```` ```mermaid ```` placeholder's state (`data-rendered`).
#[derive(Clone)]
pub enum MermaidSlot {
    Pending,
    Error,
    Empty,
    /// The diagram, drawn at a width no wider than the argument.
    Ready(DrawDiagram),
}

/// What block layout reads from the view.
pub struct Env<'a> {
    pub sheet: &'a Sheet,
    pub t: &'a Tokens,
    /// The content box width from the last frame (for tables and images).
    pub width: f32,
    pub image: &'a dyn Fn(&str) -> ImageSlot,
    pub mermaid: &'a [MermaidSlot],
    /// `MarkdownHtml`'s post-render enhancements (copy controls, image zoom).
    pub enhance: bool,
    pub hover_fence: Option<usize>,
    pub copied: &'a HashSet<usize>,
    pub hover_link: Option<(usize, usize)>,
    /// Find highlights per text block: content byte ranges, the current one flagged.
    pub highlights: &'a HashMap<usize, Vec<(Range<usize>, bool)>>,
    pub focused_image: Option<usize>,
    pub handlers: &'a Handlers,
}

type Handler<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// Callbacks into the owning view.
pub struct Handlers {
    pub fence_hover: Handler<(usize, bool)>,
    pub copy_fence: Handler<usize>,
    pub link_hover: Handler<Option<(usize, usize)>>,
    pub open_link: Handler<String>,
    pub open_image: Handler<(String, String)>,
}

/// Running counters while building one document.
#[derive(Default)]
pub struct Counters {
    pub text: usize,
    pub fence: usize,
    pub mermaid: usize,
    pub image: usize,
    pub block: usize,
    pub table: usize,
}

pub struct Build<'a, 'w> {
    pub env: &'a Env<'a>,
    pub window: &'w mut Window,
    pub n: Counters,
    /// Every inline text block, in order.
    pub texts: Vec<TextBlock>,
}

/// A rendered text block: its inputs (for the mirror), its links and where it painted.
#[derive(Clone)]
pub struct TextBlock {
    pub id: usize,
    pub spec: TextSpec,
    /// Content byte ranges of links, with the link's index in `hrefs`.
    pub links: Vec<(Range<usize>, usize)>,
    pub hrefs: Vec<String>,
    pub bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

fn hsla(c: Color) -> Hsla {
    c.into()
}

fn ink(t: &Tokens, i: Ink) -> Color {
    match i {
        Ink::Text => t.text,
        Ink::Muted => t.muted,
        Ink::Cyan => t.cyan,
        Ink::Faint => t.faint,
        Ink::Fixed(rgb) => recur_ui::hex(rgb),
    }
}

/// Inherited text properties of a block.
#[derive(Clone, Copy)]
pub struct Flow {
    pub size: f32,
    pub leading: f32,
    pub ink: Ink,
    pub weight: f32,
    /// Enclosing list depth (`ul ul` → circle, deeper → square).
    pub lists: usize,
}

/// Elements that are blocks in markdown output.
fn is_block(e: &El) -> bool {
    !e.svg
        && matches!(
            e.tag.as_str(),
            "p" | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "ul"
                | "ol"
                | "li"
                | "blockquote"
                | "pre"
                | "table"
                | "hr"
                | "div"
                | "details"
                | "summary"
                | "section"
                | "figure"
                | "dl"
                | "dd"
                | "dt"
        )
}

impl Build<'_, '_> {
    fn text_id(&mut self) -> ElementId {
        self.n.text += 1;
        ElementId::from(("md-t", self.n.text))
    }

    /// The spans for a block's inline content, as recur-ui text, and their resolved runs (for
    /// the mirror). Find highlights split runs so the current match can change color.
    fn spans(&self, inl: &Inline, flow: Flow, block: usize) -> SpanRuns {
        let t = self.env.t;
        let sh = self.env.sheet;
        let marks = self.env.highlights.get(&block).cloned().unwrap_or_default();
        let mut out = Vec::new();
        let mut runs = Vec::new();
        let mut links = Vec::new();
        let mut at = 0usize;
        for p in &inl.pieces {
            let (text, st) = match p {
                Piece::Text(s, st) => (s.clone(), Some(*st)),
                Piece::Break => ("\n".to_string(), None),
                Piece::Checkbox(_) => ("\u{200b}".to_string(), None),
                Piece::Image { alt, .. } => (alt.clone(), None),
            };
            let start = at;
            at += text.len();
            let Some(st) = st else {
                let mut sp = span(text.clone());
                let mut ib = None;
                if matches!(p, Piece::Checkbox(_)) {
                    // `margin: 3px 6px 3px 2px` around a 12px box: the line keeps its width;
                    // the control paints over it.
                    let b = InlineBox {
                        margin_left: 2. + 12.,
                        margin_right: 6.,
                        ..InlineBox::default()
                    };
                    sp = sp.boxed(b);
                    ib = Some(b);
                }
                out.push(sp);
                runs.push(RunSpec {
                    text,
                    family: Family::Ui,
                    size: flow.size,
                    weight: flow.weight,
                    italic: false,
                    inline_box: ib,
                });
                continue;
            };
            if let Some(l) = st.link {
                links.push((start..at, l));
            }
            let mut color = ink(t, st.ink);
            if let Some(l) = st.link
                && self.env.hover_link == Some((block, l))
            {
                color = if self.env.sheet.muted { t.cyan } else { t.text };
            }
            let ib = st.code.then_some(InlineBox {
                pad_top: 1.,
                pad_bottom: 1.,
                pad_left: 5.,
                pad_right: 5.,
                background: Some(hsla(t.pane_2)),
                radius: 3.,
                ..InlineBox::default()
            });
            if st.code && sh.code_colored && st.ink == flow.ink && st.link.is_none() {
                color = t.text;
            }
            // Split at the current find match so it can take `--bg` (code boxes stay whole).
            let mut pieces: Vec<(String, bool)> = vec![(text.clone(), false)];
            if ib.is_none()
                && let Some((r, _)) = marks
                    .iter()
                    .find(|(r, cur)| *cur && r.start < at && r.end > start)
            {
                let s0 = r.start.max(start) - start;
                let e0 = r.end.min(at) - start;
                pieces = [
                    (text[..s0].to_string(), false),
                    (text[s0..e0].to_string(), true),
                    (text[e0..].to_string(), false),
                ]
                .into_iter()
                .filter(|(x, _)| !x.is_empty())
                .collect();
            }
            for (piece, current) in pieces {
                let mut sp = span(piece.clone()).weight(st.weight);
                if let Some(b) = ib {
                    sp = sp.mono().size(sh.code_size).boxed(b);
                }
                if st.italic {
                    sp = sp.italic();
                }
                if st.strike {
                    sp = sp.line_through(true);
                }
                if st.underline {
                    sp = sp.underline(true);
                }
                out.push(sp.color(hsla(if current { t.bg } else { color })));
                runs.push(RunSpec {
                    text: piece,
                    family: if ib.is_some() {
                        Family::Mono
                    } else {
                        Family::Ui
                    },
                    size: if ib.is_some() {
                        sh.code_size
                    } else {
                        flow.size
                    },
                    weight: st.weight,
                    italic: st.italic,
                    inline_box: ib,
                });
            }
        }
        (out, runs, links)
    }

    fn text(&mut self, inl: &Inline, flow: Flow, _block: usize) -> Text {
        self.n.text += 1;
        let block = self.n.text;
        let has_break = inl.pieces.iter().any(|p| matches!(p, Piece::Break));
        let anywhere = self.env.sheet.class == super::style::MarkdownClass::ChatCanvas;
        let (spans, runs, link_ranges) = self.spans(inl, flow, block);
        let mut t = Text::from_spans(spans)
            .id(ElementId::from(("md-t", block)))
            .ui()
            .size(flow.size)
            .weight(flow.weight)
            .leading(flow.leading)
            .color(hsla(ink(self.env.t, flow.ink)));
        if has_break {
            t = t.pre_wrap();
        }
        if anywhere {
            t = t.break_anywhere();
        }
        self.texts.push(TextBlock {
            id: block,
            spec: TextSpec {
                runs,
                family: Family::Ui,
                size: flow.size,
                weight: flow.weight,
                leading: flow.leading,
                anywhere,
            },
            links: link_ranges,
            hrefs: inl.links.iter().map(|l| l.href.clone()).collect(),
            bounds: Rc::new(Cell::new(None)),
        });
        t
    }

    /// A text block with its bounds recorded (hit testing, find) and its find highlights
    /// painted under the glyphs.
    fn tracked(&mut self, text: Text) -> Div {
        let id = self.n.text;
        self.tracked_id(id, text)
    }

    fn tracked_id(&self, id: usize, text: Text) -> Div {
        let Some(tb) = self.texts.iter().find(|t| t.id == id) else {
            return div().child(text);
        };
        let cell = tb.bounds.clone();
        let marks = self.env.highlights.get(&tb.id).cloned().unwrap_or_default();
        let spec = tb.spec.clone();
        let t = self.env.t;
        let (hl, cur) = (t.amber.fade(0.4), t.amber);
        let back = canvas(
            move |b, _, _| cell.set(Some(b)),
            move |b: Bounds<Pixels>, _, window, _| {
                if marks.is_empty() {
                    return;
                }
                let m = Mirror::layout(&spec, f32::from(b.size.width), window);
                for (r, current) in &marks {
                    for (x, y, w, h) in m.rects(r.clone()) {
                        let rect = Bounds {
                            origin: point(b.origin.x + px(x), b.origin.y + px(y)),
                            size: gpui_kit::size(px(w), px(h)),
                        };
                        window.paint_quad(gpui_kit::fill(
                            rect,
                            hsla(if *current { cur } else { hl }),
                        ));
                    }
                }
            },
        )
        .absolute()
        .inset_0();
        div().relative().child(back).child(text)
    }

    /// An inline formatting context (a paragraph or an anonymous block).
    fn inline_block(&mut self, nodes: &[Node], flow: Flow) -> AnyElement {
        let inl = inline::collect(
            nodes,
            InlineCx {
                ink: flow.ink,
                strong_weight: self.env.sheet.strong_weight,
                em_muted: self.env.sheet.em_muted,
                weight: flow.weight,
            },
        );
        self.n.block += 1;
        let block = self.n.block;
        if inl.images_only() {
            return self.image_row(&inl, flow);
        }
        let links = inl.links.clone();
        let checkbox = inl.pieces.iter().find_map(|p| match p {
            Piece::Checkbox(c) => Some(*c),
            _ => None,
        });
        let images: Vec<Piece> = inl
            .pieces
            .iter()
            .filter(|p| matches!(p, Piece::Image { .. }))
            .cloned()
            .collect();
        let text_only = Inline {
            pieces: inl
                .pieces
                .iter()
                .filter(|p| !matches!(p, Piece::Image { .. }))
                .cloned()
                .collect(),
            links: inl.links.clone(),
        };
        let text = self.text(&text_only, flow, block);
        let (_, _, m) = text.measure(None, self.window);
        let mut el = div().relative().flex().flex_col().child(self.tracked(text));
        if let Some(checked) = checkbox {
            // The control's margin box sits 12px above to 6px below the baseline
            // (`vertical-align: -1px`), so it can deepen the line.
            let below = m.height - m.baseline;
            el = el.child(checkbox_box(checked, m.baseline - 9., self.env.t));
            if below < 6. {
                el = el.child(vgap(6. - below, self.window));
            }
        }
        let _ = links;
        for p in images {
            el = el.child(self.image(&p, flow));
        }
        el.into_any_element()
    }

    fn image(&mut self, p: &Piece, flow: Flow) -> AnyElement {
        let Piece::Image { src, alt, link, .. } = p else {
            return div().into_any_element();
        };
        let (a, d) = strut(flow, self.window);
        self.n.image += 1;
        let ix = self.n.image;
        let slot = (self.env.image)(src);
        let ImageSlot::Ready {
            image,
            width,
            height,
        } = slot
        else {
            return div().h(px(a + d)).into_any_element();
        };
        let max = self.env.width.max(1.);
        let w = width.min(max);
        let h = if width > 0. { height * w / width } else { 0. };
        let open = self.env.handlers.open_image.clone();
        let (src2, name) = (
            src.clone(),
            if alt.is_empty() {
                "Image".to_string()
            } else {
                alt.clone()
            },
        );
        let _ = link;
        let focused = self.env.focused_image == Some(ix);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("md-img", ix))
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .when(self.env.enhance, |d| {
                        d.on_click(move |_, window, cx| {
                            open((src2.clone(), name.clone()), window, cx)
                        })
                    })
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |b: Bounds<Pixels>, _, window, _| {
                                let _ = window.paint_image(
                                    b,
                                    b,
                                    Corners::all(px(6.)),
                                    image.clone(),
                                    0,
                                    false,
                                );
                            },
                        )
                        .size_full(),
                    )
                    .when(focused, |d| {
                        d.child(recur_ui::style::outline(2., 2., 6., 0., self.env.t.amber))
                    }),
            )
            .child(vgap(d.max(0.), self.window))
            .into_any_element()
    }

    /// `<p><img> <img></p>`: replaced boxes on the baseline, the strut's descent below.
    fn image_row(&mut self, inl: &Inline, flow: Flow) -> AnyElement {
        let mut row = div().flex().flex_row().flex_wrap().items_end();
        let (a, d) = strut(flow, self.window);
        let mut first = true;
        for p in &inl.pieces {
            if let Piece::Image { .. } = p {
                if !first {
                    row = row.child(hgap(space_width(flow, self.window), self.window));
                }
                first = false;
                row = row.child(self.image(p, flow));
            }
        }
        div()
            .flex()
            .flex_col()
            .min_h(px(a + d))
            .child(row)
            .into_any_element()
    }

    /// Block children of a container, with inline runs wrapped in anonymous blocks.
    pub fn blocks(&mut self, nodes: &[Node], flow: Flow, parent: &str) -> Vec<Laid> {
        let mut out = Vec::new();
        let elements: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| matches!(n, Node::El(_)))
            .map(|(i, _)| i)
            .collect();
        let first_el = elements.first().copied();
        let last_el = elements.last().copied();
        let mut run: Vec<Node> = Vec::new();
        let flush = |b: &mut Build, run: &mut Vec<Node>, out: &mut Vec<Laid>| {
            let meaningful = run.iter().any(|n| match n {
                Node::Text(t) => !t
                    .chars()
                    .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')),
                Node::El(_) => true,
            });
            if meaningful {
                let el = b.inline_block(run, flow);
                out.push(Laid { el, mt: 0., mb: 0. });
            }
            run.clear();
        };
        for (i, n) in nodes.iter().enumerate() {
            match n {
                Node::El(e) if is_block(e) => {
                    flush(self, &mut run, &mut out);
                    let pos = Pos {
                        first: Some(i) == first_el,
                        last: Some(i) == last_el,
                        parent,
                    };
                    if let Some(l) = self.block(e, flow, pos) {
                        out.push(l);
                    }
                }
                other => run.push(other.clone()),
            }
        }
        flush(self, &mut run, &mut out);
        out
    }

    fn block(&mut self, e: &El, flow: Flow, pos: Pos) -> Option<Laid> {
        let sh = self.env.sheet;
        let fs = flow.size;
        Some(match e.tag.as_str() {
            "p" => {
                let (mut mt, mut mb) = (0., lu(sh.p_margin_bottom * fs));
                if pos.parent == "blockquote" {
                    mt = lu(sh.blockquote_p_margin * fs);
                    mb = mt;
                }
                if pos.parent == "li" && sh.li_p_margin_zero {
                    mt = 0.;
                    mb = 0.;
                }
                if pos.last && sh.p_last_child_zero {
                    mb = 0.;
                }
                Laid {
                    el: self.inline_block(&e.children, flow),
                    mt,
                    mb,
                }
            }
            tag @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                let level = (tag.as_bytes()[1] - b'1') as usize;
                let h = sh.headings[level];
                let size = lu(fs * h.scale);
                let mut mt = lu(h.margin_top * size);
                if pos.first
                    && level < 3
                    && let Some(f) = sh.heading_first_top
                {
                    mt = lu(f * size);
                }
                let hf = Flow {
                    size,
                    leading: sh.heading_leading,
                    ink: if h.muted { Ink::Muted } else { Ink::Text },
                    weight: 650.,
                    lists: flow.lists,
                };
                Laid {
                    el: self.inline_block(&e.children, hf),
                    mt,
                    mb: lu(h.margin_bottom * size),
                }
            }
            "ul" | "ol" => self.list(e, flow, pos),
            "blockquote" => {
                let pad = lu(sh.blockquote_padding_v * fs);
                let inner = Flow {
                    ink: Ink::Muted,
                    ..flow
                };
                let kids = self.blocks(&e.children, inner, "blockquote");
                let (body, first, last) = stack(kids, self.window);
                let el = div()
                    .flex()
                    .flex_row()
                    .child(div().flex_none().w(px(3.)).bg(hsla(self.env.t.line_bright)))
                    .child(div().flex_none().w(px(sh.blockquote_padding_left)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(vgap(pad + first, self.window))
                            .child(body)
                            .child(vgap(pad + last, self.window)),
                    );
                Laid {
                    el: el.into_any_element(),
                    mt: lu(sh.blockquote_margin.0 * fs),
                    mb: lu(sh.blockquote_margin.1 * fs),
                }
            }
            "hr" => Laid {
                el: div()
                    .w_full()
                    .h(px(1.))
                    .bg(hsla(self.env.t.line))
                    .into_any_element(),
                mt: lu(sh.hr_margin * fs),
                mb: lu(sh.hr_margin * fs),
            },
            "pre" => {
                let fence = self.n.fence;
                self.n.fence += 1;
                let pre = self.pre(e, flow, false);
                Laid {
                    el: self.fence(pre, fence, 6.),
                    mt: lu(sh.pre_margin.0 * fs),
                    mb: lu(sh.pre_margin.1 * fs),
                }
            }
            "table" => self.table(e, flow),
            "div" if e.has_class("mdref") => self.mdref(e, flow),
            "div" if e.has_class("md-mermaid") => self.mermaid(e, flow)?,
            "li" => {
                let kids = self.blocks(&e.children, flow, "li");
                let (body, first, last) = stack(kids, self.window);
                Laid {
                    el: body.into_any_element(),
                    mt: first.max(lu(sh.li_margin * fs)),
                    mb: last.max(lu(sh.li_margin * fs)),
                }
            }
            _ => {
                let kids = self.blocks(&e.children, flow, "div");
                let (body, first, last) = stack(kids, self.window);
                Laid {
                    el: body.into_any_element(),
                    mt: first,
                    mb: last,
                }
            }
        })
    }

    fn list(&mut self, e: &El, flow: Flow, pos: Pos) -> Laid {
        let sh = self.env.sheet;
        let fs = flow.size;
        let ordered = e.tag == "ol";
        let start: i64 = e
            .attr("start")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1);
        let inner = Flow {
            lists: flow.lists + 1,
            ..flow
        };
        let marker_kind = if ordered {
            None
        } else {
            Some(match flow.lists {
                0 => Shape::Disc,
                1 => Shape::Circle,
                _ => Shape::Square,
            })
        };
        let mut items = Vec::new();
        let mut n = start;
        for c in &e.children {
            let Node::El(li) = c else { continue };
            if !li.is("li") {
                continue;
            }
            let kids = self.blocks(&li.children, inner, "li");
            let (body, first, last) = stack(kids, self.window);
            let marker = match marker_kind {
                Some(shape) => symbol_marker(shape, flow, ink(self.env.t, flow.ink), self.window),
                None => div()
                    .absolute()
                    .top_0()
                    .right(relative(1.))
                    .child(
                        recur_ui::text(format!("{n}.\u{a0}"))
                            .ui()
                            .size(fs)
                            .leading(flow.leading)
                            .tabular()
                            .color(hsla(ink(self.env.t, flow.ink)))
                            .nowrap(),
                    )
                    .into_any_element(),
            };
            n += 1;
            let item = div().relative().flex().flex_col().child(marker).child(body);
            items.push(Laid {
                el: item.into_any_element(),
                mt: first.max(lu(sh.li_margin * fs)),
                mb: last.max(lu(sh.li_margin * fs)),
            });
        }
        let (body, first, last) = stack(items, self.window);
        let (mt, mb) = if pos.parent == "li" {
            (
                lu(sh.nested_list_margin * fs),
                lu(sh.nested_list_margin * fs),
            )
        } else {
            (lu(sh.list_margin.0 * fs), lu(sh.list_margin.1 * fs))
        };
        Laid {
            el: div()
                .flex()
                .flex_row()
                .child(hgap(lu(sh.list_indent * fs), self.window))
                .child(div().flex_1().min_w_0().child(body))
                .into_any_element(),
            mt: mt.max(first),
            mb: mb.max(last),
        }
    }

    /// `pre > code`: `white-space: pre` lines on the pre's UA monospace strut.
    fn pre(&mut self, e: &El, flow: Flow, bare: bool) -> AnyElement {
        let sh = self.env.sheet;
        let t = self.env.t;
        let code_el = e.children.iter().find_map(|n| match n {
            Node::El(c) if c.is("code") => Some(c),
            _ => None,
        });
        let nodes: &[Node] = code_el.map_or(&e.children, |c| &c.children);
        let mut spans = Vec::new();
        let mut col = 0usize;
        collect_pre(nodes, flow.ink, &mut |s: &str, ink_| {
            let mut out = String::new();
            for ch in s.chars() {
                match ch {
                    '\t' => {
                        let n = 8 - col % 8;
                        out.extend(std::iter::repeat_n(' ', n));
                        col += n;
                    }
                    '\n' => {
                        out.push('\n');
                        col = 0;
                    }
                    c => {
                        out.push(c);
                        col += 1;
                    }
                }
            }
            let color = if ink_ == flow.ink && sh.code_colored {
                t.text
            } else {
                ink(t, ink_)
            };
            spans.push(span(out).mono().size(sh.pre_code_size).color(hsla(color)));
        });
        if spans.is_empty() {
            spans.push(span(""));
        }
        let text = Text::from_spans(spans)
            .id(self.text_id())
            .family(Family::Stack("Menlo".into(), Vec::new()))
            .size(flow.size)
            .leading(flow.leading)
            .pre_wrap();
        let (w, _, _) = text.measure(None, self.window);
        let dp = 1. / self.window.scale_factor();
        let content = div()
            .flex_none()
            .w(px((w / dp).ceil() * dp + dp))
            .child(text);
        let (pv, ph) = sh.pre_padding;
        self.n.block += 1;
        let scroller = div()
            .id(("md-pre", self.n.block))
            .overflow_x_scroll()
            .flex()
            .flex_row()
            .child(div().flex_none().w(px(ph)))
            .child(content)
            .child(div().flex_none().w(px(ph)));
        let inner = div()
            .flex()
            .flex_col()
            .child(div().h(px(pv)))
            .child(scroller)
            .child(div().h(px(pv)));
        if bare {
            inner.into_any_element()
        } else {
            card(t.pane, t.line, 6.).child(inner).into_any_element()
        }
    }

    /// `.md-fence`: the block with its hover copy control at `top/right: inset`.
    fn fence(&mut self, block: AnyElement, ix: usize, inset: f32) -> AnyElement {
        let env = self.env;
        if !env.enhance {
            return block;
        }
        let t = env.t;
        let hovered = env.hover_fence == Some(ix);
        let copied = env.copied.contains(&ix);
        let on_hover = env.handlers.fence_hover.clone();
        let copy = env.handlers.copy_fence.clone();
        let button = (hovered || copied).then(|| {
            let icon = if copied {
                recur_ui::svg_markup(CHECK_ICON)
            } else {
                recur_ui::icon(IconName::Copy).stroke_width(2.)
            };
            div()
                .id(("md-copy", ix))
                .absolute()
                .top(px(inset))
                .right(px(inset))
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_color(hsla(t.line))
                .bg(hsla(t.pane_2))
                .text_color(hsla(if copied { copied_green() } else { t.muted }))
                .cursor_pointer()
                .hover(move |s| {
                    s.bg(hsla(t.pane))
                        .border_color(hsla(t.line_bright))
                        .text_color(hsla(if copied { copied_green() } else { t.text }))
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    copy(ix, window, cx)
                })
                .child(icon.size(px(13.)))
        });
        div()
            .id(("md-fence", ix))
            .relative()
            .on_hover(move |h, window, cx| on_hover((ix, *h), window, cx))
            .child(block)
            .children(button)
            .into_any_element()
    }

    /// Cursor's ```` ```start:end:path ```` card.
    fn mdref(&mut self, e: &El, flow: Flow) -> Laid {
        let sh = self.env.sheet;
        let t = self.env.t;
        let fs = flow.size;
        let file = e
            .children
            .iter()
            .find_map(|n| match n {
                Node::El(c) if c.has_class("mdref-file") => Some(c.text_content()),
                _ => None,
            })
            .unwrap_or_default();
        let pre = e.children.iter().find_map(|n| match n {
            Node::El(c) if c.is("pre") => Some(c.clone()),
            _ => None,
        });
        let ix = self.n.fence;
        self.n.fence += 1;
        if !sh.mdref_card {
            // The doc sheet leaves the card unstyled: the path as a line of text, then a pre.
            let label = Inline {
                pieces: vec![Piece::Text(file, inline::RunStyle::base(flow.ink))],
                links: Vec::new(),
            };
            let label = self.text(&label, flow, 0);
            let mut col = div().flex().flex_col().child(label);
            if let Some(p) = pre {
                let pre_el = self.pre(&p, flow, false);
                col = col
                    .child(vgap(lu(sh.pre_margin.0 * fs), self.window))
                    .child(pre_el)
                    .child(vgap(lu(sh.pre_margin.1 * fs), self.window));
            }
            return Laid {
                el: col.into_any_element(),
                mt: 0.,
                mb: 0.,
            };
        }
        let head = div()
            .border_b_1()
            .border_color(hsla(t.line))
            .bg(hsla(t.pane_2))
            .pt(px(5.))
            .pb(px(5.))
            .pl(px(10.))
            .pr(px(34.))
            .child(
                recur_ui::text(file)
                    .mono()
                    .size(10.5)
                    .leading(flow.leading)
                    .color(hsla(t.muted))
                    .truncate(),
            );
        let mut body = div().flex().flex_col().child(head);
        if let Some(p) = pre {
            body = body.child(self.pre(&p, flow, true));
        }
        let el = card(t.pane, t.line, 6.).overflow_hidden().child(body);
        Laid {
            el: self.fence(el.into_any_element(), ix, 4.),
            mt: lu(sh.pre_margin.0 * fs),
            mb: lu(sh.pre_margin.1 * fs),
        }
    }

    fn mermaid(&mut self, e: &El, flow: Flow) -> Option<Laid> {
        let sh = self.env.sheet;
        let t = self.env.t;
        let fs = flow.size;
        let ix = self.n.mermaid;
        self.n.mermaid += 1;
        let slot = self
            .env
            .mermaid
            .get(ix)
            .cloned()
            .unwrap_or(MermaidSlot::Pending);
        let ((pv, ph), border, content) = match slot {
            MermaidSlot::Empty => return None,
            MermaidSlot::Ready(draw) => {
                let (pv, ph) = sh.mermaid_rendered_padding;
                let max = (self.env.width - 2. * ph - 2.).max(1.);
                ((pv, ph), t.line, draw(max, self.window))
            }
            slot => {
                let src = e.children.iter().find_map(|n| match n {
                    Node::El(c) if c.is("pre") => Some(c.clone()),
                    _ => None,
                });
                let pre =
                    src.map_or_else(|| div().into_any_element(), |p| self.pre(&p, flow, true));
                let border = if matches!(slot, MermaidSlot::Error) {
                    t.coral.mix(0.45, t.line)
                } else {
                    t.line
                };
                (sh.mermaid_padding, border, pre)
            }
        };
        self.n.block += 1;
        let el = card(t.pane, border, 6.).child(
            div()
                .id(("md-mermaid", self.n.block))
                .overflow_x_scroll()
                .flex()
                .flex_col()
                .child(div().h(px(pv)))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .child(div().flex_none().w(px(ph)))
                        .child(content)
                        .child(div().flex_none().w(px(ph))),
                )
                .child(div().h(px(pv))),
        );
        Some(Laid {
            el: el.into_any_element(),
            mt: lu(sh.pre_margin.0 * fs),
            mb: lu(sh.pre_margin.1 * fs),
        })
    }

    /// `display: block` tables with `border-collapse: collapse`: an anonymous auto-layout
    /// table at max-content width (shrinking toward min-content when the box is narrower),
    /// 1px borders centred on the grid lines, `vertical-align: top` cells.
    fn table(&mut self, e: &El, flow: Flow) -> Laid {
        let sh = self.env.sheet;
        let t = self.env.t;
        let size = sh.table_size;
        let (pv, ph) = sh.cell_padding;
        let tf = Flow { size, ..flow };
        // Rows in order (thead, then tbody), each cell with its header flag.
        let mut rows: Vec<(bool, Vec<(bool, &El)>)> = Vec::new();
        let mut body_ix = 0usize;
        fn push_rows<'e>(
            group: &'e El,
            rows: &mut Vec<(bool, Vec<(bool, &'e El)>)>,
            body: bool,
            body_ix: &mut usize,
        ) {
            for r in &group.children {
                let Node::El(tr) = r else { continue };
                if !tr.is("tr") {
                    continue;
                }
                let cells: Vec<(bool, &El)> = tr
                    .children
                    .iter()
                    .filter_map(|c| match c {
                        Node::El(c) if c.is("th") || c.is("td") => Some((c.is("th"), c)),
                        _ => None,
                    })
                    .collect();
                let even = body && {
                    *body_ix += 1;
                    (*body_ix).is_multiple_of(2)
                };
                rows.push((even, cells));
            }
        }
        for c in &e.children {
            if let Node::El(g) = c {
                match g.tag.as_str() {
                    "thead" => push_rows(g, &mut rows, false, &mut body_ix),
                    "tbody" | "tfoot" => push_rows(g, &mut rows, true, &mut body_ix),
                    "tr" => push_rows(e, &mut rows, true, &mut body_ix),
                    _ => {}
                }
            }
        }
        let cols = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        // Cell texts and their min/max content widths.
        let mut texts: Vec<Vec<Option<(usize, Text)>>> = Vec::new();
        let mut min_w = vec![0f32; cols];
        let mut max_w = vec![0f32; cols];
        for (_, cells) in &rows {
            let mut row = Vec::new();
            for (ci, (head, cell)) in cells.iter().enumerate() {
                let cf = Flow {
                    weight: if *head { 600. } else { flow.weight },
                    ink: if *head { Ink::Text } else { flow.ink },
                    ..tf
                };
                let inl = inline::collect(
                    &cell.children,
                    InlineCx {
                        ink: cf.ink,
                        strong_weight: sh.strong_weight,
                        em_muted: sh.em_muted,
                        weight: cf.weight,
                    },
                );
                self.n.block += 1;
                let text = self.text(&inl, cf, self.n.block);
                let tid = self.n.text;
                let (maxc, _, _) = text.measure(None, self.window);
                let (minc, _, _) = text.measure(Some(0.), self.window);
                let extra = 2. * ph + 1.;
                max_w[ci] = max_w[ci].max(maxc + extra);
                min_w[ci] = min_w[ci].max(minc + extra);
                row.push(Some((tid, text)));
            }
            texts.push(row);
        }
        let avail = (self.env.width - 1.).max(0.);
        let (sum_min, sum_max): (f32, f32) = (min_w.iter().sum(), max_w.iter().sum());
        let widths: Vec<f32> = if sum_max <= avail || sum_max <= sum_min {
            max_w.clone()
        } else if avail <= sum_min {
            min_w.clone()
        } else {
            let k = (avail - sum_min) / (sum_max - sum_min);
            (0..cols)
                .map(|i| lu(min_w[i] + (max_w[i] - min_w[i]) * k))
                .collect()
        };
        let line = t.line;
        let nrows = rows.len();
        let mut row_h = vec![0f32; nrows];
        let mut built: Vec<Vec<Div>> = Vec::new();
        for (ri, ((even, cells), row_texts)) in rows.iter().zip(texts).enumerate() {
            let mut row: Vec<Div> = Vec::new();
            let ncells = cells.len();
            for (ci, ((head, _), text)) in cells.iter().zip(row_texts).enumerate() {
                let w = widths.get(ci).copied().unwrap_or(0.);
                let bg = if *head {
                    Some(t.pane)
                } else if *even {
                    Some(t.pane.fade(0.45))
                } else {
                    None
                };
                let (last_col, last_row) = (ci + 1 == ncells, ri + 1 == nrows);
                // Collapsed borders: 1px, centred on the cell's grid lines, snapped as WebKit
                // snaps border rects.
                let edges = canvas(
                    |_, _, _| {},
                    move |b: Bounds<Pixels>, _, window, _| {
                        let s = window.scale_factor();
                        let snap = |v: f32| (v * s).round() / s;
                        let (x0, y0) = (f32::from(b.left()), f32::from(b.top()));
                        let (x1, y1) = (f32::from(b.right()), f32::from(b.bottom()));
                        let mut rect = |l: f32, t: f32, r: f32, bt: f32| {
                            let r = Bounds::from_corners(
                                point(px(snap(l)), px(snap(t))),
                                point(px(snap(r)), px(snap(bt))),
                            );
                            window.paint_quad(gpui_kit::fill(r, hsla(line)));
                        };
                        rect(x0 - 0.5, y0 - 0.5, x0 + 0.5, y1 + 0.5);
                        rect(x0 - 0.5, y0 - 0.5, x1 + 0.5, y0 + 0.5);
                        if last_col {
                            rect(x1 - 0.5, y0 - 0.5, x1 + 0.5, y1 + 0.5);
                        }
                        if last_row {
                            rect(x0 - 0.5, y1 - 0.5, x1 + 0.5, y1 + 0.5);
                        }
                    },
                )
                .absolute()
                .inset_0();
                let inner_w = (w - 2. * ph - 1.).max(0.);
                let text_h = text
                    .as_ref()
                    .map_or(0., |(_, t)| t.measure(Some(inner_w), self.window).1);
                let text = text.map(|(id, t)| self.tracked_id(id, t));
                row_h[ri] = row_h[ri].max(text_h);
                let cell = div()
                    .relative()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .when_some(bg, |d, c| d.bg(hsla(c)))
                    .child(hgap(w, self.window).mb(px(-1. / self.window.scale_factor())))
                    .child(
                        div()
                            .absolute()
                            .top(px(pv + 0.5))
                            .left(px(ph + 0.5))
                            .right(px(ph + 0.5))
                            .children(text),
                    )
                    .child(edges);
                row.push(cell);
            }
            built.push(row);
        }
        let mut grid = div().flex().flex_col();
        for (ri, cells) in built.into_iter().enumerate() {
            let h = row_h[ri] + 2. * pv + 1.;
            let mut row = div().flex().flex_row();
            for c in cells {
                row = row.child(c.child(vgap(h, self.window)));
            }
            grid = grid.child(row);
        }
        let table = div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .child(vgap(0.5, self.window))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .child(hgap(0.5, self.window))
                    .child(grid)
                    .child(hgap(0.5, self.window)),
            )
            .child(vgap(0.5, self.window));
        self.n.table += 1;
        let el = div()
            .id(("md-table", self.n.table))
            .overflow_x_scroll()
            .flex()
            .flex_row()
            .child(table);
        Laid {
            el: el.into_any_element(),
            mt: lu(sh.table_margin.0 * size),
            mb: lu(sh.table_margin.1 * size),
        }
    }
}

#[derive(Clone, Copy)]
struct Pos<'p> {
    first: bool,
    last: bool,
    parent: &'p str,
}

/// The strut of a block: line-box space above and below the baseline.
fn strut(flow: Flow, window: &Window) -> (f32, f32) {
    let (_, _, m) = recur_ui::text("x")
        .ui()
        .size(flow.size)
        .leading(flow.leading)
        .measure(None, window);
    (m.baseline, m.height - m.baseline)
}

fn space_width(flow: Flow, window: &Window) -> f32 {
    let w = |s: &str| {
        recur_ui::text(s.to_string())
            .ui()
            .size(flow.size)
            .nowrap()
            .measure(None, window)
            .0
    };
    w("x\u{a0}x") - w("xx")
}

/// Text runs of preformatted content with their ink.
fn collect_pre(nodes: &[Node], base: Ink, f: &mut dyn FnMut(&str, Ink)) {
    for n in nodes {
        match n {
            Node::Text(t) => f(t, base),
            Node::El(e) => {
                let mut ink_ = base;
                if let Some(c) = e.attr("class") {
                    for cls in c.split_ascii_whitespace() {
                        if cls == "sh-prompt" {
                            ink_ = Ink::Faint;
                        } else if let Some(col) = super::style::sh_color(cls) {
                            let [r, g, b, _] = col.to_rgba8();
                            ink_ = Ink::Fixed((r as u32) << 16 | (g as u32) << 8 | b as u32);
                        }
                    }
                }
                collect_pre(&e.children, ink_, f);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Disc,
    Circle,
    Square,
}

/// An outside `disc` / `circle` / `square` marker as WebKit synthesizes it: a shape sized from
/// the font's integer ascent, in a marker box `ascent * 2 / 3 + 8` before the content edge.
fn symbol_marker(shape: Shape, flow: Flow, color: Color, window: &Window) -> AnyElement {
    let (_, _, m) = recur_ui::text("x")
        .ui()
        .size(flow.size)
        .leading(flow.leading)
        .measure(None, window);
    // Margins come from the integer ascent; the shape from the float one.
    let ascent = m.ascent;
    let offset = (ascent.round() as i32 * 2 / 3) as f32;
    let bullet = (ascent * 2. / 3. + 1.) / 2.;
    let left = -(offset + 7. + 1.) + 1.;
    let top = m.baseline - ascent + 3. * (ascent - ascent * 2. / 3.) / 2.;
    let c = hsla(color);
    canvas(
        |_, _, _| {},
        move |b: Bounds<Pixels>, _, window, _| {
            let o = b.origin;
            let r = Bounds {
                origin: point(o.x + px(left), o.y + px(top)),
                size: gpui_kit::size(px(bullet), px(bullet)),
            };
            match shape {
                Shape::Disc => {
                    window.paint_quad(gpui_kit::fill(r, c).corner_radii(px(bullet / 2.)))
                }
                Shape::Square => window.paint_quad(gpui_kit::fill(r, c)),
                Shape::Circle => window.paint_quad(
                    gpui_kit::outline(r.dilate(px(0.5)), c, gpui_kit::BorderStyle::Solid)
                        .corner_radii(px(bullet / 2. + 0.5)),
                ),
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// Stack blocks with collapsed sibling margins. Returns the column and the outer margins of
/// its first and last block, which collapse through a parent without padding or border.
pub fn stack(blocks: Vec<Laid>, window: &Window) -> (Div, f32, f32) {
    let mut col = div().flex().flex_col();
    let mut prev: Option<f32> = None;
    let (mut first, mut last) = (0., 0.);
    let n = blocks.len();
    for (i, b) in blocks.into_iter().enumerate() {
        match prev {
            None => first = b.mt,
            Some(p) => col = col.child(vgap(p.max(b.mt), window)),
        }
        prev = Some(b.mb);
        if i + 1 == n {
            last = b.mb;
        }
        col = col.child(b.el);
    }
    (col, first, last)
}

/// `mdcopy.ts`'s copied check.
const CHECK_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12l4 4L19 6"/></svg>"#;

/// The native checkmark, in the control's 12px box.
const CHECKBOX_MARK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 6.1 5 8.6 9.7 2.7"/></svg>"#;

/// A disabled task checkbox as WebKit's native control paints it: a 12px rounded box (and its
/// check) at half opacity. `top` is the border box's offset in the block.
fn checkbox_box(checked: bool, top: f32, t: &Tokens) -> AnyElement {
    let fill = recur_ui::rgba(255, 255, 255, 0.5).over(t.bg);
    let edge = recur_ui::rgba(255, 255, 255, 0.36).over(t.bg);
    let check = recur_ui::rgba(153, 153, 153, 0.625).over(t.bg);
    div()
        .absolute()
        .left(px(2.))
        .top(px(top))
        .size(px(12.))
        .rounded(px(3.))
        .border(px(0.5))
        .border_color(hsla(edge))
        .bg(hsla(fill))
        .when(checked, |d| {
            d.child(
                recur_ui::svg_markup(CHECKBOX_MARK.replace("currentColor", &check.to_css()))
                    .size(px(12.)),
            )
        })
        .into_any_element()
}
