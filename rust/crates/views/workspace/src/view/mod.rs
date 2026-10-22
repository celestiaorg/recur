//! `WorkspaceView.svelte`: one workspace's workbench. The Observatory fills it; the agent chat
//! sits in its left dock with a tab strip of chat sessions (the Loop conductor `MA` and pair
//! tabs `PA`, `PA2`, …), each with its own mode, model, draft, queue and transcript. The plan
//! panel and its head ride the Observatory's plan pane.
//!
//! Per-tab state is keyed `workspaceId\tcode` ([`chat_tab_key`]) as in the Svelte, so hoisted
//! sends and drafts can move between workspaces.

mod live;
mod pickers;
mod prompts;
mod send;
mod sync;
mod tabs;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use futures::future::Shared;
use gpui_kit::{
    App, AppContext as _, BorrowAppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, ScrollHandle, Styled, Subscription, Task, Window, div,
};
use recur_agent::context_usage::ContextUsage;
use recur_domain::{Escalation, Finding, Workspace, WorkspaceChatTab};
use recur_engine::view::chat::{AgentMsg, ChatAttachmentDraft};
use recur_engine::view::loops::{LoopInfo, PolicyInfo, fallback_loops, fallback_policies};
use recur_services::Services;
use recur_view_chat::chat::{AgentChat, ChatHooks, ChatProps};
use recur_view_chat::heap::{ChatHeapCache, ChatPickerSnap, PendingChatSend};
use recur_view_chat::hoist::{HoistState, chat_tab_key};
use recur_view_chat::session_feed::SessionFeed;
use recur_view_chat::strip::ChatStripCache;
use recur_view_core::modes::{
    ChatMode, ws_chat_held, ws_create_failed, ws_creating, ws_has_run, ws_run_active,
};
use recur_view_core::models::CatalogModel;
use recur_view_core::{AppState, ViewContext};
use recur_view_observatory::{Observatory, ObservatoryProps, ObservatorySlots};

use crate::head::ChatTabsHead;
use crate::overlay::MainOverlay;
use crate::plan_head::PlanHead;
use crate::plan_panel::PlanPanel;
use crate::toolbar::DockToolbar;


/// The tab the Svelte falls back to before ListWorkspaceChats lands.
pub(crate) const DEFAULT_CODE: &str = "PA";
/// `DELETE_ARM_MS`: a closed chat's delete stays armed this long.
pub(crate) const DELETE_ARM_MS: u64 = 3000;

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A prepared prompt-action send whose routing failed (`HeldPromptSend`): Retry from its
/// toast re-routes and sends it.
#[derive(Clone, Debug)]
pub(crate) struct HeldPromptSend {
    pub hold_id: String,
    pub workspace_id: String,
    pub action_id: String,
    pub prompt: String,
    pub display: String,
    pub mode: Option<ChatMode>,
    pub tab_group: String,
    pub loop_id: String,
    pub fresh_session: bool,
}

/// A prompt action's abort (`AbortController`): the flag the routine checks after each step
/// and the wake for the run it races.
#[derive(Clone, Default)]
pub(crate) struct Abort {
    flag: Rc<std::cell::Cell<bool>>,
    wake: Rc<std::cell::RefCell<Option<futures::channel::oneshot::Sender<()>>>>,
}

impl Abort {
    pub fn aborted(&self) -> bool {
        self.flag.get()
    }

    pub fn abort(&self) {
        self.flag.set(true);
        if let Some(tx) = self.wake.borrow_mut().take() {
            let _ = tx.send(());
        }
    }

    /// Resolves when aborted.
    pub fn signal(&self) -> futures::channel::oneshot::Receiver<()> {
        let (tx, rx) = futures::channel::oneshot::channel();
        if self.aborted() {
            let _ = tx.send(());
        } else {
            *self.wake.borrow_mut() = Some(tx);
        }
        rx
    }
}

/// What a WorkspaceView instance is bound to (`$props()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceProps {
    pub workspace_id: String,
    /// The live workbench. A parked hot shell stays mounted but must not steal chat focus,
    /// engine focus or composer queries.
    pub shell_active: bool,
}

pub struct WorkspaceView {
    pub(crate) state: Entity<AppState>,
    pub(crate) services: Services,
    pub(crate) ws_id: String,
    pub(crate) shell_active: bool,

    // ---- children ----
    pub(crate) observatory: Entity<Observatory>,
    pub(crate) chat: Entity<AgentChat>,
    pub(crate) head: Entity<ChatTabsHead>,
    pub(crate) toolbar: Entity<DockToolbar>,
    pub(crate) plan_head: Entity<PlanHead>,
    pub(crate) plan_panel: Entity<PlanPanel>,
    pub(crate) feed: Entity<SessionFeed>,

    // ---- the tab strip ----
    /// `tabsByWs[boundWsId]`; `None` until the first list (or the strip cache) lands.
    pub(crate) tabs: Option<Vec<WorkspaceChatTab>>,
    pub(crate) closed_tabs: Vec<WorkspaceChatTab>,
    pub(crate) active: Option<String>,
    pub(crate) recents_open: bool,
    pub(crate) armed_delete: Option<String>,
    pub(crate) delete_arm: Option<Task<()>>,
    pub(crate) strip_scroll: ScrollHandle,
    pub(crate) strip_restore: Option<(f32, u64)>,
    pub(crate) forking: bool,
    /// Between a drop and ReorderWorkspaceChats' reply: title refreshes keep the dropped order.
    pub(crate) reorder_committing: Option<Vec<String>>,

    // ---- escalation, focus, watermarks ----
    pub(crate) live_esc: Option<Escalation>,
    pub(crate) esc_load_gen: u64,
    pub(crate) pending_focus_refetching: String,
    pub(crate) viewed_for: String,
    pub(crate) focus_composer_pending: bool,

    // ---- dock pickers ----
    pub(crate) catalog_loops: Vec<LoopInfo>,
    pub(crate) catalog_policies: Vec<PolicyInfo>,
    pub(crate) catalog_models: Vec<CatalogModel>,
    pub(crate) catalog_rev: Option<u64>,
    pub(crate) pickers_by_key: BTreeMap<String, ChatPickerSnap>,
    pub(crate) pickers_loading: HashMap<String, Shared<Task<bool>>>,
    pub(crate) pickers_fetched: HashSet<String>,
    pub(crate) picker_gen: HashMap<String, u64>,
    pub(crate) transcript_gen: HashMap<String, u64>,
    pub(crate) simplify_by_key: BTreeMap<String, bool>,
    pub(crate) simplify_fetched: HashSet<String>,
    pub(crate) simplify_gen: HashMap<String, u64>,

    // ---- the Observatory's left gutter and plan ----
    pub(crate) chat_open: bool,
    /// The setup strip shows the whole multi-line error.
    pub(crate) setup_err_open: bool,
    pub(crate) had_plan_for: String,
    pub(crate) child_reveal_for: String,

    // ---- per-tab chat state ----
    pub(crate) chat_by_key: BTreeMap<String, Vec<AgentMsg>>,
    pub(crate) usage_by_key: BTreeMap<String, ContextUsage>,
    pub(crate) input_by_key: BTreeMap<String, String>,
    pub(crate) attachments_by_key: BTreeMap<String, Vec<ChatAttachmentDraft>>,
    pub(crate) sending_by_key: BTreeMap<String, bool>,
    pub(crate) live_model_by_key: BTreeMap<String, String>,
    pub(crate) send_gen_by_key: BTreeMap<String, u64>,
    pub(crate) steer_clock: i64,
    pub(crate) hoist_send_gen_by_key: BTreeMap<String, u64>,
    pub(crate) rejected_steers_by_key: BTreeMap<String, Vec<PendingChatSend>>,
    pub(crate) pending_by_key: BTreeMap<String, Vec<PendingChatSend>>,
    pub(crate) hold_pending_by_key: BTreeMap<String, bool>,
    pub(crate) turn_started_at_by_key: BTreeMap<String, i64>,
    pub(crate) stale_idle_armed_by_key: BTreeMap<String, bool>,
    pub(crate) turn_got_text_by_key: BTreeMap<String, bool>,
    pub(crate) stream_seq_by_key: BTreeMap<String, i64>,
    /// The chat key whose draft the composer shows (`inputBoundKey`).
    pub(crate) input_bound_key: String,
    pub(crate) loaded_for: String,
    pub(crate) chat_findings: Vec<Finding>,
    pub(crate) findings_gen: u64,

    // ---- prompt actions ----
    pub(crate) prompt_running: bool,
    pub(crate) prompt_abort: Option<Abort>,
    pub(crate) prompt_target_key: String,
    pub(crate) held_prompts: Vec<HeldPromptSend>,

    /// The view's queue still flushes (`queueOwner`; off once released).
    pub(crate) queue_owner: bool,
    pub(crate) was_shell_active: bool,
    pub(crate) sync_marks: sync::Marks,
    pub(crate) props_dirty: bool,
    pub(crate) hoist_listener: u64,
    _subs: Vec<Subscription>,
}

impl WorkspaceView {
    pub fn new(
        vcx: &ViewContext,
        props: WorkspaceProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = vcx.state.clone();
        let services = vcx.services.clone();
        let ws_id = props.workspace_id.clone();

        // Warm remount: the strip and heap paint at once.
        let strip = ChatStripCache::global(cx).get(&ws_id);
        let bust = HoistState::global(cx).take_dest_bust(&ws_id);
        cx.update_global::<HoistState, _>(|h, cx| {
            h.bust_dest_heap(ChatHeapCache::global(cx), &ws_id, &bust)
        });
        let heap = ChatHeapCache::global(cx).get(&ws_id).unwrap_or_default();

        let tabs = strip.as_ref().filter(|s| !s.tabs.is_empty()).map(|s| s.tabs.clone());
        let closed_tabs = strip
            .as_ref()
            .filter(|s| !s.closed.is_empty())
            .map(|s| s.closed.clone())
            .unwrap_or_default();
        let active = strip.as_ref().filter(|s| !s.tabs.is_empty()).map(|s| {
            if s.active_code.is_empty() {
                tabs::restored_code(&state, &ws_id, &s.tabs, cx)
            } else {
                s.active_code.clone()
            }
        });
        let strip_restore = strip
            .as_ref()
            .and_then(|s| s.tab_scroll_left)
            .map(|left| (left, 1));

        let pickers_by_key: BTreeMap<String, ChatPickerSnap> = heap
            .pickers
            .iter()
            .filter(|(_, p)| !p.model.is_empty())
            .map(|(k, p)| (k.clone(), p.clone()))
            .collect();
        let simplify_by_key = heap.simplify.clone();
        let simplify_fetched: HashSet<String> = simplify_by_key.keys().cloned().collect();

        let chat_open = state
            .read(cx)
            .workspace(&ws_id)
            .map(|w| recur_view_observatory::obs_cache::observatory_cache_key(&w.project_id, &w.id))
            .and_then(|key| recur_view_observatory::obs_cache::cache(cx).get(&key))
            .and_then(|snap| snap.left_open)
            .unwrap_or(true);

        let catalog_models = state.read(cx).peek_model_catalog().to_vec();
        let feed = cx.new(|_| SessionFeed::new(services.clone()));
        let chat = cx.new(|cx| AgentChat::new(vcx, ChatProps::default(), window, cx));
        chat.update(cx, |c, cx| {
            c.set_hooks(
                ChatHooks {
                    send: true,
                    stop: true,
                    retry: true,
                    remove_queued: true,
                    cycle_mode: true,
                    open_file: true,
                    open_artifact: true,
                    answer_escalation: true,
                    fork: true,
                    fork_to_workspace: true,
                    turn_diff: true,
                    reset: true,
                    prompt_action: true,
                },
                cx,
            )
        });
        let me = cx.entity().downgrade();
        let head = cx.new(|_| ChatTabsHead::new(me.clone()));
        let toolbar = cx.new(|cx| DockToolbar::new(me.clone(), window, cx));
        let plan_panel = cx.new(|cx| PlanPanel::new(vcx, &ws_id, feed.clone(), window, cx));
        let plan_head = cx.new(|_| PlanHead::new(me.clone(), plan_panel.clone()));
        let overlay = cx.new(|_| MainOverlay::new(state.clone(), ws_id.clone()));
        chat.update(cx, |c, cx| c.set_toolbar(Some(toolbar.clone().into()), cx));

        let (project_id, main_blocked) = {
            let s = state.read(cx);
            let w = s.workspace(&ws_id);
            (
                w.map(|w| w.project_id.clone()).unwrap_or_default(),
                recur_view_core::modes::ws_main_blocked(w),
            )
        };
        let obs_props = ObservatoryProps {
            project_id,
            workspace_id: ws_id.clone(),
            loop_scoped: true,
            left_title: "Agent".into(),
            left_open: Some(chat_open),
            agent_tab_count: tabs.as_ref().map_or(0, |t| t.len()),
            agent_tabs: true,
            can_fix_findings: true,
            can_add_editor_mention: true,
            main_blocked,
            shell_active: props.shell_active,
            ..Default::default()
        };
        let slots = ObservatorySlots {
            left_panel: Some(chat.clone().into()),
            left_head: Some(head.clone().into()),
            plan_panel: Some(plan_panel.clone().into()),
            plan_head: Some(plan_head.clone().into()),
            main_overlay: Some(overlay.clone().into()),
        };
        let st = state.clone();
        let observatory = cx.new(|cx| Observatory::new(st, obs_props, slots, window, cx));

        let subs = vec![
            cx.observe(&state, |v, _, cx| v.touch(cx)),
            cx.subscribe_in(&chat, window, Self::on_chat_event),
            cx.subscribe_in(&observatory, window, Self::on_observatory_event),
            cx.subscribe(&state, Self::on_applied_event),
            cx.subscribe(&state, Self::on_shell_event),
            cx.observe(&feed, |v, _, cx| v.touch(cx)),
            cx.subscribe_in(&plan_panel, window, Self::on_plan_event),
            cx.on_release(|v: &mut Self, cx| v.release(cx)),
        ];

        HoistState::global(cx).note_live_source(&ws_id);

        let mut v = WorkspaceView {
            state,
            services,
            ws_id,
            shell_active: props.shell_active,
            observatory,
            chat,
            head,
            toolbar,
            plan_head,
            plan_panel,
            feed,
            tabs,
            closed_tabs,
            active,
            recents_open: false,
            armed_delete: None,
            delete_arm: None,
            strip_scroll: ScrollHandle::new(),
            strip_restore,
            forking: false,
            reorder_committing: None,
            live_esc: None,
            esc_load_gen: 0,
            pending_focus_refetching: String::new(),
            viewed_for: String::new(),
            focus_composer_pending: false,
            catalog_loops: fallback_loops(),
            catalog_policies: fallback_policies(),
            catalog_models,
            catalog_rev: None,
            pickers_by_key,
            pickers_loading: HashMap::new(),
            pickers_fetched: HashSet::new(),
            picker_gen: HashMap::new(),
            transcript_gen: HashMap::new(),
            simplify_by_key,
            simplify_fetched,
            simplify_gen: HashMap::new(),
            chat_open,
            setup_err_open: false,
            had_plan_for: String::new(),
            child_reveal_for: String::new(),
            chat_by_key: heap.chat,
            usage_by_key: heap.usage,
            input_by_key: heap.input,
            attachments_by_key: heap.attachments,
            sending_by_key: BTreeMap::new(),
            live_model_by_key: BTreeMap::new(),
            send_gen_by_key: BTreeMap::new(),
            steer_clock: 0,
            hoist_send_gen_by_key: BTreeMap::new(),
            rejected_steers_by_key: BTreeMap::new(),
            pending_by_key: heap.pending,
            hold_pending_by_key: heap.hold_pending,
            turn_started_at_by_key: heap.turn_started_at,
            stale_idle_armed_by_key: BTreeMap::new(),
            turn_got_text_by_key: heap.turn_got_text,
            stream_seq_by_key: heap.stream_seq,
            input_bound_key: String::new(),
            loaded_for: String::new(),
            chat_findings: Vec::new(),
            findings_gen: 0,
            prompt_running: false,
            prompt_abort: None,
            prompt_target_key: String::new(),
            held_prompts: Vec::new(),
            queue_owner: true,
            was_shell_active: true,
            sync_marks: sync::Marks::default(),
            props_dirty: true,
            hoist_listener: 0,
            _subs: subs,
        };
        v.install_hoist_listener(cx);
        v.absorb_hoist_drafts(cx);
        v.mount_effects(cx);
        v
    }

    /// `onHoistSendComplete`: a hoisted send finished on its source view.
    fn install_hoist_listener(&mut self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let async_cx = cx.to_async();
        self.hoist_listener = HoistState::global(cx).on_send_complete(move |key, gen_| {
            let (key, weak, acx) = (key.to_string(), weak.clone(), async_cx.clone());
            // Completion fires inside another view's update: apply it on the next turn.
            acx.spawn(async move |acx| {
                let _ = weak.update(acx, |v, cx| v.on_hoist_send_complete(&key, gen_, cx));
            })
            .detach();
        });
    }

    /// `onDestroy`: release the hoist source, park the heap for a warm remount.
    fn release(&mut self, cx: &mut App) {
        HoistState::global(cx).drop_live_source(&self.ws_id);
        HoistState::global(cx).off_send_complete(self.hoist_listener);
        self.feed.update(cx, |f, _| f.detach());
        self.queue_owner = false;
        let live = self
            .state
            .read(cx)
            .workspaces
            .iter()
            .any(|w| w.id == self.ws_id && !w.archived);
        if !live {
            return;
        }
        self.persist_draft_slot(cx);
        let snap = recur_view_chat::heap::ChatHeapSnapshot {
            chat: self.chat_by_key.clone(),
            usage: self.usage_by_key.clone(),
            input: self.input_by_key.clone(),
            attachments: self.attachments_by_key.clone(),
            turn_started_at: self.turn_started_at_by_key.clone(),
            stream_seq: self.stream_seq_by_key.clone(),
            turn_got_text: self.turn_got_text_by_key.clone(),
            pickers: self.pickers_by_key.clone(),
            pending: self.pending_by_key.clone(),
            hold_pending: self.hold_pending_by_key.clone(),
            simplify: self.simplify_by_key.clone(),
        };
        ChatHeapCache::global(cx).put(&self.ws_id, &snap);
    }

    // ---- props ----

    pub fn workspace_id(&self) -> &str {
        &self.ws_id
    }

    pub fn observatory(&self) -> &Entity<Observatory> {
        &self.observatory
    }

    pub fn chat(&self) -> &Entity<AgentChat> {
        &self.chat
    }

    pub fn set_shell_active(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.shell_active == on {
            return;
        }
        self.shell_active = on;
        self.touch(cx);
    }

    /// State changed: rebuild the chat's props and run the effects on the next frame.
    pub(crate) fn touch(&mut self, cx: &mut Context<Self>) {
        self.props_dirty = true;
        cx.notify();
        self.head.update(cx, |_, cx| cx.notify());
        self.toolbar.update(cx, |_, cx| cx.notify());
        self.plan_head.update(cx, |_, cx| cx.notify());
    }

    // ---- derived ----

    pub(crate) fn ws<'a>(&self, cx: &'a App) -> Option<&'a Workspace> {
        self.state.read(cx).workspace(&self.ws_id)
    }

    pub(crate) fn tabs(&self) -> &[WorkspaceChatTab] {
        self.tabs.as_deref().unwrap_or_default()
    }

    /// `activeCode`: the selected tab, else the first, else `PA`.
    pub fn active_code(&self) -> String {
        self.active
            .clone()
            .filter(|c| !c.is_empty())
            .or_else(|| self.tabs().first().map(|t| t.code.clone()))
            .unwrap_or_else(|| DEFAULT_CODE.into())
    }

    pub(crate) fn key(&self, code: &str) -> String {
        chat_tab_key(&self.ws_id, code)
    }

    pub(crate) fn active_key(&self) -> String {
        self.key(&self.active_code())
    }

    pub(crate) fn active_tab(&self) -> Option<&WorkspaceChatTab> {
        let code = self.active_code();
        self.tabs().iter().find(|t| t.code == code)
    }

    pub(crate) fn has_run(&self, cx: &App) -> bool {
        self.ws(cx).is_some_and(ws_has_run)
    }

    pub(crate) fn is_main(&self, cx: &App) -> bool {
        self.ws(cx).is_some_and(|w| w.is_main)
    }

    pub(crate) fn is_pr_host(&self, cx: &App) -> bool {
        self.ws(cx).is_some_and(|w| !w.host_pr_id.trim().is_empty())
    }

    pub(crate) fn creating(&self, cx: &App) -> bool {
        ws_creating(self.ws(cx))
    }

    pub(crate) fn create_failed(&self, cx: &App) -> bool {
        ws_create_failed(self.ws(cx))
    }

    pub(crate) fn setup_hold(&self, cx: &App) -> bool {
        ws_chat_held(self.ws(cx))
    }

    pub(crate) fn setup_failed(&self, cx: &App) -> bool {
        self.ws(cx)
            .is_some_and(|w| w.last_error.starts_with("setup command failed"))
    }

    /// `conductorChatCode`: the chat tab that owns this workspace's run (legacy empty → MA).
    pub(crate) fn conductor_chat_code(&self, cx: &App) -> String {
        let tabs = self.tabs();
        if let Some(owner) = self
            .ws(cx)
            .map(|w| w.loop_chat_code.trim())
            .filter(|o| !o.is_empty())
            && tabs.iter().any(|t| t.code == owner)
        {
            return owner.to_string();
        }
        if let Some(t) = tabs.iter().find(|t| t.loop_started) {
            return t.code.clone();
        }
        if tabs.iter().any(|t| t.code == "MA") {
            return "MA".into();
        }
        String::new()
    }

    pub(crate) fn conductor_tab_locked(&self, code: &str, cx: &App) -> bool {
        if !self.has_run(cx) || code != self.conductor_chat_code(cx) {
            return false;
        }
        ws_run_active(self.ws(cx))
    }

    pub(crate) fn on_conductor_tab(&self, cx: &App) -> bool {
        let a = self.active_code();
        !a.is_empty() && a == self.conductor_chat_code(cx)
    }

    pub(crate) fn loop_pickers_pinned(&self, cx: &App) -> bool {
        self.active_tab().is_some_and(|t| t.loop_started)
            || (self.has_run(cx) && self.conductor_chat_code(cx) == self.active_code())
    }

    pub(crate) fn dock_loop_id(&self, cx: &App) -> String {
        let t = self
            .active_tab()
            .map(|t| t.loop_id.trim().to_string())
            .unwrap_or_default();
        if !t.is_empty() {
            return t;
        }
        let s = self.state.read(cx);
        recur_engine::view::loops::workspace_loop_id(self.ws(cx), s.settings.as_ref())
    }

    pub(crate) fn dock_policy_id(&self, cx: &App) -> String {
        let t = self
            .active_tab()
            .map(|t| t.policy_id.trim().to_string())
            .unwrap_or_default();
        if !t.is_empty() {
            return t;
        }
        let s = self.state.read(cx);
        recur_engine::view::loops::workspace_policy_id(self.ws(cx), s.settings.as_ref())
    }

    pub(crate) fn chat_key_busy(&self, key: &str, cx: &App) -> bool {
        if self.sending_by_key.get(key).copied().unwrap_or(false) {
            return true;
        }
        let Some((id, code)) = key.split_once('\t') else {
            return false;
        };
        self.state
            .read(cx)
            .agents_running
            .contains(&format!("{id}-{code}"))
    }

    pub(crate) fn agent_busy(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .agents_running
            .contains(&format!("{}-{}", self.ws_id, self.active_code()))
    }

    /// `sending`: the active tab's RPC is in flight or its engine turn runs.
    pub(crate) fn sending(&self, cx: &App) -> bool {
        self.sending_by_key
            .get(&self.active_key())
            .copied()
            .unwrap_or(false)
            || self.agent_busy(cx)
    }

    pub(crate) fn prompt_busy_for(&self, key: &str) -> bool {
        self.prompt_running && !self.prompt_target_key.is_empty() && key == self.prompt_target_key
    }

    pub(crate) fn prompt_busy_here(&self) -> bool {
        self.prompt_busy_for(&self.active_key())
    }

    pub(crate) fn composer_busy(&self, cx: &App) -> bool {
        self.sending(cx) || self.prompt_busy_here()
    }

    pub(crate) fn steer_by_default(&self, cx: &App) -> bool {
        !self
            .state
            .read(cx)
            .settings
            .as_ref()
            .is_some_and(|s| s.queue_follow_ups)
    }

    /// Tab mode for the strip's identity icon: the live dock selection wins, then the
    /// persisted tab mode; MA is always the Loop thread.
    pub(crate) fn tab_mode_for(&self, tab: &WorkspaceChatTab) -> ChatMode {
        if tab.code == "MA" {
            return ChatMode::Loop;
        }
        match self.pickers_by_key.get(&self.key(&tab.code)) {
            Some(p) => p.mode,
            None => ChatMode::from_key(&tab.mode),
        }
    }

    /// `chatMode`: the dock mode of the active tab (Review on a PR host for agent / loop).
    pub(crate) fn chat_mode(&self, cx: &App) -> ChatMode {
        let code = self.active_code();
        let m = match self.pickers_by_key.get(&self.active_key()) {
            Some(p) => p.mode,
            None => match self.tabs().iter().find(|t| t.code == code) {
                Some(t) => ChatMode::from_key(&t.mode),
                None if code == "MA" => ChatMode::Loop,
                None => ChatMode::Agent,
            },
        };
        if self.is_pr_host(cx) && matches!(m, ChatMode::Agent | ChatMode::Loop) {
            return ChatMode::Review;
        }
        m
    }

    pub(crate) fn show_loop_pickers(&self, cx: &App) -> bool {
        self.ws(cx).is_some()
            && !self.is_main(cx)
            && (self.chat_mode(cx) == ChatMode::Loop || self.active_code() == "MA")
    }

    pub(crate) fn catalog_ready(&self) -> bool {
        !self.catalog_models.is_empty()
    }

    pub(crate) fn pickers_ready(&self) -> bool {
        self.catalog_ready()
            && self
                .pickers_by_key
                .get(&self.active_key())
                .is_some_and(|p| !p.model.is_empty())
    }

    pub(crate) fn chat_surface(&self, cx: &App) -> recur_engine::chat::surface::ChatSurface {
        let s = self.state.read(cx);
        let pr = s.prs.iter().find(|p| p.id == s.selected_pr_id);
        recur_engine::view::chat_surface::chat_surface_for_view(s.view.as_str(), pr)
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        let strip = self.render_failed_strip(cx);
        div()
            .id("wsv")
            .debug_selector(|| "wsv".into())
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .children(strip)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .overflow_hidden()
                    .child(self.observatory.clone()),
            )
    }
}
