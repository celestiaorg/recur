//! Bundle `[[languageData]]`: comment tokens, indent rules, bracket pairing
//! (the editor-independent parts of `cm-lang-data.ts`). Patterns are
//! JavaScript regex sources, compiled with [`JsRegex`].

use serde::{Deserialize, Serialize};

use crate::jsregex::JsRegex;
use crate::utils::js_is_space;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LanguageDataFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_comment_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_comment_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub increase_indent_pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decrease_indent_pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indent_next_line_pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zero_indent_pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_indent_corrections: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_brackets: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockComment {
    pub open: String,
    pub close: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommentTokens {
    pub line: Option<String>,
    pub block: Option<BlockComment>,
}

pub fn comment_tokens_of(d: &LanguageDataFields) -> Option<CommentTokens> {
    let trimmed = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let line = trimmed(&d.line_comment);
    let block = match (
        trimmed(&d.block_comment_start),
        trimmed(&d.block_comment_end),
    ) {
        (Some(open), Some(close)) => Some(BlockComment { open, close }),
        _ => None,
    };
    if line.is_none() && block.is_none() {
        return None;
    }
    Some(CommentTokens { line, block })
}

pub fn compile_regex(src: Option<&str>) -> Option<JsRegex> {
    let s = src?.trim();
    if s.is_empty() {
        return None;
    }
    JsRegex::new(s, "")
}

pub struct CompiledIndent {
    pub increase: Option<JsRegex>,
    pub decrease: Option<JsRegex>,
    pub indent_next: Option<JsRegex>,
    pub zero: Option<JsRegex>,
    pub disable_indent: bool,
}

pub fn compile_indent(d: &LanguageDataFields) -> Option<CompiledIndent> {
    let c = CompiledIndent {
        increase: compile_regex(d.increase_indent_pattern.as_deref()),
        decrease: compile_regex(d.decrease_indent_pattern.as_deref()),
        indent_next: compile_regex(d.indent_next_line_pattern.as_deref()),
        zero: compile_regex(d.zero_indent_pattern.as_deref()),
        disable_indent: d.disable_indent_corrections == Some(true),
    };
    if c.increase.is_none()
        && c.decrease.is_none()
        && c.indent_next.is_none()
        && c.zero.is_none()
        && !c.disable_indent
    {
        return None;
    }
    Some(c)
}

pub fn matches_pattern(re: Option<&JsRegex>, text: &str) -> bool {
    re.is_some_and(|r| r.test(text))
}

/// Leading whitespace width in columns.
pub fn leading_indent(text: &str, tab_size: usize) -> usize {
    let mut n = 0;
    for c in text.chars() {
        match c {
            ' ' => n += 1,
            '\t' => n += tab_size,
            _ => break,
        }
    }
    n
}

pub fn has_non_space(text: &str) -> bool {
    text.chars().any(|c| !js_is_space(c))
}

/// Indent columns for `line_text` given the previous non-empty line.
pub fn compute_indent(
    prev_text: Option<&str>,
    line_text: &str,
    unit: usize,
    tab_size: usize,
    p: &CompiledIndent,
) -> usize {
    let empty = !has_non_space(line_text);
    if matches_pattern(p.zero.as_ref(), line_text) {
        return 0;
    }
    let opens = |t: &str| {
        matches_pattern(p.increase.as_ref(), t) || matches_pattern(p.indent_next.as_ref(), t)
    };
    if matches_pattern(p.decrease.as_ref(), line_text) {
        let mut base = prev_text.map_or(0, |t| leading_indent(t, tab_size));
        if prev_text.is_some_and(opens) {
            base += unit;
        }
        return base.saturating_sub(unit);
    }
    if !empty && p.disable_indent {
        return leading_indent(line_text, tab_size);
    }
    let Some(prev) = prev_text else { return 0 };
    let mut base = leading_indent(prev, tab_size);
    if opens(prev) {
        base += unit;
    }
    base
}

/// CodeMirror `indentString`: tabs when the indent unit is a tab.
pub fn indent_string(cols: usize, indent_unit: &str, tab_size: usize) -> String {
    let mut out = String::new();
    let mut cols = cols;
    let mut ch = indent_unit.chars().next().unwrap_or(' ');
    if ch == '\t' {
        while cols >= tab_size && tab_size > 0 {
            out.push('\t');
            cols -= tab_size;
        }
        ch = ' ';
    }
    out.extend(std::iter::repeat_n(ch, cols));
    out
}

/// CodeMirror `getIndentUnit`: the unit's width in columns.
pub fn indent_unit_width(indent_unit: &str, tab_size: usize) -> usize {
    if indent_unit.starts_with('\t') {
        tab_size * indent_unit.chars().count()
    } else {
        indent_unit.chars().count()
    }
}

const VOID_TAGS: &[&str] = &[
    "area", "base", "basefont", "br", "col", "embed", "frame", "hr", "img", "input", "isindex",
    "link", "meta", "param", "source", "track", "wbr",
];

fn trim_end_js(s: &str) -> &str {
    s.trim_end_matches(js_is_space)
}

fn trim_start_js(s: &str) -> &str {
    s.trim_start_matches(js_is_space)
}

/// The caret sits in an empty open/close tag pair (`<div>|</div>`).
pub fn between_tag_pair(before: &str, after: &str) -> Option<String> {
    let left = trim_end_js(before);
    let right = trim_start_js(after);
    let open = JsRegex::new("<([A-Za-z][-A-Za-z0-9._]*)\\b[^>]*?>$", "")?;
    let groups = open.exec_at(left, 0)?;
    let (s, e) = groups.get(1).copied().flatten()?;
    let name = &left[s..e];
    if JsRegex::new("\\/\\s*>$", "").is_some_and(|r| r.test(left)) {
        return None;
    }
    if VOID_TAGS.iter().any(|v| v.eq_ignore_ascii_case(name)) {
        return None;
    }
    let close = JsRegex::new(&format!("^</{}\\s*>", regex_escape(name)), "i")?;
    close.test(right).then(|| name.to_string())
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The caret sits in empty `{}` / `()` / `[]`.
pub fn between_bracket_pair(before: &str, after: &str) -> bool {
    let left = trim_end_js(before);
    let right = trim_start_js(after);
    let close = match left.chars().last() {
        Some('(') => ')',
        Some('[') => ']',
        Some('{') => '}',
        _ => return false,
    };
    right.starts_with(close)
}

/// The edit TextMate's "Return Inside Empty Item" makes: newline, indented
/// inner line, closer on its own line. Offsets are byte offsets within the
/// line; `cursor` is where the caret lands, relative to the line start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairNewline {
    pub from: usize,
    pub to: usize,
    pub insert: String,
    pub cursor: usize,
}

pub fn newline_between_pairs(
    line: &str,
    col: usize,
    indent_unit: &str,
    tab_size: usize,
) -> Option<PairNewline> {
    let before = &line[..col];
    let after = &line[col..];
    if between_tag_pair(before, after).is_none() && !between_bracket_pair(before, after) {
        return None;
    }
    let left = trim_end_js(before);
    let right_trim = after.find(|c: char| !js_is_space(c));
    let from = left.len();
    let to = right_trim.map_or(col, |r| col + r);
    let base = leading_indent(line, tab_size);
    let inner = indent_string(
        base + indent_unit_width(indent_unit, tab_size),
        indent_unit,
        tab_size,
    );
    let outer = indent_string(base, indent_unit, tab_size);
    Some(PairNewline {
        from,
        to,
        cursor: from + 1 + inner.len(),
        insert: format!("\n{inner}\n{outer}"),
    })
}

/// CodeMirror's default openers plus backtick.
pub const BASE_BRACKETS: &[&str] = &["(", "[", "{", "'", "\"", "`"];
/// Self-pairing characters the skip-inside-quote rule applies to.
pub const SELF_PAIR_QUOTES: &[&str] = &["'", "\"", "`", "_", "*"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseBracketsConfig {
    pub brackets: Vec<String>,
    pub before: String,
}

pub fn close_brackets_of(d: &LanguageDataFields) -> CloseBracketsConfig {
    let mut brackets: Vec<String> = BASE_BRACKETS.iter().map(|s| s.to_string()).collect();
    for raw in d.close_brackets.iter().flatten() {
        let ch = raw.trim();
        if ch.is_empty() || ch.chars().count() != 1 || brackets.iter().any(|b| b == ch) {
            continue;
        }
        brackets.push(ch.to_string());
    }
    CloseBracketsConfig {
        brackets,
        before: ")]}:;>".into(),
    }
}

/// An odd number of unescaped `quote` characters precede `pos` on its line.
pub fn inside_paired_quote(doc: &str, pos: usize, quote: char) -> bool {
    let line_start = doc[..pos].rfind('\n').map_or(0, |i| i + 1);
    let escape_quotes = matches!(quote, '"' | '\'' | '`');
    let mut open = false;
    let mut chars = doc[line_start..pos].chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' && escape_quotes {
            chars.next();
            continue;
        }
        if ch == quote {
            open = !open;
        }
    }
    open
}

/// The editor-facing language data for a grammar (`languageDataExtension`).
/// `None` when the record carries nothing (CodeMirror defaults apply).
pub struct LanguageData {
    pub comment_tokens: Option<CommentTokens>,
    pub indent: Option<CompiledIndent>,
    pub close_brackets: CloseBracketsConfig,
}

pub fn language_data(fields: &LanguageDataFields) -> Option<LanguageData> {
    let comment_tokens = comment_tokens_of(fields);
    let indent = compile_indent(fields);
    let extras = fields
        .close_brackets
        .iter()
        .flatten()
        .any(|c| !c.trim().is_empty());
    if comment_tokens.is_none() && indent.is_none() && !extras {
        return None;
    }
    Some(LanguageData {
        comment_tokens,
        indent,
        close_brackets: close_brackets_of(fields),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Option<String> {
        Some(x.to_string())
    }

    fn go() -> LanguageDataFields {
        LanguageDataFields {
            line_comment: s("//"),
            block_comment_start: s("/*"),
            block_comment_end: s("*/"),
            increase_indent_pattern: s("[{(\\[]\\s*(?://.*)?$|^\\s*(?:case|default)\\b.*$"),
            decrease_indent_pattern: s("^\\s*(?:[})\\]]|(?:case|default)\\b)"),
            ..Default::default()
        }
    }

    fn html() -> LanguageDataFields {
        LanguageDataFields {
            block_comment_start: s("<!--"),
            block_comment_end: s("-->"),
            increase_indent_pattern: s(
                "<(?!\\/)(?!area\\b|base\\b|br\\b|col\\b|embed\\b|hr\\b|img\\b|input\\b|link\\b|meta\\b|param\\b|source\\b|track\\b|wbr\\b)[A-Za-z][-A-Za-z0-9]*(?:\\s[^>]*)?>\\s*$",
            ),
            decrease_indent_pattern: s("^\\s*<\\/[A-Za-z][-A-Za-z0-9]*>"),
            close_brackets: Some(vec!["<".into()]),
            ..Default::default()
        }
    }

    fn yaml() -> LanguageDataFields {
        LanguageDataFields {
            line_comment: s("#"),
            increase_indent_pattern: s("^\\s*.*(:|-)\\s?(?:&\\w+)?(?:\\{[^}\"']*|\\([^)\"']*)?$"),
            decrease_indent_pattern: s("^\\s+\\}$"),
            disable_indent_corrections: Some(true),
            ..Default::default()
        }
    }

    fn python() -> LanguageDataFields {
        LanguageDataFields {
            line_comment: s("#"),
            increase_indent_pattern: s(
                "^\\s*(?:class|(?:async\\s+)?(?:def|for|with)|elif|else|except|finally|if|match|case|try|while)\\b.*:\\s*(?:#.*)?$|[{[]\\s*(?:#.*)?$",
            ),
            decrease_indent_pattern: s("^\\s*(?:elif|else|except|finally)\\b|^\\s*[}\\]]"),
            disable_indent_corrections: Some(true),
            ..Default::default()
        }
    }

    #[test]
    fn comment_tokens_for_yaml_html_go() {
        assert_eq!(
            comment_tokens_of(&yaml()),
            Some(CommentTokens {
                line: s("#"),
                block: None
            })
        );
        assert_eq!(
            comment_tokens_of(&html()),
            Some(CommentTokens {
                line: None,
                block: Some(BlockComment {
                    open: "<!--".into(),
                    close: "-->".into()
                })
            })
        );
        assert_eq!(
            comment_tokens_of(&go()).unwrap().line.as_deref(),
            Some("//")
        );
        assert_eq!(
            comment_tokens_of(&LanguageDataFields {
                increase_indent_pattern: s("{$"),
                ..Default::default()
            }),
            None
        );
    }

    #[test]
    fn compiles_html_yaml_make_regexes() {
        assert!(compile_regex(html().increase_indent_pattern.as_deref()).is_some());
        assert!(compile_indent(&yaml()).is_some());
        assert!(compile_regex(Some("^[^\\t ]+:")).is_some());
    }

    #[test]
    fn computes_indent_like_the_web_editor() {
        let g = compile_indent(&go()).unwrap();
        let py = compile_indent(&python()).unwrap();
        let h = compile_indent(&html()).unwrap();
        assert_eq!(compute_indent(Some("func main() {"), "", 4, 4, &g), 4);
        assert_eq!(compute_indent(Some("\tx := 1"), "}", 4, 4, &g), 0);
        assert_eq!(compute_indent(Some("\tif a {"), "\t\t}", 4, 4, &g), 4);
        assert_eq!(compute_indent(Some("if True:"), "", 4, 4, &py), 4);
        assert_eq!(compute_indent(Some("if True:"), "    x = 1", 4, 4, &py), 4);
        assert_eq!(compute_indent(Some("<div>"), "", 2, 2, &h), 2);
        assert_eq!(compute_indent(Some("  hello"), "</div>", 2, 2, &h), 0);
    }

    fn enter(doc: &str, pos: usize) -> String {
        match newline_between_pairs(doc, pos, "  ", 4) {
            Some(e) => format!("{}{}{}", &doc[..e.from], e.insert, &doc[e.to..]),
            None => doc.to_string(),
        }
    }

    #[test]
    fn splits_empty_pairs() {
        assert_eq!(enter("<div></div>", 5), "<div>\n  \n</div>");
        assert_eq!(enter("  <p></p>", 5), "  <p>\n    \n  </p>");
        assert_eq!(enter("<br></br>", 4), "<br></br>");
        assert_eq!(enter("<div>", 5), "<div>");
        assert_eq!(enter("{}", 1), "{\n  \n}");
        assert_eq!(enter("()", 1), "(\n  \n)");
        assert_eq!(enter("[]", 1), "[\n  \n]");
        assert_eq!(enter("  {  }", 3), "  {\n    \n  }");
        assert_eq!(enter("{]", 1), "{]");
        let e = newline_between_pairs("{}", 1, "  ", 4).unwrap();
        assert_eq!(e.cursor, 4);
    }

    #[test]
    fn close_bracket_extras() {
        assert_eq!(
            close_brackets_of(&html()).brackets,
            ["(", "[", "{", "'", "\"", "`", "<"]
        );
        let md = LanguageDataFields {
            block_comment_start: s("<!--"),
            block_comment_end: s("-->"),
            close_brackets: Some(vec!["<".into(), "_".into(), "*".into()]),
            ..Default::default()
        };
        let b = close_brackets_of(&md).brackets;
        assert!(b.contains(&"_".to_string()) && b.contains(&"*".to_string()));
        assert!(!close_brackets_of(&go()).brackets.contains(&"<".to_string()));
        assert!(language_data(&LanguageDataFields::default()).is_none());
    }

    #[test]
    fn inside_paired_quote_ignores_escapes() {
        assert!(inside_paired_quote("\"hello", 6, '"'));
        assert!(!inside_paired_quote("\"hello\"", 7, '"'));
        assert!(inside_paired_quote("\"say \\\"hi", 9, '"'));
        assert!(!inside_paired_quote("foo", 3, '"'));
    }

    #[test]
    fn indent_strings() {
        assert_eq!(indent_string(9, "\t", 4), "\t\t ");
        assert_eq!(indent_string(3, "  ", 4), "   ");
        assert_eq!(indent_unit_width("\t", 4), 4);
        assert_eq!(indent_unit_width("  ", 4), 2);
    }
}
