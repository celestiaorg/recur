//! The editor entity: owns the [`EditorState`], dispatches transactions
//! and runs everything CodeMirror's view plugins do after an update
//! (layout, highlighting, completion, panels, dirty tracking, scrolling,
//! caret blink). Mirrors `CodeEditor.svelte` and `MergeEditor.svelte`.

use recur_ui::guard::Guarded;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    App, Context, EventEmitter, FocusHandle, Focusable, Font, FontFallbacks, Pixels, SharedString,
    Task, TextRun, Window, WindowTextSystem, black, font, px,
};
use recur_textmate::{CompiledFoldMarkers, ParsedTmTheme, compile_fold_markers, language_data};

use crate::autocomplete::{
    AnyWordSource, CompletionConfig, CompletionDriver, DriverStep, SnippetCompletionSource,
    SourceRef,
};
use crate::bindings::{Cmd, code_editor_keymap, merge_editor_keymap};
use crate::clock::now_ms;
use crate::display::{BlockWidget, Layout, LayoutConfig, LayoutView, TextMeasure};
use crate::highlight::{
    FILL_IDLE_BUDGET, FILL_IDLE_DELAY_MS, Highlighter, PaintSource, SharedTextMate, viewport_window,
};
use crate::history_cache::EDITOR_HISTORY_MIN_DEPTH;
use crate::keymap::{Keymap, Platform};
use crate::merge::{MergeState, RevertHover};
use crate::prefs::{EditorPrefs, language_id_for_path};
use crate::selection::EditorSelection;
use crate::snippet::SnippetProvider;
use crate::state::{
    Annotations, EditorState, Effect, LanguageConfig, ScrollStrategy, ScrollTarget, StateConfig,
    Transaction, TransactionSpec,
};
use crate::text::Text;
use crate::theme::{EditorMetrics, EditorTheme, MiniChrome};
use crate::view_state::{FileViewState, is_buffer_dirty};

/// Debounce for `DocChanged` (`EMIT_DEBOUNCE_MS`).
pub const EMIT_DEBOUNCE_MS: u64 = 120;

/// Events the host listens to (the Svelte component's callbacks).
#[derive(Clone, Debug, PartialEq)]
pub enum EditorEvent {
    /// Full text, debounced (`onDocChange`). Saving must read
    /// [`EditorView::get_doc`] instead.
    DocChanged(String),
    /// Synchronous dirty flag (`onDirtyChange`).
    DirtyChanged(bool),
    /// Mod-s (`onSave`).
    Save,
    /// Gutter line number click: copy `path:line` (`onCopyLine`).
    CopyLine(usize),
    /// Caret moved: 1-based line and column (`onCursor`).
    Cursor { line: usize, col: usize },
    /// Word to look up when LSP cannot (`onGotoDef`).
    GotoDefinition(String),
    /// The find panel opened or closed (`onSearchOpenChange`).
    SearchOpenChanged(bool),
    /// Findings gutter click (`onOpenFinding`).
    OpenFinding(String),
    /// Several snippets share a tab trigger: show a picker, then call
    /// [`EditorView::apply_snippet_at`] (`openKeybindingMenu`).
    PickSnippet {
        snippets: Vec<crate::snippet::RegisteredSnippet>,
        from: usize,
        to: usize,
    },
    /// Screen reader announcement (`EditorView.announce`).
    Announce(String),
    /// A toast for the app (`pushToast`).
    Toast { error: bool, text: String },
}

/// Code view, or the Diff view's unified merge against `original`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorKind {
    Code,
    Merge { original: String },
}

/// What the host provides when creating an editor.
#[derive(Clone)]
pub struct EditorOptions {
    pub kind: EditorKind,
    pub path: String,
    pub doc: String,
    pub read_only: bool,
    pub prefs: EditorPrefs,
    /// The editor's TextMate theme; `None` uses CodeMirror's bare theme.
    pub theme: Option<Rc<ParsedTmTheme>>,
    /// Theme kind when no parsed theme is available.
    pub dark_fallback: bool,
    pub textmate: Option<SharedTextMate>,
    /// Pane id for the TextMate document key (`tmDocIdFor`).
    pub pane: String,
    pub snippets: Option<Rc<dyn SnippetProvider>>,
    /// On-disk text when it differs from `doc` (mounts dirty).
    pub saved_doc: Option<String>,
    pub view_state: Option<FileViewState>,
    /// Observatory pane is lit (focused pane signal).
    pub lit: bool,
    /// The per-file undo cache scope (`sessionKey` / `historyScope`); empty keeps undo
    /// live-only.
    pub history_scope: String,
    /// `MiniScriptEditor`'s compact chrome instead of the code editor's.
    pub mini: Option<MiniChrome>,
}

impl EditorOptions {
    pub fn new(path: &str, doc: &str) -> EditorOptions {
        EditorOptions {
            kind: EditorKind::Code,
            path: path.to_string(),
            doc: doc.to_string(),
            read_only: false,
            prefs: EditorPrefs::default(),
            theme: None,
            dark_fallback: true,
            textmate: None,
            pane: "main".into(),
            snippets: None,
            saved_doc: None,
            view_state: None,
            lit: true,
            history_scope: String::new(),
            mini: None,
        }
    }
}

/// Widths of text in a GPUI font.
pub(crate) struct GpuiMeasure<'a> {
    pub ts: &'a WindowTextSystem,
    pub font: Font,
    pub size: Pixels,
}

impl TextMeasure for GpuiMeasure<'_> {
    fn char_xs(&self, text: &str) -> Vec<f32> {
        if text.is_empty() {
            return vec![0.0];
        }
        let run = TextRun {
            len: text.len(),
            font: self.font.clone(),
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = self.ts.shape_line(
            SharedString::from(text.to_string()),
            self.size,
            &[run],
            None,
        );
        let mut glyphs: Vec<(usize, f32)> = shaped
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter().map(|g| (g.index, f32::from(g.position.x))))
            .collect();
        glyphs.sort_by_key(|g| g.0);
        let width = f32::from(shaped.width);
        let mut xs = Vec::with_capacity(text.len() + 1);
        let mut gi = 0;
        for (b, _) in text.char_indices() {
            while gi < glyphs.len() && glyphs[gi].0 < b {
                gi += 1;
            }
            xs.push(glyphs.get(gi).map_or(width, |g| g.1));
        }
        xs.push(width);
        for i in 1..xs.len() {
            if xs[i] < xs[i - 1] {
                xs[i] = xs[i - 1];
            }
        }
        xs
    }
}

/// The editor's colors: the TextMate theme's chrome, or without one CodeMirror's bare theme
/// (the mini editor's `fallbackChrome()`).
pub(crate) fn resolve_theme(
    parsed: Option<&ParsedTmTheme>,
    dark_fallback: bool,
    mini: bool,
    app: recur_ui::ThemeId,
) -> EditorTheme {
    match parsed {
        None if mini => EditorTheme::mini_fallback(app),
        _ => EditorTheme::resolve(parsed, dark_fallback, app),
    }
}

/// The editor font from prefs.
pub(crate) fn editor_font(metrics: &EditorMetrics) -> Font {
    let mut f = font(SharedString::from(metrics.font_families[0].clone()));
    f.fallbacks = Some(FontFallbacks::from_fonts(
        metrics.font_families[1..].to_vec(),
    ));
    f
}

/// A parked tab (`TabSnapshot`): its state, scroll anchor and dirty bits.
#[derive(Clone)]
pub struct TabSnapshot {
    pub state: EditorState,
    pub scroll: Option<ScrollTarget>,
    pub dirty: bool,
    pub clean: Option<Text>,
}

/// Geometry from the last prepaint, for hit testing.
#[derive(Clone, Debug, Default)]
pub(crate) struct Frame {
    pub bounds: gpui_kit::Bounds<Pixels>,
    /// Screen x of the content box's left edge (after gutters), before
    /// horizontal scroll.
    pub content_left: f32,
    /// Screen y of the scroller top.
    pub scroller_top: f32,
    pub scroller_height: f32,
    pub scroller_width: f32,
    pub gutter_width: f32,
    pub gutters: Vec<crate::element::GutterColumn>,
    pub max_scroll_top: f32,
    pub max_scroll_left: f32,
    pub visible_lines: (usize, usize),
    pub rendered_lines: (usize, usize),
    pub content_width: f32,
}

pub struct EditorView {
    pub(crate) focus: FocusHandle,
    pub(crate) kind: EditorKind,
    pub(crate) path: String,
    pub(crate) pane: String,
    pub(crate) state: EditorState,
    pub(crate) prefs: EditorPrefs,
    pub(crate) parsed_theme: Option<Rc<ParsedTmTheme>>,
    pub(crate) dark_fallback: bool,
    pub(crate) theme: EditorTheme,
    pub(crate) metrics: EditorMetrics,
    pub(crate) font: Font,
    pub(crate) layout: RefCell<Layout>,
    pub(crate) tm: Option<SharedTextMate>,
    pub(crate) highlighter: Option<Highlighter>,
    pub(crate) scope: String,
    pub(crate) keymap: Keymap<Cmd>,
    pub(crate) platform: Platform,
    pub(crate) snippets: Option<Rc<dyn SnippetProvider>>,
    pub(crate) lsp: Option<crate::lsp::LspAttachment>,
    pub(crate) history_scope: String,
    pub(crate) mini: Option<MiniChrome>,
    /// The scroll a state rebuilt from the undo cache was captured at.
    pub(crate) restored_scroll: Option<(f32, f32)>,
    pub(crate) read_only: bool,
    pub(crate) lit: bool,
    pub(crate) scroll_top: f32,
    pub(crate) scroll_left: f32,
    pub(crate) pending_scroll: Option<ScrollTarget>,
    pub(crate) frame: Option<Frame>,
    /// When the caret's `cm-blink` animation (re)started.
    pub(crate) blink_start: std::time::Instant,
    pub(crate) blink_epoch: u64,
    pub(crate) blink_task: Option<Task<()>>,
    /// The editor holds focus; only then does the caret blink (an unfocused caret is drawn
    /// steady, so a blink would only redraw the editor and every view above it).
    pub(crate) has_focus: bool,
    pub(crate) completion: CompletionDriver,
    pub(crate) completion_timer: Option<Task<()>>,
    pub(crate) highlight_timer: Option<Task<()>>,
    pub(crate) fill_task: Option<Task<()>>,
    pub(crate) search: Option<crate::panels::SearchUi>,
    pub(crate) dialogs: Vec<crate::panels::Dialog>,
    pub(crate) dialog_seq: u64,
    pub(crate) rename_prompt: Option<crate::panels::RenamePrompt>,
    /// `foldGutter` has built its markers since the fold service arrived.
    /// A grammar loading at mount reaches CodeMirror by compartment
    /// reconfigure, which rebuilds no markers, so they appear on the first
    /// edit, fold or viewport change; an already loaded grammar shows them
    /// at mount.
    pub(crate) fold_markers_live: bool,
    /// The panel control under the pointer (hover styles on card buttons).
    pub(crate) hovered_control: Option<&'static str>,
    /// The gutter / chip `title` under the pointer and its box.
    pub(crate) title_seen: Option<(String, gpui_kit::Bounds<Pixels>)>,
    pub(crate) revert: RevertHover,
    pub(crate) revert_timer: Option<Task<()>>,
    pub(crate) drag: Option<crate::view_input::MouseDrag>,
    pub(crate) drag_scroll_speed: (f32, f32),
    pub(crate) drag_scroll_task: Option<Task<()>>,
    pub(crate) clean: Option<Text>,
    pub(crate) dirty: bool,
    pub(crate) emit_task: Option<Task<()>>,
    pub(crate) last_emitted: Option<String>,
    pub(crate) diff_timer: Option<Task<()>>,
    pub(crate) search_open: bool,
    pub(crate) tabs: HashMap<String, TabSnapshot>,
    pub(crate) tooltips: crate::tooltip_ui::Tooltips,
    pub(crate) hover_task: Option<Task<()>>,
    pub(crate) composing: bool,
    pub(crate) composition_first: bool,
    pub(crate) completion_restart_task: Option<Task<()>>,
    pub(crate) marked: Option<(usize, usize)>,
    /// `tabFocusMode`: -1 off, 0 on, else a deadline (ms) after Escape.
    pub(crate) tab_focus: i64,
    pub(crate) drop_cursor: Option<usize>,
    pub(crate) announcement: Option<String>,
    pub(crate) emacs: Option<crate::emacs::Emacs>,
    pub(crate) vim: Option<crate::vim::Vim>,
    pub(crate) gutter_hover: Option<(crate::element::GutterKind, usize)>,
    pub(crate) collapsed_hover: Option<usize>,
    pub(crate) chip_hover: bool,
    pub(crate) chip_bounds: Option<gpui_kit::Bounds<Pixels>>,
    pub(crate) revert_pointer_inside: bool,
    pub(crate) last_hover_point: Option<gpui_kit::Point<Pixels>>,
    pub(crate) lsp_sync_task: Option<Task<()>>,
    pub(crate) signature_task: Option<Task<()>>,
    /// Edits made while a formatting request is in flight.
    pub(crate) format_since: Option<crate::change::ChangeSet>,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<EditorEvent> for EditorView {}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Bundle language data and fold markers for a scope, from the catalog.
pub fn language_config(tm: Option<&SharedTextMate>, scope: &str) -> Option<Rc<LanguageConfig>> {
    let tm = tm?;
    let cat = &tm.borrow().catalog;
    let meta = cat.grammar_meta(scope)?;
    let markers: Option<CompiledFoldMarkers> =
        compile_fold_markers(cat.fold_marker_sources(scope).as_ref());
    Some(Rc::new(LanguageConfig {
        scope: scope.to_string(),
        data: language_data(&meta.language_data),
        fold_markers: markers,
    }))
}

impl EditorView {
    pub fn new(options: EditorOptions, window: &mut Window, cx: &mut Context<Self>) -> EditorView {
        let executor = cx.background_executor().clone();
        crate::clock::install_source(move || executor.now());
        let platform = Platform::current();
        let scope = options
            .textmate
            .as_ref()
            .and_then(|tm| {
                tm.borrow()
                    .catalog
                    .resolve_scope_for_path(&options.path, first_line(&options.doc))
            })
            .unwrap_or_default();
        if let (Some(tm), Some(parsed)) = (options.textmate.as_ref(), options.theme.as_ref()) {
            tm.borrow_mut().activate_theme(parsed.clone());
        }
        let grammar_resident = options
            .textmate
            .as_ref()
            .is_some_and(|tm| tm.borrow().catalog.has_grammar(&scope));
        let metrics = match &options.mini {
            Some(m) => EditorMetrics::mini(&options.prefs, m),
            None => EditorMetrics::from_prefs(&options.prefs),
        };
        let theme = resolve_theme(
            options.theme.as_deref(),
            options.dark_fallback,
            options.mini.is_some(),
            recur_ui::Theme::of(cx).id,
        );
        let lsp_attached = false;
        let keymap = Keymap::build(
            &match options.kind {
                EditorKind::Code => code_editor_keymap(lsp_attached),
                EditorKind::Merge { .. } => merge_editor_keymap(),
            },
            platform,
        );
        let mut view = EditorView {
            focus: cx.focus_handle(),
            kind: options.kind.clone(),
            path: options.path.clone(),
            pane: options.pane.clone(),
            state: EditorState::create("", StateConfig::default()),
            prefs: options.prefs.clone(),
            parsed_theme: options.theme.clone(),
            dark_fallback: options.dark_fallback,
            theme,
            font: editor_font(&metrics),
            metrics,
            layout: RefCell::new(Layout::new(
                LayoutConfig::default(),
                &EditorState::create("", StateConfig::default()),
            )),
            tm: options.textmate.clone(),
            highlighter: None,
            scope,
            keymap,
            platform,
            snippets: options.snippets.clone(),
            lsp: None,
            history_scope: options.history_scope.clone(),
            mini: options.mini.clone(),
            restored_scroll: None,
            read_only: options.read_only,
            lit: options.lit,
            scroll_top: 0.0,
            scroll_left: 0.0,
            pending_scroll: None,
            frame: None,
            blink_start: recur_ui::motion::now(cx),
            blink_epoch: 0,
            blink_task: None,
            has_focus: false,
            completion: CompletionDriver::default(),
            completion_timer: None,
            highlight_timer: None,
            fill_task: None,
            search: None,
            dialogs: Vec::new(),
            dialog_seq: 0,
            rename_prompt: None,
            fold_markers_live: grammar_resident,
            hovered_control: None,
            title_seen: None,
            revert: RevertHover::default(),
            revert_timer: None,
            drag: None,
            drag_scroll_speed: (0.0, 0.0),
            drag_scroll_task: None,
            clean: None,
            dirty: false,
            emit_task: None,
            last_emitted: None,
            diff_timer: None,
            search_open: false,
            tabs: HashMap::new(),
            tooltips: Default::default(),
            hover_task: None,
            composing: false,
            composition_first: false,
            completion_restart_task: None,
            marked: None,
            tab_focus: -1,
            drop_cursor: None,
            announcement: None,
            emacs: None,
            vim: None,
            gutter_hover: None,
            collapsed_hover: None,
            chip_hover: false,
            chip_bounds: None,
            revert_pointer_inside: false,
            last_hover_point: None,
            lsp_sync_task: None,
            signature_task: None,
            format_since: None,
            _subscriptions: Vec::new(),
        };
        let focus = view.focus.clone();
        view._subscriptions = vec![
            cx.on_blur(&focus, window, |v, window, cx| v.blurred(window, cx)),
            cx.on_focus(&focus, window, |v, _window, cx| {
                v.has_focus = true;
                v.restart_blink(cx);
                cx.notify();
            }),
            cx.observe_global_in::<recur_ui::Theme>(window, |v, window, cx| {
                let parsed = v.parsed_theme.clone();
                let dark = v.dark_fallback;
                v.set_theme(parsed, dark, window, cx);
            }),
        ];
        let state = view.restore_or_build(&options.doc, cx);
        view.mount_state(state, window, cx);
        match options.saved_doc.as_deref() {
            Some(saved) if !crate::view_state::editor_text_eq(saved, &options.doc) => {
                view.clean = Some(Text::of(saved));
                view.set_dirty(true, cx);
            }
            _ => view.clean = Some(view.state.doc.clone()),
        }
        if let Some(vs) = options.view_state.as_ref() {
            view.apply_view_state(vs, window, cx);
        }
        view.sync_keymap_mode();
        view
    }

    /// `StateConfig` for this editor from prefs, path and language.
    pub(crate) fn state_config(&self) -> StateConfig {
        let indent = self.prefs.resolve_indent(language_id_for_path(&self.path));
        let completion = match self.kind {
            EditorKind::Code => {
                let mut sources = Vec::new();
                if let Some(provider) = self.snippets.clone() {
                    let path = self.path.clone();
                    let scope = self.scope.clone();
                    sources.push(SourceRef(Rc::new(SnippetCompletionSource {
                        provider,
                        scope_path: Rc::new(move || (scope.clone(), path.clone())),
                    })));
                }
                sources.push(SourceRef(Rc::new(AnyWordSource)));
                Some(Rc::new(CompletionConfig { sources }))
            }
            EditorKind::Merge { .. } => None,
        };
        StateConfig {
            tab_size: self.prefs.tab_size,
            indent_unit: indent.unit().to_string(),
            read_only: self.read_only,
            allow_multiple_selections: true,
            language: if self.scope.is_empty() {
                None
            } else {
                language_config(self.tm.as_ref(), &self.scope)
            },
            history_min_depth: EDITOR_HISTORY_MIN_DEPTH,
            history_new_group_delay: 500,
            indent_on_input: true,
            completion,
        }
    }

    fn build_state(&self, doc: &str) -> EditorState {
        let mut state = EditorState::create(doc, self.state_config());
        if let EditorKind::Merge { original } = &self.kind {
            state.merge = Some(Rc::new(MergeState::new(original, &state.doc)));
        }
        state
    }

    /// A fresh state for `doc`, keeping the file's serialized undo history when the cache
    /// holds it for this text (`restoreEditorState`).
    fn restore_or_build(&mut self, doc: &str, cx: &mut App) -> EditorState {
        self.restored_scroll = None;
        if self.history_scope.is_empty() || self.path.is_empty() {
            return self.build_state(doc);
        }
        let key = crate::history_cache::editor_history_key(&self.history_scope, &self.path);
        let config = self.state_config();
        let restored = cx
            .default_global::<crate::history_cache::EditorHistory>()
            .0
            .restore(&key, doc, config);
        match restored {
            Some(r) => {
                let mut state = r.state;
                if let EditorKind::Merge { original } = &self.kind {
                    state.merge = Some(Rc::new(MergeState::new(original, &state.doc)));
                }
                self.restored_scroll = Some((r.scroll_top, r.scroll_left));
                state
            }
            None => self.build_state(doc),
        }
    }

    /// The scroll the mounted state was restored from the undo cache with, if any.
    pub fn restored_scroll(&self) -> Option<(f32, f32)> {
        self.restored_scroll
    }

    /// Scroll to a pixel position (a restored history snapshot's).
    pub fn set_scroll(&mut self, top: f32, left: f32, cx: &mut Context<Self>) {
        self.scroll_top = top.max(0.0);
        self.scroll_left = left.max(0.0);
        cx.notify();
    }

    /// The undo cache scope for later mounts (`sessionKey`).
    pub fn set_history_scope(&mut self, scope: &str) {
        self.history_scope = scope.to_string();
    }

    /// `captureAllHistory`: serialize the mounted tab's and the parked tabs' undo stacks into
    /// the per-file cache.
    pub fn capture_history(&self, cx: &mut App) {
        if self.history_scope.is_empty() {
            return;
        }
        let cache = &mut cx.default_global::<crate::history_cache::EditorHistory>().0;
        if !self.path.is_empty() {
            let key = crate::history_cache::editor_history_key(&self.history_scope, &self.path);
            cache.capture(&key, &self.state, self.scroll_top, self.scroll_left);
        }
        for (p, snap) in &self.tabs {
            let key = crate::history_cache::editor_history_key(&self.history_scope, p);
            cache.capture(&key, &snap.state, 0.0, 0.0);
        }
    }

    /// `parkSession`'s half: stash the mounted tab and hand over every tab snapshot. The
    /// pane mounts nothing until the next `mount_path`.
    pub fn take_tabs(&mut self) -> Vec<(String, TabSnapshot)> {
        self.stash_current();
        self.path.clear();
        self.tabs.drain().collect()
    }

    /// `adoptSession`'s half: a parked tab map for this pane.
    pub fn adopt_tabs(&mut self, tabs: Vec<(String, TabSnapshot)>) {
        self.tabs.clear();
        self.tabs.extend(tabs);
    }

    /// Whether a parked snapshot exists for `path`.
    pub fn has_tab(&self, path: &str) -> bool {
        self.tabs.contains_key(path)
    }

    /// Point the view at a state (`setState`): layout, highlighter,
    /// completion and panels restart.
    pub(crate) fn mount_state(
        &mut self,
        state: EditorState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state = state;
        self.rebuild_layout(window);
        self.mount_highlighter(window);
        let (driver, step) = CompletionDriver::new(&self.state);
        self.completion = driver;
        self.run_driver_step(step, window, cx);
        self.search_open = self.state.search.panel;
        self.restart_blink(cx);
        cx.notify();
    }

    pub(crate) fn layout_config(&self, window: &Window) -> LayoutConfig {
        let lh = self.metrics.line_height();
        let measure = GpuiMeasure {
            ts: window.text_system(),
            font: self.font.clone(),
            size: px(self.metrics.font_size),
        };
        let char_width = {
            let xs = measure.char_xs("abcdefghijklmnopqrstuvwxyz");
            xs.last().copied().unwrap_or(0.0) / 26.0
        };
        let (wrap_width, viewport_height) = self.frame.as_ref().map_or((800.0, 600.0), |f| {
            (f.scroller_width - f.gutter_width, f.scroller_height)
        });
        LayoutConfig {
            line_height: lh,
            char_width,
            tab_size: self.state.tab_size(),
            special_chars: self.prefs.special_chars,
            wrap: self.prefs.line_wrapping,
            wrap_width: (wrap_width
                - self.metrics.line_padding_left
                - self.metrics.line_padding_right)
                .max(1.0),
            padding_top: self.metrics.content_padding_y,
            padding_bottom: self.metrics.content_padding_y,
            viewport_height,
            line_padding_left: self.metrics.line_padding_left,
            line_padding_right: self.metrics.line_padding_right,
        }
    }

    /// Block widgets for the merge view (deleted chunks, collapsed runs).
    pub(crate) fn block_widgets(&self, window: &Window) -> Vec<BlockWidget> {
        let measure = GpuiMeasure {
            ts: window.text_system(),
            font: self.font.clone(),
            size: px(self.metrics.font_size),
        };
        crate::element::merge_widgets(self, &measure)
    }

    pub(crate) fn rebuild_layout(&mut self, window: &Window) {
        self.layout.borrow_mut().config = self.layout_config(window);
        let widgets = self.block_widgets(window);
        self.layout.borrow_mut().rebuild(&self.state, widgets);
    }

    fn mount_highlighter(&mut self, window: &Window) {
        if let Some(h) = self.highlighter.as_mut() {
            h.destroy();
        }
        self.highlighter = None;
        let Some(tm) = self.tm.clone() else { return };
        if self.scope.is_empty() {
            return;
        }
        let doc_id = recur_textmate::tm_doc_id_for(&self.pane, &self.path);
        let doc_id = if doc_id.is_empty() {
            format!("tm-{}", self.focus_id())
        } else {
            doc_id
        };
        let paint_bg = !matches!(self.kind, EditorKind::Merge { .. });
        let mut h = Highlighter::new(tm, &self.scope, &doc_id, PaintSource::Shared, paint_bg);
        let win = self.highlight_window(window);
        let needs = h.mount(&self.state, win);
        self.highlighter = Some(h);
        let _ = needs;
    }

    fn focus_id(&self) -> String {
        format!("{:?}", self.focus)
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect()
    }

    /// The rendered line window for highlighting.
    pub(crate) fn highlight_window(&self, _window: &Window) -> (usize, usize) {
        match &self.frame {
            Some(f) => viewport_window(f.rendered_lines, f.visible_lines.0),
            None => (
                1,
                self.state
                    .doc
                    .lines()
                    .min(crate::highlight::MAX_BUILD_LINES),
            ),
        }
    }

    /// A [`LayoutView`] for commands and hit testing.
    pub(crate) fn with_layout_view<R>(
        &self,
        window: &Window,
        f: impl FnOnce(&LayoutView) -> R,
    ) -> R {
        let measure = GpuiMeasure {
            ts: window.text_system(),
            font: self.font.clone(),
            size: px(self.metrics.font_size),
        };
        let scroll = self
            .frame
            .as_ref()
            .map(|fr| (self.scroll_top, self.scroll_top + fr.scroller_height));
        let lv = LayoutView {
            state: &self.state,
            layout: &self.layout,
            measure: &measure,
            scroll,
            atomic: Vec::new(),
        };
        f(&lv)
    }

    /// Dispatch a transaction spec (`view.dispatch`).
    pub fn dispatch(&mut self, spec: TransactionSpec, window: &mut Window, cx: &mut Context<Self>) {
        let tr = Rc::new(self.state.update(spec));
        self.apply(tr, window, cx);
    }

    /// Apply a resolved transaction (`view.update([tr])`).
    pub(crate) fn apply(
        &mut self,
        tr: Rc<Transaction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = std::mem::replace(&mut self.state, tr.state().clone());
        let state = self.state.clone();
        if tr.reconfigured() {
            self.read_only = state.read_only();
        }
        // Layout: splice changed lines, rebuild on fold / widget changes.
        let folds_changed = start.folds != state.folds;
        let merge_changed = match (&start.merge, &state.merge) {
            (Some(a), Some(b)) => !Rc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if tr.doc_changed() || folds_changed {
            self.fold_markers_live = true;
        }
        if tr.doc_changed() || folds_changed || merge_changed || tr.reconfigured() {
            self.relayout_after(
                &tr,
                &start,
                folds_changed || merge_changed || tr.reconfigured(),
                window,
            );
        }
        let win = self.highlight_window(window);
        if let Some(h) = self.highlighter.as_mut() {
            let needs = h.update(&tr, &state, win);
            self.handle_highlight_needs(needs, window, cx);
        }
        self.completion
            .update(&start, &state, std::slice::from_ref(&tr), now_ms());
        self.schedule_completion(window, cx);
        if let Some(search) = self.search.as_mut() {
            search.model.update(&tr);
            self.sync_search_fields(window, cx);
        }
        if tr.doc_changed()
            && let Some(since) = self.format_since.take()
        {
            self.format_since = Some(since.compose(&tr.changes));
        }
        self.map_drag(&tr);
        if tr.doc_changed()
            && let Some(e) = self.emacs.as_mut()
        {
            e.doc_changed(&tr.changes.desc());
        }
        for e in &tr.effects {
            match e {
                Effect::ScrollIntoView(t) => self.pending_scroll = Some(*t),
                Effect::Announce(text) => {
                    self.announcement = Some(text.clone());
                    cx.emit(EditorEvent::Announce(text.clone()));
                }
                _ => {}
            }
        }
        if tr.scroll_into_view && self.pending_scroll.is_none() {
            self.pending_scroll = Some(ScrollTarget::range(state.selection.main()));
        }
        let open = state.search.panel;
        if open != self.search_open {
            self.search_open = open;
            if open {
                self.ensure_search_ui(window, cx);
            } else {
                self.search = None;
                window.focus(&self.focus, cx);
            }
            cx.emit(EditorEvent::SearchOpenChanged(open));
        }
        if tr.doc_changed() {
            self.sync_dirty(cx);
            self.schedule_diff_recompute(window, cx);
            if let Some(lsp) = self.lsp.as_mut() {
                lsp.document_changed(&start.doc, &tr.changes, &state.doc);
            }
        }
        if tr.doc_changed() || tr.selection.is_some() {
            let head = state.selection.main().head();
            let line = state.doc.line_at(head);
            cx.emit(EditorEvent::Cursor {
                line: line.number,
                col: head - line.from + 1,
            });
            self.restart_blink(cx);
        }
        let chip = self.revert.update(&tr, false);
        self.apply_chip(chip, cx);
        self.tooltips.after_transaction(&tr, &state);
        if let Some(lsp) = self.lsp.as_mut() {
            let had = lsp.signature.is_some();
            lsp.after_transaction(&tr, &state);
            if had && tr.doc_changed() {
                self.tooltips.signature_changed();
            }
        }
        self.after_lsp_transaction(&tr, window, cx);
        cx.notify();
        crate::vim::cm::view_updated(self, &tr, window, cx);
    }

    fn relayout_after(
        &mut self,
        tr: &Transaction,
        start: &EditorState,
        full: bool,
        window: &Window,
    ) {
        if full {
            self.rebuild_layout(window);
            return;
        }
        let mut lines: Option<(usize, usize, usize, usize)> = None;
        tr.changes.iter_changed_ranges(false, |fa, ta, fb, tb| {
            let (la, lb) = (start.doc.line_at(fa).number, start.doc.line_at(ta).number);
            let (na, nb) = (
                self.state.doc.line_at(fb).number,
                self.state.doc.line_at(tb).number,
            );
            lines = Some(match lines {
                None => (la, lb, na, nb),
                Some((a, b, c, d)) => (a.min(la), b.max(lb), c.min(na), d.max(nb)),
            });
        });
        let Some((old_from, old_to, new_from, new_to)) = lines else {
            return;
        };
        let config = self.layout_config(window);
        let lh = config.line_height as f64;
        let mut l = self.layout.borrow_mut();
        let line_length = l.line_length();
        let estimates: Vec<f64> = (new_from..=new_to)
            .map(|n| {
                if !config.wrap {
                    return lh;
                }
                let line = self.state.doc.line(n);
                let chars = self.state.doc.slice(line.from, line.to).chars().count();
                crate::display::estimate_rows(chars, line_length) as f64 * lh
            })
            .collect();
        l.heights.splice(old_from, old_to - old_from + 1, estimates);
        l.forget_measured_from(old_from, old_to, new_to as isize - old_to as isize);
    }

    pub(crate) fn handle_highlight_needs(
        &mut self,
        needs: crate::highlight::HighlightNeeds,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if needs.redraw {
            cx.notify();
        }
        if let Some(at) = needs.scroll_pass_at {
            let delay = at.saturating_sub(now_ms());
            self.highlight_timer = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(delay))
                    .await;
                this.update_in(cx, |v, window, cx| {
                    let win = v.highlight_window(window);
                    let state = v.state.clone();
                    if let Some(h) = v.highlighter.as_mut() {
                        let needs = h.scroll_pass(&state, win, now_ms());
                        v.handle_highlight_needs(needs, window, cx);
                    }
                })
                .ok();
            }));
        }
        if needs.fill && self.fill_task.is_none() {
            self.fill_task = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(FILL_IDLE_DELAY_MS))
                        .await;
                    let more = this
                        .update_in(cx, |v, window, cx| {
                            let win = v.highlight_window(window);
                            let state = v.state.clone();
                            let Some(h) = v.highlighter.as_mut() else {
                                return false;
                            };
                            let needs = h.run_fill(&state, win, FILL_IDLE_BUDGET);
                            if needs.redraw {
                                cx.notify();
                            }
                            needs.fill
                        })
                        .unwrap_or(false);
                    if !more {
                        break;
                    }
                }
                this.update(cx, |v, _| v.fill_task = None).ok();
            }));
        }
    }

    /// Run the completion driver's due timers and pending queries.
    pub(crate) fn schedule_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(deadline) = self.completion.next_deadline() else {
            self.completion_timer = None;
            return;
        };
        let delay = deadline.saturating_sub(now_ms());
        self.completion_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            this.update_in(cx, |v, window, cx| {
                let state = v.state.clone();
                let step = v.completion.tick(&state, now_ms());
                v.run_driver_step(step, window, cx);
                v.schedule_completion(window, cx);
            })
            .ok();
        }));
    }

    pub(crate) fn run_driver_step(
        &mut self,
        step: DriverStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for spec in step.dispatch {
            self.dispatch(spec, window, cx);
        }
        for launch in step.launch {
            let id = launch.id;
            let fut = launch.future;
            cx.spawn_in(window, async move |this, cx| {
                let result = fut.await;
                this.update_in(cx, |v, window, cx| {
                    let state = v.state.clone();
                    let step = v.completion.query_done(id, result, &state, now_ms());
                    v.run_driver_step(step, window, cx);
                    v.schedule_completion(window, cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// Caret blink (`drawSelection({ cursorBlinkRate })`): solid for a
    /// period after activity, then on/off halves.
    pub(crate) fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_start = recur_ui::motion::now(cx);
        self.blink_epoch += 1;
        let rate = self.metrics.cursor_blink_ms;
        if rate == 0 || !self.has_focus || recur_ui::motion::is_frozen(cx) {
            self.blink_task = None;
            return;
        }
        let epoch = self.blink_epoch;
        let start = self.blink_start;
        self.blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let half = rate / 2;
                let into = start.elapsed().as_millis() as u64 % half.max(1);
                cx.background_executor()
                    .timer(Duration::from_millis(half - into + 1))
                    .await;
                let alive = this
                    .update(cx, |v, cx| {
                        if v.blink_epoch != epoch {
                            return false;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
    }

    /// `cm-blink` (`steps(1)`, opaque for the first half of each period),
    /// on recur-ui's motion clock so parity captures freeze it.
    pub(crate) fn caret_visible(&self, cx: &gpui_kit::App) -> bool {
        let rate = self.metrics.cursor_blink_ms;
        if rate == 0 {
            return true;
        }
        let t = recur_ui::motion::elapsed(self.blink_start, cx).as_millis() as u64;
        t % rate < rate / 2
    }

    fn set_dirty(&mut self, next: bool, cx: &mut Context<Self>) {
        if self.dirty == next {
            return;
        }
        self.dirty = next;
        cx.emit(EditorEvent::DirtyChanged(next));
    }

    fn remember_clean(&mut self, cx: &mut Context<Self>) {
        self.clean = Some(self.state.doc.clone());
        self.set_dirty(false, cx);
    }

    /// `syncDirtyFrom`: dirty flag now, text on a debounce.
    fn sync_dirty(&mut self, cx: &mut Context<Self>) {
        let dirty = is_buffer_dirty(&self.state.doc, self.clean.as_ref());
        self.set_dirty(dirty, cx);
        if dirty {
            if self.emit_task.is_none() {
                self.emit_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(EMIT_DEBOUNCE_MS))
                        .await;
                    this.update(cx, |v, cx| v.flush_emit(cx)).ok();
                }));
            }
        } else {
            self.flush_emit(cx);
        }
    }

    /// Serialise once and publish (`flushEmit`).
    pub(crate) fn flush_emit(&mut self, cx: &mut Context<Self>) {
        self.emit_task = None;
        let text = self.state.doc.to_string();
        self.last_emitted = Some(text.clone());
        cx.emit(EditorEvent::DocChanged(text));
    }

    fn schedule_diff_recompute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.gutters.diff_base.is_none() {
            return;
        }
        self.diff_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(crate::gutters::DIFF_RECOMPUTE_MS))
                .await;
            this.update_in(cx, |v, window, cx| {
                if v.state.gutters.diff_base.is_some() {
                    v.dispatch(
                        TransactionSpec::default()
                            .effect(Effect::Gutter(crate::gutters::GutterEffect::RecomputeDiff))
                            .add_to_history(false),
                        window,
                        cx,
                    );
                }
            })
            .ok();
        }));
    }

    pub(crate) fn apply_chip(&mut self, action: crate::merge::ChipAction, cx: &mut Context<Self>) {
        if action != crate::merge::ChipAction::Keep {
            cx.notify();
        }
        if let Some(at) = self.revert.hide_at {
            let delay = at.saturating_sub(now_ms());
            self.revert_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(delay))
                    .await;
                this.update(cx, |v, cx| {
                    let state = v.state.clone();
                    v.revert.hide_due(&state, now_ms());
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    // ---- public API (CodeEditor / MergeEditor exports) ----

    pub fn state(&self) -> &EditorState {
        &self.state
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// `peekDoc`: current text without flushing the emit debounce.
    pub fn peek_doc(&self) -> String {
        self.state.doc.to_string()
    }

    /// `getDoc`: authoritative text; flushes a pending emit.
    pub fn get_doc(&mut self, cx: &mut Context<Self>) -> String {
        if self.emit_task.is_some() {
            self.flush_emit(cx);
        }
        self.last_emitted
            .clone()
            .unwrap_or_else(|| self.state.doc.to_string())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// `markSaved`: the parent's save landed.
    pub fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.remember_clean(cx);
    }

    /// The parent's on-disk snapshot moved (`applySavedBaseline`).
    pub fn apply_saved_baseline(&mut self, text: &str, cx: &mut Context<Self>) {
        if !crate::view_state::disk_baseline_moved(self.clean.as_ref(), text) {
            return;
        }
        self.clean = Some(Text::of(text));
        self.sync_dirty(cx);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub fn has_focus(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Change read-only (no remount).
    pub fn set_read_only(&mut self, ro: bool, window: &mut Window, cx: &mut Context<Self>) {
        if ro == self.read_only {
            return;
        }
        self.read_only = ro;
        let config = self.state_config();
        self.dispatch(
            TransactionSpec::default().effect(Effect::Reconfigure(Rc::new(config))),
            window,
            cx,
        );
    }

    /// New prefs: layout, keymap mode, indentation and features.
    pub fn set_prefs(&mut self, prefs: EditorPrefs, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs = prefs;
        // The features compartment builds a fresh `foldGutter()`.
        self.fold_markers_live = true;
        self.metrics = match &self.mini {
            Some(m) => EditorMetrics::mini(&self.prefs, m),
            None => EditorMetrics::from_prefs(&self.prefs),
        };
        self.font = editor_font(&self.metrics);
        let config = self.state_config();
        self.dispatch(
            TransactionSpec::default().effect(Effect::Reconfigure(Rc::new(config))),
            window,
            cx,
        );
        self.sync_keymap_mode();
        if !self.prefs.diff_gutter && self.state.gutters.diff_base.is_some() {
            self.set_diff_base(None, window, cx);
        }
        self.rebuild_layout(window);
        self.restart_blink(cx);
        cx.notify();
    }

    /// Editor theme: the TextMate theme over the app chrome tokens of the
    /// current `recur_ui::Theme` (followed as it changes).
    pub fn set_theme(
        &mut self,
        parsed: Option<Rc<ParsedTmTheme>>,
        dark_fallback: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.parsed_theme = parsed;
        self.dark_fallback = dark_fallback;
        if let (Some(tm), Some(parsed)) = (self.tm.as_ref(), self.parsed_theme.as_ref()) {
            tm.borrow_mut().activate_theme(parsed.clone());
        }
        self.theme = resolve_theme(
            self.parsed_theme.as_deref(),
            dark_fallback,
            self.mini.is_some(),
            recur_ui::Theme::of(cx).id,
        );
        self.restyle_panel_inputs(cx);
        crate::vim::prompt::restyle(self, cx);
        let win = self.highlight_window(window);
        let state = self.state.clone();
        if let Some(h) = self.highlighter.as_mut() {
            let needs = h.viewport_changed(&state, win);
            self.handle_highlight_needs(needs, window, cx);
        }
        cx.notify();
    }

    /// The Observatory pane is lit (selection / gutter bar dimming).
    pub fn set_lit(&mut self, lit: bool, cx: &mut Context<Self>) {
        if self.lit != lit {
            self.lit = lit;
            cx.notify();
        }
    }

    /// Swap the TextMate engine (a host that pairs its own, as `MiniScriptEditor` does):
    /// re-resolve the scope and remount highlighting.
    pub fn set_textmate(
        &mut self,
        tm: Option<SharedTextMate>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tm = tm;
        if let Some(h) = self.highlighter.as_mut() {
            h.destroy();
        }
        self.highlighter = None;
        self.ensure_language(window, cx);
    }

    /// The grammar for this path may have arrived or changed (bundle
    /// reload, syntax mode change): re-resolve and remount highlighting.
    pub fn ensure_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self
            .tm
            .as_ref()
            .and_then(|tm| {
                let text = self.state.doc.line_text(1);
                tm.borrow()
                    .catalog
                    .resolve_scope_for_path(&self.path, Some(&text))
            })
            .unwrap_or_default();
        let scope_changed = scope != self.scope;
        self.scope = scope;
        let config = self.state_config();
        self.dispatch(
            TransactionSpec::default().effect(Effect::Reconfigure(Rc::new(config))),
            window,
            cx,
        );
        if scope_changed || self.highlighter.is_none() {
            self.mount_highlighter(window);
        }
        cx.notify();
    }

    /// Findings gutter marks (`setEditorFindings`).
    pub fn set_findings(
        &mut self,
        marks: Vec<crate::gutters::FindingMark>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dispatch(
            TransactionSpec::default()
                .effect(Effect::Gutter(crate::gutters::GutterEffect::SetFindings(
                    marks.into(),
                )))
                .add_to_history(false),
            window,
            cx,
        );
    }

    /// SCM base text for the diff bars (`setEditorDiffBase`); `None`
    /// clears them.
    pub fn set_diff_base(
        &mut self,
        base: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let base = if self.prefs.diff_gutter { base } else { None };
        if self.state.gutters.diff_base.as_deref() == base {
            return;
        }
        self.dispatch(
            TransactionSpec::default()
                .effect(Effect::Gutter(crate::gutters::GutterEffect::SetDiffBase(
                    base.map(Rc::from),
                )))
                .add_to_history(false),
            window,
            cx,
        );
    }

    /// LSP / host diagnostics (`setDiagnostics`).
    pub fn set_diagnostics(
        &mut self,
        diags: Vec<crate::lint::Diagnostic>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dispatch(crate::lint::set_diagnostics(diags), window, cx);
    }

    /// `scrollToLine`: select a 1-based line, line range or column span
    /// and center it.
    pub fn scroll_to_line(
        &mut self,
        line: usize,
        end_line: usize,
        start_col: usize,
        end_col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if line < 1 {
            return;
        }
        let end = (end_line > 0).then_some(end_line);
        let Some(range) = crate::view_state::mention_range_from_loc(
            &self.state.doc,
            line,
            end,
            start_col,
            end_col,
        ) else {
            return;
        };
        let (sel, scroll) = crate::view_state::mention_selection_spec(range);
        self.dispatch(
            TransactionSpec::default()
                .selection(sel)
                .effect(Effect::ScrollIntoView(scroll)),
            window,
            cx,
        );
        self.focus(window, cx);
    }

    /// `getViewState` (caret + scroll, without the view mode).
    pub fn get_view_state(&self, window: &Window) -> Option<FileViewState> {
        let frame = self.frame.as_ref()?;
        let top_pos = self.with_layout_view(window, |lv| lv.pos_at(24.0, self.scroll_top + 8.0));
        crate::view_state::capture_file_view_state(
            &self.state,
            crate::view_state::EditorViewMode::Code,
            Some(top_pos),
            (self.scroll_top as f64, self.scroll_left as f64),
            frame.scroller_height > 0.0,
        )
    }

    /// `restoreViewState` / `applyFileViewState`.
    pub fn apply_view_state(
        &mut self,
        vs: &FileViewState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = crate::view_state::selection_from_view_state(&self.state.doc, Some(vs));
        let mut spec = TransactionSpec::default()
            .selection(sel)
            .add_to_history(false);
        if let Some(t) = crate::view_state::scroll_target_from_view_state(&self.state.doc, Some(vs))
        {
            spec = spec.effect(Effect::ScrollIntoView(t));
        }
        self.dispatch(spec, window, cx);
        if vs.scroll_left > 0.0 {
            self.scroll_left = vs.scroll_left as f32;
        }
    }

    /// `getMentionSelection`.
    pub fn mention_selection(&self) -> Option<crate::view_state::MentionSelection> {
        crate::view_state::mention_selection_from_state(&self.state)
    }

    /// `toggleSearch`.
    pub fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.state.search.panel {
            self.run_cmd(Cmd::CloseSearchPanel, window, cx)
        } else {
            self.run_cmd(Cmd::OpenSearchPanel, window, cx)
        }
    }

    pub fn is_search_open(&self) -> bool {
        self.state.search.panel
    }

    /// `replaceDoc`: a fresh state for new text (reload from disk).
    pub fn replace_doc(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.remove(&self.path.clone());
        self.last_emitted = Some(text.to_string());
        let state = self.build_state(text);
        self.mount_state(state, window, cx);
        self.remember_clean(cx);
    }

    /// External change to the open file (`doc` prop drift): replace the
    /// text keeping scroll and caret, out of the undo history.
    pub fn external_doc_change(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.last_emitted.as_deref() == Some(text) || self.state.doc.to_string() == text {
            self.last_emitted = Some(text.to_string());
            self.remember_clean(cx);
            return;
        }
        let head = self.state.selection.main().head();
        let normalized = crate::text::normalize_newlines(text).into_owned();
        let spec = TransactionSpec {
            annotations: Annotations {
                add_to_history: Some(false),
                ..Default::default()
            },
            ..TransactionSpec::default()
        }
        .changes(crate::change::Change::new(
            0,
            self.state.doc.len(),
            normalized.clone(),
        ))
        .cursor(head.min(normalized.len()));
        self.dispatch(spec, window, cx);
        self.last_emitted = Some(text.to_string());
        self.remember_clean(cx);
    }

    /// Switch the pane to another file (`mountEditor`), parking the
    /// current tab and restoring a parked one when present.
    pub fn mount_path(
        &mut self,
        path: &str,
        doc: &str,
        saved: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stash_current();
        let cached = self.tabs.remove(path);
        self.path = path.to_string();
        self.scope = self
            .tm
            .as_ref()
            .and_then(|tm| {
                tm.borrow()
                    .catalog
                    .resolve_scope_for_path(path, first_line(doc))
            })
            .unwrap_or_default();
        self.last_emitted = None;
        match cached {
            Some(snap) => {
                let mut state = snap.state;
                state = state
                    .update(
                        TransactionSpec::default()
                            .effect(Effect::Reconfigure(Rc::new(self.state_config()))),
                    )
                    .state()
                    .clone();
                self.mount_state(state, window, cx);
                self.pending_scroll = snap.scroll;
                if snap.dirty {
                    self.clean = snap.clean;
                    self.set_dirty(true, cx);
                    self.flush_emit(cx);
                } else if doc != self.state.doc.to_string() {
                    self.external_doc_change(doc, window, cx);
                } else {
                    self.clean = snap.clean.or_else(|| Some(self.state.doc.clone()));
                    self.set_dirty(false, cx);
                }
            }
            None => {
                let state = self.restore_or_build(doc, cx);
                self.mount_state(state, window, cx);
                match saved {
                    Some(s) if !crate::view_state::editor_text_eq(s, doc) => {
                        self.clean = Some(Text::of(s));
                        self.set_dirty(true, cx);
                    }
                    _ => self.remember_clean(cx),
                }
                self.scroll_top = 0.0;
                self.scroll_left = 0.0;
            }
        }
        cx.notify();
    }

    fn stash_current(&mut self) {
        if self.path.is_empty() {
            return;
        }
        if !self.dirty && self.state.doc.len() > crate::view_state::EDITOR_BODY_CAP {
            return;
        }
        let scroll = self.scroll_snapshot();
        self.tabs.insert(
            self.path.clone(),
            TabSnapshot {
                state: self.state.clone(),
                scroll,
                dirty: self.dirty,
                clean: self.clean.clone(),
            },
        );
    }

    /// A scroll anchor: the line at the top and the offset into it.
    pub(crate) fn scroll_snapshot(&self) -> Option<ScrollTarget> {
        let l = self.layout.borrow();
        let n = l.line_at_y(self.scroll_top as f64);
        let top = l.block_top(n) as f32;
        let pos = self.state.doc.line(n).from;
        Some(ScrollTarget {
            snapshot_top: Some(self.scroll_top - top),
            ..ScrollTarget::pos(pos, ScrollStrategy::Start, 0.0)
        })
    }

    /// Drop cached tabs for paths no longer open (`retainPaths`).
    pub fn retain_paths(&mut self, keep: &[String]) {
        let dropped: Vec<String> = self
            .tabs
            .keys()
            .filter(|p| !keep.contains(p))
            .cloned()
            .collect();
        for p in &dropped {
            self.tabs.remove(p);
        }
        if let Some(tm) = &self.tm {
            let ids: Vec<String> = dropped
                .iter()
                .map(|p| recur_textmate::tm_doc_id_for(&self.pane, p))
                .collect();
            tm.borrow_mut()
                .catalog
                .forget_docs(ids.iter().map(String::as_str), false);
        }
    }

    /// Drop a cached tab so the next mount rebuilds (`forgetPath`).
    pub fn forget_path(&mut self, path: &str) {
        self.tabs.remove(path);
        if let Some(tm) = &self.tm {
            let id = recur_textmate::tm_doc_id_for(&self.pane, path);
            tm.borrow_mut().catalog.forget_docs([id.as_str()], true);
        }
    }

    /// Inactive-tab snapshots (`cachedBuffers`): path, text, dirty.
    pub fn cached_buffers(&self) -> Vec<(String, String, bool)> {
        self.tabs
            .iter()
            .map(|(p, s)| (p.clone(), s.state.doc.to_string(), s.dirty))
            .collect()
    }

    /// Insert a bundle snippet over the selection (keyEquivalent wrap,
    /// `applyBundleSnippet`). Returns the new text.
    pub fn apply_bundle_snippet(
        &mut self,
        sn: &crate::snippet::RegisteredSnippet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> String {
        let sel = self.state.selection.main();
        self.with_dispatcher(window, cx, |d| {
            crate::snippet::apply_snippet(d, sn, sel.from, sel.to, None);
        });
        let next = self.state.doc.to_string();
        self.last_emitted = Some(next.clone());
        next
    }

    /// Apply a snippet the picker chose (`PickSnippet` follow-up).
    pub fn apply_snippet_at(
        &mut self,
        sn: &crate::snippet::RegisteredSnippet,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_dispatcher(window, cx, |d| {
            crate::snippet::apply_snippet(d, sn, from, to, Some(""));
        });
        self.last_emitted = Some(self.state.doc.to_string());
    }

    /// Run `f` with a [`crate::state::Dispatch`] that applies through this
    /// view.
    pub(crate) fn with_dispatcher(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut dyn crate::state::Dispatch),
    ) {
        struct ViewDispatch<'a, 'b> {
            view: &'a mut EditorView,
            window: &'a mut Window,
            cx: &'a mut Context<'b, EditorView>,
        }
        impl crate::state::Dispatch for ViewDispatch<'_, '_> {
            fn state(&self) -> &EditorState {
                &self.view.state
            }
            fn dispatch(&mut self, spec: TransactionSpec) {
                self.view.dispatch(spec, self.window, self.cx);
            }
        }
        let mut d = ViewDispatch {
            view: self,
            window,
            cx,
        };
        f(&mut d);
    }

    /// Apply a bundle command's output with its routing (`applyOutput`).
    pub fn apply_output(
        &mut self,
        routing: &str,
        text: &str,
        input_mode: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let sel = self.state.selection.main();
        let doc_len = self.state.doc.len();
        let replace = |v: &mut EditorView,
                       from: usize,
                       to: usize,
                       insert: &str,
                       select: bool,
                       window: &mut Window,
                       cx: &mut Context<Self>| {
            let insert = crate::text::normalize_newlines(insert).into_owned();
            let end = from + insert.len();
            let sel = if select {
                EditorSelection::single(from, end)
            } else {
                EditorSelection::cursor(end)
            };
            v.dispatch(
                TransactionSpec::default()
                    .changes(crate::change::Change::new(from, to, insert))
                    .selection(sel),
                window,
                cx,
            );
            let next = v.state.doc.to_string();
            v.last_emitted = Some(next.clone());
            Some(next)
        };
        match routing {
            "discard" => None,
            "replaceSelection" | "insertText" => {
                replace(self, sel.from, sel.to, text, true, window, cx)
            }
            "replaceDocument" => replace(self, 0, doc_len, text, false, window, cx),
            "replaceInput" => {
                if sel.from != sel.to {
                    return replace(self, sel.from, sel.to, text, true, window, cx);
                }
                if input_mode == Some("word")
                    && let Some(w) = self.state.word_at(sel.head())
                {
                    return replace(self, w.from, w.to, text, true, window, cx);
                }
                if input_mode == Some("line") {
                    let line = self.state.doc.line_at(sel.head());
                    return replace(self, line.from, line.to, text, true, window, cx);
                }
                replace(self, 0, doc_len, text, false, window, cx)
            }
            "insertSnippet" => {
                let word = (input_mode == Some("word"))
                    .then(|| self.state.word_at(sel.head()).map(|w| (w.from, w.to)))
                    .flatten();
                let line = (input_mode == Some("line")).then(|| {
                    let l = self.state.doc.line_at(sel.head());
                    (l.from, l.to)
                });
                let (from, to) = crate::snippet::command_snippet_range(
                    sel.from, sel.to, input_mode, word, line, doc_len,
                );
                let template = crate::snippet::to_codemirror_snippet(text);
                self.with_dispatcher(window, cx, |d| {
                    crate::snippet::apply_template(d, &template, from, to, false)
                });
                Some(self.state.doc.to_string())
            }
            _ => None,
        }
    }

    /// Recur bundle command context (`getContext`).
    pub fn context(&self) -> EditorContext {
        let state = &self.state;
        let sel = state.selection.main();
        let line = state.doc.line_at(sel.head());
        let indent = self.prefs.resolve_indent(language_id_for_path(&self.path));
        EditorContext {
            selected_text: state.slice_doc(sel.from, sel.to),
            document: state.doc.to_string(),
            current_line: state.doc.slice(line.from, line.to),
            line_number: line.number,
            column: crate::view_state::col_of(&state.doc, sel.head()),
            current_word: state
                .word_at(sel.head())
                .map(|w| state.slice_doc(w.from, w.to))
                .unwrap_or_default(),
            scope: self.scope.clone(),
            tab_size: match indent {
                crate::prefs::EditorIndent::Spaces2 => 2,
                crate::prefs::EditorIndent::Spaces4 => 4,
                crate::prefs::EditorIndent::Tabs => self.prefs.tab_size,
            },
            soft_tabs: indent != crate::prefs::EditorIndent::Tabs,
            path: self.path.clone(),
        }
    }

    /// Current announcement for the accessibility live region.
    pub fn announcement(&self) -> Option<&str> {
        self.announcement.as_deref()
    }

    /// Install or drop the modal keymap for the `keymap` pref.
    pub(crate) fn sync_keymap_mode(&mut self) {
        use crate::prefs::EditorKeymap;
        match self.prefs.keymap {
            EditorKeymap::Emacs => {
                if self.emacs.is_none() {
                    self.emacs = Some(crate::emacs::Emacs::new());
                }
            }
            EditorKeymap::Vim => {
                self.emacs = None;
                crate::vim::enter(self);
            }
            EditorKeymap::Default => self.emacs = None,
        }
        if self.prefs.keymap != EditorKeymap::Vim {
            self.vim = None;
        }
    }

    /// Rebuild the keymap (LSP attached or detached).
    pub(crate) fn rebuild_keymap(&mut self) {
        let lsp = self.lsp.is_some();
        self.keymap = Keymap::build(
            &match self.kind {
                EditorKind::Code => code_editor_keymap(lsp),
                EditorKind::Merge { .. } => merge_editor_keymap(),
            },
            self.platform,
        );
    }

    /// Apply a pending scroll target against the current layout.
    pub(crate) fn resolve_pending_scroll(&mut self, window: &Window) {
        let Some(target) = self.pending_scroll.take() else {
            return;
        };
        let Some(frame) = self.frame.clone() else {
            self.pending_scroll = Some(target);
            return;
        };
        let height = frame.scroller_height;
        if let Some(offset) = target.snapshot_top {
            let n = self
                .state
                .doc
                .line_at(target.range.head().min(self.state.doc.len()))
                .number;
            let top = self.layout.borrow().block_top(n) as f32;
            self.scroll_top = (top + offset).clamp(0.0, frame.max_scroll_top.max(0.0));
            return;
        }
        let (head, anchor) = (target.range.head(), target.range.anchor());
        let (h_top, h_bottom, h_x) = self.with_layout_view(window, |lv| {
            let (t, b) = lv.row_box(head.min(self.state.doc.len()), target.range.assoc());
            (
                t,
                b,
                lv.x_at(head.min(self.state.doc.len()), target.range.assoc()),
            )
        });
        let (a_top, a_bottom) = if anchor != head {
            self.with_layout_view(window, |lv| lv.row_box(anchor.min(self.state.doc.len()), 1))
        } else {
            (h_top, h_bottom)
        };
        let (top, bottom) = (h_top.min(a_top), h_bottom.max(a_bottom));
        let (top, bottom) = if bottom - top > height {
            (h_top, h_bottom)
        } else {
            (top, bottom)
        };
        let m = target.y_margin;
        let new_top = match target.y {
            ScrollStrategy::Start => top - m,
            ScrollStrategy::End => bottom + m - height,
            ScrollStrategy::Center => (top + bottom) / 2.0 - height / 2.0,
            ScrollStrategy::Nearest => {
                if top - m < self.scroll_top {
                    top - m
                } else if bottom + m > self.scroll_top + height {
                    bottom + m - height
                } else {
                    self.scroll_top
                }
            }
        };
        self.scroll_top = new_top.clamp(0.0, frame.max_scroll_top.max(0.0));
        if !self.prefs.line_wrapping {
            let width = frame.scroller_width - frame.gutter_width;
            let x = h_x + self.metrics.line_padding_left;
            let xm = target.x_margin;
            if x - xm < self.scroll_left {
                self.scroll_left = (x - xm - frame.gutter_width.min(0.0)).max(0.0);
            } else if x + xm > self.scroll_left + width {
                self.scroll_left = (x + xm - width).min(frame.max_scroll_left.max(0.0));
            }
        }
    }
}

/// Snapshot for Recur bundle command runners (`EditorContext`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorContext {
    pub selected_text: String,
    pub document: String,
    pub current_line: String,
    pub line_number: usize,
    pub column: usize,
    pub current_word: String,
    pub scope: String,
    pub tab_size: usize,
    pub soft_tabs: bool,
    pub path: String,
}

fn first_line(doc: &str) -> Option<&str> {
    Some(doc.split('\n').next().unwrap_or(""))
}

impl gpui_kit::Render for EditorView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{InteractiveElement, ParentElement, Styled, div};
        let panels = self.render_panels(window, cx);
        let tooltips = self.render_tooltips(window, cx);
        let prompt = self.render_rename_prompt(window, cx);
        let vim_panel = crate::vim::prompt::render_panel(self, window, cx);
        let title = self.render_title_target();
        let content = div()
            .id("cm-scroller")
            .flex_1()
            .min_h_0()
            .relative()
            .track_focus(&self.focus)
            .key_context("RecurEditor")
            .on_key_down(
                cx.guarded_listener(|v, ev: &gpui_kit::KeyDownEvent, window, cx| {
                    if v.key_down(ev, window, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_modifiers_changed(cx.guarded_listener(
                |v, _: &gpui_kit::ModifiersChangedEvent, _, cx| {
                    if v.prefs.rectangular_select {
                        cx.notify();
                    }
                },
            ))
            .child(crate::element::EditorElement { view: cx.entity() });
        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .children(panels)
            .child(content)
            .children(vim_panel)
            .children(tooltips)
            .children(prompt)
            .children(title)
    }
}

impl EditorView {
    /// `coordsAtPos`: the caret box for a position in window coordinates,
    /// when it is inside the visible scroller.
    pub fn coords_at_pos(
        &self,
        pos: usize,
        side: i32,
        window: &Window,
    ) -> Option<gpui_kit::Bounds<Pixels>> {
        let (x, top, bottom) = self.pos_coords(pos, side, window)?;
        Some(gpui_kit::Bounds::new(
            gpui_kit::point(px(x), px(top)),
            gpui_kit::size(px(0.), px(bottom - top)),
        ))
    }

    /// The vertical scroll offset.
    /// The laid-out document height (content padding included); a mini editor's empty doc
    /// counts its placeholder's lines (`.cm-placeholder` is an inline block in line 1).
    pub fn content_height(&self) -> f32 {
        let l = self.layout.borrow();
        let mut h = l.content_height() as f32;
        if let Some(m) = &self.mini
            && self.state.doc.is_empty()
        {
            let lines = m.placeholder.split('\n').count().max(1) as f32;
            h = h.max(lines * self.metrics.line_height() + 2.0 * self.metrics.content_padding_y);
        }
        h
    }

    pub fn scroll_top(&self) -> f32 {
        self.scroll_top
    }
}
