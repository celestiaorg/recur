//! Indentation: CodeMirror's `IndentContext` / `getIndentation` /
//! `indentOnInput`, with Recur's `[[languageData]]` indent service
//! (`cm-lang-data.ts` `indentUsing`). Without a service CodeMirror falls back
//! to the (empty) syntax tree, which only answers at position 0.

use std::collections::HashMap;

use recur_textmate::{compute_indent, has_non_space};

use crate::change::Change;
use crate::state::{EditorState, Transaction, TransactionSpec};
use crate::text::count_column;

pub struct IndentContext<'a> {
    pub state: &'a EditorState,
    pub simulate_break: Option<usize>,
    pub simulate_double_break: bool,
    pub override_indentation: Option<&'a HashMap<usize, usize>>,
    pub unit: usize,
}

/// A (possibly simulated) line: text and start offset.
pub struct CxLine {
    pub text: String,
    pub from: usize,
}

impl<'a> IndentContext<'a> {
    pub fn new(state: &'a EditorState) -> IndentContext<'a> {
        IndentContext {
            state,
            simulate_break: None,
            simulate_double_break: false,
            override_indentation: None,
            unit: state.indent_unit_width(),
        }
    }

    pub fn line_at(&self, pos: usize, bias: i32) -> CxLine {
        let line = self.state.doc.line_at(pos);
        let text = self.state.doc.slice(line.from, line.to);
        if let Some(b) = self.simulate_break
            && b >= line.from
            && b <= line.to
        {
            if self.simulate_double_break && b == pos {
                return CxLine {
                    text: String::new(),
                    from: pos,
                };
            }
            let after = if bias < 0 { b < pos } else { b <= pos };
            if after {
                return CxLine {
                    text: text[b - line.from..].to_string(),
                    from: b,
                };
            }
            return CxLine {
                text: text[..b - line.from].to_string(),
                from: line.from,
            };
        }
        CxLine {
            text,
            from: line.from,
        }
    }

    pub fn text_after_pos(&self, pos: usize, bias: i32) -> String {
        if self.simulate_double_break && Some(pos) == self.simulate_break {
            return String::new();
        }
        let l = self.line_at(pos, bias);
        let start = pos - l.from;
        let end = l.text.len().min(start + 100);
        l.text.get(start..end).unwrap_or("").to_string()
    }

    fn first_non_space(text: &str) -> usize {
        text.find(|c: char| !c.is_whitespace())
            .unwrap_or(text.len())
    }

    pub fn column(&self, pos: usize, bias: i32) -> usize {
        let l = self.line_at(pos, bias);
        let mut result = count_column(&l.text, self.state.tab_size(), pos - l.from);
        if let Some(over) = self.override_indentation.and_then(|m| m.get(&l.from)) {
            let cur = count_column(
                &l.text,
                self.state.tab_size(),
                Self::first_non_space(&l.text),
            );
            result = (result + over).saturating_sub(cur);
        }
        result
    }

    pub fn line_indent(&self, pos: usize, bias: i32) -> usize {
        let l = self.line_at(pos, bias);
        if let Some(over) = self.override_indentation.and_then(|m| m.get(&l.from)) {
            return *over;
        }
        count_column(
            &l.text,
            self.state.tab_size(),
            Self::first_non_space(&l.text),
        )
    }
}

/// `previousNonEmptyText` from cm-lang-data.
fn previous_non_empty_text(cx: &IndentContext, line_from: usize) -> Option<String> {
    if line_from == 0 {
        return None;
    }
    let mut pos = line_from - 1;
    loop {
        let line = cx.line_at(pos, 1);
        if has_non_space(&line.text) {
            return Some(line.text);
        }
        if line.from == 0 {
            return None;
        }
        pos = line.from - 1;
    }
}

/// `getIndentation`.
pub fn get_indentation(cx: &IndentContext, pos: usize) -> Option<usize> {
    if let Some(patterns) = cx.state.language().and_then(|l| l.indent()) {
        let line = cx.line_at(pos, 1);
        let prev = previous_non_empty_text(cx, line.from);
        return Some(compute_indent(
            prev.as_deref(),
            &line.text,
            cx.unit,
            cx.state.tab_size(),
            patterns,
        ));
    }
    (pos == 0).then_some(0)
}

pub fn get_indentation_at(state: &EditorState, pos: usize) -> Option<usize> {
    get_indentation(&IndentContext::new(state), pos)
}

fn leading_whitespace(text: &str) -> &str {
    &text[..text.len() - text.trim_start_matches(char::is_whitespace).len()]
}

/// `indentOnInput`: re-indent lines whose text before the cursor matches
/// the language's `indentOnInput` rule (Recur: the decrease pattern).
pub fn indent_on_input(tr: &Transaction) -> Option<TransactionSpec> {
    if !tr.doc_changed() || (!tr.is_user_event("input.type") && !tr.is_user_event("input.complete"))
    {
        return None;
    }
    let rule = tr
        .start_state
        .language()
        .and_then(|l| l.indent())
        .and_then(|i| i.decrease.as_ref())?;
    let doc = tr.new_doc();
    let head = tr.new_selection().main().head();
    let line = doc.line_at(head);
    if head > line.from + 200 {
        return None;
    }
    if !rule.test(&doc.slice(line.from, head)) {
        return None;
    }
    let state = tr.state();
    let mut last = usize::MAX;
    let mut changes = Vec::new();
    for r in &state.selection.ranges {
        let line = state.doc.line_at(r.head());
        if line.from == last {
            continue;
        }
        last = line.from;
        let Some(indent) = get_indentation_at(state, line.from) else {
            continue;
        };
        let text = state.doc.slice(line.from, line.to);
        let cur = leading_whitespace(&text);
        let norm = state.indent_string(indent);
        if cur != norm {
            changes.push(Change::new(line.from, line.from + cur.len(), norm));
        }
    }
    (!changes.is_empty()).then(|| TransactionSpec::default().changes(changes).sequential())
}

/// `indentRange`: changes that auto-indent every line in `from..=to`.
pub fn indent_range(state: &EditorState, from: usize, to: usize) -> Vec<Change> {
    let mut updated: HashMap<usize, usize> = HashMap::new();
    let mut changes = Vec::new();
    let mut pos = from;
    while pos <= to {
        let line = state.doc.line_at(pos);
        pos = line.to + 1;
        let cx = IndentContext {
            override_indentation: Some(&updated),
            ..IndentContext::new(state)
        };
        let Some(mut indent) = get_indentation(&cx, line.from) else {
            continue;
        };
        let text = state.doc.slice(line.from, line.to);
        if !has_non_space(&text) {
            indent = 0;
        }
        let cur = leading_whitespace(&text);
        let norm = state.indent_string(indent);
        if cur != norm {
            updated.insert(line.from, indent);
            changes.push(Change::new(line.from, line.from + cur.len(), norm));
        }
        if pos > state.doc.len() {
            break;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use recur_textmate::{LanguageDataFields, language_data};

    use super::*;
    use crate::state::{LanguageConfig, StateConfig};

    fn go_state(doc: &str) -> EditorState {
        let fields = LanguageDataFields {
            line_comment: Some("//".into()),
            increase_indent_pattern: Some(
                "[{(\\[]\\s*(?://.*)?$|^\\s*(?:case|default)\\b.*$".into(),
            ),
            decrease_indent_pattern: Some("^\\s*(?:[})\\]]|(?:case|default)\\b)".into()),
            ..LanguageDataFields::default()
        };
        EditorState::create(
            doc,
            StateConfig {
                indent_unit: "\t".into(),
                indent_on_input: true,
                language: Some(Rc::new(LanguageConfig {
                    scope: "source.go".into(),
                    data: language_data(&fields),
                    fold_markers: None,
                })),
                ..StateConfig::default()
            },
        )
    }

    #[test]
    fn indentation_after_a_brace() {
        let s = go_state("func main() {\n");
        assert_eq!(get_indentation_at(&s, 14), Some(4));
        let plain = EditorState::create("x\n", StateConfig::default());
        assert_eq!(get_indentation_at(&plain, 2), None);
        assert_eq!(get_indentation_at(&plain, 0), Some(0));
    }

    #[test]
    fn simulated_break_splits_the_line() {
        let s = go_state("if x {}");
        let cx = IndentContext {
            simulate_break: Some(6),
            simulate_double_break: true,
            ..IndentContext::new(&s)
        };
        assert_eq!(cx.line_at(6, 1).text, "");
        assert_eq!(cx.line_at(3, 1).text, "if x {");
        assert_eq!(get_indentation(&cx, 6), Some(4));
    }

    #[test]
    fn typing_a_closing_brace_reindents() {
        let s = go_state("func main() {\n\t\tx\n\t\t");
        let s = s
            .update(TransactionSpec::default().cursor(s.doc.len()))
            .state()
            .clone();
        let tr = s.update(
            TransactionSpec::default()
                .changes(Change::insert(s.doc.len(), "}"))
                .cursor(s.doc.len() + 1)
                .user_event("input.type"),
        );
        assert_eq!(tr.state().doc.to_string(), "func main() {\n\t\tx\n\t}");
    }
}
