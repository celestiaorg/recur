//! Settings: the split nav, the header and the active pane, with the panes' state,
//! their loads (keyed on the active section and the values they read) and
//! `patchSettings`.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::{BTreeMap, HashMap, HashSet};

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollHandle, Styled, Subscription, Task, Window, div,
};
use recur_domain::Settings;
use recur_services::AppEvent;
use recur_services::api::{
    BundleSettingInfo, BundleSkipInfo, ClaudeStatus, CodexStatus, CursorStatus, GrammarInfo,
    PromptActionInfo, ThemeInfo,
};
use recur_ui::widgets::{Select, SelectEvent, ToastKind};
use recur_view_chat::pickers::{LoopPolicyEvent, LoopPolicyPickers, ModelPickers, PickerEvent};
use recur_view_core::commands::RegisteredCommand;
use recur_view_core::models::{CatalogModel, provider_enabled};
use recur_view_core::{ViewContext, ViewName, call, toast};

use crate::compatible::card::{CardEvent, CompatibleProviderCard};
use crate::compatible::modal::{CompatibleProviderModal, ModalEvent};
use crate::field::{Field, FieldEvent, FieldOpts};
use crate::mono_fonts::MonoFont;
use crate::nav::{Section, filtered_nav, section_after_filter};
use crate::rules::{
    PROVIDER_CHATGPT, PROVIDER_CLAUDE, PROVIDER_CODEX, PROVIDER_CURSOR, compatible_configured,
};
use crate::style::{hsla, tokens};

/// A list loaded for a pane: rows, the loading flag and the error text.
pub struct Loadable<T> {
    pub rows: Vec<T>,
    pub loading: bool,
    pub err: String,
}

impl<T> Default for Loadable<T> {
    fn default() -> Self {
        Loadable {
            rows: Vec::new(),
            loading: false,
            err: String::new(),
        }
    }
}

/// `{ ok, text }` under a Test button.
pub type TestMsg = Option<(bool, String)>;

/// `editorThemeKindFilter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    All,
    Dark,
    Light,
}

/// The chat-mode model slots plus the workspace title (`ModeSlot`).
pub const MODEL_SLOTS: [&str; 6] = ["ask", "plan", "agent", "review", "loop", "title"];

/// The Models effect's key: provider readiness and toggles, and the catalog revision.
pub(crate) type ModelsKey = (bool, bool, bool, bool, bool, u64);

/// Effects, by what they read: one rerun per change of the key, cancelled (by
/// generation) when the key changes again.
#[derive(Default)]
pub(crate) struct Effects {
    pub prefetch: Option<bool>,
    pub claude: Option<bool>,
    pub cursor: Option<Option<String>>,
    pub codex: Option<Option<String>>,
    pub codex_mounted: Option<bool>,
    pub chatgpt: Option<bool>,
    pub models: Option<Option<ModelsKey>>,
    pub section: Option<Section>,
    pub codex_key: Option<(bool, String)>,
    pub bundle_drafts: Option<(BTreeMap<String, String>, String, Vec<String>)>,
    pub loops_settings: Option<Option<Settings>>,
    pub editor_theme: Option<(String, usize)>,
    pub gens: HashMap<&'static str, u64>,
}

impl Effects {
    /// A new generation for `name`; older tasks see theirs is stale.
    pub fn bump(&mut self, name: &'static str) -> u64 {
        let g = self.gens.entry(name).or_default();
        *g += 1;
        *g
    }
    pub fn live(&self, name: &'static str, g: u64) -> bool {
        self.gens.get(name).copied() == Some(g)
    }
}

/// The MCP add / edit form.
#[derive(Default)]
pub struct McpForm {
    /// `''`, `http` or `stdio`.
    pub kind: String,
    pub edit_id: String,
    pub err: String,
}

pub struct SettingsView {
    pub(crate) vcx: ViewContext,
    pub(crate) focus: FocusHandle,
    pub(crate) fields: HashMap<String, Field>,
    pub(crate) nav_scroll: ScrollHandle,
    pub(crate) main_scroll: ScrollHandle,
    pub(crate) fx: Effects,
    /// What each child entity was last given, so a render only pushes changed props.
    pub(crate) sigs: HashMap<String, u64>,

    // Providers
    pub(crate) claude_status: Option<ClaudeStatus>,
    pub(crate) claude_busy: bool,
    pub(crate) cursor_status: Option<CursorStatus>,
    pub(crate) cursor_usage_busy: bool,
    pub(crate) codex_status: Option<CodexStatus>,
    pub(crate) codex_busy: bool,
    pub(crate) codex_key_dirty: bool,
    pub(crate) codex_key_visible: bool,
    pub(crate) codex_key_test: TestMsg,
    pub(crate) codex_key_testing: bool,
    pub(crate) installing: bool,
    pub(crate) install_msg: String,
    pub(crate) install_pct: i64,
    pub(crate) compat_cards: HashMap<String, Entity<CompatibleProviderCard>>,
    pub(crate) compat_test: HashMap<String, TestMsg>,
    pub(crate) compat_testing: HashMap<String, bool>,
    pub(crate) compat_modal: Option<Entity<CompatibleProviderModal>>,
    pub(crate) chatgpt: crate::sections::openai::ChatGPTCard,
    pub(crate) chatgpt_method: Entity<Select>,
    pub(crate) chatgpt_account: Entity<Select>,
    pub(crate) codex_login: crate::sections::openai::CodexLogin,

    // Models
    pub(crate) catalog_models: Vec<CatalogModel>,
    pub(crate) model_pickers: Vec<Entity<ModelPickers>>,

    // Loops
    pub(crate) catalog_loops: Vec<recur_engine::view::loops::LoopInfo>,
    pub(crate) catalog_policies: Vec<recur_engine::view::loops::PolicyInfo>,
    pub(crate) default_loop: String,
    pub(crate) default_policy: String,
    pub(crate) loop_pickers: Entity<LoopPolicyPickers>,

    // General
    pub(crate) github_login: String,
    pub(crate) branch_prefix_dirty: bool,
    pub(crate) skip_prefix_commit: bool,

    // Appearance
    pub(crate) theme_kind: ThemeKind,
    pub(crate) edth_scrolls: HashMap<&'static str, ScrollHandle>,
    pub(crate) edth_scroll_pending: bool,

    // Code editor
    pub(crate) mono_fonts: Vec<MonoFont>,
    pub(crate) mono_loading: bool,
    pub(crate) font_select: Entity<Select>,
    pub(crate) term_font_select: Entity<Select>,

    // Skills
    pub(crate) skills: Loadable<recur_code::view::skills::SkillRow>,

    // MCP
    pub(crate) mcp: McpForm,

    // Bundles
    pub(crate) cmds: Loadable<RegisteredCommand>,
    pub(crate) skips: Vec<BundleSkipInfo>,
    pub(crate) prompts: Loadable<PromptActionInfo>,
    pub(crate) grammars: Loadable<GrammarInfo>,
    pub(crate) themes: Loadable<ThemeInfo>,
    pub(crate) snippets: Loadable<recur_editor::snippet::RegisteredSnippet>,
    pub(crate) lsp: Loadable<recur_services::api::LanguageServerInfo>,

    // Integrations
    pub(crate) bundle_settings: Loadable<BundleSettingInfo>,
    pub(crate) bundle_focus: String,
    pub(crate) bundle_drafts: HashMap<String, String>,
    pub(crate) bundle_visible: HashSet<String>,
    pub(crate) bundle_save: Option<Task<()>>,

    // Data
    pub(crate) logs_path: String,
    pub(crate) clearing: bool,
    pub(crate) confirming_clear: bool,

    pub(crate) _subs: Vec<Subscription>,
    pub(crate) _events: Task<()>,
}

/// The Settings view of the app (one per window), for galleries and specs.
pub struct CurrentSettings(pub gpui_kit::WeakEntity<SettingsView>);

impl gpui_kit::Global for CurrentSettings {}

impl SettingsView {
    /// The live Settings view, once the shell has built it.
    pub fn current(cx: &App) -> Option<Entity<SettingsView>> {
        cx.try_global::<CurrentSettings>()
            .and_then(|c| c.0.upgrade())
    }
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsView {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = vcx.state.clone();
        let mut model_pickers = Vec::new();
        for slot in MODEL_SLOTS {
            let (ml, fl) = crate::sections::models::picker_labels(slot);
            let p = cx.new(|cx| {
                let mut p = ModelPickers::with_labels(&ml, &fl, true, false, window, cx);
                // The ghost triggers here use the mono chip font.
                p.set_trigger_style(
                    recur_ui::widgets::TriggerStyle {
                        mono_font: true,
                        ..Default::default()
                    },
                    cx,
                );
                p
            });
            model_pickers.push(p);
        }
        let loop_pickers = cx.new(|cx| LoopPolicyPickers::new(false, false, true, window, cx));
        let font_select = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Editor font")
                .menu_variant(recur_ui::logic::menu_space::MenuVariant::Font)
        });
        let term_font_select = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Terminal font")
                .menu_variant(recur_ui::logic::menu_space::MenuVariant::Font)
        });
        let chatgpt_method = cx.new(|cx| {
            Select::new(
                vec![
                    recur_ui::widgets::SelectOption::new("browser", "Browser"),
                    recur_ui::widgets::SelectOption::new("headless", "Headless (device code)"),
                ],
                window,
                cx,
            )
            .label("ChatGPT sign-in method")
        });
        let chatgpt_account =
            cx.new(|cx| Select::new(Vec::new(), window, cx).label("ChatGPT sign-in account"));
        let mut subs =
            vec![
                cx.observe_state_in(&state, Topics::ALL, window, |this, _, window, cx| {
                    this.sync(window, cx);
                    cx.notify();
                }),
            ];
        for (i, p) in model_pickers.iter().enumerate() {
            subs.push(
                cx.guarded_subscribe(p, move |this: &mut Self, _, ev: &PickerEvent, cx| {
                    let slot = MODEL_SLOTS[i];
                    match ev {
                        PickerEvent::Model(r, f) => this.set_model(slot, r.clone(), f.clone(), cx),
                        PickerEvent::Flags(f) => this.set_flags(slot, f.clone(), cx),
                    }
                }),
            );
        }
        subs.push(cx.guarded_subscribe(
            &loop_pickers,
            |this: &mut Self, _, ev: &LoopPolicyEvent, cx| match ev {
                LoopPolicyEvent::Loop(id) => {
                    let p = this.default_policy.clone();
                    this.persist_loop_policy(id.clone(), p, cx)
                }
                LoopPolicyEvent::Policy(id) => {
                    let l = this.default_loop.clone();
                    this.persist_loop_policy(l, id.clone(), cx)
                }
            },
        ));
        subs.push(cx.guarded_subscribe(
            &font_select,
            |this: &mut Self, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    let v = v.to_string();
                    this.set_editor_prefs(|p| p.font_face = v, cx);
                }
            },
        ));
        subs.push(cx.guarded_subscribe(
            &term_font_select,
            |this: &mut Self, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    let v = v.to_string();
                    this.vcx
                        .state
                        .update(cx, |s, cx| s.set_terminal_prefs(|p| p.font_face = v, cx));
                }
            },
        ));
        subs.push(cx.guarded_subscribe(
            &chatgpt_method,
            |this: &mut Self, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    this.chatgpt.method = v.to_string();
                    cx.notify();
                }
            },
        ));
        subs.push(cx.guarded_subscribe(
            &chatgpt_account,
            |this: &mut Self, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    this.chatgpt.account = v.to_string();
                    cx.notify();
                }
            },
        ));
        // `cursor.install` progress while InstallCursorBridge runs.
        let mut events = vcx.services.subscribe();
        let events_task = cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let AppEvent::CursorInstall(d) = ev else {
                    continue;
                };
                if this
                    .update(cx, |v, cx| {
                        v.install_msg = d.message.clone();
                        v.install_pct = d.percent;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let (default_loop, default_policy) = {
            let s = state.read(cx).settings.as_ref();
            (
                recur_engine::view::loops::default_loop_id(s),
                recur_engine::view::loops::default_policy_id(s),
            )
        };
        let mut v = SettingsView {
            vcx: vcx.clone(),
            focus: cx.focus_handle(),
            fields: HashMap::new(),
            nav_scroll: ScrollHandle::new(),
            main_scroll: ScrollHandle::new(),
            fx: Effects::default(),
            sigs: HashMap::new(),
            claude_status: None,
            claude_busy: false,
            cursor_status: None,
            cursor_usage_busy: false,
            codex_status: None,
            codex_busy: false,
            codex_key_dirty: false,
            codex_key_visible: false,
            codex_key_test: None,
            codex_key_testing: false,
            installing: false,
            install_msg: String::new(),
            install_pct: -1,
            compat_cards: HashMap::new(),
            compat_test: HashMap::new(),
            compat_testing: HashMap::new(),
            compat_modal: None,
            chatgpt: crate::sections::openai::ChatGPTCard {
                method: "browser".into(),
                ..Default::default()
            },
            chatgpt_method,
            chatgpt_account,
            codex_login: Default::default(),
            catalog_models: state.read(cx).peek_model_catalog().to_vec(),
            model_pickers,
            catalog_loops: recur_engine::view::loops::fallback_loops(),
            catalog_policies: recur_engine::view::loops::fallback_policies(),
            default_loop,
            default_policy,
            loop_pickers,
            github_login: String::new(),
            branch_prefix_dirty: false,
            skip_prefix_commit: false,
            theme_kind: ThemeKind::All,
            edth_scrolls: HashMap::new(),
            edth_scroll_pending: false,
            mono_fonts: Vec::new(),
            mono_loading: false,
            font_select,
            term_font_select,
            skills: Loadable::default(),
            mcp: McpForm::default(),
            cmds: Loadable::default(),
            skips: Vec::new(),
            prompts: Loadable::default(),
            grammars: Loadable::default(),
            themes: Loadable::default(),
            snippets: Loadable::default(),
            lsp: Loadable::default(),
            bundle_settings: Loadable::default(),
            bundle_focus: String::new(),
            bundle_drafts: HashMap::new(),
            bundle_visible: HashSet::new(),
            bundle_save: None,
            logs_path: String::new(),
            clearing: false,
            confirming_clear: false,
            _subs: subs,
            _events: events_task,
        };
        let me = cx.entity().downgrade();
        cx.set_global(CurrentSettings(me));
        v.sync(window, cx);
        v
    }

    pub(crate) fn settings<'a>(&self, cx: &'a App) -> Option<&'a Settings> {
        self.vcx.state.read(cx).settings.as_ref()
    }

    /// `$settingsSection` (an unknown id renders no pane).
    pub(crate) fn section(&self, cx: &App) -> Option<Section> {
        Section::from_id(&self.vcx.state.read(cx).settings_section)
    }

    pub(crate) fn set_section(&self, s: Section, cx: &mut App) {
        self.vcx.state.update(cx, |st, cx| {
            if st.settings_section != s.id() {
                st.settings_section = s.id().to_string();
                cx.notify();
            }
        });
    }

    // ---- derived provider state ----

    pub(crate) fn disabled_providers(&self, cx: &App) -> Vec<String> {
        self.settings(cx)
            .map(|s| s.disabled_providers.clone())
            .unwrap_or_default()
    }

    pub(crate) fn provider_on(&self, slug: &str, cx: &App) -> bool {
        provider_enabled(&self.disabled_providers(cx), slug)
    }

    /// `cursorSignedIn` (= `cursorConfigured`, `cursorReady`): the usage row came from Connect.
    pub(crate) fn cursor_signed_in(&self, cx: &App) -> bool {
        recur_view_core::provider_usage::can_disconnect_cursor(
            self.vcx.state.read(cx).provider_usage.as_ref(),
        )
    }

    /// `cursorUsage`: the Cursor row of the usage snapshot.
    pub(crate) fn cursor_usage<'a>(
        &self,
        cx: &'a App,
    ) -> Option<&'a recur_services::api::ProviderQuota> {
        self.vcx
            .state
            .read(cx)
            .provider_usage
            .as_ref()
            .and_then(|s| s.providers.iter().flatten().find(|p| p.id == "cursor"))
    }

    pub(crate) fn codex_configured(&self, cx: &App) -> bool {
        self.settings(cx)
            .is_some_and(|s| !s.codex_api_key.is_empty() || s.codex_chat_gpt_auth)
    }

    /// `extraReady`: configured compatible provider slugs, and ChatGPT once connected with
    /// models.
    pub(crate) fn extra_ready(&self, cx: &App) -> Vec<String> {
        let mut out: Vec<String> = self
            .settings(cx)
            .map(|s| {
                s.compatible_providers
                    .iter()
                    .filter(|p| compatible_configured(p))
                    .map(|p| p.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if self.chatgpt_ready() {
            out.push(PROVIDER_CHATGPT.into());
        }
        out
    }

    // ---- effects ----

    /// Re-run every `$effect` whose inputs changed.
    pub(crate) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let section = self.section(cx);
        let agent = section == Some(Section::Agent);
        let claude_on = self.provider_on(PROVIDER_CLAUDE, cx);
        let cursor_on = self.provider_on(PROVIDER_CURSOR, cx);
        let codex_on = self.provider_on(PROVIDER_CODEX, cx);

        // `filteredNav`: a query that hides the current pane lands on the first match.
        let query = self.field_value("nav-search", cx);
        let cur_id = self.vcx.state.read(cx).settings_section.clone();
        if let Some(next) = section_after_filter(&filtered_nav(&query), &cur_id) {
            self.set_section(next, cx);
            return;
        }

        let prefetch = agent && !claude_on;
        if self.fx.prefetch != Some(prefetch) {
            self.fx.prefetch = Some(prefetch);
            if prefetch {
                self.vcx
                    .state
                    .update(cx, |s, cx| s.prefetch_provider_usage(cx).detach());
            }
        }
        let claude = agent && claude_on;
        if self.fx.claude != Some(claude) {
            self.fx.claude = Some(claude);
            let g = self.fx.bump("claude");
            if claude {
                self.probe_claude(g, cx);
            }
        }
        let cursor = (agent && cursor_on).then(|| {
            self.settings(cx)
                .map(|s| s.cursor_api_key.clone())
                .unwrap_or_default()
        });
        if self.fx.cursor.as_ref() != Some(&cursor) {
            self.fx.cursor = Some(cursor.clone());
            let g = self.fx.bump("cursor");
            if let Some(prev) = cursor {
                self.probe_cursor(prev, g, cx);
            }
        }
        // A newly saved binary path refreshes discovery in this pane.
        let codex = (agent && codex_on).then(|| {
            self.settings(cx)
                .map(|s| s.codex_bin_path.clone())
                .unwrap_or_default()
        });
        if self.fx.codex.as_ref() != Some(&codex) {
            self.fx.codex = Some(codex.clone());
            let g = self.fx.bump("codex");
            if codex.is_some() {
                self.probe_codex(g, cx);
            }
        }
        let codex_mounted = agent && codex_on;
        if self.fx.codex_mounted != Some(codex_mounted) {
            self.fx.codex_mounted = Some(codex_mounted);
            if !codex_mounted {
                self.cancel_codex_login(cx);
            }
        }
        let chatgpt = agent && self.provider_on(PROVIDER_CHATGPT, cx);
        if self.fx.chatgpt != Some(chatgpt) {
            self.fx.chatgpt = Some(chatgpt);
            let g = self.fx.bump("chatgpt");
            self.chatgpt_mounted(chatgpt, g, cx);
        }
        let models = (section == Some(Section::Models)).then(|| {
            (
                self.cursor_signed_in(cx),
                self.codex_configured(cx),
                claude_on,
                cursor_on,
                codex_on,
                self.vcx.state.read(cx).model_catalog_rev(),
            )
        });
        if self.fx.models != Some(models) {
            self.fx.models = Some(models);
            let g = self.fx.bump("models");
            if models.is_some() {
                self.load_models(g, cx);
            }
        }
        let codex_key = (
            self.codex_key_dirty,
            self.settings(cx)
                .map(|s| s.codex_api_key.clone())
                .unwrap_or_default(),
        );
        if self.fx.codex_key.as_ref() != Some(&codex_key) {
            self.fx.codex_key = Some(codex_key.clone());
            if !codex_key.0 {
                self.field_rewrite("codex-key", codex_key.1);
            }
        }
        let settings_now = self.settings(cx).cloned();
        if self.fx.loops_settings.as_ref() != Some(&settings_now) {
            self.fx.loops_settings = Some(settings_now.clone());
            self.default_loop = recur_engine::view::loops::default_loop_id(settings_now.as_ref());
            self.default_policy =
                recur_engine::view::loops::default_policy_id(settings_now.as_ref());
        }
        let drafts = (
            settings_now
                .as_ref()
                .map(|s| s.bundle_values.clone())
                .unwrap_or_default(),
            self.bundle_focus.clone(),
            self.bundle_settings
                .rows
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>(),
        );
        if self.fx.bundle_drafts.as_ref() != Some(&drafts) {
            self.fx.bundle_drafts = Some(drafts.clone());
            self.sync_bundle_drafts(&drafts.0, &drafts.1);
        }
        let editor_theme = {
            let st = self.vcx.state.read(cx);
            (st.editor_theme.clone(), st.editor_theme_catalog.len())
        };
        if self.fx.editor_theme.as_ref() != Some(&editor_theme) {
            self.fx.editor_theme = Some(editor_theme);
            if section == Some(Section::Appearance) {
                self.edth_scroll_pending = true;
            }
        }
        if self.fx.section != section {
            self.fx.section = section;
            self.section_changed(section, window, cx);
        }
    }

    /// The effects keyed on `$settingsSection` alone.
    fn section_changed(&mut self, s: Option<Section>, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.fx.bump("section");
        if s != Some(Section::General) {
            self.branch_prefix_dirty = false;
            self.field_rewrite("branch-prefix", String::new());
        }
        match s {
            Some(Section::General) => self.load_github_login(g, cx),
            Some(Section::Appearance) => {
                self.vcx
                    .state
                    .update(cx, |st, cx| st.load_theme_catalogs(cx).detach());
                self.edth_scroll_pending = true;
            }
            Some(Section::Editor) => self.load_fonts(g, cx),
            Some(Section::Skills) => self.load_skills(g, cx),
            Some(Section::Loops) => self.load_loops(g, cx),
            Some(Section::Integrations) => self.load_bundle_settings(g, cx),
            Some(Section::Data) => self.load_logs_path(cx),
            Some(Section::BundleCommands) => self.load_commands(g, cx),
            Some(Section::BundlePrompts) => self.load_prompts(g, cx),
            Some(Section::BundleGrammars) => self.load_grammars(g, cx),
            Some(Section::BundleThemes) => self.load_themes(g, cx),
            Some(Section::BundleSnippets) => self.load_snippets(g, cx),
            Some(Section::BundleLsp) => self.load_lsp(g, cx),
            _ => {}
        }
        let _ = window;
    }

    // ---- patchSettings ----

    /// `patchSettings`: save `f`'s edit of the current settings unless it changes nothing,
    /// toast `Settings saved`, and refresh the model catalog when `catalog` (the patch touched
    /// providers or Codex credentials). Resolves to the saved settings (the current ones when
    /// nothing changed), or `None` when the save failed or settings never loaded.
    pub(crate) fn patch(
        &mut self,
        catalog: bool,
        f: impl FnOnce(&mut Settings),
        cx: &mut Context<Self>,
    ) -> Task<Option<Settings>> {
        let state = self.vcx.state.clone();
        let Some(cur) = state.read(cx).settings.clone() else {
            return Task::ready(None);
        };
        let mut next = cur.clone();
        f(&mut next);
        if next == cur {
            return Task::ready(Some(cur));
        }
        let services = self.vcx.services.clone();
        cx.spawn(async move |_, cx| {
            let saved = call(
                cx,
                services.settings.set_settings(&next),
                "Could not save settings",
            )
            .await?;
            let s = saved.clone();
            cx.update(|cx| {
                state.update(cx, |st, cx| {
                    st.set_settings(s, cx);
                    if catalog {
                        st.invalidate_model_catalog(cx);
                    }
                });
                toast(ToastKind::Success, "Settings saved", cx);
            });
            Some(saved)
        })
    }

    /// `setEditorPrefs`.
    pub(crate) fn set_editor_prefs(
        &mut self,
        f: impl FnOnce(&mut recur_editor::prefs::EditorPrefs),
        cx: &mut Context<Self>,
    ) {
        self.vcx.state.update(cx, |s, cx| s.set_editor_prefs(f, cx));
    }

    /// True when `props` (a `Debug` rendering of them) differs from what `key` was last given.
    pub(crate) fn props_changed(&mut self, key: &str, props: String) -> bool {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        props.hash(&mut h);
        let sig = h.finish();
        if self.sigs.get(key) == Some(&sig) {
            return false;
        }
        self.sigs.insert(key.to_string(), sig);
        true
    }

    // ---- fields ----

    /// The field for `key`, created on first use.
    pub(crate) fn field(
        &mut self,
        key: &str,
        opts: FieldOpts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<gpui_kit::base::input::InputState> {
        if !self.fields.contains_key(key) {
            let f = Field::new(key.to_string(), opts, Self::field_event, window, cx);
            self.fields.insert(key.to_string(), f);
        }
        let f = self.fields.get_mut(key).expect("just inserted");
        f.flush(window, cx);
        recur_ui::style::style_text_input(&f.input, cx);
        f.input.clone()
    }

    /// The field for `key` with `value={v}`.
    pub(crate) fn bound_field(
        &mut self,
        key: &str,
        v: &str,
        opts: FieldOpts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<gpui_kit::base::input::InputState> {
        let input = self.field(key, opts, window, cx);
        if let Some(f) = self.fields.get_mut(key) {
            f.bind(v, window, cx);
        }
        input
    }

    pub(crate) fn field_value(&self, key: &str, cx: &App) -> String {
        self.fields
            .get(key)
            .map(|f| f.value(cx))
            .unwrap_or_default()
    }

    /// `el.value = …` from outside a render.
    pub(crate) fn field_rewrite(&mut self, key: &str, v: String) {
        if let Some(f) = self.fields.get_mut(key) {
            f.rewrite(v);
        }
    }

    fn field_event(
        &mut self,
        key: &str,
        ev: FieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            FieldEvent::Focus => {
                if let Some(f) = self.fields.get_mut(key) {
                    f.focused(cx);
                }
            }
            FieldEvent::Change => {
                let changed = self.fields.get_mut(key).is_some_and(|f| f.take_change(cx));
                if !changed {
                    return;
                }
            }
            _ => {}
        }
        let value = self.field_value(key, cx);
        crate::sections::on_field(self, key, ev, value, window, cx);
        self.sync(window, cx);
        cx.notify();
    }

    // ---- render ----

    fn nav(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let input = self.field("nav-search", FieldOpts::default(), window, cx);
        let query = self.field_value("nav-search", cx);
        let current = self.vcx.state.read(cx).settings_section.clone();
        let me = cx.entity().downgrade();
        let groups = filtered_nav(&query)
            .into_iter()
            .map(|g| {
                let items = g
                    .items
                    .into_iter()
                    .map(|item| {
                        let (id, me) = (item.id, me.clone());
                        recur_ui::set::NavEntry {
                            key: id.id().into(),
                            label: item.label.into(),
                            icon: item.icon,
                            on: current == id.id(),
                            on_click: std::rc::Rc::new(move |_, cx| {
                                let _ = me.update(cx, |this, cx| this.set_section(id, cx));
                            }),
                        }
                    })
                    .collect();
                (g.group.into(), items)
            })
            .collect();
        recur_ui::set::nav_panel(&input, groups, &self.nav_scroll, window, cx)
    }
}

impl SettingsView {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = tokens(cx);
        let on = self.vcx.state.read(cx).view == ViewName::Settings;
        let section = self.section(cx);
        let nav = self.nav(window, cx);
        let title = section.map(Section::title).unwrap_or_default();
        let head = recur_ui::set::page_head(title, None, window, cx);
        let body = crate::sections::pane(self, section, window, cx);
        div()
            .id("v-settings")
            .debug_selector(move || format!("v-settings{}", if on { ":on" } else { "" }))
            .key_context("Settings")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .bg(hsla(t.bg))
            .child(nav)
            .child(recur_ui::set::page_main(
                head,
                body,
                &self.main_scroll,
                window,
            ))
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The modal and card events.
impl SettingsView {
    pub fn open_compat_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let taken: Vec<String> = self
            .settings(cx)
            .map(|s| {
                s.compatible_providers
                    .iter()
                    .map(|p| p.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let services = self.vcx.services.clone();
        let modal = cx.new(|cx| CompatibleProviderModal::new(taken, services, window, cx));
        let sub =
            cx.guarded_subscribe_in(&modal, window, |this, _, ev: &ModalEvent, window, cx| {
                match ev {
                    ModalEvent::Close => this.close_compat_modal(window, cx),
                    ModalEvent::Add(p) => {
                        let mut list = this
                            .settings(cx)
                            .map(|s| s.compatible_providers.clone())
                            .unwrap_or_default();
                        list.push(p.clone());
                        let save = this.patch(true, |s| s.compatible_providers = list, cx);
                        cx.spawn_in(window, async move |this, cx| {
                            if save.await.is_some() {
                                let _ = this.update_in(cx, |this, window, cx| {
                                    this.close_compat_modal(window, cx)
                                });
                            }
                        })
                        .detach();
                    }
                }
                cx.notify();
            });
        self._subs.push(sub);
        let first = modal.read(cx).initial_focus(cx);
        window.focus(&first, cx);
        cx.set_global(crate::ModalSlot(Some(modal.clone().into())));
        self.compat_modal = Some(modal);
        cx.notify();
    }

    pub(crate) fn close_compat_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.compat_modal = None;
        cx.set_global(crate::ModalSlot(None));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// One card per compatible provider, kept across renders.
    pub(crate) fn compat_card(
        &mut self,
        p: &recur_domain::CompatibleProvider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CompatibleProviderCard> {
        if let Some(c) = self.compat_cards.get(&p.id) {
            return c.clone();
        }
        let services = self.vcx.services.clone();
        let card = cx.new(|cx| CompatibleProviderCard::new(p.clone(), services, window, cx));
        let id = p.id.clone();
        let sub = cx.guarded_subscribe(
            &card,
            move |this: &mut Self, _, ev: &CardEvent, cx| match ev {
                CardEvent::Change(next) => this.update_compatible(next.clone(), cx),
                CardEvent::Test => this.test_compatible(&id, cx),
                CardEvent::Remove => this.remove_compatible(&id, cx),
            },
        );
        self._subs.push(sub);
        self.compat_cards.insert(p.id.clone(), card.clone());
        card
    }
}
