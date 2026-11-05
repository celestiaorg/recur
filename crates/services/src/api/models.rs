//! Service models: the DTOs the services take and return that no lower
//! crate owns (those are re-exported from [`crate::api`]). They follow the
//! `recur-domain` conventions: `omitempty` fields skip their zero value, and
//! a non-`omitempty` slice or map is `Option<…>` because it can be `null`.

use std::collections::BTreeMap;

use recur_domain::{IssueRef, PrParticipant};
use serde::{Deserialize, Serialize};

use super::serde_util::{is_false, is_zero, is_zero_f64};

/// AppChatSelection is the last Ask Recur model picker. Mode is always ask.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppChatSelection {
    pub model: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, String>,
}

/// AssignedIssue is one open issue that is assigned to the user or unassigned
/// in their repos / Linear teams, from GitHub or Linear.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AssignedIssue {
    /// "github" | "linear"
    pub source: String,
    /// stable tracker id; matches Workspace.Issue.ID
    pub id: String,
    /// owner/repo#12 or ENG-123
    pub r#ref: String,
    pub title: String,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
    /// "progress" | "todo" | "backlog"
    pub group: String,
    pub state: String,
    pub state_color: String,
    /// Linear workflow order within a team
    pub state_position: f64,
    /// Linear: 0 none, 1 urgent … 4 low
    pub priority: i64,
    pub priority_label: String,
    /// owner/repo, or Linear project / team
    pub project: String,
    pub labels: Option<Vec<IssueLabel>>,
    pub comments: i64,
    pub assignees: Option<Vec<PrParticipant>>,
    /// ProjectID is the Recur project this issue belongs to, or "" when no
    /// project's git remotes (GitHub) or LinearProjects setting (Linear) match.
    pub project_id: String,
    pub linear_project: String,
    /// team key, e.g. ENG
    pub linear_team: String,
}

/// IssueLabel is a label chip; Color is a hex string with or without '#'.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IssueLabel {
    pub name: String,
    pub color: String,
}

/// BundleSettingInfo is one [[setting]] for Settings → Integrations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BundleSettingInfo {
    pub id: String,
    pub name: String,
    pub label: String,
    pub description: String,
    pub secret: bool,
    pub placeholder: String,
    pub url: String,
    pub url_label: String,
    pub bundle_id: String,
    pub bundle_name: String,
    pub source_root: String,
}

/// BundleSkipInfo is a bundle directory Discover skipped (Settings / diagnostics).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BundleSkipInfo {
    pub dir: String,
    pub name: String,
    pub source: String,
    pub reason: String,
}

/// ClaudeStatus is what the Settings screen renders for the Claude Code account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClaudeStatus {
    /// Authenticated reports whether `claude auth status` shows a logged-in user.
    pub authenticated: bool,
    /// Path is the resolved Claude Code binary on PATH ("" when not found).
    pub path: String,
    /// Version is the Claude Code CLI version ("" when unknown).
    pub version: String,
    /// Provider is a human-readable API provider label (e.g. "Anthropic API").
    pub provider: String,
    /// LoginMethod is a human-readable login/plan label (e.g. "Claude Team account").
    pub login_method: String,
    /// Organization is the Claude org name when present.
    pub organization: String,
    /// Email is the logged-in account email when present.
    pub email: String,
    /// Error is a short user-facing reason when not authenticated.
    pub error: String,
}

/// CodexStatus is what Settings renders for the Codex provider.
/// Fields mirror ClaudeStatus: discrete Path / login / email / plan rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CodexStatus {
    pub bin_installed: bool,
    pub bin_path: String,
    pub authenticated: bool,
    /// LoginMethod is a human-readable auth label (e.g. "API key", "ChatGPT").
    pub login_method: String,
    /// Email is the account email when present (ChatGPT login).
    pub email: String,
    /// Plan is the plan/tier label when present (e.g. "Pro").
    pub plan: String,
    pub error: String,
}

/// CommandInfo is the plain-struct view of a bundle command for the UI.
/// No TOML crosses the service boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CommandInfo {
    pub id: String,
    pub name: String,
    pub category: String,
    pub order: i64,
    pub scope: String,
    pub file_glob: String,
    pub input: String,
    pub fallback_input: String,
    pub output: String,
    pub triggers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<CommandPromptDto>,
    pub key_equivalent: String,
    pub language: String,
    pub open_in_preview: bool,
    pub interpreter: String,
    pub bundle_name: String,
    pub source_root: String,
}

/// CommandPromptDto is a prompt field for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CommandPromptDto {
    pub name: String,
    pub label: String,
    pub r#type: String,
    pub default: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// ConductorImport is what an Import from Conductor run did. Skipped rows
/// read "<title>: <reason>".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConductorImport {
    pub projects_added: usize,
    pub workspaces_imported: usize,
    pub chats_imported: usize,
    pub skipped: Vec<String>,
}

/// EditorBuffer is one unsaved disk buffer: the user's draft plus a hash of the
/// on-disk text it diverged from. The UI computes and compares the hash;
/// this service just stores it. Keeping a hash instead of the full saved copy
/// halves the payload and still lets restore detect "disk moved underneath the
/// stash" (agents rewrite worktree files while tabs sit closed).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorBuffer {
    pub draft: String,
    pub base_hash: String,
}

/// EditorFileView is per-file caret, scroll, and Code|Preview|Diff. Coordinates
/// are 1-based (line/col/topLine); pixels are a fallback when topLine is absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorFileView {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub view_mode: String,
    pub line: i64,
    pub col: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub anchor_line: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub anchor_col: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub top_line: i64,
    pub scroll_top: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub scroll_left: i64,
    /// FilePreview scroller. Independent of CodeMirror ScrollTop.
    #[serde(skip_serializing_if = "is_zero")]
    pub preview_scroll_top: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub preview_scroll_left: i64,
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub preview_zoom: f64,
}

/// EditorScratchDoc is a `scratch://…` virtual document (command output, notes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorScratchDoc {
    pub title: String,
    pub content: String,
    pub lang: String,
    /// When set (e.g. image/*), the observatory renders as binary instead of text.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mime: String,
}

/// EditorSession is the full per-workspace snapshot, as stored on disk and as
/// returned to the UI on restore.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorSession {
    pub open_files: Option<Vec<String>>,
    pub selected_path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub split_path: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub scratch_seq: i64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub scratch_docs: BTreeMap<String, EditorScratchDoc>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub buffers: BTreeMap<String, EditorBuffer>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub file_views: BTreeMap<String, EditorFileView>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub syntax_by_path: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub syntax_by_file_type: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub main_surface: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub left_kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub right_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_open: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_open: Option<bool>,
}

/// EditorSessionMeta is the cheap, often-rewritten part of a session: tab strip,
/// selection, split pane, scratch tab sequence, per-file view state,
/// TextMate language-mode maps, the Observatory Editor vs Review column, and
/// left/right pane kind plus visibility. A patch replaces it wholesale.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorSessionMeta {
    pub open_files: Option<Vec<String>>,
    pub selected_path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub split_path: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub scratch_seq: i64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub file_views: BTreeMap<String, EditorFileView>,
    /// Syntax maps are optional on a meta patch: nil (JSON omitted) means
    /// unchanged, so a tab-strip write before restore cannot wipe stored picks.
    /// A non-nil empty map clears.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syntax_by_path: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syntax_by_file_type: Option<BTreeMap<String, String>>,
    /// "review" keeps the Review column on cold remount. Omitted / anything
    /// else is the Editor column.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub main_surface: String,
    /// Observatory pane chrome. Empty kind is the side default (maestro / files).
    /// Open flags are pointers so JSON omitempty does not treat a hidden pane
    /// (`false`) as "unset": nil means default-open, matching old sessions.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub left_kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub right_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_open: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_open: Option<bool>,
}

/// EditorSessionPatch is one incremental update. Meta replaces wholesale when
/// present; Buffers/ScratchDocs upsert by path; Drop* delete by path. Fields
/// left nil mean "unchanged".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorSessionPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<EditorSessionMeta>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub buffers: BTreeMap<String, EditorBuffer>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drop_buffers: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub scratch_docs: BTreeMap<String, EditorScratchDoc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drop_scratch_docs: Vec<String>,
}

/// GrammarInfo is a TextMate grammar contributed by a bundle (metadata only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GrammarInfo {
    pub scope_name: String,
    pub id: String,
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub file_types: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub first_line_match: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inject_to: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub embedded_languages: BTreeMap<String, String>,
    pub bundle_name: String,
    pub source_root: String,
    /// Optional [[languageData]] merged by scopeName (and dotted prefixes).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub line_comment: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub block_comment_start: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub block_comment_end: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub increase_indent_pattern: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub decrease_indent_pattern: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub indent_next_line_pattern: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub zero_indent_pattern: String,
    #[serde(skip_serializing_if = "is_false")]
    pub disable_indent_corrections: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub close_brackets: Vec<String>,
}

/// IssuesView is the Issues page's layout and filters. The UI validates
/// values on load, so an unknown or stale entry falls back to its default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IssuesView {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub layout: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sort: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
}

/// LoopInfo is a bundle [[loop]] for the dock / New Task picker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LoopInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub entry: String,
    pub bundle_name: String,
    pub source_root: String,
}

/// PaletteSearchDto is the outcome of SearchPalette.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PaletteSearchDto {
    pub groups: Option<Vec<PaletteSearchGroupDto>>,
}

/// PaletteSearchGroupDto is one ⌘K section from bundle [[search]] items.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PaletteSearchGroupDto {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub icon_svg: String,
    pub hits: Option<Vec<PromptCompleteHitDto>>,
}

/// PolicyInfo is a bundle [[policy]] for the dock / New Task picker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PolicyInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub max_rounds: i64,
    pub on_max_rounds: String,
    pub plan_approval: String,
    pub extend_rounds: i64,
    pub bundle_name: String,
    pub source_root: String,
}

/// PromptActionInfo is the plain-struct view of a [[prompt]] for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptActionInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub slash: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub r#loop: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub timeout_ms: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub fresh_session: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tab_group: String,
    pub interpreter: String,
    #[serde(skip_serializing_if = "is_false")]
    pub takes_args: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub has_complete: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub arg_placeholder: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub complete_interpreter: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub icon_svg: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<PromptArgInfo>,
    pub bundle_name: String,
    pub source_root: String,
}

/// PromptArgInfo is one named [[prompt.arg]] slot for the composer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptArgInfo {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    #[serde(skip_serializing_if = "is_false")]
    pub optional: bool,
}

/// PromptCompleteHitDto is one trailing-arg suggestion from a complete script.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptCompleteHitDto {
    pub insert: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub psub: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub badge: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub action: String,
}

/// PromptCompleteRequestDto is the composer slot state for RunPromptComplete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptCompleteRequestDto {
    /// Args is the active slot's current value (also ctx.args / RECUR_PROMPT_ARGS).
    pub args: String,
    /// Arg is the [[prompt.arg]] name being completed (ctx.arg / RECUR_PROMPT_COMPLETE_ARG).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub arg: String,
    /// Params are all named slot values so far.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// WorkspaceID sets cwd to the workspace worktree so tools like `gh` see the right repo.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// PromptCompleteResultDto is the outcome of RunPromptComplete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptCompleteResultDto {
    pub hits: Option<Vec<PromptCompleteHitDto>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

/// PromptContextDto is workspace + editor state the UI sends for RunPromptAction.
/// Workspace fields may be filled in by the UI (no store lookup required).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptContextDto {
    pub workspace_id: String,
    pub project_id: String,
    pub worktree: String,
    pub port: i64,
    pub selected_text: String,
    pub document: String,
    pub current_line: String,
    pub line_number: i64,
    pub column: i64,
    pub current_word: String,
    pub filepath: String,
    pub filename: String,
    pub scope: String,
    pub clipboard: String,
    pub tab_size: i64,
    pub soft_tabs: bool,
    pub work_dir: String,
    /// TimeoutMs overrides the default / manifest shebang deadline when > 0.
    #[serde(skip_serializing_if = "is_zero")]
    pub timeout_ms: i64,
    /// Surface is the operator's looking-at view: workspace, local_pr, github_pr.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    /// ChatCode is the chat tab the prompt will be sent to (PA, PA2, …).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub chat_code: String,
    /// Args is the trailing composer remainder after `/slash`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub args: String,
    /// Params are named [[prompt.arg]] values.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

/// PromptResultDto is the outcome of RunPromptAction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptResultDto {
    pub prompt: String,
    pub display: String,
    /// FreshSession, when set, overrides the manifest fresh_session flag for
    /// this send (a /re-review continuing the same chat's review resumes it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fresh_session: Option<bool>,
    /// Issue is the tracker issue the prompt is about (e.g. /linear). The UI
    /// links it to the workspace the prompt runs in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueRef>,
}

/// RoleInfo is a bundle [[role]] for pickers / Settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RoleInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub code: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model_slot: String,
    pub bundle_name: String,
    pub source_root: String,
}

/// RunContextDto is the editor state the UI sends for RunCommand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunContextDto {
    pub selected_text: String,
    pub current_line: String,
    pub line_number: i64,
    pub column_number: i64,
    pub current_word: String,
    pub file_path: String,
    pub file_name: String,
    pub directory: String,
    pub project_directory: String,
    pub scope: String,
    pub tab_size: i64,
    pub soft_tabs: bool,
    pub clipboard: String,
    pub input: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub prompt_values: BTreeMap<String, String>,
    pub work_dir: String,
    /// TimeoutMs overrides the default 30s shebang deadline when > 0 (e.g. willSave).
    #[serde(skip_serializing_if = "is_zero")]
    pub timeout_ms: i64,
}

/// RunResultDto is the outcome of RunCommand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunResultDto {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
    pub output: String,
}

/// ShellState is the last top-level UI the user was looking at. Persisted under
/// the managed data root (next to window.json) so the open workspace/view
/// survives app restarts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShellState {
    pub view: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project_id: String,
}

/// SnippetInfo is a bundle [[snippet]] for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SnippetInfo {
    pub id: String,
    pub name: String,
    pub tab_trigger: String,
    pub scope: String,
    pub file_glob: String,
    pub body: String,
    pub key_equivalent: String,
    pub order: i64,
    pub bundle_name: String,
    pub source_root: String,
}

/// SymbolInfo is a bundle [[symbol]] rule for the open-file outline.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SymbolInfo {
    pub id: String,
    pub scope: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub transform: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub show: bool,
    pub bundle_name: String,
    pub source_root: String,
}

/// TerminalDockChrome is the Observatory dock layout for one workspace.
/// ListTerminalDockChrome returns these at boot; SetTerminalDockChrome
/// writes them onto terminals.json.gz next to the tab snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TerminalDockChrome {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    pub open: bool,
    pub maximized: bool,
    pub editor_hidden: bool,
    pub beside: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub beside_pct: i64,
}

/// TerminalPaletteInfo is an xterm.js ITheme from [[terminalTheme]].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TerminalPaletteInfo {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub background: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub foreground: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cursor: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cursor_accent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub selection_background: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub selection_foreground: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub selection_inactive_background: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub black: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub red: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub green: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub yellow: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub blue: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub magenta: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cyan: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub white: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bright_black: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bright_red: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bright_green: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bright_yellow: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bright_blue: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bright_magenta: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bright_cyan: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bright_white: String,
}

/// TerminalThemeInfo is an xterm.js ITheme contributed by a bundle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TerminalThemeInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub ui_theme: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub order: i64,
    pub palette: TerminalPaletteInfo,
    pub bundle_name: String,
    pub source_root: String,
}

/// ThemeInfo is a TextMate / VS Code theme contributed by a bundle (metadata only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub ui_theme: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub order: i64,
    pub preview: ThemePreviewInfo,
    pub bundle_name: String,
    pub source_root: String,
}

/// ThemePreviewInfo is Settings picker swatches from [[theme]].preview.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemePreviewInfo {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bg: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fg: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub r#fn: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub string: String,
}

/// WorkspaceChatSelection is the last model + flags + mode the solo/pair chat
/// dock used for a chat tab. Empty Model means fall back to that mode's Settings
/// → Models default; empty Mode means default to agent. Simplify is the
/// composer chip for this session (nil = never set → Settings default).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceChatSelection {
    pub model: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simplify: Option<bool>,
}
