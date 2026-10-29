//! Editor state and transactions, a port of CodeMirror's `EditorState` /
//! `Transaction` / `TransactionSpec`.
//!
//! CodeMirror composes behaviour from facets and state fields. Recur's
//! editor has a fixed set of those, so they are plain fields here, each
//! updated by the same rules as its CodeMirror counterpart.

use std::cell::OnceCell;
use std::rc::Rc;

use recur_textmate::{
    CloseBracketsConfig, CommentTokens, CompiledFoldMarkers, CompiledIndent, LanguageData,
};

use crate::change::{Change, ChangeDesc, ChangeSet};
use crate::clock::now_ms;
use crate::history::{FromHistory, HistoryState};
use crate::selection::{EditorSelection, SelectionRange};
use crate::text::{Line, Text, find_cluster_break};

/// Language support for the buffer's TextMate scope: bundle language data
/// and grammar fold markers.
#[derive(Default)]
pub struct LanguageConfig {
    pub scope: String,
    pub data: Option<LanguageData>,
    pub fold_markers: Option<CompiledFoldMarkers>,
}

impl LanguageConfig {
    pub fn comment_tokens(&self) -> Option<&CommentTokens> {
        self.data.as_ref().and_then(|d| d.comment_tokens.as_ref())
    }

    pub fn indent(&self) -> Option<&CompiledIndent> {
        self.data.as_ref().and_then(|d| d.indent.as_ref())
    }

    /// `closeBrackets` language data (absent without a `[[languageData]]`).
    pub fn close_brackets(&self) -> Option<&CloseBracketsConfig> {
        self.data.as_ref().map(|d| &d.close_brackets)
    }
}

/// Configuration CodeMirror spreads over facets.
#[derive(Clone)]
pub struct StateConfig {
    pub tab_size: usize,
    pub indent_unit: String,
    pub read_only: bool,
    pub allow_multiple_selections: bool,
    pub language: Option<Rc<LanguageConfig>>,
    pub history_min_depth: usize,
    pub history_new_group_delay: u64,
    pub indent_on_input: bool,
    /// Autocompletion (`None`: not enabled, as in the merge editor).
    pub completion: Option<Rc<crate::autocomplete::CompletionConfig>>,
}

impl std::fmt::Debug for StateConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateConfig")
            .field("tab_size", &self.tab_size)
            .field("indent_unit", &self.indent_unit)
            .field("read_only", &self.read_only)
            .field("language", &self.language.as_ref().map(|l| l.scope.clone()))
            .finish_non_exhaustive()
    }
}

impl Default for StateConfig {
    fn default() -> Self {
        StateConfig {
            tab_size: 4,
            indent_unit: "  ".into(),
            read_only: false,
            allow_multiple_selections: false,
            language: None,
            history_min_depth: 100,
            history_new_group_delay: 500,
            indent_on_input: false,
            completion: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isolate {
    Before,
    After,
    Full,
}

#[derive(Clone, Debug, Default)]
pub struct Annotations {
    pub user_event: Option<String>,
    pub add_to_history: Option<bool>,
    pub time: Option<u64>,
    pub isolate_history: Option<Isolate>,
    pub remote: bool,
    pub(crate) from_history: Option<Rc<FromHistory>>,
}

/// Where to scroll after a transaction (`EditorView.scrollIntoView`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollStrategy {
    Nearest,
    Start,
    End,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollTarget {
    pub range: SelectionRange,
    pub y: ScrollStrategy,
    pub x: ScrollStrategy,
    pub y_margin: f32,
    pub x_margin: f32,
    /// A scroll snapshot: restore this exact pixel offset relative to the
    /// line block at `range.head`.
    pub snapshot_top: Option<f32>,
}

impl ScrollTarget {
    pub fn pos(pos: usize, y: ScrollStrategy, y_margin: f32) -> ScrollTarget {
        ScrollTarget {
            range: SelectionRange::cursor(pos),
            y,
            x: ScrollStrategy::Nearest,
            y_margin,
            x_margin: 5.0,
            snapshot_top: None,
        }
    }

    /// `EditorView.scrollIntoView(range)` with default options.
    pub fn range(range: SelectionRange) -> ScrollTarget {
        ScrollTarget {
            range,
            y: ScrollStrategy::Nearest,
            x: ScrollStrategy::Nearest,
            y_margin: 5.0,
            x_margin: 5.0,
            snapshot_top: None,
        }
    }

    fn map(&self, changes: &ChangeDesc) -> ScrollTarget {
        ScrollTarget {
            range: self.range.map(changes, -1),
            ..*self
        }
    }
}

/// Typed state effects. Each maps through document changes the way its
/// CodeMirror `StateEffect` does.
#[derive(Clone, Debug)]
pub enum Effect {
    Fold {
        from: usize,
        to: usize,
    },
    Unfold {
        from: usize,
        to: usize,
    },
    ScrollIntoView(ScrollTarget),
    /// `EditorView.announce`: text for the screen reader live region.
    Announce(String),
    Reconfigure(Rc<StateConfig>),
    Search(crate::search::SearchEffect),
    Lint(crate::lint::LintEffect),
    Completion(crate::autocomplete::CompletionEffect),
    Snippet(crate::snippet::SnippetEffect),
    CloseBrackets(crate::close_brackets::BracketEffect),
    Gutter(crate::gutters::GutterEffect),
    Merge(crate::merge::MergeEffect),
}

fn map_range(from: usize, to: usize, changes: &ChangeDesc) -> Option<(usize, usize)> {
    let f = changes.map_pos(from, 1);
    let t = changes.map_pos(to, -1);
    (f < t).then_some((f, t))
}

impl Effect {
    /// `StateEffect.mapEffects`: `None` drops the effect.
    pub fn map(&self, changes: &ChangeDesc) -> Option<Effect> {
        if changes.is_empty() {
            return Some(self.clone());
        }
        match self {
            Effect::Fold { from, to } => {
                map_range(*from, *to, changes).map(|(from, to)| Effect::Fold { from, to })
            }
            Effect::Unfold { from, to } => {
                map_range(*from, *to, changes).map(|(from, to)| Effect::Unfold { from, to })
            }
            Effect::ScrollIntoView(t) => Some(Effect::ScrollIntoView(t.map(changes))),
            Effect::Completion(e) => e.map(changes).map(Effect::Completion),
            Effect::Snippet(e) => e.map(changes).map(Effect::Snippet),
            Effect::CloseBrackets(e) => e.map(changes).map(Effect::CloseBrackets),
            Effect::Merge(e) => Some(Effect::Merge(e.map(changes))),
            other => Some(other.clone()),
        }
    }
}

pub fn map_effects(effects: &[Effect], changes: &ChangeDesc) -> Vec<Effect> {
    effects.iter().filter_map(|e| e.map(changes)).collect()
}

/// Changes for a spec: requested edits, or a ready change set.
#[derive(Clone, Debug, Default)]
pub enum ChangesInput {
    #[default]
    None,
    List(Vec<Change>),
    Set(ChangeSet),
}

impl From<Vec<Change>> for ChangesInput {
    fn from(v: Vec<Change>) -> Self {
        ChangesInput::List(v)
    }
}

impl From<Change> for ChangesInput {
    fn from(c: Change) -> Self {
        ChangesInput::List(vec![c])
    }
}

impl From<ChangeSet> for ChangesInput {
    fn from(c: ChangeSet) -> Self {
        ChangesInput::Set(c)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TransactionSpec {
    pub changes: ChangesInput,
    pub selection: Option<EditorSelection>,
    pub effects: Vec<Effect>,
    pub annotations: Annotations,
    pub scroll_into_view: bool,
    /// `filter: false` skips transaction filters.
    pub filter: Option<bool>,
    /// Changes start in the document produced by the previous spec.
    pub sequential: bool,
}

impl TransactionSpec {
    pub fn changes(mut self, c: impl Into<ChangesInput>) -> Self {
        self.changes = c.into();
        self
    }

    pub fn selection(mut self, s: EditorSelection) -> Self {
        self.selection = Some(s);
        self
    }

    pub fn cursor(self, pos: usize) -> Self {
        self.selection(EditorSelection::cursor(pos))
    }

    pub fn effect(mut self, e: Effect) -> Self {
        self.effects.push(e);
        self
    }

    pub fn effects(mut self, e: impl IntoIterator<Item = Effect>) -> Self {
        self.effects.extend(e);
        self
    }

    pub fn user_event(mut self, e: &str) -> Self {
        self.annotations.user_event = Some(e.to_string());
        self
    }

    pub fn add_to_history(mut self, add: bool) -> Self {
        self.annotations.add_to_history = Some(add);
        self
    }

    pub fn scroll(mut self) -> Self {
        self.scroll_into_view = true;
        self
    }

    pub fn time(mut self, t: u64) -> Self {
        self.annotations.time = Some(t);
        self
    }

    pub fn isolate(mut self, i: Isolate) -> Self {
        self.annotations.isolate_history = Some(i);
        self
    }

    pub fn no_filter(mut self) -> Self {
        self.filter = Some(false);
        self
    }

    pub fn sequential(mut self) -> Self {
        self.sequential = true;
        self
    }
}

/// The result of `changeByRange`.
pub struct ChangeByRange {
    pub changes: ChangeSet,
    pub selection: EditorSelection,
    pub effects: Vec<Effect>,
}

impl ChangeByRange {
    pub fn spec(self) -> TransactionSpec {
        TransactionSpec {
            changes: ChangesInput::Set(self.changes),
            selection: Some(self.selection),
            effects: self.effects,
            ..TransactionSpec::default()
        }
    }
}

/// One range's contribution to `changeByRange`.
pub struct RangeChange {
    pub changes: Vec<Change>,
    pub range: SelectionRange,
    pub effects: Vec<Effect>,
}

impl RangeChange {
    pub fn range(range: SelectionRange) -> RangeChange {
        RangeChange {
            changes: Vec::new(),
            range,
            effects: Vec::new(),
        }
    }

    pub fn with(changes: Vec<Change>, range: SelectionRange) -> RangeChange {
        RangeChange {
            changes,
            range,
            effects: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharCategory {
    Word,
    Space,
    Other,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `makeCategorizer` for a grapheme cluster.
pub fn categorize(s: &str, word_chars: &str) -> CharCategory {
    if s.chars().all(is_js_space) {
        return CharCategory::Space;
    }
    if s.chars().any(is_word_char) {
        return CharCategory::Word;
    }
    if word_chars.chars().any(|w| s.contains(w)) {
        return CharCategory::Word;
    }
    CharCategory::Other
}

/// JavaScript's `\s`.
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

#[derive(Clone)]
pub struct EditorState {
    pub doc: Text,
    pub selection: EditorSelection,
    pub config: Rc<StateConfig>,
    pub history: Rc<HistoryState>,
    pub folds: crate::fold::FoldState,
    pub search: crate::search::SearchState,
    pub lint: crate::lint::LintState,
    pub completion: crate::autocomplete::CompletionState,
    pub snippet: Option<Rc<crate::snippet::ActiveSnippet>>,
    pub brackets: crate::close_brackets::BracketState,
    pub gutters: crate::gutters::GutterState,
    pub merge: Option<Rc<crate::merge::MergeState>>,
}

impl EditorState {
    pub fn create(doc: &str, config: StateConfig) -> EditorState {
        EditorState::create_with(Text::of(doc), EditorSelection::cursor(0), config)
    }

    pub fn create_with(doc: Text, selection: EditorSelection, config: StateConfig) -> EditorState {
        let selection = if config.allow_multiple_selections {
            selection
        } else {
            selection.as_single()
        };
        let doc_len = doc.len();
        let selection = clamp_selection(selection, doc_len);
        EditorState {
            doc,
            selection,
            config: Rc::new(config),
            history: Rc::new(HistoryState::default()),
            folds: crate::fold::FoldState::default(),
            search: crate::search::SearchState::default(),
            lint: crate::lint::LintState::default(),
            completion: crate::autocomplete::CompletionState::default(),
            snippet: None,
            brackets: crate::close_brackets::BracketState::default(),
            gutters: crate::gutters::GutterState::default(),
            merge: None,
        }
    }

    pub fn tab_size(&self) -> usize {
        self.config.tab_size
    }

    pub fn read_only(&self) -> bool {
        self.config.read_only
    }

    pub fn line_break(&self) -> &'static str {
        "\n"
    }

    pub fn indent_unit(&self) -> &str {
        &self.config.indent_unit
    }

    /// `getIndentUnit`: the unit's width in columns.
    pub fn indent_unit_width(&self) -> usize {
        recur_textmate::indent_unit_width(&self.config.indent_unit, self.config.tab_size)
    }

    pub fn indent_string(&self, cols: usize) -> String {
        recur_textmate::indent_string(cols, &self.config.indent_unit, self.config.tab_size)
    }

    pub fn language(&self) -> Option<&LanguageConfig> {
        self.config.language.as_deref()
    }

    pub fn slice_doc(&self, from: usize, to: usize) -> String {
        self.doc.slice(from, to)
    }

    pub fn line_at(&self, pos: usize) -> Line {
        self.doc.line_at(pos)
    }

    /// `ChangeSet.of` in this document.
    pub fn changes(&self, changes: &[Change]) -> ChangeSet {
        ChangeSet::of(changes, self.doc.len())
    }

    pub fn categorize(&self, s: &str) -> CharCategory {
        categorize(s, "")
    }

    /// The word around `pos` (`wordAt`).
    pub fn word_at(&self, pos: usize) -> Option<SelectionRange> {
        let line = self.doc.line_at(pos);
        let text = self.doc.slice(line.from, line.to);
        let (mut start, mut end) = (pos - line.from, pos - line.from);
        while start > 0 {
            let prev = find_cluster_break(&text, start, false);
            if self.categorize(&text[prev..start]) != CharCategory::Word {
                break;
            }
            start = prev;
        }
        while end < text.len() {
            let next = find_cluster_break(&text, end, true);
            if self.categorize(&text[end..next]) != CharCategory::Word {
                break;
            }
            end = next;
        }
        (start != end).then(|| SelectionRange::range(start + line.from, end + line.from))
    }

    /// Replace every range with `text` (`replaceSelection`).
    pub fn replace_selection(&self, text: &str) -> ChangeByRange {
        let text = crate::text::normalize_newlines(text).into_owned();
        self.change_by_range(|r| {
            RangeChange::with(
                vec![Change::new(r.from, r.to, text.clone())],
                SelectionRange::cursor(r.from + text.len()),
            )
        })
    }

    /// Run `f` per range and merge the results (`changeByRange`).
    pub fn change_by_range(
        &self,
        mut f: impl FnMut(SelectionRange) -> RangeChange,
    ) -> ChangeByRange {
        let sel = &self.selection;
        let first = f(sel.ranges[0]);
        let mut changes = self.changes(&first.changes);
        let mut ranges = vec![first.range];
        let mut effects = first.effects;
        for i in 1..sel.ranges.len() {
            let result = f(sel.ranges[i]);
            let new_changes = self.changes(&result.changes);
            let new_mapped = new_changes.map(&changes, false);
            for r in ranges.iter_mut().take(i) {
                *r = r.map(&new_mapped.desc(), -1);
            }
            let map_by = changes.map_desc(&new_changes.desc(), true);
            ranges.push(result.range.map(&map_by, -1));
            changes = changes.compose(&new_mapped);
            let mut mapped = map_effects(&effects, &new_mapped.desc());
            mapped.extend(map_effects(&result.effects, &map_by));
            effects = mapped;
        }
        ChangeByRange {
            changes,
            selection: EditorSelection::create(ranges, sel.main_index),
            effects,
        }
    }

    /// `state.update(...specs)`.
    pub fn update(&self, spec: TransactionSpec) -> Transaction {
        self.update_all(vec![spec])
    }

    pub fn update_all(&self, specs: Vec<TransactionSpec>) -> Transaction {
        resolve_transaction(self, specs, true)
    }

    /// Apply a transaction (`Transaction.state`).
    pub fn apply(&self, tr: &Transaction) -> EditorState {
        tr.state().clone()
    }

    fn apply_inner(&self, tr: &Transaction) -> EditorState {
        let mut config = self.config.clone();
        for e in &tr.effects {
            if let Effect::Reconfigure(c) = e {
                config = c.clone();
            }
        }
        let doc = tr.new_doc().clone();
        let selection = tr.new_selection();
        let selection = if self.config.allow_multiple_selections {
            selection
        } else {
            selection.as_single()
        };
        let mut next = EditorState {
            doc,
            selection,
            config,
            history: self.history.clone(),
            folds: self.folds.clone(),
            search: self.search.clone(),
            lint: self.lint.clone(),
            completion: self.completion.clone(),
            snippet: self.snippet.clone(),
            brackets: self.brackets.clone(),
            gutters: self.gutters.clone(),
            merge: self.merge.clone(),
        };
        next.history = Rc::new(self.history.update(tr, &next.config));
        next.folds = self.folds.update(tr);
        next.search = self.search.update(tr);
        next.lint = self.lint.update(tr);
        next.brackets = self.brackets.update(tr);
        next.snippet = crate::snippet::update_snippet_state(self.snippet.as_ref(), tr, &next);
        next.gutters = self.gutters.update(tr, &next.doc);
        next.merge = crate::merge::update_merge_state(self.merge.as_ref(), tr, &next.doc);
        next.completion = self.completion.update(tr, &next);
        next
    }

    /// `{doc, selection}` plus the serialized history (`toJSON` with
    /// `historyField`).
    pub fn to_json_with_history(&self) -> serde_json::Value {
        serde_json::json!({
            "doc": self.doc.to_string(),
            "selection": self.selection.to_json(),
            "history": self.history.to_json(),
        })
    }

    /// `EditorState.fromJSON` restoring history.
    pub fn from_json_with_history(
        json: &serde_json::Value,
        config: StateConfig,
    ) -> Option<EditorState> {
        let doc = json.get("doc")?.as_str()?;
        let selection = EditorSelection::from_json(json.get("selection")?)?;
        let mut state = EditorState::create_with(Text::of(doc), selection, config);
        if let Some(h) = json.get("history") {
            state.history = Rc::new(HistoryState::from_json(h)?);
        }
        Some(state)
    }
}

fn clamp_selection(sel: EditorSelection, len: usize) -> EditorSelection {
    if sel.ranges.iter().all(|r| r.to <= len) {
        return sel;
    }
    EditorSelection::create(
        sel.ranges
            .iter()
            .map(|r| SelectionRange::range(r.anchor().min(len), r.head().min(len)))
            .collect(),
        sel.main_index,
    )
}

/// Something that holds a state and applies transactions to it: the
/// editor view, or a bare state in tests. Commands that dispatch more than
/// once (snippets, completion `apply`) take this.
pub trait Dispatch {
    fn state(&self) -> &EditorState;
    fn dispatch(&mut self, spec: TransactionSpec);
}

impl Dispatch for EditorState {
    fn state(&self) -> &EditorState {
        self
    }

    fn dispatch(&mut self, spec: TransactionSpec) {
        *self = self.update(spec).state().clone();
    }
}

pub struct Transaction {
    pub start_state: EditorState,
    pub changes: ChangeSet,
    pub selection: Option<EditorSelection>,
    pub effects: Vec<Effect>,
    pub annotations: Annotations,
    pub scroll_into_view: bool,
    doc: OnceCell<Text>,
    state: OnceCell<EditorState>,
}

impl Transaction {
    fn create(
        start_state: &EditorState,
        changes: ChangeSet,
        selection: Option<EditorSelection>,
        effects: Vec<Effect>,
        mut annotations: Annotations,
        scroll_into_view: bool,
    ) -> Transaction {
        if annotations.time.is_none() {
            annotations.time = Some(now_ms());
        }
        let selection = selection.map(|s| clamp_selection(s, changes.new_len()));
        Transaction {
            start_state: start_state.clone(),
            changes,
            selection,
            effects,
            annotations,
            scroll_into_view,
            doc: OnceCell::new(),
            state: OnceCell::new(),
        }
    }

    pub fn new_doc(&self) -> &Text {
        self.doc
            .get_or_init(|| self.changes.apply(&self.start_state.doc))
    }

    pub fn new_selection(&self) -> EditorSelection {
        match &self.selection {
            Some(s) => s.clone(),
            None => self.start_state.selection.map(&self.changes.desc(), -1),
        }
    }

    pub fn state(&self) -> &EditorState {
        self.state
            .get_or_init(|| self.start_state.apply_inner(self))
    }

    pub fn doc_changed(&self) -> bool {
        !self.changes.is_empty()
    }

    pub fn user_event(&self) -> Option<&str> {
        self.annotations.user_event.as_deref()
    }

    /// `isUserEvent`: equal to or more specific than `event`.
    pub fn is_user_event(&self, event: &str) -> bool {
        match self.user_event() {
            Some(e) => {
                e == event
                    || (e.len() > event.len()
                        && e.starts_with(event)
                        && e.as_bytes()[event.len()] == b'.')
            }
            None => false,
        }
    }

    pub fn reconfigured(&self) -> bool {
        self.effects
            .iter()
            .any(|e| matches!(e, Effect::Reconfigure(_)))
    }

    pub fn time(&self) -> u64 {
        self.annotations.time.unwrap_or(0)
    }
}

struct Resolved {
    changes: ChangeSet,
    selection: Option<EditorSelection>,
    effects: Vec<Effect>,
    annotations: Annotations,
    scroll_into_view: bool,
}

fn resolve_inner(state: &EditorState, spec: TransactionSpec, doc_size: usize) -> Resolved {
    let changes = match spec.changes {
        ChangesInput::None => ChangeSet::empty(doc_size),
        ChangesInput::List(l) => ChangeSet::of(&l, doc_size),
        ChangesInput::Set(s) => s,
    };
    let _ = state;
    Resolved {
        changes,
        selection: spec.selection,
        effects: spec.effects,
        annotations: spec.annotations,
        scroll_into_view: spec.scroll_into_view,
    }
}

fn merge_annotations(a: Annotations, b: Annotations) -> Annotations {
    Annotations {
        user_event: a.user_event.or(b.user_event),
        add_to_history: a.add_to_history.or(b.add_to_history),
        time: a.time.or(b.time),
        isolate_history: a.isolate_history.or(b.isolate_history),
        remote: a.remote || b.remote,
        from_history: a.from_history.or(b.from_history),
    }
}

fn merge_transaction(a: Resolved, b: Resolved, sequential: bool) -> Resolved {
    let (map_for_a, map_for_b, changes) = if sequential {
        let len = b.changes.len();
        let c = a.changes.compose(&b.changes);
        (b.changes.desc(), ChangeSet::empty(len).desc(), c)
    } else {
        let mapped = b.changes.map(&a.changes, false);
        let for_b = a.changes.map_desc(&b.changes.desc(), true);
        let c = a.changes.compose(&mapped);
        (mapped.desc(), for_b, c)
    };
    let selection = match b.selection {
        Some(s) => Some(s.map(&map_for_b, -1)),
        None => a.selection.map(|s| s.map(&map_for_a, -1)),
    };
    let mut effects = map_effects(&a.effects, &map_for_a);
    effects.extend(map_effects(&b.effects, &map_for_b));
    Resolved {
        changes,
        selection,
        effects,
        annotations: merge_annotations(a.annotations, b.annotations),
        scroll_into_view: a.scroll_into_view || b.scroll_into_view,
    }
}

fn resolve_transaction(
    state: &EditorState,
    specs: Vec<TransactionSpec>,
    mut filter: bool,
) -> Transaction {
    let mut specs = specs.into_iter();
    let first = specs.next().unwrap_or_default();
    if first.filter == Some(false) {
        filter = false;
    }
    let mut s = resolve_inner(state, first, state.doc.len());
    for spec in specs {
        if spec.filter == Some(false) {
            filter = false;
        }
        let seq = spec.sequential;
        let size = if seq {
            s.changes.new_len()
        } else {
            state.doc.len()
        };
        let next = resolve_inner(state, spec, size);
        s = merge_transaction(s, next, seq);
    }
    let tr = Transaction::create(
        state,
        s.changes,
        s.selection,
        s.effects,
        s.annotations,
        s.scroll_into_view,
    );
    if filter { filter_transaction(tr) } else { tr }
}

/// Transaction filters in effect: `indentOnInput`.
fn filter_transaction(tr: Transaction) -> Transaction {
    if tr.start_state.config.indent_on_input
        && let Some(extra) = crate::indent::indent_on_input(&tr)
    {
        let state = tr.start_state.clone();
        let base = TransactionSpec {
            changes: ChangesInput::Set(tr.changes.clone()),
            selection: tr.selection.clone(),
            effects: tr.effects.clone(),
            annotations: tr.annotations.clone(),
            scroll_into_view: tr.scroll_into_view,
            ..TransactionSpec::default()
        };
        return resolve_transaction(&state, vec![base, extra], false);
    }
    tr
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(doc: &str) -> EditorState {
        EditorState::create(
            doc,
            StateConfig {
                allow_multiple_selections: true,
                ..StateConfig::default()
            },
        )
    }

    #[test]
    fn applies_changes_and_maps_selection() {
        let s = st("hello").update(TransactionSpec::default().cursor(5));
        let s = s.state().clone();
        let tr = s.update(TransactionSpec::default().changes(Change::insert(0, ">> ")));
        assert_eq!(tr.state().doc.to_string(), ">> hello");
        assert_eq!(tr.state().selection.main().head(), 8);
    }

    #[test]
    fn change_by_range_merges_multiple_cursors() {
        let s = st("ab\ncd");
        let s = s
            .update(
                TransactionSpec::default().selection(EditorSelection::create(
                    vec![SelectionRange::cursor(1), SelectionRange::cursor(4)],
                    0,
                )),
            )
            .state()
            .clone();
        let spec = s.replace_selection("XY").spec();
        let next = s.update(spec).state().clone();
        assert_eq!(next.doc.to_string(), "aXYb\ncXYd");
        let heads: Vec<usize> = next.selection.ranges.iter().map(|r| r.head()).collect();
        assert_eq!(heads, vec![3, 8]);
    }

    #[test]
    fn word_at_and_user_events() {
        let s = st("foo bar_baz.qux");
        let w = s.word_at(6).unwrap();
        assert_eq!((w.from, w.to), (4, 11));
        assert!(s.word_at(3).is_some());
        let tr = s.update(TransactionSpec::default().user_event("input.type.compose"));
        assert!(tr.is_user_event("input"));
        assert!(tr.is_user_event("input.type"));
        assert!(!tr.is_user_event("inp"));
        assert!(!tr.is_user_event("delete"));
    }

    #[test]
    fn sequential_specs_compose() {
        let s = st("abc");
        let tr = s.update_all(vec![
            TransactionSpec::default().changes(Change::insert(0, "x")),
            TransactionSpec::default()
                .changes(Change::insert(4, "y"))
                .sequential(),
        ]);
        assert_eq!(tr.state().doc.to_string(), "xabcy");
        let tr = s.update_all(vec![
            TransactionSpec::default().changes(Change::insert(0, "x")),
            TransactionSpec::default().changes(Change::insert(3, "y")),
        ]);
        assert_eq!(tr.state().doc.to_string(), "xabcy");
    }
}
