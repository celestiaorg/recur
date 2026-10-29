//! `Parser.ts` and `Renderer.ts`: tokens to HTML. [`Renderer`]'s default methods are
//! marked's renderer; an engine overrides the ones it `use`s.

use super::helpers::{clean_url, escape_html_entities};
use super::lexer::{Cell, ListItem, Token};
use super::rules::rules;

/// One `renderer` override set (`marked.use({ renderer })`). Every method defaults to marked's.
pub trait Renderer {
    fn code(&self, _p: &Parser, text: &str, lang: Option<&str>) -> String {
        default_code(text, lang)
    }

    fn blockquote(&self, p: &Parser, tokens: &[Token]) -> String {
        format!("<blockquote>\n{}</blockquote>\n", p.parse(tokens))
    }

    fn html(&self, _p: &Parser, text: &str) -> String {
        text.to_string()
    }

    fn heading(&self, p: &Parser, depth: usize, tokens: &[Token]) -> String {
        format!("<h{depth}>{}</h{depth}>\n", p.parse_inline(tokens))
    }

    fn hr(&self, _p: &Parser) -> String {
        "<hr>\n".into()
    }

    fn list(&self, p: &Parser, ordered: bool, start: Option<u64>, items: &[ListItem]) -> String {
        let mut body = String::new();
        for item in items {
            body.push_str(&self.listitem(p, item));
        }
        let ty = if ordered { "ol" } else { "ul" };
        let start_attr = match start {
            Some(s) if ordered && s != 1 => format!(" start=\"{s}\""),
            _ => String::new(),
        };
        format!("<{ty}{start_attr}>\n{body}</{ty}>\n")
    }

    fn listitem(&self, p: &Parser, item: &ListItem) -> String {
        format!("<li>{}</li>\n", p.parse(&item.tokens))
    }

    fn checkbox(&self, _p: &Parser, checked: bool) -> String {
        format!(
            "<input {}disabled=\"\" type=\"checkbox\"> ",
            if checked { "checked=\"\" " } else { "" }
        )
    }

    fn paragraph(&self, p: &Parser, tokens: &[Token]) -> String {
        format!("<p>{}</p>\n", p.parse_inline(tokens))
    }

    fn table(&self, p: &Parser, header: &[Cell], rows: &[Vec<Cell>]) -> String {
        let mut cell = String::new();
        for h in header {
            cell.push_str(&self.tablecell(p, h));
        }
        let head = self.tablerow(p, &cell);
        let mut body = String::new();
        for row in rows {
            let mut cell = String::new();
            for c in row {
                cell.push_str(&self.tablecell(p, c));
            }
            body.push_str(&self.tablerow(p, &cell));
        }
        if !body.is_empty() {
            body = format!("<tbody>{body}</tbody>");
        }
        format!("<table>\n<thead>\n{head}</thead>\n{body}</table>\n")
    }

    fn tablerow(&self, _p: &Parser, text: &str) -> String {
        format!("<tr>\n{text}</tr>\n")
    }

    fn tablecell(&self, p: &Parser, cell: &Cell) -> String {
        let content = p.parse_inline(&cell.tokens);
        let ty = if cell.header { "th" } else { "td" };
        let tag = match cell.align {
            Some(a) => format!("<{ty} align=\"{}\">", a.as_str()),
            None => format!("<{ty}>"),
        };
        format!("{tag}{content}</{ty}>\n")
    }

    fn strong(&self, p: &Parser, tokens: &[Token]) -> String {
        format!("<strong>{}</strong>", p.parse_inline(tokens))
    }

    fn em(&self, p: &Parser, tokens: &[Token]) -> String {
        format!("<em>{}</em>", p.parse_inline(tokens))
    }

    fn codespan(&self, _p: &Parser, text: &str) -> String {
        format!("<code>{}</code>", escape_html_entities(text, true))
    }

    fn br(&self, _p: &Parser) -> String {
        "<br>".into()
    }

    fn del(&self, p: &Parser, tokens: &[Token]) -> String {
        format!("<del>{}</del>", p.parse_inline(tokens))
    }

    fn link(&self, p: &Parser, href: &str, title: Option<&str>, tokens: &[Token]) -> String {
        let text = p.parse_inline(tokens);
        let Some(href) = clean_url(href) else {
            return text;
        };
        let mut out = format!("<a href=\"{href}\"");
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            out.push_str(&format!(" title=\"{}\"", escape_html_entities(t, false)));
        }
        out.push('>');
        out.push_str(&text);
        out.push_str("</a>");
        out
    }

    fn image(
        &self,
        p: &Parser,
        href: &str,
        title: Option<&str>,
        text: &str,
        tokens: &[Token],
    ) -> String {
        let _ = text;
        let text = p.parse_text(tokens);
        let Some(href) = clean_url(href) else {
            return escape_html_entities(&text, false);
        };
        let mut out = format!(
            "<img src=\"{href}\" alt=\"{}\"",
            escape_html_entities(&text, false)
        );
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            out.push_str(&format!(" title=\"{}\"", escape_html_entities(t, false)));
        }
        out.push('>');
        out
    }

    fn text(&self, p: &Parser, token: &Token) -> String {
        match token {
            Token::Text {
                tokens: Some(tokens),
                ..
            } => p.parse_inline(tokens),
            Token::Text { text, escaped, .. } => {
                if *escaped {
                    text.clone()
                } else {
                    escape_html_entities(text, false)
                }
            }
            Token::Escape { text, .. } => escape_html_entities(text, false),
            _ => String::new(),
        }
    }
}

/// marked's `code` renderer.
pub fn default_code(text: &str, lang: Option<&str>) -> String {
    let o = &rules().other;
    let lang_string = o
        .not_space_start
        .exec(lang.unwrap_or(""))
        .map(|c| c.full().to_string())
        .unwrap_or_default();
    let code = format!("{}\n", o.ending_newline.replace(text, ""));
    if lang_string.is_empty() {
        return format!(
            "<pre><code>{}</code></pre>\n",
            escape_html_entities(&code, true)
        );
    }
    format!(
        "<pre><code class=\"language-{}\">{}</code></pre>\n",
        escape_html_entities(&lang_string, false),
        escape_html_entities(&code, true)
    )
}

pub struct Parser<'r> {
    renderer: &'r dyn Renderer,
}

impl<'r> Parser<'r> {
    pub fn new(renderer: &'r dyn Renderer) -> Parser<'r> {
        Parser { renderer }
    }

    /// `Parser.parse`: block tokens.
    pub fn parse(&self, tokens: &[Token]) -> String {
        let r = self.renderer;
        let mut out = String::new();
        for t in tokens {
            let s = match t {
                Token::Space { .. } | Token::Def { .. } => String::new(),
                Token::Hr { .. } => r.hr(self),
                Token::Heading { depth, tokens, .. } => r.heading(self, *depth, tokens),
                Token::Code { text, lang, .. } => r.code(self, text, lang.as_deref()),
                Token::Table { header, rows, .. } => r.table(self, header, rows),
                Token::Blockquote { tokens, .. } => r.blockquote(self, tokens),
                Token::List {
                    ordered,
                    start,
                    items,
                    ..
                } => r.list(self, *ordered, *start, items),
                Token::Checkbox { checked, .. } => r.checkbox(self, *checked),
                Token::Html { text, .. } => r.html(self, text),
                Token::Paragraph { tokens, .. } => r.paragraph(self, tokens),
                Token::Text { .. } => r.text(self, t),
                // Inline tokens never reach the block parser.
                _ => String::new(),
            };
            out.push_str(&s);
        }
        out
    }

    /// `Parser.parseInline` with the engine's renderer.
    pub fn parse_inline(&self, tokens: &[Token]) -> String {
        let r = self.renderer;
        let mut out = String::new();
        for t in tokens {
            let s = match t {
                Token::Escape { .. } | Token::Text { .. } => r.text(self, t),
                Token::Html { text, .. } => r.html(self, text),
                Token::Link {
                    href,
                    title,
                    tokens,
                    ..
                } => r.link(self, href, title.as_deref(), tokens),
                Token::Image {
                    href,
                    title,
                    text,
                    tokens,
                    ..
                } => r.image(self, href, title.as_deref(), text, tokens),
                Token::Checkbox { checked, .. } => r.checkbox(self, *checked),
                Token::Strong { tokens, .. } => r.strong(self, tokens),
                Token::Em { tokens, .. } => r.em(self, tokens),
                Token::Codespan { text, .. } => r.codespan(self, text),
                Token::Br { .. } => r.br(self),
                Token::Del { tokens, .. } => r.del(self, tokens),
                _ => String::new(),
            };
            out.push_str(&s);
        }
        out
    }

    /// `parseInline(tokens, textRenderer)`: the text-only rendering image alts use.
    pub fn parse_text(&self, tokens: &[Token]) -> String {
        let mut out = String::new();
        for t in tokens {
            match t {
                Token::Strong { text, .. }
                | Token::Em { text, .. }
                | Token::Codespan { text, .. }
                | Token::Del { text, .. }
                | Token::Html { text, .. }
                | Token::Text { text, .. }
                | Token::Escape { text, .. }
                | Token::Link { text, .. }
                | Token::Image { text, .. } => out.push_str(text),
                Token::Checkbox { raw, .. } => out.push_str(raw),
                _ => {}
            }
        }
        out
    }
}
