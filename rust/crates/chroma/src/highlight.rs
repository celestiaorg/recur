//! Syntax highlighting of diff lines into the small token palette the review
//! UI styles (`.kw .str .fn .cm .ty .nm`).
//!
//! Highlighting uses this crate's port of chroma, the library the
//! Go app used, so persisted `codeHtml` and freshly highlighted lines are
//! byte-identical across the two apps.

use recur_domain::FileDiff;

use crate::TokenType;
use crate::text::{go_lower, html_escape, html_unescape};

/// Max highlighted lines in one file: the diff view's initial line cap, so
/// the first screen is uniformly styled. Later lines stay plain-escaped so
/// large full-file adds stay responsive.
pub const MAX_HIGHLIGHT_LINES: usize = 500;

/// Cap across a whole patch: a 100-file review × 500 lines would otherwise
/// block the request for seconds.
pub(crate) const MAX_HIGHLIGHT_TOTAL_LINES: usize = 8 * MAX_HIGHLIGHT_LINES;

/// Upgrades plain (escaped) `code_html` on add/del/ctx lines to highlighted
/// markup. Lines that already contain token spans are left alone. Returns
/// whether any line changed, so callers can persist the result once.
pub fn highlight_files(files: &mut [FileDiff]) -> bool {
    let mut changed = false;
    let mut left = MAX_HIGHLIGHT_TOTAL_LINES;
    for f in files {
        if left == 0 {
            break;
        }
        let (c, n) = highlight_file_budget(f, left);
        left -= n;
        changed |= c;
    }
    changed
}

fn highlight_file_budget(f: &mut FileDiff, budget: usize) -> (bool, usize) {
    let budget = budget.min(MAX_HIGHLIGHT_LINES);
    let mut changed = false;
    let mut n = 0;
    let path = f.path.clone();
    for l in f.lines.iter_mut().flatten() {
        if l.r#type == "hunk" || l.code_html.is_empty() || l.code_html.contains("<span class=\"") {
            continue;
        }
        if n >= budget {
            break;
        }
        n += 1;
        let next = highlight_line(&path, &html_unescape(&l.code_html));
        if next != l.code_html {
            l.code_html = next;
            changed = true;
        }
    }
    (changed, n)
}

/// The lowercase chroma lexer name for a path (`go`, `typescript`, …), or
/// empty when none matches. Shares the lexer cache with highlighting.
pub fn lexer_name(path: &str) -> String {
    crate::lexer_for(path)
        .map(|l| go_lower(&l.config().name))
        .unwrap_or_default()
}

/// One source line as escaped HTML with token spans. Unknown languages and
/// lexer failures fall back to plain escaping, so raw markup never leaks.
pub(crate) fn highlight_line(path: &str, code: &str) -> String {
    if code.is_empty() {
        return String::new();
    }
    let Some(lexer) = crate::lexer_for(path) else {
        return html_escape(code);
    };
    let Ok(tokens) = crate::tokenise(&lexer, code) else {
        return html_escape(code);
    };
    let mut b = String::new();
    for tok in tokens {
        let text = html_escape(&tok.value);
        if text.is_empty() {
            continue;
        }
        match token_class(tok.ty) {
            "" => b.push_str(&text),
            class => {
                b.push_str("<span class=\"");
                b.push_str(class);
                b.push_str("\">");
                b.push_str(&text);
                b.push_str("</span>");
            }
        }
    }
    b
}

/// Maps chroma token types onto Recur's token palette. The order matters:
/// every literal (numbers included) is in the string category.
fn token_class(t: TokenType) -> &'static str {
    if t.in_category(TokenType::COMMENT) {
        "cm"
    } else if t.in_category(TokenType::LITERAL_STRING) {
        "str"
    } else if [
        TokenType::KEYWORD_TYPE,
        TokenType::NAME_CLASS,
        TokenType::NAME_TAG,
        TokenType::NAME_EXCEPTION,
    ]
    .contains(&t)
    {
        "ty"
    } else if t.in_category(TokenType::KEYWORD) {
        "kw"
    } else if [
        TokenType::NAME_FUNCTION,
        TokenType::NAME_FUNCTION_MAGIC,
        TokenType::NAME_DECORATOR,
        TokenType::NAME_ATTRIBUTE,
    ]
    .contains(&t)
    {
        "fn"
    } else if [
        TokenType::NAME_CONSTANT,
        TokenType::NAME_BUILTIN,
        TokenType::NAME_ENTITY,
    ]
    .contains(&t)
        || t.in_category(TokenType::LITERAL_NUMBER)
    {
        "nm"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use recur_domain::DiffLine;

    use super::*;

    #[test]
    fn highlight_line_typescript_and_fallbacks() {
        let got = highlight_line(
            "src/auth/oauth.ts",
            "const secret = await secrets.get('oauth/client_secret')",
        );
        for want in [
            "<span class=\"kw\">const</span>",
            "<span class=\"kw\">await</span>",
            "class=\"str\"",
            "oauth/client_secret",
        ] {
            assert!(
                got.contains(want),
                "highlight_line(ts) missing {want:?} in {got:?}"
            );
        }
        assert_eq!(
            highlight_line("NOTES", "<b>hi</b>"),
            "&lt;b&gt;hi&lt;/b&gt;"
        );
        assert_eq!(highlight_line("a.go", ""), "");
    }

    #[test]
    fn highlight_files_backfills_plain() {
        let mut files = vec![FileDiff {
            path: "tetris.py".into(),
            lines: Some(vec![
                DiffLine {
                    r#type: "hunk".into(),
                    code_html: "Lines 1–3".into(),
                    ..Default::default()
                },
                DiffLine {
                    r#type: "add".into(),
                    new: "1".into(),
                    code_html: "def draw_board(stdscr, game):".into(),
                    ..Default::default()
                },
                DiffLine {
                    r#type: "add".into(),
                    new: "2".into(),
                    code_html: "<span class=\"kw\">def</span> already()".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }];
        assert!(
            highlight_files(&mut files),
            "HighlightFiles should report a change for plain lines"
        );
        let lines = files[0].lines.as_deref().unwrap_or_default();
        assert!(
            lines[1].code_html.contains("class=\"kw\""),
            "plain python line not highlighted: {:?}",
            lines[1].code_html
        );
        assert_eq!(
            lines[2].code_html,
            "<span class=\"kw\">def</span> already()"
        );
        assert!(
            !highlight_files(&mut files),
            "second HighlightFiles pass should be a no-op"
        );
    }

    #[test]
    fn highlight_files_total_budget() {
        let mut files: Vec<FileDiff> = (0..9)
            .map(|i| FileDiff {
                path: format!("f{i}.go"),
                lines: Some(
                    (0..MAX_HIGHLIGHT_LINES)
                        .map(|j| DiffLine {
                            r#type: "add".into(),
                            new: (j + 1).to_string(),
                            code_html: "package main".into(),
                            ..Default::default()
                        })
                        .collect(),
                ),
                ..Default::default()
            })
            .collect();
        highlight_files(&mut files);
        let highlighted = files
            .iter()
            .flat_map(|f| f.lines.iter().flatten())
            .filter(|l| l.code_html.contains("<span class=\""))
            .count();
        assert_eq!(highlighted, MAX_HIGHLIGHT_TOTAL_LINES);
    }

    #[test]
    fn lexer_names() {
        assert_eq!(lexer_name("main.go"), "go");
        assert_eq!(lexer_name("web/app.ts"), "typescript");
        assert_eq!(lexer_name("NOTES"), "");
    }
}
