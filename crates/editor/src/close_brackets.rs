//! Bracket auto-closing: `@codemirror/autocomplete`'s `closeBrackets`
//! (input handler, closed-bracket tracking, `deleteBracketPair`) and
//! Recur's paired-quote input handler from `[[languageData]]`.
//!
//! Recur has no Lezer syntax tree, so CodeMirror's `nodeStart` and
//! `probablyInString` are always false and string prefixes never apply.

use recur_textmate::{BASE_BRACKETS, SELF_PAIR_QUOTES, inside_paired_quote};

use crate::change::{Change, ChangeDesc, MapMode};
use crate::selection::SelectionRange;
use crate::state::{
    CharCategory, EditorState, Effect, RangeChange, Transaction, TransactionSpec, is_js_space,
};

const DEFAULT_BRACKETS: &[&str] = &["(", "[", "{", "'", "\""];
const DEFAULT_BEFORE: &str = ")]}:;>";

/// Positions of closing brackets the extension inserted (`bracketState`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BracketState {
    pub closed: Vec<usize>,
}

/// `closeBracketEffect`: a closing bracket was inserted at this position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BracketEffect(pub usize);

impl BracketEffect {
    pub(crate) fn map(&self, changes: &ChangeDesc) -> Option<BracketEffect> {
        changes
            .map_pos_mode(self.0, -1, MapMode::TrackAfter)
            .map(BracketEffect)
    }
}

impl BracketState {
    pub(crate) fn update(&self, tr: &Transaction) -> BracketState {
        let changes = tr.changes.desc();
        let mut closed: Vec<usize> = if changes.is_empty() {
            self.closed.clone()
        } else {
            self.closed
                .iter()
                .filter_map(|&p| {
                    let from = changes.map_pos(p, 1);
                    let to = changes.map_pos(p + 1, -1);
                    (from < to).then_some(from)
                })
                .collect()
        };
        if let Some(sel) = &tr.selection {
            let line = tr.new_doc().line_at(sel.main().head());
            closed.retain(|&p| p >= line.from && p <= line.to);
        }
        for e in &tr.effects {
            if let Effect::CloseBrackets(BracketEffect(p)) = e {
                closed.push(*p);
            }
        }
        closed.sort_unstable();
        closed.dedup();
        BracketState { closed }
    }
}

struct Config {
    brackets: Vec<String>,
    before: String,
}

fn config(state: &EditorState) -> Config {
    match state.language().and_then(|l| l.close_brackets()) {
        Some(c) => Config {
            brackets: c.brackets.clone(),
            before: c.before.clone(),
        },
        None => Config {
            brackets: DEFAULT_BRACKETS.iter().map(|s| s.to_string()).collect(),
            before: DEFAULT_BEFORE.into(),
        },
    }
}

/// `closing`: the closing counterpart of an opening character.
pub fn closing(ch: char) -> char {
    const DEFINED: &[char] = &[
        '(', ')', '[', ']', '{', '}', '<', '>', '«', '»', '»', '«', '［', '］', '｛', '｝',
    ];
    for pair in DEFINED.chunks(2) {
        if pair[0] == ch {
            return pair[1];
        }
    }
    if (ch as u32) < 128 {
        ch
    } else {
        char::from_u32(ch as u32 + 1).unwrap_or(ch)
    }
}

fn next_char(state: &EditorState, pos: usize) -> String {
    state.doc.char_at(pos).map(String::from).unwrap_or_default()
}

fn prev_char(state: &EditorState, pos: usize) -> String {
    state
        .doc
        .char_before(pos)
        .map(String::from)
        .unwrap_or_default()
}

fn closed_bracket_at(state: &EditorState, pos: usize) -> bool {
    state.brackets.closed.contains(&pos)
}

fn first_char(s: &str) -> char {
    s.chars().next().unwrap_or('\0')
}

/// `insertBracket`: the transaction for typing `bracket`, when it opens a
/// pair or steps over a closed one.
pub fn insert_bracket(state: &EditorState, bracket: &str) -> Option<TransactionSpec> {
    let conf = config(state);
    for tok in &conf.brackets {
        let closed = closing(first_char(tok)).to_string();
        if bracket == tok {
            return if closed == *tok {
                handle_same(
                    state,
                    tok,
                    conf.brackets.iter().any(|b| *b == tok.repeat(3)),
                )
            } else {
                handle_open(state, tok, &closed, &conf.before)
            };
        }
        if bracket == closed && closed_bracket_at(state, state.selection.main().from) {
            return handle_close(state, &closed);
        }
    }
    None
}

fn finish(
    state: &EditorState,
    mut dont: bool,
    f: impl FnMut(SelectionRange, &mut bool) -> RangeChange,
) -> Option<TransactionSpec> {
    let mut f = f;
    let result = state.change_by_range(|r| f(r, &mut dont));
    (!dont).then(|| result.spec().scroll().user_event("input.type"))
}

fn handle_open(
    state: &EditorState,
    open: &str,
    close: &str,
    close_before: &str,
) -> Option<TransactionSpec> {
    finish(state, false, |range, dont| {
        if !range.is_empty() {
            return RangeChange {
                changes: vec![
                    Change::insert(range.from, open),
                    Change::insert(range.to, close),
                ],
                effects: vec![Effect::CloseBrackets(BracketEffect(range.to + open.len()))],
                range: SelectionRange::range(
                    range.anchor() + open.len(),
                    range.head() + open.len(),
                ),
            };
        }
        let next = next_char(state, range.head());
        if next.is_empty() || next.chars().all(is_js_space) || close_before.contains(next.as_str())
        {
            return RangeChange {
                changes: vec![Change::insert(range.head(), format!("{open}{close}"))],
                effects: vec![Effect::CloseBrackets(BracketEffect(
                    range.head() + open.len(),
                ))],
                range: SelectionRange::cursor(range.head() + open.len()),
            };
        }
        *dont = true;
        RangeChange::range(range)
    })
}

fn handle_close(state: &EditorState, close: &str) -> Option<TransactionSpec> {
    finish(state, false, |range, dont| {
        if range.is_empty() && next_char(state, range.head()) == close {
            return RangeChange::with(
                vec![Change::new(range.head(), range.head() + close.len(), close)],
                SelectionRange::cursor(range.head() + close.len()),
            );
        }
        *dont = true;
        RangeChange::range(range)
    })
}

/// `canStartStringAt` without string prefixes.
fn can_start_string_at(state: &EditorState, pos: usize) -> bool {
    state.categorize(&prev_char(state, pos)) != CharCategory::Word
}

fn handle_same(state: &EditorState, token: &str, allow_triple: bool) -> Option<TransactionSpec> {
    finish(state, false, |range, dont| {
        if !range.is_empty() {
            return RangeChange {
                changes: vec![
                    Change::insert(range.from, token),
                    Change::insert(range.to, token),
                ],
                effects: vec![Effect::CloseBrackets(BracketEffect(range.to + token.len()))],
                range: SelectionRange::range(
                    range.anchor() + token.len(),
                    range.head() + token.len(),
                ),
            };
        }
        let pos = range.head();
        let next = next_char(state, pos);
        if next == token {
            if closed_bracket_at(state, pos) {
                let triple = token.repeat(3);
                let is_triple = allow_triple
                    && state.slice_doc(pos, (pos + triple.len()).min(state.doc.len())) == triple;
                let content = if is_triple { triple } else { token.to_string() };
                return RangeChange::with(
                    vec![Change::new(pos, pos + content.len(), content.clone())],
                    SelectionRange::cursor(pos + content.len()),
                );
            }
        } else if state.categorize(&next) != CharCategory::Word && can_start_string_at(state, pos) {
            return RangeChange {
                changes: vec![Change::insert(pos, format!("{token}{token}"))],
                effects: vec![Effect::CloseBrackets(BracketEffect(pos + token.len()))],
                range: SelectionRange::cursor(pos + token.len()),
            };
        }
        *dont = true;
        RangeChange::range(range)
    })
}

/// The `closeBrackets` input handler: typed text replacing exactly the main
/// selection, one character, outside IME composition.
pub fn close_brackets_input(
    state: &EditorState,
    from: usize,
    to: usize,
    insert: &str,
    composing: bool,
) -> Option<TransactionSpec> {
    if composing || state.read_only() {
        return None;
    }
    let sel = state.selection.main();
    if insert.chars().count() != 1 || from != sel.from || to != sel.to {
        return None;
    }
    insert_bracket(state, insert)
}

/// Recur's high-precedence handler (only with bundle language data):
/// typing a self-pairing quote inside an open one on the line inserts it
/// plainly instead of pairing or stepping over.
pub fn skip_paired_quote_input(
    state: &EditorState,
    from: usize,
    to: usize,
    insert: &str,
) -> Option<TransactionSpec> {
    let conf = state.language()?.close_brackets()?;
    if insert.chars().count() != 1 || from != to {
        return None;
    }
    let sel = state.selection.main();
    if !sel.is_empty() || from != sel.from {
        return None;
    }
    let brackets: Vec<&str> = if conf.brackets.is_empty() {
        BASE_BRACKETS.to_vec()
    } else {
        conf.brackets.iter().map(String::as_str).collect()
    };
    if !SELF_PAIR_QUOTES.contains(&insert) || !brackets.contains(&insert) {
        return None;
    }
    let line = state.doc.line_at(from);
    let text = state.doc.slice(line.from, line.to);
    if !inside_paired_quote(&text, from - line.from, first_char(insert)) {
        return None;
    }
    Some(
        TransactionSpec::default()
            .changes(Change::new(from, to, insert))
            .cursor(from + insert.len())
            .user_event("input.type"),
    )
}

/// `deleteBracketPair` (Backspace between an empty pair).
pub fn delete_bracket_pair(state: &EditorState) -> Option<TransactionSpec> {
    if state.read_only() {
        return None;
    }
    let conf = config(state);
    let mut dont = false;
    let result = state.change_by_range(|range| {
        if range.is_empty() {
            let before = prev_char(state, range.head());
            for token in &conf.brackets {
                if *token == before
                    && next_char(state, range.head()) == closing(first_char(token)).to_string()
                {
                    return RangeChange::with(
                        vec![Change::delete(
                            range.head() - token.len(),
                            range.head() + token.len(),
                        )],
                        SelectionRange::cursor(range.head() - token.len()),
                    );
                }
            }
        }
        dont = true;
        RangeChange::range(range)
    });
    (!dont).then(|| result.spec().scroll().user_event("delete.backward"))
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use recur_textmate::{LanguageDataFields, language_data};

    use super::*;
    use crate::state::{LanguageConfig, StateConfig};

    fn st(doc: &str, pos: usize, lang: bool) -> EditorState {
        let language = lang.then(|| {
            Rc::new(LanguageConfig {
                scope: "source.js".into(),
                data: language_data(&LanguageDataFields {
                    line_comment: Some("//".into()),
                    ..Default::default()
                }),
                fold_markers: None,
            })
        });
        let s = EditorState::create(
            doc,
            StateConfig {
                language,
                ..StateConfig::default()
            },
        );
        s.update(TransactionSpec::default().cursor(pos))
            .state()
            .clone()
    }

    fn typed(s: &EditorState, ch: &str) -> EditorState {
        let sel = s.selection.main();
        let spec = skip_paired_quote_input(s, sel.from, sel.to, ch)
            .or_else(|| close_brackets_input(s, sel.from, sel.to, ch, false))
            .unwrap_or_else(|| {
                TransactionSpec::default()
                    .changes(Change::new(sel.from, sel.to, ch))
                    .cursor(sel.from + ch.len())
                    .user_event("input.type")
            });
        s.update(spec).state().clone()
    }

    #[test]
    fn opens_and_steps_over_pairs() {
        let s = typed(&st("", 0, false), "(");
        assert_eq!(s.doc.to_string(), "()");
        assert_eq!(s.selection.main().head(), 1);
        assert_eq!(s.brackets.closed, vec![1]);
        let s = typed(&s, ")");
        assert_eq!(s.doc.to_string(), "()");
        assert_eq!(s.selection.main().head(), 2);
        let s = typed(&st("x", 0, false), "(");
        assert_eq!(s.doc.to_string(), "(x");
    }

    #[test]
    fn quotes_pair_unless_after_a_word() {
        let s = typed(&st("", 0, false), "\"");
        assert_eq!(s.doc.to_string(), "\"\"");
        let s = typed(&s, "\"");
        assert_eq!(
            (s.doc.to_string(), s.selection.main().head()),
            ("\"\"".into(), 2)
        );
        let s = typed(&st("ab", 2, false), "'");
        assert_eq!(s.doc.to_string(), "ab'");
        let s = typed(&st("", 0, false), "`");
        assert_eq!(s.doc.to_string(), "`");
        let s = typed(&st("", 0, true), "`");
        assert_eq!(s.doc.to_string(), "``");
    }

    #[test]
    fn recur_skips_pairing_inside_an_open_quote() {
        let s = typed(&st("\"abc", 4, true), "\"");
        assert_eq!(s.doc.to_string(), "\"abc\"");
        let s = typed(&st("\"a\\\"b", 5, true), "\"");
        assert_eq!(s.doc.to_string(), "\"a\\\"b\"");
    }

    #[test]
    fn wraps_selections_and_deletes_pairs() {
        let s = st("word", 0, false);
        let s = s
            .update(
                TransactionSpec::default()
                    .selection(crate::selection::EditorSelection::single(0, 4)),
            )
            .state()
            .clone();
        let s = typed(&s, "[");
        assert_eq!(s.doc.to_string(), "[word]");
        assert_eq!((s.selection.main().from, s.selection.main().to), (1, 5));
        let s = st("()", 1, false);
        let s = s.update(delete_bracket_pair(&s).unwrap()).state().clone();
        assert_eq!(s.doc.to_string(), "");
        assert!(delete_bracket_pair(&st("(x", 1, false)).is_none());
    }

    #[test]
    fn closed_markers_drop_when_moving_lines() {
        let s = typed(&st("\nx", 0, false), "(");
        assert_eq!(s.brackets.closed, vec![1]);
        let s = s
            .update(TransactionSpec::default().cursor(4))
            .state()
            .clone();
        assert!(s.brackets.closed.is_empty());
        assert_eq!(closing('“'), '”');
    }
}
