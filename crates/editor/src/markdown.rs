//! LSP documentation as `.cm-lsp-documentation` renders it: markdown
//! through `marked` (GFM), plaintext escaped, fenced code tinted by
//! `codehl`, laid out with the UA stylesheet's block margins (collapsed as
//! CSS collapses them).

use std::cell::Cell;
use std::rc::Rc;

use comrak::nodes::{ListType, NodeValue};
use comrak::{Arena, Node, Options};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Div, Hsla, IntoElement, ParentElement, SharedString, Styled, div, px,
};
use recur_textmate::{highlight_line_spans, lang_from_code_class};
use recur_ui::text::{Align, InlineBox};
use recur_ui::{Color, Span, Text, TextStyled, Theme, span, style::card};

use crate::autocomplete::Doc;

/// Colors and the base size of a documentation block.
#[derive(Clone)]
pub struct DocStyle {
    pub text: Color,
    pub muted: Color,
    pub line: Color,
    pub pane_2: Color,
    pub cyan: Color,
    /// Base font size (12.5px in hover and completion docs, 10px in
    /// signature docs).
    pub size: f32,
    /// Inside `.cm-completionInfo` (`white-space: pre-line`): source line
    /// breaks in paragraphs stay breaks.
    pub pre_line: bool,
    /// Hands out selection keys when the text is selectable (in a
    /// `recur_ui::selection` scope).
    pub select: Option<Rc<Cell<u64>>>,
}

impl DocStyle {
    pub fn from_theme(theme: &Theme, size: f32) -> DocStyle {
        let t = &theme.tokens;
        DocStyle {
            text: t.text,
            muted: t.muted,
            line: t.line,
            pane_2: t.pane_2,
            cyan: t.cyan,
            size,
            pre_line: false,
            select: None,
        }
    }

    pub(crate) fn selectable(&self, t: Text) -> Text {
        let Some(keys) = &self.select else { return t };
        let key = keys.get();
        keys.set(key + 1);
        t.selectable(key, "\n")
    }
}

/// The documentation's `line-height`.
const LEADING: f32 = 1.45;

#[derive(Clone, Default, PartialEq)]
struct Inline {
    code: bool,
    strong: bool,
    em: bool,
    strike: bool,
    link: Option<SharedString>,
}

#[derive(Default)]
struct Para {
    text: String,
    runs: Vec<(usize, Inline)>,
}

impl Para {
    fn push(&mut self, s: &str, st: Inline) {
        if s.is_empty() {
            return;
        }
        match self.runs.last_mut() {
            Some((len, last)) if *last == st => *len += s.len(),
            _ => self.runs.push((s.len(), st)),
        }
        self.text.push_str(s);
    }
}

enum Block {
    Para(Para),
    Heading(u8, Para),
    Code {
        lang: String,
        text: String,
    },
    Rule,
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Table(Vec<Vec<Para>>),
}

/// Blocks from GFM markdown (tables, strikethrough, autolinks). `soft_break` is what a soft
/// line break becomes (`" "`, or `"\n"` under pre-line).
fn parse(src: &str, soft_break: &'static str) -> Vec<Block> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    let arena = Arena::new();
    blocks(comrak::parse_document(&arena, src, &o), soft_break)
}

fn blocks(parent: Node<'_>, sb: &'static str) -> Vec<Block> {
    parent.children().filter_map(|n| block(n, sb)).collect()
}

fn block(n: Node<'_>, sb: &'static str) -> Option<Block> {
    Some(match &n.data().value {
        NodeValue::Paragraph => Block::Para(inline(n, sb)),
        NodeValue::Heading(h) => Block::Heading(h.level, inline(n, sb)),
        NodeValue::CodeBlock(c) => Block::Code {
            lang: if c.fenced {
                format!(
                    "language-{}",
                    c.info.split_whitespace().next().unwrap_or("")
                )
            } else {
                String::new()
            },
            text: c
                .literal
                .strip_suffix('\n')
                .unwrap_or(&c.literal)
                .to_string(),
        },
        NodeValue::ThematicBreak => Block::Rule,
        NodeValue::List(l) => Block::List {
            start: (l.list_type == ListType::Ordered).then_some(l.start as u64),
            items: n.children().map(|item| blocks(item, sb)).collect(),
        },
        NodeValue::BlockQuote => Block::Quote(blocks(n, sb)),
        NodeValue::Table(_) => Block::Table(
            n.children()
                .map(|row| row.children().map(|cell| inline(cell, sb)).collect())
                .collect(),
        ),
        NodeValue::HtmlBlock(h) => {
            let mut p = Para::default();
            p.push(h.literal.trim_end_matches('\n'), Inline::default());
            Block::Para(p)
        }
        _ => return None,
    })
}

fn inline(n: Node<'_>, sb: &'static str) -> Para {
    let mut p = Para::default();
    runs(n, &Inline::default(), sb, &mut p);
    p
}

fn runs(n: Node<'_>, st: &Inline, sb: &'static str, p: &mut Para) {
    for c in n.children() {
        let with = |f: fn(&mut Inline)| {
            let mut st = st.clone();
            f(&mut st);
            st
        };
        match &c.data().value {
            NodeValue::Text(t) => p.push(t, st.clone()),
            NodeValue::Code(code) => p.push(&code.literal, with(|s| s.code = true)),
            NodeValue::HtmlInline(h) if h.trim_start().to_ascii_lowercase().starts_with("<br") => {
                p.push("\n", st.clone())
            }
            NodeValue::SoftBreak => p.push(sb, st.clone()),
            NodeValue::LineBreak => p.push("\n", st.clone()),
            NodeValue::Strong => runs(c, &with(|s| s.strong = true), sb, p),
            NodeValue::Emph => runs(c, &with(|s| s.em = true), sb, p),
            NodeValue::Strikethrough => runs(c, &with(|s| s.strike = true), sb, p),
            NodeValue::Link(l) => {
                let st = Inline {
                    link: Some(l.url.clone().into()),
                    ..st.clone()
                };
                runs(c, &st, sb, p)
            }
            NodeValue::Image(_) => runs(c, st, sb, p),
            _ => {}
        }
    }
}

fn hsla(c: Color) -> Hsla {
    c.into()
}

/// Inline runs: `code` is mono at .95em in a padded, bordered box; links
/// are cyan with their `border-bottom` (drawn as an underline).
/// simplify: recur-ui inline boxes take one border for all sides, so the
/// link's bottom-only border is an underline; swap for a per-side inline
/// border when recur-ui grows one.
fn spans(p: &Para, s: &DocStyle, size: f32, bold: bool) -> Vec<Span> {
    let mut out = Vec::new();
    let mut off = 0;
    for (len, st) in &p.runs {
        let t = p.text[off..off + len].to_string();
        off += len;
        let mut sp = span(t);
        if st.code {
            let cs = 0.95 * size;
            sp = sp.mono().size(cs).boxed(InlineBox {
                pad_top: 0.1 * cs,
                pad_bottom: 0.1 * cs,
                pad_left: 0.35 * cs,
                pad_right: 0.35 * cs,
                border: 1.,
                border_color: hsla(s.line),
                background: Some(hsla(s.pane_2)),
                radius: 4.,
                ..InlineBox::default()
            });
        }
        if st.strong || bold {
            sp = sp.weight(700.);
        }
        if st.em {
            sp = sp.italic();
        }
        if st.strike {
            sp = sp.line_through(true);
        }
        if st.link.is_some() {
            sp = sp.color(hsla(s.cyan)).underline(true);
        }
        out.push(sp);
    }
    out
}

/// The LSP client opens http(s) links from LSP docs in the browser and leaves
/// the rest alone.
fn open_link(href: &str, cx: &mut App) {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        cx.open_url(href);
    }
}

fn para_text(p: &Para, s: &DocStyle, size: f32, color: Color, bold: bool) -> Text {
    let mut t = Text::from_spans(spans(p, s, size, bold))
        .ui()
        .size(size)
        .leading(LEADING)
        .color(hsla(color));
    for (i, (_, st)) in p.runs.iter().enumerate() {
        if let Some(href) = st.link.clone() {
            t = t.on_click_span(i, move |_, cx| open_link(&href, cx));
        }
    }
    let t = s.selectable(t);
    if s.pre_line || p.text.contains('\n') {
        t.pre_wrap()
    } else {
        t
    }
}

/// A block with its outer vertical margins (they collapse with siblings).
struct Laid {
    el: AnyElement,
    mt: f32,
    mb: f32,
}

/// `.cm-lsp-documentation pre`: 12px mono, line-height 1.4, pre-wrap; the
/// `code` inside is .95em.
fn code_block(lang: &str, text: &str, s: &DocStyle) -> Div {
    let id = lang_from_code_class(lang);
    let mut runs = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            runs.push(span("\n").size(12. * 0.95));
        }
        for (r, k) in highlight_line_spans(line, id) {
            let color = k.map_or(s.text, |k| {
                crate::color::ui(crate::theme::doc_code_token(k))
            });
            runs.push(
                span(line[r].to_string())
                    .size(12. * 0.95)
                    .color(hsla(color)),
            );
        }
    }
    if runs.is_empty() {
        runs.push(span(""));
    }
    card(s.pane_2, s.line, 6.).py(px(8.)).px(px(10.)).child(
        s.selectable(Text::from_spans(runs))
            .mono()
            .size(12.)
            .leading(1.4)
            .color(hsla(s.text))
            .pre_wrap()
            .break_anywhere(),
    )
}

/// UA `h1`–`h6`: font size and block margins, in em of the heading.
fn heading_metrics(level: u8) -> (f32, f32) {
    match level {
        1 => (2.0, 0.67),
        2 => (1.5, 0.83),
        3 => (1.17, 1.0),
        4 => (1.0, 1.33),
        5 => (0.83, 1.67),
        _ => (0.67, 2.33),
    }
}

fn lay_block(b: Block, s: &DocStyle, last: bool, nested_list: bool) -> Laid {
    let size = s.size;
    match b {
        Block::Para(p) => Laid {
            el: para_text(&p, s, size, s.muted, false).into_any_element(),
            mt: 0.,
            mb: if last { 0. } else { 0.55 * size },
        },
        Block::Heading(level, p) => {
            let (scale, margin) = heading_metrics(level);
            let fs = size * scale;
            Laid {
                el: para_text(&p, s, fs, s.text, true).into_any_element(),
                mt: margin * fs,
                mb: margin * fs,
            }
        }
        Block::Code { lang, text } => Laid {
            el: code_block(&lang, &text, s).into_any_element(),
            mt: 0.35 * 12.,
            mb: if last { 0. } else { 0.55 * 12. },
        },
        Block::Rule => Laid {
            el: div()
                .w_full()
                .border_t_1()
                .border_color(hsla(s.line))
                .into_any_element(),
            mt: 0.7 * size,
            mb: 0.7 * size,
        },
        Block::List { start, items } => {
            let margin = if nested_list { 0. } else { size };
            let n = items.len();
            let mut list = div().flex().flex_col().pl(px(40.));
            let mut prev_mb: Option<f32> = None;
            let (mut first_mt, mut last_mb) = (0., 0.);
            for (k, item) in items.into_iter().enumerate() {
                let marker = match start {
                    Some(first) => format!("{}. ", first + k as u64),
                    None => "• ".to_string(),
                };
                let (body, mt, mb) = lay_blocks(item, s, true);
                match prev_mb {
                    None => first_mt = mt,
                    Some(p) => list = list.child(div().h(px(p.max(mt)))),
                }
                prev_mb = Some(mb);
                if k + 1 == n {
                    last_mb = mb;
                }
                list = list.child(
                    div()
                        .relative()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left(px(-40.))
                                .w(px(40.))
                                .flex()
                                .justify_end()
                                .child(
                                    recur_ui::text(marker)
                                        .ui()
                                        .size(size)
                                        .leading(LEADING)
                                        .color(hsla(s.muted))
                                        .pre_wrap()
                                        .align(Align::Right),
                                ),
                        )
                        .child(body),
                );
            }
            Laid {
                el: list.into_any_element(),
                mt: margin.max(first_mt),
                mb: margin.max(last_mb),
            }
        }
        Block::Quote(inner) => {
            let (body, mt, mb) = lay_blocks(inner, s, false);
            Laid {
                el: div().mx(px(40.)).child(body).into_any_element(),
                mt: size.max(mt),
                mb: size.max(mb),
            }
        }
        Block::Table(rows) => {
            let mut table = div().flex().flex_col().gap(px(2.)).p(px(2.));
            for (r, row) in rows.into_iter().enumerate() {
                let mut tr = div().flex().gap(px(2.));
                for cell in row {
                    let head = r == 0;
                    let t = para_text(&cell, s, size, if head { s.text } else { s.muted }, head)
                        .when(head, |t| t.align(Align::Center));
                    tr = tr.child(div().flex_1().p(px(1.)).child(t));
                }
                table = table.child(tr);
            }
            Laid {
                el: table.into_any_element(),
                mt: 0.,
                mb: 0.,
            }
        }
    }
}

/// Blocks in one parent with collapsed sibling margins; returns the body
/// and the outer margins of its first and last child (which collapse
/// through a parent without padding).
fn lay_blocks(blocks: Vec<Block>, s: &DocStyle, in_list: bool) -> (Div, f32, f32) {
    let n = blocks.len();
    let mut out = div().flex().flex_col();
    let mut prev_mb: Option<f32> = None;
    let (mut first_mt, mut last_mb) = (0., 0.);
    for (i, b) in blocks.into_iter().enumerate() {
        let last = i + 1 == n;
        let laid = lay_block(b, s, last, in_list);
        match prev_mb {
            None => first_mt = laid.mt,
            Some(p) => out = out.child(div().flex_none().h(px(p.max(laid.mt)))),
        }
        prev_mb = Some(laid.mb);
        if last {
            last_mb = laid.mb;
        }
        out = out.child(laid.el);
    }
    (out, first_mt, last_mb)
}

/// The documentation root: margins of its first and last block stay
/// inside the padded tooltip.
fn root(blocks: Vec<Block>, s: &DocStyle) -> Div {
    let (body, mt, mb) = lay_blocks(blocks, s, false);
    div()
        .flex()
        .flex_col()
        .when(mt > 0., |d| d.child(div().flex_none().h(px(mt))))
        .child(body)
        .when(mb > 0., |d| d.child(div().flex_none().h(px(mb))))
}

/// Render a [`Doc`] as `docToHTML` would.
pub fn render_doc(doc: &Doc, s: &DocStyle) -> AnyElement {
    if !doc.markdown {
        let t = s
            .selectable(recur_ui::text(doc.value.clone()))
            .ui()
            .size(s.size)
            .leading(LEADING)
            .color(hsla(s.text));
        return if s.pre_line { t.pre_wrap() } else { t }.into_any_element();
    }
    render_markdown(&doc.value, s).into_any_element()
}

pub fn render_markdown(src: &str, s: &DocStyle) -> Div {
    root(parse(src, if s.pre_line { "\n" } else { " " }), s)
}

/// Code in a language (`renderCode` for `MarkedString {language}`): a
/// fenced block on its own (`pre:last-child`).
pub fn render_code(language: &str, value: &str, s: &DocStyle) -> AnyElement {
    root(
        vec![Block::Code {
            lang: format!("language-{language}"),
            text: value.to_string(),
        }],
        s,
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_and_inline_runs() {
        let b = parse(
            "Some `code` and **bold**\n\n```go\nx := 1\n```\n\n- a\n- b",
            " ",
        );
        assert_eq!(b.len(), 3);
        let Block::Para(p) = &b[0] else { panic!() };
        assert_eq!(p.text, "Some code and bold");
        assert!(p.runs.iter().any(|(_, s)| s.code));
        assert!(p.runs.iter().any(|(_, s)| s.strong));
        assert!(
            matches!(&b[1], Block::Code { lang, text } if lang == "language-go" && text == "x := 1")
        );
        assert!(matches!(&b[2], Block::List { start: None, items } if items.len() == 2));
        assert_eq!(heading_metrics(2), (1.5, 0.83));
    }

    #[test]
    fn nests_inline_styles_links_and_tables() {
        let b = parse(
            "*em **both** ~~gone~~* [doc](https://x.dev) see https://y.dev\nnext<br>line\n\n\
             1. one\n   - inner\n\n| h | i |\n|---|---|\n| c | d |",
            "\n",
        );
        assert_eq!(b.len(), 3);
        let Block::Para(p) = &b[0] else { panic!() };
        assert_eq!(p.text, "em both gone doc see https://y.dev\nnext\nline");
        let run = |needle: &str| {
            let at = p.text.find(needle).unwrap();
            let mut off = 0;
            p.runs
                .iter()
                .find(|(len, _)| {
                    off += len;
                    off > at
                })
                .unwrap()
                .1
                .clone()
        };
        assert!(run("both").em && run("both").strong);
        assert!(run("gone").em && run("gone").strike && !run("gone").strong);
        assert_eq!(run("doc").link.as_deref(), Some("https://x.dev"));
        assert_eq!(run("https://y.dev").link.as_deref(), Some("https://y.dev"));
        let Block::List { start, items } = &b[1] else {
            panic!()
        };
        assert_eq!(*start, Some(1));
        assert!(matches!(
            &items[0][..],
            [Block::Para(_), Block::List { start: None, .. }]
        ));
        let Block::Table(rows) = &b[2] else { panic!() };
        let cells: Vec<Vec<&str>> = rows
            .iter()
            .map(|r| r.iter().map(|c| c.text.as_str()).collect())
            .collect();
        assert_eq!(cells, [["h", "i"], ["c", "d"]]);
    }
}
