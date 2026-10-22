//! The component's `$effect`s, run before each frame from what changed since the last one,
//! and the props the children get (the chat, its toolbar, the Observatory).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, Subscription, Window, div,
};
use recur_domain::WorkspaceChatTab;
use recur_engine::view::chat::{AgentMsg, ChatAttachmentDraft, ChatLogSession};
use recur_engine::view::loop_cockpit::{
    is_loop_cockpit_tab, merge_loop_cockpit, specialist_live_codes,
};
use recur_engine::view::modes::ws_setup_status;
use recur_view_chat::cards::session::{SessionCard, SessionCardProps};
use recur_view_chat::chat::{ChatEvent, ChatHooks, ChatProps, SessionCardRenderer};
use recur_view_core::ViewContext;
use recur_view_core::modes::ChatMode;
use recur_view_core::state::ChatRef;
use recur_view_observatory::ObservatoryProps;

use super::{WorkspaceView, now_ms, tabs::save_active_code};

const EMPTY_MA: &str = "Ask where this loop stands, or steer it — the Maestro can read the worktree, edit the plan, re-run review, and dispatch specialists; rails hold at execution.";
const EMPTY_PA: &str = "Ask the agent, or tell it what to change — the mode selector below decides whether it may edit.";

/// What the effects saw last time they ran.
#[derive(Default)]
pub(crate) struct Marks {
    mounted: bool,
    scan_pending: Option<bool>,
    focus_sent: Option<(String, Option<Vec<WorkspaceChatTab>>)>,
    focused_chat: Option<ChatRef>,
    pickers_for: Option<(String, bool, bool)>,
    simplify_for: Option<(String, bool)>,
    feed_for: Option<(String, String)>,
    feed_live: Vec<String>,
    flush_for: Option<(bool, Vec<(String, usize)>)>,
    obs_props: Option<ObservatoryProps>,
    reset_hook: Option<bool>,
}

/// `.obs-left, .obs-side { font-size: 12px }`: the dock's inherited font size.
const OBS_PANEL_FONT: f32 = 12.;

pub(crate) type Cards = Rc<RefCell<HashMap<String, (Entity<SessionCard>, Subscription)>>>;

impl WorkspaceView {
    /// The once-on-mount effects that do not depend on later changes.
    pub(crate) fn mount_effects(&mut self, cx: &mut Context<Self>) {
        self.load_escalation(cx);
        self.load_loops(cx);
        self.load_findings(cx);
        self.install_session_card(cx);
    }

    fn install_session_card(&mut self, cx: &mut Context<Self>) {
        let cards = self.session_cards.clone();
        let weak = cx.entity().downgrade();
        let vcx = ViewContext {
            state: self.state.clone(),
            services: self.services.clone(),
        };
        let renderer: SessionCardRenderer = Rc::new(
            move |log: &ChatLogSession, first: bool, _: &mut Window, cx: &mut App| -> AnyElement {
                let Some(view) = weak.upgrade() else {
                    return div().into_any_element();
                };
                let v = view.read(cx);
                let props = SessionCardProps {
                    code: log.code.clone(),
                    name: log.name.clone(),
                    messages: log.messages.clone(),
                    live: log.live,
                    presentation_active: v.presentation_active(cx),
                    root_path: v
                        .ws(cx)
                        .map(|w| w.worktree_path.clone())
                        .unwrap_or_default(),
                    workspace_id: v.ws_id.clone(),
                    findings: v.chat_findings.clone(),
                    open_file: true,
                    first,
                    font_size: OBS_PANEL_FONT,
                };
                let card = {
                    let mut map = cards.borrow_mut();
                    match map.get(&log.id) {
                        Some((c, _)) => c.clone(),
                        None => {
                            let c = cx.new(|_| SessionCard::new(vcx.clone(), props.clone()));
                            let w = weak.clone();
                            let sub = cx.subscribe(&c, move |_, ev: &ChatEvent, cx| {
                                if let ChatEvent::OpenFile {
                                    path,
                                    line,
                                    end_line,
                                    start_col,
                                    end_col,
                                } = ev
                                {
                                    let _ = w.update(cx, |v, cx| {
                                        v.open_agent_file(
                                            path, *line, *end_line, *start_col, *end_col, cx,
                                        )
                                    });
                                }
                            });
                            map.insert(log.id.clone(), (c.clone(), sub));
                            c
                        }
                    }
                };
                card.update(cx, |c, cx| c.set_props(props, cx));
                card.into_any_element()
            },
        );
        self.chat
            .update(cx, |c, cx| c.set_session_card(Some(renderer), cx));
    }

    /// The transcript is on screen (`leftPanel(presentationActive)`).
    pub(crate) fn presentation_active(&self, cx: &App) -> bool {
        self.observatory.read(cx).maestro_panel_visible(cx)
    }

    pub(crate) fn specialist_live(&self, cx: &App) -> Vec<String> {
        let s = self.state.read(cx);
        let feed = self.feed.read(cx);
        specialist_live_codes(
            &self.ws_id,
            s.agents_running.iter().map(String::as_str),
            feed.live_codes.iter().map(String::as_str),
        )
    }

    pub(crate) fn show_loop_cockpit(&self, cx: &App) -> bool {
        is_loop_cockpit_tab(&self.active_code(), self.loop_pickers_pinned(cx))
    }

    /// `chat`: the conductor's rows merged with the cockpit, or the tab's rows with its
    /// subagent cards.
    pub(crate) fn chat_messages(&self, cx: &App) -> Vec<AgentMsg> {
        let ma = self
            .chat_by_key
            .get(&self.active_key())
            .cloned()
            .unwrap_or_default();
        let feed = self.feed.read(cx);
        if self.show_loop_cockpit(cx) {
            let goal = self
                .ws(cx)
                .map(|w| {
                    if w.goal_display.is_empty() {
                        w.goal.clone()
                    } else {
                        w.goal_display.clone()
                    }
                })
                .unwrap_or_default();
            return merge_loop_cockpit(
                &ma,
                &feed.sessions,
                &feed.facts,
                &self.specialist_live(cx),
                &goal,
            );
        }
        let cards: Vec<_> = feed
            .facts
            .iter()
            .filter(|f| f.card == "fanout")
            .cloned()
            .collect();
        if cards.is_empty() {
            ma
        } else {
            merge_loop_cockpit(&ma, &[], &cards, &[], "")
        }
    }

    /// "Waiting for N subagents…" while this tab's children have not reported.
    fn waiting_subagents(&self, cx: &App) -> String {
        if self.composer_busy(cx) {
            return String::new();
        }
        let code = self.active_code();
        let n = self
            .state
            .read(cx)
            .workspaces
            .iter()
            .filter(|w| {
                w.parent_id == self.ws_id
                    && !w.archived
                    && w.fan_out.as_ref().map_or("PA", |f| {
                        if f.chat.is_empty() {
                            "PA"
                        } else {
                            f.chat.as_str()
                        }
                    }) == code
                    && w.fan_out
                        .as_ref()
                        .is_none_or(|f| f.outcome.is_empty() || f.outcome == "report-pending")
            })
            .count();
        if n == 0 {
            String::new()
        } else {
            format!("Waiting for {n} subagent{}…", if n == 1 { "" } else { "s" })
        }
    }

    fn can_steer(&self, cx: &App) -> bool {
        recur_view_core::models::chat_steer_follow_up(
            self.chat_key_busy(&self.active_key(), cx),
            self.chat_mode(cx).key(),
            !self.chat_attachments(cx).is_empty(),
            &self.active_code(),
        )
    }

    fn placeholder(&self, cx: &App) -> String {
        let steer = self.can_steer(cx);
        if self.composer_busy(cx) {
            return if steer && self.steer_by_default(cx) {
                "Steer the agent… (@ files, / skills, ⏎ steers, ⌘⏎ queues)"
            } else if steer {
                "Add a follow-up… (@ files, / skills, ⏎ queues, ⌘⏎ steers)"
            } else {
                "Add a follow-up… (@ files, / skills, ⏎ to queue)"
            }
            .into();
        }
        if self.create_failed(cx) {
            let has_tree = self.ws(cx).is_some_and(|w| !w.worktree_path.is_empty());
            return if self.setup_failed(cx) && has_tree {
                "Setup failed — retry setup or remove this workspace"
            } else {
                "Couldn’t create — remove this workspace to try again"
            }
            .into();
        }
        if self.setup_hold(cx) {
            return "Message the agent… (@ files, / skills, ⏎ queues until setup finishes)".into();
        }
        if self.active_code() == "MA" {
            return "Ask where things stand, or steer the loop… (@ files, / skills)".into();
        }
        "Message the agent… (@ files, / skills, ⏎ to send)".into()
    }

    fn chat_props(&self, cx: &App) -> ChatProps {
        let code = self.active_code();
        let key = self.active_key();
        let busy = self.composer_busy(cx);
        let can_steer = self.can_steer(cx);
        let setup = ws_setup_status(self.ws(cx));
        let status = if setup.is_empty() {
            self.waiting_subagents(cx)
        } else {
            setup.to_string()
        };
        let ws = self.ws(cx);
        ChatProps {
            messages: self.chat_messages(cx),
            sending: busy,
            steer_follow_up: can_steer && self.steer_by_default(cx),
            can_steer,
            turn_started_at: self.turn_started_at_by_key.get(&key).copied().unwrap_or(0),
            status_text: status,
            can_stop: self.loop_walk_live(cx),
            can_flush_queue: !busy
                && !self.setup_hold(cx)
                && !self.create_failed(cx)
                && self.pending_by_key.get(&key).is_some_and(|q| !q.is_empty()),
            compact: true,
            font_size: OBS_PANEL_FONT,
            shell_active: self.shell_active,
            presentation_active: self.presentation_active(cx),
            reset_key: key,
            empty_text: if code == "MA" { EMPTY_MA } else { EMPTY_PA }.into(),
            placeholder: self.placeholder(cx),
            context_usage: self.usage_by_key.get(&self.active_key()).cloned(),
            root_path: ws.map(|w| w.worktree_path.clone()).unwrap_or_default(),
            project_id: ws.map(|w| w.project_id.clone()).unwrap_or_default(),
            workspace_id: self.ws_id.clone(),
            agent_code: code,
            escalation: if self.on_conductor_tab(cx) {
                self.live_esc.clone()
            } else {
                None
            },
            findings: self.chat_findings.clone(),
            prompt_busy: self.prompt_running,
            ..ChatProps::default()
        }
    }

    fn obs_props(&self, cx: &App) -> ObservatoryProps {
        let ws = self.ws(cx);
        ObservatoryProps {
            project_id: ws.map(|w| w.project_id.clone()).unwrap_or_default(),
            workspace_id: self.ws_id.clone(),
            loop_scoped: true,
            left_title: "Agent".into(),
            left_open: Some(self.chat_open),
            agent_tab_count: self.tabs().len(),
            agent_tabs: true,
            can_fix_findings: true,
            can_add_editor_mention: true,
            main_blocked: recur_view_core::modes::ws_main_blocked(ws),
            shell_active: self.shell_active,
            ..Default::default()
        }
    }

    /// Runs the effects, then pushes props to the children.
    pub(crate) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ws(cx).is_none() {
            return;
        }
        let id = self.ws_id.clone();
        let code = self.active_code();
        let key = self.active_key();
        let (scan_pending, settings_on) = {
            let s = self.state.read(cx);
            (s.scan_pending, s.settings.is_some())
        };

        // Mount, and again once a workspace scan settles: the strip and the closed list.
        if !scan_pending && self.sync_marks.scan_pending != Some(false) {
            self.load_tabs(cx);
            self.refresh_closed_tabs(cx);
        }
        self.sync_marks.scan_pending = Some(scan_pending);
        if !self.sync_marks.mounted {
            self.sync_marks.mounted = true;
        }

        // A parked shell coming back: re-apply the chat's offset and refetch the strip.
        if self.shell_active && !self.was_shell_active {
            self.chat.update(cx, |c, cx| c.restore_parked_scroll(cx));
            self.refetch_tabs(cx);
        }
        self.was_shell_active = self.shell_active;

        self.publish_focused_chat(&id, &code, cx);
        self.take_pending_chat_focus(cx);
        self.mark_viewed(cx);
        self.sync_catalog(cx);

        // Engine focus and the saved tab: on every strip change of a live, settled shell.
        if self.shell_active && !scan_pending && self.tabs.is_some() {
            let mark = (code.clone(), self.tabs.clone());
            if self.sync_marks.focus_sent.as_ref() != Some(&mark) {
                self.sync_marks.focus_sent = Some(mark);
                save_active_code(&self.state, &id, &code, cx);
                let (svc, i, c) = (self.services.clone(), id.clone(), code.clone());
                cx.background_executor()
                    .spawn(async move {
                        let _ = svc.workspaces.set_workspace_chat_focus(&i, &c).await;
                    })
                    .detach();
                if let Some(tabs) = self.tabs.as_mut()
                    && tabs.iter().any(|t| t.code == code && t.unseen)
                {
                    for t in tabs.iter_mut().filter(|t| t.code == code) {
                        t.unseen = false;
                    }
                    self.put_strip(cx);
                }
            }
        }

        // PR detail's dock is this chat: a loop workspace talks on its conductor.
        {
            let s = self.state.read(cx);
            let pr_dock =
                s.view == recur_view_core::ViewName::PrDetail && !s.selected_pr_id.is_empty();
            if pr_dock && self.has_run(cx) {
                let owner = self.conductor_chat_code(cx);
                if !owner.is_empty() && owner != code {
                    self.select_chat_tab(&owner, cx);
                }
            }
        }

        let pickers_mark = (key.clone(), self.catalog_ready(), settings_on);
        if self.sync_marks.pickers_for.as_ref() != Some(&pickers_mark) {
            self.sync_marks.pickers_for = Some(pickers_mark);
            if self.catalog_ready() && settings_on {
                self.load_chat_pickers(&id, &code, cx).detach();
            }
        }
        let simplify_mark = (key.clone(), settings_on);
        if self.sync_marks.simplify_for.as_ref() != Some(&simplify_mark) {
            self.sync_marks.simplify_for = Some(simplify_mark);
            if settings_on && code == "MA" {
                self.load_simplify(&id, &code, cx);
            }
        }

        self.plan_filed_transition(window, cx);
        self.child_transcript(window, cx);

        let feed_mark = (id.clone(), code.clone());
        if self.sync_marks.feed_for.as_ref() != Some(&feed_mark) {
            self.sync_marks.feed_for = Some(feed_mark);
            self.feed.update(cx, |f, cx| f.attach(&id, &code, cx));
        }
        let mut live = self.specialist_live(cx);
        live.sort();
        if live != self.sync_marks.feed_live {
            self.sync_marks.feed_live = live.clone();
            self.feed.update(cx, |f, cx| {
                f.mark_running(live.iter().map(String::as_str), cx)
            });
        }

        // First sight of a busy tab without a local start stamps it once.
        if self.sending(cx) && !self.turn_started_at_by_key.contains_key(&key) {
            self.turn_started_at_by_key.insert(key.clone(), now_ms());
            self.props_dirty = true;
        }

        self.swap_draft(&key, cx);

        // The transcript on every switch: the store-authoritative pass on focus.
        if !scan_pending && key != self.loaded_for {
            self.loaded_for = key.clone();
            self.resync_chat(&id, &code, cx);
        }

        self.apply_seed(cx);
        self.take_prompt_request(window, cx);

        // Queued sends flush once setup releases them.
        let setup_hold = self.setup_hold(cx);
        let prefix = format!("{id}\t");
        let flush_mark = (
            setup_hold,
            self.pending_by_key
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, q)| (k.clone(), q.len()))
                .collect::<Vec<_>>(),
        );
        if self.sync_marks.flush_for.as_ref() != Some(&flush_mark) {
            self.sync_marks.flush_for = Some(flush_mark);
            if !setup_hold {
                let keys: Vec<String> = self
                    .pending_by_key
                    .keys()
                    .filter(|k| k.starts_with(&prefix))
                    .cloned()
                    .collect();
                for k in keys {
                    self.try_flush_pending(&k, cx);
                }
            }
        }

        if self.focus_composer_pending {
            self.focus_composer_pending = false;
            if self.shell_active && self.presentation_active(cx) {
                self.chat.update(cx, |c, cx| c.focus_composer(window, cx));
            }
        }
        self.save_strip_scroll(cx);
        self.push_children(window, cx);
    }

    fn push_children(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reset = self.active_code() != "MA" && !self.loop_pickers_pinned(cx);
        if self.sync_marks.reset_hook != Some(reset) {
            self.sync_marks.reset_hook = Some(reset);
            self.chat.update(cx, |c, cx| {
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
                        reset,
                        prompt_action: true,
                    },
                    cx,
                )
            });
        }
        if self.props_dirty {
            self.props_dirty = false;
            let props = self.chat_props(cx);
            self.chat.update(cx, |c, cx| c.set_props(props, cx));
        }
        self.push_obs_props(window, cx);
        let toolbar = self.toolbar.clone();
        toolbar.update(cx, |t, cx| t.sync(self, window, cx));
    }

    pub(crate) fn push_obs_props(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let obs = self.obs_props(cx);
        if self.sync_marks.obs_props.as_ref() != Some(&obs) {
            self.sync_marks.obs_props = Some(obs.clone());
            self.observatory
                .update(cx, |o, cx| o.set_props(obs, window, cx));
        }
    }

    /// `focusedChat`: which tab is on screen, so PermissionPrompts can skip a floating card
    /// for a request the chat already shows inline.
    fn publish_focused_chat(&mut self, id: &str, code: &str, cx: &mut Context<Self>) {
        let next = (self.shell_active && !id.is_empty() && !code.is_empty()).then(|| ChatRef {
            workspace_id: id.to_string(),
            code: code.to_string(),
        });
        if self.sync_marks.focused_chat == next {
            return;
        }
        let prev = std::mem::replace(&mut self.sync_marks.focused_chat, next.clone());
        self.state.update(cx, |s, cx| {
            match next {
                Some(r) => s.focused_chat = Some(r),
                // Cleanup: clear only what this shell published.
                None => {
                    if s.focused_chat.is_some() && s.focused_chat == prev {
                        s.focused_chat = None;
                    }
                }
            }
            cx.notify();
        });
    }

    /// `pendingChatFocus` (an ask toast, an inbox `question:` target): select that tab once
    /// the strip has it, refetching once when it is missing.
    fn take_pending_chat_focus(&mut self, cx: &mut Context<Self>) {
        let pending = self.state.read(cx).pending_chat_focus.clone();
        let Some(pending) = pending else {
            self.pending_focus_refetching.clear();
            return;
        };
        if pending.workspace_id != self.ws_id || self.tabs().is_empty() {
            return;
        }
        let code = pending.code.clone();
        if self.tabs().iter().any(|t| t.code == code) {
            self.state.update(cx, |s, cx| {
                s.pending_chat_focus = None;
                cx.notify();
            });
            self.pending_focus_refetching.clear();
            self.set_active(&code, cx);
            let codes: Vec<String> = self.tabs().iter().map(|t| t.code.clone()).collect();
            self.resync_activated_chats(&codes, None, &[], cx);
            self.refetch_tabs(cx);
            return;
        }
        let key = self.key(&code);
        if self.pending_focus_refetching == key {
            return;
        }
        self.pending_focus_refetching = key.clone();
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let res = svc.workspaces.list_workspace_chats(&id).await;
            let _ = this.update(cx, |v, cx| {
                if v.pending_focus_refetching != key {
                    return;
                }
                if let Ok(fresh) = res
                    && !fresh.is_empty()
                {
                    let has = fresh.iter().any(|t| t.code == code);
                    let codes: Vec<String> = fresh.iter().map(|t| t.code.clone()).collect();
                    v.tabs = Some(fresh);
                    if has {
                        v.active = Some(code.clone());
                    }
                    v.put_strip(cx);
                    v.resync_activated_chats(&codes, None, &[], cx);
                }
                v.state.update(cx, |s, cx| {
                    s.pending_chat_focus = None;
                    cx.notify();
                });
                v.pending_focus_refetching.clear();
                v.touch(cx);
            });
        })
        .detach();
    }

    /// The viewed watermark: committing to a workspace (not a rapid scan) marks its activity
    /// seen, clearing the Inbox card's changed-dot.
    fn mark_viewed(&mut self, cx: &mut Context<Self>) {
        if !self.shell_active {
            self.viewed_for.clear();
            return;
        }
        let s = self.state.read(cx);
        if s.scan_pending || self.viewed_for == self.ws_id || s.briefing.generated_at == 0 {
            return;
        }
        let activity = s
            .briefing
            .workspaces
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|w| w.workspace_id == self.ws_id)
            .map_or(0, |w| w.last_activity_at);
        self.viewed_for = self.ws_id.clone();
        if activity > 0 {
            let (svc, id) = (self.services.clone(), self.ws_id.clone());
            cx.background_executor()
                .spawn(async move {
                    let _ = svc.briefing.mark_workspace_viewed(&id, activity).await;
                })
                .detach();
        }
    }

    /// Swap the composer draft when the active chat tab changes.
    fn swap_draft(&mut self, key: &str, cx: &mut Context<Self>) {
        if key == self.input_bound_key {
            return;
        }
        if !self.input_bound_key.is_empty() {
            self.persist_draft_slot(cx);
        }
        self.input_bound_key = key.to_string();
        let input = self.input_by_key.get(key).cloned().unwrap_or_default();
        let atts: Vec<ChatAttachmentDraft> = self
            .attachments_by_key
            .get(key)
            .cloned()
            .unwrap_or_default();
        self.props_dirty = true;
        // The chat restores the new tab's chrome on its reset key; the draft follows.
        let props = self.chat_props(cx);
        self.chat.update(cx, |c, cx| {
            c.set_props(props, cx);
            c.set_draft(input, atts, cx);
        });
    }

    /// Point at the Plan panel when a plan lands on the open workspace (not on open).
    fn plan_filed_transition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.ws(cx) else { return };
        let marker = format!("{}\t{}", ws.id, if ws.plan.is_some() { "1" } else { "0" });
        if self.had_plan_for == marker {
            return;
        }
        let before = std::mem::replace(&mut self.had_plan_for, marker);
        if before == format!("{}\t0", ws.id) && ws.plan.is_some() {
            let id = ws.id.clone();
            self.observatory.update(cx, |o, cx| {
                o.select_code_panel(recur_view_core::prefs::CodePanelKey::Plan, window, cx)
            });
            self.state.update(cx, |s, cx| {
                s.observatory_jump = Some(recur_view_core::state::ObsJump {
                    workspace_id: id,
                    panel: "plan".into(),
                    ..Default::default()
                });
                cx.notify();
            });
            recur_view_core::toast(
                recur_ui::widgets::ToastKind::Success,
                "Plan filed — review and approve it in the Plan panel",
                cx,
            );
        }
    }

    /// A merged child's transcript (a plan step link): switch to the conductor, then scroll
    /// its wave card into view.
    fn child_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(child) = self
            .state
            .read(cx)
            .child_transcript_request()
            .map(str::to_string)
        else {
            return;
        };
        let conductor = self
            .ws(cx)
            .map(|w| w.loop_chat_code.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "MA".into());
        if self.active_code() != conductor {
            self.set_active(&conductor, cx);
            return;
        }
        let fact = self
            .feed
            .read(cx)
            .facts
            .iter()
            .find(|f| f.card == "fanout" && f.data.contains(&format!("\"{child}\"")))
            .map(|f| f.id.clone());
        let Some(fact_id) = fact.filter(|f| !f.is_empty()) else {
            return;
        };
        let key = format!("{}\t{child}", self.ws_id);
        if self.child_reveal_for == key {
            return;
        }
        self.child_reveal_for = key.clone();
        self.observatory
            .update(cx, |o, cx| o.show_agent_panel(window, cx));
        if !self.chat_open {
            self.chat_open = true;
            self.touch(cx);
        }
        let reveal = self.chat.update(cx, |c, cx| c.reveal_fact(&fact_id, cx));
        cx.spawn(async move |this, cx| {
            reveal.await;
            let _ = this.update(cx, |v, _| {
                if v.child_reveal_for == key {
                    v.child_reveal_for.clear();
                }
            });
        })
        .detach();
    }

    /// The New-workspace opening instruction: mode and model onto the first tab, then sent (or
    /// queued until setup finishes) as the first message. The seed is consumed only once the
    /// dock is ready, so it is never dropped.
    fn apply_seed(&mut self, cx: &mut Context<Self>) {
        let Some(seed) = self
            .state
            .read(cx)
            .workspace_seeds
            .get(&self.ws_id)
            .cloned()
        else {
            return;
        };
        let Some(ws) = self.ws(cx).cloned() else {
            return;
        };
        if !ws.is_main && self.create_failed(cx) {
            self.state
                .update(cx, |s, cx| s.take_workspace_seed(&ws.id, cx));
            return;
        }
        let code = self.active_code();
        let atts: Vec<ChatAttachmentDraft> = seed
            .attachments
            .iter()
            .map(|a| ChatAttachmentDraft {
                id: a.id.clone(),
                name: a.name.clone(),
                mime: a.mime.clone(),
                data_url: a.data_url.clone(),
            })
            .collect();
        if (!seed.text.is_empty() || !atts.is_empty()) && !code.is_empty() {
            self.paint_opening_user(&self.key(&code), &seed.text, &seed.display, &atts);
            self.props_dirty = true;
        }
        if self.composer_busy(cx) {
            return;
        }
        if let Some(on) = seed.simplify
            && !code.is_empty()
            && code != "MA"
        {
            let key = self.key(&code);
            if self.simplify_by_key.get(&key) != Some(&on) {
                *self.simplify_gen.entry(key.clone()).or_default() += 1;
                self.simplify_by_key.insert(key.clone(), on);
            }
            self.simplify_fetched.insert(key);
        }
        if (!seed.mode.is_empty() || !seed.model.is_empty()) && code != "MA" {
            let settings = self.state.read(cx).settings.clone();
            if !self.catalog_ready() || settings.is_none() {
                return;
            }
            let key = self.key(&code);
            if self
                .pickers_by_key
                .get(&key)
                .is_none_or(|p| p.model.is_empty())
            {
                let mode = ChatMode::from_key(&seed.mode);
                let d = recur_view_core::modes::resolved_default_for_mode(
                    mode.key(),
                    settings.as_ref(),
                    &self.catalog_models,
                );
                let model = if seed.model.is_empty() {
                    d.model
                } else {
                    seed.model.clone()
                };
                let cur = if seed.flags.is_empty() {
                    d.flags
                } else {
                    seed.flags.clone()
                };
                let flags = recur_view_core::models::normalize_flags(
                    &recur_view_core::models::flags_for_model(&self.catalog_models, &model),
                    Some(&cur),
                );
                *self.picker_gen.entry(key.clone()).or_default() += 1;
                self.pickers_by_key.insert(
                    key,
                    recur_view_chat::heap::ChatPickerSnap { model, flags, mode },
                );
                self.props_dirty = true;
            }
        }
        if !seed.text.is_empty() || !atts.is_empty() {
            if code != "MA" && !self.pickers_ready() {
                return;
            }
            let Some(seed) = self
                .state
                .update(cx, |s, cx| s.take_workspace_seed(&ws.id, cx))
            else {
                return;
            };
            let display = seed.display.trim().to_string();
            self.set_chat_draft(seed.text.clone(), atts, cx);
            self.send(&display, false, cx);
            return;
        }
        self.state
            .update(cx, |s, cx| s.take_workspace_seed(&ws.id, cx));
    }

    /// `paintOpeningUser`: the opening bubble at once, before the worktree or the send.
    fn paint_opening_user(
        &mut self,
        key: &str,
        text: &str,
        display: &str,
        drafts: &[ChatAttachmentDraft],
    ) {
        let marked = recur_engine::view::mentions::compose_marked_text(
            text,
            drafts.iter().map(|d| d.id.as_str()),
        )
        .trim()
        .to_string();
        let body = if display.trim().is_empty() {
            recur_engine::view::chat::display_user_chat_text(&marked)
        } else {
            display.trim().to_string()
        };
        if body.is_empty() && drafts.is_empty() {
            return;
        }
        let live = self.chat_by_key.get(key).cloned().unwrap_or_default();
        if live.iter().any(|m| m.me && m.optimistic && m.body == body) {
            return;
        }
        self.push_msg(
            key,
            AgentMsg {
                me: true,
                body,
                optimistic: true,
                attachments: super::send::bubble_attachments(drafts),
                ..Default::default()
            },
        );
    }

    /// `.wsv-strip`: Couldn't create / Setup failed with the error, its output toggle and the
    /// retry, edit and remove actions.
    pub(crate) fn render_failed_strip(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        crate::overlay::failed_strip(self, cx)
    }
}
