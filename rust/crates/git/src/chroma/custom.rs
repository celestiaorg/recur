//! The lexer behavior chroma writes in Go rather than XML: Go raw-string
//! templates, HTTP headers and bodies, reStructuredText code blocks, Haxe
//! preprocessor state, and Markdown frontmatter.

use super::lexer::{EmitterFunc, LexResult, Lexer, LexerState, Token, TokeniseOptions};
use super::token::TokenType;
use super::{raku, registry};

pub(crate) fn emit_func(
    f: EmitterFunc,
    groups: &[String],
    state: &mut LexerState,
) -> LexResult<Vec<Token>> {
    let g = |i: usize| groups.get(i).cloned().unwrap_or_default();
    match f {
        EmitterFunc::GoTemplateString => go_template_string(&g(0)),
        EmitterFunc::HttpHeaderBlock => Ok(vec![
            Token::new(TokenType::NAME, g(1)),
            Token::new(TokenType::TEXT, g(2)),
            Token::new(TokenType::OPERATOR, g(3)),
            Token::new(TokenType::TEXT, g(4)),
            Token::new(TokenType::LITERAL, g(5)),
            Token::new(TokenType::TEXT, g(6)),
        ]),
        EmitterFunc::HttpContinuousHeaderBlock => Ok(vec![
            Token::new(TokenType::TEXT, g(1)),
            Token::new(TokenType::LITERAL, g(2)),
            Token::new(TokenType::TEXT, g(3)),
        ]),
        EmitterFunc::HttpContentBlock => Ok(vec![Token::new(TokenType::GENERIC, g(0))]),
        EmitterFunc::RstCodeBlock => rst_code_block(groups),
        EmitterFunc::RakuQuote => raku::quote(state),
        EmitterFunc::RakuPodConfig => raku::pod_config(groups, state),
        EmitterFunc::RakuPodCode => raku::pod_code(state),
    }
}

/// Go raw strings are lexed as Go text templates with `Other` shown as
/// `LiteralString` (chroma `UsingLexer(TypeRemappingLexer(GoTextTemplate, …))`).
fn go_template_string(text: &str) -> LexResult<Vec<Token>> {
    let Some(template) = registry::get("Go Text Template") else {
        return Ok(Vec::new());
    };
    let mut tokens = template.tokenise(Some(&TokeniseOptions::nested("root")), text)?;
    for t in &mut tokens {
        if t.ty == TokenType::OTHER {
            t.ty = TokenType::LITERAL_STRING;
        }
    }
    Ok(tokens)
}

fn rst_code_block(groups: &[String]) -> LexResult<Vec<Token>> {
    let g = |i: usize| groups.get(i).cloned().unwrap_or_default();
    let mut tokens = vec![
        Token::new(TokenType::PUNCTUATION, g(1)),
        Token::new(TokenType::TEXT, g(2)),
        Token::new(TokenType::OPERATOR_WORD, g(3)),
        Token::new(TokenType::PUNCTUATION, g(4)),
        Token::new(TokenType::TEXT, g(5)),
        Token::new(TokenType::KEYWORD, g(6)),
        Token::new(TokenType::TEXT, g(7)),
    ];
    let code: String = groups
        .get(8..)
        .map(|rest| rest.concat())
        .unwrap_or_default();
    match registry::get(&g(6)) {
        None => tokens.push(Token::new(TokenType::LITERAL_STRING, code)),
        Some(lexer) => tokens.extend(lexer.tokenise(None, &code)?),
    }
    Ok(tokens)
}

/// Haxe `#if`/`#else`/`#end`: remembers the state stack at `#if` so each
/// branch lexes from the same state.
pub(crate) fn haxe_pre_proc(state: &mut LexerState) {
    const KEY: &str = "haxe-pre-proc";
    let mut stack: Vec<Vec<String>> = state
        .get::<Vec<Vec<String>>>(KEY)
        .cloned()
        .unwrap_or_default();
    let proc = state.groups.get(2).cloned().unwrap_or_default();
    match proc.as_str() {
        "if" => stack.push(state.stack.clone()),
        "else" | "elseif" => {
            if let Some(top) = stack.last() {
                state.stack.clone_from(top);
            }
        }
        "end" => {
            stack.pop();
        }
        _ => {}
    }
    if proc == "if" || proc == "elseif" {
        state.stack.push("preproc-expr".to_string());
    }
    if proc == "error" {
        state.stack.push("preproc-error".to_string());
    }
    state.set(KEY, stack);
}

/// A leading `---` YAML frontmatter block, if the document starts with one.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
        return None;
    }
    let line_end = text.find('\n')?;
    if text[..line_end]
        .strip_suffix('\r')
        .unwrap_or(&text[..line_end])
        != "---"
    {
        return None;
    }
    let mut pos = line_end + 1;
    while pos < text.len() {
        let next = text[pos..].find('\n')?;
        let end = pos + next;
        let line = &text[pos..end];
        if line.strip_suffix('\r').unwrap_or(line) == "---" {
            return Some((&text[..=end], &text[end + 1..]));
        }
        pos = end + 1;
    }
    None
}

pub(crate) fn markdown_tokenise(
    inner: &Lexer,
    options: Option<&TokeniseOptions>,
    text: &str,
) -> LexResult<Vec<Token>> {
    let Some((frontmatter, rest)) = split_frontmatter(text) else {
        return inner.tokenise(options, text);
    };
    let Some(yaml) = registry::get("YAML") else {
        return inner.tokenise(options, text);
    };
    let mut tokens = yaml.tokenise(options, frontmatter)?;
    tokens.extend(inner.tokenise(options, rest)?);
    Ok(tokens)
}

/// chroma's HTTP lexer: bodies are lexed by their `Content-Type`. The body
/// lexer's first token replaces the body token; the rest follow the HTTP
/// tokens, as chroma's iterator yields them.
pub(crate) fn http_tokenise(
    inner: &Lexer,
    options: Option<&TokeniseOptions>,
    text: &str,
) -> LexResult<Vec<Token>> {
    let mut content_type = String::new();
    let mut is_content_type = false;
    let mut sub: Option<std::collections::VecDeque<Token>> = None;
    let mut out = Vec::new();
    for mut token in inner.tokenise(options, text)? {
        if token.ty == TokenType::NAME && crate::text::go_lower(&token.value) == "content-type" {
            is_content_type = true;
        } else if token.ty == TokenType::LITERAL && is_content_type {
            is_content_type = false;
            content_type = token.value.trim().to_string();
            if let Some(pos) = content_type.find(';').filter(|&p| p > 0) {
                content_type = content_type[..pos].trim().to_string();
            }
        } else if token.ty == TokenType::GENERIC && !content_type.is_empty() {
            let reg = registry::registry();
            let mut lexer = reg.match_mime_type(&content_type);
            // application/calendar+xml can be treated as application/xml.
            if lexer.is_none()
                && content_type.contains('+')
                && let (Some(slash), Some(plus)) = (content_type.find('/'), content_type.rfind('+'))
            {
                content_type = format!("{}{}", &content_type[..=slash], &content_type[plus + 1..]);
                lexer = reg.match_mime_type(&content_type);
            }
            match lexer {
                None => token.ty = TokenType::TEXT,
                Some(lexer) => {
                    let mut tokens: std::collections::VecDeque<Token> =
                        lexer.tokenise(None, &token.value)?.into();
                    match tokens.pop_front() {
                        Some(first) => {
                            out.push(first);
                            sub = Some(tokens);
                            continue;
                        }
                        // An empty body iterator ends chroma's token stream.
                        None => return Ok(out),
                    }
                }
            }
        }
        out.push(token);
    }
    out.extend(sub.into_iter().flatten());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_split() {
        assert_eq!(
            split_frontmatter("---\na: 1\n---\n# Hi\n"),
            Some(("---\na: 1\n---\n", "# Hi\n"))
        );
        assert_eq!(split_frontmatter("---\na: 1\n"), None);
        assert_eq!(split_frontmatter("# Hi\n"), None);
    }
}
