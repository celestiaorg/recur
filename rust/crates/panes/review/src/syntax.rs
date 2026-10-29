//! Diff code highlighting with the bundle grammars the editor uses, into the review palette.
//! Each side of a hunk keeps its own grammar state: the old side runs through deletions, the
//! new side through additions, and context lines carry both.

use recur_domain::DiffLine;
use recur_textmate::{Grammar, StateStack};

use crate::html::{CodeRun, Token, parse_code_runs};

/// Highlighted lines per card: the diff view's initial line cap, so the first screen is
/// uniformly styled. At most `REVIEW_HYDRATE_MAX` cards mount, so a patch highlights at most
/// 4,000 lines.
pub const MAX_HIGHLIGHT_LINES: usize = 500;

/// Longer lines (minified code) stay plain.
const MAX_LINE_BYTES: usize = 20_000;

/// A line's runs and the markup they were made from (`None`: not highlighted).
pub type Highlighted = Vec<Option<(String, Vec<CodeRun>)>>;

/// Scope prefixes and their palette token, most specific first. `None` keeps the text plain.
const SCOPES: &[(&str, Option<Token>)] = &[
    ("comment", Some(Token::Cm)),
    ("string", Some(Token::Str)),
    ("constant.character", Some(Token::Str)),
    ("constant.numeric", Some(Token::Nm)),
    ("constant.language", Some(Token::Nm)),
    ("support.constant", Some(Token::Nm)),
    ("keyword.operator", None),
    ("entity.name.type", Some(Token::Ty)),
    ("entity.name.class", Some(Token::Ty)),
    ("entity.name.tag", Some(Token::Ty)),
    ("entity.other.inherited-class", Some(Token::Ty)),
    ("support.type", Some(Token::Ty)),
    ("support.class", Some(Token::Ty)),
    ("keyword", Some(Token::Kw)),
    ("storage", Some(Token::Kw)),
    ("variable.language", Some(Token::Kw)),
    ("entity.name.function", Some(Token::Fn)),
    ("support.function", Some(Token::Fn)),
    ("entity.other.attribute-name", Some(Token::Fn)),
    ("meta.decorator", Some(Token::Fn)),
];

fn token_of(scopes: &[String]) -> Option<Token> {
    let has = |s: &str, p: &str| {
        s.strip_prefix(p)
            .is_some_and(|r| r.is_empty() || r.starts_with('.'))
    };
    scopes
        .iter()
        .rev()
        .find_map(|s| SCOPES.iter().find(|(p, _)| has(s, p)).map(|(_, t)| *t))
        .flatten()
}

/// The lines [`highlight_lines`] reads: through the [`MAX_HIGHLIGHT_LINES`]th code line.
pub fn budgeted(lines: &[DiffLine]) -> &[DiffLine] {
    let end = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.r#type != "hunk")
        .nth(MAX_HIGHLIGHT_LINES)
        .map_or(lines.len(), |(i, _)| i);
    &lines[..end]
}

/// Each line's runs, index for index. Hunks restart both sides at the grammar's top level.
pub fn highlight_lines(grammar: &Grammar, lines: &[DiffLine]) -> Highlighted {
    let (mut old, mut new): (Option<StateStack>, Option<StateStack>) = (None, None);
    lines
        .iter()
        .map(|l| {
            let ctx = l.r#type == "ctx";
            let stack = match l.r#type.as_str() {
                "hunk" => {
                    (old, new) = (None, None);
                    return None;
                }
                "del" => &mut old,
                "add" | "ctx" => &mut new,
                _ => return None,
            };
            let text: String = parse_code_runs(&l.code_html)
                .into_iter()
                .map(|r| r.text)
                .collect();
            if text.is_empty() || text.len() > MAX_LINE_BYTES {
                return None;
            }
            let r = grammar.tokenize_line(&text, stack.as_ref()).ok()?;
            *stack = Some(r.rule_stack);
            if ctx {
                old = new.clone();
            }
            let mut runs: Vec<CodeRun> = Vec::new();
            for t in &r.tokens {
                let piece = text.get(t.start_index..t.end_index.min(text.len()))?;
                let token = token_of(&t.scopes);
                match runs.last_mut() {
                    Some(last) if last.token == token => last.text.push_str(piece),
                    _ if piece.is_empty() => {}
                    _ => runs.push(CodeRun {
                        text: piece.to_string(),
                        token,
                    }),
                }
            }
            Some((l.code_html.clone(), runs))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_textmate::TmEngine;
    use std::sync::Arc;

    fn grammar() -> Arc<Grammar> {
        let mut e = TmEngine::new();
        e.register_grammar(
            "source.t",
            r#"{"scopeName":"source.t","patterns":[
                {"begin":"/\\*","end":"\\*/","name":"comment.block.t"},
                {"match":"\"[^\"]*\"","name":"string.quoted.double.t"},
                {"match":"\\b(fn|let)\\b","name":"keyword.control.t"},
                {"match":"\\b[0-9]+\\b","name":"constant.numeric.t"},
                {"match":"==","name":"keyword.operator.t"},
                {"match":"\\b([a-z]+)(?=\\()","captures":{"1":{"name":"entity.name.function.t"}}}
            ]}"#,
            "t.json",
            &[],
        );
        assert!(matches!(e.ensure_grammar("source.t"), Ok(true)));
        e.grammar("source.t").expect("compiled")
    }

    fn line(kind: &str, html: &str) -> DiffLine {
        DiffLine {
            r#type: kind.into(),
            code_html: html.into(),
            ..Default::default()
        }
    }

    fn tokens(h: &Option<(String, Vec<CodeRun>)>) -> Vec<(String, Option<Token>)> {
        h.as_ref()
            .map(|(_, r)| r.iter().map(|r| (r.text.clone(), r.token)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn highlights_into_the_palette_per_side() {
        let g = grammar();
        let lines = [
            line("hunk", "Lines 1–4"),
            line("ctx", "let a = f(1) == &#34;x&lt;&#34;"),
            line("del", "/* old"),
            line("add", "fn b"),
            line("del", "still */ 2"),
            line("hunk", "Lines 9–9"),
            line("add", "<span class=\"cm\">*/</span> 3"),
            line("ctx", ""),
        ];
        let h = highlight_lines(&g, &lines);
        assert_eq!(h.len(), lines.len());
        assert!(h[0].is_none() && h[5].is_none() && h[7].is_none());
        assert_eq!(
            tokens(&h[1]),
            vec![
                ("let".into(), Some(Token::Kw)),
                (" a = ".into(), None),
                ("f".into(), Some(Token::Fn)),
                ("(".into(), None),
                ("1".into(), Some(Token::Nm)),
                (") == ".into(), None),
                ("\"x<\"".into(), Some(Token::Str)),
            ]
        );
        assert_eq!(tokens(&h[3])[0], ("fn".into(), Some(Token::Kw)));
        assert_eq!(
            tokens(&h[4]),
            vec![
                ("still */".into(), Some(Token::Cm)),
                (" ".into(), None),
                ("2".into(), Some(Token::Nm)),
            ]
        );
        assert_eq!(tokens(&h[6])[1], ("3".into(), Some(Token::Nm)));
        assert_eq!(
            h[6].as_ref().map(|(m, _)| m.as_str()),
            Some(lines[6].code_html.as_str())
        );
    }

    #[test]
    fn budget_stops_at_the_cap() {
        let mut lines = vec![line("hunk", "Lines 1–9")];
        lines.extend((0..MAX_HIGHLIGHT_LINES + 5).map(|_| line("add", "let")));
        let b = budgeted(&lines);
        assert_eq!(b.len(), MAX_HIGHLIGHT_LINES + 1);
        assert!(
            highlight_lines(&grammar(), b)
                .iter()
                .skip(1)
                .all(|h| tokens(h) == vec![("let".into(), Some(Token::Kw))])
        );
        assert_eq!(budgeted(&lines[..3]).len(), 3);
    }
}
