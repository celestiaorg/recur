//! LSP documentation as `.cm-lsp-documentation` renders it: markdown
//! through `marked` (GFM), plaintext escaped, fenced code tinted by
//! `codehl` (`sanitizeLspHTML`).

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Div, Font, FontStyle, FontWeight, Hsla, IntoElement, ParentElement, Rgba,
    SharedString, StrikethroughStyle, Styled, StyledText, TextRun, UnderlineStyle, div, px, rems,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use recur_textmate::{ShKind, highlight_line_spans, lang_from_code_class};

use crate::autocomplete::Doc;
use crate::tokens::{AppTokens, mono_font, ui_font, with_font};

/// Colors and sizes for a documentation block.
#[derive(Clone)]
pub struct DocStyle {
    pub text: Hsla,
    pub muted: Hsla,
    pub line: Hsla,
    pub pane_2: Hsla,
    pub cyan: Hsla,
    /// Base font size (12.5px in hover and signature tooltips).
    pub size: f32,
}

impl DocStyle {
    pub fn from_tokens(t: &AppTokens, size: f32) -> DocStyle {
        let h = |c: Rgba| -> Hsla { c.into() };
        DocStyle {
            text: h(t.text),
            muted: h(t.muted),
            line: h(t.line),
            pane_2: h(t.pane_2),
            cyan: h(t.cyan),
            size,
        }
    }
}

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

fn sh_color(k: ShKind) -> Hsla {
    let c = match k {
        ShKind::Number => 0xd19a66,
        ShKind::Keyword => 0x6cb6ff,
        ShKind::String => 0x7ee08a,
        ShKind::Comment => 0x7fa9ad,
    };
    gpui_kit::rgb(c).into()
}

struct Builder<'a> {
    events: std::iter::Peekable<Parser<'a>>,
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
                Event::SoftBreak => p.push(" ", st),
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

fn para_text(p: Para, s: &DocStyle, color: Hsla, bold: bool) -> StyledText {
    let (ui, mono) = (ui_font(), mono_font());
    let runs = p
        .runs
        .iter()
        .map(|(len, st)| {
            let mut font: Font = if st.code { mono.clone() } else { ui.clone() };
            if st.strong || bold {
                font.weight = FontWeight::BOLD;
            }
            if st.em {
                font.style = FontStyle::Italic;
            }
            let color = if st.link { s.cyan } else { color };
            TextRun {
                len: *len,
                font,
                color,
                background_color: st.code.then_some(s.pane_2),
                underline: st.link.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(Hsla {
                        a: s.cyan.a * 0.45,
                        ..s.cyan
                    }),
                    wavy: false,
                }),
                strikethrough: st.strike.then_some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(color),
                }),
            }
        })
        .collect();
    StyledText::new(SharedString::from(p.text)).with_runs(runs)
}

fn code_block(lang: &str, text: &str, s: &DocStyle) -> Div {
    let id = lang_from_code_class(lang);
    let mono = mono_font();
    let mut lines = div().flex().flex_col();
    for line in text.split('\n') {
        let line = if line.is_empty() { " " } else { line };
        let runs = highlight_line_spans(line, id)
            .into_iter()
            .map(|(r, k)| TextRun {
                len: r.len(),
                font: mono.clone(),
                color: k.map_or(s.text, sh_color),
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect();
        lines = lines.child(StyledText::new(SharedString::from(line.to_string())).with_runs(runs));
    }
    with_font(div(), &mono)
        .mt(px(0.35 * s.size))
        .mb(px(0.55 * s.size))
        .py(px(8.))
        .px(px(10.))
        .rounded(px(6.))
        .bg(s.pane_2)
        .border_1()
        .border_color(s.line)
        .text_size(px(12.))
        .line_height(gpui_kit::relative(1.4))
        .text_color(s.text)
        .child(lines)
}

fn render_blocks(blocks: Vec<Block>, s: &DocStyle, top: bool) -> Div {
    let n = blocks.len();
    let mut out = div().flex().flex_col();
    for (i, b) in blocks.into_iter().enumerate() {
        let last = i + 1 == n;
        out = out.child(match b {
            Block::Para(p) => div()
                .when(!last || !top, |d| d.mb(px(0.55 * s.size)))
                .text_color(s.muted)
                .child(para_text(p, s, s.muted, false)),
            Block::Heading(level, p) => {
                let scale = [2.0, 1.5, 1.17, 1.0, 0.83, 0.67][(level as usize).clamp(1, 6) - 1];
                div()
                    .text_size(px(s.size * scale))
                    .my(px(s.size * scale * 0.67))
                    .child(para_text(p, s, s.text, true))
            }
            Block::Code { lang, text } => {
                code_block(&lang, &text, s).when(last && top, |d| d.mb(px(0.)))
            }
            Block::Rule => div().w_full().my(px(0.7 * s.size)).h(px(1.)).bg(s.line),
            Block::List { start, items } => {
                let mut list = div().flex().flex_col().pl(rems(2.5)).my(px(s.size));
                for (k, item) in items.into_iter().enumerate() {
                    let marker = match start {
                        Some(n) => format!("{}.", n + k as u64),
                        None => "•".to_string(),
                    };
                    list = list.child(
                        div()
                            .flex()
                            .child(
                                div()
                                    .absolute()
                                    .ml(px(-18.))
                                    .text_color(s.muted)
                                    .child(marker),
                            )
                            .child(render_blocks(item, s, false)),
                    );
                }
                list
            }
            Block::Quote(inner) => div()
                .mx(rems(2.5))
                .my(px(s.size))
                .child(render_blocks(inner, s, false)),
            Block::Table(rows) => {
                let mut table = div().flex().flex_col().my(px(0.55 * s.size));
                for (r, row) in rows.into_iter().enumerate() {
                    let mut tr = div().flex().gap(px(8.));
                    for cell in row {
                        tr = tr.child(div().flex_1().child(para_text(
                            cell,
                            s,
                            if r == 0 { s.text } else { s.muted },
                            r == 0,
                        )));
                    }
                    table = table.child(tr);
                }
                table
            }
        });
    }
    out
}

/// Render a [`Doc`] as `docToHTML` would.
pub fn render_doc(doc: &Doc, s: &DocStyle) -> AnyElement {
    if !doc.markdown {
        return div()
            .whitespace_normal()
            .child(doc.value.clone())
            .into_any_element();
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
    };
    let blocks = b.blocks(None);
    render_blocks(blocks, s, true)
}

/// Code in a language (`renderCode` for `MarkedString {language}`): a
/// fenced block.
pub fn render_code(language: &str, value: &str, s: &DocStyle) -> AnyElement {
    code_block(&format!("language-{language}"), value, s)
        .mb(px(0.))
        .into_any_element()
}
