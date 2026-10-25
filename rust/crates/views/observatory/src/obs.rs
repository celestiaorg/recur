//! The Observatory entity: props, slots and events (the Svelte component's API), its state,
//! and the values `Observatory.svelte` derived from it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{
    AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    Focusable, ScrollHandle, Subscription, Task, WeakEntity, Window,
};
use recur_code::glob::{CompiledGlob, compile_excludes, is_excluded_by};
use recur_domain::{FileContent, Finding, OutlineSymbol, SearchHit, SymbolRef, TreeEntry};
use recur_editor::session::{ScratchDoc, StashedBuffer};
use recur_editor::view::EditorView;
use recur_editor::view_state::{EditorViewMode, FileViewState};
use recur_services::Services;
use recur_view_core::AppState;
use recur_view_core::prefs::{CodePanelKey, PanelKey, PanelSide};
use recur_view_review::review_diff::ReviewTab;
use recur_view_review::surface::ReviewSurface;

use crate::model::{FocusZone, MainSurface, Pane, TabBuffer};

/// What the workspace view passes (`$props()` of `Observatory.svelte`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObservatoryProps {
    pub project_id: String,
    pub workspace_id: String,
    pub initial_path: String,
    /// Workspace mode: start on changed files instead of the full tree.
    pub loop_scoped: bool,
    /// Replaces "Maestro" as the left dock's label (`leftTitle`).
    pub left_title: String,
    /// `leftOpen` when the host owns the left gutter (`onToggleLeft`); `None` keeps it local.
    pub left_open: Option<bool>,
    /// Solo/pair agent chat tabs: ⌘T / ⌘W / ⌥⌘←→ while the agent dock is focused.
    pub agent_tab_count: usize,
    /// The host handles `AgentNewTab` / `AgentCloseTab` / `AgentRotateTab`.
    pub agent_tabs: bool,
    /// The host handles `FixFindings` (`onFixFindings`).
    pub can_fix_findings: bool,
    /// The host handles `AddEditorMention` (`onAddEditorMention`).
    pub can_add_editor_mention: bool,
    /// Cover the editor and files while the worktree allocates; the chat stays usable.
    pub main_blocked: bool,
    /// The live workbench. Parked hot shells keep their state but take no focus, watchers,
    /// key bindings or menu.
    pub shell_active: bool,
}

/// What [`Observatory::scope_paths`], [`Observatory::review_paths`] and
/// [`Observatory::exclude_matchers`] last computed, keyed by their inputs.
type ReviewMemo = (Rc<Vec<String>>, Rc<Vec<CompiledGlob>>, Rc<Vec<String>>);
type ExcludedMemo = (Rc<Vec<CompiledGlob>>, HashMap<String, bool>);
type ChangedTreeMemo = (Rc<Vec<String>>, Rc<Vec<TreeEntry>>);

#[derive(Default)]
pub(crate) struct PathsMemo {
    scope: Option<(u64, Rc<Vec<String>>)>,
    excludes: Option<(Vec<String>, Rc<Vec<CompiledGlob>>)>,
    /// The scope and excludes it was filtered from, and the review paths.
    review: Option<ReviewMemo>,
    dir_status: Option<(u64, Rc<BTreeMap<String, String>>)>,
    /// Exclusion by path under the matchers it was tested against.
    excluded: Option<ExcludedMemo>,
    /// The changed-files tree of the review paths it was built from.
    changed_tree: Option<ChangedTreeMemo>,
}

/// The host-owned content the Svelte component took as snippets.
#[derive(Clone, Default)]
pub struct ObservatorySlots {
    /// `leftPanel`: the agent chat (the Maestro panel's body).
    pub left_panel: Option<AnyView>,
    /// `leftHead`: replaces the left dock's title row while the Agent panel shows.
    pub left_head: Option<AnyView>,
    /// `planPanel`: the Plan panel body.
    pub plan_panel: Option<AnyView>,
    /// `planHead`: the Plan pane's title row.
    pub plan_head: Option<AnyView>,
    /// `mainOverlay`: frosts editor and files while `main_blocked`.
    pub main_overlay: Option<AnyView>,
}

/// The Svelte callbacks, and the values the snippets were called with.
#[derive(Clone, Debug, PartialEq)]
pub enum ObservatoryEvent {
    /// `onToggleLeft(open)`.
    ToggleLeft(bool),
    /// `onAgentNewTab`.
    AgentNewTab,
    /// `onAgentCloseTab`.
    AgentCloseTab,
    /// `onAgentRotateTab(dir)`.
    AgentRotateTab(i32),
    /// `onFixFindings`.
    FixFindings,
    /// `onAddEditorMention(path, startLine?, endLine?, startCol?, endCol?)` (0 = absent).
    AddEditorMention {
        path: String,
        start_line: usize,
        end_line: usize,
        start_col: usize,
        end_col: usize,
    },
    /// Put keys in the visible agent composer (`focusAgentComposer`).
    FocusAgentComposer,
    /// `leftPanel(visible)`: the agent transcript is on screen.
    MaestroVisible(bool),
    /// `leftHead(lit)`: the agent dock has keyboard focus.
    AgentLit(bool),
    /// `planPanel(selectedArtifactId)`.
    SelectedArtifact(String),
    /// A finding jump asked the Plan panel for this tab (`planPanelTab`).
    PlanTab(String),
    /// PR detail's agent dock took or released focus (`prDetailFocus`).
    PrDetailFocus(bool),
}

/// A right-click menu on a tree entry or a file tab (`path` empty: the tree's empty area).
#[derive(Clone, Debug, PartialEq)]
pub struct CtxMenu {
    pub x: f32,
    pub y: f32,
    pub path: String,
    pub dir: bool,
    pub name: String,
    pub tab: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftMode {
    Rename,
    NewFile,
    NewFolder,
}

/// The inline rename / new file / new folder field in the tree.
pub struct DraftEdit {
    pub mode: DraftMode,
    pub parent: String,
    pub path: String,
    pub dir: bool,
    pub input: Entity<gpui_kit::base::input::InputState>,
}

impl DraftEdit {
    pub fn name(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }
}

/// The in-app clipboard for tree copy / cut / paste (not the OS clipboard).
#[derive(Clone, Debug, PartialEq)]
pub struct FileClip {
    pub cut: bool,
    pub path: String,
    pub dir: bool,
}

/// How the explorer follows an open (`ExplorerReveal`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerReveal {
    /// Select in the tree and expand ancestors (tree click, tab).
    Reveal,
    /// Reveal, then scroll the Files list to the row (Quick Open).
    Scroll,
    /// Chat chips: only select and scroll when Files shows the row already.
    IfVisible,
}

/// `openPath(path, line, preferDiff, endLine, explorer, fromDisk, startCol, endCol)`.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenRequest {
    pub path: String,
    pub line: usize,
    pub prefer_diff: bool,
    pub end_line: usize,
    pub explorer: ExplorerReveal,
    pub from_disk: bool,
    pub start_col: usize,
    pub end_col: usize,
}

impl OpenRequest {
    pub fn new(path: impl Into<String>) -> Self {
        OpenRequest {
            path: path.into(),
            line: 0,
            prefer_diff: false,
            end_line: 0,
            explorer: ExplorerReveal::Reveal,
            from_disk: false,
            start_col: 0,
            end_col: 0,
        }
    }

    pub fn line(mut self, line: usize) -> Self {
        self.line = line;
        self
    }

    pub fn end_line(mut self, end: usize) -> Self {
        self.end_line = end;
        self
    }

    pub fn prefer_diff(mut self, on: bool) -> Self {
        self.prefer_diff = on;
        self
    }

    pub fn explorer(mut self, e: ExplorerReveal) -> Self {
        self.explorer = e;
        self
    }

    pub fn from_disk(mut self, on: bool) -> Self {
        self.from_disk = on;
        self
    }

    pub fn cols(mut self, start: usize, end: usize) -> Self {
        self.start_col = start;
        self.end_col = end;
        self
    }
}

/// The maximize overlay animates from the dock's slot to the whole Observatory and back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TermMaxPhase {
    #[default]
    Idle,
    Enter,
    Max,
    Exit,
}

/// The overlay box (`--term-overlay-*`), relative to the Observatory.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlayRect {
    pub top: f32,
    pub left: f32,
    pub width: f32,
    pub height: f32,
}

/// The findings status filter (`FindingsStatusFilter`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FindingsFilter {
    #[default]
    Open,
    Ignored,
    Invalid,
    Resolved,
    All,
}

impl FindingsFilter {
    pub const ALL: [FindingsFilter; 5] = [
        FindingsFilter::Open,
        FindingsFilter::Ignored,
        FindingsFilter::Invalid,
        FindingsFilter::Resolved,
        FindingsFilter::All,
    ];

    pub fn key(self) -> &'static str {
        match self {
            FindingsFilter::Open => "open",
            FindingsFilter::Ignored => "ignored",
            FindingsFilter::Invalid => "invalid",
            FindingsFilter::Resolved => "resolved",
            FindingsFilter::All => "all",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FindingsFilter::Open => "Open",
            FindingsFilter::Ignored => "Ignored",
            FindingsFilter::Invalid => "Invalid",
            FindingsFilter::Resolved => "Resolved",
            FindingsFilter::All => "All",
        }
    }
}

/// `LIST_CAP`: side panel rows before "Show all N…".
pub const LIST_CAP: usize = 300;
/// The tree skeleton and file-loading text wait this long before drawing.
pub const SKELETON_DELAY_MS: u64 = 250;
/// Once drawn, "Loading …" stays at least this long.
pub const FILE_LOAD_MIN_VISIBLE_MS: u64 = 400;
/// Same-tick FSEvents coalesce into one refresh.
pub const LIVE_REFRESH_MS: u64 = 50;
/// The review stack rebuilds at most this often while a loop writes.
pub const REVIEW_REFRESH_MS: u64 = 1600;
/// A path stays missing this long across refreshes before it counts as gone.
pub const GONE_CONFIRM_MS: u64 = 600;
/// Stop re-arming the confirm timer after this long from the first miss.
pub const GONE_CONFIRM_MAX_MS: u64 = GONE_CONFIRM_MS * 3;
/// `BASE_DOC_CACHE_MAX`.
pub const BASE_DOC_CACHE_MAX: usize = 32;
/// The maximize FLIP duration.
pub const TERM_MAX_MS: u64 = 320;

pub struct Observatory {
    pub(crate) state: Entity<AppState>,
    pub(crate) services: Services,
    pub(crate) props: ObservatoryProps,
    pub(crate) slots: ObservatorySlots,

    // ---- focus ----
    pub(crate) focus: FocusHandle,
    pub(crate) left_focus: FocusHandle,
    pub(crate) main_focus: FocusHandle,
    pub(crate) files_focus: FocusHandle,
    pub(crate) scm_focus: FocusHandle,
    pub(crate) tabs_focus: FocusHandle,
    pub(crate) review_focus: FocusHandle,
    /// The left and side docks' resize handles.
    pub(crate) resize_focus: [FocusHandle; 2],
    /// The portaled PR dock's hide toggle (`#prd-agent button.prd-chat-toggle`).
    pub(crate) pr_toggle_focus: FocusHandle,
    pub(crate) focus_zone: FocusZone,
    pub(crate) last_focus: Option<FocusHandle>,
    pub(crate) seen_syntax_epoch: Option<u64>,

    // ---- root ----
    pub(crate) loaded_root: String,
    pub(crate) root_token: u64,
    pub(crate) editor_session_key: String,
    pub(crate) session_load_settled: bool,
    pub(crate) syntax_maps_ready: bool,
    pub(crate) session_restored: bool,
    pub(crate) restoring_session_views: bool,
    pub(crate) restoring_session_views_token: u64,

    // ---- explorer ----
    pub(crate) tree: Vec<TreeEntry>,
    pub(crate) expanded: BTreeSet<String>,
    pub(crate) loaded_dirs: BTreeSet<String>,
    pub(crate) loading_dirs: BTreeSet<String>,
    pub(crate) changed_paths: Vec<String>,
    /// path → m|a|d|u versus base (workspace) or HEAD (project).
    pub(crate) git_status: BTreeMap<String, String>,
    pub(crate) git_status_ready: bool,
    /// Bumped whenever `changed_paths` or `git_status` changes (keys `paths_memo`).
    pub(crate) paths_gen: u64,
    pub(crate) paths_memo: std::cell::RefCell<PathsMemo>,
    /// Uncommitted porcelain versus HEAD.
    pub(crate) working_status: BTreeMap<String, String>,
    pub(crate) staged_files: Vec<String>,
    pub(crate) browse_all: bool,
    pub(crate) scope_chosen: bool,
    pub(crate) changed_collapsed: BTreeSet<String>,
    pub(crate) tree_path: String,
    pub(crate) ctx_menu: Option<CtxMenu>,
    pub(crate) draft_edit: Option<DraftEdit>,
    pub(crate) file_clip: Option<FileClip>,
    pub(crate) tree_drop_dir: Option<String>,
    pub(crate) tree_loading: bool,
    pub(crate) show_tree_skeleton: bool,
    pub(crate) skeleton_timer: Option<Task<()>>,
    pub(crate) skipped_tree_for_create: bool,
    pub(crate) worktree_missing: bool,
    pub(crate) dir_not_found_since: HashMap<String, Instant>,
    pub(crate) files_scroll: ScrollHandle,
    pub(crate) scm_scroll: ScrollHandle,
    pub(crate) hovered_row: Option<String>,
    pub(crate) skip_node_click: bool,
    pub(crate) git_status_gen: u64,
    pub(crate) working_status_gen: u64,
    pub(crate) changed_files_gen: u64,
    pub(crate) pending_explorer_scroll: Option<String>,

    // ---- live refresh ----
    pub(crate) live_refresh_alive: bool,
    pub(crate) live_refresh_timer: Option<Task<()>>,
    pub(crate) review_refresh_timer: Option<Task<()>>,
    pub(crate) gone_confirm_timer: Option<Task<()>>,
    pub(crate) refresh_loaded_dirs_in_flight: bool,
    pub(crate) refresh_loaded_dirs_pending: bool,
    pub(crate) refresh_open_file_in_flight: bool,
    pub(crate) refresh_open_file_pending: bool,
    pub(crate) open_file_not_found_path: String,
    pub(crate) open_file_not_found_since: Option<Instant>,

    // ---- tabs and buffers ----
    pub(crate) open_files: Vec<String>,
    pub(crate) closed_tabs: Vec<String>,
    pub(crate) tab_buffers: BTreeMap<String, TabBuffer>,
    pub(crate) dirty_tabs: BTreeSet<String>,
    pub(crate) stashed_buffers: BTreeMap<String, StashedBuffer>,
    pub(crate) scratch_docs: BTreeMap<String, ScratchDoc>,
    pub(crate) scratch_seq: u64,
    pub(crate) file_views: BTreeMap<String, FileViewState>,
    pub(crate) file_tab_scroll_restore: Option<(f32, u64)>,
    pub(crate) file_tab_scroll_restore_id: u64,
    pub(crate) file_tab_reveal_nonce: u64,
    pub(crate) file_tab_scroll: ScrollHandle,
    pub(crate) max_open_files_seen: usize,
    /// A fingerprint of the workspace ids the caches were last pruned to.
    pub(crate) pruned_live: Option<u64>,

    // ---- the main pane ----
    pub(crate) selected_path: String,
    pub(crate) file: Option<FileContent>,
    pub(crate) selected_is_dir: bool,
    pub(crate) draft: String,
    pub(crate) saved_content: String,
    /// The path in the code editor; lags `selected_path` until that file's body is ready.
    pub(crate) editor_path: String,
    pub(crate) hydrate_failed_path: String,
    pub(crate) main_editor_dirty: bool,
    pub(crate) view_mode: EditorViewMode,
    pub(crate) skip_view_restore: bool,
    pub(crate) highlight_line: usize,
    pub(crate) nav_gen: u64,
    pub(crate) file_load_gen: u64,
    pub(crate) file_loading: bool,
    pub(crate) show_file_loading: bool,
    pub(crate) file_loading_shown_at: Option<Instant>,
    pub(crate) file_loading_timer: Option<Task<()>>,
    pub(crate) file_loading_hide_timer: Option<Task<()>>,
    pub(crate) pending_rehydrate: Option<String>,
    pub(crate) outline: Vec<OutlineSymbol>,
    pub(crate) cursor_line: usize,
    pub(crate) cursor_col: usize,
    pub(crate) find_panel_open: bool,
    pub(crate) editor_reveal_gen: u64,

    // ---- base revisions (Diff and the SCM gutter) ----
    pub(crate) base_doc: String,
    pub(crate) base_doc_path: String,
    pub(crate) base_doc_cache: Vec<(String, String)>,
    pub(crate) base_doc_inflight: HashSet<String>,
    pub(crate) base_doc_epoch: u64,
    pub(crate) tracked_base: (String, String, i64),

    // ---- the split pane ----
    pub(crate) split_open: bool,
    pub(crate) split_path: String,
    pub(crate) split_draft: String,
    pub(crate) split_saved: String,
    pub(crate) split_writable: bool,
    pub(crate) split_editor_dirty: bool,
    pub(crate) focused_pane: Pane,
    pub(crate) split_pct: i64,
    pub(crate) split_resizing: bool,
    /// The editor status bar's width when its items stopped fitting on one line each; they
    /// wrap (`.obs-statusbar` items shrink to min-content) until the bar is wider again.
    pub(crate) status_wrap: Option<f32>,

    // ---- editors and previews ----
    pub(crate) main_editor: Option<Entity<EditorView>>,
    pub(crate) split_editor: Option<Entity<EditorView>>,
    pub(crate) merge_editor: Option<(String, Entity<EditorView>)>,
    pub(crate) merge_sync: crate::editor_host::MergeSync,
    pub(crate) main_editor_sync: crate::editor_host::EditorSync,
    pub(crate) split_editor_sync: crate::editor_host::EditorSync,
    pub(crate) file_preview: Option<(
        recur_content::preview::PreviewKind,
        Entity<recur_content::FilePreview>,
    )>,
    pub(crate) pdf_preview: Option<Entity<recur_content::PdfPreview>>,
    pub(crate) preview_scroll: recur_content::SharedScroll,
    pub(crate) image: crate::media::MediaPane,
    pub(crate) media_zoom: Option<Entity<recur_content::AttachmentLightbox>>,
    pub(crate) editor_subs: Vec<Subscription>,
    pub(crate) editor_fullscreen: bool,
    pub(crate) window_was_fullscreen: bool,
    pub(crate) fs_exit_visible: bool,
    pub(crate) fs_exit_timer: Option<Task<()>>,
    pub(crate) fs_exit_hovered: bool,
    pub(crate) last_window_fullscreen: bool,

    // ---- review ----
    pub(crate) main_surface_pref: MainSurface,
    pub(crate) review_unhid_for: String,
    pub(crate) review_tab: ReviewTab,
    pub(crate) review_refresh: u64,
    pub(crate) review_bound: recur_view_review::surface::SurfaceBound,
    pub(crate) review_restore: Option<(String, f32)>,
    pub(crate) review_surface: Option<Entity<ReviewSurface>>,
    pub(crate) review_sig: Option<crate::panels::ReviewSig>,
    pub(crate) review_subs: Vec<Subscription>,

    // ---- panels ----
    pub(crate) left_kind: CodePanelKey,
    pub(crate) right_kind: CodePanelKey,
    pub(crate) show_side: bool,
    pub(crate) local_left_open: bool,
    pub(crate) findings: Vec<Finding>,
    pub(crate) finding_busy_id: String,
    pub(crate) findings_filter: FindingsFilter,
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) search_skipped_large: i64,
    pub(crate) search_truncated: bool,
    pub(crate) search_offset: i64,
    pub(crate) search_regex: bool,
    pub(crate) search_case: bool,
    pub(crate) refs: Vec<SymbolRef>,
    pub(crate) expanded_lists: BTreeSet<String>,
    pub(crate) scm_collapsed: BTreeSet<String>,
    pub(crate) scm_file_list_mode: recur_view_review::review_diff::FileListMode,
    pub(crate) commit_busy: bool,
    pub(crate) commit_msg_busy: bool,
    pub(crate) problem_fix_busy: String,
    pub(crate) problem_hovered: Option<usize>,
    pub(crate) scm_hovered: Option<String>,
    pub(crate) scm_name_hover: Option<String>,
    pub(crate) scm_probes: crate::render::ScmProbes,
    pub(crate) inputs: crate::inputs::Inputs,
    pub(crate) findings_panel: Entity<crate::findings_panel::FindingsPanel>,
    pub(crate) artifacts_panel: Option<Entity<crate::artifacts_panel::ArtifactsPanel>>,
    pub(crate) quick_open: Entity<crate::quick_open::QuickOpen>,
    pub(crate) prompts: Entity<recur_view_chat::palettes::CommandPrompts>,
    /// Quick Open and the command prompts behind cached boundaries (see `recur_ui::pane`).
    pub(crate) layers: [Entity<recur_ui::pane::Layer>; 2],
    /// The side panel as its own cached view, when it holds no chat.
    pub(crate) side_pane: Option<Entity<recur_ui::pane::Pane<Observatory>>>,
    /// The main pane's empty state as its own cached view.
    pub(crate) empty_pane: Option<Entity<recur_ui::pane::Pane<Observatory>>>,
    pub(crate) toolbar_pane: Option<Entity<recur_ui::pane::Pane<Observatory>>>,
    pub(crate) scm_body_pane: Option<Entity<recur_ui::pane::Pane<Observatory>>>,
    pub(crate) crumb_pane: Option<Entity<recur_ui::pane::Pane<Observatory>>>,
    pub(crate) status_pane: Option<Entity<recur_ui::pane::Pane<Observatory>>>,
    /// The window-corner padding the status bar needs, as of the last render.
    pub(crate) status_corner: (bool, bool),
    /// The side heads' views (the chat tabs, the plan head) behind cached boundaries.
    pub(crate) head_layers: Vec<(gpui_kit::EntityId, Entity<recur_ui::pane::Layer>)>,
    pub(crate) prompts_reply:
        Option<futures::channel::oneshot::Sender<Option<BTreeMap<String, String>>>>,
    pub(crate) chord_index: Rc<crate::keys::ChordIndex>,
    pub(crate) chord_task: Option<Task<()>>,
    pub(crate) chord_key: String,

    // ---- LSP ----
    pub(crate) lsp_attach: Option<crate::lsp::client::LspAttach>,
    pub(crate) lsp_attach_gen: u64,
    pub(crate) lsp_outline_timer: Option<Task<()>>,
    pub(crate) lsp_sync_task: Option<Task<()>>,
    pub(crate) prev_disabled_ls: Option<String>,

    // ---- terminal dock ----
    pub(crate) dock: Option<(String, Entity<crate::dock::TerminalDock>)>,
    pub(crate) dock_subs: Vec<Subscription>,
    pub(crate) term_tab_count: usize,
    pub(crate) term_max_phase: TermMaxPhase,
    pub(crate) term_max_anim: Option<(OverlayRect, OverlayRect, Instant)>,
    pub(crate) term_max_gen: u64,
    pub(crate) term_max_for_ws: String,
    pub(crate) term_slot_h: f32,
    pub(crate) overlay_keep_editor_hidden: bool,
    pub(crate) overlay_rect: OverlayRect,
    pub(crate) term_open_for_ws: String,
    pub(crate) term_was_open: bool,
    pub(crate) term_beside_resizing: bool,
    pub(crate) bounds: crate::geometry::Geometry,

    // ---- shell activation ----
    pub(crate) was_shell_active: bool,
    pub(crate) watch_armed_for: String,
    pub(crate) emitted_maestro_visible: Option<bool>,
    pub(crate) emitted_agent_lit: Option<bool>,
    pub(crate) emitted_artifact: Option<String>,
    pub(crate) event_task: Option<Task<()>>,
    pub(crate) anim_epoch: Instant,
    pub(crate) window: Option<AnyWindowHandle>,

    pub(crate) weak: WeakEntity<Observatory>,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ObservatoryEvent> for Observatory {}

impl Focusable for Observatory {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Observatory {
    pub fn new(
        state: Entity<AppState>,
        props: ObservatoryProps,
        slots: ObservatorySlots,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::init_globals(cx);
        let services = state.read(cx).services().clone();
        let storage_split = state
            .read(cx)
            .storage()
            .get("recur.editorSplitPct")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|n| n.is_finite() && (20. ..=80.).contains(n))
            .map(|n| n.round() as i64)
            .unwrap_or(50);
        let review_tab = recur_view_review::review_diff::read_review_tab(state.read(cx).storage());
        let file_list_mode =
            recur_view_review::review_diff::read_file_list_mode(state.read(cx).storage());
        let inputs = crate::inputs::Inputs::new(state.clone(), window, cx);
        let findings_panel =
            cx.new(|cx| crate::findings_panel::FindingsPanel::new(state.clone(), window, cx));
        let quick_open = cx.new(|cx| crate::quick_open::QuickOpen::new(state.clone(), window, cx));
        let prompts = cx.new(recur_view_chat::palettes::CommandPrompts::new);
        let mut o = Observatory {
            services,
            props: ObservatoryProps::default(),
            slots,
            focus: cx.focus_handle(),
            left_focus: cx.focus_handle(),
            main_focus: cx.focus_handle(),
            files_focus: cx.focus_handle().tab_stop(true),
            scm_focus: cx.focus_handle(),
            tabs_focus: cx.focus_handle(),
            review_focus: cx.focus_handle(),
            resize_focus: [cx.focus_handle(), cx.focus_handle()],
            pr_toggle_focus: cx.focus_handle().tab_stop(true),
            focus_zone: FocusZone::Editor,
            last_focus: None,
            seen_syntax_epoch: None,
            loaded_root: String::new(),
            root_token: 0,
            editor_session_key: String::new(),
            session_load_settled: false,
            syntax_maps_ready: false,
            session_restored: false,
            restoring_session_views: false,
            restoring_session_views_token: 0,
            tree: Vec::new(),
            expanded: BTreeSet::new(),
            loaded_dirs: BTreeSet::new(),
            loading_dirs: BTreeSet::new(),
            changed_paths: Vec::new(),
            git_status: BTreeMap::new(),
            git_status_ready: false,
            paths_gen: 0,
            paths_memo: Default::default(),
            working_status: BTreeMap::new(),
            staged_files: Vec::new(),
            browse_all: false,
            scope_chosen: false,
            changed_collapsed: BTreeSet::new(),
            tree_path: String::new(),
            ctx_menu: None,
            draft_edit: None,
            file_clip: None,
            tree_drop_dir: None,
            tree_loading: false,
            show_tree_skeleton: false,
            skeleton_timer: None,
            skipped_tree_for_create: false,
            worktree_missing: false,
            dir_not_found_since: HashMap::new(),
            files_scroll: ScrollHandle::new(),
            scm_scroll: ScrollHandle::new(),
            hovered_row: None,
            skip_node_click: false,
            git_status_gen: 0,
            working_status_gen: 0,
            changed_files_gen: 0,
            pending_explorer_scroll: None,
            live_refresh_alive: true,
            live_refresh_timer: None,
            review_refresh_timer: None,
            gone_confirm_timer: None,
            refresh_loaded_dirs_in_flight: false,
            refresh_loaded_dirs_pending: false,
            refresh_open_file_in_flight: false,
            refresh_open_file_pending: false,
            open_file_not_found_path: String::new(),
            open_file_not_found_since: None,
            open_files: Vec::new(),
            closed_tabs: Vec::new(),
            tab_buffers: BTreeMap::new(),
            dirty_tabs: BTreeSet::new(),
            stashed_buffers: BTreeMap::new(),
            scratch_docs: BTreeMap::new(),
            scratch_seq: 0,
            file_views: BTreeMap::new(),
            file_tab_scroll_restore: None,
            file_tab_scroll_restore_id: 0,
            file_tab_reveal_nonce: 0,
            file_tab_scroll: ScrollHandle::new(),
            max_open_files_seen: 0,
            pruned_live: None,
            selected_path: String::new(),
            file: None,
            selected_is_dir: false,
            draft: String::new(),
            saved_content: String::new(),
            editor_path: String::new(),
            hydrate_failed_path: String::new(),
            main_editor_dirty: false,
            view_mode: EditorViewMode::Code,
            skip_view_restore: false,
            highlight_line: 0,
            nav_gen: 0,
            file_load_gen: 0,
            file_loading: false,
            show_file_loading: false,
            file_loading_shown_at: None,
            file_loading_timer: None,
            file_loading_hide_timer: None,
            pending_rehydrate: None,
            outline: Vec::new(),
            cursor_line: 1,
            cursor_col: 1,
            find_panel_open: false,
            editor_reveal_gen: 0,
            base_doc: String::new(),
            base_doc_path: String::new(),
            base_doc_cache: Vec::new(),
            base_doc_inflight: HashSet::new(),
            base_doc_epoch: 0,
            tracked_base: (String::new(), String::new(), 0),
            split_open: false,
            split_path: String::new(),
            split_draft: String::new(),
            split_saved: String::new(),
            split_writable: false,
            split_editor_dirty: false,
            focused_pane: Pane::Main,
            split_pct: storage_split,
            split_resizing: false,
            status_wrap: None,
            main_editor: None,
            split_editor: None,
            merge_editor: None,
            merge_sync: Default::default(),
            main_editor_sync: Default::default(),
            split_editor_sync: Default::default(),
            file_preview: None,
            pdf_preview: None,
            preview_scroll: Rc::new(std::cell::RefCell::new(
                recur_content::preview::scroll::PreviewScrollStore::new(),
            )),
            image: Default::default(),
            media_zoom: None,
            editor_subs: Vec::new(),
            editor_fullscreen: false,
            window_was_fullscreen: false,
            fs_exit_visible: false,
            fs_exit_timer: None,
            fs_exit_hovered: false,
            last_window_fullscreen: false,
            main_surface_pref: MainSurface::Editor,
            review_unhid_for: String::new(),
            review_tab,
            review_refresh: 0,
            review_bound: Default::default(),
            review_restore: None,
            review_surface: None,
            review_sig: None,
            review_subs: Vec::new(),
            left_kind: CodePanelKey::Maestro,
            right_kind: CodePanelKey::Files,
            show_side: true,
            local_left_open: false,
            findings: Vec::new(),
            finding_busy_id: String::new(),
            findings_filter: FindingsFilter::Open,
            hits: Vec::new(),
            search_skipped_large: 0,
            search_truncated: false,
            search_offset: 0,
            search_regex: false,
            search_case: false,
            refs: Vec::new(),
            expanded_lists: BTreeSet::new(),
            scm_collapsed: BTreeSet::new(),
            scm_file_list_mode: file_list_mode,
            commit_busy: false,
            commit_msg_busy: false,
            problem_fix_busy: String::new(),
            problem_hovered: None,
            scm_hovered: None,
            scm_name_hover: None,
            scm_probes: Default::default(),
            inputs,
            findings_panel,
            artifacts_panel: None,
            layers: [
                recur_ui::pane::Layer::new(quick_open.clone(), cx),
                recur_ui::pane::Layer::new(prompts.clone(), cx),
            ],
            side_pane: None,
            empty_pane: None,
            toolbar_pane: None,
            scm_body_pane: None,
            crumb_pane: None,
            status_pane: None,
            status_corner: (false, false),
            head_layers: Vec::new(),
            quick_open,
            prompts,
            prompts_reply: None,
            chord_index: Rc::default(),
            chord_task: None,
            chord_key: String::new(),
            lsp_attach: None,
            lsp_attach_gen: 0,
            lsp_outline_timer: None,
            lsp_sync_task: None,
            prev_disabled_ls: None,
            dock: None,
            dock_subs: Vec::new(),
            term_tab_count: 0,
            term_max_phase: TermMaxPhase::Idle,
            term_max_anim: None,
            term_max_gen: 0,
            term_max_for_ws: String::new(),
            term_slot_h: 0.,
            overlay_keep_editor_hidden: false,
            overlay_rect: OverlayRect::default(),
            term_open_for_ws: String::new(),
            term_was_open: false,
            term_beside_resizing: false,
            bounds: Default::default(),
            was_shell_active: true,
            watch_armed_for: String::new(),
            emitted_maestro_visible: None,
            emitted_agent_lit: None,
            emitted_artifact: None,
            event_task: None,
            anim_epoch: Instant::now(),
            window: Some(window.window_handle()),
            weak: cx.entity().downgrade(),
            _subscriptions: Vec::new(),
            state,
        };
        o.subscribe(window, cx);
        o.set_props(props, window, cx);
        o
    }

    pub fn props(&self) -> &ObservatoryProps {
        &self.props
    }

    pub fn state(&self) -> &Entity<AppState> {
        &self.state
    }

    pub fn services(&self) -> &Services {
        &self.services
    }

    /// Host-owned content (snippets).
    pub fn set_slots(&mut self, slots: ObservatorySlots, cx: &mut Context<Self>) {
        self.slots = slots;
        cx.notify();
    }

    // ---- derived: workspace, project ----

    pub(crate) fn worktree_path(&self, cx: &App) -> String {
        if self.props.workspace_id.is_empty() {
            return String::new();
        }
        self.state
            .read(cx)
            .workspace(&self.props.workspace_id)
            .map(|w| w.worktree_path.clone())
            .unwrap_or_default()
    }

    /// `workspaceRoot`: the worktree without a trailing slash.
    pub(crate) fn workspace_root(&self, cx: &App) -> String {
        self.worktree_path(cx).trim_end_matches('/').to_string()
    }

    /// Source Control and git polls only apply to git checkouts.
    pub(crate) fn is_git(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .project(&self.props.project_id)
            .is_some_and(|p| p.is_git)
    }

    /// Mutating tree ops need a workspace worktree.
    pub(crate) fn tree_writable(&self) -> bool {
        !self.props.workspace_id.is_empty()
    }

    /// The review surface is the main surface (the toolbar's Review changes toggle is on).
    pub fn showing_review(&self, cx: &App) -> bool {
        self.main_surface(cx) == MainSurface::Review
    }

    /// The main area's bounds from the last frame (the editor or the review).
    pub fn main_bounds(&self) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        self.bounds.main.get()
    }

    /// What the toolbar's Review changes button does.
    pub fn click_review_toggle(&mut self, cx: &mut Context<Self>) {
        self.toggle_review(cx);
    }

    pub(crate) fn main_surface(&self, cx: &App) -> MainSurface {
        if self.is_git(cx) && self.main_surface_pref == MainSurface::Review {
            MainSurface::Review
        } else {
            MainSurface::Editor
        }
    }

    /// The base branch label for review (`reviewBase`): empty on the main working copy.
    pub(crate) fn review_base(&self, cx: &App) -> String {
        let s = self.state.read(cx);
        let Some(ws) = s.workspace(&self.props.workspace_id) else {
            return String::new();
        };
        if ws.is_main {
            return String::new();
        }
        let b = if ws.base_branch.is_empty() {
            s.project(&self.props.project_id)
                .map(|p| p.base_branch.clone())
                .unwrap_or_default()
        } else {
            ws.base_branch.clone()
        };
        b.trim().to_string()
    }

    pub(crate) fn review_has_commits(&self, cx: &App) -> bool {
        self.is_git(cx)
    }

    pub(crate) fn review_strip_tab(&self, cx: &App) -> ReviewTab {
        if !self.review_has_commits(cx) && self.review_tab == ReviewTab::Commits {
            ReviewTab::Changes
        } else {
            self.review_tab
        }
    }

    // ---- derived: dock chrome ----

    pub(crate) fn term_open(&self, cx: &App) -> bool {
        self.state.read(cx).dock.is_open(&self.props.workspace_id)
    }

    pub(crate) fn term_maximized(&self, cx: &App) -> bool {
        self.term_open(cx)
            && self
                .state
                .read(cx)
                .dock
                .is_maximized(&self.props.workspace_id)
    }

    pub(crate) fn editor_hidden(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .dock
            .is_editor_hidden(&self.props.workspace_id)
    }

    pub(crate) fn term_beside(&self, cx: &App) -> bool {
        self.state.read(cx).dock.is_beside(&self.props.workspace_id)
    }

    pub(crate) fn term_beside_pct(&self, cx: &App) -> i64 {
        self.state
            .read(cx)
            .dock
            .beside_pct(&self.props.workspace_id)
    }

    pub(crate) fn term_overlaying(&self) -> bool {
        self.term_max_phase != TermMaxPhase::Idle
    }

    pub(crate) fn editor_tucked(&self, cx: &App) -> bool {
        if self.term_overlaying() {
            self.overlay_keep_editor_hidden
        } else {
            self.editor_hidden(cx)
        }
    }

    pub(crate) fn term_beside_layout(&self, cx: &App) -> bool {
        self.term_open(cx) && self.term_beside(cx) && !self.editor_tucked(cx)
    }

    // ---- derived: panels ----

    pub(crate) fn left_gutter_open(&self) -> bool {
        self.props.left_open.unwrap_or(self.local_left_open)
    }

    fn panel_available(&self, k: CodePanelKey) -> bool {
        match k {
            CodePanelKey::Maestro => self.slots.left_panel.is_some(),
            CodePanelKey::Plan => self.slots.plan_panel.is_some(),
            CodePanelKey::Findings | CodePanelKey::Artifacts => !self.props.workspace_id.is_empty(),
            _ => true,
        }
    }

    pub(crate) fn panels_on(&self, side: PanelSide, cx: &App) -> Vec<CodePanelKey> {
        self.state
            .read(cx)
            .panels
            .panels_on_side(side)
            .into_iter()
            .filter(|k| self.panel_available(*k))
            .collect()
    }

    pub(crate) fn panel_side(&self, k: CodePanelKey, cx: &App) -> PanelSide {
        self.state
            .read(cx)
            .panels
            .sides
            .get(&k)
            .copied()
            .unwrap_or_else(|| k.default_side())
    }

    pub(crate) fn has_left(&self, cx: &App) -> bool {
        !self.panels_on(PanelSide::Left, cx).is_empty()
    }

    pub(crate) fn has_right(&self, cx: &App) -> bool {
        !self.panels_on(PanelSide::Right, cx).is_empty()
    }

    /// The label (`label`) and tooltip title (`title`) of a panel.
    pub(crate) fn panel_label(&self, k: CodePanelKey) -> (String, String) {
        if k == CodePanelKey::Maestro && !self.props.left_title.is_empty() {
            return (self.props.left_title.clone(), self.props.left_title.clone());
        }
        let label = k.label().to_string();
        let title = if k == CodePanelKey::Refs {
            "References".to_string()
        } else {
            label.clone()
        };
        (label, title)
    }

    /// The left dock's kind, kept on a panel that exists on that side.
    pub(crate) fn left_meta(&self, cx: &App) -> Option<CodePanelKey> {
        let left = self.panels_on(PanelSide::Left, cx);
        if left.contains(&self.left_kind) {
            Some(self.left_kind)
        } else {
            left.first().copied()
        }
    }

    pub(crate) fn right_meta(&self, cx: &App) -> Option<CodePanelKey> {
        let right = self.panels_on(PanelSide::Right, cx);
        if right.contains(&self.right_kind) {
            Some(self.right_kind)
        } else {
            right.first().copied()
        }
    }

    pub(crate) fn maestro_dock_width(&self, cx: &App) -> f32 {
        self.state.read(cx).panels.size(PanelKey::MaestroDock)
    }

    pub(crate) fn obs_side_width(&self, cx: &App) -> f32 {
        self.state.read(cx).panels.size(PanelKey::ObsSide)
    }

    /// This Observatory's agent dock occupies the PR-detail slot.
    pub(crate) fn portal_agent_to_pr(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        if s.view != recur_view_core::ViewName::PrDetail || self.props.workspace_id.is_empty() {
            return false;
        }
        let linked = s
            .pr(&s.selected_pr_id)
            .map(|p| p.workspace_id.as_str())
            .unwrap_or("");
        recur_view_core::pr::pr_agent_workspace_id(&s.selected_pr_id, linked, &s.workspaces)
            == self.props.workspace_id
    }

    /// The Artifacts list is actually on screen.
    pub(crate) fn artifacts_panel_focused(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        self.props.shell_active
            && s.view == recur_view_core::ViewName::Detail
            && !self.term_overlaying()
            && match self.panel_side(CodePanelKey::Artifacts, cx) {
                PanelSide::Left => {
                    self.has_left(cx)
                        && self.left_gutter_open()
                        && self.left_kind == CodePanelKey::Artifacts
                }
                PanelSide::Right => {
                    self.has_right(cx)
                        && self.show_side
                        && self.right_kind == CodePanelKey::Artifacts
                }
            }
    }

    /// The agent transcript is on screen (`maestroPanelVisible`).
    pub fn maestro_panel_visible(&self, cx: &App) -> bool {
        let portal = self.portal_agent_to_pr(cx);
        let s = self.state.read(cx);
        self.props.shell_active
            && self.slots.left_panel.is_some()
            && (s.view == recur_view_core::ViewName::Detail || portal)
            && (portal || !self.term_overlaying())
            && if portal {
                s.pr_agent_dock_open
            } else {
                match self.panel_side(CodePanelKey::Maestro, cx) {
                    PanelSide::Left => {
                        self.has_left(cx)
                            && self.left_gutter_open()
                            && self.left_kind == CodePanelKey::Maestro
                    }
                    PanelSide::Right => {
                        self.has_right(cx)
                            && self.show_side
                            && self.right_kind == CodePanelKey::Maestro
                    }
                }
            }
    }

    /// The Files explorer is the active panel and its dock is showing.
    pub(crate) fn files_explorer_is_showing(&self, cx: &App) -> bool {
        match self.panel_side(CodePanelKey::Files, cx) {
            PanelSide::Left => {
                self.has_left(cx)
                    && self.left_gutter_open()
                    && self.left_kind == CodePanelKey::Files
            }
            PanelSide::Right => {
                self.has_right(cx) && self.show_side && self.right_kind == CodePanelKey::Files
            }
        }
    }

    // ---- derived: explorer scope ----

    /// Paths changed since base (git status ∪ changed files), locale-sorted.
    /// The changed-since-base paths plus every git status path, sorted (memoized: the explorer,
    /// the review count and the panel badges read it on every render).
    pub(crate) fn scope_paths(&self) -> Rc<Vec<String>> {
        let key = self.paths_gen;
        let mut memo = self.paths_memo.borrow_mut();
        if let Some((k, v)) = &memo.scope
            && *k == key
        {
            return v.clone();
        }
        let mut set: BTreeSet<String> = self.changed_paths.iter().cloned().collect();
        set.extend(self.git_status.keys().cloned());
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort_by(|a, b| recur_code::collate::locale_compare(a, b));
        let v = Rc::new(v);
        memo.scope = Some((key, v.clone()));
        v
    }

    pub(crate) fn exclude_matchers(&self, cx: &App) -> Rc<Vec<CompiledGlob>> {
        let patterns = crate::explorer::explorer_exclude(self.state.read(cx).storage());
        let mut memo = self.paths_memo.borrow_mut();
        if let Some((p, m)) = &memo.excludes
            && *p == patterns
        {
            return m.clone();
        }
        let m = Rc::new(compile_excludes(&patterns));
        memo.excludes = Some((patterns, m.clone()));
        m
    }

    /// [`crate::explorer::dir_status_index`] of the git status (memoized: every explorer row
    /// reads it).
    pub(crate) fn dir_status(&self) -> Rc<BTreeMap<String, String>> {
        let key = self.paths_gen;
        let mut memo = self.paths_memo.borrow_mut();
        if let Some((k, v)) = &memo.dir_status
            && *k == key
        {
            return v.clone();
        }
        let v = Rc::new(crate::explorer::dir_status_index(&self.git_status));
        memo.dir_status = Some((key, v.clone()));
        v
    }

    /// Whether the exclude globs hide `path` (memoized per path until the globs change).
    pub(crate) fn excluded(&self, path: &str, matchers: &Rc<Vec<CompiledGlob>>) -> bool {
        let mut memo = self.paths_memo.borrow_mut();
        if memo
            .excluded
            .as_ref()
            .is_none_or(|(m, _)| !Rc::ptr_eq(m, matchers))
        {
            memo.excluded = Some((matchers.clone(), HashMap::new()));
        }
        let (_, by) = memo.excluded.as_mut().expect("excluded memo");
        if let Some(&x) = by.get(path) {
            return x;
        }
        let x = is_excluded_by(path, matchers);
        by.insert(path.to_string(), x);
        x
    }

    /// The tree of [`Observatory::review_paths`] (memoized).
    pub(crate) fn changed_tree(&self, cx: &App) -> Rc<Vec<TreeEntry>> {
        let paths = self.review_paths(cx);
        let mut memo = self.paths_memo.borrow_mut();
        if let Some((p, t)) = &memo.changed_tree
            && Rc::ptr_eq(p, &paths)
        {
            return t.clone();
        }
        let t = Rc::new(recur_code::view::changed_tree::build_tree_from_paths(
            &paths,
        ));
        memo.changed_tree = Some((paths, t.clone()));
        t
    }

    /// Changed-since-base membership after the exclude globs (`reviewPaths`).
    pub(crate) fn review_paths(&self, cx: &App) -> Rc<Vec<String>> {
        let m = self.exclude_matchers(cx);
        let scope = self.scope_paths();
        let mut memo = self.paths_memo.borrow_mut();
        if let Some((sc, ex, v)) = &memo.review
            && Rc::ptr_eq(sc, &scope)
            && Rc::ptr_eq(ex, &m)
        {
            return v.clone();
        }
        let v = Rc::new(
            scope
                .iter()
                .filter(|p| !is_excluded_by(p, &m))
                .cloned()
                .collect(),
        );
        memo.review = Some((scope, m, Rc::clone(&v)));
        v
    }

    pub(crate) fn filter_changed(&self) -> bool {
        !self.browse_all && !self.scope_paths().is_empty()
    }

    pub(crate) fn show_changed_first(&self) -> bool {
        self.props.loop_scoped && !self.browse_all && !self.changed_paths.is_empty()
    }

    pub(crate) fn working_dirty_count(&self) -> usize {
        self.working_status.len()
    }

    pub(crate) fn open_findings_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.status.is_empty() || f.status == "open")
            .count()
    }

    pub(crate) fn plan_open_count(&self, cx: &App) -> usize {
        self.state
            .read(cx)
            .workspace(&self.props.workspace_id)
            .and_then(|w| w.plan.as_ref())
            .map(|p| {
                p.steps
                    .iter()
                    .filter(|s| s.status != "done" && s.status != "skipped")
                    .count()
            })
            .unwrap_or(0)
    }

    pub(crate) fn unseen_chat_count(&self, cx: &App) -> usize {
        if self.props.workspace_id.is_empty() {
            return 0;
        }
        self.state
            .read(cx)
            .attention
            .workspace(&self.props.workspace_id)
            .max(0) as usize
    }

    pub(crate) fn unseen_artifact_count(&self, cx: &App) -> usize {
        crate::unseen_artifacts(cx)
            .get(&self.props.workspace_id)
            .map_or(0, Vec::len)
    }

    /// The focused pane's path drives chrome (breadcrumb, status) and where opens land.
    pub(crate) fn active_path(&self) -> &str {
        if self.focused_pane == Pane::Split && self.split_open && !self.split_path.is_empty() {
            &self.split_path
        } else {
            &self.selected_path
        }
    }

    pub(crate) fn selected_git_status(&self) -> &str {
        if self.selected_path.is_empty() {
            return "";
        }
        self.git_status
            .get(&self.selected_path)
            .map_or("", String::as_str)
    }

    /// The artifact open in the focused pane, if any.
    /// The file open in the main editor.
    pub fn selected_path(&self) -> &str {
        &self.selected_path
    }

    /// The explorer row highlighted as current (`treePath`).
    pub fn tree_path(&self) -> &str {
        &self.tree_path
    }

    /// The panel the right dock shows.
    pub fn right_panel(&self, cx: &App) -> Option<CodePanelKey> {
        self.right_meta(cx)
    }

    pub fn selected_artifact_id(&self) -> String {
        recur_engine::view::artifacts::artifact_id_from_scratch_path(self.active_path())
    }

    /// The focus zone (amber tab underline).
    pub fn focus_zone(&self) -> FocusZone {
        self.focus_zone
    }

    /// `findingRelPath`: a finding file normalized to the editor's rel path.
    pub(crate) fn finding_rel_path(&self, file: &str, cx: &App) -> String {
        let wt = self.worktree_path(cx);
        let wt = wt.trim_end_matches('/');
        let mut p = file.replace('\\', "/");
        if let Some(rest) = p.strip_prefix("./") {
            p = rest.to_string();
        }
        if !wt.is_empty() && (p == wt || p.starts_with(&format!("{wt}/"))) {
            p = p.get(wt.len() + 1..).unwrap_or("").to_string();
        }
        p
    }
}
