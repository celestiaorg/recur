//! TextMate highlighting for an editor: synchronous viewport
//! tokenization on edits and first paint, a debounced pass after cold
//! scrolls, a fill to the end of the document that tokenizes on the
//! background executor, and per-line paint spans rebuilt from the engine's
//! token cache.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use recur_textmate::{
    ChangedRange, FillChunk, FillJob, FillSubscription, LineDoc, TmCatalog, TmEngine, TmFill,
    TokenPaint, line_replace_from_changes, paint_for_meta, tm_doc_fingerprint,
};

use crate::state::{EditorState, Transaction};
use crate::text::Text;

/// The app's TextMate runtime: the catalog (grammars, themes, the shared
/// engine and colour map) and the idle fill scheduler.
#[derive(Default)]
pub struct TextMate {
    pub catalog: TmCatalog,
    pub fill: TmFill,
}

pub type SharedTextMate = Rc<RefCell<TextMate>>;

impl TextMate {
    /// Make `parsed` the active editor theme as `editorThemeExtension` and
    /// `ensureTextMateBootstrap` do: the tokenizer theme, the
    /// `editor.background` and the root `source` / `text` fills token
    /// backgrounds are compared against.
    pub fn activate_theme(&mut self, parsed: Rc<recur_textmate::ParsedTmTheme>) {
        let bg = parsed
            .colors
            .get("editor.background")
            .cloned()
            .unwrap_or_else(|| crate::theme::EDITOR_BACKGROUND_FALLBACK.to_string());
        self.catalog.set_active_editor_background(&bg);
        self.catalog.set_active_root_backgrounds(
            parsed.root_backgrounds.source.as_deref(),
            parsed.root_backgrounds.text.as_deref(),
        );
        self.catalog.set_active_theme(parsed);
    }
}

/// Debounced cold-scroll tokenize pass.
pub const SCROLL_PASS_MS: u64 = 50;
/// Small documents always tokenize inline.
pub const CHEAP_DOC_LINES: usize = 1500;
/// An accurate resume within this many lines counts as cheap.
pub const INLINE_RESUME_SLACK: usize = 120;
/// No TextMate at all above this many lines.
pub const MAX_TM_LINES: usize = 200_000;
/// Cap on the lines one build covers.
pub const MAX_BUILD_LINES: usize = 1500;
/// `tokenizeViewport` budget.
pub const VIEWPORT_BUDGET: Duration = Duration::from_millis(25);
/// Delay before the fill starts, and between retries while it cannot run.
pub const FILL_IDLE_DELAY_MS: u64 = 32;
/// Lines and time one background fill step covers before the UI applies it.
pub const FILL_CHUNK_LINES: usize = 1000;
pub const FILL_CHUNK_BUDGET: Duration = Duration::from_millis(20);

/// A painted token span in document offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenSpan {
    pub from: usize,
    pub to: usize,
    pub paint: TokenPaint,
}

struct TextLines<'a>(&'a Text);

impl LineDoc for TextLines<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn line_count(&self) -> usize {
        self.0.lines()
    }

    fn line_number_at(&self, offset: usize) -> usize {
        self.0.line_at(offset).number
    }

    fn line_text(&self, n: usize) -> String {
        self.0.line_text(n)
    }
}

fn fingerprint(doc: &Text) -> recur_textmate::TmDocFingerprint {
    tm_doc_fingerprint(|n| doc.line_text(n), doc.lines(), doc.len())
}

/// The line window to decorate (`viewportWindow`): the rendered lines,
/// capped around the visible ones.
pub fn viewport_window(rendered: (usize, usize), visible_from: usize) -> (usize, usize) {
    let (from, to) = rendered;
    if to - from < MAX_BUILD_LINES {
        return (from, to);
    }
    let start = from.max(
        visible_from
            .saturating_sub(MAX_BUILD_LINES >> 2)
            .min(to + 1 - MAX_BUILD_LINES),
    );
    (start, to.min(start + MAX_BUILD_LINES - 1))
}

/// Selection ranges that suppress token washes.
fn selection_ranges(state: &EditorState) -> Vec<(usize, usize)> {
    state
        .selection
        .ranges
        .iter()
        .filter(|r| !r.is_empty())
        .map(|r| (r.from, r.to))
        .collect()
}

/// Where the paint colours come from: the shared active theme (code
/// editors) or a chrome-paired engine (settings script editors).
#[derive(Clone)]
pub enum PaintSource {
    Shared,
    ChromePaired { theme_id: String },
}

/// What the view should do after a highlighter step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighlightNeeds {
    /// Repaint now.
    pub redraw: bool,
    /// Run the scroll pass at this time (ms, `clock::now_ms`).
    pub scroll_pass_at: Option<u64>,
    /// Keep the idle fill running.
    pub fill: bool,
}

#[derive(Default)]
struct FillEvents {
    changed: Option<(usize, usize)>,
    done: bool,
}

/// One editor's TextMate highlighter (the `tmPlugin` view plugin).
pub struct Highlighter {
    tm: SharedTextMate,
    pub scope: String,
    doc_id: String,
    source: PaintSource,
    opened: bool,
    map_epoch: u64,
    fill_sub: Option<FillSubscription>,
    fill_events: Rc<RefCell<FillEvents>>,
    scroll_pass_at: Option<u64>,
    prev_sel_nonempty: bool,
    built: Option<(usize, usize)>,
    pub paint_backgrounds: bool,
    lines: HashMap<usize, Vec<TokenSpan>>,
}

impl Highlighter {
    pub fn new(
        tm: SharedTextMate,
        scope: &str,
        doc_id: &str,
        source: PaintSource,
        paint_backgrounds: bool,
    ) -> Highlighter {
        let map_epoch = tm.borrow().catalog.color_map_epoch();
        Highlighter {
            tm,
            scope: scope.to_string(),
            doc_id: doc_id.to_string(),
            source,
            opened: false,
            map_epoch,
            fill_sub: None,
            fill_events: Rc::new(RefCell::new(FillEvents::default())),
            scroll_pass_at: None,
            prev_sel_nonempty: false,
            built: None,
            paint_backgrounds,
            lines: HashMap::new(),
        }
    }

    pub fn doc_id(&self) -> &str {
        &self.doc_id
    }

    fn with_engine<R>(&self, f: impl FnOnce(&mut TmEngine) -> R) -> Option<R> {
        let mut tm = self.tm.borrow_mut();
        match &self.source {
            PaintSource::Shared => Some(f(tm.catalog.engine_mut())),
            PaintSource::ChromePaired { theme_id } => {
                if tm.catalog.chrome_paired_theme_id() != Some(theme_id.as_str()) {
                    return None;
                }
                tm.catalog.ensure_chrome_paired_engine(theme_id).map(f)
            }
        }
    }

    /// Spans for a line in the current build (empty when not built).
    pub fn spans(&self, line: usize) -> &[TokenSpan] {
        self.lines.get(&line).map_or(&[], Vec::as_slice)
    }

    /// `mountOrLoad`: compile the grammar if needed and paint.
    pub fn mount(&mut self, state: &EditorState, win: (usize, usize)) -> HighlightNeeds {
        if self.opened {
            return HighlightNeeds::default();
        }
        let has = {
            let mut tm = self.tm.borrow_mut();
            tm.catalog.has_grammar(&self.scope) || tm.catalog.load_grammar(&self.scope)
        };
        if !has || state.doc.lines() > MAX_TM_LINES {
            return HighlightNeeds::default();
        }
        let scope = self.scope.clone();
        let doc_id = self.doc_id.clone();
        let fp = fingerprint(&state.doc);
        let opened = self.with_engine(|e| {
            if !e.reopen_doc(&doc_id, &scope, fp) {
                e.open_doc(&doc_id, &scope, &state.doc.to_string());
            }
        });
        if opened.is_none() {
            return HighlightNeeds::default();
        }
        self.opened = true;
        self.paint_viewport(state, win, true)
    }

    /// The theme changed: drop everything and paint as if just opened.
    fn remount(&mut self, state: &EditorState, win: (usize, usize)) -> HighlightNeeds {
        let doc_id = self.doc_id.clone();
        if self.opened {
            self.with_engine(|e| {
                if e.has_doc(&doc_id) {
                    e.release_doc(&doc_id);
                }
            });
        }
        self.stop_fill();
        self.opened = false;
        self.lines.clear();
        self.built = None;
        self.prev_sel_nonempty = false;
        let mut needs = self.mount(state, win);
        needs.redraw = true;
        needs
    }

    /// Point at another document (`switchDoc`).
    pub fn switch_doc(
        &mut self,
        doc_id: &str,
        scope: &str,
        state: &EditorState,
        win: (usize, usize),
    ) -> HighlightNeeds {
        let old = self.doc_id.clone();
        if self.opened {
            self.with_engine(|e| e.release_doc(&old));
        }
        self.stop_fill();
        self.doc_id = doc_id.to_string();
        self.scope = scope.to_string();
        self.opened = false;
        self.lines.clear();
        self.built = None;
        self.mount(state, win)
    }

    /// `update(ViewUpdate)` for one transaction.
    pub fn update(
        &mut self,
        tr: &Transaction,
        state: &EditorState,
        win: (usize, usize),
    ) -> HighlightNeeds {
        if tr.doc_changed() && self.opened {
            let mut ranges = Vec::new();
            tr.changes
                .iter_changed_ranges(false, |from_a, to_a, from_b, to_b| {
                    ranges.push(ChangedRange {
                        from_a,
                        to_a,
                        from_b,
                        to_b,
                    });
                });
            let old = TextLines(&tr.start_state.doc);
            let new = TextLines(&state.doc);
            if let Some(replace) = line_replace_from_changes(&old, &new, &ranges) {
                let doc_id = self.doc_id.clone();
                self.with_engine(|e| e.edit_doc(&doc_id, &replace));
            }
        }
        let epoch = self.tm.borrow().catalog.color_map_epoch();
        if epoch != self.map_epoch {
            self.map_epoch = epoch;
            return self.remount(state, win);
        }
        if !self.opened {
            return HighlightNeeds::default();
        }
        let sel_nonempty =
            self.paint_backgrounds && state.selection.ranges.iter().any(|r| !r.is_empty());
        let selection_changed = tr.selection.is_some() && (sel_nonempty || self.prev_sel_nonempty);
        if selection_changed {
            self.prev_sel_nonempty = sel_nonempty;
        }
        if tr.doc_changed() || selection_changed {
            return self.paint_viewport(state, win, true);
        }
        if self.built != Some(win) {
            return self.paint_viewport(state, win, false);
        }
        HighlightNeeds::default()
    }

    /// The rendered window moved (scroll / resize) without a transaction.
    pub fn viewport_changed(&mut self, state: &EditorState, win: (usize, usize)) -> HighlightNeeds {
        let epoch = self.tm.borrow().catalog.color_map_epoch();
        if epoch != self.map_epoch {
            self.map_epoch = epoch;
            return self.remount(state, win);
        }
        if !self.opened {
            return self.mount(state, win);
        }
        if self.built != Some(win) {
            return self.paint_viewport(state, win, false);
        }
        HighlightNeeds::default()
    }

    /// Rebuild the window's spans, tokenizing first when allowed or cheap.
    pub fn paint_viewport(
        &mut self,
        state: &EditorState,
        win: (usize, usize),
        may_tokenize: bool,
    ) -> HighlightNeeds {
        let mut needs = HighlightNeeds {
            redraw: true,
            ..Default::default()
        };
        if !self.opened {
            return needs;
        }
        let doc_id = self.doc_id.clone();
        let has_doc = self.with_engine(|e| e.has_doc(&doc_id)).unwrap_or(false);
        if !has_doc {
            return self.remount(state, win);
        }
        let (from, to) = win;
        let lines = state.doc.lines();
        let fresh = self
            .with_engine(|e| e.has_fresh_tokens(&doc_id, from, to))
            .unwrap_or(true);
        if !fresh {
            let inline = may_tokenize
                || lines <= CHEAP_DOC_LINES
                || self
                    .with_engine(|e| e.can_resume_accurately(&doc_id, from, INLINE_RESUME_SLACK))
                    .unwrap_or(false);
            if inline {
                self.with_engine(|e| e.tokenize_viewport(&doc_id, from, to, VIEWPORT_BUDGET));
            } else {
                let at = crate::clock::now_ms() + SCROLL_PASS_MS;
                self.scroll_pass_at = Some(at);
                needs.scroll_pass_at = Some(at);
            }
        }
        self.build(state, win);
        needs.fill = self.ensure_fill(lines);
        needs
    }

    fn colors(&self) -> (Vec<String>, Vec<String>, String) {
        let tm = self.tm.borrow();
        let cat = &tm.catalog;
        match &self.source {
            PaintSource::Shared => {
                let mut bgs = vec![cat.active_editor_background().to_string()];
                bgs.extend(cat.active_root_backgrounds());
                (
                    cat.color_map().to_vec(),
                    bgs,
                    cat.active_pane_background(&self.scope),
                )
            }
            PaintSource::ChromePaired { theme_id } => {
                let parsed = cat.parsed_theme(theme_id);
                let (bg, source, text) = parsed.as_ref().map_or(
                    (
                        crate::theme::EDITOR_BACKGROUND_FALLBACK.to_string(),
                        None,
                        None,
                    ),
                    |p| {
                        (
                            p.colors
                                .get("editor.background")
                                .cloned()
                                .unwrap_or_else(|| crate::theme::EDITOR_BACKGROUND_FALLBACK.into()),
                            p.root_backgrounds.source.clone().filter(|s| !s.is_empty()),
                            p.root_backgrounds.text.clone().filter(|s| !s.is_empty()),
                        )
                    },
                );
                let mut bgs = vec![bg.clone()];
                bgs.extend(source.clone());
                bgs.extend(text.clone());
                let pane = recur_textmate::pane_background(
                    &self.scope,
                    &bg,
                    source.as_deref(),
                    text.as_deref(),
                );
                drop(tm);
                let map = self.with_engine(|e| e.color_map()).unwrap_or_default();
                (map, bgs, pane)
            }
        }
    }

    fn build(&mut self, state: &EditorState, win: (usize, usize)) {
        let (color_map, bgs, pane_bg) = self.colors();
        let sel = if self.paint_backgrounds {
            selection_ranges(state)
        } else {
            Vec::new()
        };
        let mut memo: HashMap<(u32, bool), TokenPaint> = HashMap::new();
        let mut out: HashMap<usize, Vec<TokenSpan>> = HashMap::new();
        let doc_id = self.doc_id.clone();
        let paint_backgrounds = self.paint_backgrounds;
        let last = win.1.min(state.doc.lines());
        self.with_engine(|e| {
            for n in win.0.max(1)..=last {
                let Some(tokens) = e.tokens_for_line(&doc_id, n) else {
                    continue;
                };
                let line = state.doc.line(n);
                let len = line.len();
                let mut spans = Vec::new();
                let mut i = 0;
                while i + 1 < tokens.len() {
                    let start = (tokens[i] as usize).min(len);
                    let meta = tokens[i + 1];
                    let end = if i + 2 < tokens.len() {
                        (tokens[i + 2] as usize).min(len)
                    } else {
                        len
                    };
                    i += 2;
                    if end <= start {
                        continue;
                    }
                    let (from, to) = (line.from + start, line.from + end);
                    let paint_bg =
                        paint_backgrounds && !sel.iter().any(|&(f, t)| from < t && to > f);
                    let paint = *memo.entry((meta, paint_bg)).or_insert_with(|| {
                        paint_for_meta(meta, paint_bg, &color_map, &bgs, &pane_bg)
                    });
                    spans.push(TokenSpan { from, to, paint });
                }
                out.insert(n, spans);
            }
        });
        self.lines = out;
        self.built = Some(win);
    }

    /// The debounced scroll pass is due.
    pub fn scroll_pass(
        &mut self,
        state: &EditorState,
        win: (usize, usize),
        now: u64,
    ) -> HighlightNeeds {
        match self.scroll_pass_at {
            Some(at) if at <= now => {
                self.scroll_pass_at = None;
                self.paint_viewport(state, win, true)
            }
            _ => HighlightNeeds::default(),
        }
    }

    fn ensure_fill(&mut self, lines: usize) -> bool {
        if self.fill_sub.is_some() {
            return true;
        }
        let doc_id = self.doc_id.clone();
        let filled = self
            .with_engine(|e| e.filled_through(&doc_id))
            .unwrap_or(lines);
        if filled >= lines {
            return false;
        }
        let events = self.fill_events.clone();
        let sub = self.tm.borrow_mut().fill.ensure(
            &doc_id,
            Box::new(move |changed, done| {
                let mut ev = events.borrow_mut();
                if let Some((f, t)) = changed {
                    ev.changed = Some(ev.changed.map_or((f, t), |(a, b)| (a.min(f), b.max(t))));
                }
                ev.done |= done;
            }),
        );
        self.fill_sub = Some(sub);
        true
    }

    fn stop_fill(&mut self) {
        if let Some(sub) = self.fill_sub.take() {
            let doc_id = self.doc_id.clone();
            self.tm.borrow_mut().fill.unsubscribe(&doc_id, sub);
        }
        *self.fill_events.borrow_mut() = FillEvents::default();
    }

    /// The next stretch of the fill, to run off the UI thread. `None` while
    /// nothing can run (filled, grammar not compiled).
    pub fn fill_job(&self) -> Option<FillJob> {
        self.fill_sub?;
        self.with_engine(|e| e.fill_job(&self.doc_id, FILL_CHUNK_LINES))
            .flatten()
    }

    /// Apply a finished fill step (`None` when no job could run); repaints
    /// when visible lines changed or the fill finished.
    pub fn apply_fill(
        &mut self,
        chunk: Option<FillChunk>,
        state: &EditorState,
        win: (usize, usize),
    ) -> HighlightNeeds {
        if self.fill_sub.is_none() {
            return HighlightNeeds::default();
        }
        let more = {
            let mut guard = self.tm.borrow_mut();
            let TextMate { catalog, fill } = &mut *guard;
            let engine = match &self.source {
                PaintSource::Shared => Some(catalog.engine_mut()),
                PaintSource::ChromePaired { theme_id } => {
                    catalog.ensure_chrome_paired_engine(theme_id)
                }
            };
            match engine {
                Some(e) => fill.apply(e, &self.doc_id, chunk),
                None => false,
            }
        };
        let events = std::mem::take(&mut *self.fill_events.borrow_mut());
        if events.done || !more {
            self.fill_sub = None;
        }
        let visible = events
            .changed
            .is_some_and(|(f, t)| t >= win.0 && f <= win.1);
        let mut needs = HighlightNeeds {
            fill: self.fill_sub.is_some(),
            ..Default::default()
        };
        if visible || events.done {
            let painted = self.paint_viewport(state, win, false);
            needs.redraw = true;
            needs.scroll_pass_at = painted.scroll_pass_at;
            needs.fill |= painted.fill;
        }
        needs
    }

    /// The view is going away (`destroy`).
    pub fn destroy(&mut self) {
        self.stop_fill();
        if self.opened {
            let doc_id = self.doc_id.clone();
            self.with_engine(|e| e.release_doc(&doc_id));
            self.opened = false;
        }
    }
}

impl Drop for Highlighter {
    fn drop(&mut self) {
        if let Ok(mut tm) = self.tm.try_borrow_mut() {
            if let Some(sub) = self.fill_sub.take() {
                tm.fill.unsubscribe(&self.doc_id, sub);
            }
            if self.opened && matches!(self.source, PaintSource::Shared) {
                tm.catalog.engine_mut().release_doc(&self.doc_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::change::Change;
    use crate::state::{StateConfig, TransactionSpec};

    fn bundles() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles")
    }

    fn shared() -> Option<SharedTextMate> {
        let grammar =
            std::fs::read_to_string(bundles().join("Languages/syntaxes/go.tmLanguage.json"))
                .ok()?;
        let theme =
            std::fs::read_to_string(bundles().join("Recur/themes/charcoal.tmTheme.json")).ok();
        let tm = Rc::new(RefCell::new(TextMate::default()));
        {
            let mut t = tm.borrow_mut();
            t.catalog.add_grammar(
                recur_textmate::GrammarInfo {
                    scope_name: "source.go".into(),
                    file_types: Some(vec!["go".into()]),
                    ..Default::default()
                },
                Rc::new(move || Some(grammar.clone())),
            );
            if let Some(theme) = theme
                && let Ok(parsed) = recur_textmate::parse_theme_json(&theme, "charcoal")
            {
                t.catalog.set_active_theme(Rc::new(parsed));
            }
        }
        Some(tm)
    }

    #[test]
    fn paints_and_follows_edits() {
        let Some(tm) = shared() else { return };
        let s = EditorState::create("package main\n\nfunc main() {}\n", StateConfig::default());
        let mut h = Highlighter::new(tm, "source.go", "t1", PaintSource::Shared, true);
        let needs = h.mount(&s, (1, 4));
        assert!(needs.redraw);
        assert!(!h.spans(1).is_empty());
        let first = h.spans(1)[0];
        assert_eq!((first.from, first.to), (0, 7));
        let tr = s.update(TransactionSpec::default().changes(Change::insert(0, "// x\n")));
        h.update(&tr, tr.state(), (1, 5));
        assert_eq!(h.spans(2)[0].from, 5);
        h.destroy();
    }
}
