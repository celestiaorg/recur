//! Restart healing of chats (Go `chat_recover.go`): agents persisted live
//! with no turn are idled, their streaming rows closed and unfinished tool
//! calls answered; interrupted PA*/MA tabs are queued and AutoResume
//! continues them; a zombie live tab is healed by Stop (R14).

use std::collections::HashSet;

use recur_domain::{
    Agent, CHAT_MODE_LOOP, ContentBlock, Message, STREAM_COMPLETE, STREAM_ERROR, STREAM_PENDING,
    STREAM_STREAMING, Workspace, normalize_chat_mode, run_active,
};

use super::children::is_live_agent_status;
use crate::Engine;
use crate::bus::{AgentUpdated, EngineEvent};
use crate::codes::{
    MAESTRO_AGENT_CODE, chat_turn_key, is_chat_attention_code, is_chat_child, is_pair_chat_code,
    normalize_pair_chat_code,
};
use crate::loops::lifecycle::resumable;
use crate::turn::finalize::{
    CHAT_INTERRUPTED_REPLY, CHAT_RESUME_DISPLAY, CHAT_RESUME_PROMPT, CHAT_STOPPED_REPLY,
};

/// One coding or Maestro tab Recover found live with no turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedChat {
    pub workspace_id: String,
    pub code: String,
}

/// A Maestro continuation could steer a run that has no walk to receive it
/// (in flight, queued, or parked-but-resumable): the steer would print
/// "Re-planned" and never land. Settled loops are conversational only.
pub fn maestro_resume_needs_loop(ws: &Workspace) -> bool {
    run_active(&ws.state) || resumable(ws)
}

impl Engine {
    /// Idles every agent persisted live with no in-memory turn or walk,
    /// closes leftover streaming rows, and queues PA*/MA chats for
    /// AutoResume. Call from Recover at startup, while nothing is live.
    pub(crate) fn recover_interrupted_chats(&self) {
        let mut queued = Vec::new();
        for ws in self.store.workspaces() {
            if ws.archived {
                continue;
            }
            for a in self.store.agents(&ws.id) {
                if !is_live_agent_status(&a.status) {
                    continue;
                }
                if self.live_chat_turn(&ws.id, &a.code).is_some() {
                    continue;
                }
                if self.loop_key_running(&ws.id) && !is_chat_attention_code(&a.code) {
                    continue;
                }
                let resume = self.should_auto_resume_chat(&ws, &a);
                let note = if is_chat_attention_code(&a.code) {
                    CHAT_INTERRUPTED_REPLY
                } else {
                    ""
                };
                self.heal_interrupted_chat(&ws.id, &a.code, note);
                if resume {
                    queued.push(InterruptedChat {
                        workspace_id: ws.id.clone(),
                        code: a.code.clone(),
                    });
                }
            }
            if ws.scheduled_prompt_pending {
                self.store
                    .update_workspace(&ws.id, |w| w.scheduled_prompt_pending = false);
            }
        }
        *self.interrupted_chats.lock() = queued;
        for sess in self.store.streaming_app_sessions().unwrap_or_default() {
            let _ = self.store.set_session_status(&sess.id, "idle");
        }
    }

    /// A human-facing tab that was mid-turn (PA* working, MA directing)
    /// whose worktree still exists. A scheduled run's interrupted first turn
    /// is not replayed; a subagent's is (its parent waits for a report). A
    /// missing path must not spawn a turn that fails on cwd.
    fn should_auto_resume_chat(&self, ws: &Workspace, a: &Agent) -> bool {
        if ws.scheduled_prompt_pending && !is_chat_child(ws) {
            return false;
        }
        let path = ws.worktree_path.trim();
        if path.is_empty() || !std::path::Path::new(path).is_dir() {
            return false;
        }
        if !is_chat_attention_code(&a.code) {
            return false;
        }
        if a.code == MAESTRO_AGENT_CODE {
            a.status == "directing"
        } else {
            a.status == "working"
        }
    }

    /// Returns a zombie live agent to idle: streaming rows closed, error
    /// results on unfinished tool calls, `note` in the transcript.
    pub(crate) fn heal_interrupted_chat(&self, workspace_id: &str, code: &str, note: &str) {
        if workspace_id.is_empty() || code.is_empty() {
            return;
        }
        self.close_interrupted_chat_session(workspace_id, code, note);
        self.idle_agent_no_unseen(workspace_id, code);
    }

    /// status=idle and `agent.updated`, without the unseen badge (a heal is
    /// not a reply that landed while the human looked away).
    fn idle_agent_no_unseen(&self, workspace_id: &str, code: &str) {
        self.store
            .update_agent(workspace_id, code, |a| a.status = "idle".into());
        self.bus.publish(EngineEvent::AgentUpdated(AgentUpdated {
            agent_id: format!("{workspace_id}-{code}"),
            code: None,
            status: "idle".into(),
            surface: None,
            workspace_id: workspace_id.to_owned(),
        }));
    }

    /// Finalizes leftover streaming rows and pending tool_use blocks so a
    /// reload does not paint the turn as in flight.
    fn close_interrupted_chat_session(&self, workspace_id: &str, code: &str, note: &str) {
        let persist_note = |e: &Engine| {
            if !note.trim().is_empty() {
                e.persist_chat_failure(
                    workspace_id,
                    code,
                    "",
                    "",
                    note,
                    0,
                    &recur_agent::TurnResult::default(),
                    false,
                    "",
                );
            }
        };
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            persist_note(self);
            return;
        };
        let msgs = match self.store.session_transcript(&sess.id) {
            Ok(m) => m.unwrap_or_default(),
            Err(_) => {
                let _ = self.store.set_session_status(&sess.id, "idle");
                return;
            }
        };
        let mut pending: HashSet<String> = HashSet::new();
        let mut order: Vec<String> = Vec::new();
        for m in &msgs {
            if m.stream_status == STREAM_STREAMING || m.stream_status == STREAM_PENDING {
                let _ = self
                    .store
                    .complete_message(&m.id, STREAM_ERROR, &Message::default());
            }
            for b in m.blocks() {
                if let Some(tu) = b.as_tool_use()
                    && !tu.id.is_empty()
                {
                    if !pending.contains(&tu.id) {
                        order.push(tu.id.clone());
                    }
                    pending.insert(tu.id);
                }
                if let Some(tr) = b.as_tool_result() {
                    pending.remove(&tr.tool_use_id);
                }
            }
        }
        let result_note = match note.trim() {
            "" => CHAT_INTERRUPTED_REPLY,
            n => n,
        };
        let blocks: Vec<ContentBlock> = order
            .iter()
            .filter(|id| pending.contains(*id))
            .map(|id| ContentBlock::tool_result(id, result_note, true))
            .collect();
        if !blocks.is_empty() {
            let _ = self.store.append_message(
                &sess.id,
                Message {
                    role: "user".into(),
                    stream_status: STREAM_COMPLETE.into(),
                    content: Some(blocks),
                    ..Message::default()
                },
            );
        }
        let _ = self.store.set_session_status(&sess.id, "idle");
        persist_note(self);
    }

    /// Idles a tab the UI thinks is live but that has no registered turn
    /// (Recover missed it, or the UI seeded busy from a stale row). True
    /// when it changed something.
    pub(crate) async fn heal_zombie_chat(&self, workspace_id: &str, code: &str) -> bool {
        let mut code = code.trim().to_owned();
        if is_pair_chat_code(&code) {
            code = normalize_pair_chat_code(&code);
        }
        if !is_chat_attention_code(&code) {
            return false;
        }
        let Some(a) = self.store.agent(workspace_id, &code) else {
            return false;
        };
        if !is_live_agent_status(&a.status) || self.live_chat_turn(workspace_id, &code).is_some() {
            return false;
        }
        if code == MAESTRO_AGENT_CODE && self.loop_key_running(workspace_id) {
            return false;
        }
        self.heal_interrupted_chat(workspace_id, &code, CHAT_STOPPED_REPLY);
        true
    }

    /// Continues queued tabs, each on its own task so a slow turn cannot
    /// hold up loop AutoResume or the window.
    fn launch_interrupted_chats(&self, chats: Vec<InterruptedChat>) {
        for c in chats {
            tracing::info!(
                "recur: auto-resuming chat {} {} after restart",
                c.workspace_id,
                c.code
            );
            self.resuming_chats
                .lock()
                .insert(chat_turn_key(&c.workspace_id, &c.code));
            let eng = self.arc();
            tokio::spawn(async move {
                eng.resume_interrupted_chat(&c.workspace_id, &c.code).await;
            });
        }
    }

    /// A tab of the workspace that AutoResume launched and whose resumed
    /// turn has not finished.
    pub(crate) fn chat_resuming(&self, workspace_id: &str) -> bool {
        let prefix = chat_turn_key(workspace_id, "");
        self.resuming_chats
            .lock()
            .iter()
            .any(|k| k.starts_with(&prefix))
    }

    /// Launches the queued chats except those on workspaces whose loop
    /// AutoResume resumes first.
    pub(crate) async fn launch_interrupted_chats_except(&self, hold: &HashSet<String>) {
        let now = {
            let mut q = self.interrupted_chats.lock();
            let (held, now): (Vec<_>, Vec<_>) = std::mem::take(&mut *q)
                .into_iter()
                .partition(|c| hold.contains(&c.workspace_id));
            *q = held;
            now
        };
        self.launch_interrupted_chats(now);
    }

    /// A workspace's held chats, right after its loop's Resume returned.
    pub(crate) async fn launch_interrupted_chats_for(&self, workspace_id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        let now = {
            let mut q = self.interrupted_chats.lock();
            let (now, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *q)
                .into_iter()
                .partition(|c| c.workspace_id == workspace_id);
            *q = rest;
            now
        };
        self.launch_interrupted_chats(now);
    }

    /// Whatever is still queued after the loop pass.
    pub(crate) async fn launch_remaining_interrupted_chats(&self) {
        let rest = std::mem::take(&mut *self.interrupted_chats.lock());
        self.launch_interrupted_chats(rest);
    }

    async fn resume_interrupted_chat(&self, workspace_id: &str, code: &str) {
        self.resume_interrupted_chat_inner(workspace_id, code).await;
        self.resuming_chats
            .lock()
            .remove(&chat_turn_key(workspace_id, code));
    }

    async fn resume_interrupted_chat_inner(&self, workspace_id: &str, code: &str) {
        if !self.chat_resume_still_idle(workspace_id, code) {
            return;
        }
        if self.store.workspace(workspace_id).is_none() {
            return;
        }
        let (model, flags, mode) = self.workspace_chat_selection(workspace_id, code);
        if code == MAESTRO_AGENT_CODE || normalize_chat_mode(&mode) == CHAT_MODE_LOOP {
            if !self.loop_key_running(workspace_id) {
                match self.store.workspace(workspace_id) {
                    Some(ws) if !maestro_resume_needs_loop(&ws) => {}
                    _ => return,
                }
            }
            self.send_maestro_message_to(
                workspace_id,
                code,
                CHAT_RESUME_PROMPT,
                &[],
                CHAT_RESUME_DISPLAY,
            )
            .await;
            return;
        }
        // The tab's own dock selection: an empty one would fall back to the
        // Settings default while the stored handle belongs to another
        // provider.
        if !self.chat_resume_still_idle(workspace_id, code) {
            return;
        }
        self.send_workspace_message(
            workspace_id,
            code,
            CHAT_RESUME_PROMPT,
            &model,
            Some(&flags),
            &[],
            &mode,
            CHAT_RESUME_DISPLAY,
        )
        .await;
    }

    /// Recover's queued tab is still waiting: the agent exists, is idle,
    /// and nothing registered a turn (a user send or close in the window
    /// after Recover wins).
    fn chat_resume_still_idle(&self, workspace_id: &str, code: &str) -> bool {
        if self.live_chat_turn(workspace_id, code).is_some() {
            return false;
        }
        self.store
            .agent(workspace_id, code)
            .is_some_and(|a| a.status == "idle")
    }
}
