//! `AgentChat.svelte` as an entity: the transcript, the live turn chrome, the inline cards and
//! the composer dock. Hosts set [`ChatProps`] (the Svelte props), declare which callbacks
//! they handle in [`ChatHooks`], subscribe to [`ChatEvent`] (the callbacks), and read the
//! draft back (`bind:input` / `bind:attachments`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter,
    FocusHandle, Focusable, ScrollHandle, Subscription, Task, Window, point, px,
};
use recur_agent::context_usage::ContextUsage;
use recur_content::{MarkdownView, StreamingMarkdown};
use recur_domain::{Escalation, Finding, PermissionRequest, UserQuestion};
use recur_engine::view::chat::{
    AgentMsg, ChatAttachmentDraft, ChatLogSession, current_turn_has_ended,
};
use recur_engine::view::composer::{ComposerAtt, HydrateOpts, PromptChipSpec};
use recur_engine::view::digest::{TurnDigestFinding, TurnDigestNote, still_open_findings_for_turn};
use recur_engine::view::mentions::{
    MentionQuery, extract_attachment_ids, strip_attachment_markers, unique_artifact_id_for_title,
};
use recur_engine::view::prompts::{
    PromptArgsQuery, parse_committed_prompt_line, prompt_action_query_at, prompt_args_query_at,
};
use recur_services::Services;
use recur_services::api::{PromptActionInfo, PromptCompleteHitDto};
use recur_view_core::chat::{WorkspaceArtifact, WorkspaceTerminalTab};
use recur_view_core::nav::ViewName;
use recur_view_core::{AppState, ShellEvent, ViewContext};

use crate::blocks::{Block, block_keys, group_blocks, last_retry_error_index};
use crate::composer_doc::{ChipKind, ComposerDoc};
use crate::composer_undo::{ComposerUndo, UndoSnap};
use crate::findings_ui::digest_ref;
use crate::flow::FlowLayout;
use crate::glance::is_first_checkpoint;
use crate::prompt_actions::{enabled_prompts, prompt_action};
use crate::tools::ToolCx;
use crate::transcript_ui::{
    LiveChrome, ScrollRestoreGate, TranscriptUiCache, TranscriptUiState, expansion_still_valid,
    next_near_bottom, restore_block_heights, scroll_restore_ready, scroller_near_bottom,
    snapshot_transcript_ui, transcript_fingerprint,
};
use crate::window::{
    BlockRange, CHAT_HYDRATE_ALL_BELOW, CHAT_HYDRATE_MAX, CHAT_HYDRATE_OVERSCAN, CHAT_VIEW_PAD_PX,
    block_at_scroll_top, in_view_keys, pick_hydrated,
};

/// Which surface the chat serves: a workspace chat, or the restricted Ask Recur panel (no
/// mentions, attachments, forks, or workspace permission / question cards).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatSurfaceKind {
    #[default]
    Workspace,
    App,
}

/// The Svelte props (everything but the bindable draft, the snippets and the callbacks).
#[derive(Clone, Debug)]
pub struct ChatProps {
    pub messages: Vec<AgentMsg>,
    pub sending: bool,
    /// A busy turn: ⏎ injects the follow-up into the live turn instead of queueing.
    pub steer_follow_up: bool,
    /// A busy turn can take a steer; ⌘⏎ does whichever of steer / queue ⏎ does not.
    pub can_steer: bool,
    /// Durable turn start (ms): elapsed survives remounts and workspace switches.
    pub turn_started_at: i64,
    /// Setup / wait copy shown as a turn-status row while the worktree allocates.
    pub status_text: String,
    /// A loop walk is live on this tab: Stop even when the conductor is not sending.
    pub can_stop: bool,
    /// Idle with held follow-ups (after Stop): Send / Enter flushes the queue.
    pub can_flush_queue: bool,
    /// Tighter padding for the Code-tab dock.
    pub compact: bool,
    /// The live workbench. A parked hot shell stays mounted but must not steal keys.
    pub shell_active: bool,
    /// Actual panel visibility, including closed docks and other app views.
    pub presentation_active: bool,
    /// Chat identity (workspace + agent code). Switching restores that tab's chrome.
    pub reset_key: String,
    pub empty_text: String,
    pub placeholder: String,
    /// Last-turn context fill for the composer meter (hidden until ready).
    pub context_usage: Option<ContextUsage>,
    /// Worktree root, stripped from displayed paths.
    pub root_path: String,
    pub project_id: String,
    pub workspace_id: String,
    /// Agent tab code (PA / PA2 / MA): scopes inline permission prompts.
    pub agent_code: String,
    /// The open loop escalation: the same inline card as ask_user, on the MA tab.
    pub escalation: Option<Escalation>,
    /// Transcript only: no composer dock (SessionCard in the Loop cockpit).
    pub readonly: bool,
    /// Workspace findings: resolves update_finding rows and the turn digest.
    pub findings: Vec<Finding>,
    /// A prompt script is running (spinner / disable).
    pub prompt_busy: bool,
    pub surface: ChatSurfaceKind,
    /// Composer only (the New-workspace modal): no transcript, queue, or send row.
    pub composer_only: bool,
    /// Inside a `.fanout-transcript .fanout-body` (a specialist or child row's transcript):
    /// hugs its content up to `session_max`, 2px 10px 6px padding, full-width column,
    /// 6px user bubble margins and 20px edge fades.
    pub session_body: bool,
    /// `--sess-body-max`: 420px in a session card, 360px under a fan-out row.
    pub session_max: f32,
    /// The inherited font size: `body`'s 13px, 12px in an Observatory panel (`.obs-left`).
    pub font_size: f32,
}

impl Default for ChatProps {
    fn default() -> Self {
        ChatProps {
            messages: Vec::new(),
            sending: false,
            steer_follow_up: false,
            can_steer: false,
            turn_started_at: 0,
            status_text: String::new(),
            can_stop: false,
            can_flush_queue: false,
            compact: false,
            shell_active: true,
            presentation_active: true,
            reset_key: String::new(),
            empty_text: "Ask where this loop stands, or steer it — it can re-plan within your rails, but never cross them.".into(),
            placeholder: "Ask where things stand, or steer the loop…".into(),
            context_usage: None,
            root_path: String::new(),
            project_id: String::new(),
            workspace_id: String::new(),
            agent_code: String::new(),
            escalation: None,
            readonly: false,
            findings: Vec::new(),
            prompt_busy: false,
            surface: ChatSurfaceKind::Workspace,
            composer_only: false,
            session_body: false,
            session_max: 420.,
            font_size: 13.,
        }
    }
}

/// Which callbacks the host handles (Svelte renders differently when one is absent).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChatHooks {
    pub send: bool,
    pub stop: bool,
    pub retry: bool,
    pub remove_queued: bool,
    pub cycle_mode: bool,
    pub open_file: bool,
    pub open_artifact: bool,
    pub answer_escalation: bool,
    pub fork: bool,
    pub fork_to_workspace: bool,
    pub turn_diff: bool,
    pub reset: bool,
    pub prompt_action: bool,
}

/// An artifact row to open (`ListedArtifact`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenArtifact {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub path: String,
    pub created_at: String,
    pub created_by: String,
}

/// The callbacks.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatEvent {
    /// `onSend(alt)`.
    Send {
        alt: bool,
    },
    Stop,
    Retry,
    RemoveQueued(String),
    CycleMode,
    OpenFile {
        path: String,
        line: Option<i64>,
        end_line: Option<i64>,
        start_col: Option<i64>,
        end_col: Option<i64>,
    },
    OpenArtifact(OpenArtifact),
    AnswerEscalation {
        choice: String,
        text: String,
    },
    Fork(String),
    ForkToWorkspace(String),
    TurnDiff(String),
    Reset {
        message_id: String,
        restores_files: bool,
    },
    PromptAction {
        slash: String,
        args: String,
        alt: bool,
    },
    /// The draft changed (`bind:input` / `bind:attachments`).
    Draft,
}

pub(crate) type TitleMap = HashMap<String, String>;
pub(crate) type ChatAct = Rc<dyn Fn(&mut AgentChat, &mut Context<AgentChat>)>;
pub(crate) type ChatFn = Box<dyn Fn(&mut AgentChat, &mut Context<AgentChat>)>;
pub(crate) type ChatWinFn = Box<dyn Fn(&mut AgentChat, &mut Window, &mut Context<AgentChat>)>;

/// Renders a specialist SessionCard for a cockpit session log (`sessionCard` snippet); the
/// flag is true for the first row of its card (`.fanout-row:first-of-type`).
pub type SessionCardRenderer =
    Rc<dyn Fn(&ChatLogSession, bool, &mut Window, &mut App) -> AnyElement>;

/// A skill in the `/` menu.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillHit {
    pub name: String,
    pub description: String,
    pub scope: String,
}

/// A `/` menu row: prompts first, then skills.
#[derive(Clone, Debug, PartialEq)]
pub enum SlashHit {
    Prompt {
        slash: String,
        name: String,
        description: String,
        takes_args: bool,
        args: Vec<String>,
        mode: String,
    },
    Skill(SkillHit),
}

/// A row of the prompt-argument menu.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptArgHit {
    Run {
        slash: String,
        args: String,
        name: String,
        description: String,
    },
    Suggest(PromptCompleteHitDto),
}

/// A row of the `@` menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MentionHit {
    File {
        item: String,
        label: String,
        score: i64,
        dir: bool,
    },
    Terminal {
        id: String,
        title: String,
        active: bool,
        exited: bool,
    },
    Artifact {
        id: String,
        title: String,
        kind: String,
    },
}

impl MentionHit {
    pub fn key(&self) -> String {
        match self {
            MentionHit::Terminal { id, .. } => format!("t:{id}"),
            MentionHit::Artifact { id, .. } => format!("a:{id}"),
            MentionHit::File { item, .. } => format!("f:{item}"),
        }
    }

    pub(crate) fn same(&self, o: &MentionHit) -> bool {
        if self.key() != o.key() {
            return false;
        }
        match (self, o) {
            (
                MentionHit::Terminal {
                    title: a,
                    active: aa,
                    exited: ae,
                    ..
                },
                MentionHit::Terminal {
                    title: b,
                    active: ba,
                    exited: be,
                    ..
                },
            ) => a == b && aa == ba && ae == be,
            (
                MentionHit::Artifact {
                    title: a, kind: ak, ..
                },
                MentionHit::Artifact {
                    title: b, kind: bk, ..
                },
            ) => a == b && ak == bk,
            (MentionHit::File { dir: a, .. }, MentionHit::File { dir: b, .. }) => a == b,
            _ => false,
        }
    }
}

use recur_ui::logic::hover_gate::HoverGate;

/// Escape dismissed this `@` / `/` query; keyup must not reopen it until it changes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Escaped {
    pub skill: bool,
    pub start: usize,
    pub query: String,
}

/// The fork menu under a turn footer.
#[derive(Clone, Debug, PartialEq)]
pub struct ForkMenu {
    pub message_id: String,
    pub left: f32,
    pub top: f32,
}

/// What the per-tool presentation reads, snapshotted per frame so rows can borrow it while
/// they build listeners.
#[derive(Default)]
pub(crate) struct ToolData {
    pub root_path: String,
    pub has_open_file: bool,
    pub findings: Vec<Finding>,
    pub prs: Vec<recur_domain::Pr>,
    pub workspace_id: String,
    pub briefs: Vec<recur_domain::WorkspaceBrief>,
}

impl ToolData {
    pub fn cx(&self) -> ToolCx<'_> {
        ToolCx {
            root_path: &self.root_path,
            has_open_file: self.has_open_file,
            findings: &self.findings,
            prs: &self.prs,
            workspace_id: &self.workspace_id,
            briefs: &self.briefs,
            workspaces: &[],
            projects: &[],
        }
    }
}

/// The `{src, name}` the lightbox shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Lightbox {
    pub src: String,
    pub name: String,
}

/// A deferred scroll restore (`pendingScroll`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingScroll {
    pub scroll_top: f32,
    pub near_bottom: bool,
    pub row_count: usize,
}

/// `RESTORE_LAYOUT_FRAMES`.
const RESTORE_LAYOUT_FRAMES: u32 = 16;

/// Clock refresh while a turn runs (`setInterval(tick, 250)`).
const CLOCK_TICK: Duration = Duration::from_millis(250);

/// The text a debug selector carries for a row: its first non-blank line, at most 60 chars.
pub fn debug_key(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(60)
        .collect()
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub struct AgentChat {
    pub(crate) state: Entity<AppState>,
    pub(crate) services: Services,
    pub(crate) props: ChatProps,
    pub(crate) hooks: ChatHooks,
    pub(crate) toolbar: Option<AnyView>,
    pub(crate) empty: Option<AnyView>,
    pub(crate) session_card: Option<SessionCardRenderer>,
    pub(crate) focus: FocusHandle,
    pub(crate) window_handle: gpui_kit::AnyWindowHandle,
    /// `crate::rows::tc_icon_shift` for this frame's fonts.
    pub(crate) tc_shift: f32,
    /// The window's device scale this frame (exact gaps).
    pub(crate) scale: f32,

    // ---- the draft ----
    pub(crate) doc: ComposerDoc,
    pub(crate) input: String,
    pub(crate) attachments: Vec<ChatAttachmentDraft>,
    pub(crate) undo: ComposerUndo<ChatAttachmentDraft>,
    /// Undo bursts are timed on the executor clock, so tests can advance it.
    pub(crate) undo_epoch: std::time::Instant,
    /// The composer's flow layout from the last frame (caret, hit tests).
    pub(crate) composer_layout: Rc<std::cell::RefCell<Option<FlowLayout>>>,
    pub(crate) composer_scroll: ScrollHandle,
    pub(crate) composer_marked: Option<std::ops::Range<usize>>,
    /// The prompt chip's slot inputs, by slot index.
    pub(crate) prompt_slots: Vec<Entity<gpui_kit::base::input::InputState>>,
    pub(crate) prompt_slot_subs: Vec<Subscription>,
    pub(crate) prompt_slot_focus: Option<usize>,
    pub(crate) caret_blink_epoch: std::time::Instant,
    /// When this view mounted: the clock CSS animations run on.
    pub(crate) anim_epoch: std::time::Instant,
    pub(crate) think_scrolls: HashMap<String, ScrollHandle>,
    pub(crate) tool_data: Rc<ToolData>,
    pub(crate) image_loading: HashMap<String, Task<()>>,
    /// Fork buttons' bounds from the last frame (the menu anchors to them).
    pub(crate) fork_anchor:
        Rc<std::cell::RefCell<HashMap<String, gpui_kit::Bounds<gpui_kit::Pixels>>>>,
    pub(crate) composer_geom: Rc<crate::dock::ComposerGeom>,
    pub(crate) meter: Entity<crate::meter::ContextMeter>,
    pub(crate) selecting: bool,

    // ---- menus ----
    pub(crate) skill_index: Vec<SkillHit>,
    pub(crate) skill_index_key: String,
    pub(crate) skill_index_loading: bool,
    pub(crate) skill_task: Option<Task<()>>,
    pub(crate) prompt_index: Rc<Vec<PromptActionInfo>>,
    pub(crate) prompt_index_loading: bool,
    pub(crate) prompt_task: Option<Task<()>>,
    pub(crate) mention: Option<MentionQuery>,
    pub(crate) skill_mention: Option<MentionQuery>,
    pub(crate) prompt_gate_open: bool,
    pub(crate) prompt_args: Option<PromptArgsQuery>,
    pub(crate) args_menu_escaped: bool,
    pub(crate) complete_hits: Vec<PromptCompleteHitDto>,
    pub(crate) complete_message: String,
    pub(crate) complete_gen: u64,
    pub(crate) complete_task: Option<Task<()>>,
    pub(crate) mention_sel: usize,
    pub(crate) mention_hits: Vec<MentionHit>,
    pub(crate) mention_search_gen: u64,
    pub(crate) search_task: Option<Task<()>>,
    pub(crate) file_index_loading: bool,
    pub(crate) file_index_tick: u64,
    pub(crate) hover_gate: HoverGate,
    pub(crate) mention_escaped: Option<Escaped>,
    pub(crate) listed_terminals_for_mention: bool,
    pub(crate) listed_artifacts_for_mention: bool,
    pub(crate) menu_scroll: ScrollHandle,
    pub(crate) last_menu_key: String,

    // ---- transcript chrome ----
    pub(crate) open_groups: BTreeMap<usize, bool>,
    pub(crate) open_rows: BTreeMap<String, bool>,
    pub(crate) finding_accord_open: BTreeMap<String, bool>,
    /// Drop restored expand maps if a refetch rewrites the list; cleared on toggle.
    pub(crate) expansion_guard: Option<(usize, String)>,
    pub(crate) near_bottom: bool,
    pub(crate) last_scroll_top: f32,
    pub(crate) last_row_count: usize,
    pub(crate) last_fingerprint: String,
    pub(crate) pending_scroll: Option<PendingScroll>,
    pub(crate) restore_attempts: u32,
    pub(crate) ignore_swap_scroll: u32,
    pub(crate) scroll: ScrollHandle,
    /// Offset seen at the last frame (`onChatScroll` fires when it changes).
    pub(crate) seen_scroll_top: f32,
    pub(crate) seen_max_scroll: f32,
    pub(crate) viewport_h: f32,
    pub(crate) fade_ready: bool,
    /// Measured block heights per chat key (`blockHByChat`).
    pub(crate) block_h_by_chat: HashMap<String, BTreeMap<String, f32>>,
    /// Block tops and heights in the column, from the last frame.
    pub(crate) block_ranges: Rc<std::cell::RefCell<BTreeMap<String, (f32, f32)>>>,
    pub(crate) chat_hydrated: HashSet<String>,
    pub(crate) chat_in_view: Vec<String>,
    pub(crate) reveal_key: String,
    pub(crate) copied_row: Option<String>,
    pub(crate) copied_bubble: Option<String>,
    pub(crate) copy_timer: Option<Task<()>>,
    pub(crate) fork_menu: Option<ForkMenu>,
    pub(crate) lightbox: Option<Entity<recur_content::AttachmentLightbox>>,
    pub(crate) img_ctx: Option<Entity<recur_content::image_menu::ImageContextMenu>>,
    pub(crate) file_drop_over: bool,

    // ---- the turn digest ----
    pub(crate) digest_comment_for: String,
    pub(crate) digest_comment_busy: String,
    pub(crate) digest_comment_input: Option<Entity<gpui_kit::base::input::TextareaState>>,
    pub(crate) digest_notes_by_id: HashMap<String, Vec<TurnDigestNote>>,
    /// The findings generation the overlay was written against.
    pub(crate) digest_notes_at: Option<u64>,
    pub(crate) findings_gen: u64,
    pub(crate) digest_status_by_id: HashMap<String, String>,
    pub(crate) digest_status_busy: String,

    // ---- the turn clock ----
    pub(crate) turn_elapsed_sec: i64,
    pub(crate) clock_origin: i64,
    pub(crate) clock_key: String,
    pub(crate) clock_task: Option<Task<()>>,

    // ---- rendered content ----
    pub(crate) markdown: HashMap<String, Entity<MarkdownView>>,
    pub(crate) streams: HashMap<String, Entity<StreamingMarkdown>>,
    pub(crate) cards: crate::cards::CardViews,
    pub(crate) markdown_seen: HashSet<String>,

    pub(crate) _subs: Vec<Subscription>,
    pub(crate) _events: Option<Task<()>>,
}

impl EventEmitter<ChatEvent> for AgentChat {}

impl Focusable for AgentChat {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AgentChat {
    pub fn new(
        vcx: &ViewContext,
        props: ChatProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = vcx.state.clone();
        let initial = if props.reset_key.is_empty() {
            None
        } else {
            TranscriptUiCache::global(cx).claim(&props.reset_key)
        };
        let mut subs = vec![cx.observe(&state, |this: &mut Self, _, cx| {
            this.sync_mentions_from_stores(cx);
            cx.notify();
        })];
        subs.push(
            cx.subscribe(&state, |_: &mut Self, _, ev: &ShellEvent, cx| {
                if let ShellEvent::WorkspaceGone(_) = ev {
                    cx.notify();
                }
            }),
        );
        subs.push(cx.observe_window_activation(window, |_, _, cx| cx.notify()));
        // Unmount parks the chrome under the key, as the resetKey effect's cleanup does.
        subs.push(cx.on_release(|this: &mut Self, cx| {
            let key = this.props.reset_key.clone();
            if !key.is_empty() {
                let snap = this.capture_transcript_ui(&key, cx);
                TranscriptUiCache::global(cx).save(&key, snap);
            }
        }));
        subs.push(Self::intercept_keys(cx));
        let mut this = AgentChat {
            state,
            services: vcx.services.clone(),
            props: ChatProps::default(),
            hooks: ChatHooks::default(),
            toolbar: None,
            empty: None,
            session_card: None,
            focus: cx.focus_handle().tab_stop(true),
            window_handle: window.window_handle(),
            tc_shift: 0.,
            scale: window.scale_factor(),
            doc: ComposerDoc::default(),
            input: String::new(),
            attachments: Vec::new(),
            undo_epoch: cx.background_executor().now(),
            undo: ComposerUndo::new(crate::composer_undo::UNDO_PAUSE),
            composer_layout: Rc::default(),
            composer_scroll: ScrollHandle::new(),
            composer_marked: None,
            prompt_slots: Vec::new(),
            prompt_slot_subs: Vec::new(),
            prompt_slot_focus: None,
            caret_blink_epoch: std::time::Instant::now(),
            anim_epoch: std::time::Instant::now(),
            think_scrolls: HashMap::new(),
            tool_data: Rc::default(),
            image_loading: HashMap::new(),
            fork_anchor: Rc::default(),
            composer_geom: Rc::default(),
            meter: cx.new(|_| crate::meter::ContextMeter::new()),
            selecting: false,
            skill_index: Vec::new(),
            skill_index_key: String::new(),
            skill_index_loading: false,
            skill_task: None,
            prompt_index: Rc::default(),
            prompt_index_loading: false,
            prompt_task: None,
            mention: None,
            skill_mention: None,
            prompt_gate_open: false,
            prompt_args: None,
            args_menu_escaped: false,
            complete_hits: Vec::new(),
            complete_message: String::new(),
            complete_gen: 0,
            complete_task: None,
            mention_sel: 0,
            mention_hits: Vec::new(),
            mention_search_gen: 0,
            search_task: None,
            file_index_loading: false,
            file_index_tick: 0,
            hover_gate: HoverGate::default(),
            mention_escaped: None,
            listed_terminals_for_mention: false,
            listed_artifacts_for_mention: false,
            menu_scroll: ScrollHandle::new(),
            last_menu_key: String::new(),
            open_groups: initial
                .as_ref()
                .map(|u| u.open_groups.clone())
                .unwrap_or_default(),
            open_rows: initial
                .as_ref()
                .map(|u| u.open_rows.clone())
                .unwrap_or_default(),
            finding_accord_open: initial
                .as_ref()
                .map(|u| u.finding_accord_open.clone())
                .unwrap_or_default(),
            expansion_guard: initial
                .as_ref()
                .filter(|u| u.row_count > 0 || !u.fingerprint.is_empty())
                .map(|u| (u.row_count, u.fingerprint.clone())),
            near_bottom: initial.as_ref().is_none_or(|u| u.near_bottom),
            last_scroll_top: initial.as_ref().map_or(0., |u| u.scroll_top),
            last_row_count: initial.as_ref().map_or(0, |u| u.row_count),
            last_fingerprint: initial
                .as_ref()
                .map(|u| u.fingerprint.clone())
                .unwrap_or_default(),
            pending_scroll: initial
                .as_ref()
                .filter(|u| !u.near_bottom)
                .map(|u| PendingScroll {
                    scroll_top: u.scroll_top,
                    near_bottom: u.near_bottom,
                    row_count: u.row_count,
                }),
            restore_attempts: 0,
            ignore_swap_scroll: 0,
            scroll: ScrollHandle::new(),
            seen_scroll_top: 0.,
            seen_max_scroll: 0.,
            viewport_h: 0.,
            fade_ready: false,
            block_h_by_chat: HashMap::new(),
            block_ranges: Rc::default(),
            chat_hydrated: HashSet::new(),
            chat_in_view: Vec::new(),
            reveal_key: String::new(),
            copied_row: None,
            copied_bubble: None,
            copy_timer: None,
            fork_menu: None,
            lightbox: None,
            img_ctx: None,
            file_drop_over: false,
            digest_comment_for: String::new(),
            digest_comment_busy: String::new(),
            digest_comment_input: None,
            digest_notes_by_id: HashMap::new(),
            digest_notes_at: None,
            findings_gen: 0,
            digest_status_by_id: HashMap::new(),
            digest_status_busy: String::new(),
            turn_elapsed_sec: 0,
            clock_origin: 0,
            clock_key: String::new(),
            clock_task: None,
            markdown: HashMap::new(),
            streams: HashMap::new(),
            cards: Default::default(),
            markdown_seen: HashSet::new(),
            _subs: subs,
            _events: None,
        };
        this.listen(cx);
        this.props.reset_key = props.reset_key.clone();
        this.set_props(props, cx);
        if initial.is_some() {
            this.restore_transcript_ui(initial, cx);
        }
        this
    }

    // ---- props ----

    pub fn props(&self) -> &ChatProps {
        &self.props
    }

    pub fn set_hooks(&mut self, hooks: ChatHooks, cx: &mut Context<Self>) {
        self.hooks = hooks;
        cx.notify();
    }

    /// `toolbar`: rendered inside the input box, bottom-left (the model selector).
    pub fn set_toolbar(&mut self, toolbar: Option<AnyView>, cx: &mut Context<Self>) {
        self.toolbar = toolbar;
        cx.notify();
    }

    /// `empty`: the empty-transcript slot (replaces `empty_text`).
    pub fn set_empty(&mut self, empty: Option<AnyView>, cx: &mut Context<Self>) {
        self.empty = empty;
        cx.notify();
    }

    pub fn set_session_card(&mut self, r: Option<SessionCardRenderer>, cx: &mut Context<Self>) {
        self.session_card = r;
        cx.notify();
    }

    /// Replaces the props. A new `reset_key` saves the leaving chat's chrome and restores the
    /// entering one's, as one AgentChat serves every tab.
    pub fn set_props(&mut self, props: ChatProps, cx: &mut Context<Self>) {
        let key_changed = props.reset_key != self.props.reset_key;
        let scope_changed = props.project_id != self.props.project_id
            || props.workspace_id != self.props.workspace_id;
        let findings_changed = props.findings != self.props.findings;
        let was_windowed = self.window_chat();
        if key_changed {
            let old = self.props.reset_key.clone();
            if !old.is_empty() {
                let snap = self.capture_transcript_ui(&old, cx);
                TranscriptUiCache::global(cx).save(&old, snap);
            }
        }
        self.props = props;
        if findings_changed {
            self.findings_gen += 1;
            if self.digest_notes_at.is_some_and(|g| g != self.findings_gen)
                && !self.digest_notes_by_id.is_empty()
            {
                self.digest_notes_by_id.clear();
                self.digest_notes_at = None;
            }
        }
        if scope_changed {
            let key = format!("{}\t{}", self.props.project_id, self.props.workspace_id);
            if key != self.skill_index_key {
                self.skill_index_key = key;
                self.skill_index.clear();
                self.skill_mention = None;
            }
            self.prefetch_artifacts(cx);
        }
        if key_changed {
            // A new draft owner never inherits the previous tab's undo history.
            self.undo.reset(
                UndoSnap {
                    text: self.input.clone(),
                    atts: self.attachments.clone(),
                    caret: self.input.len(),
                },
                true,
            );
            let key = self.props.reset_key.clone();
            if !key.is_empty() {
                let saved = TranscriptUiCache::global(cx).claim(&key);
                self.restore_transcript_ui(saved, cx);
            }
            self.chat_hydrated.clear();
            self.chat_in_view.clear();
        }
        if was_windowed != self.window_chat() && !self.window_chat() {
            self.chat_hydrated.clear();
        }
        self.check_expansion_guard();
        self.sync_turn_clock(cx);
        cx.notify();
    }

    /// The draft wire text (`bind:input`).
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The image drafts (`bind:attachments`).
    pub fn attachments(&self) -> &[ChatAttachmentDraft] {
        &self.attachments
    }

    /// Sets the draft from the host (tab swaps, restored drafts): rehydrates the chips.
    pub fn set_draft(
        &mut self,
        input: String,
        attachments: Vec<ChatAttachmentDraft>,
        cx: &mut Context<Self>,
    ) {
        if input == self.input && attachments == self.attachments {
            return;
        }
        self.input = input;
        self.attachments = attachments;
        if self.input.starts_with('/') {
            self.ensure_prompt_index(cx);
        }
        self.hydrate_doc(cx);
        self.undo.reset(
            UndoSnap {
                text: self.input.clone(),
                atts: self.attachments.clone(),
                caret: self.input.len(),
            },
            false,
        );
        self.composer_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    pub(crate) fn app_surface(&self) -> bool {
        self.props.surface == ChatSurfaceKind::App
    }

    // ---- derived stores ----

    pub(crate) fn live_terminals(&self, cx: &App) -> Vec<WorkspaceTerminalTab> {
        self.state
            .read(cx)
            .workspace_terminals(&self.props.workspace_id)
            .to_vec()
    }

    pub(crate) fn live_artifacts(&self, cx: &App) -> Vec<WorkspaceArtifact> {
        self.state
            .read(cx)
            .workspace_artifacts(&self.props.workspace_id)
            .to_vec()
    }

    pub(crate) fn terminal_title_for(&self, id: &str, cx: &App) -> String {
        let tabs = self.live_terminals(cx);
        if id.is_empty() {
            return tabs
                .iter()
                .find(|t| t.active && !t.exited)
                .or(tabs.first())
                .map(|t| t.title.clone())
                .unwrap_or_default();
        }
        tabs.iter()
            .find(|t| t.id == id)
            .map(|t| t.title.clone())
            .unwrap_or_default()
    }

    pub(crate) fn artifact_title_for(&self, id: &str, cx: &App) -> String {
        let arts = self.live_artifacts(cx);
        if id.is_empty() {
            return arts.first().map(|a| a.title.clone()).unwrap_or_default();
        }
        arts.iter()
            .find(|a| a.id == id)
            .map(|a| a.title.clone())
            .unwrap_or_default()
    }

    /// `chatPermissions`: this tab's chat-surface prompts, only while the workspace detail
    /// is on screen.
    pub(crate) fn chat_permissions(&self, cx: &App) -> Vec<PermissionRequest> {
        let p = &self.props;
        let s = self.state.read(cx);
        if p.readonly
            || p.workspace_id.is_empty()
            || p.agent_code.is_empty()
            || s.view != ViewName::Detail
            || !p.shell_active
        {
            return Vec::new();
        }
        s.permission_requests
            .iter()
            .filter(|r| {
                r.surface == "chat"
                    && r.workspace_id == p.workspace_id
                    && r.agent_code == p.agent_code
            })
            .cloned()
            .collect()
    }

    pub(crate) fn chat_questions(&self, cx: &App) -> Vec<UserQuestion> {
        let p = &self.props;
        let s = self.state.read(cx);
        if p.readonly
            || p.workspace_id.is_empty()
            || p.agent_code.is_empty()
            || s.view != ViewName::Detail
        {
            return Vec::new();
        }
        s.user_questions
            .iter()
            .filter(|q| q.workspace_id == p.workspace_id && q.agent_code == p.agent_code)
            .cloned()
            .collect()
    }

    pub(crate) fn chat_escalation(&self, cx: &App) -> Option<Escalation> {
        let p = &self.props;
        if p.readonly || self.state.read(cx).view != ViewName::Detail {
            return None;
        }
        p.escalation
            .clone()
            .filter(|e| e.status.as_str() == "open" && !e.question.is_empty())
    }

    /// Refreshes [`ToolData`] from the props and stores.
    pub(crate) fn snapshot_tool_data(&mut self, cx: &App) {
        let s = self.state.read(cx);
        self.tool_data = Rc::new(ToolData {
            root_path: self.props.root_path.clone(),
            has_open_file: self.hooks.open_file,
            findings: self.props.findings.clone(),
            prs: s.prs.clone(),
            workspace_id: self.props.workspace_id.clone(),
            briefs: s.briefing.workspaces.clone().unwrap_or_default(),
        });
    }

    /// Queued follow-ups wait in a strip above the composer, not in the transcript.
    pub(crate) fn queued_messages(&self) -> Vec<&AgentMsg> {
        self.props
            .messages
            .iter()
            .filter(|m| m.queued && m.me)
            .collect()
    }

    pub(crate) fn transcript(&self) -> Vec<AgentMsg> {
        self.props
            .messages
            .iter()
            .filter(|m| !m.queued)
            .cloned()
            .collect()
    }

    pub(crate) fn blocks(&self, msgs: &[AgentMsg]) -> (Vec<Block>, Vec<String>) {
        let blocks = group_blocks(msgs);
        let keys = block_keys(&self.props.reset_key, &blocks, msgs);
        (blocks, keys)
    }

    pub(crate) fn window_chat(&self) -> bool {
        group_blocks(&self.transcript()).len() > CHAT_HYDRATE_ALL_BELOW
    }

    pub(crate) fn last_retry_error(&self, msgs: &[AgentMsg]) -> Option<usize> {
        last_retry_error_index(
            msgs,
            self.props.readonly,
            self.props.sending,
            self.hooks.retry,
        )
    }

    // ---- send / stop / retry ----

    /// `canSend`: text, an image, or held follow-ups to flush.
    pub(crate) fn can_send(&self) -> bool {
        !js_trim(&strip_attachment_markers(&self.input)).is_empty()
            || !self.attachments.is_empty()
            || self.props.can_flush_queue
    }

    /// `composerEmpty`: no visible text and no image chips.
    pub(crate) fn composer_empty(&self) -> bool {
        js_trim(&strip_attachment_markers(&self.input)).is_empty() && self.attachments.is_empty()
    }

    pub(crate) fn enabled_prompt_index(&self, cx: &App) -> Vec<PromptActionInfo> {
        enabled_prompts(self.state.read(cx).settings.as_ref(), &self.prompt_index)
    }

    pub(crate) fn prompt_chip_specs(&self, cx: &App) -> Vec<PromptChipSpec> {
        self.enabled_prompt_index(cx)
            .iter()
            .map(prompt_action)
            .filter(|p| recur_engine::view::prompts::prompt_takes_args(Some(p)))
            .map(|p| PromptChipSpec {
                slash: p.slash.clone(),
                args: recur_engine::view::prompts::prompt_arg_specs(Some(&p)),
            })
            .collect()
    }

    /// `send(alt)`.
    pub fn send(&mut self, alt: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_send() {
            return;
        }
        if self.attachments.is_empty()
            && self.hooks.prompt_action
            && let Some((slash, args)) = parse_committed_prompt_line(&self.input)
        {
            let index = self.enabled_prompt_index(cx);
            if let Some(info) = index.iter().find(|p| p.slash == slash) {
                if self.props.prompt_busy {
                    return;
                }
                let action = prompt_action(info);
                let spec = PromptChipSpec {
                    slash: info.slash.clone(),
                    args: recur_engine::view::prompts::prompt_arg_specs(Some(&action)),
                };
                let values = recur_engine::view::composer::parse_prompt_chip_values(&args, &spec);
                if recur_engine::view::prompts::prompt_takes_args(Some(&action))
                    && !recur_engine::view::prompts::prompt_required_args_filled(
                        Some(&action),
                        &values,
                    )
                {
                    self.enter_prompt_args(&slash, &args, window, cx);
                    return;
                }
                self.mention = None;
                self.skill_mention = None;
                self.prompt_args = None;
                self.near_bottom = true;
                self.pending_scroll = None;
                self.persist_transcript_ui(cx);
                let args = self.prompt_chip_args(&slash, &args);
                cx.emit(ChatEvent::PromptAction { slash, args, alt });
                return;
            }
        }
        self.mention = None;
        self.skill_mention = None;
        self.near_bottom = true;
        self.pending_scroll = None;
        self.persist_transcript_ui(cx);
        cx.emit(ChatEvent::Send { alt });
    }

    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.props.sending && !self.props.can_stop {
            return;
        }
        if self.hooks.stop {
            cx.emit(ChatEvent::Stop);
        }
    }

    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        if self.props.sending || self.props.readonly || !self.hooks.retry {
            return;
        }
        self.near_bottom = true;
        self.pending_scroll = None;
        self.persist_transcript_ui(cx);
        cx.emit(ChatEvent::Retry);
    }

    // ---- transcript chrome ----

    pub(crate) fn block_heights(&mut self) -> &mut BTreeMap<String, f32> {
        let key = self.props.reset_key.clone();
        self.block_h_by_chat.entry(key).or_default()
    }

    pub(crate) fn persist_transcript_ui(&mut self, cx: &mut Context<Self>) {
        if self.props.reset_key.is_empty() {
            return;
        }
        let msgs = self.transcript();
        self.last_row_count = msgs.len();
        self.last_fingerprint = transcript_fingerprint(msgs.iter().map(Into::into), None);
        let heights = self.block_heights().clone();
        let snap = snapshot_transcript_ui(
            &LiveChrome {
                open_groups: &self.open_groups,
                open_rows: &self.open_rows,
                finding_accord_open: &self.finding_accord_open,
                scroll_top: self.last_scroll_top,
                near_bottom: self.near_bottom,
                scroller_hidden: false,
                row_count: self.last_row_count,
                fingerprint: &self.last_fingerprint,
                block_heights: &heights,
            },
            None,
        );
        TranscriptUiCache::global(cx).save(&self.props.reset_key, snap);
    }

    pub(crate) fn persist_scroll(&mut self, cx: &mut Context<Self>) {
        if self.props.reset_key.is_empty() {
            return;
        }
        let (k, top, near) = (
            self.props.reset_key.clone(),
            self.last_scroll_top,
            self.near_bottom,
        );
        TranscriptUiCache::global(cx).save_scroll(&k, top, near);
    }

    fn capture_transcript_ui(&self, key: &str, cx: &mut App) -> TranscriptUiState {
        let heights = self.block_h_by_chat.get(key).cloned().unwrap_or_default();
        let prev = TranscriptUiCache::global(cx).load(key);
        snapshot_transcript_ui(
            &LiveChrome {
                open_groups: &self.open_groups,
                open_rows: &self.open_rows,
                finding_accord_open: &self.finding_accord_open,
                scroll_top: self.last_scroll_top,
                near_bottom: self.near_bottom,
                scroller_hidden: true,
                row_count: self.last_row_count,
                fingerprint: &self.last_fingerprint,
                block_heights: &heights,
            },
            prev.as_ref(),
        )
    }

    fn restore_transcript_ui(&mut self, saved: Option<TranscriptUiState>, cx: &mut Context<Self>) {
        let next = saved.clone().unwrap_or_default();
        self.open_groups = next.open_groups.clone();
        self.open_rows = next.open_rows.clone();
        self.finding_accord_open = next.finding_accord_open.clone();
        self.near_bottom = next.near_bottom;
        self.last_scroll_top = next.scroll_top;
        self.last_row_count = next.row_count;
        self.last_fingerprint = next.fingerprint.clone();
        self.expansion_guard = (next.row_count > 0 || !next.fingerprint.is_empty())
            .then(|| (next.row_count, next.fingerprint.clone()));
        let key = self.props.reset_key.clone();
        let m = self.block_h_by_chat.entry(key).or_default();
        if m.is_empty() {
            restore_block_heights(m, &next.block_heights);
        }
        // First visit (no cache): follow the default pin.
        self.pending_scroll = saved.map(|s| PendingScroll {
            scroll_top: s.scroll_top,
            near_bottom: s.near_bottom,
            row_count: s.row_count,
        });
        self.restore_attempts = 0;
        // Swallow the clamp `scroll` a swap fires until the restore frames run.
        self.ignore_swap_scroll = 2;
        cx.notify();
    }

    fn check_expansion_guard(&mut self) {
        let Some((count, fp)) = self.expansion_guard.clone() else {
            return;
        };
        let rows = self.transcript();
        let cur = transcript_fingerprint(rows.iter().map(Into::into), Some(count));
        if expansion_still_valid(count, &fp, &cur, rows.len()) {
            return;
        }
        self.open_groups.clear();
        self.open_rows.clear();
        self.expansion_guard = None;
    }

    pub(crate) fn toggle_group(&mut self, start: usize, open: bool, cx: &mut Context<Self>) {
        self.expansion_guard = None;
        self.open_groups.insert(start, !open);
        self.persist_transcript_ui(cx);
        cx.notify();
    }

    pub(crate) fn toggle_row(&mut self, key: &str, visual_open: bool, cx: &mut Context<Self>) {
        self.expansion_guard = None;
        self.open_rows.insert(key.to_string(), !visual_open);
        self.persist_transcript_ui(cx);
        cx.notify();
    }

    pub(crate) fn row_is_open(&self, key: &str, fallback: bool) -> bool {
        self.open_rows.get(key).copied().unwrap_or(fallback)
    }

    pub(crate) fn toggle_finding_accord(&mut self, key: &str, cx: &mut Context<Self>) {
        if key.is_empty() {
            return;
        }
        let open = self.finding_accord_open.get(key).copied().unwrap_or(false);
        self.finding_accord_open.insert(key.to_string(), !open);
        self.persist_transcript_ui(cx);
        cx.notify();
    }

    // ---- scrolling ----

    /// Pins the transcript to the bottom (a prompt-action submit bypasses `send`).
    pub fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.near_bottom = true;
        self.pending_scroll = None;
        self.persist_transcript_ui(cx);
        self.pin_bottom();
        cx.notify();
    }

    /// Re-applies the offset after this workbench is shown again.
    pub fn restore_parked_scroll(&mut self, cx: &mut Context<Self>) {
        if self.near_bottom {
            self.pin_bottom();
        } else if self.last_scroll_top > 0. {
            self.scroll
                .set_offset(point(px(0.), px(-self.last_scroll_top)));
        }
        cx.notify();
    }

    pub(crate) fn pin_bottom(&mut self) {
        if !self.near_bottom {
            return;
        }
        let max = f32::from(self.scroll.max_offset().y);
        self.scroll.set_offset(point(px(0.), px(-max)));
        self.last_scroll_top = max;
    }

    /// Brings an engine-fact block (a fan-out wave card) to the top of the scrollport,
    /// pursuing it for a bounded number of frames while restores and stub swaps settle.
    pub fn reveal_fact(&mut self, fact_id: &str, cx: &mut Context<Self>) -> Task<bool> {
        let key = crate::transcript_ui::transcript_block_key(
            &self.props.reset_key,
            &format!("fact:{fact_id}"),
        );
        self.reveal_key = key.clone();
        cx.spawn(async move |this, cx| {
            let frame = Duration::from_millis(16);
            let mut waited = Duration::ZERO;
            loop {
                let found = this
                    .update(cx, |c, _| c.block_ranges.borrow().contains_key(&key))
                    .unwrap_or(false);
                if found || waited > Duration::from_millis(4000) {
                    break;
                }
                cx.background_executor().timer(frame).await;
                waited += frame;
            }
            let mut held = 0;
            let mut ok = false;
            for _ in 0..90 {
                if held >= 4 {
                    break;
                }
                let step = this.update(cx, |c, cx| {
                    if c.reveal_key != key {
                        return Some(true);
                    }
                    let range = c.block_ranges.borrow().get(&key).copied()?;
                    if c.window_chat() && !c.chat_hydrated.contains(&key) {
                        c.chat_hydrated.insert(key.clone());
                    }
                    c.near_bottom = false;
                    c.pending_scroll = None;
                    let max = f32::from(c.scroll.max_offset().y);
                    let target = (range.0 + 26. - 6.).round().clamp(0., max.max(0.));
                    let top = -f32::from(c.scroll.offset().y);
                    let stub = c.window_chat() && !c.chat_hydrated.contains(&key);
                    if (top - target).abs() > 1. {
                        c.scroll.set_offset(point(px(0.), px(-target)));
                        c.last_scroll_top = target;
                        held = 0;
                    } else if !stub {
                        held += 1;
                    }
                    cx.notify();
                    None
                });
                match step {
                    Ok(Some(done)) => return done,
                    Ok(None) => ok = true,
                    Err(_) => return false,
                }
                cx.background_executor().timer(frame).await;
            }
            let _ = this.update(cx, |c, _| {
                if c.reveal_key == key {
                    c.reveal_key.clear();
                }
            });
            ok
        })
    }

    /// Called each frame with the scroller's geometry: `onChatScroll`, the pending restore,
    /// the bottom pin and the stub window.
    pub(crate) fn on_scroll_frame(&mut self, viewport_h: f32, cx: &mut Context<Self>) {
        let top = -f32::from(self.scroll.offset().y);
        let max = f32::from(self.scroll.max_offset().y).max(0.);
        let content_h = max + viewport_h;
        self.viewport_h = viewport_h;
        let moved = (top - self.seen_scroll_top).abs() > 0.01;
        let grew = (max - self.seen_max_scroll).abs() > 0.01;
        self.seen_scroll_top = top;
        self.seen_max_scroll = max;
        if moved && viewport_h > 0. {
            if self.ignore_swap_scroll > 0 {
                self.ignore_swap_scroll -= 1;
            } else {
                self.near_bottom = next_near_bottom(
                    self.near_bottom,
                    self.last_scroll_top,
                    content_h,
                    top,
                    viewport_h,
                );
                self.last_scroll_top = top;
                self.pending_scroll = None;
                self.persist_scroll(cx);
                self.fork_menu = None;
            }
        }
        if grew || self.pending_scroll.is_some() {
            self.apply_pending_scroll(viewport_h, content_h, cx);
        }
        if self.near_bottom && (top - max).abs() > 0.5 && self.pending_scroll.is_none() {
            self.pin_bottom();
            self.seen_scroll_top = max;
            cx.notify();
        }
        if self.ignore_swap_scroll > 0 && !moved {
            self.ignore_swap_scroll = self.ignore_swap_scroll.saturating_sub(1);
        }
        // `fadeScrollEdges`: the leading fade stays until the scroller measured off the top
        // or a pinned restore can still move it.
        let from_top = top > 1.;
        if from_top || !(self.near_bottom && max > crate::transcript_ui::NEAR_BOTTOM_PX) {
            self.fade_ready = true;
        }
        self.refresh_chat_window(top, viewport_h, cx);
    }

    fn apply_pending_scroll(&mut self, client_h: f32, scroll_h: f32, cx: &mut Context<Self>) {
        let Some(saved) = self.pending_scroll.clone() else {
            return;
        };
        let rows = self.transcript().len();
        let gate = scroll_restore_ready(
            saved.scroll_top,
            saved.near_bottom,
            saved.row_count,
            client_h,
            scroll_h,
            rows,
        );
        if gate == ScrollRestoreGate::Wait {
            return;
        }
        let max_scroll = (scroll_h - client_h).max(0.);
        let honored = saved.near_bottom || saved.scroll_top <= 0. || max_scroll >= saved.scroll_top;
        if !honored
            && gate != ScrollRestoreGate::Abandon
            && self.restore_attempts < RESTORE_LAYOUT_FRAMES
        {
            self.restore_attempts += 1;
            self.near_bottom = false;
            cx.notify();
            return;
        }
        if gate == ScrollRestoreGate::Apply {
            let target = if saved.near_bottom {
                max_scroll
            } else {
                saved.scroll_top.min(max_scroll)
            };
            self.scroll.set_offset(point(px(0.), px(-target)));
            self.seen_scroll_top = target;
        }
        self.pending_scroll = None;
        self.restore_attempts = 0;
        self.last_scroll_top = -f32::from(self.scroll.offset().y);
        self.near_bottom = scroller_near_bottom(scroll_h, self.last_scroll_top, client_h);
        self.persist_scroll(cx);
        cx.notify();
    }

    /// `refreshChatWindow`: which blocks render live while the transcript is windowed.
    fn refresh_chat_window(&mut self, top: f32, view_h: f32, cx: &mut Context<Self>) {
        if !self.window_chat() {
            return;
        }
        let msgs = self.transcript();
        let (_, keys) = self.blocks(&msgs);
        let ranges: Vec<BlockRange> = {
            let r = self.block_ranges.borrow();
            let mut v: Vec<BlockRange> = keys
                .iter()
                .filter_map(|k| {
                    r.get(k).map(|&(t, h)| BlockRange {
                        key: k.clone(),
                        top: t,
                        bottom: t + h,
                    })
                })
                .collect();
            v.sort_by(|a, b| a.top.total_cmp(&b.top));
            v
        };
        let in_view = in_view_keys(&ranges, top, view_h, CHAT_VIEW_PAD_PX);
        let selected = if self.near_bottom {
            keys.last().cloned().unwrap_or_default()
        } else {
            let at = block_at_scroll_top(&ranges, top);
            if at.is_empty() {
                keys.last().cloned().unwrap_or_default()
            } else {
                at
            }
        };
        let mut next = pick_hydrated(
            &keys,
            &selected,
            &in_view,
            CHAT_HYDRATE_MAX,
            CHAT_HYDRATE_OVERSCAN,
        );
        if !self.reveal_key.is_empty() && self.block_ranges.borrow().contains_key(&self.reveal_key)
        {
            next.insert(self.reveal_key.clone());
        }
        let next: HashSet<String> = next.into_iter().collect();
        if self.chat_in_view != in_view {
            self.chat_in_view = in_view;
        }
        if next != self.chat_hydrated {
            self.chat_hydrated = next;
            cx.notify();
        }
    }

    // ---- the turn clock ----

    /// `showTurnElapsed`: a running turn whose footer has not landed and no card waits.
    pub(crate) fn show_turn_elapsed(&self, cx: &App) -> bool {
        self.props.sending
            && !current_turn_has_ended(&self.props.messages, self.props.turn_started_at)
            && self.chat_permissions(cx).is_empty()
            && self.chat_questions(cx).is_empty()
            && self.chat_escalation(cx).is_none()
    }

    pub(crate) fn sync_turn_clock(&mut self, cx: &mut Context<Self>) {
        if !self.show_turn_elapsed(cx) {
            self.turn_elapsed_sec = 0;
            self.clock_origin = 0;
            self.clock_key.clear();
            self.clock_task = None;
            return;
        }
        let key = self.props.reset_key.clone();
        if self.props.turn_started_at > 0 {
            self.clock_origin = self.props.turn_started_at;
            self.clock_key = key;
        } else if self.clock_origin == 0 || self.clock_key != key {
            // One instance serves every tab: never reuse another chat's origin.
            self.clock_origin = now_ms();
            self.clock_key = key;
        }
        self.tick_clock();
        if self.clock_task.is_none() {
            self.clock_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(CLOCK_TICK).await;
                    let alive = this.update(cx, |c, cx| {
                        let before = c.turn_elapsed_sec;
                        c.tick_clock();
                        if c.turn_elapsed_sec != before {
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            }));
        }
    }

    fn tick_clock(&mut self) {
        self.turn_elapsed_sec = ((now_ms() - self.clock_origin) / 1000).max(0);
    }

    pub(crate) fn first_checkpoint(&self, msgs: &[AgentMsg], blocks: &[Block], cx: &App) -> bool {
        let live: Vec<&AgentMsg> = match blocks.last() {
            Some(Block::Steps { rows, .. }) => rows.iter().map(|&i| &msgs[i]).collect(),
            _ => Vec::new(),
        };
        is_first_checkpoint(self.show_turn_elapsed(cx), self.turn_elapsed_sec, &live)
    }

    // ---- live events ----

    fn listen(&mut self, cx: &mut Context<Self>) {
        let mut events = self.services.subscribe();
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let gone = this.update(cx, |c, cx| c.on_app_event(&ev, cx)).is_err();
                if gone {
                    break;
                }
            }
        }));
    }

    fn on_app_event(&mut self, ev: &recur_services::AppEvent, cx: &mut Context<Self>) {
        use recur_services::AppEvent;
        match ev {
            AppEvent::ArtifactCreated(a) => {
                let ws = a.workspace_id.clone();
                let raw = serde_json::to_value(&a.artifact).unwrap_or_default();
                let Some(art) = recur_engine::view::artifacts::artifact_from_event(&raw) else {
                    return;
                };
                if art.id.is_empty() || ws.is_empty() {
                    return;
                }
                self.state.update(cx, |s, cx| {
                    s.upsert_workspace_artifact(
                        &ws,
                        WorkspaceArtifact {
                            title: if art.title.is_empty() {
                                art.id.clone()
                            } else {
                                art.title.clone()
                            },
                            id: art.id,
                            kind: art.kind,
                            path: art.path,
                        },
                        cx,
                    )
                });
            }
            AppEvent::WorkspaceFileIndex(f) => {
                let key = if self.props.workspace_id.is_empty() {
                    &self.props.project_id
                } else {
                    &self.props.workspace_id
                };
                if &f.workspace_id == key && self.mention.is_some() {
                    self.file_index_tick += 1;
                    self.run_file_search(cx);
                }
            }
            AppEvent::FilesDropped(d) => self.on_files_dropped(d, cx),
            _ => {}
        }
    }

    // ---- copy ----

    pub(crate) fn mark_copied(
        &mut self,
        row: Option<String>,
        bubble: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.copied_row = row.clone();
        self.copied_bubble = bubble.clone();
        cx.notify();
        self.copy_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |c, cx| {
                if row.is_some() && c.copied_row == row {
                    c.copied_row = None;
                }
                if bubble.is_some() && c.copied_bubble == bubble {
                    c.copied_bubble = None;
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn copy_text(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    // ---- the turn digest ----

    pub(crate) fn turn_digest_visible(
        &self,
        msgs: &[AgentMsg],
        end_idx: usize,
    ) -> Vec<TurnDigestFinding> {
        let refs: Vec<_> = self.props.findings.iter().map(digest_ref).collect();
        still_open_findings_for_turn(msgs, end_idx, &refs)
            .into_iter()
            .filter(|f| {
                if f.id.is_empty() {
                    return true;
                }
                self.digest_status_by_id
                    .get(&f.id)
                    .is_none_or(|st| st == "open")
            })
            .collect()
    }

    pub(crate) fn notes_for_digest(&self, f: &TurnDigestFinding) -> Vec<TurnDigestNote> {
        if !f.id.is_empty()
            && self.digest_notes_at == Some(self.findings_gen)
            && let Some(n) = self.digest_notes_by_id.get(&f.id)
        {
            return n.clone();
        }
        f.notes.clone()
    }

    fn apply_digest_notes(&mut self, id: &str, notes: Vec<TurnDigestNote>) {
        self.digest_notes_by_id.insert(
            id.to_string(),
            notes
                .into_iter()
                .filter(|n| !js_trim(&n.body).is_empty())
                .collect(),
        );
        self.digest_notes_at = Some(self.findings_gen);
    }

    pub(crate) fn set_digest_finding_status(
        &mut self,
        id: &str,
        status: &str,
        cx: &mut Context<Self>,
    ) {
        if self.props.workspace_id.is_empty()
            || id.is_empty()
            || !self.digest_status_busy.is_empty()
        {
            return;
        }
        self.digest_status_busy = id.to_string();
        let prev = self.digest_status_by_id.get(id).cloned();
        self.digest_status_by_id
            .insert(id.to_string(), status.to_string());
        if self.digest_comment_for == id {
            self.digest_comment_for.clear();
            self.digest_comment_input = None;
        }
        cx.notify();
        let services = self.services.clone();
        let (ws, id, status) = (
            self.props.workspace_id.clone(),
            id.to_string(),
            status.to_string(),
        );
        cx.spawn(async move |this, cx| {
            let res = services
                .observatory
                .set_finding_status(&ws, &id, &status)
                .await;
            let _ = this.update(cx, |c, cx| {
                if let Err(err) = res {
                    match prev {
                        Some(p) => c.digest_status_by_id.insert(id.clone(), p),
                        None => c.digest_status_by_id.remove(&id),
                    };
                    let msg = recur_view_core::bridge_error_message(&err);
                    recur_view_core::toast(
                        recur_ui::widgets::toasts::ToastKind::Error,
                        if msg.is_empty() {
                            "Could not update finding".into()
                        } else {
                            msg
                        },
                        cx,
                    );
                }
                c.digest_status_busy.clear();
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn jump_to_finding(&mut self, id: &str, focus_note: bool, cx: &mut Context<Self>) {
        let f = (!id.is_empty())
            .then(|| {
                self.props
                    .findings
                    .iter()
                    .find(|x| crate::findings_ui::finding_matches_ref(x, id))
            })
            .flatten()
            .cloned();
        let (ws, project) = (
            self.props.workspace_id.clone(),
            self.props.project_id.clone(),
        );
        let id = id.to_string();
        self.state.update(cx, |s, cx| {
            s.observatory_jump = Some(recur_view_core::ObsJump {
                workspace_id: ws,
                project_id: project,
                panel: "findings".into(),
                finding_id: id.clone(),
                focus_note: !id.is_empty() && focus_note,
                path: f.as_ref().map(|f| f.file.clone()).unwrap_or_default(),
                line: f.as_ref().map_or(0, |f| f.line.max(0) as u32),
                ..Default::default()
            });
            cx.notify();
        });
    }

    pub(crate) fn open_digest_compose(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if id.is_empty() {
            return;
        }
        self.digest_comment_for = id.to_string();
        self.finding_accord_open.insert(id.to_string(), true);
        let input = cx.new(|cx| {
            gpui_kit::base::input::TextareaState::new(window, cx)
                .rows(2)
                .placeholder("Add a note, clarification, or context…")
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self.digest_comment_input = Some(input);
        cx.notify();
    }

    pub(crate) fn cancel_digest_compose(&mut self, cx: &mut Context<Self>) {
        self.digest_comment_for.clear();
        self.digest_comment_input = None;
        cx.notify();
    }

    pub(crate) fn submit_digest_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        let body = self
            .digest_comment_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        let body = js_trim(&body).to_string();
        if self.props.workspace_id.is_empty()
            || id.is_empty()
            || body.is_empty()
            || !self.digest_comment_busy.is_empty()
        {
            return;
        }
        self.digest_comment_busy = id.to_string();
        cx.notify();
        let services = self.services.clone();
        let (ws, id) = (self.props.workspace_id.clone(), id.to_string());
        cx.spawn(async move |this, cx| {
            let res = services.observatory.add_finding_note(&ws, &id, &body).await;
            let _ = this.update(cx, |c, cx| {
                match res {
                    Ok(updated) => {
                        c.digest_comment_for.clear();
                        c.digest_comment_input = None;
                        c.apply_digest_notes(&id, crate::findings_ui::finding_notes(&updated));
                    }
                    Err(err) => {
                        let msg = recur_view_core::bridge_error_message(&err);
                        recur_view_core::toast(
                            recur_ui::widgets::toasts::ToastKind::Error,
                            if msg.is_empty() {
                                "Could not add note".into()
                            } else {
                                msg
                            },
                            cx,
                        );
                    }
                }
                c.digest_comment_busy.clear();
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn delete_digest_note(
        &mut self,
        finding_id: &str,
        note_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.props.workspace_id.is_empty()
            || finding_id.is_empty()
            || note_id.is_empty()
            || !self.digest_comment_busy.is_empty()
        {
            return;
        }
        let current: Vec<TurnDigestNote> = self
            .digest_notes_by_id
            .get(finding_id)
            .cloned()
            .or_else(|| {
                self.props
                    .findings
                    .iter()
                    .find(|f| crate::findings_ui::finding_matches_ref(f, finding_id))
                    .map(crate::findings_ui::finding_notes)
            })
            .unwrap_or_default();
        self.digest_comment_busy = finding_id.to_string();
        self.apply_digest_notes(
            finding_id,
            current
                .iter()
                .filter(|n| n.id != note_id)
                .cloned()
                .collect(),
        );
        cx.notify();
        let services = self.services.clone();
        let (ws, fid, nid) = (
            self.props.workspace_id.clone(),
            finding_id.to_string(),
            note_id.to_string(),
        );
        cx.spawn(async move |this, cx| {
            let res = services
                .observatory
                .delete_finding_note(&ws, &fid, &nid)
                .await;
            let _ = this.update(cx, |c, cx| {
                match res {
                    Ok(updated) => {
                        c.apply_digest_notes(&fid, crate::findings_ui::finding_notes(&updated))
                    }
                    Err(err) => {
                        c.apply_digest_notes(&fid, current);
                        let msg = recur_view_core::bridge_error_message(&err);
                        recur_view_core::toast(
                            recur_ui::widgets::toasts::ToastKind::Error,
                            if msg.is_empty() {
                                "Could not delete note".into()
                            } else {
                                msg
                            },
                            cx,
                        );
                    }
                }
                c.digest_comment_busy.clear();
                cx.notify();
            });
        })
        .detach();
    }

    // ---- fork menu ----

    /// `toggleForkMenu`: without a fork-to-workspace handler the button forks at once.
    pub(crate) fn toggle_fork_menu(
        &mut self,
        message_id: &str,
        anchor: gpui_kit::Bounds<gpui_kit::Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.hooks.fork_to_workspace {
            cx.emit(ChatEvent::Fork(message_id.to_string()));
            return;
        }
        if self
            .fork_menu
            .as_ref()
            .is_some_and(|m| m.message_id == message_id)
        {
            self.fork_menu = None;
            cx.notify();
            return;
        }
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let (pad, mw, mh) = (8., 220., 84.);
        let mut left = f32::from(anchor.left());
        if left + mw > vw - pad {
            left = pad.max(vw - mw - pad);
        }
        let mut top = f32::from(anchor.bottom()) + 6.;
        if top + mh > vh - pad {
            top = pad.max(f32::from(anchor.top()) - mh - 6.);
        }
        self.fork_menu = Some(ForkMenu {
            message_id: message_id.to_string(),
            left,
            top,
        });
        cx.notify();
    }

    // ---- lightbox ----

    pub(crate) fn open_lightbox(
        &mut self,
        src: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if src.is_empty() {
            return;
        }
        let decoded = cx
            .try_global::<recur_content::images::ImageCache>()
            .and_then(|c| c.0.get(src))
            .cloned();
        let this = cx.entity().downgrade();
        let lb = cx.new(|cx| {
            recur_content::AttachmentLightbox::new(
                src.to_string(),
                name.to_string(),
                decoded,
                Rc::new(move |_, cx: &mut App| {
                    let _ = this.update(cx, |c, cx| {
                        c.lightbox = None;
                        c.state.update(cx, |s, cx| {
                            s.set_chat_occluder("attachment-lightbox", false, cx)
                        });
                        cx.notify();
                    });
                }),
                window,
                cx,
            )
        });
        let sub = cx.subscribe(&lb, |_, _, ev: &recur_content::LightboxEvent, cx| {
            let recur_content::LightboxEvent::Toast { error, message } = ev;
            recur_view_core::toast(
                if *error {
                    recur_ui::widgets::toasts::ToastKind::Error
                } else {
                    recur_ui::widgets::toasts::ToastKind::Success
                },
                message.clone(),
                cx,
            );
        });
        self._subs.push(sub);
        self.state.update(cx, |s, cx| {
            s.set_chat_occluder("attachment-lightbox", true, cx)
        });
        self.lightbox = Some(lb);
        cx.notify();
    }

    /// `openImageCtx`: Copy Image at the pointer.
    pub(crate) fn open_image_menu(
        &mut self,
        src: &str,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let png = cx
            .try_global::<recur_content::images::ImageCache>()
            .and_then(|c| c.0.get(src))
            .map(|d| d.png.clone());
        let menu = cx.new(|cx| {
            recur_content::image_menu::ImageContextMenu::new(
                (f32::from(at.x), f32::from(at.y)),
                png,
                window,
                cx,
            )
        });
        let sub = cx.subscribe(
            &menu,
            |this: &mut Self, _, ev: &recur_content::image_menu::ImageMenuEvent, cx| {
                use recur_content::image_menu::ImageMenuEvent;
                match ev {
                    ImageMenuEvent::Close => this.img_ctx = None,
                    ImageMenuEvent::Toast { error, message } => recur_view_core::toast(
                        if *error {
                            recur_ui::widgets::toasts::ToastKind::Error
                        } else {
                            recur_ui::widgets::toasts::ToastKind::Success
                        },
                        message.clone(),
                        cx,
                    ),
                }
                cx.notify();
            },
        );
        self._subs.push(sub);
        self.img_ctx = Some(menu);
        cx.notify();
    }

    // ---- stores the menus follow ----

    pub(crate) fn sync_mentions_from_stores(&mut self, cx: &mut Context<Self>) {
        if self.mention.is_some() {
            self.splice_live_mention_rows(cx);
        }
    }

    /// What sits behind the transcript: `--bg` (a `.sess-body` is `--bg` too).
    pub(crate) fn backdrop(&self, t: &recur_ui::Tokens) -> recur_ui::Color {
        t.bg
    }

    /// Terminal titles, artifact titles, and artifact `(id, title)` pairs.
    pub(crate) fn hydrate_opts_titles(
        &self,
        cx: &App,
    ) -> (TitleMap, TitleMap, Vec<(String, String)>) {
        let terms = self
            .live_terminals(cx)
            .into_iter()
            .map(|t| (t.id, t.title))
            .collect();
        let arts = self.live_artifacts(cx);
        let titles = arts
            .iter()
            .map(|a| (a.id.clone(), a.title.clone()))
            .collect();
        let pairs = arts.into_iter().map(|a| (a.id, a.title)).collect();
        (terms, titles, pairs)
    }

    /// `hydrateComposer` with this chat's chip options.
    pub(crate) fn hydrate_wire(&self, wire: &str, cx: &App) -> ComposerDoc {
        let atts: Vec<ComposerAtt> = self
            .attachments
            .iter()
            .map(|a| ComposerAtt {
                id: a.id.clone(),
                data_url: a.data_url.clone(),
                name: a.name.clone(),
            })
            .collect();
        let (terms, arts, pairs) = self.hydrate_opts_titles(cx);
        let specs = self.prompt_chip_specs(cx);
        let term = |id: &str| -> String {
            if id.is_empty() {
                return self.terminal_title_for("", cx);
            }
            terms.get(id).cloned().unwrap_or_default()
        };
        let art = |id: &str| -> String {
            if id.is_empty() {
                return pairs.first().map(|p| p.1.clone()).unwrap_or_default();
            }
            arts.get(id).cloned().unwrap_or_default()
        };
        let by_title = |title: &str| -> String {
            unique_artifact_id_for_title(title, pairs.iter().map(|(i, t)| (i.as_str(), t.as_str())))
        };
        let opts = HydrateOpts {
            chip_files: true,
            chip_skills: true,
            terminal_title: Some(&term),
            artifact_title: Some(&art),
            artifact_id_for_title: Some(&by_title),
            prompt_chips: &specs,
        };
        ComposerDoc::hydrate(wire, &atts, &opts)
    }

    pub(crate) fn hydrate_doc(&mut self, cx: &mut Context<Self>) {
        let mut doc = self.hydrate_wire(&self.input.clone(), cx);
        doc.caret = doc.text.len();
        doc.anchor = doc.caret;
        self.doc = doc;
        self.rebuild_prompt_slots(None, cx);
    }

    pub(crate) fn kind_before_caret(&self) -> Option<ChipKind> {
        self.doc.kind_before_caret()
    }

    /// `syncInputFromComposer`: the wire text from the doc; dropped image chips drop their
    /// drafts.
    pub(crate) fn sync_input_from_doc(&mut self, record_undo: bool, cx: &mut Context<Self>) {
        let atts: Vec<ComposerAtt> = self
            .attachments
            .iter()
            .map(|a| ComposerAtt {
                id: a.id.clone(),
                data_url: a.data_url.clone(),
                name: a.name.clone(),
            })
            .collect();
        self.doc.chipify_attachments(&atts);
        self.doc.ensure_chip_boundaries();
        let next = self.doc.wire();
        let alive: HashSet<String> = extract_attachment_ids(&next).into_iter().collect();
        if self.attachments.iter().any(|a| !alive.contains(&a.id)) {
            self.attachments.retain(|a| alive.contains(&a.id));
        }
        self.input = next.clone();
        if record_undo {
            self.undo.record(
                UndoSnap {
                    text: next,
                    atts: self.attachments.clone(),
                    caret: self.doc.wire_caret(),
                },
                cx.background_executor()
                    .now()
                    .duration_since(self.undo_epoch)
                    .as_millis() as u64,
            );
        }
        cx.emit(ChatEvent::Draft);
        cx.notify();
    }

    pub(crate) fn caret_wire(&self) -> usize {
        self.doc.wire_caret()
    }
}

/// JS `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    recur_engine::view::js::trim(s)
}

pub(crate) fn prompt_gate(text: &str, caret: usize) -> bool {
    prompt_action_query_at(text, caret).is_some()
}

pub(crate) fn args_query(
    text: &str,
    caret: usize,
    takes_args: &dyn Fn(&str) -> bool,
) -> Option<PromptArgsQuery> {
    prompt_args_query_at(text, caret, takes_args)
}
