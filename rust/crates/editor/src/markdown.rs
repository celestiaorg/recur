//! LSP documentation as `.cm-lsp-documentation` renders it: markdown
//! through `marked` (GFM), plaintext escaped, fenced code tinted by
//! `codehl` (`sanitizeLspHTML`), laid out with the UA stylesheet's block
//! margins (collapsed as CSS collapses them) and `_codemirror.scss`.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{AnyElement, Div, Hsla, IntoElement, ParentElement, Styled, div, px};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
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
        }
    }
}

/// The documentation's `line-height`.
const LEADING: f32 = 1.45;

#[derive(Clone, Copy, Default, PartialEq)]
struct Inline {
    code: bool,
    strong: bool,
    em: bool,
    strike: bool,
    link: bool,
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

struct Builder<'a> {
    events: std::iter::Peekable<Parser<'a>>,
    /// What a soft line break becomes (`" "`, or `"\n"` under pre-line).
    soft_break: &'static str,
}

impl Builder<'_> {
    /// Blocks until `end` (or the end of input).
    fn blocks(&mut self, end: Option<TagEnd>) -> Vec<Block> {
        let mut out = Vec::new();
        let mut loose = Para::default();
        let mut st = Inline::default();
        while let Some(ev) = self.events.next() {
            match ev {
                Event::End(e) if Some(e) == end => break,
                Event::Start(Tag::Paragraph) => {
                    out.push(Block::Para(self.inline(TagEnd::Paragraph)))
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    out.push(Block::Heading(n, self.inline(TagEnd::Heading(level))));
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => {
                            format!("language-{}", info.split_whitespace().next().unwrap_or(""))
                        }
                        CodeBlockKind::Indented => String::new(),
                    };
                    let mut text = String::new();
                    for ev in self.events.by_ref() {
                        match ev {
                            Event::Text(t) => text.push_str(&t),
                            Event::End(TagEnd::CodeBlock) => break,
                            _ => {}
                        }
                    }
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    out.push(Block::Code { lang, text });
                }
                Event::Rule => out.push(Block::Rule),
                Event::Start(Tag::List(start)) => {
                    let mut items = Vec::new();
                    while let Some(ev) = self.events.next() {
                        match ev {
                            Event::Start(Tag::Item) => items.push(self.blocks(Some(TagEnd::Item))),
                            Event::End(TagEnd::List(_)) => break,
                            _ => {}
                        }
                    }
                    out.push(Block::List { start, items });
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    out.push(Block::Quote(self.blocks(Some(TagEnd::BlockQuote(None)))))
                }
                Event::Start(Tag::Table(_)) => {
                    let mut rows: Vec<Vec<Para>> = Vec::new();
                    while let Some(ev) = self.events.next() {
                        match ev {
                            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                                rows.push(Vec::new())
                            }
                            Event::Start(Tag::TableCell) => {
                                let cell = self.inline(TagEnd::TableCell);
                                if let Some(r) = rows.last_mut() {
                                    r.push(cell);
                                }
                            }
                            Event::End(TagEnd::Table) => break,
                            _ => {}
                        }
                    }
                    out.push(Block::Table(rows));
                }
                Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                    loose.push(&t, st)
                }
                Event::SoftBreak | Event::HardBreak => loose.push("\n", st),
                Event::Start(Tag::Strong) => st.strong = true,
                Event::End(TagEnd::Strong) => st.strong = false,
                _ => {}
            }
            if !loose.text.is_empty()
                && !matches!(
                    self.events.peek(),
                    Some(Event::Text(_)) | Some(Event::Code(_))
                )
            {
                out.push(Block::Para(std::mem::take(&mut loose)));
            }
        }
        if !loose.text.is_empty() {
            out.push(Block::Para(loose));
        }
        out
    }

    fn inline(&mut self, end: TagEnd) -> Para {
        let mut p = Para::default();
        let mut st = Inline::default();
        for ev in self.events.by_ref() {
            match ev {
                Event::End(e) if e == end => break,
                Event::Text(t) => p.push(&t, st),
                Event::Code(t) => p.push(&t, Inline { code: true, ..st }),
                Event::Html(t) | Event::InlineHtml(t)
                    if t.trim_start().to_ascii_lowercase().starts_with("<br") =>
                {
                    p.push("\n", st)
                }
                Event::SoftBreak => p.push(self.soft_break, st),
                Event::HardBreak => p.push("\n", st),
                Event::Start(Tag::Strong) => st.strong = true,
                Event::End(TagEnd::Strong) => st.strong = false,
                Event::Start(Tag::Emphasis) => st.em = true,
                Event::End(TagEnd::Emphasis) => st.em = false,
                Event::Start(Tag::Strikethrough) => st.strike = true,
                Event::End(TagEnd::Strikethrough) => st.strike = false,
                Event::Start(Tag::Link { .. }) => st.link = true,
                Event::End(TagEnd::Link) => st.link = false,
                _ => {}
            }
        }
        p
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
        if st.link {
            sp = sp.color(hsla(s.cyan)).underline(true);
        }
        out.push(sp);
    }
    out
}

fn para_text(p: &Para, s: &DocStyle, size: f32, color: Color, bold: bool) -> Text {
    let t = Text::from_spans(spans(p, s, size, bold))
        .ui()
        .size(size)
        .leading(LEADING)
        .color(hsla(color));
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
        Text::from_spans(runs)
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
        let t = recur_ui::text(doc.value.clone())
            .ui()
            .size(s.size)
            .leading(LEADING)
            .color(hsla(s.text));
        return if s.pre_line { t.pre_wrap() } else { t }.into_any_element();
    }
    render_markdown(&doc.value, s).into_any_element()
}

pub fn render_markdown(src: &str, s: &DocStyle) -> Div {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_GFM);
    let mut b = Builder {
        events: Parser::new_ext(src, opts).peekable(),
        soft_break: if s.pre_line { "\n" } else { " " },
    };
    let blocks = b.blocks(None);
    root(blocks, s)
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

    fn blocks(src: &str) -> Vec<Block> {
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        Builder {
            events: Parser::new_ext(src, opts).peekable(),
            soft_break: " ",
        }
        .blocks(None)
    }

    #[test]
    fn parses_blocks_and_inline_runs() {
        let b = blocks("Some `code` and **bold**\n\n```go\nx := 1\n```\n\n- a\n- b");
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
}
