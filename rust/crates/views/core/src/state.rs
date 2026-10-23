//! [`AppState`]: the app-wide stores of `lib/stores.ts` and the module state of `lib/api.ts`,
//! as one GPUI entity every view reads. Views observe it for changes and subscribe to its
//! [`ShellEvent`]s; mutations go through its methods so the persistence and side effects the
//! Svelte stores carried stay in one place.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use gpui_kit::{
    App, AppContext, AsyncApp, Context, Entity, EventEmitter, Global, SharedString, Task,
};
use recur_domain::{
    BriefingSnapshot, DiffStat, InboxItem, OpenPathResult, PermissionRequest, Policy, Pr, Project,
    ProjectGroup, Resource, RunCommand, ScheduledPrompt, Settings, UserQuestion, Workspace,
};
use recur_services::api::{Snapshot, TerminalDockChrome, TerminalThemeInfo, ThemeInfo};
use recur_services::events::MenuState;
use recur_services::{Error, Services};
use recur_ui::theme::{Appearance, SystemSync};
use recur_ui::widgets::{ToastKind, ToastOptions, push_toast};

use crate::local_storage::LocalStorage;
use crate::modes::ws_main_blocked;
use crate::nav::{Crumb, CrumbStyle, ViewName, seg};
use crate::prefs::{
    self, DockChrome, PanelKey, PanelLayout, SyncName, TerminalPrefs, save_rail_collapsed,
};
use crate::run::{RunDevRequest, RunStore, workspace_dev_cmd, workspace_run};
use crate::shell_state::ShellWriter;

/// Unseen finished chat tabs plus the rollups for sidebar and project badges (D-attention).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attention {
    pub unseen: i64,
    pub inbox_unread: i64,
    pub dock: i64,
    pub app_unseen: i64,
    pub by_workspace: BTreeMap<String, i64>,
    pub by_project: BTreeMap<String, i64>,
}

impl Attention {
    pub fn workspace(&self, id: &str) -> i64 {
        self.by_workspace.get(id).copied().unwrap_or(0)
    }

    pub fn project(&self, id: &str) -> i64 {
        self.by_project.get(id).copied().unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanKind {
    Land,
    Commit,
}

/// Keyboard-cycle progress the sidebar follows: a land peek-expands collapsed ancestors, a
/// commit (dwell or click) makes it stick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceScanEvent {
    pub seq: u64,
    pub kind: ScanKind,
    pub workspace_id: String,
}

/// A chat tab: `{ workspaceId, code }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatRef {
    pub workspace_id: String,
    pub code: String,
}

/// A deep link into the code observatory (`ObsJump`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObsJump {
    pub path: String,
    pub line: u32,
    /// Inclusive end line; when past `line` the editor selects the range.
    pub end_line: u32,
    /// 1-based inclusive columns for a partial-line `@file` chip.
    pub start_col: u32,
    pub end_col: u32,
    pub workspace_id: String,
    pub project_id: String,
    /// `findings` / `plan` / `review`.
    pub panel: String,
    /// `details` / `todos` / `attachments` with panel `plan`.
    pub plan_tab: String,
    pub finding_id: String,
    pub focus_note: bool,
    pub prefer_diff: bool,
}

/// A jump into PR detail's diff (`prDiffJump`): the PR chat host's transcript chips open
/// files there instead of in the Observatory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrDiffJump {
    pub file: String,
    pub line: u32,
}

/// The Approve / Request changes modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrReviewKind {
    Approve,
    Request,
}

/// Prefill for the New workspace modal (`NewTaskDraft`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewTaskDraft {
    pub text: String,
    pub display: String,
    pub mode: String,
    pub linear_key: String,
    pub linked_project_id: String,
}

/// Opens the New/Edit schedule modal from anywhere.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScheduleEditorRequest {
    pub project_id: Option<String>,
    pub schedule: Option<ScheduledPrompt>,
}

/// An image pasted into the New workspace modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedAttachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub data_url: String,
}

/// The opening instruction, mode and model the New workspace modal hands a fresh workspace,
/// keyed by workspace id (`WorkspaceSeed`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceSeed {
    pub id: String,
    pub text: String,
    /// UI-only chat bubble; the wire stays in `text`.
    pub display: String,
    pub mode: String,
    pub model: String,
    pub flags: BTreeMap<String, String>,
    pub simplify: Option<bool>,
    pub attachments: Vec<SeedAttachment>,
}

/// The full-screen dialogs and palettes. At most one of the palette-like overlays is open:
/// opening one dismisses the others.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overlays {
    pub palette: bool,
    pub commands: bool,
    pub shortcuts: bool,
    pub open_in: bool,
    pub run_menu: bool,
    pub stop_menu: bool,
    pub syntax_mode: bool,
    pub new_task: bool,
    pub new_project: bool,
    pub pr_review: Option<PrReviewKind>,
    /// The TextMate-style numbered keybinding picker is open.
    pub keybinding_menu: bool,
    /// Registered chat occluders (`setChatOccluder`): lightbox, Quick Open, `agent-pane`, …
    pub chat_occluders: BTreeSet<String>,
}

/// Collapsed / off-screen chat: occludes attention without being a full-screen overlay.
const ATTENTION_ONLY_OCCLUDERS: [&str; 1] = ["agent-pane"];

impl Overlays {
    /// `chatUIOccluded`.
    pub fn chat_ui_occluded(&self) -> bool {
        !self.chat_occluders.is_empty()
    }

    /// `chatOverlayOpen`: a full-screen occluder only.
    pub fn chat_overlay_open(&self) -> bool {
        self.chat_occluders
            .iter()
            .any(|id| !ATTENTION_ONLY_OCCLUDERS.contains(&id.as_str()))
    }

    /// Escape-to-back yields to these: each handles its own Escape.
    pub fn blocks_escape(&self) -> bool {
        self.palette
            || self.commands
            || self.open_in
            || self.run_menu
            || self.stop_menu
            || self.syntax_mode
            || self.shortcuts
            || self.new_task
            || self.new_project
            || self.pr_review.is_some()
    }

    /// `workspaceCycleBlocked`.
    pub fn blocks_workspace_cycle(&self) -> bool {
        self.new_task
            || self.new_project
            || self.palette
            || self.commands
            || self.open_in
            || self.shortcuts
            || self.syntax_mode
            || self.run_menu
            || self.stop_menu
            || self.keybinding_menu
            || self.chat_overlay_open()
            || self.pr_review.is_some()
    }

    fn close_palettes(&mut self) {
        self.palette = false;
        self.commands = false;
        self.shortcuts = false;
        self.open_in = false;
        self.syntax_mode = false;
        self.run_menu = false;
        self.stop_menu = false;
    }
}

/// A slash prompt the workspace view should run (`promptActionRequest`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptActionRequest {
    pub workspace_id: String,
    pub slash: String,
    pub args: String,
    pub nonce: u64,
}

/// What [`AppState`] announces beyond `cx.notify()`.
#[derive(Clone, Debug)]
pub enum ShellEvent {
    /// A native menu item or its accelerator fired (`menu.action`). The shell handles the
    /// app-level ones; views handle what they own (the Observatory: tabs, panels, focus).
    MenuAction(SharedString),
    /// `openPR`: the PR detail view resets its tab and drafts.
    PrOpened(String),
    /// A workspace left the live list or lost its worktree: drop per-workspace client state
    /// (chat tab memory, editor sessions, land cache, parked terminals).
    WorkspaceGone(String),
    /// `app.focused`: the OS window regained focus (with the workspace the backend refreshed).
    AppFocused(String),
    /// A backend `app.open` landed and was applied.
    PathOpened(Box<OpenPathResult>),
    /// `onQuestionAnswered`: the UI accepted an `ask_user` answer, before the RPC returns
    /// (paint the user bubble before the resume streams).
    QuestionAnswered(Box<(recur_domain::UserQuestion, crate::chat::AskUserAnswer)>),
    /// `onQuestionAnswerFailed`: the backend rejected that answer.
    QuestionAnswerFailed(Box<(recur_domain::UserQuestion, crate::chat::AskUserAnswer)>),
}

/// Holds the app-wide [`AppState`] entity.
pub struct GlobalAppState(pub Entity<AppState>);

impl Global for GlobalAppState {}

/// Runs the injected drop hook (the desktop allowlists Finder drops for file serving).
pub type DropHook = std::rc::Rc<dyn Fn(&[String])>;

pub struct AppState {
    pub(crate) services: Services,
    pub(crate) storage: LocalStorage,

    // ---- data (seeded from the backend, updated by events) ----
    pub project_groups: Vec<ProjectGroup>,
    pub projects: Vec<Project>,
    pub workspaces: Vec<Workspace>,
    pub prs: Vec<Pr>,
    pub inbox: Vec<InboxItem>,
    pub briefing: BriefingSnapshot,
    pub attention: Attention,
    pub policy: Option<Policy>,
    pub settings: Option<Settings>,
    pub resource: Option<Resource>,
    /// `RECUR_INSTANCE_LABEL` (dogfood titlebar chip). Empty in packaged builds.
    pub instance_label: String,
    /// Subscription quotas for the sidebar-footer usage meter.
    pub provider_usage: Option<Snapshot>,
    /// Workspaces with at least one live agent.
    pub workspaces_running: HashSet<String>,
    /// Individual live agent ids (`workspaceId-code`).
    pub agents_running: HashSet<String>,
    /// Live tool-permission prompts, oldest first.
    pub permission_requests: Vec<PermissionRequest>,
    /// Live ask_user prompts.
    pub user_questions: Vec<UserQuestion>,
    pub schedules: Vec<ScheduledPrompt>,
    pub schedules_version: u64,
    /// Bumped when an issue's assignment may have changed.
    pub issues_stale: u64,
    /// Bumped after Reload Bundles so views holding bundle-derived state rebuild.
    pub bundles_epoch: u64,
    pub booted: bool,

    // ---- `lib/api.ts` module state ----
    pub(crate) live_diff: HashMap<String, DiffStat>,
    pub(crate) dropped_workspace_ids: HashSet<String>,
    pub(crate) fetching_workspace: HashSet<String>,
    pub(crate) refetch_workspace: HashSet<String>,
    pub(crate) running_agent_ids: BTreeMap<String, BTreeSet<String>>,
    pub(crate) agent_event_seq: u64,
    pub(crate) workspace_agent_event_seq: HashMap<String, u64>,
    pub(crate) agents_running_seed_gen: u64,
    pub(crate) briefing_refresh: Option<Task<()>>,
    pub(crate) git_warned: bool,
    pub(crate) last_github_pr_sync: Option<std::time::Instant>,
    pub(crate) event_bridge: Option<Task<()>>,

    // ---- navigation ----
    pub view: ViewName,
    /// Kept as restore memory after leaving the detail view.
    pub selected_workspace_id: String,
    pub selected_pr_id: String,
    /// The project the Project screen edits; '' creates a new one.
    pub project_form_id: String,
    pub crumb: Crumb,
    /// Overview section to scroll into view (`needs` / `working` / `pickup`).
    pub briefing_focus: String,
    /// Active Settings pane (survives leaving Settings).
    pub settings_section: String,
    pub observatory_jump: Option<ObsJump>,
    /// Consumed by PR detail's Files tab.
    pub pr_diff_jump: Option<PrDiffJump>,
    /// One-shot: WorkspaceView selects this agent tab when the workspace lands.
    pub pending_chat_focus: Option<ChatRef>,
    /// The chat tab on screen.
    pub focused_chat: Option<ChatRef>,
    /// The PR-detail agent dock is shown (session only).
    pub pr_agent_dock_open: bool,
    pub scan_event: WorkspaceScanEvent,
    /// True between a cycle land and its dwell/leave commit: skip seen-marking and git fetch.
    pub scan_pending: bool,
    pub(crate) scan_cursor: String,
    pub(crate) scan_timer: Option<Task<()>>,
    pub(crate) shell_persist: bool,
    pub(crate) shell_writer: ShellWriter,

    // ---- overlays ----
    pub overlays: Overlays,
    /// Prefill for New workspace's repo picker; cleared after apply.
    pub new_task_repo_id: String,
    pub new_task_draft: Option<NewTaskDraft>,
    pub(crate) new_project_resume_new_task: bool,
    pub schedule_editor: Option<ScheduleEditorRequest>,
    /// The titlebar asked the open workspace view to run a slash prompt (commit, update, …).
    pub prompt_action: Option<PromptActionRequest>,
    pub workspace_seeds: BTreeMap<String, WorkspaceSeed>,
    /// The collapsed rail is overlaid from the left edge (ephemeral).
    pub rail_peek: bool,
    /// A flyout drag or menu is in progress: hide paths must not yank the rail.
    pub rail_peek_locked: bool,

    // ---- prefs ----
    pub rail_collapsed: bool,
    pub panels: PanelLayout,
    pub appearance: Appearance,
    pub system_dark: bool,
    pub(crate) native_dark: Option<bool>,
    /// The applied code editor theme key (`editorTheme`).
    pub editor_theme: String,
    pub editor_sync: SystemSync,
    pub terminal_prefs: TerminalPrefs,
    /// `editorPrefs` (`recur.editorPrefs`): font, indent, keymap and display for every editor.
    pub editor_prefs: recur_editor::prefs::EditorPrefs,
    pub terminal_sync: SystemSync,
    /// Bundle editor themes (`listThemeMeta`), in catalog order.
    pub editor_theme_catalog: Vec<ThemeInfo>,
    /// Bundle terminal themes (`terminalThemeCatalog`).
    pub terminal_theme_catalog: Vec<TerminalThemeInfo>,
    /// The native menubar's contextual items.
    pub menu_state: MenuState,
    pub dock: DockChrome,
    pub run: RunStore,
    pub(crate) busy_watch: HashMap<String, Task<()>>,
    pub(crate) drop_hook: Option<DropHook>,
    pub(crate) commands: crate::commands::CommandRegistry,
    pub(crate) chat: crate::chat::ChatStores,
}

impl EventEmitter<ShellEvent> for AppState {}

/// `bridgeErrorMessage`: a human message out of a rejected call (sometimes JSON-wrapped).
pub fn bridge_error_message(err: &Error) -> String {
    let raw = err.message.as_str();
    if raw.starts_with('{')
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(raw)
        && let Some(m) = v.get("message").and_then(|m| m.as_str())
        && !m.is_empty()
    {
        return m.to_string();
    }
    raw.to_string()
}

/// `call()`: await a service call; on failure push `"{fail}: {message}"` as an error toast and
/// return `None`.
pub async fn call<T>(
    cx: &mut AsyncApp,
    fut: impl Future<Output = recur_services::Result<T>>,
    fail: &str,
) -> Option<T> {
    match fut.await {
        Ok(v) => Some(v),
        Err(err) => {
            let text = format!("{fail}: {}", bridge_error_message(&err));
            cx.update(|cx| push_toast(ToastKind::Error, text, ToastOptions::default(), cx));
            None
        }
    }
}

/// Push a toast from anywhere with an app context.
pub fn toast(kind: ToastKind, text: impl Into<SharedString>, cx: &mut App) {
    push_toast(kind, text, ToastOptions::default(), cx);
}

impl AppState {
    /// A state over `services` with prefs read from `storage` (`initTheme`, `initEditorTheme`,
    /// `initPanelWidths`, `initCodePanelSides`, `initRailCollapsed`, `initTerminalPrefs`).
    pub fn new(services: Services, storage: LocalStorage) -> Self {
        let appearance = prefs::load_appearance(&storage);
        let native_dark = prefs::load_system_dark(&storage);
        let mut state = Self {
            services,
            project_groups: Vec::new(),
            projects: Vec::new(),
            workspaces: Vec::new(),
            prs: Vec::new(),
            inbox: Vec::new(),
            briefing: BriefingSnapshot::default(),
            attention: Attention::default(),
            policy: None,
            settings: None,
            resource: None,
            instance_label: String::new(),
            provider_usage: None,
            workspaces_running: HashSet::new(),
            agents_running: HashSet::new(),
            permission_requests: Vec::new(),
            user_questions: Vec::new(),
            schedules: Vec::new(),
            schedules_version: 0,
            issues_stale: 0,
            bundles_epoch: 0,
            booted: false,
            live_diff: HashMap::new(),
            dropped_workspace_ids: HashSet::new(),
            fetching_workspace: HashSet::new(),
            refetch_workspace: HashSet::new(),
            running_agent_ids: BTreeMap::new(),
            agent_event_seq: 0,
            workspace_agent_event_seq: HashMap::new(),
            agents_running_seed_gen: 0,
            briefing_refresh: None,
            git_warned: false,
            last_github_pr_sync: None,
            event_bridge: None,
            view: ViewName::Briefing,
            selected_workspace_id: String::new(),
            selected_pr_id: String::new(),
            project_form_id: String::new(),
            crumb: vec![seg("Inbox", CrumbStyle::BoldOn)],
            briefing_focus: String::new(),
            settings_section: "general".into(),
            observatory_jump: None,
            pr_diff_jump: None,
            pending_chat_focus: None,
            focused_chat: None,
            pr_agent_dock_open: true,
            scan_event: WorkspaceScanEvent {
                seq: 0,
                kind: ScanKind::Commit,
                workspace_id: String::new(),
            },
            scan_pending: false,
            scan_cursor: String::new(),
            scan_timer: None,
            shell_persist: false,
            shell_writer: ShellWriter::default(),
            overlays: Overlays::default(),
            new_task_repo_id: String::new(),
            new_task_draft: None,
            new_project_resume_new_task: false,
            schedule_editor: None,
            prompt_action: None,
            workspace_seeds: BTreeMap::new(),
            rail_peek: false,
            rail_peek_locked: false,
            rail_collapsed: prefs::load_rail_collapsed(&storage),
            panels: PanelLayout::load(&storage),
            appearance,
            system_dark: native_dark.unwrap_or(false),
            native_dark,
            editor_theme: prefs::load_editor_theme(&storage),
            editor_sync: SyncName::EditorTheme.load(&storage),
            terminal_prefs: TerminalPrefs::load(&storage),
            editor_prefs: prefs::load_editor_prefs(&storage),
            terminal_sync: SyncName::TerminalTheme.load(&storage),
            editor_theme_catalog: Vec::new(),
            terminal_theme_catalog: Vec::new(),
            menu_state: crate::menu::menu_state_idle(),
            dock: DockChrome::default(),
            run: RunStore::default(),
            busy_watch: HashMap::new(),
            drop_hook: None,
            commands: Default::default(),
            chat: Default::default(),
            storage,
        };
        state.init_themes();
        state
    }

    /// Creates the entity and installs it as the app's global.
    pub fn install(services: Services, storage: LocalStorage, cx: &mut App) -> Entity<AppState> {
        let entity = cx.new(|_| AppState::new(services, storage));
        cx.set_global(GlobalAppState(entity.clone()));
        entity
    }

    /// The app's state. Panics before [`AppState::install`].
    pub fn global(cx: &App) -> Entity<AppState> {
        cx.global::<GlobalAppState>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<AppState>> {
        cx.try_global::<GlobalAppState>().map(|g| g.0.clone())
    }

    pub fn services(&self) -> &Services {
        &self.services
    }

    pub fn storage(&self) -> &LocalStorage {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut LocalStorage {
        &mut self.storage
    }

    /// The desktop's hook for Finder drops (allowlisting them for file serving).
    pub fn set_drop_hook(&mut self, hook: DropHook) {
        self.drop_hook = Some(hook);
    }

    pub fn workspace(&self, id: &str) -> Option<&Workspace> {
        if id.is_empty() {
            return None;
        }
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn pr(&self, id: &str) -> Option<&Pr> {
        self.prs.iter().find(|p| p.id == id)
    }

    /// The workspace the titlebar and menus act on.
    pub fn selected_workspace(&self) -> Option<&Workspace> {
        self.workspace(&self.selected_workspace_id)
    }

    /// The workspace on screen in the detail view, if any.
    pub fn detail_workspace_id(&self) -> &str {
        if self.view == ViewName::Detail {
            &self.selected_workspace_id
        } else {
            ""
        }
    }

    pub fn show_hidden_projects(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|s| s.show_hidden_projects)
    }

    /// `appOverlayOpen`: a full-screen dialog is up, so in-page menus close.
    pub fn app_overlay_open(&self) -> bool {
        let o = &self.overlays;
        o.new_task
            || o.new_project
            || o.palette
            || o.commands
            || o.open_in
            || o.shortcuts
            || o.syntax_mode
            || o.run_menu
            || o.stop_menu
            || o.keybinding_menu
            || o.chat_overlay_open()
            || !self.permission_requests.is_empty()
            || o.pr_review.is_some()
    }

    /// What App.svelte reports through `SetChatUIOccluded`: a dialog covers the chat, so a
    /// finish on the selected tab still raises unseen.
    pub fn chat_occluded(&self) -> bool {
        let o = &self.overlays;
        o.new_task
            || o.new_project
            || o.pr_review.is_some()
            || o.palette
            || o.commands
            || o.open_in
            || o.run_menu
            || o.stop_menu
            || o.syntax_mode
            || o.shortcuts
            || !self.permission_requests.is_empty()
            || o.keybinding_menu
            || o.chat_ui_occluded()
    }

    // ---- overlays ----

    /// Toggle the shortcuts list; opening it dismisses the other overlays.
    pub fn toggle_shortcuts(&mut self, cx: &mut Context<Self>) {
        let next = !self.overlays.shortcuts;
        if next {
            self.overlays.close_palettes();
            self.overlays.new_task = false;
            self.overlays.new_project = false;
        }
        self.overlays.shortcuts = next;
        cx.notify();
    }

    /// Toggle the Run Command palette (⌘⇧P).
    pub fn toggle_commands_palette(&mut self, cx: &mut Context<Self>) {
        let next = !self.overlays.commands;
        if next {
            self.overlays.close_palettes();
            self.overlays.new_task = false;
            self.overlays.new_project = false;
        }
        self.overlays.commands = next;
        cx.notify();
    }

    /// Open the Run Command palette (status-bar Bundles).
    pub fn open_commands_palette(&mut self, cx: &mut Context<Self>) {
        let syntax = self.overlays.syntax_mode;
        self.overlays.close_palettes();
        self.overlays.syntax_mode = syntax;
        self.overlays.new_task = false;
        self.overlays.new_project = false;
        self.overlays.commands = true;
        cx.notify();
    }

    /// Toggle the go-to palette (⌘K).
    pub fn toggle_go_palette(&mut self, cx: &mut Context<Self>) {
        let next = !self.overlays.palette;
        if next {
            self.overlays.close_palettes();
            self.overlays.new_task = false;
            self.overlays.new_project = false;
        }
        self.overlays.palette = next;
        cx.notify();
    }

    /// Toggle Open in… (⌘O). Opening is a no-op unless a workspace is on screen.
    pub fn toggle_open_in(&mut self, cx: &mut Context<Self>) {
        let next = !self.overlays.open_in;
        if next {
            if self.view != ViewName::Detail
                || self.selected_workspace_id.is_empty()
                || ws_main_blocked(self.selected_workspace())
            {
                return;
            }
            self.overlays.close_palettes();
            self.overlays.new_task = false;
            self.overlays.new_project = false;
        }
        self.overlays.open_in = next;
        cx.notify();
    }

    /// Open the language-mode picker for the focused editor file.
    pub fn open_syntax_mode_picker(&mut self, cx: &mut Context<Self>) {
        self.overlays.close_palettes();
        self.overlays.new_task = false;
        self.overlays.new_project = false;
        self.overlays.syntax_mode = true;
        cx.notify();
    }

    /// Close one overlay (each owns its own Escape).
    pub fn close_overlay(&mut self, f: impl FnOnce(&mut Overlays), cx: &mut Context<Self>) {
        f(&mut self.overlays);
        cx.notify();
    }

    /// `openNewTask`: the New workspace modal, defaulting to the focused project.
    pub fn open_new_task(
        &mut self,
        repo_id: &str,
        draft: Option<NewTaskDraft>,
        cx: &mut Context<Self>,
    ) {
        self.new_task_repo_id = if repo_id.is_empty() {
            self.focused_project_id()
        } else {
            repo_id.to_string()
        };
        self.new_task_draft = draft;
        self.overlays.close_palettes();
        self.overlays.new_project = false;
        self.overlays.new_task = true;
        cx.notify();
    }

    pub fn close_new_task(&mut self, cx: &mut Context<Self>) {
        self.overlays.new_task = false;
        self.new_task_draft = None;
        cx.notify();
    }

    /// Open the quick-create project modal.
    pub fn open_new_project(&mut self, resume_new_task: bool, cx: &mut Context<Self>) {
        self.new_project_resume_new_task = resume_new_task;
        self.overlays.close_palettes();
        self.overlays.new_task = false;
        self.overlays.new_project = true;
        cx.notify();
    }

    pub fn close_new_project(&mut self, cx: &mut Context<Self>) {
        self.overlays.new_project = false;
        self.new_project_resume_new_task = false;
        cx.notify();
    }

    /// After create, reopen New workspace with the new project selected.
    pub fn take_new_project_resume_new_task(&mut self) -> bool {
        std::mem::take(&mut self.new_project_resume_new_task)
    }

    pub fn open_pr_review(&mut self, kind: PrReviewKind, cx: &mut Context<Self>) {
        self.overlays.pr_review = Some(kind);
        cx.notify();
    }

    pub fn close_pr_review(&mut self, cx: &mut Context<Self>) {
        self.overlays.pr_review = None;
        cx.notify();
    }

    /// `setChatOccluder`: register or clear an occluding layer by stable id.
    pub fn set_chat_occluder(&mut self, id: &str, active: bool, cx: &mut Context<Self>) {
        let changed = if active {
            self.overlays.chat_occluders.insert(id.to_string())
        } else {
            self.overlays.chat_occluders.remove(id)
        };
        if changed {
            cx.notify();
        }
    }

    pub fn set_keybinding_menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.overlays.keybinding_menu != open {
            self.overlays.keybinding_menu = open;
            cx.notify();
        }
    }

    pub fn set_schedule_editor(
        &mut self,
        req: Option<ScheduleEditorRequest>,
        cx: &mut Context<Self>,
    ) {
        self.schedule_editor = req;
        cx.notify();
    }

    pub fn bump_schedules_version(&mut self, cx: &mut Context<Self>) {
        self.schedules_version += 1;
        cx.notify();
    }

    pub fn set_schedules(&mut self, list: Vec<ScheduledPrompt>, cx: &mut Context<Self>) {
        self.schedules = list;
        cx.notify();
    }

    // ---- prompt actions (`lib/promptActions.ts`) ----

    /// `requestPromptAction`: ask the open workspace view to run a slash prompt.
    pub fn request_prompt_action(
        &mut self,
        workspace_id: &str,
        slash: &str,
        args: &str,
        cx: &mut Context<Self>,
    ) {
        let s = slash.trim_start_matches('/').trim();
        if workspace_id.is_empty() || s.is_empty() {
            return;
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        self.prompt_action = Some(PromptActionRequest {
            workspace_id: workspace_id.to_string(),
            slash: s.to_string(),
            args: args.to_string(),
            nonce,
        });
        cx.notify();
    }

    /// `takePromptActionRequest`: claim a pending request for this workspace, once.
    pub fn take_prompt_action(&mut self, workspace_id: &str) -> Option<(String, String)> {
        if self.prompt_action.as_ref()?.workspace_id != workspace_id {
            return None;
        }
        self.prompt_action.take().map(|r| (r.slash, r.args))
    }

    // ---- seeds ----

    pub fn set_workspace_seed(&mut self, seed: WorkspaceSeed, cx: &mut Context<Self>) {
        self.workspace_seeds.insert(seed.id.clone(), seed);
        cx.notify();
    }

    /// Claim this workspace's seed; `None` if another owner took it.
    pub fn take_workspace_seed(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Option<WorkspaceSeed> {
        let seed = self.workspace_seeds.remove(id)?;
        cx.notify();
        Some(seed)
    }

    // ---- rail ----

    pub fn set_rail_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.rail_collapsed = collapsed;
        save_rail_collapsed(&mut self.storage, collapsed);
        if !collapsed {
            self.rail_peek = false;
        }
        cx.notify();
    }

    pub fn toggle_rail(&mut self, cx: &mut Context<Self>) {
        self.set_rail_collapsed(!self.rail_collapsed, cx);
    }

    pub fn open_rail_peek(&mut self, cx: &mut Context<Self>) {
        if !self.rail_peek {
            self.rail_peek = true;
            cx.notify();
        }
    }

    /// `closeRailPeek`: refused while locked unless forced. The sidebar parks focus first.
    pub fn close_rail_peek(&mut self, force: bool, cx: &mut Context<Self>) -> bool {
        if !self.rail_peek || (!force && self.rail_peek_locked) {
            return false;
        }
        self.rail_peek = false;
        cx.notify();
        true
    }

    pub fn set_rail_peek_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if self.rail_peek_locked != locked {
            self.rail_peek_locked = locked;
            cx.notify();
        }
    }

    // ---- panel sizes ----

    /// `setPanelWidthLive`: per-frame drag updates, not persisted.
    pub fn set_panel_live(&mut self, key: PanelKey, size: f32, cx: &mut Context<Self>) {
        self.panels.set_live(key, size);
        cx.notify();
    }

    /// `persistPanelWidths`: once a drag settles.
    pub fn persist_panels(&mut self) {
        self.panels.persist(&mut self.storage);
    }

    /// `setPanelWidth`.
    pub fn set_panel(&mut self, key: PanelKey, size: f32, cx: &mut Context<Self>) {
        self.set_panel_live(key, size, cx);
        self.persist_panels();
    }

    pub fn set_code_panel_side(
        &mut self,
        key: prefs::CodePanelKey,
        side: prefs::PanelSide,
        cx: &mut Context<Self>,
    ) {
        self.panels.set_side(key, side, &mut self.storage);
        cx.notify();
    }

    // ---- terminal dock chrome ----

    fn persist_dock_chrome(&mut self, id: &str, cx: &mut Context<Self>) {
        if id.is_empty() {
            return;
        }
        let chrome: TerminalDockChrome = self.dock.snapshot(id);
        let services = self.services.clone();
        let id = id.to_string();
        cx.background_spawn(async move {
            let _ = services
                .terminal
                .set_terminal_dock_chrome(&id, &chrome)
                .await;
        })
        .detach();
    }

    pub fn set_terminal_dock_open(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if id.is_empty() {
            return;
        }
        self.dock.set_open(id, on);
        self.persist_dock_chrome(id, cx);
        cx.notify();
    }

    pub fn toggle_terminal_dock(&mut self, id: &str, cx: &mut Context<Self>) {
        let on = !self.dock.is_open(id);
        self.set_terminal_dock_open(id, on, cx);
    }

    pub fn set_terminal_dock_maximized(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if id.is_empty() {
            return;
        }
        self.dock.set_maximized(id, on);
        self.persist_dock_chrome(id, cx);
        cx.notify();
    }

    pub fn toggle_terminal_dock_maximized(&mut self, id: &str, cx: &mut Context<Self>) {
        let on = !self.dock.is_maximized(id);
        self.set_terminal_dock_maximized(id, on, cx);
    }

    pub fn set_editor_hidden(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if id.is_empty() {
            return;
        }
        self.dock.set_editor_hidden(id, on);
        self.persist_dock_chrome(id, cx);
        cx.notify();
    }

    pub fn toggle_editor_hidden(&mut self, id: &str, cx: &mut Context<Self>) {
        let on = !self.dock.is_editor_hidden(id);
        self.set_editor_hidden(id, on, cx);
    }

    pub fn set_terminal_beside(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if id.is_empty() {
            return;
        }
        self.dock.set_beside(id, on);
        self.persist_dock_chrome(id, cx);
        cx.notify();
    }

    pub fn set_terminal_beside_pct(
        &mut self,
        id: &str,
        pct: f64,
        persist: bool,
        cx: &mut Context<Self>,
    ) {
        if id.is_empty() {
            return;
        }
        if self.dock.set_beside_pct(id, pct) {
            cx.notify();
        }
        if persist {
            self.persist_dock_chrome(id, cx);
        }
    }

    // ---- run commands (`lib/run-dev.ts`) ----

    /// `openRunMenu`: the named-run picker (⌘R) for a workspace with a run command.
    pub fn open_run_menu(&mut self, workspace_id: &str, cx: &mut Context<Self>) -> bool {
        if workspace_id.is_empty()
            || ws_main_blocked(self.workspace(workspace_id))
            || workspace_dev_cmd(workspace_id, &self.workspaces, &self.projects).is_empty()
        {
            return false;
        }
        self.overlays.stop_menu = false;
        let next = !self.overlays.run_menu;
        if next {
            self.overlays.palette = false;
            self.overlays.commands = false;
            self.overlays.open_in = false;
            self.overlays.shortcuts = false;
            self.overlays.syntax_mode = false;
        }
        self.overlays.run_menu = next;
        cx.notify();
        true
    }

    /// `openStopMenu`: stop the only running command, or open the Stop menu for several.
    pub fn open_stop_menu(&mut self, workspace_id: &str, cx: &mut Context<Self>) -> bool {
        if workspace_id.is_empty() {
            return false;
        }
        let runs = self.run.running_runs(workspace_id);
        if runs.is_empty() {
            return false;
        }
        self.overlays.run_menu = false;
        if runs.len() == 1 {
            self.overlays.stop_menu = false;
            cx.notify();
            return self.request_stop_run(workspace_id, &runs[0].0, cx);
        }
        let next = !self.overlays.stop_menu;
        if next {
            self.overlays.palette = false;
            self.overlays.commands = false;
            self.overlays.open_in = false;
            self.overlays.shortcuts = false;
            self.overlays.syntax_mode = false;
        }
        self.overlays.stop_menu = next;
        cx.notify();
        true
    }

    /// `requestRunDev`: open the terminal dock and run a command (the pinned default when
    /// `run` is `None`). Does not stop other running commands.
    pub fn request_run_dev(
        &mut self,
        workspace_id: &str,
        run: Option<RunCommand>,
        cx: &mut Context<Self>,
    ) -> bool {
        if ws_main_blocked(self.workspace(workspace_id)) {
            return false;
        }
        let chosen = run.or_else(|| workspace_run(workspace_id, &self.workspaces, &self.projects));
        let cmd = chosen
            .as_ref()
            .map(|r| r.cmd.trim().to_string())
            .unwrap_or_default();
        if cmd.is_empty() {
            toast(
                ToastKind::Info,
                "No run command — set one in project setup",
                cx,
            );
            return false;
        }
        self.overlays.run_menu = false;
        let name = chosen
            .map(|r| r.name.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "dev".into());
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        self.run.request = Some(RunDevRequest {
            workspace_id: workspace_id.to_string(),
            cmd,
            name,
            nonce,
        });
        self.set_terminal_dock_open(workspace_id, true, cx);
        true
    }

    /// `takeRunDevRequest`.
    pub fn take_run_dev_request(&mut self, workspace_id: &str) -> Option<RunDevRequest> {
        self.run.take_request(workspace_id)
    }

    /// `requestStopRun`: signal one running command's foreground process group.
    pub fn request_stop_run(
        &mut self,
        workspace_id: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self
            .run
            .sessions
            .get(workspace_id)
            .and_then(|by| by.get(name))
            .cloned()
        else {
            return false;
        };
        self.request_stop_session(workspace_id, name, &session, cx)
    }

    /// `requestStopSession`. The login shell stays; `term.idle` (or this call) clears the
    /// running UI.
    pub fn request_stop_session(
        &mut self,
        workspace_id: &str,
        name: &str,
        session_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        if !self.run.inflight_stop.insert(session_id.to_string()) {
            return true;
        }
        self.overlays.stop_menu = false;
        cx.notify();
        let services = self.services.clone();
        let (ws, name, sid) = (
            workspace_id.to_string(),
            name.to_string(),
            session_id.to_string(),
        );
        cx.spawn(async move |this, cx| {
            let res = services.terminal.interrupt_terminal(&sid).await;
            let _ = this.update(cx, |s, cx| {
                s.run.inflight_stop.remove(&sid);
                match res {
                    Ok(()) => {
                        s.run.clear_run_session(&ws, &name, Some(&sid));
                    }
                    Err(err) => {
                        let msg = bridge_error_message(&err);
                        if is_missing_session(&msg) {
                            s.run.clear_run_session(&ws, &name, Some(&sid));
                        } else {
                            toast(
                                ToastKind::Error,
                                format!("Failed to stop {name}: {msg}"),
                                cx,
                            );
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        true
    }

    pub fn request_stop_all(&mut self, workspace_id: &str, cx: &mut Context<Self>) -> bool {
        let runs = self.run.running_runs(workspace_id);
        if runs.is_empty() {
            return false;
        }
        for (name, session) in runs {
            self.request_stop_session(workspace_id, &name, &session, cx);
        }
        true
    }

    pub fn set_run_session(
        &mut self,
        workspace_id: &str,
        name: &str,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.run.set_run_session(workspace_id, name, session_id) {
            cx.notify();
        }
    }

    pub fn mark_run_starting(&mut self, session_id: &str) {
        self.run.mark_run_starting(session_id);
        self.busy_watch.remove(session_id);
    }

    pub fn mark_run_started(&mut self, session_id: &str) {
        self.run.mark_run_started(session_id);
    }

    /// `armRunBusyWatch`: drop the optimistic "running" state if the idle watcher never saw a
    /// foreground command within 2s.
    pub fn arm_run_busy_watch(&mut self, session_id: &str, cx: &mut Context<Self>) {
        if session_id.is_empty() {
            return;
        }
        self.busy_watch.remove(session_id);
        if self.run.saw_busy.contains(session_id) {
            return;
        }
        let sid = session_id.to_string();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |s, cx| {
                s.busy_watch.remove(&sid);
                if !s.run.saw_busy.contains(&sid) {
                    s.run.launched.remove(&sid);
                    s.run.clear_run_by_session_id(&sid);
                    cx.notify();
                }
                s.run.saw_busy.remove(&sid);
            });
        });
        self.busy_watch.insert(session_id.to_string(), task);
    }

    pub(crate) fn on_term_event(&mut self, ev: &recur_term::Event, cx: &mut Context<Self>) {
        use recur_term::Event as TermEvent;
        match ev {
            TermEvent::Busy { id, .. } => {
                self.busy_watch.remove(id);
                self.run.on_busy(id);
            }
            TermEvent::Idle { id, .. } | TermEvent::Exit { id, .. } => self.run.on_done(id),
            TermEvent::Data { .. } => return,
        }
        cx.notify();
    }

    /// `pinProjectRun`: persist the pinned Run-button default on the project.
    pub fn pin_project_run(
        &mut self,
        project: &Project,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let mut next = project.clone();
        next.pinned_run = name.trim().to_string();
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let Some(saved) = call(
                cx,
                services.projects.update_project_run_config(&next),
                "Could not pin run command",
            )
            .await
            else {
                return false;
            };
            this.update(cx, |s, cx| {
                if let Some(p) = s.projects.iter_mut().find(|p| p.id == saved.id) {
                    *p = saved;
                }
                cx.notify();
            })
            .is_ok()
        })
    }
}

fn is_missing_session(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("terminal: session ") && m.contains(" not found")
}
