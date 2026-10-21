//! Loop engage from a chat tab (Go `workspace_chat.go` Loop-mode send):
//! one chat = one loop. The first Loop-mode message starts this tab's walk
//! with no conductor LLM turn (specialists do the phase work; the planner
//! carries the rename nudge); later messages talk to the conductor.

use recur_agent::UserImage;
use recur_agent::attach::strip_markers;
use recur_domain::{AgentRole, CHAT_MODE_LOOP, ChatAttachment, Workspace, WorkspaceState};

use crate::Engine;
use crate::codes::{MAESTRO_AGENT_CODE, is_pair_chat_code, loop_owner_code};
use crate::loop_registry::LOOP_MAP_SEP;
use crate::session_config::AgentConfig;
use crate::turn::mirror::SessionMirror;

impl Engine {
    /// One Loop-mode send on tab `code` (Go `sendLoopChatMessage`): engages
    /// this tab's walk on the first message, else a conductor turn.
    pub async fn send_loop_chat_message(
        &self,
        ws: Workspace,
        code: &str,
        text: &str,
        attachments: &[ChatAttachment],
        display_text: &str,
    ) -> String {
        let (mut loop_id, mut policy_id) = self.chat_loop_policy(&ws.id, code);
        if loop_id.is_empty() {
            loop_id = ws.loop_id.trim().to_owned();
        }
        if policy_id.is_empty() {
            policy_id = ws.policy_id.trim().to_owned();
        }
        if self.chat_loop_started(&ws.id, code) || (ws.has_run() && loop_owner_code(&ws) == code) {
            return self
                .send_maestro_message_to(&ws.id, code, text, attachments, display_text)
                .await;
        }
        let (mut life, held) = self.lock_chat_life(&ws.id, code).await;
        if !held {
            return "I can't find that workspace anymore — it may have been removed.".into();
        }
        let (text, imgs) = match crate::attachments::prepare_chat_attachments(
            &self.data_dir,
            &ws.id,
            text,
            attachments,
        ) {
            Ok(v) => v,
            Err(_) => {
                return "I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP."
                    .into();
            }
        };
        if text.is_empty() && imgs.is_empty() {
            return String::new();
        }
        // No conductor turn on engage: the provisional label is a
        // placeholder and the planner turn carries the rename nudge.
        let mut name_from = display_text.trim().to_owned();
        let mut goal = strip_markers(&text);
        if goal.is_empty() && !imgs.is_empty() {
            goal = "image attachment".into();
        }
        if name_from.is_empty() {
            name_from = goal.clone();
        }
        if goal.is_empty() {
            goal = name_from.clone();
        }
        if name_from.is_empty() {
            return String::new();
        }
        let ws = self.drop_early_titles(ws, code, &name_from);
        let nudge = self.rename_nudge_prompt(&ws.id, code, CHAT_MODE_LOOP, &name_from);
        if !nudge.is_empty() {
            self.loop_rename_nudge
                .lock()
                .insert(format!("{}{LOOP_MAP_SEP}{code}", ws.id), nudge);
        }
        if !ws.has_run() {
            let on = self.effective_chat_simplify(&ws.id, code);
            self.remember_chat_simplify(&ws.id, code, on);
            self.remember_chat_simplify(&ws.id, MAESTRO_AGENT_CODE, on);
            self.ensure_pair_agent(&ws.id, code, "");
            self.maybe_apply_provisional_workspace_title(&ws, &name_from);
            self.maybe_apply_provisional_chat_title(&ws.id, code, &name_from);
            self.store
                .update_workspace(&ws.id, |w| w.loop_chat_code = code.to_owned());
            self.stamp_chat_loop(
                &ws.id,
                code,
                &loop_id,
                &policy_id,
                &goal,
                display_text,
                false,
            );
            self.stamp_loop_goal_images(&ws.id, code, &imgs);
            self.record_loop_engage_user(&ws.id, code, &text, display_text, &imgs);
            // dispatch locks this tab itself.
            life.unlock();
            if let Err(e) = self
                .start_run_on_workspace_with(&ws.id, &goal, &loop_id, &policy_id)
                .await
            {
                return format!("I couldn't engage the loop: {e}");
            }
            let (_life2, held) = self.lock_chat_life(&ws.id, code).await;
            if !held || self.store.agent(&ws.id, code).is_none() {
                return String::new();
            }
            self.stamp_loop_owner(&ws.id, code, &loop_id, &policy_id, &goal, display_text);
            return String::new();
        }
        // Another chat already engaged this worktree: this tab still gets its
        // own walk from its [[loop]] entry, never a sibling's checkpoint.
        self.stamp_chat_loop(
            &ws.id,
            code,
            &loop_id,
            &policy_id,
            &goal,
            display_text,
            true,
        );
        self.stamp_loop_goal_images(&ws.id, code, &imgs);
        self.maybe_apply_provisional_chat_title(&ws.id, code, &name_from);
        self.record_loop_engage_user(&ws.id, code, &text, display_text, &imgs);
        life.unlock();
        self.start_chat_loop_walk(&ws.id, code, &loop_id, &policy_id, &goal)
            .await;
        String::new()
    }

    /// Stores this tab's `[[loop]]` + `[[policy]]` before engage.
    pub async fn remember_chat_loop_policy(
        &self,
        workspace_id: &str,
        code: &str,
        loop_id: &str,
        policy_id: &str,
    ) {
        if self.store.workspace(workspace_id).is_none() {
            return;
        }
        let mut code = crate::codes::normalize_pair_chat_code(code);
        if code != MAESTRO_AGENT_CODE && !is_pair_chat_code(&code) {
            code = crate::codes::PAIR_AGENT_CODE.to_owned();
        }
        let (_life, held) = self.lock_chat_life(workspace_id, &code).await;
        if !held {
            return;
        }
        if self.store.agent(workspace_id, &code).is_none() {
            if code == MAESTRO_AGENT_CODE {
                self.ensure_maestro_agent(workspace_id, "");
            } else {
                self.ensure_pair_agent(workspace_id, &code, "");
            }
        }
        self.stamp_chat_loop(workspace_id, &code, loop_id, policy_id, "", "", false);
    }

    /// Persists the engage message's user bubble without a conductor turn.
    pub fn record_loop_engage_user(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
        display_text: &str,
        imgs: &[UserImage],
    ) {
        self.ensure_pair_agent(workspace_id, code, "");
        let sess = match self.store.session_for_loop_agent(workspace_id, code) {
            Some(s) => s,
            None => {
                let provider = self.settings().coder_model().provider;
                let Some(s) = self.ensure_agent_session(
                    workspace_id,
                    AgentRole::Coder.as_str(),
                    code,
                    "Agent",
                    &provider,
                ) else {
                    return;
                };
                s
            }
        };
        SessionMirror::new(
            std::sync::Arc::clone(&self.store),
            &sess.id,
            "",
            &sess.provider,
        )
        .record_user("", text, display_text, imgs);
    }

    /// Begins this chat's graph walk: beside a live sibling as a second
    /// walk keyed by chat, or (idle workspace) re-dispatched from the new
    /// `[[loop]]` entry.
    pub(crate) async fn start_chat_loop_walk(
        &self,
        workspace_id: &str,
        code: &str,
        loop_id: &str,
        policy_id: &str,
        goal: &str,
    ) {
        if self.running_loop_by_chat(workspace_id, code).is_some() {
            return;
        }
        let (mut life, held) = self.lock_chat_life(workspace_id, code).await;
        if !held || self.store.agent(workspace_id, code).is_none() {
            return;
        }
        let Some(mut rec) = self.store.workspace(workspace_id) else {
            return;
        };
        if rec.worktree_path.is_empty() || self.creating_in_flight(workspace_id) {
            return;
        }
        if !loop_id.trim().is_empty() {
            rec.loop_id = loop_id.to_owned();
        }
        if !policy_id.trim().is_empty() {
            rec.policy_id = policy_id.to_owned();
        }
        rec.loop_chat_code = code.to_owned();
        let (g, d) = self.chat_loop_goal(workspace_id, code);
        let display = if g == goal.trim() { d } else { String::new() };
        rec.replace_goal(goal, &display);
        if self.has_open_sibling_walk(workspace_id, code) {
            life.unlock();
            self.dispatch_fresh_chat_walk(rec).await;
            return;
        }
        let (lid, pid, g, d) = (
            loop_id.to_owned(),
            policy_id.to_owned(),
            goal.to_owned(),
            display.clone(),
        );
        self.store.update_workspace(workspace_id, |w| {
            if !lid.trim().is_empty() {
                w.loop_id = lid;
            }
            if !pid.trim().is_empty() {
                w.policy_id = pid;
            }
            w.loop_chat_code = code.to_owned();
            w.replace_goal(&g, &d);
            w.checkpoint = None;
            w.iteration = 0;
        });
        if rec.state == WorkspaceState::Merged {
            return;
        }
        life.unlock();
        self.run_start_walk_hook();
        if self.running_loop_by_id(workspace_id).is_some() {
            self.dispatch_fresh_chat_walk(rec).await;
            return;
        }
        self.set_workspace_state(workspace_id, WorkspaceState::Paused, "starting");
        self.resume_with_owner(workspace_id, code).await;
    }

    async fn dispatch_fresh_chat_walk(&self, mut rec: Workspace) {
        if !self.gov.try_acquire_loop() {
            return;
        }
        rec.checkpoint = None;
        rec.iteration = 0;
        if self.dispatch_additional_chat_run(rec).await.is_err() {
            self.gov.release_loop();
            return;
        }
        self.publish_resource();
    }

    /// Another chat's walk still runs on this worktree (not one just
    /// stopped whose agent row is gone).
    pub fn has_open_sibling_walk(&self, workspace_id: &str, code: &str) -> bool {
        let code = code.trim();
        self.running_loops_on(workspace_id).iter().any(|rl| {
            let c = rl.handle.chat_code();
            let c = c.trim();
            !c.is_empty() && c != code && self.store.agent(workspace_id, c).is_some()
        })
    }

    /// Records this tab as the workspace's conductor and stamps its loop.
    pub fn stamp_loop_owner(
        &self,
        workspace_id: &str,
        code: &str,
        loop_id: &str,
        policy_id: &str,
        goal: &str,
        display: &str,
    ) {
        self.store.update_workspace(workspace_id, |w| {
            w.loop_chat_code = code.to_owned();
            if !loop_id.trim().is_empty() {
                w.loop_id = loop_id.to_owned();
            }
            if !policy_id.trim().is_empty() {
                w.policy_id = policy_id.to_owned();
            }
        });
        let (mut goal, mut display) = (goal.to_owned(), display.to_owned());
        if goal.trim().is_empty()
            && let Some(rec) = self.store.workspace(workspace_id)
        {
            goal = rec.goal;
            display = rec.goal_display;
        }
        self.stamp_chat_loop(
            workspace_id,
            code,
            loop_id,
            policy_id,
            &goal,
            &display,
            true,
        );
    }

    /// Records the opening paste set on this tab's loop session.
    pub fn stamp_loop_goal_images(&self, workspace_id: &str, code: &str, imgs: &[UserImage]) {
        if code != MAESTRO_AGENT_CODE && !is_pair_chat_code(code) {
            return;
        }
        let ids: Vec<String> = imgs
            .iter()
            .map(|i| i.id.trim().to_owned())
            .filter(|id| !id.is_empty())
            .collect();
        if code == MAESTRO_AGENT_CODE {
            self.ensure_maestro_agent_with_status(workspace_id, "", "idle");
        } else {
            self.ensure_pair_agent(workspace_id, code, "");
        }
        let (kind, name, provider) = self.chat_session_meta(workspace_id, code);
        let Some(sess) = self.ensure_agent_session(workspace_id, &kind, code, &name, &provider)
        else {
            return;
        };
        let mut cfg = AgentConfig::parse(&sess.config_json);
        cfg.goal_images = Some(ids);
        if let Ok(raw) = recur_domain::json::to_string(&cfg) {
            let _ = self.store.update_session_config_json(&sess.id, &raw);
        }
    }
}
