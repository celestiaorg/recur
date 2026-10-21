//! `Lexer.ts` and `Tokenizer.ts`: block lexing, the deferred inline queue and inline
//! lexing, with every tokenizer rule as marked 18 applies it.
//!
//! Offsets are UTF-8 bytes where marked counts UTF-16 units. The masked source keeps its
//! byte layout aligned with the source (marked keeps UTF-16 alignment), so delimiter
//! offsets found in one index the other.

use std::collections::HashMap;

use super::helpers::{
    expand_tabs, find_closing_bracket, rtrim, split_cells, trim_trailing_blank_lines,
};
use super::rules::{Rules, rules};
use crate::js::{self, utf16_len};

/// Table cell alignment (`align` on `th`/`td`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    pub fn as_str(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub tokens: Vec<Token>,
    pub inline: Option<usize>,
    pub header: bool,
    pub align: Option<Align>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub raw: String,
    pub task: bool,
    pub checked: Option<bool>,
    pub loose: bool,
    pub text: String,
    pub tokens: Vec<Token>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkDef {
    pub href: String,
    pub title: Option<String>,
}

/// A marked token. `inline` is the deferred inline-queue slot a block token's `tokens` fill
/// from once block lexing ends (`lexer.inline(src)`).
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Space {
        raw: String,
    },
    Code {
        raw: String,
        lang: Option<String>,
        text: String,
    },
    Heading {
        raw: String,
        depth: usize,
        text: String,
        tokens: Vec<Token>,
        inline: Option<usize>,
    },
    Hr {
        raw: String,
    },
    Blockquote {
        raw: String,
        text: String,
        tokens: Vec<Token>,
    },
    List {
        raw: String,
        ordered: bool,
        start: Option<u64>,
        loose: bool,
        items: Vec<ListItem>,
    },
    Checkbox {
        raw: String,
        checked: bool,
    },
    Html {
        raw: String,
        text: String,
        block: bool,
    },
    Def {
        raw: String,
        tag: String,
        href: String,
        title: Option<String>,
    },
    Table {
        raw: String,
        header: Vec<Cell>,
        align: Vec<Option<Align>>,
        rows: Vec<Vec<Cell>>,
    },
    Paragraph {
        raw: String,
        text: String,
        tokens: Vec<Token>,
        inline: Option<usize>,
    },
    Text {
        raw: String,
        text: String,
        /// Block text has inline tokens; inline text does not.
        tokens: Option<Vec<Token>>,
        inline: Option<usize>,
        escaped: bool,
    },
    Escape {
        raw: String,
        text: String,
    },
    Link {
        raw: String,
        href: String,
        title: Option<String>,
        text: String,
        tokens: Vec<Token>,
    },
    Image {
        raw: String,
        href: String,
        title: Option<String>,
        text: String,
        tokens: Vec<Token>,
    },
    Strong {
        raw: String,
        text: String,
        tokens: Vec<Token>,
    },
    Em {
        raw: String,
        text: String,
        tokens: Vec<Token>,
    },
    Codespan {
        raw: String,
        text: String,
    },
    Br {
        raw: String,
    },
    Del {
        raw: String,
        text: String,
        tokens: Vec<Token>,
    },
}

impl Token {
    pub fn raw(&self) -> &str {
        match self {
            Token::Space { raw }
            | Token::Code { raw, .. }
            | Token::Heading { raw, .. }
            | Token::Hr { raw }
            | Token::Blockquote { raw, .. }
            | Token::List { raw, .. }
            | Token::Checkbox { raw, .. }
            | Token::Html { raw, .. }
            | Token::Def { raw, .. }
            | Token::Table { raw, .. }
            | Token::Paragraph { raw, .. }
            | Token::Text { raw, .. }
            | Token::Escape { raw, .. }
            | Token::Link { raw, .. }
            | Token::Image { raw, .. }
            | Token::Strong { raw, .. }
            | Token::Em { raw, .. }
            | Token::Codespan { raw, .. }
            | Token::Br { raw }
            | Token::Del { raw, .. } => raw,
        }
    }

    fn raw_mut(&mut self) -> &mut String {
        match self {
            Token::Space { raw }
            | Token::Code { raw, .. }
            | Token::Heading { raw, .. }
            | Token::Hr { raw }
            | Token::Blockquote { raw, .. }
            | Token::List { raw, .. }
            | Token::Checkbox { raw, .. }
            | Token::Html { raw, .. }
            | Token::Def { raw, .. }
            | Token::Table { raw, .. }
            | Token::Paragraph { raw, .. }
            | Token::Text { raw, .. }
            | Token::Escape { raw, .. }
            | Token::Link { raw, .. }
            | Token::Image { raw, .. }
            | Token::Strong { raw, .. }
            | Token::Em { raw, .. }
            | Token::Codespan { raw, .. }
            | Token::Br { raw }
            | Token::Del { raw, .. } => raw,
        }
    }

    fn text_mut(&mut self) -> Option<&mut String> {
        match self {
            Token::Code { text, .. }
            | Token::Heading { text, .. }
            | Token::Blockquote { text, .. }
            | Token::Html { text, .. }
            | Token::Paragraph { text, .. }
            | Token::Text { text, .. }
            | Token::Escape { text, .. }
            | Token::Link { text, .. }
            | Token::Image { text, .. }
            | Token::Strong { text, .. }
            | Token::Em { text, .. }
            | Token::Codespan { text, .. }
            | Token::Del { text, .. } => Some(text),
            _ => None,
        }
    }

    fn is_paragraph(&self) -> bool {
        matches!(self, Token::Paragraph { .. })
    }

    fn is_text(&self) -> bool {
        matches!(self, Token::Text { .. })
    }

    fn inline_slot(&self) -> Option<usize> {
        match self {
            Token::Paragraph { inline, .. }
            | Token::Heading { inline, .. }
            | Token::Text { inline, .. } => *inline,
            _ => None,
        }
    }
}

/// `inlineQueue` entry: the source for one deferred token list.
struct Queued {
    slot: usize,
    src: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError(pub String);

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LexError {}

pub struct Lexer {
    rules: &'static Rules,
    pub links: HashMap<String, LinkDef>,
    queue: Vec<Queued>,
    slots: Vec<Vec<Token>>,
    in_link: bool,
    in_raw_block: bool,
    top: bool,
    error: Option<LexError>,
}

impl Default for Lexer {
    fn default() -> Self {
        Lexer::new()
    }
}

/// `src.substring(n)` on a byte offset.
fn after(src: &str, n: usize) -> &str {
    &src[n.min(src.len())..]
}

impl Lexer {
    pub fn new() -> Lexer {
        Lexer {
            rules: rules(),
            links: HashMap::new(),
            queue: Vec::new(),
            slots: Vec::new(),
            in_link: false,
            in_raw_block: false,
            top: true,
            error: None,
        }
    }

    /// `Lexer.lex`: block tokens, then every queued inline source.
    pub fn lex(mut self, src: &str) -> Result<Vec<Token>, LexError> {
        let src = self.rules.other.carriage_return.replace(src, "\n");
        let mut tokens = Vec::new();
        self.block_tokens(&src, &mut tokens, false);
        let queue = std::mem::take(&mut self.queue);
        for q in &queue {
            let mut out = std::mem::take(&mut self.slots[q.slot]);
            self.inline_tokens(&q.src, &mut out);
            self.slots[q.slot] = out;
        }
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        self.resolve(&mut tokens);
        Ok(tokens)
    }

    /// Move each deferred slot into the token that owns it.
    fn resolve(&mut self, tokens: &mut [Token]) {
        for t in tokens {
            match t {
                Token::Paragraph { tokens, inline, .. } | Token::Heading { tokens, inline, .. } => {
                    if let Some(s) = inline.take() {
                        *tokens = std::mem::take(&mut self.slots[s]);
                    }
                }
                Token::Text {
                    tokens: Some(tokens),
                    inline,
                    ..
                } => {
                    if let Some(s) = inline.take() {
                        *tokens = std::mem::take(&mut self.slots[s]);
                    }
                }
                Token::Blockquote { tokens, .. } => self.resolve(tokens),
                Token::List { items, .. } => {
                    for item in items {
                        self.resolve(&mut item.tokens);
                    }
                }
                Token::Table { header, rows, .. } => {
                    for c in header.iter_mut().chain(rows.iter_mut().flatten()) {
                        if let Some(s) = c.inline.take() {
                            c.tokens = std::mem::take(&mut self.slots[s]);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn infinite_loop(&mut self, src: &str) {
        let byte = src.encode_utf16().next().unwrap_or(0);
        if self.error.is_none() {
            self.error = Some(LexError(format!("Infinite loop on byte: {byte}")));
        }
    }

    /// `lexer.inline(src)`: queue a deferred inline token list.
    fn inline(&mut self, src: &str) -> usize {
        let slot = self.slots.len();
        self.slots.push(Vec::new());
        self.queue.push(Queued {
            slot,
            src: src.to_string(),
        });
        slot
    }

    fn set_last_queued_src(&mut self, src: &str) {
        if let Some(q) = self.queue.last_mut() {
            q.src = src.to_string();
        }
    }

    pub fn block_tokens(
        &mut self,
        src: &str,
        tokens: &mut Vec<Token>,
        mut last_paragraph_clipped: bool,
    ) {
        let mut src = src;
        let mut src_length = usize::MAX;
        while !src.is_empty() {
            if src.len() < src_length {
                src_length = src.len();
            } else {
                self.infinite_loop(src);
                break;
            }

            // newline
            if let Some(raw) = self.space(src) {
                src = after(src, raw.len());
                match tokens.last_mut() {
                    Some(last) if raw.len() == 1 => last.raw_mut().push('\n'),
                    _ => tokens.push(Token::Space { raw }),
                }
                continue;
            }

            // code
            if let Some(t) = self.code(src) {
                src = after(src, t.raw().len());
                if let Some(last) = tokens
                    .last_mut()
                    .filter(|l| l.is_paragraph() || l.is_text())
                {
                    let Token::Code { raw, text, .. } = &t else {
                        unreachable!()
                    };
                    let sep = if last.raw().ends_with('\n') { "" } else { "\n" };
                    let r = last.raw_mut();
                    r.push_str(sep);
                    r.push_str(raw);
                    if let Some(lt) = last.text_mut() {
                        lt.push('\n');
                        lt.push_str(text);
                        let joined = lt.clone();
                        self.set_last_queued_src(&joined);
                    }
                } else {
                    tokens.push(t);
                }
                continue;
            }

            // fences
            if let Some(t) = self.fences(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }

            // heading
            if let Some(t) = self.heading(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }

            // hr
            if let Some(t) = self.hr(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }

            // blockquote
            if let Some(t) = self.blockquote(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }

            // list
            if let Some(t) = self.list(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }

            // html
            if let Some(t) = self.html(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }

            // def
            if let Some(t) = self.def(src) {
                src = after(src, t.raw().len());
                if let Some(last) = tokens
                    .last_mut()
                    .filter(|l| l.is_paragraph() || l.is_text())
                {
                    let sep = if last.raw().ends_with('\n') { "" } else { "\n" };
                    let r = last.raw_mut();
                    r.push_str(sep);
                    r.push_str(t.raw());
                    if let Some(lt) = last.text_mut() {
                        lt.push('\n');
                        lt.push_str(t.raw());
                        let joined = lt.clone();
                        self.set_last_queued_src(&joined);
                    }
                } else if let Token::Def {
                    tag, href, title, ..
                } = &t
                    && !self.links.contains_key(tag)
                {
                    self.links.insert(
                        tag.clone(),
                        LinkDef {
                            href: href.clone(),
                            title: title.clone(),
                        },
                    );
                    tokens.push(t);
                }
                continue;
            }

            // table (gfm)
            if let Some(t) = self.table(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }

            // lheading
            if let Some(t) = self.lheading(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }

            // top-level paragraph
            if self.top
                && let Some(t) = self.paragraph(src)
            {
                let raw_len = t.raw().len();
                match tokens.last_mut() {
                    Some(last) if last_paragraph_clipped && last.is_paragraph() => {
                        let Token::Paragraph { raw, text, .. } = &t else {
                            unreachable!()
                        };
                        let sep = if last.raw().ends_with('\n') { "" } else { "\n" };
                        let r = last.raw_mut();
                        r.push_str(sep);
                        r.push_str(raw);
                        if let Some(lt) = last.text_mut() {
                            lt.push('\n');
                            lt.push_str(text);
                            let joined = lt.clone();
                            self.queue.pop();
                            self.set_last_queued_src(&joined);
                        }
                    }
                    _ => tokens.push(t),
                }
                // No block extensions: the paragraph is never clipped.
                last_paragraph_clipped = false;
                src = after(src, raw_len);
                continue;
            }

            // text
            if let Some(t) = self.text(src) {
                src = after(src, t.raw().len());
                match tokens.last_mut() {
                    Some(last) if last.is_text() => {
                        let Token::Text { raw, text, .. } = &t else {
                            unreachable!()
                        };
                        let sep = if last.raw().ends_with('\n') { "" } else { "\n" };
                        let r = last.raw_mut();
                        r.push_str(sep);
                        r.push_str(raw);
                        if let Some(lt) = last.text_mut() {
                            lt.push('\n');
                            lt.push_str(text);
                            let joined = lt.clone();
                            self.queue.pop();
                            self.set_last_queued_src(&joined);
                        }
                    }
                    _ => tokens.push(t),
                }
                continue;
            }

            if !src.is_empty() {
                self.infinite_loop(src);
                break;
            }
        }
        self.top = true;
    }

    pub fn inline_tokens(&mut self, src: &str, tokens: &mut Vec<Token>) {
        let r = self.rules;
        let mut masked = src.to_string();
        if !self.links.is_empty() {
            let links = &self.links;
            masked = r.inline.reflink_search.replace_with(&masked, |c| {
                let m = c.full();
                let label_start = m.rfind('[').map_or(0, |i| i + 1);
                let label = &m[label_start..m.len().saturating_sub(1).max(label_start)];
                if links.contains_key(label) {
                    format!("[{}]", "a".repeat(m.len().saturating_sub(2)))
                } else {
                    m.to_string()
                }
            });
        }
        masked = r
            .inline
            .any_punctuation
            .replace_with(&masked, |c| "+".repeat(c.full().len()));
        masked = r.inline.block_skip.replace_with(&masked, |c| {
            let m = c.full();
            let offset = c.get(2).map_or(0, str::len);
            format!(
                "{}[{}]",
                &m[..offset],
                "a".repeat(m.len().saturating_sub(offset + 2))
            )
        });

        let mut src = src;
        let mut keep_prev_char = false;
        let mut prev_char = String::new();
        let mut src_length = usize::MAX;
        while !src.is_empty() {
            if src.len() < src_length {
                src_length = src.len();
            } else {
                self.infinite_loop(src);
                break;
            }
            if !keep_prev_char {
                prev_char.clear();
            }
            keep_prev_char = false;

            // escape
            if let Some(t) = self.escape(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // tag
            if let Some(t) = self.tag(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // link
            if let Some(t) = self.link(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // reflink, nolink
            if let Some(t) = self.reflink(src) {
                src = after(src, t.raw().len());
                match (tokens.last_mut(), &t) {
                    (
                        Some(Token::Text { raw, text, .. }),
                        Token::Text {
                            raw: r2, text: t2, ..
                        },
                    ) => {
                        raw.push_str(r2);
                        text.push_str(t2);
                    }
                    _ => tokens.push(t),
                }
                continue;
            }
            // em & strong
            if let Some(t) = self.em_strong(src, &masked, &prev_char) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // code
            if let Some(t) = self.codespan(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // br
            if let Some(t) = self.br(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // del (gfm)
            if let Some(t) = self.del(src, &masked, &prev_char) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // autolink
            if let Some(t) = self.autolink(src) {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // url (gfm)
            if !self.in_link
                && let Some(t) = self.url(src)
            {
                src = after(src, t.raw().len());
                tokens.push(t);
                continue;
            }
            // text
            if let Some(t) = self.inline_text(src) {
                src = after(src, t.raw().len());
                let raw = t.raw();
                if !raw.ends_with('_') {
                    prev_char = raw.chars().last().map(String::from).unwrap_or_default();
                }
                keep_prev_char = true;
                match (tokens.last_mut(), &t) {
                    (
                        Some(Token::Text { raw, text, .. }),
                        Token::Text {
                            raw: r2, text: t2, ..
                        },
                    ) => {
                        raw.push_str(r2);
                        text.push_str(t2);
                    }
                    _ => tokens.push(t),
                }
                continue;
            }

            if !src.is_empty() {
                self.infinite_loop(src);
                break;
            }
        }
    }

    // ---- Tokenizer: block ----

    fn space(&self, src: &str) -> Option<String> {
        let cap = self.rules.block.newline.exec(src)?;
        (!cap.full().is_empty()).then(|| cap.full().to_string())
    }

    fn code(&self, src: &str) -> Option<Token> {
        let cap = self.rules.block.code.exec(src)?;
        let raw = trim_trailing_blank_lines(cap.full());
        let text = self.rules.other.code_remove_indent.replace(&raw, "");
        Some(Token::Code {
            raw,
            lang: None,
            text,
        })
    }

    fn fences(&self, src: &str) -> Option<Token> {
        let cap = self.rules.block.fences.exec(src)?;
        let raw = cap.full().to_string();
        let text = indent_code_compensation(&raw, cap.str(3), self.rules);
        let lang = cap.get(2).map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                self.rules.inline.any_punctuation.replace(js::trim(l), "$1")
            }
        });
        Some(Token::Code { raw, lang, text })
    }

    fn heading(&mut self, src: &str) -> Option<Token> {
        let cap = self.rules.block.heading.exec(src)?;
        let mut text = js::trim(cap.str(2)).to_string();
        if self.rules.other.ending_hash.test(&text) {
            let trimmed = rtrim(&text, '#', false);
            if trimmed.is_empty() || self.rules.other.ending_space_char.test(&trimmed) {
                text = js::trim(&trimmed).to_string();
            }
        }
        let raw = rtrim(cap.full(), '\n', false);
        let depth = cap.str(1).len();
        let inline = self.inline(&text);
        Some(Token::Heading {
            raw,
            depth,
            text,
            tokens: Vec::new(),
            inline: Some(inline),
        })
    }

    fn hr(&self, src: &str) -> Option<Token> {
        let cap = self.rules.block.hr.exec(src)?;
        Some(Token::Hr {
            raw: rtrim(cap.full(), '\n', false),
        })
    }

    fn blockquote(&mut self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r.block.blockquote.exec(src)?;
        let trimmed = rtrim(cap.full(), '\n', false);
        let mut lines: Vec<String> = trimmed.split('\n').map(String::from).collect();
        let mut raw = String::new();
        let mut text = String::new();
        let mut tokens: Vec<Token> = Vec::new();
        while !lines.is_empty() {
            let mut in_blockquote = false;
            let mut current: Vec<String> = Vec::new();
            let mut i = 0;
            while i < lines.len() {
                if r.other.blockquote_start.test(&lines[i]) {
                    current.push(lines[i].clone());
                    in_blockquote = true;
                } else if !in_blockquote {
                    current.push(lines[i].clone());
                } else {
                    break;
                }
                i += 1;
            }
            lines.drain(..i);
            let current_raw = current.join("\n");
            let current_text = r.other.blockquote_setext_replace2.replace(
                &r.other
                    .blockquote_setext_replace
                    .replace(&current_raw, "\n    $1"),
                "",
            );
            raw = if raw.is_empty() {
                current_raw.clone()
            } else {
                format!("{raw}\n{current_raw}")
            };
            text = if text.is_empty() {
                current_text.clone()
            } else {
                format!("{text}\n{current_text}")
            };
            let top = self.top;
            self.top = true;
            self.block_tokens(&current_text, &mut tokens, true);
            self.top = top;
            if lines.is_empty() {
                break;
            }
            match tokens.last() {
                Some(Token::Code { .. }) => break,
                Some(Token::Blockquote {
                    raw: old_raw,
                    text: old_text,
                    ..
                }) => {
                    let (old_raw, old_text) = (old_raw.clone(), old_text.clone());
                    let new_text = format!("{old_raw}\n{}", lines.join("\n"));
                    let Some(new_token) = self.blockquote(&new_text) else {
                        break;
                    };
                    let (nr, nt) = match &new_token {
                        Token::Blockquote { raw, text, .. } => (raw.clone(), text.clone()),
                        _ => unreachable!(),
                    };
                    if let Some(last) = tokens.last_mut() {
                        *last = new_token;
                    }
                    raw = format!("{}{}", &raw[..raw.len() - old_raw.len()], nr);
                    text = format!("{}{}", &text[..text.len() - old_text.len()], nt);
                    break;
                }
                Some(Token::List { raw: old_raw, .. }) => {
                    let old_raw = old_raw.clone();
                    let new_text = format!("{old_raw}\n{}", lines.join("\n"));
                    let Some(new_token) = self.list(&new_text) else {
                        break;
                    };
                    let nr = new_token.raw().to_string();
                    if let Some(last) = tokens.last_mut() {
                        *last = new_token;
                    }
                    raw = format!("{}{}", &raw[..raw.len() - old_raw.len()], nr);
                    text = format!(
                        "{}{}",
                        &text[..text.len().saturating_sub(old_raw.len())],
                        nr
                    );
                    lines = after(&new_text, nr.len())
                        .split('\n')
                        .map(String::from)
                        .collect();
                    continue;
                }
                _ => {}
            }
        }
        Some(Token::Blockquote { raw, text, tokens })
    }

    fn list(&mut self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r.block.list.exec(src)?;
        let bull = js::trim(cap.str(1)).to_string();
        let ordered = utf16_len(&bull) > 1;
        let start = if ordered {
            Some(bull[..bull.len() - 1].parse::<u64>().unwrap_or(0))
        } else {
            None
        };
        let mut list_raw = String::new();
        let mut loose = false;
        let mut items: Vec<ListItem> = Vec::new();
        let bull_re = if ordered {
            format!(r"\d{{1,9}}\{}", &bull[bull.len() - 1..])
        } else {
            format!(r"\{bull}")
        };
        let item_regex = r.other.list_item(&bull_re);
        let mut ends_with_blank_line = false;
        let mut src = src;
        while !src.is_empty() {
            let mut end_early = false;
            let Some(cap) = item_regex.exec(src) else {
                break;
            };
            if r.block.hr.test(src) {
                break;
            }
            let mut raw = cap.full().to_string();
            let c1 = cap.str(1).to_string();
            let c2 = cap.str(2).to_string();
            src = after(src, raw.len());

            let mut line = expand_tabs(c2.split('\n').next().unwrap_or(""), utf16_len(&c1));
            let mut next_line = src.split('\n').next().unwrap_or("").to_string();
            let mut blank_line = js::trim(&line).is_empty();
            let mut item_contents = String::new();
            let indent;
            if blank_line {
                indent = utf16_len(&c1) + 1;
            } else {
                let found = r.other.non_space_char.search(&line).unwrap_or(0);
                let found = if found > 4 { 1 } else { found };
                item_contents = line[found.min(line.len())..].to_string();
                indent = found + utf16_len(&c1);
            }

            if blank_line && r.other.blank_line.test(&next_line) {
                raw.push_str(&next_line);
                raw.push('\n');
                src = after(src, next_line.len() + 1);
                end_early = true;
            }

            if !end_early {
                let ir = r.other.indent(indent);
                while !src.is_empty() {
                    let raw_line = src.split('\n').next().unwrap_or("").to_string();
                    next_line = raw_line.clone();
                    let next_line_without_tabs =
                        r.other.tab_char_global.replace(&next_line, "    ");

                    if ir.fences_begin.test(&next_line)
                        || ir.heading_begin.test(&next_line)
                        || ir.html_begin.test(&next_line)
                        || ir.blockquote_begin.test(&next_line)
                        || ir.next_bullet.test(&next_line)
                        || ir.hr.test(&next_line)
                    {
                        break;
                    }

                    let first = r.other.non_space_char.search(&next_line_without_tabs);
                    if first.is_some_and(|i| i >= indent) || js::trim(&next_line).is_empty() {
                        item_contents.push('\n');
                        item_contents.push_str(slice_from(&next_line_without_tabs, indent));
                    } else {
                        if blank_line {
                            break;
                        }
                        let line_no_tabs = r.other.tab_char_global.replace(&line, "    ");
                        if r.other
                            .non_space_char
                            .search(&line_no_tabs)
                            .is_some_and(|i| i >= 4)
                        {
                            break;
                        }
                        if ir.fences_begin.test(&line)
                            || ir.heading_begin.test(&line)
                            || ir.hr.test(&line)
                        {
                            break;
                        }
                        item_contents.push('\n');
                        item_contents.push_str(&next_line);
                    }

                    blank_line = js::trim(&next_line).is_empty();
                    raw.push_str(&raw_line);
                    raw.push('\n');
                    src = after(src, raw_line.len() + 1);
                    line = slice_from(&next_line_without_tabs, indent).to_string();
                }
            }

            if !loose {
                if ends_with_blank_line {
                    loose = true;
                } else if r.other.double_blank_line.test(&raw) {
                    ends_with_blank_line = true;
                }
            }

            items.push(ListItem {
                raw: raw.clone(),
                task: r.other.list_is_task.test(&item_contents),
                checked: None,
                loose: false,
                text: item_contents,
                tokens: Vec::new(),
            });
            list_raw.push_str(&raw);
        }

        let last = items.last_mut()?;
        last.raw = js::trim_end(&last.raw).to_string();
        last.text = js::trim_end(&last.text).to_string();
        let list_raw = js::trim_end(&list_raw).to_string();

        for item in items.iter_mut() {
            self.top = false;
            let mut toks = Vec::new();
            self.block_tokens(&item.text, &mut toks, false);
            item.tokens = toks;
            let first_is_text = item
                .tokens
                .first()
                .is_some_and(|t| t.is_text() || t.is_paragraph());
            if item.task && first_is_text {
                item.text = r.other.list_replace_task.replace(&item.text, "");
                if let Some(t0) = item.tokens.first_mut() {
                    let nr = r.other.list_replace_task.replace(t0.raw(), "");
                    *t0.raw_mut() = nr;
                    if let Some(tt) = t0.text_mut() {
                        let nt = r.other.list_replace_task.replace(tt, "");
                        *tt = nt;
                    }
                }
                for q in self.queue.iter_mut().rev() {
                    if r.other.list_is_task.test(&q.src) {
                        q.src = r.other.list_replace_task.replace(&q.src, "");
                        break;
                    }
                }
                if let Some(task_raw) = r.other.list_task_checkbox.exec(&item.raw) {
                    let checkbox_raw = format!("{} ", task_raw.full());
                    let checked = task_raw.full() != "[ ]";
                    item.checked = Some(checked);
                    let checkbox = Token::Checkbox {
                        raw: checkbox_raw.clone(),
                        checked,
                    };
                    if loose {
                        match item.tokens.first_mut() {
                            Some(t0) if t0.inline_slot().is_some() => {
                                let slot = t0.inline_slot().unwrap_or(0);
                                let nr = format!("{checkbox_raw}{}", t0.raw());
                                *t0.raw_mut() = nr;
                                if let Some(tt) = t0.text_mut() {
                                    *tt = format!("{checkbox_raw}{tt}");
                                }
                                self.slots[slot].insert(0, checkbox);
                            }
                            _ => {
                                let slot = self.slots.len();
                                self.slots.push(vec![checkbox]);
                                item.tokens.insert(
                                    0,
                                    Token::Paragraph {
                                        raw: checkbox_raw.clone(),
                                        text: checkbox_raw,
                                        tokens: Vec::new(),
                                        inline: Some(slot),
                                    },
                                );
                            }
                        }
                    } else {
                        item.tokens.insert(0, checkbox);
                    }
                }
            } else if item.task {
                item.task = false;
            }

            if !loose {
                let has_multiple_line_breaks = item
                    .tokens
                    .iter()
                    .any(|t| matches!(t, Token::Space { raw } if r.other.any_line.test(raw)));
                loose = has_multiple_line_breaks;
            }
        }

        if loose {
            for item in items.iter_mut() {
                item.loose = true;
                for t in item.tokens.iter_mut() {
                    if let Token::Text {
                        raw,
                        text,
                        tokens,
                        inline,
                        ..
                    } = t
                    {
                        *t = Token::Paragraph {
                            raw: std::mem::take(raw),
                            text: std::mem::take(text),
                            tokens: tokens.take().unwrap_or_default(),
                            inline: inline.take(),
                        };
                    }
                }
            }
        }

        Some(Token::List {
            raw: list_raw,
            ordered,
            start,
            loose,
            items,
        })
    }

    fn html(&self, src: &str) -> Option<Token> {
        let cap = self.rules.block.html.exec(src)?;
        let raw = trim_trailing_blank_lines(cap.full());
        Some(Token::Html {
            text: raw.clone(),
            raw,
            block: true,
        })
    }

    fn def(&self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r.block.def.exec(src)?;
        let tag = r
            .other
            .multiple_space_global
            .replace(&cap.str(1).to_lowercase(), " ");
        let href = if cap.truthy(2) {
            r.inline
                .any_punctuation
                .replace(&r.other.href_brackets.replace(cap.str(2), "$1"), "$1")
        } else {
            String::new()
        };
        let title = cap.get(3).map(|t| {
            if t.is_empty() {
                String::new()
            } else {
                let inner = &t[1..t.len() - 1];
                r.inline.any_punctuation.replace(inner, "$1")
            }
        });
        Some(Token::Def {
            tag,
            raw: rtrim(cap.full(), '\n', false),
            href,
            title,
        })
    }

    fn table(&mut self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r.block.table.exec(src)?;
        if !r.other.table_delimiter.test(cap.str(2)) {
            return None;
        }
        let headers = split_cells(cap.str(1), None);
        let aligns: Vec<String> = r
            .other
            .table_align_chars
            .replace(cap.str(2), "")
            .split('|')
            .map(String::from)
            .collect();
        let rows: Vec<String> = if cap.get(3).is_some_and(|c| !js::trim(c).is_empty()) {
            r.other
                .table_row_blank_line
                .replace(cap.str(3), "")
                .split('\n')
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };
        if headers.len() != aligns.len() {
            return None;
        }
        let align: Vec<Option<Align>> = aligns
            .iter()
            .map(|a| {
                if r.other.table_align_right.test(a) {
                    Some(Align::Right)
                } else if r.other.table_align_center.test(a) {
                    Some(Align::Center)
                } else if r.other.table_align_left.test(a) {
                    Some(Align::Left)
                } else {
                    None
                }
            })
            .collect();
        let raw = rtrim(cap.full(), '\n', false);
        let mut header = Vec::new();
        for (i, h) in headers.iter().enumerate() {
            let slot = self.inline(h);
            header.push(Cell {
                text: h.clone(),
                tokens: Vec::new(),
                inline: Some(slot),
                header: true,
                align: align[i],
            });
        }
        let mut out_rows = Vec::new();
        for row in &rows {
            let cells = split_cells(row, Some(header.len()));
            let mut out = Vec::new();
            for (i, cell) in cells.into_iter().enumerate() {
                let slot = self.inline(&cell);
                out.push(Cell {
                    text: cell,
                    tokens: Vec::new(),
                    inline: Some(slot),
                    header: false,
                    align: align.get(i).copied().flatten(),
                });
            }
            out_rows.push(out);
        }
        Some(Token::Table {
            raw,
            header,
            align,
            rows: out_rows,
        })
    }

    fn lheading(&mut self, src: &str) -> Option<Token> {
        let cap = self.rules.block.lheading.exec(src)?;
        let text = js::trim(cap.str(1)).to_string();
        let depth = if cap.str(2).starts_with('=') { 1 } else { 2 };
        let raw = rtrim(cap.full(), '\n', false);
        let inline = self.inline(&text);
        Some(Token::Heading {
            raw,
            depth,
            text,
            tokens: Vec::new(),
            inline: Some(inline),
        })
    }

    fn paragraph(&mut self, src: &str) -> Option<Token> {
        let cap = self.rules.block.paragraph.exec(src)?;
        let c1 = cap.str(1);
        let text = c1.strip_suffix('\n').unwrap_or(c1).to_string();
        let raw = cap.full().to_string();
        let inline = self.inline(&text);
        Some(Token::Paragraph {
            raw,
            text,
            tokens: Vec::new(),
            inline: Some(inline),
        })
    }

    fn text(&mut self, src: &str) -> Option<Token> {
        let cap = self.rules.block.text.exec(src)?;
        let raw = cap.full().to_string();
        let inline = self.inline(&raw);
        Some(Token::Text {
            text: raw.clone(),
            raw,
            tokens: Some(Vec::new()),
            inline: Some(inline),
            escaped: false,
        })
    }

    // ---- Tokenizer: inline ----

    fn escape(&self, src: &str) -> Option<Token> {
        let cap = self.rules.inline.escape.exec(src)?;
        Some(Token::Escape {
            raw: cap.full().to_string(),
            text: cap.str(1).to_string(),
        })
    }

    fn tag(&mut self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r.inline.tag.exec(src)?;
        let m = cap.full();
        if !self.in_link && r.other.start_a_tag.test(m) {
            self.in_link = true;
        } else if self.in_link && r.other.end_a_tag.test(m) {
            self.in_link = false;
        }
        if !self.in_raw_block && r.other.start_pre_script_tag.test(m) {
            self.in_raw_block = true;
        } else if self.in_raw_block && r.other.end_pre_script_tag.test(m) {
            self.in_raw_block = false;
        }
        Some(Token::Html {
            raw: m.to_string(),
            text: m.to_string(),
            block: false,
        })
    }

    fn link(&mut self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r.inline.link.exec(src)?;
        let mut c0 = cap.full().to_string();
        let c1 = cap.str(1).to_string();
        let mut c2 = cap.str(2).to_string();
        let mut c3 = cap.get(3).map(String::from);
        let trimmed_url = js::trim(&c2).to_string();
        if r.other.start_angle_bracket.test(&trimmed_url) {
            if !r.other.end_angle_bracket.test(&trimmed_url) {
                return None;
            }
            let rtrim_slash = rtrim(&trimmed_url[..trimmed_url.len() - 1], '\\', false);
            if (utf16_len(&trimmed_url) - utf16_len(&rtrim_slash)).is_multiple_of(2) {
                return None;
            }
        } else {
            let last_paren = find_closing_bracket(&c2, ('(', ')'));
            if last_paren == -2 {
                return None;
            }
            if last_paren > -1 {
                let lp = last_paren as usize;
                let start = if c0.starts_with('!') { 5 } else { 4 };
                let link_len = start + c1.len() + lp;
                c2 = c2[..lp].to_string();
                c0 = js::trim(&c0[..link_len.min(c0.len())]).to_string();
                c3 = Some(String::new());
            }
        }
        let mut href = c2.clone();
        let title = match &c3 {
            Some(t) if !t.is_empty() => {
                let chars: Vec<char> = t.chars().collect();
                chars[1..chars.len().saturating_sub(1).max(1)]
                    .iter()
                    .collect()
            }
            _ => String::new(),
        };
        href = js::trim(&href).to_string();
        if r.other.start_angle_bracket.test(&href) {
            let chars: Vec<char> = href.chars().collect();
            href = if chars.len() >= 2 {
                chars[1..chars.len() - 1].iter().collect()
            } else {
                String::new()
            };
        }
        let href = if href.is_empty() {
            href
        } else {
            r.inline.any_punctuation.replace(&href, "$1")
        };
        let title = if title.is_empty() {
            title
        } else {
            r.inline.any_punctuation.replace(&title, "$1")
        };
        Some(self.output_link(&c0, &c1, href, Some(title)))
    }

    /// `outputLink`.
    fn output_link(&mut self, c0: &str, c1: &str, href: String, title: Option<String>) -> Token {
        let title = title.filter(|t| !t.is_empty());
        let text = self.rules.other.output_link_replace.replace(c1, "$1");
        self.in_link = true;
        let mut tokens = Vec::new();
        self.inline_tokens(&text, &mut tokens);
        self.in_link = false;
        if c0.starts_with('!') {
            Token::Image {
                raw: c0.to_string(),
                href,
                title,
                text,
                tokens,
            }
        } else {
            Token::Link {
                raw: c0.to_string(),
                href,
                title,
                text,
                tokens,
            }
        }
    }

    fn reflink(&mut self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r
            .inline
            .reflink
            .exec(src)
            .or_else(|| r.inline.nolink.exec(src))?;
        let label = if cap.truthy(2) {
            cap.str(2)
        } else {
            cap.str(1)
        };
        let link_string = r.other.multiple_space_global.replace(label, " ");
        let Some(link) = self.links.get(&link_string.to_lowercase()).cloned() else {
            let text = cap
                .full()
                .chars()
                .next()
                .map(String::from)
                .unwrap_or_default();
            return Some(Token::Text {
                raw: text.clone(),
                text,
                tokens: None,
                inline: None,
                escaped: false,
            });
        };
        let (c0, c1) = (cap.full().to_string(), cap.str(1).to_string());
        Some(self.output_link(&c0, &c1, link.href, link.title))
    }

    fn em_strong(&mut self, src: &str, masked: &str, prev_char: &str) -> Option<Token> {
        let r = self.rules;
        let m = r.inline.em_strong_l_delim.exec(src)?;
        if !m.truthy(1) && !m.truthy(2) && !m.truthy(3) && !m.truthy(4) {
            return None;
        }
        if m.truthy(4) && r.other.unicode_alpha_numeric.test(prev_char) {
            return None;
        }
        let next_char = if m.truthy(1) { m.str(1) } else { m.str(3) };
        if !(next_char.is_empty() || prev_char.is_empty() || r.inline.punctuation.test(prev_char)) {
            return None;
        }
        let l_length = m.full().chars().count() - 1;
        let mut delim_total = l_length as i64;
        let mut mid_delim_total = 0i64;
        let end_reg = if m.full().starts_with('*') {
            &r.inline.em_strong_r_delim_ast
        } else {
            &r.inline.em_strong_r_delim_und
        };
        let masked = clip_masked(masked, src.len(), l_length);
        let mut at = 0;
        while let Some(c) = end_reg.exec_at(masked, at) {
            let range = c.range();
            at = if range.is_empty() {
                range.end + masked[range.end..].chars().next().map_or(1, char::len_utf8)
            } else {
                range.end
            };
            let r_delim = (1..=6).map(|i| c.str(i)).find(|s| !s.is_empty());
            let Some(r_delim) = r_delim else {
                if at > masked.len() {
                    break;
                }
                continue;
            };
            let mut r_length = r_delim.chars().count() as i64;
            if c.truthy(3) || c.truthy(4) {
                delim_total += r_length;
                continue;
            } else if (c.truthy(5) || c.truthy(6))
                && l_length % 3 != 0
                && (l_length as i64 + r_length) % 3 == 0
            {
                mid_delim_total += r_length;
                continue;
            }
            delim_total -= r_length;
            if delim_total > 0 {
                continue;
            }
            r_length = r_length.min(r_length + delim_total + mid_delim_total);
            let last_char_length = c.full().chars().next().map_or(0, char::len_utf8);
            let end = l_length + c.index() + last_char_length + r_length.max(0) as usize;
            let raw = &src[..end.min(src.len())];
            if (l_length as i64).min(r_length) % 2 == 1 {
                let text = &raw[1..raw.len() - 1];
                let mut tokens = Vec::new();
                self.inline_tokens(text, &mut tokens);
                return Some(Token::Em {
                    raw: raw.to_string(),
                    text: text.to_string(),
                    tokens,
                });
            }
            let text = &raw[2..raw.len().saturating_sub(2).max(2)];
            let mut tokens = Vec::new();
            self.inline_tokens(text, &mut tokens);
            return Some(Token::Strong {
                raw: raw.to_string(),
                text: text.to_string(),
                tokens,
            });
        }
        None
    }

    fn codespan(&self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r.inline.code.exec(src)?;
        let mut text = r.other.new_line_char_global.replace(cap.str(2), " ");
        let has_non_space = r.other.non_space_char.test(&text);
        let both_ends =
            r.other.starting_space_char.test(&text) && r.other.ending_space_char.test(&text);
        if has_non_space && both_ends {
            text = text[1..text.len() - 1].to_string();
        }
        Some(Token::Codespan {
            raw: cap.full().to_string(),
            text,
        })
    }

    fn br(&self, src: &str) -> Option<Token> {
        let cap = self.rules.inline.br.exec(src)?;
        Some(Token::Br {
            raw: cap.full().to_string(),
        })
    }

    fn del(&mut self, src: &str, masked: &str, prev_char: &str) -> Option<Token> {
        let r = self.rules;
        let m = r.inline.del_l_delim.exec(src)?;
        let next_char = m.str(1);
        if !(next_char.is_empty() || prev_char.is_empty() || r.inline.punctuation.test(prev_char)) {
            return None;
        }
        let l_length = m.full().chars().count() - 1;
        let mut delim_total = l_length as i64;
        let masked = clip_masked(masked, src.len(), l_length);
        let mut at = 0;
        while let Some(c) = r.inline.del_r_delim.exec_at(masked, at) {
            let range = c.range();
            at = if range.is_empty() {
                range.end + masked[range.end..].chars().next().map_or(1, char::len_utf8)
            } else {
                range.end
            };
            let Some(r_delim) = (1..=6).map(|i| c.str(i)).find(|s| !s.is_empty()) else {
                if at > masked.len() {
                    break;
                }
                continue;
            };
            let mut r_length = r_delim.chars().count() as i64;
            if r_length != l_length as i64 {
                continue;
            }
            if c.truthy(3) || c.truthy(4) {
                delim_total += r_length;
                continue;
            }
            delim_total -= r_length;
            if delim_total > 0 {
                continue;
            }
            r_length = r_length.min(r_length + delim_total);
            let last_char_length = c.full().chars().next().map_or(0, char::len_utf8);
            let end = l_length + c.index() + last_char_length + r_length.max(0) as usize;
            let raw = &src[..end.min(src.len())];
            let text = &raw[l_length..raw.len().saturating_sub(l_length).max(l_length)];
            let mut tokens = Vec::new();
            self.inline_tokens(text, &mut tokens);
            return Some(Token::Del {
                raw: raw.to_string(),
                text: text.to_string(),
                tokens,
            });
        }
        None
    }

    fn autolink(&self, src: &str) -> Option<Token> {
        let cap = self.rules.inline.autolink.exec(src)?;
        let text = cap.str(1).to_string();
        let href = if cap.str(2) == "@" {
            format!("mailto:{text}")
        } else {
            text.clone()
        };
        Some(Token::Link {
            raw: cap.full().to_string(),
            text: text.clone(),
            href,
            title: None,
            tokens: vec![Token::Text {
                raw: text.clone(),
                text,
                tokens: None,
                inline: None,
                escaped: false,
            }],
        })
    }

    fn url(&self, src: &str) -> Option<Token> {
        let r = self.rules;
        let cap = r.inline.url.exec(src)?;
        let (text, href, raw);
        if cap.str(2) == "@" {
            text = cap.full().to_string();
            href = format!("mailto:{text}");
            raw = text.clone();
        } else {
            let mut c0 = cap.full().to_string();
            loop {
                let prev = c0.clone();
                c0 = r
                    .inline
                    .backpedal
                    .exec(&c0)
                    .map(|m| m.full().to_string())
                    .unwrap_or_default();
                if prev == c0 {
                    break;
                }
            }
            text = c0.clone();
            href = if cap.str(1) == "www." {
                format!("http://{c0}")
            } else {
                c0.clone()
            };
            raw = c0;
        }
        Some(Token::Link {
            raw,
            text: text.clone(),
            href,
            title: None,
            tokens: vec![Token::Text {
                raw: text.clone(),
                text,
                tokens: None,
                inline: None,
                escaped: false,
            }],
        })
    }

    fn inline_text(&self, src: &str) -> Option<Token> {
        let cap = self.rules.inline.text.exec(src)?;
        Some(Token::Text {
            raw: cap.full().to_string(),
            text: cap.full().to_string(),
            tokens: None,
            inline: None,
            escaped: self.in_raw_block,
        })
    }
}

/// `maskedSrc.slice(-1 * src.length + lLength)`.
fn clip_masked(masked: &str, src_len: usize, l_length: usize) -> &str {
    let offset = l_length as i64 - src_len as i64;
    let start = if offset < 0 {
        (masked.len() as i64 + offset).max(0) as usize
    } else {
        (offset as usize).min(masked.len())
    };
    let mut start = start;
    while !masked.is_char_boundary(start) {
        start += 1;
    }
    &masked[start..]
}

/// `str.slice(n)` for a char count `n` on ASCII-indented text.
fn slice_from(s: &str, n: usize) -> &str {
    let mut idx = 0;
    for (count, (i, _)) in s.char_indices().enumerate() {
        if count == n {
            return &s[i..];
        }
        idx = i;
    }
    if s.chars().count() <= n {
        ""
    } else {
        &s[idx..]
    }
}

fn indent_code_compensation(raw: &str, text: &str, rules: &Rules) -> String {
    let Some(m) = rules.other.indent_code_compensation.exec(raw) else {
        return text.to_string();
    };
    let indent_to_code = m.str(1);
    text.split('\n')
        .map(|node| match rules.other.beginning_space.exec(node) {
            Some(mi) if utf16_len(mi.full()) >= utf16_len(indent_to_code) => {
                slice_from(node, indent_to_code.chars().count()).to_string()
            }
            _ => node.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
