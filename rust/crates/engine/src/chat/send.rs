//! The chat send pipeline (Go `workspace_chat.go` `SendWorkspaceMessage` /
//! `sendWorkspaceTurn`): one turn of a workspace coding-agent tab.
//!
//! Per tab, sends are FIFO by ticket. A text follow-up steers a live turn
//! (with the life lock released: Claude acks after the next tool result and
//! switch_mode needs the lock, R6); otherwise the send waits out the live
//! predecessor, applies any fork inject, registers its turn atomically with
//! the Stop check, and runs. Cleanup runs in Go's defer order.

use std::sync::Arc;
use std::time::Instant;

use recur_agent::attach::DISPLAY_HIDDEN_SENTINEL;
use recur_agent::{AgentEvent, Flags, TurnRequest, TurnResult, UserImage};
use recur_domain::{
    AgentRole, CHAT_MODE_AGENT, CHAT_MODE_LOOP, CHAT_MODE_PLAN, ChatAttachment,
    normalize_chat_mode, run_active,
};
use tokio_util::sync::CancellationToken;

use super::modes::chat_mode_shape;
use super::surface::{ChatSurface, peel_chat_surface};
use crate::Engine;
use crate::chatlife::ChatTurn;
use crate::codes::{
    CHAT_RETRY_DISPLAY_SENTINEL, MAESTRO_AGENT_CODE, PAIR_AGENT_CODE, chat_turn_key,
    is_chat_retry_display, is_pair_chat_code, loop_owner_code, normalize_pair_chat_code,
};
use crate::composer::ExpandOpts;
use crate::composer::skills::join_append_prompt;
use crate::governor::AgentPermit;
use crate::session_config::AgentConfig;
use crate::turn::checkpoint::last_chat_prompt_id;
use crate::turn::finalize::{
    CHAT_INJECTED_SENTINEL, CHAT_RESUME_DISPLAY, CHAT_STOPPED_REPLY, ChatOutcome, is_canceled,
};
use crate::turn::stream::ChatStream;
use crate::turn::transcript::is_mode_switch_continue_display;
use crate::turn::{AgentSpec, Emit};

pub(crate) const WORKSPACE_GONE: &str =
    "I can't find that workspace anymore — it may have been removed.";

/// A FIFO ticket not yet claimed by the send that took it. A send that
/// returns early claims and finishes it so later tickets advance; a dropped
/// send does so on a task.
struct Ticket {
    slot: Arc<crate::chatlife::ChatTabSlot>,
    seq: u64,
    pending: bool,
}

impl Ticket {
    async fn settle(&mut self) {
        if std::mem::take(&mut self.pending) {
            self.slot.claim(self.seq).await;
            self.slot.finish(self.slot.lock());
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !std::mem::take(&mut self.pending) {
            return;
        }
        let (slot, seq) = (Arc::clone(&self.slot), self.seq);
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                slot.claim(seq).await;
                slot.finish(slot.lock());
            });
        }
    }
}

/// The turn's teardown, in Go's defer order: idle status, the agent slot,
/// the MCP token, the remount continuation, the turn handle, the cancel.
struct TurnCleanup<'a> {
    eng: &'a Engine,
    workspace_id: &'a str,
    code: &'a str,
    cancel: CancellationToken,
    /// A switch_mode continuation (it gives back its remount count when it
    /// never registers).
    continuation: bool,
    turn: Option<Arc<ChatTurn>>,
    mcp_token: Option<String>,
    permit: Option<AgentPermit>,
    working: bool,
    canceled: bool,
}

impl Drop for TurnCleanup<'_> {
    fn drop(&mut self) {
        // A turn cancelled by quit stays live so Recover resumes it; one that
        // finished (or failed otherwise) goes idle even while quit unwinds.
        if self.working && !(self.eng.is_shutting_down() && self.canceled) {
            self.eng
                .update_agent_status(self.workspace_id, self.code, "idle");
        }
        drop(self.permit.take());
        if let Some(t) = self.mcp_token.take() {
            self.eng.unregister_mcp(&t);
        }
        match self.turn.take() {
            Some(turn) => {
                self.eng
                    .maybe_continue_after_mode_switch(self.workspace_id, self.code, &turn);
                self.eng
                    .unregister_chat_turn(self.workspace_id, self.code, &turn);
            }
            None if self.continuation => {
                self.eng.uncount_mode_remount(self.workspace_id, self.code);
            }
            None => {}
        }
        self.cancel.cancel();
    }
}

impl Engine {
    /// One turn of a workspace coding-agent tab (PA, PA2, …; empty = PA)
    /// in the worktree. The provider keeps its own system prompt; the tab's
    /// mode shapes tools (agent = read + edit; ask/plan/review read-only).
    /// `model` is a qualified ref or "" for the mode's Settings default;
    /// empty `flags` take that role's defaults; `mode` "" is the tab's
    /// persisted mode. `display_text` labels the user bubble (prompt
    /// actions). Returns the reply; tokens stream on the bus.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_workspace_message(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
        model: &str,
        flags: Option<&Flags>,
        attachments: &[ChatAttachment],
        mode: &str,
        display_text: &str,
    ) -> String {
        self.send_workspace_turn(
            workspace_id,
            code,
            text,
            model,
            flags,
            attachments,
            mode,
            display_text,
            false,
        )
        .await
    }

    /// [`Engine::send_workspace_message`] plus `fresh_session`: the turn
    /// starts a new provider session on the same tab (prompt actions).
    #[allow(clippy::too_many_arguments)]
    pub async fn send_workspace_turn(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
        model: &str,
        flags: Option<&Flags>,
        attachments: &[ChatAttachment],
        mode: &str,
        display_text: &str,
        fresh_session: bool,
    ) -> String {
        self.send_workspace_turn_epoch(
            workspace_id,
            code,
            text,
            model,
            flags,
            attachments,
            mode,
            display_text,
            fresh_session,
            0,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_workspace_turn_epoch(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
        model: &str,
        flags: Option<&Flags>,
        attachments: &[ChatAttachment],
        mode: &str,
        display_text: &str,
        fresh_session: bool,
        continue_epoch: u64,
    ) -> String {
        let mut code = normalize_pair_chat_code(code);
        let (text, wired) = match peel_chat_surface(text) {
            Some((rest, s)) => (rest, Some(s)),
            None => (text.to_owned(), None),
        };
        // The Maestro thread is a tab like any other, routed to the conductor
        // (which owns its model, tools and attachments; its mode is loop).
        if code == MAESTRO_AGENT_CODE {
            let (ws, c) = self.resolve_hoisted_chat(workspace_id, &code);
            return self
                .send_maestro_message_to(&ws, &c, &text, attachments, display_text)
                .await;
        }
        if !is_pair_chat_code(&code) {
            code = PAIR_AGENT_CODE.to_owned();
        }
        let (workspace_id, code) = self.resolve_hoisted_chat(workspace_id, &code);
        let slot = self.chat_tab_slot(&workspace_id, &code);
        let (seq, stop_gen) = slot.begin();
        let mut ticket = Ticket {
            slot: Arc::clone(&slot),
            seq,
            pending: true,
        };
        let reply = self
            .send_ticketed(
                &mut ticket,
                &workspace_id,
                &code,
                text,
                wired,
                model,
                flags,
                attachments,
                mode,
                display_text,
                fresh_session,
                continue_epoch,
                stop_gen,
            )
            .await;
        ticket.settle().await;
        reply
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_ticketed(
        &self,
        ticket: &mut Ticket,
        workspace_id: &str,
        code: &str,
        text: String,
        wired: Option<ChatSurface>,
        model: &str,
        flags: Option<&Flags>,
        attachments: &[ChatAttachment],
        mode: &str,
        display_text: &str,
        fresh_session: bool,
        continue_epoch: u64,
        stop_gen: u64,
    ) -> String {
        self.run_async_hook(|h| h.before_chat_claim.clone()).await;
        let slot = Arc::clone(&ticket.slot);
        let retry = is_chat_retry_display(display_text);
        let continuation = is_mode_switch_continue_display(display_text);
        let Some(mut ws) = self.store.workspace(workspace_id) else {
            return WORKSPACE_GONE.into();
        };
        let mut mode = self.resolve_chat_mode(workspace_id, code, mode);
        if ws.is_pr_chat_host() && (mode == CHAT_MODE_AGENT || mode == CHAT_MODE_LOOP) {
            return "Create a workspace from this pull request to edit files in Agent or Loop. Ask, Plan, and Review still work from the project checkout.".into();
        }
        if retry && mode == CHAT_MODE_LOOP {
            // Loop on a tab means "engage the autopilot", which Retry must
            // not do: replay in the tab's persisted non-loop mode.
            let persisted = normalize_chat_mode(&self.chat_mode_for(workspace_id, code));
            mode = if !persisted.is_empty() && persisted != CHAT_MODE_LOOP {
                persisted.to_owned()
            } else {
                CHAT_MODE_AGENT.to_owned()
            };
        }
        if !retry && mode == CHAT_MODE_LOOP {
            return self
                .send_loop_chat_message(ws, code, &text, attachments, display_text)
                .await;
        }
        let (mut life, held) = self.lock_chat_life(workspace_id, code).await;
        if !held {
            if continuation {
                self.uncount_mode_remount(workspace_id, code);
            }
            return WORKSPACE_GONE.into();
        }
        let mut cleanup = TurnCleanup {
            eng: self,
            workspace_id,
            code,
            cancel: CancellationToken::new(),
            continuation: false,
            turn: None,
            mcp_token: None,
            permit: None,
            working: false,
            canceled: false,
        };
        if continuation {
            if !self.continue_gen_live(continue_epoch) {
                return String::new();
            }
            if self.workspace_remove_blocks_chat(workspace_id) {
                self.uncount_mode_remount(workspace_id, code);
                return String::new();
            }
            // The remount registers after its FIFO claim so a later ticket
            // cannot pin `ended` while an earlier send still waits.
            cleanup.continuation = true;
        }
        let mut wired = wired;
        let text_in = text;
        let (text, imgs): (String, Vec<UserImage>) = if retry {
            let Some(last) = self.last_chat_user_prompt(workspace_id, code) else {
                return "There's nothing to retry — send a new message.".into();
            };
            if let Some(s) = self
                .last_chat_surfaces
                .lock()
                .get(&chat_turn_key(workspace_id, code))
                .filter(|s| !s.surface.trim().is_empty())
            {
                wired = Some(s.clone());
            }
            (last.wire, last.imgs)
        } else {
            let Ok((t, imgs)) = crate::attachments::prepare_chat_attachments(
                &self.data_dir,
                workspace_id,
                &text_in,
                attachments,
            ) else {
                return "I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP."
                    .into();
            };
            if t.is_empty() && imgs.is_empty() {
                return String::new();
            }
            if !continuation {
                self.reset_mode_remounts(workspace_id, code);
                if let Some(p) = self.take_mode_continue(workspace_id, code) {
                    self.drop_continue_gen(p.epoch);
                }
            }
            (t, imgs)
        };
        if mode == CHAT_MODE_AGENT
            && ws.has_run()
            && run_active(&ws.state)
            && (loop_owner_code(&ws) == code || self.chat_drives_live_loop(workspace_id, code))
        {
            // Only the tab driving the loop is locked; siblings share the
            // worktree as sibling Loop tabs already do.
            return "A loop is running in this tab, so Agent-mode edits here wait until it pauses or settles. Other tabs keep working.".into();
        }
        let set = self.settings();
        let (def_model, def_flags) = set.model_for_chat_mode(&mode);
        // A qualified ref, so the turn routes to whichever provider the
        // mode's Settings slot names.
        let model = if model.is_empty() {
            def_model
        } else {
            model.to_owned()
        };
        let flags = match flags {
            Some(f) if !f.is_empty() => f.clone(),
            _ => def_flags,
        };

        // Persist mode + model + flags before the turn runs so a mid-turn
        // reload never reads a stale config.
        self.ensure_pair_agent(workspace_id, code, &model);
        self.remember_pair_chat_selection(workspace_id, code, &model, Some(&flags), &mode, None);

        // The nudge is computed before the provisional title lands (the
        // workspace would already read as named). Retry skips naming.
        let mut nudge = String::new();
        let mut name_from = display_text.to_owned();
        if name_from.is_empty() {
            name_from = text.clone();
        }
        if name_from.is_empty() && !imgs.is_empty() {
            name_from = "image attachment".into();
        }
        if !retry {
            ws = self.drop_early_titles(ws, code, &name_from);
            nudge = self.rename_nudge_prompt(workspace_id, code, &mode, &name_from);
            self.maybe_apply_provisional_workspace_title(&ws, &name_from);
            self.maybe_apply_provisional_chat_title(workspace_id, code, &name_from);
        }

        let surface = self.resolve_chat_surface(&ws, wired.as_ref());
        if !retry {
            self.last_chat_surfaces
                .lock()
                .insert(chat_turn_key(workspace_id, code), surface.clone());
        }
        let subject = self.subject_append_prompt(&surface);

        // Expand the composer (leading prompt action, then the host wrap).
        // Retry replays the already-expanded wire.
        let mut prompt = text.clone();
        let mut display = display_text.to_owned();
        let mut isolate = false;
        if retry {
            display = CHAT_RETRY_DISPLAY_SENTINEL.into();
        } else {
            let expanded = match self
                .expand_composer(
                    &CancellationToken::new(),
                    &text,
                    ExpandOpts {
                        worktree: ws.worktree_path.clone(),
                        workspace_id: workspace_id.to_owned(),
                        ..ExpandOpts::default()
                    },
                )
                .await
            {
                Ok(r) => r,
                // No-send success (e.g. /update finished in git).
                Err(e) if e.is_empty_prompt_action() => return String::new(),
                Err(e) => return format!("I couldn't run that prompt action: {e}"),
            };
            prompt = expanded.prompt;
            if display.is_empty() && (!expanded.display.is_empty() || prompt != text) {
                display = if expanded.display.is_empty() {
                    text.clone()
                } else {
                    expanded.display
                };
            }
            isolate = expanded.fresh_session || fresh_session;
        }
        if continuation || display_text == CHAT_RESUME_DISPLAY {
            isolate = false;
        }

        // Isolated prompt actions start a new provider session on this tab:
        // drop the handle and pending fork inject now, so a pre-init failure
        // cannot retry into the old conversation.
        if isolate {
            self.clear_agent_session(workspace_id, code);
            self.consume_fork_inject(workspace_id, code);
            self.mark_native_history_cutoff(workspace_id, code);
        }
        let mut resume = String::new();
        if !isolate && let Some(a) = self.store.agent(workspace_id, code) {
            resume = a.session_id;
        }

        let nested = !ws.parent_id.trim().is_empty();
        let shape = chat_mode_shape(&mode, &set, nested);
        // The user row id is minted here so the worktree is checkpointed
        // under it before the agent touches anything.
        let user_msg_id = uuid::Uuid::now_v7().to_string();
        let resume_turn = display_text == CHAT_RESUME_DISPLAY;
        let mut req = TurnRequest {
            workspace_id: workspace_id.to_owned(),
            chat_child: nested,
            agent_id: format!("{workspace_id}-{code}"),
            // Coder-style session and tools, but no system prompt: the
            // provider keeps its own.
            role: AgentRole::Coder,
            model: model.clone(),
            flags,
            append_system_prompt: join_append_prompt(
                &join_append_prompt(
                    &shape.append_prompt,
                    &self.simplify_chat_append(workspace_id, code, &mode),
                ),
                &nudge,
            ),
            turn_append_prompt: subject.clone(),
            allowed_tools: Some(shape.tools.clone()),
            prompt: prompt.clone(),
            display_text: if retry {
                String::new()
            } else {
                display.clone()
            },
            skip_record_user: retry || continuation,
            user_message_id: user_msg_id.clone(),
            images: imgs.clone(),
            worktree: ws.worktree_path.clone().into(),
            permission_mode: shape.permission.clone(),
            resume_session: resume.clone(),
            include_partials: true,
            refresh_standing: self.refresh_standing(
                workspace_id,
                code,
                &mode,
                display_text,
                &resume,
            ),
            ..TurnRequest::default()
        };

        // Release the life lock before the FIFO wait: a later send holding
        // it would otherwise deadlock this earlier ticket.
        life.unlock();
        slot.claim(ticket.seq).await;
        ticket.pending = false;
        let (relocked, held) = self.lock_chat_life(workspace_id, code).await;
        life = relocked;
        if !held {
            slot.finish(slot.lock());
            return WORKSPACE_GONE.into();
        }
        // An agent-to-agent notice (hidden display: a subagent report) never
        // steers: injected mid-reasoning it gets skimmed. It runs as its own
        // turn on the same session.
        let notice = display_text.starts_with(DISPLAY_HIDDEN_SENTINEL);
        let mut steered = false;
        if !retry && !resume_turn && !notice && !continuation && !isolate && imgs.is_empty() {
            let ui = if display.is_empty() { &text } else { &display };
            // Claude acks a steer after a tool result; holding the life lock
            // across that wait deadlocks switch_mode (R6).
            life.unlock();
            steered = self
                .try_inject_chat(
                    workspace_id,
                    code,
                    &join_append_prompt(&subject, &prompt),
                    ui,
                )
                .await;
            let (relocked, held) = self.lock_chat_life(workspace_id, code).await;
            life = relocked;
            if !held {
                slot.finish(slot.lock());
                return WORKSPACE_GONE.into();
            }
        }
        {
            let held = slot.lock();
            if steered {
                slot.finish(held);
                return CHAT_INJECTED_SENTINEL.into();
            }
            if slot.stop_gen() != stop_gen {
                slot.finish(held);
                return CHAT_STOPPED_REPLY.into();
            }
        }
        // Wait out a live predecessor before resume/fork so a follow-up that
        // lost the inject still continues that conversation.
        if !resume_turn {
            if let Some(ended) = self.chat_turn_ended(workspace_id, code) {
                life.unlock();
                ended.cancelled().await;
                let (relocked, held) = self.lock_chat_life(workspace_id, code).await;
                life = relocked;
                if !held {
                    slot.finish(slot.lock());
                    return WORKSPACE_GONE.into();
                }
                let held = slot.lock();
                if slot.stop_gen() != stop_gen {
                    slot.finish(held);
                    return CHAT_STOPPED_REPLY.into();
                }
            }
            if !isolate {
                resume = self
                    .store
                    .agent(workspace_id, code)
                    .map(|a| a.session_id)
                    .unwrap_or_default();
                req.resume_session = resume.clone();
                req.refresh_standing =
                    self.refresh_standing(workspace_id, code, &mode, display_text, &resume);
            }
        }
        self.run_async_hook(|h| h.before_apply_chat_fork.clone())
            .await;
        let injected = !isolate && self.apply_chat_fork(workspace_id, code, &mut req, resume_turn);
        let turn = {
            let held = slot.lock();
            if slot.stop_gen() != stop_gen {
                slot.finish(held);
                return CHAT_STOPPED_REPLY.into();
            }
            let turn = if resume_turn || continuation {
                match self.register_chat_turn_if_idle(workspace_id, code, cleanup.cancel.clone()) {
                    Some(t) => t,
                    None => {
                        slot.finish(held);
                        if self.workspace_remove_blocks_chat(workspace_id) {
                            return WORKSPACE_GONE.into();
                        }
                        return String::new();
                    }
                }
            } else {
                match self.register_chat_turn(workspace_id, code, cleanup.cancel.clone()) {
                    Some(t) => t,
                    None => {
                        slot.finish(held);
                        return WORKSPACE_GONE.into();
                    }
                }
            };
            cleanup.turn = Some(Arc::clone(&turn));
            turn.arm(set.agent_turn_timeout());
            let steer = turn.arm_chat_inject();
            slot.finish(held);
            (turn, steer)
        };
        let (turn, steer) = turn;
        if nested {
            self.reopen_chat_child(workspace_id);
        }
        if shape.mcp {
            // Every mode needs the per-agent Recur MCP: for its tools and to
            // route permission prompts. A text-only fallback would let a Plan
            // turn "file" a plan that went nowhere.
            match self
                .register_mcp(workspace_id, code, code, &shape.tools, &req.model)
                .await
            {
                Ok((token, url)) => {
                    cleanup.mcp_token = Some(token);
                    req.mcp_url = url;
                }
                Err(e) => return format!("I can't run right now: {e}"),
            }
        }
        life.unlock();
        let cancel = cleanup.cancel.clone();
        let Some(permit) = self.acquire_agent(&cancel).await else {
            if cancel.is_cancelled() {
                if self.chat_turn_was_parked(workspace_id, code) {
                    return String::new();
                }
                return CHAT_STOPPED_REPLY.into();
            }
            return "I'm at capacity right now — every agent slot is busy. Try again in a moment."
                .into();
        };
        cleanup.permit = Some(permit);

        self.ensure_pair_agent_run(workspace_id, code, &model);
        self.update_agent_status(workspace_id, code, "working");
        cleanup.working = true;

        // Plan-mode chats should file via submit_plan; a provider's own
        // plan proposal is harvested into the Plan panel (summary only).
        let (eng, wsid, c) = (self.arc(), workspace_id.to_owned(), code.to_owned());
        let stream = ChatStream::with(
            self.arc(),
            workspace_id,
            code,
            mode == CHAT_MODE_PLAN,
            Some(Box::new(move |session_id: &str| {
                eng.set_agent_session(&wsid, &c, session_id);
            })),
        );
        // Retry / remounts, and a restart whose last prompt is still an open
        // turn, append to that user row: its start checkpoint exists, only
        // the end moves. Looked up after IfIdle so a send that lands during
        // the prologue cannot have its work folded into a stale pair.
        let mut user_msg_id = user_msg_id;
        let mut resume_open_id = String::new();
        if resume_turn {
            resume_open_id = self.last_open_chat_prompt_id(&ws, code).await;
            if !resume_open_id.is_empty() {
                req.skip_record_user = true;
            }
        }
        if req.skip_record_user {
            user_msg_id = if resume_open_id.is_empty() {
                last_chat_prompt_id(&self.store, workspace_id, code)
            } else {
                resume_open_id
            };
        } else {
            self.take_checkpoint(&ws, &user_msg_id, "start").await;
        }
        let skip_user = req.skip_record_user;
        let spec_user_msg_id = req.user_message_id.clone();
        let stream_emit = stream.emit();
        let t = Arc::clone(&turn);
        let emit: Emit = Arc::new(move |ev: &AgentEvent| {
            if matches!(ev, AgentEvent::TurnEnd(_)) {
                t.stop_chat_inject();
            }
            stream_emit(ev);
        });
        let started = Instant::now();
        let out = self
            .run_agent(
                &cancel,
                AgentSpec {
                    req,
                    steer,
                    emit: Some(emit),
                },
            )
            .await;
        let (res, err): (TurnResult, Option<&recur_agent::ProviderError>) = match &out {
            Ok(r) => (r.clone(), None),
            Err(e) => ((*e.partial).clone(), Some(e)),
        };
        cleanup.canceled = is_canceled(err.map(|e| e as &(dyn std::error::Error + 'static)));
        if out.is_ok() {
            self.remember_standing_mode(workspace_id, code, &mode);
        }
        self.take_checkpoint(&ws, &user_msg_id, "end").await;
        let err_dyn = err.map(|e| e as &(dyn std::error::Error + 'static));
        self.publish_resource();
        if !res.session_id.is_empty() {
            self.set_agent_session(workspace_id, code, &res.session_id);
        }
        self.finish_chat_fork(workspace_id, code, injected, &res);
        // History persists through the session mirror; failed turns also
        // persist a note so a resync does not erase the failure.
        self.finalize_chat_reply(ChatOutcome {
            workspace_id,
            code,
            prompt: &prompt,
            display: &display,
            streamed: &stream.text(),
            res: &res,
            err: err_dyn,
            empty_fallback: "Done.",
            started: Some(started),
            skip_user,
            user_msg_id: &spec_user_msg_id,
        })
    }

    /// Stamps the last transcript message so app-history providers replay
    /// only what follows this fresh_session reset; the visible transcript is
    /// unchanged.
    pub(crate) fn mark_native_history_cutoff(&self, workspace_id: &str, code: &str) {
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return;
        };
        let msgs = self
            .store
            .session_transcript(&sess.id)
            .ok()
            .flatten()
            .unwrap_or_default();
        let Some(last) = msgs.last().map(|m| m.id.trim()).filter(|id| !id.is_empty()) else {
            return;
        };
        let mut m = std::collections::BTreeMap::new();
        m.insert("nativeHistoryAfter", last);
        let Ok(raw) = recur_domain::json::to_string(&m) else {
            return;
        };
        let _ = self.store.patch_session_config_merge(&sess.id, &raw);
    }

    /// Provider standing notes must be re-attached: an agent-initiated
    /// remount, or a human dock change since the last run turn. A missing
    /// standing mode (older sessions) is unknown, not agent.
    pub(crate) fn refresh_standing(
        &self,
        workspace_id: &str,
        code: &str,
        mode: &str,
        display_text: &str,
        resume: &str,
    ) -> bool {
        if is_mode_switch_continue_display(display_text) {
            return true;
        }
        if resume.is_empty() {
            return false;
        }
        self.standing_mode_for(workspace_id, code) != mode
    }

    fn standing_mode_for(&self, workspace_id: &str, code: &str) -> String {
        self.store
            .session_for_loop_agent(workspace_id, &normalize_pair_chat_code(code))
            .map(|s| {
                AgentConfig::parse(&s.config_json)
                    .standing_mode
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default()
    }

    /// Records the mode whose notes the provider session now carries.
    pub(crate) fn remember_standing_mode(&self, workspace_id: &str, code: &str, mode: &str) {
        let code = normalize_pair_chat_code(code);
        let mode = normalize_chat_mode(mode);
        if mode.is_empty() || mode == CHAT_MODE_LOOP {
            return;
        }
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, &code) else {
            return;
        };
        let mut cfg = AgentConfig::parse(&sess.config_json);
        if cfg.standing_mode == mode {
            return;
        }
        cfg.standing_mode = mode.to_owned();
        let Ok(raw) = recur_domain::json::to_string(&cfg) else {
            return;
        };
        let _ = self.store.update_session_config_json(&sess.id, &raw);
    }
}
