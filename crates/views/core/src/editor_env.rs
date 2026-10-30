//! The app's editor environment: bundle grammars registered with the shared
//! TextMate catalog (read lazily through `BundleService::read_grammar`), the active editor
//! theme parsed from `read_theme` and followed as `AppState::editor_theme` changes, the editor
//! prefs (`recur.editorPrefs`), and the bundle snippet catalog. Editors observe the entity and
//! re-apply on [`EditorEnv::epoch`] changes.

use crate::{ObserveState as _, Topics};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::AppState;
use gpui_kit::{App, AppContext as _, Context, Entity, Global, Subscription, Task};
use recur_editor::highlight::{SharedTextMate, TextMate};
use recur_editor::prefs::EditorPrefs;
use recur_services::Services;
use recur_textmate::{GrammarInfo, ParsedTmTheme, parse_theme_json};

use crate::snippets::BundleSnippets;

pub use crate::prefs::EDITOR_PREFS_KEY;

pub struct EditorEnv {
    services: Services,
    pub tm: SharedTextMate,
    pub snippets: Rc<BundleSnippets>,
    pub prefs: EditorPrefs,
    /// The applied theme id and its parsed theme (`None` until read, or when it fails).
    pub theme_id: String,
    pub theme: Option<Rc<ParsedTmTheme>>,
    /// The CodeMirror bare theme's kind when no parsed theme exists (`uiTheme !== 'light'`).
    pub dark_fallback: bool,
    parsed: HashMap<String, Rc<ParsedTmTheme>>,
    /// Bumps whenever editors should re-resolve (grammars, theme, prefs, syntax overrides).
    pub epoch: u64,
    pub grammars_ready: bool,
    /// Bundle `[[symbol]]` rules for the TextMate outline (`loadRules`).
    pub symbol_rules: Rc<Vec<recur_textmate::SymbolRule>>,
    seen_bundles_epoch: u64,
    seen_syntax_epoch: u64,
    disabled_snippets: Vec<String>,
    boot: Option<Task<()>>,
    theme_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

struct EnvHandle(Entity<EditorEnv>);

impl Global for EnvHandle {}

/// The app's editor environment, created (and bootstrapped) on first use.
pub fn editor_env(state: &Entity<AppState>, cx: &mut App) -> Entity<EditorEnv> {
    if let Some(h) = cx.try_global::<EnvHandle>() {
        return h.0.clone();
    }
    let state = state.clone();
    let env = cx.new(|cx| EditorEnv::new(state, cx));
    cx.set_global(EnvHandle(env.clone()));
    env
}

/// The environment if one exists (no bootstrap).
pub fn editor_env_peek(cx: &App) -> Option<Entity<EditorEnv>> {
    cx.try_global::<EnvHandle>().map(|h| h.0.clone())
}

/// The editor prefs (defaults before the environment exists).
pub fn editor_prefs(cx: &App) -> EditorPrefs {
    cx.try_global::<EnvHandle>()
        .map(|h| h.0.read(cx).prefs.clone())
        .unwrap_or_default()
}

fn grammar_info(g: &recur_services::api::GrammarInfo) -> Option<GrammarInfo> {
    serde_json::to_value(g)
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
}

impl EditorEnv {
    /// The syntax-mode overrides' epoch the environment last applied.
    pub fn syntax_epoch(&self) -> u64 {
        self.seen_syntax_epoch
    }

    fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let s = state.read(cx);
        let services = s.services().clone();
        let prefs = s.editor_prefs.clone();
        let mut env = EditorEnv {
            snippets: Rc::new(BundleSnippets::new(services.clone())),
            services,
            tm: Rc::new(RefCell::new(TextMate::default())),
            prefs,
            theme_id: String::new(),
            theme: None,
            dark_fallback: true,
            parsed: HashMap::new(),
            epoch: 0,
            grammars_ready: false,
            symbol_rules: Rc::default(),
            seen_bundles_epoch: s.bundles_epoch,
            seen_syntax_epoch: 0,
            disabled_snippets: Vec::new(),
            boot: None,
            theme_task: None,
            _subs: Vec::new(),
        };
        env._subs
            .push(cx.observe_state(&state, Topics::ALL, |env, state, cx| {
                env.state_changed(&state, cx)
            }));
        env.sync_disabled_snippets(&state, cx);
        env.load_grammars(false, &state, cx);
        env.state_changed(&state, cx);
        env
    }

    fn bump(&mut self, cx: &mut Context<Self>) {
        self.epoch += 1;
        cx.notify();
    }

    fn sync_disabled_snippets(&mut self, state: &Entity<AppState>, cx: &App) {
        let ids = state
            .read(cx)
            .settings
            .as_ref()
            .map(|s| s.disabled_bundle_snippets.clone())
            .unwrap_or_default();
        if ids != self.disabled_snippets {
            self.snippets.set_disabled(ids.clone());
            self.disabled_snippets = ids;
        }
    }

    fn state_changed(&mut self, state: &Entity<AppState>, cx: &mut Context<Self>) {
        self.sync_disabled_snippets(state, cx);
        self.reload_prefs(state, cx);
        let s = state.read(cx);
        if s.bundles_epoch != self.seen_bundles_epoch {
            self.seen_bundles_epoch = s.bundles_epoch;
            self.snippets.invalidate();
            self.parsed.clear();
            self.theme_id.clear();
            self.load_grammars(true, state, cx);
        }
        let key = state.read(cx).editor_theme.clone();
        if key != self.theme_id {
            self.load_theme(key, state, cx);
        }
        let syntax = self.tm.borrow().catalog.syntax_mode.epoch();
        if syntax != self.seen_syntax_epoch {
            self.seen_syntax_epoch = syntax;
            self.bump(cx);
        }
    }

    /// `ensureTextMateBootstrap`: register every bundle grammar; their text loads on first use.
    fn load_grammars(&mut self, reload: bool, state: &Entity<AppState>, cx: &mut Context<Self>) {
        let services = self.services.clone();
        let state = state.downgrade();
        self.boot = Some(cx.spawn(async move |this, cx| {
            if let Ok(list) = services.bundle.list_symbols().await {
                let rules: Vec<recur_textmate::SymbolRule> = list
                    .into_iter()
                    .filter(|r| !r.scope.is_empty())
                    .map(|r| recur_textmate::SymbolRule {
                        id: r.id,
                        scope: r.scope,
                        show: r.show,
                        transform: r.transform,
                        kind: r.kind,
                    })
                    .collect();
                let _ = this.update(cx, |env, _| env.symbol_rules = Rc::new(rules));
            }
            let grammars = match services.bundle.list_grammars().await {
                Ok(g) => g,
                Err(err) => {
                    tracing::warn!("ListGrammars failed: {}", err.message);
                    if reload {
                        return;
                    }
                    Vec::new()
                }
            };
            // Every grammar is read here, off the UI thread and concurrently, before any is
            // registered: the catalog's loader is then a lookup, and a grammar that embeds
            // another finds it when it compiles.
            let texts = futures::future::join_all(
                grammars
                    .iter()
                    .map(|g| services.bundle.read_grammar(&g.scope_name)),
            )
            .await;
            let _ = this.update(cx, |env, cx| {
                {
                    let mut tm = env.tm.borrow_mut();
                    if reload {
                        tm.catalog.forget_bundle_contribs();
                    }
                    for (g, text) in grammars.iter().zip(texts) {
                        let Some(info) = grammar_info(g) else {
                            continue;
                        };
                        let text = text.ok();
                        tm.catalog.add_grammar(info, Rc::new(move || text.clone()));
                    }
                }
                env.grammars_ready = true;
                if reload && let Some(state) = state.upgrade() {
                    let key = state.read(cx).editor_theme.clone();
                    env.load_theme(key, &state, cx);
                }
                env.bump(cx);
            });
        }));
    }

    /// `editorThemeExtension(key)`: read and parse the theme, then activate it.
    fn load_theme(&mut self, key: String, state: &Entity<AppState>, cx: &mut Context<Self>) {
        self.theme_id = key.clone();
        let ui_light = state
            .read(cx)
            .editor_theme_catalog
            .iter()
            .find(|t| t.id == key)
            .map(|t| t.ui_theme == "light");
        if let Some(parsed) = self.parsed.get(&key).cloned() {
            self.activate(Some(parsed), cx);
            return;
        }
        self.dark_fallback = ui_light.is_none_or(|l| !l);
        let services = self.services.clone();
        self.theme_task = Some(cx.spawn(async move |this, cx| {
            let parsed = match services.bundle.read_theme(&key).await {
                Ok(text) => parse_theme_json(&text, &key).ok().map(Rc::new),
                Err(_) => None,
            };
            let _ = this.update(cx, |env, cx| {
                if env.theme_id != key {
                    return;
                }
                if let Some(p) = &parsed {
                    env.parsed.insert(key.clone(), p.clone());
                }
                env.activate(parsed, cx);
            });
        }));
    }

    fn activate(&mut self, parsed: Option<Rc<ParsedTmTheme>>, cx: &mut Context<Self>) {
        if let Some(p) = &parsed {
            self.tm.borrow_mut().activate_theme(p.clone());
            self.dark_fallback = p.ui_theme == recur_textmate::UiTheme::Dark;
        }
        self.theme = parsed;
        self.bump(cx);
    }

    /// Follow `AppState::editor_prefs` (Settings → Code editor changed them).
    pub fn reload_prefs(&mut self, state: &Entity<AppState>, cx: &mut Context<Self>) {
        let next = state.read(cx).editor_prefs.clone();
        if next != self.prefs {
            self.prefs = next;
            self.bump(cx);
        }
    }

    /// Syntax overrides moved (the status bar picker, a restored session).
    pub fn syntax_changed(&mut self, cx: &mut Context<Self>) {
        self.seen_syntax_epoch = self.tm.borrow().catalog.syntax_mode.epoch();
        self.bump(cx);
    }

    /// `applyLanguageMode`: the language picker's per-file pick (`None` auto-detects).
    pub fn apply_language_mode(&mut self, path: &str, scope: Option<&str>, cx: &mut Context<Self>) {
        self.tm
            .borrow_mut()
            .catalog
            .apply_language_mode(path, scope);
        self.syntax_changed(cx);
    }

    /// The picker's "Always open *.ext as…": associate the file type (`None` clears it) and drop
    /// `path`'s own override so the association applies.
    pub fn set_file_type_language(
        &mut self,
        key: &str,
        scope: Option<&str>,
        path: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        {
            let mut tm = self.tm.borrow_mut();
            let modes = &mut tm.catalog.syntax_mode;
            modes.set_for_file_type(key, scope);
            if let Some(p) = path {
                modes.set_for_path(p, None);
            }
        }
        self.syntax_changed(cx);
    }

    /// The language label for the status bar (`languageLabelForPath`).
    pub fn language_label(&self, path: &str) -> String {
        self.tm.borrow().catalog.language_label_for_path(path)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use gpui::TestAppContext;
    use recur_services::Result;
    use recur_services::fake::{Fake, Overrides, Release, hold};

    use super::*;
    use crate::LocalStorage;

    /// Grammar reads are in flight while the UI thread runs on: nothing blocks on them, and
    /// the grammars register once every read has landed.
    #[gpui::test]
    fn grammars_load_without_blocking_the_ui_thread(cx: &mut TestAppContext) {
        type Held = Vec<(String, Release<Result<String>>)>;
        let held: Arc<Mutex<Held>> = Arc::default();
        let h = held.clone();
        let fake = Fake::fresh(Overrides::default().read_grammar(move |_, scope| {
            let (answer, release) = hold();
            h.lock().unwrap().push((scope, release));
            answer
        }));
        let env = cx.update(|cx| {
            let state = AppState::install(fake.services(), LocalStorage::in_memory(), cx);
            editor_env(&state, cx)
        });
        cx.run_until_parked();
        let pending = std::mem::take(&mut *held.lock().unwrap());
        assert!(!pending.is_empty(), "every grammar is read up front");
        assert!(!env.read_with(cx, |e, _| e.grammars_ready));
        let first = pending[0].0.clone();
        for (scope, release) in pending {
            release.send(Ok(format!(
                "{{\"scopeName\": \"{scope}\", \"patterns\": []}}"
            )));
        }
        cx.run_until_parked();
        assert!(env.read_with(cx, |e, _| e.grammars_ready));
        let loaded = env.update(cx, |e, _| e.tm.borrow_mut().catalog.load_grammar(&first));
        assert!(loaded, "{first} compiles from the text read up front");
    }
}
