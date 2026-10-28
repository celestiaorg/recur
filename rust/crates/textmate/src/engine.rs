//! Tokenizer engine with per-document line-state caches (`tm-engine.ts`).
//!
//! Two token tiers per document:
//!  - accurate: state-chained from line 1, watermark = `cache.filled()`
//!  - provisional: guessed-start viewport tokens beyond the watermark (VS
//!    Code's tokenizeHeuristically), corrected when the accurate sweep
//!    reaches them and never written to the state store.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::delta::{LineReplace, TmDocFingerprint, splice_line_map, tm_doc_fingerprint};
use crate::grammar::{Grammar, StateStack};
use crate::raw::{RawGrammar, parse_raw_grammar};
use crate::registry::{GrammarSource, LoadGrammarError, Registry};
use crate::stacks::{LineTokens, TmStackCache, tokenize_line_range};
use crate::theme::RawTheme;
use crate::utils::utf16_len;

/// Documents nothing displays are kept until these budgets overflow.
pub const MAX_IDLE_DOCS: usize = 128;
pub const MAX_IDLE_LINES: usize = 500_000;
/// Lines longer than this (UTF-16 units) tokenize as empty (VS Code: 20k).
const MAX_TOKENIZE_LINE_LENGTH: usize = 20_000;
/// Upper bound on the backwards indentation walk for guessed starts.
const MAX_GUESS_SCAN_LINES: usize = 5_000;
/// Lines one [`TmEngine::fill_slice`] step snapshots.
const SLICE_JOB_LINES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrammarSourceText {
    pub content: String,
    pub path_hint: String,
}

pub type GrammarSourceLoader = Box<dyn FnMut(&str) -> Option<GrammarSourceText>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportMode {
    Accurate,
    Guessed,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportResult {
    pub mode: ViewportMode,
    pub tokenized: usize,
    /// Every requested line has tokens (accurate or provisional).
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillProgress {
    pub filled: usize,
    pub total: usize,
    /// 1-based range whose tokens differ from what was shown before.
    pub changed_from: Option<usize>,
    pub changed_to: usize,
    pub done: bool,
}

/// An owned snapshot of the next stretch of the accurate sweep, so it can
/// tokenize off the UI thread. [`TmEngine::apply_fill`] takes the result.
pub struct FillJob {
    grammar: Arc<Grammar>,
    version: u64,
    filled: usize,
    from: usize,
    stack: Option<StateStack>,
    /// Text and stale end stack of each line from `from`.
    lines: Vec<(String, Option<StateStack>)>,
}

/// What a [`FillJob`] tokenized, for [`TmEngine::apply_fill`].
pub struct FillChunk {
    version: u64,
    filled: usize,
    from: usize,
    lines: Vec<(Vec<u32>, StateStack)>,
    /// The last line ended in its stale state.
    restabilized: bool,
    failed: bool,
}

impl FillJob {
    /// Tokenize until a line ends in its stale state, the snapshot runs out
    /// or `budget` is spent (at least one line).
    pub fn run(self, budget: Duration) -> FillChunk {
        let deadline = Instant::now() + budget;
        let mut chunk = FillChunk {
            version: self.version,
            filled: self.filled,
            from: self.from,
            lines: Vec::with_capacity(self.lines.len()),
            restabilized: false,
            failed: false,
        };
        let mut stack = self.stack;
        for (text, stale) in self.lines {
            let Ok(r) = self.grammar.tokenize_line2(&text, stack.as_ref()) else {
                chunk.failed = true;
                break;
            };
            chunk.restabilized = stale.is_some_and(|s| r.rule_stack.equals(&s));
            stack = Some(r.rule_stack.clone());
            chunk.lines.push((r.tokens, r.rule_stack));
            if chunk.restabilized || Instant::now() > deadline {
                break;
            }
        }
        chunk
    }
}

/// A token with its scope list (`tokenizeLine`), byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeToken {
    pub start: usize,
    pub end: usize,
    pub scopes: Vec<String>,
}

struct TmDoc {
    scope: String,
    lines: Vec<String>,
    cache: TmStackCache,
    provisional: HashMap<usize, LineTokens>,
    /// Changes on every edit, so fill results for older text are dropped.
    version: u64,
    /// A grammar regex failed to compile; highlighting stops for this doc.
    failed: bool,
}

fn first_non_whitespace_column(text: &str) -> usize {
    text.bytes()
        .position(|c| c != b' ' && c != b'\t')
        .map_or(0, |i| i + 1)
}

fn doc_length(lines: &[String]) -> usize {
    lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1)
}

fn line_text_for_tokenize(doc: &TmDoc, n: usize) -> String {
    let text = doc.lines.get(n - 1).map_or("", String::as_str);
    // Minified monster lines tokenize as a stub so the grammar state stays
    // sane without a regex storm (VS Code nulls these too).
    if text.len() > MAX_TOKENIZE_LINE_LENGTH && utf16_len(text) > MAX_TOKENIZE_LINE_LENGTH {
        String::new()
    } else {
        text.to_string()
    }
}

fn first_unfresh_line(cache: &TmStackCache, first: usize, last: usize) -> usize {
    (first..=last)
        .find(|&n| n > cache.filled() || cache.tokens_at(n).is_none())
        .unwrap_or(last + 1)
}

struct Sources<'a> {
    sources: &'a mut HashMap<String, GrammarSourceText>,
    loader: &'a mut Option<GrammarSourceLoader>,
    inject_to: &'a HashMap<String, Vec<String>>,
}

impl GrammarSource for Sources<'_> {
    fn load_grammar(&mut self, scope: &str) -> Result<Option<RawGrammar>, LoadGrammarError> {
        if !self.sources.contains_key(scope)
            && let Some(loader) = self.loader.as_mut()
            && let Some(got) = loader(scope)
        {
            self.sources.insert(scope.to_string(), got);
        }
        let Some(src) = self.sources.get(scope) else {
            return Ok(None);
        };
        parse_raw_grammar(&src.content, Some(&src.path_hint))
            .map(Some)
            .map_err(|e| LoadGrammarError::Source {
                scope: scope.to_string(),
                message: e.to_string(),
            })
    }

    fn injections(&mut self, scope: &str) -> Option<Vec<String>> {
        self.inject_to.get(scope).cloned()
    }
}

#[derive(Default)]
pub struct TmEngine {
    registry: Option<Registry>,
    theme: Option<RawTheme>,
    sources: HashMap<String, GrammarSourceText>,
    inject_to: HashMap<String, Vec<String>>,
    path_hints: HashMap<String, String>,
    grammar_cache: HashMap<String, Arc<Grammar>>,
    docs: HashMap<String, TmDoc>,
    source_loader: Option<GrammarSourceLoader>,
    live_refs: HashMap<String, usize>,
    idle_docs: Vec<String>,
    versions: u64,
}

impl TmEngine {
    pub fn new() -> TmEngine {
        TmEngine::default()
    }

    pub fn set_source_loader(&mut self, loader: Option<GrammarSourceLoader>) {
        self.source_loader = loader;
    }

    pub fn note_grammar(&mut self, scope: &str, path_hint: &str, inject_to: &[String]) {
        self.sources.remove(scope);
        self.path_hints
            .insert(scope.to_string(), path_hint.to_string());
        for host in inject_to {
            let set = self.inject_to.entry(host.clone()).or_default();
            if !set.iter().any(|s| s == scope) {
                set.push(scope.to_string());
            }
        }
        if self.registry.is_some() {
            self.drop_registry();
        }
    }

    pub fn register_grammar(
        &mut self,
        scope: &str,
        content: &str,
        path_hint: &str,
        inject_to: &[String],
    ) {
        self.note_grammar(scope, path_hint, inject_to);
        self.sources.insert(
            scope.to_string(),
            GrammarSourceText {
                content: content.to_string(),
                path_hint: path_hint.to_string(),
            },
        );
    }

    /// Apply a theme and drop every document: token metadata embeds colour
    /// ids under the old theme, so nothing survives a theme change.
    pub fn set_theme(&mut self, raw: RawTheme) -> Vec<String> {
        self.ensure_registry().set_theme(&raw);
        self.theme = Some(raw);
        self.docs.clear();
        self.idle_docs.clear();
        self.live_refs.clear();
        self.color_map()
    }

    pub fn color_map(&mut self) -> Vec<String> {
        self.ensure_registry().color_map()
    }

    pub fn has_grammar(&self, scope: &str) -> bool {
        self.grammar_cache.contains_key(scope)
    }

    /// Load and compile a grammar. `Ok(false)` when nothing provides it.
    pub fn ensure_grammar(&mut self, scope: &str) -> Result<bool, LoadGrammarError> {
        if self.grammar_cache.contains_key(scope) {
            return Ok(true);
        }
        self.ensure_registry();
        let registry = self.registry.as_mut().expect("ensured above");
        let mut src = Sources {
            sources: &mut self.sources,
            loader: &mut self.source_loader,
            inject_to: &self.inject_to,
        };
        match registry.load_grammar(scope, &mut src)? {
            Some(g) => {
                self.grammar_cache.insert(scope.to_string(), g);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Open or reuse a document and take a live ref on it. Same id, scope and
    /// text keeps the line-state cache.
    pub fn open_doc(&mut self, doc_id: &str, scope: &str, text: &str) -> bool {
        self.retain_doc(doc_id);
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        if let Some(prev) = self.docs.get(doc_id)
            && prev.scope == scope
            && prev.lines == lines
        {
            return true;
        }
        self.versions += 1;
        self.docs.insert(
            doc_id.to_string(),
            TmDoc {
                scope: scope.to_string(),
                lines,
                cache: TmStackCache::default(),
                provisional: HashMap::new(),
                version: self.versions,
                failed: false,
            },
        );
        false
    }

    /// Reuse the resident doc when the fingerprint matches (no text copy).
    pub fn reopen_doc(&mut self, doc_id: &str, scope: &str, fp: TmDocFingerprint) -> bool {
        if !self.match_doc(doc_id, scope, fp) {
            return false;
        }
        self.retain_doc(doc_id);
        true
    }

    pub fn match_doc(&self, doc_id: &str, scope: &str, fp: TmDocFingerprint) -> bool {
        let Some(doc) = self.docs.get(doc_id) else {
            return false;
        };
        if doc.scope != scope || doc.lines.len() != fp.lines {
            return false;
        }
        let length = doc_length(&doc.lines);
        if length != fp.length {
            return false;
        }
        tm_doc_fingerprint(|n| doc.lines[n - 1].clone(), doc.lines.len(), length).sample
            == fp.sample
    }

    /// The fingerprint `match_doc` compares against, for a text.
    pub fn fingerprint_of(text: &str) -> TmDocFingerprint {
        let lines: Vec<&str> = text.split('\n').collect();
        tm_doc_fingerprint(|n| lines[n - 1].to_string(), lines.len(), text.len())
    }

    fn retain_doc(&mut self, doc_id: &str) {
        *self.live_refs.entry(doc_id.to_string()).or_default() += 1;
        self.idle_docs.retain(|id| id != doc_id);
    }

    /// Drop a live ref; caches stay until the idle budget overflows.
    pub fn release_doc(&mut self, doc_id: &str) {
        let n = self
            .live_refs
            .get(doc_id)
            .copied()
            .unwrap_or(1)
            .saturating_sub(1);
        if n > 0 {
            self.live_refs.insert(doc_id.to_string(), n);
            return;
        }
        self.live_refs.remove(doc_id);
        self.idle_docs.retain(|id| id != doc_id);
        self.idle_docs.push(doc_id.to_string());
        self.trim_idle();
    }

    fn trim_idle(&mut self) {
        let mut lines: usize = self
            .idle_docs
            .iter()
            .map(|id| self.docs.get(id).map_or(0, |d| d.lines.len()))
            .sum();
        while !self.idle_docs.is_empty()
            && (self.idle_docs.len() > MAX_IDLE_DOCS || lines > MAX_IDLE_LINES)
        {
            let evict = self.idle_docs.remove(0);
            lines -= self.docs.get(&evict).map_or(0, |d| d.lines.len());
            if !self.live_refs.contains_key(&evict) {
                self.docs.remove(&evict);
            }
        }
    }

    /// Free a document unless a view still holds it.
    pub fn close_if_idle(&mut self, doc_id: &str) -> bool {
        if self.live_refs.contains_key(doc_id) {
            return false;
        }
        self.close_doc(doc_id);
        true
    }

    pub fn has_doc(&self, doc_id: &str) -> bool {
        self.docs.contains_key(doc_id)
    }

    pub fn edit_doc(&mut self, doc_id: &str, replace: &LineReplace) {
        let Some(doc) = self.docs.get_mut(doc_id) else {
            return;
        };
        self.versions += 1;
        doc.version = self.versions;
        let from = replace.from_line.max(1);
        let start = (from - 1).min(doc.lines.len());
        let end = (start + replace.old_line_count).min(doc.lines.len());
        doc.lines
            .splice(start..end, replace.new_lines.iter().cloned());
        doc.cache
            .splice(from, replace.old_line_count, replace.new_lines.len());
        if !doc.provisional.is_empty() {
            let prov = std::mem::take(&mut doc.provisional);
            doc.provisional =
                splice_line_map(prov, from, replace.old_line_count, replace.new_lines.len());
        }
    }

    pub fn close_doc(&mut self, doc_id: &str) {
        self.docs.remove(doc_id);
        self.live_refs.remove(doc_id);
        self.idle_docs.retain(|id| id != doc_id);
    }

    pub fn line_count(&self, doc_id: &str) -> usize {
        self.docs.get(doc_id).map_or(0, |d| d.lines.len())
    }

    pub fn doc_scope(&self, doc_id: &str) -> Option<&str> {
        self.docs.get(doc_id).map(|d| d.scope.as_str())
    }

    pub fn line_at(&self, doc_id: &str, n: usize) -> Option<&str> {
        let doc = self.docs.get(doc_id)?;
        if n < 1 {
            return None;
        }
        doc.lines.get(n - 1).map(String::as_str)
    }

    /// Advance the accurate sweep until EOF or `budget`; true when filled.
    pub fn fill_sync(&mut self, doc_id: &str, budget: Duration) -> bool {
        if !self.docs.contains_key(doc_id) {
            return false;
        }
        let deadline = Instant::now() + budget;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let slice = (deadline - now).min(Duration::from_millis(8));
            let p = self.fill_slice(doc_id, slice);
            if p.done {
                return true;
            }
            if p.total == 0 {
                return false;
            }
        }
        self.docs
            .get(doc_id)
            .is_some_and(|d| d.cache.filled() >= d.lines.len())
    }

    /// Scope tokens for 1-based line `n`; needs the accurate stack of `n - 1`.
    pub fn scopes_for_line(&self, doc_id: &str, n: usize) -> Option<Vec<ScopeToken>> {
        let doc = self.docs.get(doc_id)?;
        if n < 1 || n > doc.lines.len() {
            return None;
        }
        let grammar = self.grammar_cache.get(&doc.scope)?;
        let stack = if n == 1 {
            None
        } else {
            Some(doc.cache.stack_at(n - 1)?)
        };
        let r = grammar
            .tokenize_line(&line_text_for_tokenize(doc, n), stack.as_ref())
            .ok()?;
        Some(
            r.tokens
                .into_iter()
                .map(|t| ScopeToken {
                    start: t.start_index,
                    end: t.end_index,
                    scopes: t.scopes,
                })
                .collect(),
        )
    }

    /// Best tokens for a line: accurate, else provisional, else stale.
    pub fn tokens_for_line(&self, doc_id: &str, n: usize) -> Option<LineTokens> {
        let doc = self.docs.get(doc_id)?;
        if n <= doc.cache.filled() {
            return doc.cache.tokens_at(n);
        }
        doc.provisional
            .get(&n)
            .cloned()
            .or_else(|| doc.cache.tokens_at(n))
    }

    /// Every line in `from..=to` has tokens for its current text. Stale
    /// arrays past the watermark (kept for early-stop) do not count.
    pub fn has_fresh_tokens(&self, doc_id: &str, from_line: usize, to_line: usize) -> bool {
        let Some(doc) = self.docs.get(doc_id) else {
            return false;
        };
        let last = to_line.min(doc.lines.len());
        (from_line.max(1)..=last).all(|n| {
            (n <= doc.cache.filled() && doc.cache.tokens_at(n).is_some())
                || doc.provisional.contains_key(&n)
        })
    }

    /// The accurate sweep can reach `from_line` with at most `slack` extra lines.
    pub fn can_resume_accurately(&self, doc_id: &str, from_line: usize, slack: usize) -> bool {
        self.docs
            .get(doc_id)
            .is_some_and(|d| d.cache.filled() + 1 + slack >= from_line)
    }

    /// Synchronous viewport tokenization (VS Code's tokenizeHeuristically).
    pub fn tokenize_viewport(
        &mut self,
        doc_id: &str,
        from_line: usize,
        to_line: usize,
        budget: Duration,
    ) -> ViewportResult {
        let none = ViewportResult {
            mode: ViewportMode::None,
            tokenized: 0,
            complete: false,
        };
        let Some(doc) = self.docs.get_mut(doc_id) else {
            return none;
        };
        let Some(grammar) = self.grammar_cache.get(&doc.scope).cloned() else {
            return none;
        };
        if doc.failed {
            return none;
        }
        let last = to_line.min(doc.lines.len());
        let first = from_line.max(1);
        if first > last {
            return ViewportResult {
                mode: ViewportMode::Accurate,
                tokenized: 0,
                complete: true,
            };
        }
        let deadline = Instant::now() + budget;

        if doc.cache.filled() + 1 >= first {
            let start = first_unfresh_line(&doc.cache, first, last);
            let mut tokenized = 0;
            if start <= last {
                let lines = &doc.lines;
                let text = |n: usize| {
                    let t = lines.get(n - 1).map_or("", String::as_str);
                    if t.len() > MAX_TOKENIZE_LINE_LENGTH && utf16_len(t) > MAX_TOKENIZE_LINE_LENGTH
                    {
                        String::new()
                    } else {
                        t.to_string()
                    }
                };
                let r = tokenize_line_range(
                    text,
                    &mut doc.cache,
                    |text, stack| {
                        grammar
                            .tokenize_line2(text, stack)
                            .map(|r| (LineTokens::from(r.tokens), r.rule_stack))
                    },
                    start,
                    last,
                    true,
                    None,
                    Some(deadline),
                );
                match r {
                    Ok(r) => tokenized = r.tokenized,
                    Err(_) => {
                        doc.failed = true;
                        return none;
                    }
                }
            }
            let filled = doc.cache.filled();
            for n in first..=last.min(filled) {
                doc.provisional.remove(&n);
            }
            return ViewportResult {
                mode: ViewportMode::Accurate,
                tokenized,
                complete: doc.cache.has_tokens(first, last),
            };
        }

        // Cold jump: guessed start state; never advances the watermark.
        let Ok(mut stack) = guess_start_stack(doc, &grammar, first) else {
            doc.failed = true;
            return none;
        };
        let mut tokenized = 0;
        let mut n = first;
        while n <= last {
            if n > first && Instant::now() > deadline {
                break;
            }
            match grammar.tokenize_line2(&line_text_for_tokenize(doc, n), stack.as_ref()) {
                Ok(r) => {
                    stack = Some(r.rule_stack);
                    doc.provisional.insert(n, r.tokens.into());
                }
                Err(_) => {
                    doc.failed = true;
                    return none;
                }
            }
            tokenized += 1;
            n += 1;
        }
        ViewportResult {
            mode: ViewportMode::Guessed,
            tokenized,
            complete: n > last,
        }
    }

    /// Advance the accurate sweep for about `budget`, correcting provisional
    /// guesses on the way and reporting the line range whose tokens changed.
    pub fn fill_slice(&mut self, doc_id: &str, budget: Duration) -> FillProgress {
        let Some(doc) = self.docs.get(doc_id) else {
            return FillProgress {
                filled: 0,
                total: 0,
                changed_from: None,
                changed_to: 0,
                done: true,
            };
        };
        if doc.failed || !self.grammar_cache.contains_key(&doc.scope) {
            return FillProgress {
                filled: doc.cache.filled(),
                total: doc.lines.len(),
                changed_from: None,
                changed_to: 0,
                done: false,
            };
        }
        let deadline = Instant::now() + budget;
        let mut changed_from: Option<usize> = None;
        let mut changed_to = 0;
        while let Some(job) = self.fill_job(doc_id, SLICE_JOB_LINES) {
            let chunk = job.run(deadline.saturating_duration_since(Instant::now()));
            let p = self.apply_fill(doc_id, chunk);
            if let Some(f) = p.changed_from {
                changed_from.get_or_insert(f);
                changed_to = changed_to.max(p.changed_to);
            }
            if p.done || Instant::now() > deadline {
                break;
            }
        }
        let doc = &self.docs[doc_id];
        FillProgress {
            filled: doc.cache.filled(),
            total: doc.lines.len(),
            changed_from,
            changed_to,
            done: doc.cache.filled() >= doc.lines.len(),
        }
    }

    /// Snapshot up to `max_lines` of the accurate sweep for [`FillJob::run`].
    /// `None` when the doc is filled, failed or its grammar is not compiled.
    pub fn fill_job(&self, doc_id: &str, max_lines: usize) -> Option<FillJob> {
        let doc = self.docs.get(doc_id)?;
        let total = doc.lines.len();
        let filled = doc.cache.filled();
        if doc.failed || filled >= total {
            return None;
        }
        let grammar = self.grammar_cache.get(&doc.scope)?.clone();
        let (from, stack) = doc.cache.resume_at(filled + 1);
        let to = total.min(from + max_lines.max(1) - 1);
        Some(FillJob {
            grammar,
            version: doc.version,
            filled,
            from,
            stack,
            lines: (from..=to)
                .map(|n| (line_text_for_tokenize(doc, n), doc.cache.stack_at(n)))
                .collect(),
        })
    }

    /// Apply a [`FillJob`]'s result, unless the doc was edited or the sweep
    /// moved since the snapshot.
    pub fn apply_fill(&mut self, doc_id: &str, chunk: FillChunk) -> FillProgress {
        let Some(doc) = self.docs.get_mut(doc_id) else {
            return FillProgress {
                filled: 0,
                total: 0,
                changed_from: None,
                changed_to: 0,
                done: true,
            };
        };
        let mut changed_from: Option<usize> = None;
        let mut changed_to = 0;
        if doc.version == chunk.version && doc.cache.filled() == chunk.filled {
            let mut line = chunk.from;
            for (tokens, stack) in chunk.lines {
                let shown = doc
                    .provisional
                    .remove(&line)
                    .or_else(|| doc.cache.tokens_at(line));
                let tokens: LineTokens = tokens.into();
                if shown.as_deref() != Some(&*tokens) {
                    changed_from.get_or_insert(line);
                    changed_to = line;
                }
                doc.cache.remember(line, stack, Some(tokens));
                line += 1;
            }
            if chunk.restabilized {
                doc.cache.mark_stable(line - 1);
            }
            doc.failed |= chunk.failed;
        }
        let total = doc.lines.len();
        FillProgress {
            filled: doc.cache.filled(),
            total,
            changed_from,
            changed_to,
            done: doc.cache.filled() >= total,
        }
    }

    pub fn filled_through(&self, doc_id: &str) -> usize {
        self.docs.get(doc_id).map_or(0, |d| d.cache.filled())
    }

    pub fn provisional_count(&self, doc_id: &str) -> usize {
        self.docs.get(doc_id).map_or(0, |d| d.provisional.len())
    }

    /// Doc ids currently resident (tests, diagnostics).
    pub fn doc_ids(&self) -> HashSet<String> {
        self.docs.keys().cloned().collect()
    }

    fn drop_registry(&mut self) {
        self.registry = None;
        self.grammar_cache.clear();
    }

    fn ensure_registry(&mut self) -> &mut Registry {
        let theme = self.theme.as_ref();
        self.registry.get_or_insert_with(|| Registry::new(theme))
    }
}

/// VS Code's findLikelyRelevantLines: walk back over strictly less-indented
/// lines (the enclosing scopes), stop at column 1 or a known end state, then
/// tokenize those lines to synthesize the state entering `line_number`.
fn guess_start_stack(
    doc: &TmDoc,
    grammar: &Grammar,
    line_number: usize,
) -> Result<Option<StateStack>, crate::grammar::TokenizeError> {
    let mut relevant: Vec<&str> = Vec::new();
    let mut column =
        first_non_whitespace_column(doc.lines.get(line_number - 1).map_or("", String::as_str));
    let mut initial: Option<StateStack> = None;
    let scan_floor = line_number.saturating_sub(MAX_GUESS_SCAN_LINES).max(1);
    let mut i = line_number - 1;
    while column > 1 && i >= scan_floor {
        let text = doc.lines.get(i - 1).map_or("", String::as_str);
        let c = first_non_whitespace_column(text);
        if c != 0 && c < column {
            relevant.push(text);
            column = c;
            let known = if i <= doc.cache.filled() {
                doc.cache.stack_at(i)
            } else {
                None
            };
            if let Some(k) = known {
                initial = Some(k);
                break;
            }
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    relevant.reverse();
    let mut stack = initial;
    for text in relevant {
        stack = Some(grammar.tokenize_line2(text, stack.as_ref())?.rule_stack);
    }
    Ok(stack)
}
