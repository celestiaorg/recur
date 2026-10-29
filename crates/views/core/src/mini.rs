//! `MiniScriptEditor.svelte`: the compact CodeMirror surface for setup / teardown scripts,
//! run commands and dotenv. Themed with the bundle theme paired to the app chrome
//! (`editorThemeIdForAppChrome`), highlighted by its own TextMate engine so the code editor's
//! theme is untouched (`ensureChromePairedTmEngine`), on `recur_editor`'s mini chrome.

use recur_ui::guard::Guarded;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::AppState;
use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable, Global, Hsla, IntoElement,
    ParentElement, Render, Styled, Subscription, Task, Window, div, px,
};
use recur_editor::highlight::{SharedTextMate, TextMate};
use recur_editor::theme::MiniChrome;
use recur_editor::theme::catalog::editor_theme_id_for_app_chrome;
use recur_editor::view::{EditorEvent, EditorOptions, EditorView};
use recur_services::Services;
use recur_textmate::{GrammarInfo, ParsedTmTheme, parse_theme_json};
use recur_ui::{Theme, rgba};

/// `lang`: which grammar and font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiniLang {
    /// `source.shell` (`setup.sh`).
    Shell,
    /// `source.dotenv` (`.env` is a basename match).
    Env,
    /// Markdown prose (`plan.md`).
    Md,
}

impl MiniLang {
    fn path(self) -> &'static str {
        match self {
            MiniLang::Shell => "setup.sh",
            MiniLang::Env => ".env",
            MiniLang::Md => "plan.md",
        }
    }
}

/// The paired theme and its engine.
#[derive(Clone)]
pub struct Paired {
    pub theme: Option<Rc<ParsedTmTheme>>,
    pub tm: SharedTextMate,
}

type PairedLoad = Shared<Task<Paired>>;

/// Paired engines by theme id, and the bundle grammar list they all register.
#[derive(Default)]
struct MiniEnv {
    paired: HashMap<String, PairedLoad>,
    bundles_epoch: u64,
}

impl Global for MiniEnv {}

fn grammar_info(g: &recur_services::api::GrammarInfo) -> Option<GrammarInfo> {
    serde_json::to_value(g)
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
}

/// `ensureChromePairedTmEngine(themeId)` with the parsed theme.
fn paired_engine(state: &Entity<AppState>, cx: &mut App) -> (String, PairedLoad) {
    let s = state.read(cx);
    let app_key = Theme::of(cx).id.key().to_string();
    let disabled = s
        .settings
        .as_ref()
        .map(|x| x.disabled_bundle_themes.clone())
        .unwrap_or_default();
    let catalog = s.editor_theme_catalog.clone();
    let id = editor_theme_id_for_app_chrome(&app_key, |id| {
        catalog.iter().any(|t| t.id == id) && !disabled.iter().any(|d| d == id)
    });
    let epoch = s.bundles_epoch;
    let services: Services = s.services().clone();
    let env = cx.default_global::<MiniEnv>();
    if env.bundles_epoch != epoch {
        env.paired.clear();
        env.bundles_epoch = epoch;
    }
    if let Some(load) = env.paired.get(&id) {
        return (id, load.clone());
    }
    let key = id.clone();
    let task: Task<Paired> = cx.spawn(async move |_| {
        let tm: SharedTextMate = Rc::new(RefCell::new(TextMate::default()));
        if let Ok(grammars) = services.bundle.list_grammars().await {
            let mut t = tm.borrow_mut();
            for g in &grammars {
                let Some(info) = grammar_info(g) else {
                    continue;
                };
                let scope = g.scope_name.clone();
                let svc = services.clone();
                let cache: Rc<RefCell<Option<Option<String>>>> = Rc::default();
                t.catalog.add_grammar(
                    info,
                    Rc::new(move || {
                        if let Some(text) = cache.borrow().clone() {
                            return text;
                        }
                        // simplify: the catalog's loader is synchronous (as the Observatory's),
                        // so a grammar read blocks once per scope. Upgrade: an async loader in
                        // recur_textmate.
                        let text =
                            futures::executor::block_on(svc.bundle.read_grammar(&scope)).ok();
                        *cache.borrow_mut() = Some(text.clone());
                        text
                    }),
                );
            }
        }
        let theme = match services.bundle.read_theme(&key).await {
            Ok(text) => parse_theme_json(&text, &key).ok().map(Rc::new),
            Err(_) => None,
        };
        if let Some(p) = &theme {
            tm.borrow_mut().activate_theme(p.clone());
        }
        Paired { theme, tm }
    });
    let load = task.shared();
    cx.default_global::<MiniEnv>()
        .paired
        .insert(id.clone(), load.clone());
    (id, load)
}

/// The props.
#[derive(Clone, Debug, PartialEq)]
pub struct MiniProps {
    pub lang: MiniLang,
    pub placeholder: String,
    pub min_lines: u32,
    pub max_lines: u32,
    pub fill: bool,
    pub readonly: bool,
}

impl Default for MiniProps {
    fn default() -> Self {
        MiniProps {
            lang: MiniLang::Shell,
            placeholder: String::new(),
            min_lines: 4,
            max_lines: 16,
            fill: false,
            readonly: false,
        }
    }
}

/// `onChange(value)`.
#[derive(Clone, Debug, PartialEq)]
pub struct MiniChange(pub String);

pub struct MiniScriptEditor {
    state: Entity<AppState>,
    props: MiniProps,
    editor: Entity<EditorView>,
    last_emitted: String,
    applying: bool,
    theme_id: String,
    app_theme: String,
    _load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<MiniChange> for MiniScriptEditor {}

/// The mini editor's prefs: line numbers and wrapping, nothing else CodeMirror's basic setup
/// would add (`MiniScriptEditor`'s extension list).
fn mini_prefs(base: &recur_editor::prefs::EditorPrefs) -> recur_editor::prefs::EditorPrefs {
    recur_editor::prefs::EditorPrefs {
        line_numbers: true,
        line_wrapping: true,
        active_line: false,
        fold_gutter: false,
        diff_gutter: false,
        bracket_matching: false,
        special_chars: false,
        scroll_past_end: false,
        ruler_column: 0,
        whitespace: recur_editor::prefs::EditorWhitespace::Off,
        keymap: recur_editor::prefs::EditorKeymap::Default,
        ..base.clone()
    }
}

impl MiniScriptEditor {
    pub fn new(
        state: &Entity<AppState>,
        value: &str,
        props: MiniProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mini = MiniChrome {
            prose: props.lang == MiniLang::Md,
            fill: props.fill,
            placeholder: props.placeholder.clone(),
        };
        let base = state.read(cx).editor_prefs.clone();
        // Each editor its own highlighter doc: the setup, teardown and run editors share a path.
        let pane = format!("mini-{}", cx.entity_id());
        let editor = cx.new(|cx| {
            let mut opts = EditorOptions::new(props.lang.path(), value);
            opts.prefs = mini_prefs(&base);
            opts.read_only = props.readonly;
            opts.mini = Some(mini);
            opts.pane = pane;
            EditorView::new(opts, window, cx)
        });
        let subs = vec![
            cx.guarded_subscribe(&editor, |this: &mut Self, _, ev: &EditorEvent, cx| {
                if let EditorEvent::DocChanged(text) = ev
                    && !this.applying
                {
                    this.last_emitted = text.clone();
                    cx.emit(MiniChange(text.clone()));
                    // Only a growing editor's box follows its content.
                    if !this.props.fill {
                        cx.notify();
                    }
                }
            }),
            cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
        ];
        let mut me = MiniScriptEditor {
            state: state.clone(),
            props,
            editor,
            last_emitted: value.to_string(),
            applying: false,
            theme_id: String::new(),
            app_theme: String::new(),
            _load: None,
            _subs: subs,
        };
        me.apply_paired_theme(window, cx);
        me
    }

    /// `applyPairedTheme`: follow the app chrome's paired theme (and its engine).
    fn apply_paired_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = Theme::of(cx).id.key().to_string();
        let (id, load) = paired_engine(&self.state, cx);
        if id == self.theme_id && app == self.app_theme {
            return;
        }
        self.theme_id = id;
        self.app_theme = app;
        let editor = self.editor.clone();
        self._load = Some(cx.spawn_in(window, async move |_, cx| {
            let paired = load.await;
            let _ = cx.update(|window, cx| {
                editor.update(cx, |e, cx| {
                    e.set_textmate(Some(paired.tm.clone()), window, cx);
                    let dark = paired
                        .theme
                        .as_ref()
                        .is_none_or(|p| p.ui_theme == recur_textmate::UiTheme::Dark);
                    e.set_theme(paired.theme.clone(), dark, window, cx);
                });
            });
        }));
    }

    /// `value={…}`: an external reseed (another project) replaces the doc; the echo of the
    /// user's own typing does not.
    pub fn set_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        if value == self.last_emitted {
            return;
        }
        let cur = self.editor.read(cx).peek_doc();
        self.last_emitted = value.to_string();
        if cur == value {
            return;
        }
        self.applying = true;
        self.editor
            .update(cx, |e, cx| e.replace_doc(value, window, cx));
        self.applying = false;
    }

    pub fn set_readonly(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.props.readonly != on {
            self.props.readonly = on;
            self.editor
                .update(cx, |e, cx| e.set_read_only(on, window, cx));
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let h = self.editor.focus_handle(cx);
        window.focus(&h, cx);
    }

    pub fn value(&self, cx: &App) -> String {
        self.editor.read(cx).peek_doc()
    }

    /// The editor's border-box height: the content clamped to `minLines`..`maxLines`.
    fn height(&self, cx: &App) -> f32 {
        let line = MiniChrome::LINE_PX;
        let pad_y = 16.;
        let min = self.props.min_lines as f32 * line + pad_y;
        let max = self.props.max_lines as f32 * line + pad_y;
        let content = self.editor.read(cx).content_height().max(min - 2.);
        content.clamp(min, max)
    }
}

impl MiniScriptEditor {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.apply_paired_theme(window, cx);
        let t = Theme::of(cx).tokens;
        let focused = self.editor.focus_handle(cx).contains_focused(window, cx);
        let ro = self.props.readonly;
        // `.mini-ed:focus-within { border-color: rgba(242, 179, 61, .5) }` (not when read-only).
        let border = if focused && !ro {
            rgba(242, 179, 61, 0.5)
        } else {
            t.line
        };
        // A filled editor's box never follows its content, so it is isolated: a key (or a caret
        // blink) renders the editor alone instead of every view up to the window root.
        if self.props.fill {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .child(recur_ui::style::isolated_view(
                    &self.editor,
                    gpui_kit::StyleRefinement::default().size_full(),
                    cx,
                ))
                .into_any_element();
        }
        let h = self.height(cx);
        div()
            .w_full()
            .flex_none()
            .rounded(px(6.))
            .border(px(1.))
            .border_color(Hsla::from(border))
            .bg(t.pane)
            .overflow_hidden()
            .when(ro, |d| d.cursor_default())
            .child(div().w_full().h(px(h)).child(self.editor.clone()))
            .into_any_element()
    }
}

impl Render for MiniScriptEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
