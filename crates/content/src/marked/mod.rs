//! A port of marked 18 (`marked/src`) for the options Recur sets: `gfm: true`,
//! `breaks: true`, no extensions, hooks or `walkTokens`.

mod helpers;
mod lexer;
mod parser;
mod rules;

pub use helpers::{clean_url, escape_html_entities};
pub use lexer::{Align, Cell, LexError, Lexer, LinkDef, ListItem, Token};
pub use parser::{Parser, Renderer, default_code};

/// `marked.lexer(src)`.
pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new().lex(src)
}

/// `marked.parse(src)` with `renderer`'s overrides.
pub fn parse(src: &str, renderer: &dyn Renderer) -> Result<String, LexError> {
    let tokens = lex(src)?;
    Ok(Parser::new(renderer).parse(&tokens))
}

/// marked's own renderer (no overrides).
pub struct DefaultRenderer;

impl Renderer for DefaultRenderer {}

#[cfg(test)]
mod tests;
