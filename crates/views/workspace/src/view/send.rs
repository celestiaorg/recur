//! Send, the follow-up queue, steer, Stop and Retry (`send`, `dispatchSend`, `dispatchSteer`,
//! `tryFlushPending`, `stopChat`, `retryChat`), and the composer answers to `ask_user` and loop
//! escalations.

use std::collections::BTreeMap;

use gpui_kit::{App, Context, Task};
use recur_domain::ChatAttachment;
use recur_engine::view::chat::{
    AgentMsg, CHAT_RETRY_DISPLAY, ChatAttachmentDraft, ChatAttachmentView, display_user_chat_text,
    drop_optimistic_user, finish_turn, format_ask_user_display_text, is_chat_injected_reply,
    is_chat_stopped_reply, merge_rejected_steer_attachments, merge_rejected_steer_draft,
    pending_send_bubble_body, should_skip_idle_transcript_reset, to_send_attachments,
};
use recur_engine::view::chat_surface::attach_chat_surface;
use recur_engine::view::mentions::{compose_marked_text, strip_attachment_markers};
use recur_ui::widgets::ToastKind;
use recur_view_chat::heap::{ChatPickerSnap, PendingChatSend};
use recur_view_chat::hoist::HoistState;
use recur_view_core::chat::AskUserAnswer;
use recur_view_core::models::{FlagMap, chat_steer_follow_up};
use recur_view_core::modes::ChatMode;
use recur_view_core::{bridge_error_message, toast};

use super::{WorkspaceView, now_ms};
use crate::uuid::uuidv7;

/// A routed prompt-action send: where it lands and with which dock selection.
#[derive(Clone, Debug)]
pub(crate) struct Routed {
    pub id: String,
    pub code: String,
    pub key: String,
    pub mode: ChatMode,
    pub model: String,
    pub flags: FlagMap,
}

pub(crate) fn wire_attachments(drafts: &[ChatAttachmentDraft]) -> Vec<ChatAttachment> {
    to_send_attachments(drafts)
        .into_iter()
        .map(|a| ChatAttachment {
            id: a.id,
            name: a.name,
            mime: a.mime,
            data: a.data,
        })
        .collect()
}

pub(crate) fn bubble_attachments(drafts: &[ChatAttachmentDraft]) -> Vec<ChatAttachmentView> {
    drafts
        .iter()
        .map(|d| ChatAttachmentView {
            id: d.id.clone(),
            name: d.name.clone(),
            mime: d.mime.clone(),
            data_url: d.data_url.clone(),
            ..Default::default()
        })
        .collect()
}

/// `takeFlushBatch`: prompt-action items flush alone; consecutive normal sends merge.
pub(crate) fn take_flush_batch(
    queue: &[PendingChatSend],
) -> (Vec<PendingChatSend>, Vec<PendingChatSend>) {
    if queue.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if queue[0].prompt_action {
        return (vec![queue[0].clone()], queue[1..].to_vec());
    }
    let n = queue
        .iter()
        .skip(1)
        .position(|p| p.prompt_action)
        .map_or(queue.len(), |i| i + 1);
    (queue[..n].to_vec(), queue[n..].to_vec())
}

/// `mergePendingSends`: the latest mode / model / flags win; text and attachments concatenate
/// in queue order. Display text is only synthesized when an entry has one (prompt actions).
pub(crate) fn merge_pending_sends(queue: &[PendingChatSend]) -> PendingChatSend {
    let last = queue.last().cloned().unwrap_or_default();
    let has_display = queue.iter().any(|p| !p.display_text.is_empty());
    let join = |f: &dyn Fn(&PendingChatSend) -> String| {
        queue
            .iter()
            .map(f)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let display = if has_display {
        join(&|p| {
            if p.display_text.is_empty() {
                p.plain.trim().to_string()
            } else {
                p.display_text.trim().to_string()
            }
        })
    } else {
        String::new()
    };
    PendingChatSend {
        id: last.id.clone(),
        marked: join(&|p| p.marked.clone()),
        plain: join(&|p| p.plain.clone()),
        drafts: queue.iter().flat_map(|p| p.drafts.clone()).collect(),
        mode: last.mode,
        model: last.model.clone(),
        flags: last.flags.clone(),
        display_text: display,
        prompt_action: queue.len() == 1 && queue[0].prompt_action,
        seq: 0,
        fresh_session: queue.len() == 1 && queue[0].fresh_session,
    }
}

/// Text restored into a composer after a rejected answer: the sent text, then whatever was
/// typed since; attachments without duplicates.
fn restore_after_reject(
    saved_input: &str,
    saved_atts: &[ChatAttachmentDraft],
    cur_input: &str,
    cur_atts: &[ChatAttachmentDraft],
) -> (String, Vec<ChatAttachmentDraft>) {
    let cur_plain = strip_attachment_markers(cur_input).trim().to_string();
    let restored = [saved_input.to_string(), cur_plain]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut atts: Vec<ChatAttachmentDraft> = saved_atts
        .iter()
        .filter(|a| !cur_atts.iter().any(|c| c.id == a.id))
        .cloned()
        .collect();
    atts.extend_from_slice(cur_atts);
    (restored, atts)
}

impl WorkspaceView {
    // ---- the draft (`bind:input` / `bind:attachments`) ----

    pub(crate) fn chat_input(&self, cx: &App) -> String {
        self.chat.read(cx).input().to_string()
    }

    pub(crate) fn chat_attachments(&self, cx: &App) -> Vec<ChatAttachmentDraft> {
        self.chat.read(cx).attachments().to_vec()
    }

    pub(crate) fn set_chat_draft(
        &mut self,
        input: String,
        atts: Vec<ChatAttachmentDraft>,
        cx: &mut Context<Self>,
    ) {
        self.chat.update(cx, |c, cx| c.set_draft(input, atts, cx));
    }

    /// The composer's draft into its key's slot.
    pub(crate) fn persist_draft_slot(&mut self, cx: &App) {
        let key = self.input_bound_key.clone();
        if key.is_empty() {
            return;
        }
        self.input_by_key.insert(key.clone(), self.chat_input(cx));
        self.attachments_by_key
            .insert(key, self.chat_attachments(cx));
    }

    /// Write a key's draft slot and, when it is the bound tab, the composer too.
    fn write_draft(
        &mut self,
        key: &str,
        input: String,
        atts: Vec<ChatAttachmentDraft>,
        cx: &mut Context<Self>,
    ) {
        self.input_by_key.insert(key.to_string(), input.clone());
        self.attachments_by_key
            .insert(key.to_string(), atts.clone());
        if self.input_bound_key == key {
            self.set_chat_draft(input, atts, cx);
        }
    }

    fn draft_of(&self, key: &str, cx: &App) -> (String, Vec<ChatAttachmentDraft>) {
        if self.input_bound_key == key {
            (self.chat_input(cx), self.chat_attachments(cx))
        } else {
            (
                self.input_by_key.get(key).cloned().unwrap_or_default(),
                self.attachments_by_key
                    .get(key)
                    .cloned()
                    .unwrap_or_default(),
            )
        }
    }

    // ---- transcript rows ----

    pub(crate) fn push_msg(&mut self, key: &str, m: AgentMsg) {
        let list = self.chat_by_key.entry(key.to_string()).or_default();
        if m.me {
            for row in list.iter_mut().filter(|r| r.streaming) {
                row.streaming = false;
            }
        }
        list.push(m);
    }

    pub(crate) fn finish_turn(&mut self, key: &str, fallback: &str) {
        let started = self.turn_started_at_by_key.get(key).copied().unwrap_or(0);
        let elapsed = if started > 0 {
            (now_ms() - started).max(0)
        } else {
            0
        };
        let got = self.turn_got_text_by_key.get(key).copied().unwrap_or(false);
        let list = self.chat_by_key.get(key).cloned().unwrap_or_default();
        self.chat_by_key.insert(
            key.to_string(),
            finish_turn(&list, fallback, got, elapsed, now_ms()),
        );
    }

    pub(crate) fn pending_bubbles(&self, key: &str) -> Vec<AgentMsg> {
        self.pending_by_key
            .get(key)
            .into_iter()
            .flatten()
            .map(|p| AgentMsg {
                me: true,
                body: pending_send_bubble_body(&p.display_text, &p.marked),
                queued: true,
                pending_id: p.id.clone(),
                attachments: bubble_attachments(&p.drafts),
                ..Default::default()
            })
            .collect()
    }

    fn clear_hold_pending(&mut self, key: &str) {
        self.hold_pending_by_key.remove(key);
    }

    // ---- send ----

    /// `send(opts)`. `alt` (⌘⏎) swaps steer and queue for a follow-up sent while busy.
    pub(crate) fn send(&mut self, display_text: &str, alt: bool, cx: &mut Context<Self>) {
        if self.ws(cx).is_none() || self.create_failed(cx) {
            return;
        }
        // Wait for dock restore on coding tabs: sending before the pickers land would use the
        // agent fallback and could overwrite a persisted Review mode.
        let code = self.active_code();
        if code != "MA" && !self.pickers_ready() {
            return;
        }
        let input = self.chat_input(cx);
        let drafts = self.chat_attachments(cx);
        let marked = compose_marked_text(&input, drafts.iter().map(|d| d.id.as_str()))
            .trim()
            .to_string();
        let plain = strip_attachment_markers(&marked).trim().to_string();
        let id = self.ws_id.clone();
        let key = self.key(&code);
        let busy = self.chat_key_busy(&key, cx) || self.prompt_busy_for(&key);
        let held = self.pending_by_key.get(&key).cloned().unwrap_or_default();
        let setup_hold = self.setup_hold(cx);

        // After Stop, Enter with an empty composer releases the held follow-ups.
        if plain.is_empty() && drafts.is_empty() {
            if busy || setup_hold || held.is_empty() {
                return;
            }
            self.clear_hold_pending(&key);
            self.flush_pending_now(&key, cx);
            self.touch(cx);
            return;
        }

        // A pending ask_user on this tab: the composer send is the answer.
        let pending_ask = self
            .state
            .read(cx)
            .user_questions
            .iter()
            .find(|q| q.workspace_id == id && q.agent_code == code)
            .cloned();
        if let Some(q) = pending_ask {
            let saved = (input.clone(), drafts.clone());
            self.write_draft(&key, String::new(), Vec::new(), cx);
            let answer = AskUserAnswer {
                choice: String::new(),
                free_text: plain,
                attachments: wire_attachments(&drafts),
            };
            let task = self
                .state
                .update(cx, |s, cx| s.answer_question(&q.id, answer, cx));
            cx.spawn(async move |this, cx| {
                if task.await {
                    return;
                }
                let _ = this.update(cx, |v, cx| {
                    let (ci, ca) = v.draft_of(&key, cx);
                    let (text, atts) = restore_after_reject(&saved.0, &saved.1, &ci, &ca);
                    v.write_draft(&key, text, atts, cx);
                });
            })
            .detach();
            return;
        }

        // An open loop escalation on the conductor tab: the composer send is the answer.
        if self.on_conductor_tab(cx)
            && self.live_esc.as_ref().is_some_and(|e| e.status == "open")
            && !plain.is_empty()
        {
            let saved = (input.clone(), drafts.clone());
            self.write_draft(&key, String::new(), Vec::new(), cx);
            let task = self.answer_loop_escalation("", &plain, cx);
            cx.spawn(async move |this, cx| {
                if task.await {
                    return;
                }
                let _ = this.update(cx, |v, cx| {
                    let (ci, ca) = v.draft_of(&key, cx);
                    let (text, atts) = restore_after_reject(&saved.0, &saved.1, &ci, &ca);
                    v.write_draft(&key, text, atts, cx);
                });
            })
            .detach();
            return;
        }

        let opening = display_text.trim().to_string();
        let mode = self.chat_mode(cx);
        let (model, flags) = self.dock_model(cx);
        let pending = PendingChatSend {
            id: uuidv7(),
            marked: marked.clone(),
            plain: plain.clone(),
            drafts: drafts.clone(),
            mode,
            model,
            flags,
            display_text: opening.clone(),
            ..Default::default()
        };
        self.freeze_simplify(&key, cx);
        self.write_draft(&key, String::new(), Vec::new(), cx);
        self.rejected_steers_by_key.remove(&key);
        let bubble = pending_send_bubble_body(&opening, &marked);
        if busy || setup_hold {
            if !setup_hold
                && self.chat_key_busy(&key, cx)
                && self.steer_by_default(cx) != alt
                && chat_steer_follow_up(true, pending.mode.key(), !pending.drafts.is_empty(), &code)
            {
                let display = pending_send_bubble_body(&pending.display_text, &marked);
                self.push_msg(
                    &key,
                    AgentMsg {
                        me: true,
                        body: display,
                        optimistic: true,
                        pending_id: pending.id.clone(),
                        attachments: bubble_attachments(&drafts),
                        ..Default::default()
                    },
                );
                self.dispatch_steer(&id, &code, &key, pending, cx);
                self.touch(cx);
                return;
            }
            if setup_hold {
                let name_from = if opening.is_empty() {
                    marked.clone()
                } else {
                    opening.clone()
                };
                let persist = if code == "MA" {
                    Task::ready(())
                } else {
                    self.persist_chat_dock(&id, &code, None, Some(&pending), cx)
                };
                let (svc, id2, code2) = (self.services.clone(), id.clone(), code.clone());
                cx.background_executor()
                    .spawn(async move {
                        persist.await;
                        let _ = svc
                            .workspaces
                            .apply_provisional_chat_titles(&id2, &code2, &name_from)
                            .await;
                    })
                    .detach();
            }
            let row = AgentMsg {
                me: true,
                body: bubble.clone(),
                queued: true,
                pending_id: pending.id.clone(),
                attachments: bubble_attachments(&drafts),
                ..Default::default()
            };
            // A seed may have painted this body without a pending id: upgrade it so hydrate
            // reattaches it through the pending bubbles instead of duplicating.
            let list = self.chat_by_key.entry(key.clone()).or_default();
            match list
                .iter()
                .position(|m| m.me && m.optimistic && m.pending_id.is_empty() && m.body == bubble)
            {
                Some(i) => {
                    let m = &mut list[i];
                    m.body = row.body;
                    m.queued = true;
                    m.pending_id = row.pending_id;
                    m.attachments = row.attachments;
                }
                None => self.push_msg(&key, row),
            }
            self.pending_by_key
                .entry(key.clone())
                .or_default()
                .push(pending);
            self.touch(cx);
            return;
        }

        // Idle: enqueue behind held follow-ups, then flush one batch.
        self.clear_hold_pending(&key);
        if !held.is_empty() {
            self.pending_by_key
                .entry(key.clone())
                .or_default()
                .push(pending);
            self.flush_pending_now(&key, cx);
            self.touch(cx);
            return;
        }
        let live = self.chat_by_key.get(&key).cloned().unwrap_or_default();
        if !live
            .iter()
            .any(|m| m.me && m.optimistic && m.body == bubble)
        {
            self.push_msg(
                &key,
                AgentMsg {
                    me: true,
                    body: bubble,
                    optimistic: true,
                    attachments: bubble_attachments(&drafts),
                    ..Default::default()
                },
            );
        }
        self.dispatch_send(&id, &code, &key, pending, cx);
        self.touch(cx);
    }

    /// The dock's model and flags for the active tab (`model` / `flags` derivations).
    pub(crate) fn dock_model(&self, cx: &App) -> (String, FlagMap) {
        let p = self.pickers_by_key.get(&self.active_key());
        let s = self.state.read(cx);
        let raw = p
            .map(|p| p.model.clone())
            .filter(|m| !m.is_empty())
            .or_else(|| {
                s.settings
                    .as_ref()
                    .map(|s| s.model_agent.clone())
                    .filter(|m| !m.is_empty())
            })
            .unwrap_or_else(|| "claude-code/claude-sonnet-5-5".into());
        let disabled = s
            .settings
            .as_ref()
            .map(|s| s.disabled_providers.clone())
            .unwrap_or_default();
        if recur_view_core::models::provider_enabled(
            &disabled,
            recur_view_core::models::model_provider(&raw),
        ) {
            return (raw, p.map(|p| p.flags.clone()).unwrap_or_default());
        }
        let d = recur_view_core::modes::resolved_default_for_mode(
            self.chat_mode(cx).key(),
            s.settings.as_ref(),
            &self.catalog_models,
        );
        (d.model, d.flags)
    }

    /// Retry: the engine replays the last stored wire prompt. A Loop-mode PA tab sends agent so
    /// Retry never engages a run.
    pub(crate) fn retry_chat(&mut self, cx: &mut Context<Self>) {
        if self.ws(cx).is_none() || self.setup_hold(cx) {
            return;
        }
        let code = self.active_code();
        if code != "MA" && !self.pickers_ready() {
            return;
        }
        let (id, key) = (self.ws_id.clone(), self.key(&code));
        if self.chat_key_busy(&key, cx) || self.prompt_busy_for(&key) {
            return;
        }
        let msgs = self.chat_by_key.get(&key).cloned().unwrap_or_default();
        if !msgs.iter().any(|m| m.me && !m.queued) {
            return;
        }
        let mode = match self.chat_mode(cx) {
            ChatMode::Loop => ChatMode::Agent,
            m => m,
        };
        let (model, flags) = self.dock_model(cx);
        let pending = PendingChatSend {
            id: uuidv7(),
            marked: CHAT_RETRY_DISPLAY.into(),
            mode,
            model,
            flags,
            display_text: CHAT_RETRY_DISPLAY.into(),
            ..Default::default()
        };
        self.dispatch_send(&id, &code, &key, pending, cx);
        self.touch(cx);
    }

    pub(crate) fn remove_queued(&mut self, pending_id: &str, cx: &mut Context<Self>) {
        if pending_id.is_empty() || self.ws(cx).is_none() {
            return;
        }
        let key = self.active_key();
        let Some(q) = self.pending_by_key.get_mut(&key) else {
            return;
        };
        if !q.iter().any(|p| p.id == pending_id) {
            return;
        }
        q.retain(|p| p.id != pending_id);
        if let Some(list) = self.chat_by_key.get_mut(&key) {
            list.retain(|m| m.pending_id != pending_id);
        }
        self.touch(cx);
    }

    /// Fold every queued follow-up into one next turn once idle; not after Stop until the user
    /// sends again.
    pub(crate) fn try_flush_pending(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.queue_owner || self.hold_pending_by_key.get(key).copied().unwrap_or(false) {
            return;
        }
        self.flush_pending_now(key, cx);
    }

    pub(crate) fn flush_pending_now(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.queue_owner
            || self.create_failed(cx)
            || self.setup_hold(cx)
            || self.chat_key_busy(key, cx)
            || self.prompt_busy_for(key)
        {
            return;
        }
        let q = self.pending_by_key.get(key).cloned().unwrap_or_default();
        if q.is_empty() {
            return;
        }
        let (batch, rest) = take_flush_batch(&q);
        if batch.is_empty() {
            return;
        }
        self.pending_by_key.insert(key.to_string(), rest);
        let ids: Vec<String> = batch.iter().map(|p| p.id.clone()).collect();
        let merged = merge_pending_sends(&batch);
        let body = pending_send_bubble_body(&merged.display_text, &merged.marked);
        let list = self.chat_by_key.entry(key.to_string()).or_default();
        list.retain(|m| m.pending_id.is_empty() || !ids.contains(&m.pending_id));
        list.push(AgentMsg {
            me: true,
            body,
            optimistic: true,
            attachments: bubble_attachments(&merged.drafts),
            ..Default::default()
        });
        let Some((id, code)) = key.split_once('\t') else {
            return;
        };
        let (id, code) = (id.to_string(), code.to_string());
        self.dispatch_send(&id, &code, key, merged, cx);
        self.touch(cx);
    }

    /// Inject a follow-up into a live turn. The original dispatch still owns the turn's
    /// busy flag and generation.
    pub(crate) fn dispatch_steer(
        &mut self,
        id: &str,
        code: &str,
        key: &str,
        pending: PendingChatSend,
        cx: &mut Context<Self>,
    ) {
        let mut job = pending;
        if job.seq == 0 {
            self.steer_clock += 1;
            job.seq = self.steer_clock;
        }
        let atts = wire_attachments(&job.drafts);
        let retry = job.display_text == CHAT_RETRY_DISPLAY || job.marked == CHAT_RETRY_DISPLAY;
        let wire = if retry {
            job.marked.clone()
        } else {
            attach_chat_surface(&job.marked, &self.chat_surface(cx))
        };
        let display = pending_send_bubble_body(&job.display_text, &job.marked);
        let (svc, id, code, key) = (
            self.services.clone(),
            id.to_string(),
            code.to_string(),
            key.to_string(),
        );
        let flags: BTreeMap<String, String> = job.flags.clone();
        cx.spawn(async move |this, cx| {
            let res = svc
                .workspaces
                .send_workspace_message(
                    &id,
                    &code,
                    &wire,
                    &job.model,
                    &flags,
                    &atts,
                    job.mode.key(),
                    &job.display_text,
                    job.fresh_session,
                )
                .await;
            let _ = this.update(cx, |v, cx| {
                match res {
                    Ok(reply) => {
                        if is_chat_injected_reply(&reply) {
                            return;
                        }
                        let text = if reply.trim().is_empty() {
                            "Done.".to_string()
                        } else {
                            reply.trim().to_string()
                        };
                        if is_chat_stopped_reply(&text) {
                            v.hold_pending_by_key.insert(key.clone(), true);
                            v.drop_rejected_steer(&key, job, &display, cx);
                            return;
                        }
                        v.finish_turn(&key, &text);
                    }
                    Err(_) => {
                        v.drop_rejected_steer(&key, job, &display, cx);
                        toast(
                            ToastKind::Error,
                            "Could not steer the agent — try again",
                            cx,
                        );
                    }
                }
                v.touch(cx);
            });
        })
        .detach();
    }

    fn drop_rejected_steer(
        &mut self,
        key: &str,
        pending: PendingChatSend,
        display: &str,
        cx: &mut Context<Self>,
    ) {
        let list = self.chat_by_key.get(key).cloned().unwrap_or_default();
        self.chat_by_key.insert(
            key.to_string(),
            drop_optimistic_user(&list, display, &pending.id),
        );
        let fails = self
            .rejected_steers_by_key
            .entry(key.to_string())
            .or_default();
        fails.retain(|p| p.id != pending.id);
        fails.push(pending);
        fails.sort_by_key(|p| p.seq);
        self.restore_steer_draft(key, cx);
        self.touch(cx);
    }

    fn restore_steer_draft(&mut self, key: &str, cx: &mut Context<Self>) {
        let fails = self
            .rejected_steers_by_key
            .get(key)
            .cloned()
            .unwrap_or_default();
        let (cur_input, cur_atts) = self.draft_of(key, cx);
        let cur_plain = strip_attachment_markers(&cur_input).trim().to_string();
        let marked: Vec<String> = fails.iter().map(|p| p.marked.clone()).collect();
        let drafts: Vec<Vec<ChatAttachmentDraft>> =
            fails.iter().map(|p| p.drafts.clone()).collect();
        let restored = merge_rejected_steer_draft(&marked, &cur_plain);
        let atts = merge_rejected_steer_attachments(&drafts, &cur_atts, |a| a.id.as_str());
        self.write_draft(key, restored, atts, cx);
    }

    /// `persistChatDock`: remember the tab's selection (MA: only Simplify).
    pub(crate) fn persist_chat_dock(
        &self,
        id: &str,
        code: &str,
        simplify: Option<bool>,
        sel: Option<&PendingChatSend>,
        cx: &App,
    ) -> Task<()> {
        let svc = self.services.clone();
        let (id, code) = (id.to_string(), code.to_string());
        if code == "MA"
            && let Some(on) = simplify
        {
            return cx.background_executor().spawn(async move {
                let _ = svc
                    .workspaces
                    .remember_workspace_chat_simplify(&id, &code, on)
                    .await;
            });
        }
        let snap = match sel {
            Some(p) => Some(ChatPickerSnap {
                model: p.model.clone(),
                flags: p.flags.clone(),
                mode: p.mode,
            }),
            None => self.pickers_by_key.get(&self.key(&code)).cloned(),
        };
        let mode = snap
            .as_ref()
            .map(|p| p.mode)
            .unwrap_or_else(|| self.chat_mode(cx));
        let (model, flags) = snap.map(|p| (p.model, p.flags)).unwrap_or_default();
        cx.background_executor().spawn(async move {
            let _ = svc
                .workspaces
                .remember_workspace_chat_selection(&id, &code, &model, &flags, mode.key(), simplify)
                .await;
        })
    }

    pub(crate) fn dispatch_send(
        &mut self,
        id: &str,
        code: &str,
        key: &str,
        pending: PendingChatSend,
        cx: &mut Context<Self>,
    ) {
        let gen_ = self.send_gen_by_key.get(key).copied().unwrap_or(0) + 1;
        self.send_gen_by_key.insert(key.to_string(), gen_);
        self.sending_by_key.insert(key.to_string(), true);
        self.live_model_by_key
            .entry(key.to_string())
            .or_insert_with(|| pending.model.clone());
        self.hoist_send_gen_by_key.remove(key);
        self.turn_started_at_by_key
            .insert(key.to_string(), now_ms());
        self.turn_got_text_by_key.insert(key.to_string(), false);
        // Optimistic provisional tab title (the backend also paints and upgrades it).
        if let Some(tab) = self
            .tabs
            .as_mut()
            .and_then(|tabs| tabs.iter_mut().find(|t| t.code == code))
            && (tab.title.is_empty() || tab.title == "New chat")
        {
            let label =
                if pending.display_text == CHAT_RETRY_DISPLAY || pending.display_text.is_empty() {
                    pending.plain.clone()
                } else {
                    pending.display_text.clone()
                };
            let from = display_user_chat_text(&label);
            let first = from
                .split('\n')
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(|l| l.chars().take(28).collect::<String>());
            tab.title = first.unwrap_or_else(|| {
                if pending.drafts.is_empty() {
                    "New chat".into()
                } else {
                    "Image".into()
                }
            });
        }
        self.put_strip(cx);
        let agent_id = format!("{id}-{code}");
        let atts = wire_attachments(&pending.drafts);
        let retry =
            pending.display_text == CHAT_RETRY_DISPLAY || pending.marked == CHAT_RETRY_DISPLAY;
        let wire = if retry {
            pending.marked.clone()
        } else {
            attach_chat_surface(&pending.marked, &self.chat_surface(cx))
        };
        let simplify = self.freeze_simplify(key, cx);
        self.persist_chat_dock(id, code, Some(simplify), Some(&pending), cx)
            .detach();
        let (live_id, live_code, _) = HoistState::global(cx).live_chat_target(id, code);
        let svc = self.services.clone();
        let (id, code, key) = (id.to_string(), code.to_string(), key.to_string());
        let flags: BTreeMap<String, String> = pending.flags.clone();
        cx.spawn(async move |this, cx| {
            let res = svc
                .workspaces
                .send_workspace_message(
                    &live_id,
                    &live_code,
                    &wire,
                    &pending.model,
                    &flags,
                    &atts,
                    pending.mode.key(),
                    &pending.display_text,
                    pending.fresh_session,
                )
                .await;
            let _ = this.update(cx, |v, cx| {
                v.on_send_reply(&id, &code, &key, gen_, &agent_id, &pending, res, cx)
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn on_send_reply(
        &mut self,
        id: &str,
        code: &str,
        key: &str,
        gen_: u64,
        agent_id: &str,
        pending: &PendingChatSend,
        res: recur_services::Result<String>,
        cx: &mut Context<Self>,
    ) {
        let running = |v: &Self, aid: &str, cx: &App| v.state.read(cx).agents_running.contains(aid);
        let mut injected = false;
        match res {
            Ok(reply) => {
                if is_chat_injected_reply(&reply) {
                    injected = true;
                } else {
                    let text = reply.trim().to_string();
                    // Stop may race ahead of stopChat's hold flag: the reply is authoritative.
                    if is_chat_stopped_reply(&text) {
                        self.hold_pending_by_key.insert(key.to_string(), true);
                    }
                    let (_, _, cur_key) = HoistState::global(cx).live_chat_target(id, code);
                    // Idle can flush the queue before this reply lands; painting then would
                    // repeat this turn's text under the follow-up.
                    let current = self.send_gen_by_key.get(key) == Some(&gen_)
                        || self.send_gen_by_key.get(&cur_key) == Some(&gen_);
                    if current {
                        let fallback = if !text.is_empty() {
                            text
                        } else if pending.mode == ChatMode::Loop {
                            String::new()
                        } else {
                            "Done.".into()
                        };
                        self.finish_turn(key, &fallback);
                    }
                }
            }
            Err(_) => {
                // The RPC can fail while the engine turn still runs; idle owns the ending then.
                if !running(self, agent_id, cx) {
                    self.finish_turn(key, "The agent hit an error — try again.");
                }
            }
        }
        // finally
        let (live_id, live_code, live_key) = HoistState::global(cx).live_chat_target(id, code);
        HoistState::global(cx).clear_sending(key, gen_);
        HoistState::global(cx).clear_sending(&live_key, gen_);
        // Loop engage is keyed to this send's mode, even when a newer dispatch owns busy.
        if pending.mode == ChatMode::Loop
            && live_code != "MA"
            && pending.display_text != CHAT_RETRY_DISPLAY
        {
            self.on_loop_engaged(&live_id, cx);
        }
        let current = self.send_gen_by_key.get(key) == Some(&gen_)
            || self.send_gen_by_key.get(&live_key) == Some(&gen_);
        if !current || injected {
            self.touch(cx);
            return;
        }
        self.sending_by_key.insert(key.to_string(), false);
        self.sending_by_key.insert(live_key.clone(), false);
        if !running(self, agent_id, cx) {
            self.live_model_by_key.remove(key);
        }
        // Keep the turn stamp while the engine still works (binding timeout, remount); skip
        // the reset only when a newer turn already opened after a footer.
        let list = self
            .chat_by_key
            .get(&live_key)
            .or_else(|| self.chat_by_key.get(key))
            .cloned()
            .unwrap_or_default();
        let started = self
            .turn_started_at_by_key
            .get(&live_key)
            .or_else(|| self.turn_started_at_by_key.get(key))
            .copied()
            .unwrap_or(0);
        let armed = self
            .stale_idle_armed_by_key
            .get(&live_key)
            .copied()
            .unwrap_or(false)
            || self
                .stale_idle_armed_by_key
                .get(key)
                .copied()
                .unwrap_or(false);
        let skip = should_skip_idle_transcript_reset(&list, started, armed);
        let live_agent = format!("{live_id}-{live_code}");
        if !running(self, agent_id, cx) && !running(self, &live_agent, cx) && !skip {
            self.turn_started_at_by_key.remove(key);
            self.turn_started_at_by_key.remove(&live_key);
        }
        if !skip {
            // The session store is the authority; mid-turn this merges with the live tail.
            self.resync_chat(&live_id, &live_code, cx);
        }
        if live_code != "MA" {
            self.refresh_tab_titles(cx);
        }
        self.try_flush_pending(&live_key, cx);
        self.touch(cx);
    }

    /// After a Loop-mode send: refresh the run state and the strip.
    fn on_loop_engaged(&mut self, id: &str, cx: &mut Context<Self>) {
        let (svc, id) = (self.services.clone(), id.to_string());
        cx.spawn(async move |this, cx| {
            let list = svc.workspaces.list_workspace_chats(&id).await.ok();
            let fresh = svc.workspaces.list_workspaces().await.ok();
            let _ = this.update(cx, |v, cx| {
                if let Some(fresh) = fresh {
                    v.state
                        .update(cx, |s, cx| s.set_workspaces_from_list(fresh, cx));
                }
                if let Some(list) = list.filter(|l| !l.is_empty())
                    && id == v.ws_id
                {
                    v.set_tabs(list, cx);
                }
            });
        })
        .detach();
    }

    /// Stop: interrupt this tab's turn (and on a loop cockpit, the walk); queued follow-ups
    /// come back into the composer instead of sending.
    pub(crate) fn stop_chat(&mut self, cx: &mut Context<Self>) {
        let sending = self.sending(cx);
        let walk = self.loop_walk_live(cx);
        if self.prompt_busy_here() {
            if let Some(a) = &self.prompt_abort {
                a.abort();
            }
            if !sending && !walk {
                return;
            }
        }
        if self.ws(cx).is_none() || (!sending && !walk) {
            return;
        }
        let code = self.active_code();
        let owner = self
            .live_esc
            .as_ref()
            .map(|e| e.agent_code.trim().to_string())
            .unwrap_or_default();
        let mine = if owner.is_empty() {
            code == self.conductor_chat_code(cx)
        } else {
            owner == code
        };
        if self.live_esc.is_some() && mine {
            self.esc_load_gen += 1;
            self.live_esc = None;
        }
        let key = self.key(&code);
        // Hold before cancel resolves: idle can fire before the stopped reply lands.
        self.hold_pending_by_key.insert(key.clone(), true);
        let held = self.pending_by_key.get(&key).cloned().unwrap_or_default();
        if !held.is_empty() {
            let merged = merge_pending_sends(&held);
            let ids: Vec<String> = held.iter().map(|p| p.id.clone()).collect();
            self.pending_by_key.insert(key.clone(), Vec::new());
            if let Some(list) = self.chat_by_key.get_mut(&key) {
                list.retain(|m| m.pending_id.is_empty() || !ids.contains(&m.pending_id));
            }
            let draft_plain = strip_attachment_markers(&self.chat_input(cx))
                .trim()
                .to_string();
            let text = [merged.plain.clone(), draft_plain]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            let mut atts = merged.drafts.clone();
            atts.extend(self.chat_attachments(cx));
            self.write_draft(&key, text, atts, cx);
        }
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.background_executor()
            .spawn(async move {
                let _ = svc.workspaces.cancel_workspace_message(&id, &code).await;
            })
            .detach();
        self.touch(cx);
    }

    /// The loop walk is live on a cockpit tab: Stop shows even when the conductor is idle.
    pub(crate) fn loop_walk_live(&self, cx: &App) -> bool {
        self.show_loop_cockpit(cx)
            && self.ws(cx).is_some()
            && (!self.specialist_live(cx).is_empty()
                || recur_view_core::modes::ws_run_active(self.ws(cx)))
    }

    /// The composer's answer to an open loop escalation (same card as `ask_user`).
    pub(crate) fn answer_loop_escalation(
        &mut self,
        choice: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(held) = self.live_esc.clone() else {
            return Task::ready(false);
        };
        if self.ws(cx).is_none() {
            return Task::ready(false);
        }
        let id = self.ws_id.clone();
        let owner = self.conductor_chat_code(cx);
        let key = self.key(if owner.is_empty() { "MA" } else { &owner });
        let body = format_ask_user_display_text(choice, text);
        self.esc_load_gen += 1;
        self.live_esc = None;
        let last_same = self
            .chat_by_key
            .get(&key)
            .and_then(|l| l.last())
            .is_some_and(|m| m.me && m.body == body);
        if !last_same {
            self.push_msg(
                &key,
                AgentMsg {
                    me: true,
                    body: body.clone(),
                    optimistic: true,
                    ts: Some(now_ms()),
                    ..Default::default()
                },
            );
        }
        self.touch(cx);
        let (svc, choice, text) = (self.services.clone(), choice.to_string(), text.to_string());
        cx.spawn(async move |this, cx| {
            let res = svc
                .escalation
                .answer_escalation(&id, &choice, &text, &held.agent_code)
                .await;
            let ok = res.is_ok();
            let _ = this.update(cx, |v, cx| {
                if let Err(err) = res {
                    let msg = bridge_error_message(&err);
                    toast(
                        ToastKind::Error,
                        if msg.is_empty() {
                            "Could not send the answer".to_string()
                        } else {
                            msg
                        },
                        cx,
                    );
                    v.live_esc = Some(held);
                    if let Some(list) = v.chat_by_key.get_mut(&key) {
                        list.retain(|m| !(m.optimistic && m.me && m.body == body));
                    }
                    v.touch(cx);
                }
            });
            ok
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, marked: &str, display: &str, prompt: bool) -> PendingChatSend {
        PendingChatSend {
            id: id.into(),
            marked: marked.into(),
            plain: marked.into(),
            display_text: display.into(),
            prompt_action: prompt,
            ..Default::default()
        }
    }

    #[test]
    fn a_prompt_action_flushes_alone_and_normal_sends_merge() {
        let q = vec![
            p("1", "a", "", false),
            p("2", "b", "", false),
            p("3", "c", "Commit", true),
            p("4", "d", "", false),
        ];
        let (batch, rest) = take_flush_batch(&q);
        assert_eq!(batch.len(), 2);
        assert_eq!(rest.len(), 2);
        let (batch, rest) = take_flush_batch(&rest);
        assert_eq!(batch[0].id, "3");
        assert_eq!(rest[0].id, "4");
    }

    #[test]
    fn merged_sends_join_text_and_keep_display_only_for_prompt_batches() {
        let m = merge_pending_sends(&[p("1", "a", "", false), p("2", "b", "", false)]);
        assert_eq!(m.marked, "a\n\nb");
        assert_eq!(m.display_text, "");
        assert_eq!(m.id, "2");
        assert!(!m.prompt_action);
        let m = merge_pending_sends(&[p("3", "wire", "Commit", true)]);
        assert_eq!(m.display_text, "Commit");
        assert!(m.prompt_action);
    }

    #[test]
    fn a_rejected_answer_restores_the_sent_text_before_new_typing() {
        let d = |id: &str| ChatAttachmentDraft {
            id: id.into(),
            ..Default::default()
        };
        let (text, atts) = restore_after_reject("sent", &[d("a"), d("b")], "typed", &[d("b")]);
        assert_eq!(text, "sent\n\ntyped");
        assert_eq!(
            atts.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }
}
