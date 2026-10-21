//! Autocompletion: `@codemirror/autocomplete`'s completion state, fuzzy
//! matcher, option sorting, commands and the query driver (the
//! `completionPlugin` view plugin), with the two sources Recur's editor
//! configures (`override`): bundle snippets and `completeAnyWord`.
//!
//! Recur's `override` list also shadows `@codemirror/lsp-client`'s
//! language-data completion source, so the editor shows no server
//! completions; [`crate::lsp::LspCompletionSource`] exists for hosts that
//! add it to [`CompletionConfig::sources`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use recur_textmate::JsRegex;

use crate::change::{Change, ChangeDesc, Touch};
use crate::clock::now_ms;
use crate::selection::SelectionRange;
use crate::snippet::{RegisteredSnippet, SnippetProvider, apply_snippet, snippets_matching_prefix};
use crate::state::{Dispatch, EditorState, Effect, RangeChange, Transaction, TransactionSpec};
use crate::text::{locale_compare, normalize_newlines};

/// Documentation shown beside the selected option (`info`), or in LSP
/// hover/signature tooltips: markdown, or plain text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub markdown: bool,
    pub value: String,
}

pub type ApplyFn = Rc<dyn Fn(&mut dyn Dispatch, &Completion, usize, usize)>;

#[derive(Clone)]
pub enum CompletionApply {
    Text(String),
    Fn(ApplyFn),
}

impl PartialEq for CompletionApply {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CompletionApply::Text(a), CompletionApply::Text(b)) => a == b,
            (CompletionApply::Fn(a), CompletionApply::Fn(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// A completion option (`Completion`).
#[derive(Clone, Default)]
pub struct Completion {
    pub label: String,
    pub display_label: Option<String>,
    pub detail: Option<String>,
    pub info: Option<Doc>,
    pub apply: Option<CompletionApply>,
    /// `type`: drives the icon (`cm-completionIcon-<type>`).
    pub kind: Option<String>,
    pub commit_characters: Option<Vec<String>>,
    pub boost: Option<i32>,
    pub sort_text: Option<String>,
}

impl std::fmt::Debug for Completion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Completion")
            .field("label", &self.label)
            .field("detail", &self.detail)
            .field("kind", &self.kind)
            .finish()
    }
}

/// `validFor`: when the typed text still matches, the result is reused
/// without querying the source again.
#[derive(Clone)]
pub struct ValidFor(Rc<JsRegex>);

impl ValidFor {
    /// A JS regexp source; anchored at both ends like `ensureAnchor`.
    pub fn regex(source: &str, flags: &str) -> Option<ValidFor> {
        let start = !source.starts_with('^');
        let end = !source.ends_with('$');
        let anchored = format!(
            "{}(?:{source}){}",
            if start { "^" } else { "" },
            if end { "$" } else { "" }
        );
        let src = if start || end {
            anchored
        } else {
            source.to_string()
        };
        JsRegex::new(&src, flags).map(|r| ValidFor(Rc::new(r)))
    }

    fn test(&self, text: &str) -> bool {
        self.0.test(text)
    }
}

pub type ResultMapFn = Rc<dyn Fn(&CompletionResult, &ChangeDesc) -> Option<CompletionResult>>;

/// What a source returns (`CompletionResult`).
#[derive(Clone)]
pub struct CompletionResult {
    pub from: usize,
    pub to: Option<usize>,
    pub options: Rc<[Rc<Completion>]>,
    pub valid_for: Option<ValidFor>,
    pub commit_characters: Option<Vec<String>>,
    pub map: Option<ResultMapFn>,
}

impl CompletionResult {
    pub fn new(from: usize, options: Vec<Completion>) -> CompletionResult {
        CompletionResult {
            from,
            to: None,
            options: options.into_iter().map(Rc::new).collect(),
            valid_for: None,
            commit_characters: None,
            map: None,
        }
    }
}

type AbortListener = Box<dyn FnOnce()>;

/// Abort signal for a running query (`CompletionContext.addEventListener`).
#[derive(Default)]
pub struct AbortSignal {
    listeners: RefCell<Option<Vec<AbortListener>>>,
    aborted: Cell<bool>,
    on_doc_change: Cell<bool>,
}

impl AbortSignal {
    pub fn aborted(&self) -> bool {
        self.aborted.get()
    }

    /// Run `f` when the query is aborted. `on_doc_change` aborts it on any
    /// document change, not only resets.
    pub fn add_listener(&self, f: impl FnOnce() + 'static, on_doc_change: bool) {
        if self.aborted.get() {
            return;
        }
        self.listeners
            .borrow_mut()
            .get_or_insert_with(Vec::new)
            .push(Box::new(f));
        if on_doc_change {
            self.on_doc_change.set(true);
        }
    }

    fn abort(&self) {
        self.aborted.set(true);
        for f in self.listeners.borrow_mut().take().unwrap_or_default() {
            f();
        }
    }
}

/// `CompletionContext`.
#[derive(Clone)]
pub struct CompletionContext {
    pub state: EditorState,
    pub pos: usize,
    pub explicit: bool,
    pub abort: Rc<AbortSignal>,
}

impl CompletionContext {
    pub fn new(state: EditorState, pos: usize, explicit: bool) -> CompletionContext {
        CompletionContext {
            state,
            pos,
            explicit,
            abort: Rc::new(AbortSignal::default()),
        }
    }

    /// `matchBefore`: the match of `source` (a JS regexp) ending at the
    /// cursor, looking back at most 250 characters on the line.
    pub fn match_before(&self, source: &str, flags: &str) -> Option<(usize, usize, String)> {
        let line = self.state.doc.line_at(self.pos);
        let start = self
            .state
            .doc
            .clamp_to_char(line.from.max(self.pos.saturating_sub(250)));
        let text = self.state.slice_doc(start, self.pos);
        let src = if source.ends_with('$') {
            source.to_string()
        } else {
            format!("(?:{source})$")
        };
        let re = JsRegex::new(&src, flags)?;
        let g = re.exec_at(&text, 0)?;
        let (s, _) = g[0]?;
        Some((start + s, self.pos, text[s..].to_string()))
    }
}

pub type QueryFuture = LocalBoxFuture<'static, Result<Option<CompletionResult>, String>>;

/// A source's answer: right away, or later.
pub enum SourceResult {
    Ready(Option<CompletionResult>),
    Pending(QueryFuture),
}

pub trait CompletionSource {
    fn complete(&self, cx: &CompletionContext) -> SourceResult;
}

/// A configured source, compared by identity.
#[derive(Clone)]
pub struct SourceRef(pub Rc<dyn CompletionSource>);

impl PartialEq for SourceRef {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::addr_eq(Rc::as_ptr(&self.0), Rc::as_ptr(&other.0))
    }
}

/// `autocompletion({ override, defaultKeymap: false })` as Recur configures
/// it; every other option keeps CodeMirror's default.
#[derive(Clone)]
pub struct CompletionConfig {
    pub sources: Vec<SourceRef>,
}

pub const ACTIVATE_ON_TYPING_DELAY: u64 = 100;
pub const INTERACTION_DELAY: u64 = 75;
pub const UPDATE_SYNC_TIME: u64 = 100;
pub const MAX_RENDERED_OPTIONS: usize = 100;
const MAX_UPDATE_COUNT: usize = 50;
const MIN_ABORT_TIME: u64 = 1000;

#[derive(Clone)]
pub struct ActiveResult {
    pub limit: usize,
    pub result: Rc<CompletionResult>,
    pub from: usize,
    pub to: usize,
}

#[derive(Clone)]
pub enum SourceState {
    Inactive,
    Pending,
    Result(Rc<ActiveResult>),
}

/// A source's state (`ActiveSource` / `ActiveResult`).
#[derive(Clone)]
pub struct ActiveSource {
    pub source: SourceRef,
    pub state: SourceState,
    pub explicit: bool,
}

const TYPING: u8 = 1;
const BACKSPACING: u8 = 2;
const SIMPLE_INTERACTION: u8 = 3;
const ACTIVATE: u8 = 4;
const RESET: u8 = 8;
const RESET_IF_TOUCHING: u8 = 16;

fn cur(state: &EditorState) -> usize {
    state.selection.main().from
}

fn update_type(tr: &Transaction) -> u8 {
    let typing = tr.is_user_event("input.type");
    if typing {
        ACTIVATE | TYPING
    } else if tr.is_user_event("delete.backward") {
        BACKSPACING
    } else if tr.selection.is_some() {
        RESET
    } else if tr.doc_changed() {
        RESET_IF_TOUCHING
    } else {
        0
    }
}

fn check_valid(valid_for: Option<&ValidFor>, state: &EditorState, from: usize, to: usize) -> bool {
    valid_for.is_some_and(|v| v.test(&state.slice_doc(from, to)))
}

impl ActiveSource {
    fn new(source: SourceRef, state: SourceState, explicit: bool) -> ActiveSource {
        ActiveSource {
            source,
            state,
            explicit,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.state, SourceState::Pending)
    }

    pub fn is_inactive(&self) -> bool {
        matches!(self.state, SourceState::Inactive)
    }

    pub fn result(&self) -> Option<&Rc<ActiveResult>> {
        match &self.state {
            SourceState::Result(r) => Some(r),
            _ => None,
        }
    }

    fn same(&self, other: &ActiveSource) -> bool {
        self.source == other.source
            && self.explicit == other.explicit
            && match (&self.state, &other.state) {
                (SourceState::Inactive, SourceState::Inactive)
                | (SourceState::Pending, SourceState::Pending) => true,
                (SourceState::Result(a), SourceState::Result(b)) => {
                    Rc::ptr_eq(&a.result, &b.result)
                        && a.from == b.from
                        && a.to == b.to
                        && a.limit == b.limit
                }
                _ => false,
            }
    }

    fn touches(&self, tr: &Transaction, state: &EditorState) -> bool {
        match &self.state {
            SourceState::Result(r) => tr.changes.touches_range(r.from, r.to) != Touch::No,
            _ => {
                let p = cur(state);
                tr.changes.touches_range(p, p) != Touch::No
            }
        }
    }

    /// `state` is the transaction's new state (its fields may still be
    /// under construction).
    fn update(&self, tr: &Transaction, state: &EditorState) -> ActiveSource {
        let ty = update_type(tr);
        let mut value = self.clone();
        if ty & RESET != 0 || (ty & RESET_IF_TOUCHING != 0 && self.touches(tr, state)) {
            value = ActiveSource::new(value.source, SourceState::Inactive, false);
        }
        if ty & ACTIVATE != 0 && value.is_inactive() {
            value = ActiveSource::new(self.source.clone(), SourceState::Pending, false);
        }
        value = value.update_for(tr, state, ty);
        for e in &tr.effects {
            match e {
                Effect::Completion(CompletionEffect::Start(explicit)) => {
                    value = ActiveSource::new(value.source, SourceState::Pending, *explicit);
                }
                Effect::Completion(CompletionEffect::Close) => {
                    value = ActiveSource::new(value.source, SourceState::Inactive, false);
                }
                Effect::Completion(CompletionEffect::SetActive(list)) => {
                    for a in list.iter() {
                        if a.source == value.source {
                            value = a.clone();
                        }
                    }
                }
                _ => {}
            }
        }
        value
    }

    fn update_for(&self, tr: &Transaction, state: &EditorState, ty: u8) -> ActiveSource {
        let SourceState::Result(r) = &self.state else {
            return self.clone();
        };
        if ty & SIMPLE_INTERACTION == 0 {
            return self.map(&tr.changes.desc());
        }
        let changes = tr.changes.desc();
        let mut result = Some(r.result.clone());
        if let Some(map) = &r.result.map
            && !changes.is_empty()
        {
            result = map(&r.result, &changes).map(Rc::new);
        }
        let from = changes.map_pos(r.from, -1);
        let to = changes.map_pos(r.to, 1);
        let pos = cur(state);
        let Some(result) = result else {
            return self.restart(ty);
        };
        if pos > to || (ty & BACKSPACING != 0 && (cur(&tr.start_state) == r.from || pos < r.limit))
        {
            return self.restart(ty);
        }
        let limit = changes.map_pos(r.limit, -1);
        if check_valid(result.valid_for.as_ref(), state, from, to) {
            return ActiveSource::new(
                self.source.clone(),
                SourceState::Result(Rc::new(ActiveResult {
                    limit,
                    result,
                    from,
                    to,
                })),
                self.explicit,
            );
        }
        ActiveSource::new(self.source.clone(), SourceState::Pending, self.explicit)
    }

    fn restart(&self, ty: u8) -> ActiveSource {
        let state = if ty & ACTIVATE != 0 {
            SourceState::Pending
        } else {
            SourceState::Inactive
        };
        ActiveSource::new(self.source.clone(), state, false)
    }

    fn map(&self, changes: &ChangeDesc) -> ActiveSource {
        let SourceState::Result(r) = &self.state else {
            return self.clone();
        };
        if changes.is_empty() {
            return self.clone();
        }
        let result = match &r.result.map {
            Some(map) => map(&r.result, changes).map(Rc::new),
            None => Some(r.result.clone()),
        };
        let Some(result) = result else {
            return ActiveSource::new(self.source.clone(), SourceState::Inactive, false);
        };
        ActiveSource::new(
            self.source.clone(),
            SourceState::Result(Rc::new(ActiveResult {
                limit: changes.map_pos(r.limit, -1),
                result,
                from: changes.map_pos(r.from, -1),
                to: changes.map_pos(r.to, 1),
            })),
            self.explicit,
        )
    }
}

#[derive(Clone)]
pub enum CompletionEffect {
    Start(bool),
    Close,
    SetSelected(usize),
    SetActive(Rc<[ActiveSource]>),
}

impl std::fmt::Debug for CompletionEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompletionEffect::Start(e) => write!(f, "Start({e})"),
            CompletionEffect::Close => write!(f, "Close"),
            CompletionEffect::SetSelected(i) => write!(f, "SetSelected({i})"),
            CompletionEffect::SetActive(l) => write!(f, "SetActive({})", l.len()),
        }
    }
}

impl CompletionEffect {
    pub(crate) fn map(&self, changes: &ChangeDesc) -> Option<CompletionEffect> {
        Some(match self {
            CompletionEffect::SetActive(list) => {
                CompletionEffect::SetActive(list.iter().map(|s| s.map(changes)).collect())
            }
            other => other.clone(),
        })
    }
}

/// A filtered, scored option (`Option`): `matched` holds byte ranges of
/// the rendered label to highlight (`cm-completionMatchedText`).
#[derive(Clone)]
pub struct CompletionOption {
    pub completion: Rc<Completion>,
    pub source: SourceRef,
    pub matched: Vec<(usize, usize)>,
    pub score: i64,
}

/// The open completion list (`CompletionDialog`).
#[derive(Clone)]
pub struct CompletionDialog {
    pub options: Rc<[CompletionOption]>,
    pub selected: Option<usize>,
    pub disabled: bool,
    pub timestamp: u64,
    /// Where the tooltip anchors (below the line, `above: false`).
    pub pos: usize,
}

impl CompletionDialog {
    fn set_selected(self: &Rc<Self>, selected: usize) -> Rc<CompletionDialog> {
        if Some(selected) == self.selected || selected >= self.options.len() {
            return self.clone();
        }
        Rc::new(CompletionDialog {
            selected: Some(selected),
            ..(**self).clone()
        })
    }

    fn set_disabled(self: &Rc<Self>) -> Rc<CompletionDialog> {
        Rc::new(CompletionDialog {
            disabled: true,
            ..(**self).clone()
        })
    }

    fn build(
        active: &[ActiveSource],
        state: &EditorState,
        prev: Option<&Rc<CompletionDialog>>,
        did_set_active: bool,
    ) -> Option<Rc<CompletionDialog>> {
        let any_pending = active.iter().any(ActiveSource::is_pending);
        if let Some(p) = prev
            && !did_set_active
            && any_pending
        {
            return Some(p.set_disabled());
        }
        let options = sort_options(active, state);
        if options.is_empty() {
            return match prev {
                Some(p) if any_pending => Some(p.set_disabled()),
                _ => None,
            };
        }
        let mut selected = Some(0);
        if let Some(p) = prev
            && p.selected != selected
            && let Some(ps) = p.selected
        {
            let value = &p.options[ps].completion;
            if let Some(i) = options
                .iter()
                .position(|o| Rc::ptr_eq(&o.completion, value))
            {
                selected = Some(i);
            }
        }
        let pos = active
            .iter()
            .filter_map(|a| a.result().map(|r| r.from))
            .min()
            .unwrap_or(100_000_000);
        Some(Rc::new(CompletionDialog {
            options: options.into(),
            selected,
            disabled: false,
            timestamp: prev.map_or_else(now_ms, |p| p.timestamp),
            pos,
        }))
    }

    fn map(self: &Rc<Self>, changes: &ChangeDesc) -> Rc<CompletionDialog> {
        Rc::new(CompletionDialog {
            pos: changes.map_pos(self.pos, -1),
            ..(**self).clone()
        })
    }
}

/// The completion state field.
#[derive(Clone, Default)]
pub struct CompletionState {
    pub active: Rc<[ActiveSource]>,
    pub open: Option<Rc<CompletionDialog>>,
}

fn same_results(a: &[ActiveSource], b: &[ActiveSource]) -> bool {
    let ra: Vec<_> = a.iter().filter_map(ActiveSource::result).collect();
    let rb: Vec<_> = b.iter().filter_map(ActiveSource::result).collect();
    ra.len() == rb.len()
        && ra
            .iter()
            .zip(&rb)
            .all(|(x, y)| Rc::ptr_eq(&x.result, &y.result))
}

impl CompletionState {
    pub(crate) fn update(&self, tr: &Transaction, state: &EditorState) -> CompletionState {
        let Some(conf) = state.config.completion.as_ref() else {
            return CompletionState::default();
        };
        let any_active = self.active.iter().any(|a| !a.is_inactive());
        let mut active: Vec<ActiveSource> = conf
            .sources
            .iter()
            .map(|source| {
                let value = self
                    .active
                    .iter()
                    .find(|s| s.source == *source)
                    .cloned()
                    .unwrap_or_else(|| {
                        ActiveSource::new(
                            source.clone(),
                            if any_active {
                                SourceState::Pending
                            } else {
                                SourceState::Inactive
                            },
                            false,
                        )
                    });
                value.update(tr, state)
            })
            .collect();
        let unchanged = active.len() == self.active.len()
            && active
                .iter()
                .zip(self.active.iter())
                .all(|(a, b)| a.same(b));
        let mut active_rc: Rc<[ActiveSource]> = if unchanged {
            self.active.clone()
        } else {
            active.clone().into()
        };
        let mut open = self.open.clone();
        let did_set = tr
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Completion(CompletionEffect::SetActive(_))));
        if let Some(o) = &open
            && tr.doc_changed()
        {
            open = Some(o.map(&tr.changes.desc()));
        }
        let touched = active.iter().any(|a| {
            a.result()
                .is_some_and(|r| tr.changes.touches_range(r.from, r.to) != Touch::No)
        });
        if tr.selection.is_some() || touched || !same_results(&active, &self.active) || did_set {
            open = CompletionDialog::build(&active, state, open.as_ref(), did_set);
        } else if open.as_ref().is_some_and(|o| o.disabled)
            && !active.iter().any(ActiveSource::is_pending)
        {
            open = None;
        }
        if open.is_none()
            && active.iter().all(|a| !a.is_pending())
            && active.iter().any(|a| a.result().is_some())
        {
            active = active
                .iter()
                .map(|a| {
                    if a.result().is_some() {
                        ActiveSource::new(a.source.clone(), SourceState::Inactive, false)
                    } else {
                        a.clone()
                    }
                })
                .collect();
            active_rc = active.into();
        }
        for e in &tr.effects {
            if let Effect::Completion(CompletionEffect::SetSelected(i)) = e {
                open = open.map(|o| o.set_selected(*i));
            }
        }
        CompletionState {
            active: active_rc,
            open,
        }
    }

    /// The tooltip, when the list is open.
    pub fn tooltip(&self) -> Option<&Rc<CompletionDialog>> {
        self.open.as_ref()
    }

    /// `completionStatus`.
    pub fn status(&self) -> Option<&'static str> {
        if self.active.iter().any(ActiveSource::is_pending) {
            Some("pending")
        } else if self.active.iter().any(|a| !a.is_inactive()) {
            Some("active")
        } else {
            None
        }
    }
}

/// `FuzzyMatcher`: scores a label against the typed pattern. Positions
/// and lengths are counted in UTF-16 units like CodeMirror, so scores (and
/// therefore ordering) match.
pub struct FuzzyMatcher {
    pattern: String,
    chars: Vec<u32>,
    folded: Vec<u32>,
    astral: bool,
    any: Vec<usize>,
    precise: Vec<usize>,
    by_word: Vec<usize>,
    /// `byWord.length` on CodeMirror's reused buffer: once any label set
    /// it, it stays non-zero for later labels.
    by_word_used: bool,
}

const NOT_FULL: i64 = -100;
const BY_WORD: i64 = -100;
const CASE_FOLD: i64 = -200;
const NOT_START: i64 = -700;
const GAP: i64 = -1100;

fn utf16_units(s: &str) -> Vec<(usize, u32, usize)> {
    let mut out = Vec::with_capacity(s.len());
    let mut u = 0;
    for (b, c) in s.char_indices() {
        out.push((u, c as u32, b));
        u += c.len_utf16();
    }
    out
}

impl FuzzyMatcher {
    pub fn new(pattern: &str) -> FuzzyMatcher {
        let mut chars = Vec::new();
        let mut folded = Vec::new();
        for c in pattern.chars() {
            chars.push(c as u32);
            let part = c.to_string();
            let upper = part.to_uppercase();
            let f = if upper == part {
                part.to_lowercase()
            } else {
                upper
            };
            folded.push(f.chars().next().map_or(c as u32, |x| x as u32));
        }
        FuzzyMatcher {
            astral: pattern.chars().any(|c| c.len_utf16() == 2),
            pattern: pattern.to_string(),
            chars,
            folded,
            any: Vec::new(),
            precise: Vec::new(),
            by_word: Vec::new(),
            by_word_used: false,
        }
    }

    /// Score and matched UTF-16 ranges, or `None` for no match.
    pub fn match_word(&mut self, word: &str) -> Option<(i64, Vec<usize>)> {
        let pattern_len16 = self.pattern.encode_utf16().count();
        if self.pattern.is_empty() {
            return Some((NOT_FULL, Vec::new()));
        }
        let units = utf16_units(word);
        let word_len16: usize = word.chars().map(char::len_utf16).sum();
        if word_len16 < pattern_len16 {
            return None;
        }
        let len = self.chars.len();
        if len == 1 {
            let first = units[0].1;
            let first_size = char::from_u32(first).map_or(1, char::len_utf16);
            let mut score = if first_size == word_len16 {
                0
            } else {
                NOT_FULL
            };
            if first == self.chars[0] {
            } else if first == self.folded[0] {
                score += CASE_FOLD;
            } else {
                return None;
            }
            return Some((score, vec![0, first_size]));
        }
        let direct = word
            .find(&self.pattern)
            .map(|b| word[..b].encode_utf16().count());
        if direct == Some(0) {
            let score = if word_len16 == pattern_len16 {
                0
            } else {
                NOT_FULL
            };
            return Some((score, vec![0, pattern_len16]));
        }
        self.any.resize(len, 0);
        self.precise.resize(len, 0);
        self.by_word.resize(len, 0);
        let mut any_to = 0;
        if direct.is_none() {
            for &(i, next, _) in units.iter().take_while(|u| u.0 < 200) {
                if any_to >= len {
                    break;
                }
                if next == self.chars[any_to] || next == self.folded[any_to] {
                    self.any[any_to] = i;
                    any_to += 1;
                }
            }
            if any_to < len {
                return None;
            }
        }
        let mut precise_to = 0;
        let mut by_word_to = 0;
        let mut by_word_folded = false;
        let mut adjacent_to = 0;
        let mut adjacent_start: Option<usize> = None;
        let mut adjacent_end = 0;
        let has_lower = word.bytes().any(|b| b.is_ascii_lowercase());
        let mut word_adjacent = true;
        let mut prev_type = 0u8;
        for &(i, next, _) in units.iter().take_while(|u| u.0 < 200) {
            if by_word_to >= len {
                break;
            }
            if direct.is_none() {
                if precise_to < len && next == self.chars[precise_to] {
                    self.precise[precise_to] = i;
                    precise_to += 1;
                }
                if adjacent_to < len {
                    if next == self.chars[adjacent_to] || next == self.folded[adjacent_to] {
                        if adjacent_to == 0 {
                            adjacent_start = Some(i);
                        }
                        adjacent_end = i + 1;
                        adjacent_to += 1;
                    } else {
                        adjacent_to = 0;
                    }
                }
            }
            let ty = if next < 0xff {
                if (48..=57).contains(&next) || (97..=122).contains(&next) {
                    2
                } else if (65..=90).contains(&next) {
                    1
                } else {
                    0
                }
            } else {
                let ch = char::from_u32(next).unwrap_or(' ');
                let s = ch.to_string();
                if s != s.to_lowercase() {
                    1
                } else if s != s.to_uppercase() {
                    2
                } else {
                    0
                }
            };
            if i == 0 || (ty == 1 && has_lower) || (prev_type == 0 && ty != 0) {
                let exact = self.chars[by_word_to] == next;
                let fold = !exact && self.folded[by_word_to] == next;
                if exact || fold {
                    if fold {
                        by_word_folded = true;
                    }
                    self.by_word[by_word_to] = i;
                    by_word_to += 1;
                    self.by_word_used = true;
                } else if self.by_word_used {
                    word_adjacent = false;
                }
            }
            prev_type = ty;
        }
        let w = word_len16 as i64;
        if by_word_to == len && self.by_word[0] == 0 && word_adjacent {
            let positions = self.by_word[..len].to_vec();
            return Some(self.result(
                BY_WORD + if by_word_folded { CASE_FOLD } else { 0 },
                &positions,
                &units,
                w,
            ));
        }
        if adjacent_to == len && adjacent_start == Some(0) {
            let full = if adjacent_end == word_len16 {
                0
            } else {
                NOT_FULL
            };
            return Some((CASE_FOLD - w + full, vec![0, adjacent_end]));
        }
        if let Some(d) = direct {
            return Some((NOT_START - w, vec![d, d + pattern_len16]));
        }
        if adjacent_to == len {
            return Some((
                CASE_FOLD + NOT_START - w,
                vec![adjacent_start.unwrap_or(0), adjacent_end],
            ));
        }
        if by_word_to == len {
            let positions = self.by_word[..len].to_vec();
            let score = BY_WORD
                + if by_word_folded { CASE_FOLD } else { 0 }
                + NOT_START
                + if word_adjacent { 0 } else { GAP };
            return Some(self.result(score, &positions, &units, w));
        }
        if len == 2 {
            return None;
        }
        let positions = self.any[..len].to_vec();
        let score = if positions[0] != 0 { NOT_START } else { 0 } + CASE_FOLD + GAP;
        Some(self.result(score, &positions, &units, w))
    }

    fn result(
        &self,
        score: i64,
        positions: &[usize],
        units: &[(usize, u32, usize)],
        word_len: i64,
    ) -> (i64, Vec<usize>) {
        let mut out: Vec<usize> = Vec::new();
        for &pos in positions {
            let size = if self.astral {
                units
                    .iter()
                    .find(|u| u.0 == pos)
                    .and_then(|u| char::from_u32(u.1))
                    .map_or(1, char::len_utf16)
            } else {
                1
            };
            let to = pos + size;
            if !out.is_empty() && out[out.len() - 1] == pos {
                let n = out.len();
                out[n - 1] = to;
            } else {
                out.push(pos);
                out.push(to);
            }
        }
        (score - word_len, out)
    }
}

/// UTF-16 match pairs to byte ranges of `label`.
fn utf16_ranges_to_bytes(label: &str, pairs: &[usize]) -> Vec<(usize, usize)> {
    let to_byte = |u: usize| {
        let mut acc = 0;
        for (b, c) in label.char_indices() {
            if acc >= u {
                return b;
            }
            acc += c.len_utf16();
        }
        label.len()
    };
    pairs
        .chunks(2)
        .filter(|p| p.len() == 2)
        .map(|p| (to_byte(p[0]), to_byte(p[1])))
        .collect()
}

fn option_score(c: &Completion) -> i64 {
    i64::from(c.boost.unwrap_or(0)) * 100
        + if c.apply.is_some() { 10 } else { 0 }
        + if c.info.is_some() { 5 } else { 0 }
        + i64::from(c.kind.is_some())
}

/// `sortOptions`: fuzzy-filter every result, sort by score then
/// `(sortText || label).localeCompare`, and drop duplicates.
fn sort_options(active: &[ActiveSource], state: &EditorState) -> Vec<CompletionOption> {
    let mut options: Vec<CompletionOption> = Vec::new();
    for a in active {
        let Some(r) = a.result() else { continue };
        let pattern = state.slice_doc(r.from, r.to);
        let mut matcher = FuzzyMatcher::new(&pattern);
        for option in r.result.options.iter() {
            if let Some((score, matched)) = matcher.match_word(&option.label) {
                let matched = if option.display_label.is_none() {
                    utf16_ranges_to_bytes(&option.label, &matched)
                } else {
                    Vec::new()
                };
                options.push(CompletionOption {
                    completion: option.clone(),
                    source: a.source.clone(),
                    matched,
                    score: score + i64::from(option.boost.unwrap_or(0)),
                });
            }
        }
    }
    options.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            let ka = a
                .completion
                .sort_text
                .as_deref()
                .unwrap_or(&a.completion.label);
            let kb = b
                .completion
                .sort_text
                .as_deref()
                .unwrap_or(&b.completion.label);
            locale_compare(ka, kb)
        })
    });
    let mut result: Vec<CompletionOption> = Vec::new();
    let mut prev: Option<Rc<Completion>> = None;
    for opt in options {
        let c = opt.completion.clone();
        let distinct = match &prev {
            None => true,
            Some(p) => {
                p.label != c.label
                    || p.detail != c.detail
                    || (p.kind.is_some() && c.kind.is_some() && p.kind != c.kind)
                    || p.apply != c.apply
                    || p.boost != c.boost
            }
        };
        if distinct {
            result.push(opt);
        } else if prev
            .as_ref()
            .is_some_and(|p| option_score(&c) > option_score(p))
        {
            let n = result.len();
            result[n - 1] = opt;
        }
        prev = Some(c);
    }
    result
}

/// `insertCompletionText`: replace `from..to` in the main selection and the
/// same text before every other cursor.
pub fn insert_completion_text(
    state: &EditorState,
    text: &str,
    from: usize,
    to: usize,
) -> TransactionSpec {
    let main = state.selection.main();
    let main_index = state.selection.main_index;
    let from_off = from as i64 - main.from as i64;
    let to_off = to as i64 - main.from as i64;
    let replaced = state.slice_doc(from, to);
    let insert = normalize_newlines(text).into_owned();
    let mut index = 0;
    state
        .change_by_range(|range| {
            let is_main = index == main_index;
            index += 1;
            let start = range.from as i64 + from_off;
            let end = range.from as i64 + to_off;
            if start < 0 || end < 0 {
                return RangeChange::range(range);
            }
            let (start, end) = (start as usize, end as usize);
            if !is_main
                && from != to
                && state.slice_doc(start, end.min(state.doc.len())) != replaced
            {
                return RangeChange::range(range);
            }
            RangeChange::with(
                vec![Change::new(
                    start,
                    if to == main.from { range.to } else { end },
                    insert.clone(),
                )],
                SelectionRange::cursor(start + insert.len()),
            )
        })
        .spec()
        .scroll()
        .user_event("input.complete")
}

/// `applyCompletion`.
pub fn apply_completion(d: &mut dyn Dispatch, option: &CompletionOption) -> bool {
    let active = d.state().completion.active.clone();
    let Some(r) = active
        .iter()
        .find(|a| a.source == option.source)
        .and_then(ActiveSource::result)
        .cloned()
    else {
        return false;
    };
    let c = option.completion.clone();
    match &c.apply {
        Some(CompletionApply::Fn(f)) => f(d, &c, r.from, r.to),
        Some(CompletionApply::Text(t)) => {
            let spec = insert_completion_text(d.state(), t, r.from, r.to);
            d.dispatch(spec);
        }
        None => {
            let spec = insert_completion_text(d.state(), &c.label, r.from, r.to);
            d.dispatch(spec);
        }
    }
    true
}

fn interactive(state: &EditorState) -> Option<&Rc<CompletionDialog>> {
    let open = state.completion.open.as_ref()?;
    (!open.disabled && now_ms().saturating_sub(open.timestamp) >= INTERACTION_DELAY).then_some(open)
}

/// `moveCompletionSelection`. `page` is the number of options a page
/// moves (from the rendered list height), for PageUp/PageDown.
pub fn move_completion_selection(
    state: &EditorState,
    forward: bool,
    page: Option<usize>,
) -> Option<TransactionSpec> {
    let open = interactive(state)?;
    let step = page.map_or(1, |p| p.max(2)) as i64;
    let length = open.options.len() as i64;
    let mut selected = match open.selected {
        Some(s) => s as i64 + step * if forward { 1 } else { -1 },
        None if forward => 0,
        None => length - 1,
    };
    if selected < 0 {
        selected = if page.is_some() { 0 } else { length - 1 };
    } else if selected >= length {
        selected = if page.is_some() { length - 1 } else { 0 };
    }
    Some(
        TransactionSpec::default().effect(Effect::Completion(CompletionEffect::SetSelected(
            selected as usize,
        ))),
    )
}

/// `acceptCompletion` (Enter).
pub fn accept_completion(d: &mut dyn Dispatch) -> bool {
    let state = d.state();
    if state.read_only() {
        return false;
    }
    let Some(open) = interactive(state) else {
        return false;
    };
    let Some(sel) = open.selected else {
        return false;
    };
    let option = open.options[sel].clone();
    apply_completion(d, &option)
}

/// `startCompletion` (Ctrl-Space, ⌥` and ⌥I on macOS).
pub fn start_completion(state: &EditorState) -> Option<TransactionSpec> {
    state.config.completion.as_ref()?;
    Some(TransactionSpec::default().effect(Effect::Completion(CompletionEffect::Start(true))))
}

/// `closeCompletion` (Escape).
pub fn close_completion(state: &EditorState) -> Option<TransactionSpec> {
    if !state.completion.active.iter().any(|a| !a.is_inactive()) {
        return None;
    }
    Some(TransactionSpec::default().effect(Effect::Completion(CompletionEffect::Close)))
}

/// `commitCharacters`: typing one of the selected option's commit
/// characters accepts it first (the typed character is still inserted).
pub fn commit_character(d: &mut dyn Dispatch, key: &str) {
    let state = d.state();
    let Some(open) = state.completion.open.as_ref() else {
        return;
    };
    let Some(sel) = open.selected.filter(|_| !open.disabled) else {
        return;
    };
    let option = open.options[sel].clone();
    let chars = option.completion.commit_characters.clone().or_else(|| {
        state
            .completion
            .active
            .iter()
            .find(|a| a.source == option.source)
            .and_then(|a| a.result().and_then(|r| r.result.commit_characters.clone()))
    });
    if chars.is_some_and(|c| c.iter().any(|k| k == key)) {
        apply_completion(d, &option);
    }
}

/// `rangeAroundSelected`: the window of options the list renders.
pub fn range_around_selected(total: usize, selected: Option<usize>, max: usize) -> (usize, usize) {
    if total <= max {
        return (0, total);
    }
    let selected = selected.unwrap_or(0);
    if selected <= total >> 1 {
        let off = selected / max;
        return (off * max, (off + 1) * max);
    }
    let off = (total - selected).div_ceil(max);
    (total - off * max, total - (off - 1) * max)
}

/// `completeAnyWord`: every distinct word in the document (up to 2000),
/// skipping the one being typed.
pub struct AnyWordSource;

const MAX_WORDS: usize = 2000;

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric() || c == '_'
}

impl CompletionSource for AnyWordSource {
    fn complete(&self, cx: &CompletionContext) -> SourceResult {
        let token = cx.match_before("[\\p{Alphabetic}\\p{Number}_]+$", "u");
        if token.is_none() && !cx.explicit {
            return SourceResult::Ready(None);
        }
        let from = token.as_ref().map_or(cx.pos, |t| t.0);
        let doc = &cx.state.doc;
        let mut seen = std::collections::HashSet::new();
        let mut options = Vec::new();
        'lines: for n in 1..=doc.lines() {
            let line = doc.line(n);
            let text = doc.slice(line.from, line.to);
            let mut start: Option<usize> = None;
            for (i, c) in text
                .char_indices()
                .chain(std::iter::once((text.len(), ' ')))
            {
                if is_word_char(c) && i < text.len() {
                    start.get_or_insert(i);
                    continue;
                }
                if let Some(s) = start.take() {
                    let word = &text[s..i];
                    if line.from + s != from && seen.insert(word.to_string()) {
                        options.push(Completion {
                            label: word.to_string(),
                            kind: Some("text".into()),
                            ..Completion::default()
                        });
                        if options.len() >= MAX_WORDS {
                            break 'lines;
                        }
                    }
                }
            }
        }
        let mut result = CompletionResult::new(from, options);
        result.valid_for = ValidFor::regex("^[\\p{Alphabetic}\\p{Number}_]+", "u");
        SourceResult::Ready(Some(result))
    }
}

/// `bundleSnippetCompletionSource`: tab-trigger snippets for the buffer's
/// scope and path, boosted above words.
pub struct SnippetCompletionSource {
    pub provider: Rc<dyn SnippetProvider>,
    pub scope_path: Rc<dyn Fn() -> (String, String)>,
}

fn snippet_options(matches: Vec<RegisteredSnippet>) -> Vec<Completion> {
    matches
        .into_iter()
        .map(|sn| {
            let label = sn.tab_trigger.clone();
            let detail = sn.label.clone();
            let sn = Rc::new(sn);
            Completion {
                label,
                detail: Some(detail),
                kind: Some("snippet".into()),
                boost: Some(99),
                apply: Some(CompletionApply::Fn(Rc::new(
                    move |d: &mut dyn Dispatch, _: &Completion, from, to| {
                        apply_snippet(d, &sn, from, to, Some(""));
                    },
                ))),
                ..Completion::default()
            }
        })
        .collect()
}

impl CompletionSource for SnippetCompletionSource {
    fn complete(&self, cx: &CompletionContext) -> SourceResult {
        let found = cx.match_before("[A-Za-z_][A-Za-z0-9_]*", "");
        if found.is_none() && !cx.explicit {
            return SourceResult::Ready(None);
        }
        let prefix = found.as_ref().map(|f| f.2.clone()).unwrap_or_default();
        if prefix.is_empty() && !cx.explicit {
            return SourceResult::Ready(None);
        }
        let (scope, path) = (self.scope_path)();
        let load = self.provider.load(&scope, &path);
        let explicit = cx.explicit;
        let from = found.as_ref().map_or(cx.pos, |f| f.0);
        SourceResult::Pending(Box::pin(async move {
            let matches = snippets_matching_prefix(&load.await, &prefix, explicit);
            if matches.is_empty() {
                return Ok(None);
            }
            let mut result = CompletionResult::new(from, snippet_options(matches));
            result.valid_for = ValidFor::regex("^[A-Za-z_][A-Za-z0-9_]*$", "");
            Ok(Some(result))
        }))
    }
}

/// A query the view must run: poll `future`, then report back with
/// [`CompletionDriver::query_done`].
pub struct QueryLaunch {
    pub id: u64,
    pub future: QueryFuture,
}

struct RunningQuery {
    id: u64,
    source: SourceRef,
    explicit: bool,
    context: CompletionContext,
    time: u64,
    updates: Vec<Rc<Transaction>>,
    done: Option<Option<CompletionResult>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Composing {
    None,
    Started,
    Changed,
    ChangedAndMoved,
}

/// What the view should do after driving the completion plugin.
#[derive(Default)]
pub struct DriverStep {
    pub dispatch: Vec<TransactionSpec>,
    pub launch: Vec<QueryLaunch>,
}

/// The `completionPlugin` view plugin: debounces activation, runs source
/// queries, aborts stale ones and folds finished results back into the
/// state. Time is passed in; the view schedules a wakeup at
/// [`CompletionDriver::next_deadline`].
pub struct CompletionDriver {
    running: Vec<RunningQuery>,
    debounce_update: Option<u64>,
    debounce_accept: Option<u64>,
    pending_start: bool,
    composing: Composing,
    next_id: u64,
}

impl Default for CompletionDriver {
    fn default() -> Self {
        CompletionDriver {
            running: Vec::new(),
            debounce_update: None,
            debounce_accept: None,
            pending_start: false,
            composing: Composing::None,
            next_id: 0,
        }
    }
}

impl CompletionDriver {
    pub fn new(state: &EditorState) -> (CompletionDriver, DriverStep) {
        let mut d = CompletionDriver::default();
        let mut step = DriverStep::default();
        for a in state.completion.active.iter() {
            if a.is_pending() {
                step.launch.push(d.start_query(a, state));
            }
        }
        (d, step)
    }

    pub fn next_deadline(&self) -> Option<u64> {
        match (self.debounce_update, self.debounce_accept) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn start_query(&mut self, active: &ActiveSource, state: &EditorState) -> QueryLaunch {
        self.next_id += 1;
        let id = self.next_id;
        let context = CompletionContext::new(state.clone(), cur(state), active.explicit);
        let future: QueryFuture = match active.source.0.complete(&context) {
            SourceResult::Ready(r) => Box::pin(futures::future::ready(Ok(r))),
            SourceResult::Pending(f) => f,
        };
        self.running.push(RunningQuery {
            id,
            source: active.source.clone(),
            explicit: active.explicit,
            context,
            time: now_ms(),
            updates: Vec::new(),
            done: None,
        });
        QueryLaunch { id, future }
    }

    /// `update(ViewUpdate)`: `transactions` were just applied, turning
    /// `start` into `state`.
    pub fn update(
        &mut self,
        start: &EditorState,
        state: &EditorState,
        transactions: &[Rc<Transaction>],
        now: u64,
    ) {
        let selection_set = transactions.iter().any(|t| t.selection.is_some());
        let doc_changed = transactions.iter().any(|t| t.doc_changed());
        let same_state = Rc::ptr_eq(&start.completion.active, &state.completion.active)
            && match (&start.completion.open, &state.completion.open) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            };
        if !selection_set && !doc_changed && same_state {
            return;
        }
        let does_reset = transactions.iter().any(|tr| {
            let ty = update_type(tr);
            ty & RESET != 0
                || ((tr.selection.is_some() || tr.doc_changed()) && ty & SIMPLE_INTERACTION == 0)
        });
        let mut i = 0;
        while i < self.running.len() {
            let q = &mut self.running[i];
            if does_reset
                || (q.context.abort.on_doc_change.get() && doc_changed)
                || (q.updates.len() + transactions.len() > MAX_UPDATE_COUNT
                    && now.saturating_sub(q.time) > MIN_ABORT_TIME)
            {
                q.context.abort.abort();
                self.running.remove(i);
            } else {
                q.updates.extend(transactions.iter().cloned());
                i += 1;
            }
        }
        if transactions.iter().any(|t| {
            t.effects
                .iter()
                .any(|e| matches!(e, Effect::Completion(CompletionEffect::Start(_))))
        }) {
            self.pending_start = true;
        }
        let delay = if self.pending_start {
            50
        } else {
            ACTIVATE_ON_TYPING_DELAY
        };
        let needs_start = state
            .completion
            .active
            .iter()
            .any(|a| a.is_pending() && !self.running.iter().any(|q| q.source == a.source));
        self.debounce_update = needs_start.then_some(now + delay);
        if self.composing != Composing::None {
            for tr in transactions {
                if tr.is_user_event("input.type") {
                    self.composing = Composing::Changed;
                } else if self.composing == Composing::Changed && tr.selection.is_some() {
                    self.composing = Composing::ChangedAndMoved;
                }
            }
        }
    }

    /// Fire due timers.
    pub fn tick(&mut self, state: &EditorState, now: u64) -> DriverStep {
        let mut step = DriverStep::default();
        if self.debounce_update.is_some_and(|d| d <= now) {
            self.debounce_update = None;
            self.pending_start = false;
            for a in state.completion.active.iter() {
                if a.is_pending() && !self.running.iter().any(|q| q.source == a.source) {
                    step.launch.push(self.start_query(a, state));
                }
            }
            if !self.running.is_empty()
                && state.completion.open.as_ref().is_some_and(|o| o.disabled)
            {
                self.debounce_accept = Some(now + UPDATE_SYNC_TIME);
            }
        }
        if self.debounce_accept.is_some_and(|d| d <= now) {
            let more = self.accept(state);
            step.dispatch.extend(more.dispatch);
            step.launch.extend(more.launch);
        }
        step
    }

    /// A launched query finished. Errors close completion.
    pub fn query_done(
        &mut self,
        id: u64,
        result: Result<Option<CompletionResult>, String>,
        state: &EditorState,
        now: u64,
    ) -> DriverStep {
        let Some(q) = self.running.iter_mut().find(|q| q.id == id) else {
            return DriverStep::default();
        };
        if q.context.abort.aborted() {
            return DriverStep::default();
        }
        match result {
            Err(_) => DriverStep {
                dispatch: vec![
                    TransactionSpec::default().effect(Effect::Completion(CompletionEffect::Close)),
                ],
                launch: Vec::new(),
            },
            Ok(r) => {
                q.done = Some(r);
                if self.running.iter().all(|q| q.done.is_some()) {
                    self.accept(state)
                } else {
                    if self.debounce_accept.is_none() {
                        self.debounce_accept = Some(now + UPDATE_SYNC_TIME);
                    }
                    DriverStep::default()
                }
            }
        }
    }

    fn accept(&mut self, state: &EditorState) -> DriverStep {
        self.debounce_accept = None;
        let mut step = DriverStep::default();
        let mut updated: Vec<ActiveSource> = Vec::new();
        let mut i = 0;
        while i < self.running.len() {
            if self.running[i].done.is_none() {
                i += 1;
                continue;
            }
            let q = self.running.remove(i);
            let done = q.done.flatten();
            if let Some(result) = done.clone() {
                let pos = cur(q.updates.first().map_or(state, |t| &t.start_state));
                let limit = pos.min(result.from + usize::from(!q.explicit));
                let to = result.to.unwrap_or(pos);
                let from = result.from;
                let mut active = ActiveSource::new(
                    q.source.clone(),
                    SourceState::Result(Rc::new(ActiveResult {
                        limit,
                        result: Rc::new(result),
                        from,
                        to,
                    })),
                    q.explicit,
                );
                for tr in &q.updates {
                    active = active.update(tr, tr.state());
                }
                if active.result().is_some() {
                    updated.push(active);
                    continue;
                }
            }
            let current = state
                .completion
                .active
                .iter()
                .find(|a| a.source == q.source)
                .cloned();
            if let Some(current) = current
                && current.is_pending()
            {
                if done.is_none() {
                    let mut active =
                        ActiveSource::new(q.source.clone(), SourceState::Inactive, false);
                    for tr in &q.updates {
                        active = active.update(tr, tr.state());
                    }
                    if !active.is_pending() {
                        updated.push(active);
                    }
                } else {
                    step.launch.push(self.start_query(&current, state));
                }
            }
        }
        if !updated.is_empty() || state.completion.open.as_ref().is_some_and(|o| o.disabled) {
            step.dispatch
                .push(TransactionSpec::default().effect(Effect::Completion(
                    CompletionEffect::SetActive(updated.into()),
                )));
        }
        step
    }

    /// Editor blur: close the list unless focus moved into it. The view
    /// dispatches the returned close 10ms later, like CodeMirror.
    pub fn blur(&self, state: &EditorState, focus_in_tooltip: bool) -> Option<TransactionSpec> {
        (state.completion.open.is_some() && !focus_in_tooltip)
            .then(|| TransactionSpec::default().effect(Effect::Completion(CompletionEffect::Close)))
    }

    pub fn composition_start(&mut self) {
        self.composing = Composing::Started;
    }

    /// IME composition ended: restart completion if it moved the cursor
    /// after changing text (dispatched 20ms later).
    pub fn composition_end(&mut self) -> Option<TransactionSpec> {
        let restart = self.composing == Composing::ChangedAndMoved;
        self.composing = Composing::None;
        restart.then(|| {
            TransactionSpec::default().effect(Effect::Completion(CompletionEffect::Start(false)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateConfig;

    fn fuzzy(pattern: &str, word: &str) -> Option<(i64, Vec<usize>)> {
        FuzzyMatcher::new(pattern).match_word(word)
    }

    #[test]
    fn fuzzy_matcher_scores() {
        assert_eq!(fuzzy("f", "foo"), Some((-100, vec![0, 1])));
        assert_eq!(fuzzy("F", "foo"), Some((-300, vec![0, 1])));
        assert_eq!(fuzzy("x", "foo"), None);
        assert_eq!(fuzzy("fo", "foo"), Some((-100, vec![0, 2])));
        assert_eq!(fuzzy("foo", "foo"), Some((0, vec![0, 3])));
        assert_eq!(
            fuzzy("gEB", "getElementById"),
            Some((-114, vec![0, 1, 3, 4, 10, 11]))
        );
        assert_eq!(fuzzy("oo", "foo"), Some((-703, vec![1, 3])));
        assert_eq!(fuzzy("FO", "foo"), Some((-303, vec![0, 2])));
        assert_eq!(fuzzy("fb", "fooBar"), Some((-306, vec![0, 1, 3, 4])));
        assert_eq!(fuzzy("fr", "fooBar"), None);
        assert_eq!(
            fuzzy("fbr", "fooBar"),
            Some((-1306, vec![0, 1, 3, 4, 5, 6]))
        );
        assert_eq!(fuzzy("", "foo"), Some((-100, vec![])));
    }

    #[test]
    fn range_window() {
        assert_eq!(range_around_selected(50, Some(10), 100), (0, 50));
        assert_eq!(range_around_selected(250, Some(10), 100), (0, 100));
        assert_eq!(range_around_selected(250, Some(130), 100), (50, 150));
        assert_eq!(range_around_selected(250, Some(240), 100), (150, 250));
    }

    fn words_state(doc: &str) -> EditorState {
        EditorState::create(
            doc,
            StateConfig {
                completion: Some(Rc::new(CompletionConfig {
                    sources: vec![SourceRef(Rc::new(AnyWordSource))],
                })),
                ..StateConfig::default()
            },
        )
    }

    fn drive(s: &mut EditorState, driver: &mut CompletionDriver, spec: TransactionSpec) {
        let start = s.clone();
        let tr = Rc::new(s.update(spec));
        *s = tr.state().clone();
        driver.update(&start, s, &[tr], now_ms());
    }

    fn run_ready(s: &mut EditorState, driver: &mut CompletionDriver, step: DriverStep) {
        for spec in step.dispatch {
            drive(s, driver, spec);
        }
        for launch in step.launch {
            let r = futures::executor::block_on(launch.future);
            let next = driver.query_done(launch.id, r, s, now_ms());
            run_ready(s, driver, next);
        }
    }

    #[test]
    fn typing_opens_word_completions_and_accept_inserts() {
        let mut s = words_state("alpha alphabet beta\n");
        let (mut driver, _) = CompletionDriver::new(&s);
        let end = s.doc.len();
        drive(&mut s, &mut driver, TransactionSpec::default().cursor(end));
        drive(
            &mut s,
            &mut driver,
            TransactionSpec::default()
                .changes(Change::insert(end, "al"))
                .cursor(end + 2)
                .user_event("input.type"),
        );
        assert!(s.completion.active[0].is_pending());
        let step = driver.tick(&s, now_ms() + ACTIVATE_ON_TYPING_DELAY + 1);
        run_ready(&mut s, &mut driver, step);
        let open = s.completion.open.clone().expect("open");
        let labels: Vec<_> = open
            .options
            .iter()
            .map(|o| o.completion.label.clone())
            .collect();
        assert_eq!(labels, vec!["alpha", "alphabet"]);
        assert_eq!(open.options[0].matched, vec![(0, 2)]);
        let option = open.options[1].clone();
        assert!(apply_completion(&mut s, &option));
        assert_eq!(s.doc.to_string(), "alpha alphabet beta\nalphabet");
        assert!(s.completion.open.is_none());
    }

    #[test]
    fn typing_more_reuses_a_valid_result() {
        let mut s = words_state("alpha alphabet beta\n");
        let (mut driver, _) = CompletionDriver::new(&s);
        let end = s.doc.len();
        drive(&mut s, &mut driver, TransactionSpec::default().cursor(end));
        drive(
            &mut s,
            &mut driver,
            TransactionSpec::default()
                .changes(Change::insert(end, "a"))
                .cursor(end + 1)
                .user_event("input.type"),
        );
        let step = driver.tick(&s, now_ms() + 1000);
        run_ready(&mut s, &mut driver, step);
        let first = s.completion.active[0].result().unwrap().result.clone();
        drive(
            &mut s,
            &mut driver,
            TransactionSpec::default()
                .changes(Change::insert(end + 1, "l"))
                .cursor(end + 2)
                .user_event("input.type"),
        );
        let again = s.completion.active[0].result().unwrap().result.clone();
        assert!(Rc::ptr_eq(&first, &again));
        assert_eq!(s.completion.open.as_ref().unwrap().options.len(), 2);
        let close = close_completion(&s).unwrap();
        drive(&mut s, &mut driver, close);
        assert!(s.completion.open.is_none());
    }

    #[test]
    fn insert_text_applies_to_matching_cursors() {
        let s = EditorState::create(
            "ab\nab",
            StateConfig {
                allow_multiple_selections: true,
                ..StateConfig::default()
            },
        );
        let s = s
            .update(TransactionSpec::default().selection(
                crate::selection::EditorSelection::create(
                    vec![SelectionRange::cursor(2), SelectionRange::cursor(5)],
                    0,
                ),
            ))
            .state()
            .clone();
        let next = s
            .update(insert_completion_text(&s, "abc", 0, 2))
            .state()
            .clone();
        assert_eq!(next.doc.to_string(), "abc\nabc");
    }
}
