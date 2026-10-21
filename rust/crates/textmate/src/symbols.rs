//! Open-file outline from TextMate token scopes + bundle `[[symbol]]` rules
//! (`tm-symbols.ts`). Rules come in as data (the app lists them from
//! bundles); the outline covers what the tokenizer has filled.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::{ScopeToken, TmEngine};
use crate::jsregex::JsRegex;
use crate::scope_selector::{
    CompiledSelector, compile_selector, scope_prefix_match, selector_score,
};

/// A `[[symbol]]` rule (Go `SymbolInfo` fields used here).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SymbolRule {
    pub id: String,
    pub scope: String,
    pub show: bool,
    pub transform: String,
    pub kind: String,
}

/// Go `domain.OutlineSymbol`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutlineSymbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<usize>,
}

pub struct CompiledRule {
    pub rule: SymbolRule,
    pub sel: CompiledSelector,
}

pub const MAX_SYMBOLS: usize = 200;

pub fn compile_rules(rules: &[SymbolRule]) -> Vec<CompiledRule> {
    rules
        .iter()
        .filter(|r| !r.scope.is_empty())
        .map(|r| CompiledRule {
            sel: compile_selector(&r.scope),
            rule: r.clone(),
        })
        .collect()
}

fn unescape_repl(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('u') => {
                let hex: String = chars.clone().skip(1).take(4).collect();
                match (hex.len() == 4)
                    .then(|| u32::from_str_radix(&hex, 16).ok())
                    .flatten()
                {
                    Some(code) => {
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        for _ in 0..5 {
                            chars.next();
                        }
                    }
                    None => out.push('\\'),
                }
            }
            Some('n') => {
                out.push('\n');
                chars.next();
            }
            Some('t') => {
                out.push('\t');
                chars.next();
            }
            Some('/') => {
                out.push('/');
                chars.next();
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// Apply `s/pattern/replacement/flags` lines in order.
pub fn apply_symbol_transform(text: &str, transform: &str) -> String {
    let src = transform.trim();
    if src.is_empty() {
        return text.to_string();
    }
    let Some(re) = JsRegex::new(
        r"^\s*s\/((?:\\.|[^/])*)\/((?:\\.|[^/])*)\/([gimsuy]*)",
        "gm",
    ) else {
        return text.to_string();
    };
    let mut out = text.to_string();
    let mut any = false;
    let mut pos = 0;
    while let Some(groups) = re.exec_at(src, pos) {
        let Some((_, end)) = groups[0] else { break };
        any = true;
        let g = |i: usize| {
            groups
                .get(i)
                .copied()
                .flatten()
                .map_or("", |(s, e)| &src[s..e])
        };
        // Invalid JS regexes (Oniguruma leftovers) are dropped.
        if let Some(step) = JsRegex::new(g(1), g(3)) {
            out = step.replace(&out, &unescape_repl(g(2)));
        }
        pos = end.max(pos + 1);
        if pos > src.len() {
            break;
        }
    }
    if any { out } else { text.to_string() }
}

/// Fenced / indented code in Markdown: don't list inner languages.
fn in_fenced_code(scopes: &[String]) -> bool {
    scopes.iter().any(|s| {
        scope_prefix_match("markup.fenced_code", s) || scope_prefix_match("markup.raw.block", s)
    })
}

fn has_word(s: &str, w: &str) -> bool {
    let b = s.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    s.match_indices(w).any(|(i, _)| {
        let before = i == 0 || !word(b[i - 1]);
        let after = i + w.len() >= b.len() || !word(b[i + w.len()]);
        before && after
    })
}

pub fn infer_symbol_kind(rule: &SymbolRule) -> String {
    if !rule.kind.is_empty() {
        return rule.kind.clone();
    }
    let s = &rule.scope;
    let kind = if s.contains("heading") || s.contains("section") || s.contains("toc-list") {
        "heading"
    } else if has_word(s, "class") {
        "class"
    } else if s.contains("function") {
        "func"
    } else if s.contains("entity.name.type") {
        "type"
    } else if s.match_indices(".tag").any(|(i, _)| {
        let after = s.as_bytes().get(i + 4);
        after.is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
    }) {
        "key"
    } else if s.contains("selector") || s.contains("attribute.id") {
        "id"
    } else {
        "symbol"
    };
    kind.to_string()
}

pub fn best_symbol_rule<'a>(
    scopes: &[String],
    rules: &'a [CompiledRule],
) -> Option<&'a CompiledRule> {
    let mut best = None;
    let mut best_score = 0;
    for rule in rules {
        let score = selector_score(&rule.sel, scopes);
        if score > best_score {
            best = Some(rule);
            best_score = score;
        }
    }
    best
}

fn escape_regexp(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Prefix a Go method with its receiver type (`Engine.Foo`); same line only.
pub fn qualify_go_receiver(line: &str, name: &str) -> String {
    if name.is_empty() || name.contains('.') {
        return name.to_string();
    }
    let trimmed = line.trim_start();
    if !trimmed.starts_with("func") {
        return name.to_string();
    }
    let Some(re) = JsRegex::new(
        &format!("^func\\s+\\(([^)]*)\\)\\s+{}\\b", escape_regexp(name)),
        "",
    ) else {
        return name.to_string();
    };
    let Some(groups) = re.exec_at(trimmed, 0) else {
        return name.to_string();
    };
    let Some((s, e)) = groups.get(1).copied().flatten() else {
        return name.to_string();
    };
    let mut recv = trimmed[s..e].trim().to_string();
    if let Some(var) = JsRegex::new("^[_A-Za-z][\\w]*\\s+", "") {
        recv = var.replace(&recv, "");
    }
    recv = recv.trim_start_matches('*').to_string();
    if let Some(generic) = JsRegex::new("\\[[^\\]]*\\]$", "") {
        recv = generic.replace(&recv, "");
    }
    let recv = recv.trim();
    let ty: String = match recv.chars().next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => recv
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
            .collect(),
        _ => String::new(),
    };
    if ty.is_empty() || ty == name {
        return name.to_string();
    }
    format!("{ty}.{name}")
}

/// Collect outline symbols from per-line scope tokens.
pub fn collect_symbols(
    lines: &[String],
    tokens_for_line: impl Fn(usize) -> Option<Vec<ScopeToken>>,
    rules: &[SymbolRule],
    max: usize,
) -> Vec<OutlineSymbol> {
    let compiled = compile_rules(rules);
    let mut out: Vec<OutlineSymbol> = Vec::new();
    if compiled.is_empty() {
        return out;
    }
    struct Pending<'a> {
        rule: &'a CompiledRule,
        name: String,
        line: usize,
        src: String,
    }
    let flush = |pending: &mut Option<Pending>, out: &mut Vec<OutlineSymbol>| {
        let Some(p) = pending.take() else { return };
        // Trim the raw token text, not the transform result: Markdown
        // heading indent is leading em-spaces from the transform.
        let label = apply_symbol_transform(
            &qualify_go_receiver(&p.src, p.name.trim()),
            &p.rule.rule.transform,
        );
        if label.trim().is_empty() {
            return;
        }
        out.push(OutlineSymbol {
            name: label,
            kind: infer_symbol_kind(&p.rule.rule),
            line: p.line,
            end: None,
        });
    };
    let mut pending: Option<Pending> = None;
    for n in 1..=lines.len() {
        if out.len() >= max {
            break;
        }
        let Some(tokens) = tokens_for_line(n) else {
            flush(&mut pending, &mut out);
            continue;
        };
        let line = &lines[n - 1];
        for tok in tokens {
            if out.len() >= max {
                break;
            }
            if in_fenced_code(&tok.scopes) {
                flush(&mut pending, &mut out);
                continue;
            }
            let Some(rule) = best_symbol_rule(&tok.scopes, &compiled).filter(|r| r.rule.show)
            else {
                flush(&mut pending, &mut out);
                continue;
            };
            let chunk = line
                .get(tok.start.min(line.len())..tok.end.min(line.len()))
                .unwrap_or("");
            if chunk.is_empty() {
                continue;
            }
            if let Some(p) = pending.as_mut()
                && p.rule.rule.id == rule.rule.id
                && p.line == n
            {
                p.name.push_str(chunk);
                continue;
            }
            flush(&mut pending, &mut out);
            pending = Some(Pending {
                rule,
                name: chunk.to_string(),
                line: n,
                src: line.clone(),
            });
        }
        flush(&mut pending, &mut out);
    }
    out
}

/// Outline the filled part of an engine document.
pub fn collect_from_engine(
    engine: &TmEngine,
    doc_id: &str,
    rules: &[SymbolRule],
) -> Vec<OutlineSymbol> {
    let total = engine.line_count(doc_id);
    if total == 0 {
        return Vec::new();
    }
    let filled = total.min(engine.filled_through(doc_id));
    let lines: Vec<String> = (1..=filled)
        .map(|n| engine.line_at(doc_id, n).unwrap_or("").to_string())
        .collect();
    collect_symbols(
        &lines,
        |n| engine.scopes_for_line(doc_id, n),
        rules,
        MAX_SYMBOLS,
    )
}

/// Outline an open document (`outlineOpenFile` without the async plumbing):
/// `None` when the file has no grammar / engine document / rules, so the
/// caller falls back to the language-agnostic outline. Fills up to 80ms
/// first; `complete` is false when the rest still needs the idle sweep.
pub fn outline_open_file(
    engine: &mut TmEngine,
    doc_id: &str,
    rules: &[SymbolRule],
) -> Option<(Vec<OutlineSymbol>, bool)> {
    if doc_id.is_empty() || !rules.iter().any(|r| !r.scope.is_empty()) || !engine.has_doc(doc_id) {
        return None;
    }
    let scope = engine.doc_scope(doc_id)?.to_string();
    engine.ensure_grammar(&scope).ok()?;
    engine.fill_sync(doc_id, Duration::from_millis(80));
    let syms = collect_from_engine(engine, doc_id, rules);
    let complete = engine.filled_through(doc_id) >= engine.line_count(doc_id);
    Some((syms, complete))
}

/// Last symbol at or before `line` (the status-bar current symbol).
pub fn symbol_at_line(syms: &[OutlineSymbol], line: usize) -> Option<&OutlineSymbol> {
    let mut best: Option<&OutlineSymbol> = None;
    for s in syms {
        if s.line > line {
            continue;
        }
        if best.is_none_or(|b| s.line >= b.line) {
            best = Some(s);
        }
    }
    best
}

/// Bare identifier for definitions / references.
pub fn symbol_lookup_name(name: &str, kind: &str) -> String {
    let t = name.trim();
    if !kind.is_empty() && kind != "func" {
        return t.to_string();
    }
    match t.rfind('.') {
        Some(i) => t[i + 1..].to_string(),
        None => t.to_string(),
    }
}

/// A ranked symbol for Go to Symbol.
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolHit<'a> {
    pub item: &'a OutlineSymbol,
    pub score: i64,
    pub label: String,
}

/// Go to Symbol ranking. An empty query keeps document order; typed
/// queries use the app's fuzzy ranker over `"<name> <kind>"` and tie-break
/// by line. `rank` is `fuzzyFilterPrepared` (lib/fuzzy, ported with Quick
/// Open): it returns `(index, score)` pairs for the best `limit` labels.
pub fn filter_symbols<'a>(
    symbols: &'a [OutlineSymbol],
    query: &str,
    limit: usize,
    rank: impl FnOnce(&[String], &str, usize) -> Vec<(usize, i64)>,
) -> Vec<SymbolHit<'a>> {
    if query.trim().is_empty() {
        return symbols
            .iter()
            .take(limit)
            .map(|s| SymbolHit {
                item: s,
                score: 0,
                label: s.name.clone(),
            })
            .collect();
    }
    let labels: Vec<String> = symbols
        .iter()
        .map(|s| format!("{} {}", s.name.trim(), s.kind))
        .collect();
    let mut ranked: Vec<SymbolHit> = rank(&labels, query, limit)
        .into_iter()
        .filter_map(|(i, score)| {
            Some(SymbolHit {
                item: symbols.get(i)?,
                score,
                label: labels[i].clone(),
            })
        })
        .collect();
    ranked.sort_by(|a, b| b.score.cmp(&a.score).then(a.item.line.cmp(&b.item.line)));
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, scope: &str, show: bool, transform: &str, kind: &str) -> SymbolRule {
        SymbolRule {
            id: id.into(),
            scope: scope.into(),
            show,
            transform: transform.into(),
            kind: kind.into(),
        }
    }

    fn func_rule() -> SymbolRule {
        rule("func", "entity.name.function", true, "", "func")
    }

    fn hide_support() -> SymbolRule {
        rule(
            "hide-support",
            "entity.name.function.support",
            false,
            "",
            "",
        )
    }

    fn id_rule() -> SymbolRule {
        rule(
            "html-id",
            "meta.attribute.id.html > string",
            true,
            "s/^[\"']|[\"']$//g\ns/^/ID: /",
            "id",
        )
    }

    fn tok(start: usize, end: usize, scopes: &[&str]) -> ScopeToken {
        ScopeToken {
            start,
            end,
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn sym(name: &str, kind: &str, line: usize) -> OutlineSymbol {
        OutlineSymbol {
            name: name.into(),
            kind: kind.into(),
            line,
            end: None,
        }
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn applies_sequential_replacements() {
        assert_eq!(
            apply_symbol_transform("\"main\"", &id_rule().transform),
            "ID: main"
        );
        assert_eq!(
            apply_symbol_transform("func Foo", "s/^\\s*func\\s+/ /"),
            " Foo"
        );
        assert_eq!(apply_symbol_transform("keep", ""), "keep");
        assert_eq!(
            apply_symbol_transform("## Layout", "s/^##\\s+/\\u2003/"),
            "\u{2003}Layout"
        );
    }

    #[test]
    fn infers_kinds() {
        assert_eq!(infer_symbol_kind(&func_rule()), "func");
        assert_eq!(
            infer_symbol_kind(&rule("x", "entity.name.section", true, "", "")),
            "heading"
        );
        assert_eq!(
            infer_symbol_kind(&rule("x", "entity.name.tag.yaml", true, "", "")),
            "key"
        );
    }

    #[test]
    fn prefers_a_more_specific_exclude() {
        let rules = compile_rules(&[func_rule(), hide_support()]);
        let support = lines(&["source.go", "entity.name.function.support.go"]);
        let def = lines(&["source.go", "entity.name.function.go"]);
        assert_eq!(best_symbol_rule(&def, &rules).unwrap().rule.id, "func");
        assert_eq!(
            best_symbol_rule(&support, &rules).unwrap().rule.id,
            "hide-support"
        );
        assert!(!best_symbol_rule(&support, &rules).unwrap().rule.show);
    }

    #[test]
    fn emits_functions_and_skips_support_calls() {
        let ls = lines(&["func Foo()", "    len(x)"]);
        let got = collect_symbols(
            &ls,
            |n| match n {
                1 => Some(vec![tok(5, 8, &["source.go", "entity.name.function.go"])]),
                2 => Some(vec![tok(
                    4,
                    7,
                    &["source.go", "entity.name.function.support.go"],
                )]),
                _ => None,
            },
            &[func_rule(), hide_support()],
            MAX_SYMBOLS,
        );
        assert_eq!(got, vec![sym("Foo", "func", 1)]);
    }

    #[test]
    fn prefixes_go_methods_with_the_receiver() {
        let line = "func (e *Engine) PendingAskUsers() bool {";
        let start = line.find("PendingAskUsers").unwrap();
        let ls = lines(&[line]);
        let got = collect_symbols(
            &ls,
            |_| {
                Some(vec![tok(
                    start,
                    start + 15,
                    &["source.go", "entity.name.function.go"],
                )])
            },
            &[func_rule()],
            MAX_SYMBOLS,
        );
        assert_eq!(got, vec![sym("Engine.PendingAskUsers", "func", 1)]);
    }

    #[test]
    fn joins_consecutive_tokens_and_transforms() {
        let ls = lines(&["id=\"main\""]);
        let scopes = [
            "text.html.basic",
            "meta.attribute.id.html",
            "string.quoted.double.html",
        ];
        let got = collect_symbols(
            &ls,
            |n| (n == 1).then(|| vec![tok(3, 4, &scopes), tok(4, 8, &scopes), tok(8, 9, &scopes)]),
            &[id_rule()],
            MAX_SYMBOLS,
        );
        assert_eq!(got, vec![sym("ID: main", "id", 1)]);
    }

    #[test]
    fn indents_markdown_headings() {
        let heading = rule(
            "md-h",
            "markup.heading.markdown",
            true,
            "s/^\\s{0,3}###\\s+/\\u2003\\u2003/\ns/^\\s{0,3}##\\s+/\\u2003/\ns/^\\s{0,3}#\\s+//",
            "heading",
        );
        let ls = lines(&["# What this is", "## Layout", "### Nested"]);
        let p = [
            "text.html.markdown",
            "markup.heading.markdown",
            "punctuation.definition.heading.markdown",
        ];
        let e = [
            "text.html.markdown",
            "markup.heading.markdown",
            "entity.name.section.markdown",
        ];
        let got = collect_symbols(
            &ls,
            |n| match n {
                1 => Some(vec![tok(0, 1, &p), tok(1, 14, &e)]),
                2 => Some(vec![tok(0, 2, &p), tok(2, 9, &e)]),
                3 => Some(vec![tok(0, 3, &p), tok(3, 10, &e)]),
                _ => None,
            },
            &[heading],
            MAX_SYMBOLS,
        );
        assert_eq!(
            got,
            vec![
                sym("What this is", "heading", 1),
                sym("\u{2003}Layout", "heading", 2),
                sym("\u{2003}\u{2003}Nested", "heading", 3)
            ]
        );
    }

    #[test]
    fn skips_symbols_inside_fenced_code() {
        let heading = rule(
            "md-h",
            "markup.heading.markdown",
            true,
            "s/^\\s{0,3}#\\s+//",
            "heading",
        );
        let ls = lines(&["# Intro", "func Foo()"]);
        let got = collect_symbols(
            &ls,
            |n| match n {
                1 => Some(vec![
                    tok(
                        0,
                        1,
                        &[
                            "text.html.markdown",
                            "markup.heading.markdown",
                            "punctuation.definition.heading.markdown",
                        ],
                    ),
                    tok(
                        1,
                        7,
                        &[
                            "text.html.markdown",
                            "markup.heading.markdown",
                            "entity.name.section.markdown",
                        ],
                    ),
                ]),
                2 => Some(vec![tok(
                    5,
                    8,
                    &[
                        "text.html.markdown",
                        "markup.fenced_code.block.markdown",
                        "meta.embedded.block.go",
                        "source.go",
                        "entity.name.function.go",
                    ],
                )]),
                _ => None,
            },
            &[func_rule(), heading],
            MAX_SYMBOLS,
        );
        assert_eq!(got, vec![sym("Intro", "heading", 1)]);
    }

    #[test]
    fn symbol_at_line_picks_the_last_at_or_before() {
        let syms = [
            sym("Foo", "func", 3),
            sym("Bar", "func", 20),
            sym("Baz", "func", 20),
        ];
        assert_eq!(symbol_at_line(&syms[..2], 1), None);
        assert_eq!(symbol_at_line(&syms[..2], 3).unwrap().name, "Foo");
        assert_eq!(symbol_at_line(&syms[..2], 10).unwrap().name, "Foo");
        assert_eq!(symbol_at_line(&syms[..2], 20).unwrap().name, "Bar");
        assert_eq!(symbol_at_line(&syms[..2], 99).unwrap().name, "Bar");
        assert_eq!(symbol_at_line(&syms, 20).unwrap().name, "Baz");
    }

    #[test]
    fn filter_keeps_document_order_for_empty_queries() {
        let em = '\u{2003}';
        let syms = [
            sym("AGENTS.md", "heading", 1),
            sym("What this is", "heading", 5),
            sym(&format!("{em}Layout"), "heading", 15),
            sym(&format!("{em}Dev environment tips"), "heading", 27),
            sym(&format!("{em}{em}Cursor SDK pin"), "heading", 53),
        ];
        let lines: Vec<usize> = filter_symbols(&syms, "", 40, |_, _, _| vec![])
            .iter()
            .map(|h| h.item.line)
            .collect();
        assert_eq!(lines, [1, 5, 15, 27, 53]);
        let two: Vec<usize> = filter_symbols(&syms, "", 2, |_, _, _| vec![])
            .iter()
            .map(|h| h.item.line)
            .collect();
        assert_eq!(two, [1, 5]);
        let twins = [
            sym(&format!("{em}{em}Foo"), "heading", 53),
            sym(&format!("{em}Foo"), "heading", 15),
        ];
        let got: Vec<usize> = filter_symbols(&twins, "foo", 40, |labels, _, _| {
            (0..labels.len()).map(|i| (i, 5)).collect()
        })
        .iter()
        .map(|h| h.item.line)
        .collect();
        assert_eq!(got, [15, 53]);
    }

    #[test]
    fn qualifies_go_receivers() {
        assert_eq!(
            qualify_go_receiver("func (e *Engine) Foo() {", "Foo"),
            "Engine.Foo"
        );
        assert_eq!(
            qualify_go_receiver("func (b askUserBroker) park() {", "park"),
            "askUserBroker.park"
        );
        assert_eq!(
            qualify_go_receiver("func (s *Set[T]) Add(v T) {", "Add"),
            "Set.Add"
        );
        assert_eq!(
            qualify_go_receiver("func (c *http.Client) Do() {", "Do"),
            "http.Client.Do"
        );
        assert_eq!(qualify_go_receiver("func Foo() {", "Foo"), "Foo");
        assert_eq!(qualify_go_receiver("def Foo(self):", "Foo"), "Foo");
    }

    #[test]
    fn lookup_names_strip_receivers_for_funcs_only() {
        assert_eq!(symbol_lookup_name("Engine.Foo", "func"), "Foo");
        assert_eq!(symbol_lookup_name("http.Client.Do", "func"), "Do");
        assert_eq!(symbol_lookup_name("foo.bar", "key"), "foo.bar");
        assert_eq!(symbol_lookup_name("Foo", "func"), "Foo");
    }
}
