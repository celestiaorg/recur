//! The Maestro: a loop's conductor chat (Go `maestro.go`, PRD §8.5). One
//! chat turn runs on the caller's task, never the walk's, with the live loop
//! context in its system prompt; a steer it files is applied within the rails
//! and queued onto the running walk. Multi-turn continuity resumes the
//! conductor's provider session.

use std::time::{Duration, Instant};

use recur_agent::TurnRequest;
use recur_domain::{
    AgentRole, AgentStep, CHAT_MODE_LOOP, ChatAttachment, ChatItem, Workspace, WorkspaceState,
};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::codes::{
    CHAT_RETRY_DISPLAY_SENTINEL, MAESTRO_AGENT_CODE, is_chat_retry_display, is_pair_chat_code,
};
use crate::composer::ExpandOpts;
use crate::loops::open_pr::format_pr_thread;
use crate::roles::{MaestroContext, maestro_system_prompt, maestro_tools};
use crate::text::{html_escape, truncate};
use crate::turn::AgentSpec;
use crate::turn::finalize::{CHAT_STOPPED_REPLY, ChatOutcome, is_canceled};
use crate::turn::stream::ChatStream;

/// Bounds one Maestro chat turn so a wedged provider never blocks the call.
pub const MAESTRO_CHAT_TIMEOUT: Duration = Duration::from_secs(120);

const LOOP_GONE: &str = "I can't find that loop anymore — it may have been removed.";

/// Removes simple HTML tags from a feed line for a plain-text prompt.
pub fn strip_tags(s: &str) -> String {
    let mut b = String::with_capacity(s.len());
    let mut depth = 0usize;
    for r in s.chars() {
        match r {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => b.push(r),
            _ => {}
        }
    }
    b
}

impl Engine {
    /// One Maestro chat turn on the workspace's MA tab; returns the reply.
    pub async fn send_maestro_message(
        &self,
        workspace_id: &str,
        text: &str,
        attachments: &[ChatAttachment],
    ) -> String {
        self.send_maestro_message_to(workspace_id, MAESTRO_AGENT_CODE, text, attachments, "")
            .await
    }

    /// One conductor turn on tab `code` (Go `sendMaestroMessage`). A turn
    /// that produces no reply returns an honest fallback, never an error.
    pub async fn send_maestro_message_to(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
        attachments: &[ChatAttachment],
        display_text: &str,
    ) -> String {
        let code = if code.trim().is_empty() {
            MAESTRO_AGENT_CODE
        } else {
            code
        };
        let (mut life, held) = self.lock_chat_life(workspace_id, code).await;
        if !held {
            return LOOP_GONE.into();
        }
        let retry = is_chat_retry_display(display_text);
        let (text, imgs) = if retry {
            let Some(last) = self.last_chat_user_prompt(workspace_id, code) else {
                return "There's nothing to retry — send a new message.".into();
            };
            (last.wire, last.imgs)
        } else {
            match crate::attachments::prepare_chat_attachments(
                &self.data_dir,
                workspace_id,
                text,
                attachments,
            ) {
                Ok(v) => v,
                Err(_) => {
                    return "I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP.".into();
                }
            }
        };
        if !retry && text.is_empty() && imgs.is_empty() {
            return String::new();
        }
        let Some(mut lp) = self.store.workspace(workspace_id) else {
            return LOOP_GONE.into();
        };

        let mut nudge = String::new();
        let mut name_from = display_text.to_owned();
        if name_from.is_empty() {
            name_from = text.clone();
        }
        if name_from.is_empty() && !imgs.is_empty() {
            name_from = "image attachment".into();
        }
        if !retry {
            lp = self.drop_early_titles(lp, code, &name_from);
            nudge = self.rename_nudge_prompt(workspace_id, code, CHAT_MODE_LOOP, &name_from);
            self.maybe_apply_provisional_workspace_title(&lp, &name_from);
            if is_pair_chat_code(code) {
                self.ensure_pair_agent(workspace_id, code, "");
            } else {
                self.ensure_maestro_agent(workspace_id, lp.goal_label());
            }
            self.remember_pair_chat_selection(workspace_id, code, "", None, CHAT_MODE_LOOP, None);
            self.maybe_apply_provisional_chat_title(workspace_id, code, &name_from);
        }

        let resume = self
            .store
            .agent(workspace_id, code)
            .map(|a| a.session_id)
            .unwrap_or_default();
        let set = self.settings();
        let mut persist_display = display_text.to_owned();
        let mut spec_display = display_text.to_owned();
        let mut prompt = text.clone();
        if retry {
            persist_display = CHAT_RETRY_DISPLAY_SENTINEL.into();
            spec_display.clear();
        } else {
            let raw = text.clone();
            let expanded = match self
                .expand_composer(
                    &CancellationToken::new(),
                    &text,
                    ExpandOpts {
                        worktree: lp.worktree_path.clone(),
                        workspace_id: workspace_id.to_owned(),
                        ..ExpandOpts::default()
                    },
                )
                .await
            {
                Ok(r) => r,
                Err(e) if e.is_empty_prompt_action() => return String::new(),
                Err(e) => return format!("I couldn't run that prompt action: {e}"),
            };
            prompt = expanded.prompt;
            if persist_display.is_empty() && (!expanded.display.is_empty() || prompt != raw) {
                persist_display = if expanded.display.is_empty() {
                    raw
                } else {
                    expanded.display
                };
                spec_display = persist_display.clone();
            }
        }
        let sys = maestro_system_prompt(&self.maestro_context_for_chat(&lp, code));
        let model = set.maestro_model().to_string();
        let allowed = maestro_tools();
        let req = TurnRequest {
            workspace_id: workspace_id.to_owned(),
            chat_code: code.to_owned(),
            agent_id: format!("{workspace_id}-{code}"),
            role: AgentRole::Maestro,
            model: model.clone(),
            flags: set.maestro_flags(),
            system_prompt: sys.clone(),
            append_system_prompt: nudge,
            allowed_tools: Some(allowed.clone()),
            prompt: prompt.clone(),
            display_text: spec_display,
            skip_record_user: retry,
            images: imgs,
            worktree: lp.worktree_path.clone().into(),
            permission_mode: set.permission_mode().into(),
            resume_session: resume.clone(),
            include_partials: true,
            refresh_standing: self.standing_prompt_changed(workspace_id, code, &sys, &resume),
            ..TurnRequest::default()
        };
        // Without the MCP the Maestro could not steer, escalate or file a
        // plan: every action it claimed would be a no-op. Say so.
        let (token, mcp_url) = match self
            .register_mcp(workspace_id, code, code, &allowed, &model)
            .await
        {
            Ok(t) => t,
            Err(e) => return format!("I can't run right now: {e}"),
        };
        let mut req = req;
        req.mcp_url = mcp_url;

        // Cancel-only token: the chat turn owns the wedged-turn deadline so
        // a parked ask_user can hold it.
        let cancel = CancellationToken::new();
        let Some(turn) = self.register_chat_turn(workspace_id, code, cancel.clone()) else {
            self.unregister_mcp(&token);
            return LOOP_GONE.into();
        };
        life.unlock();
        turn.arm(MAESTRO_CHAT_TIMEOUT);
        let finish = |e: &Engine| {
            e.unregister_chat_turn(workspace_id, code, &turn);
            e.unregister_mcp(&token);
        };
        let Some(permit) = self.acquire_agent(&cancel).await else {
            let reply = if cancel.is_cancelled() {
                if self.chat_turn_was_parked(workspace_id, code) {
                    String::new()
                } else {
                    CHAT_STOPPED_REPLY.to_owned()
                }
            } else {
                "I'm at capacity right now — every agent slot is busy. Try again in a moment."
                    .to_owned()
            };
            finish(self);
            return reply;
        };

        self.update_agent_status(workspace_id, code, "directing");
        let (eng, ws, c, t) = (
            self.arc(),
            workspace_id.to_owned(),
            code.to_owned(),
            truncate(&text, 80),
        );
        let stream = ChatStream::with(
            self.arc(),
            workspace_id,
            code,
            false,
            Some(Box::new(move |session_id: &str| {
                eng.set_agent_session(&ws, &c, session_id);
                eng.append_agent_step(
                    &ws,
                    &c,
                    AgentStep {
                        kind: "note".into(),
                        text: format!("chat: {t}"),
                        ..AgentStep::default()
                    },
                );
            })),
        );
        let started = Instant::now();
        let mut spec = AgentSpec::new(req);
        spec.emit = Some(stream.emit());
        let out = self.run_agent(&cancel, spec).await;
        if out.is_ok() {
            self.remember_standing_prompt(workspace_id, code, &sys);
        }
        self.publish_resource();
        let (res, err) = match &out {
            Ok(r) => (r.clone(), None),
            Err(e) => ((*e.partial).clone(), Some(e)),
        };
        if !res.session_id.is_empty() {
            self.set_agent_session(workspace_id, code, &res.session_id);
        }
        let err_dyn = err.map(|e| e as &(dyn std::error::Error + 'static));
        let reply = self.finalize_chat_reply(ChatOutcome {
            workspace_id,
            code,
            prompt: &prompt,
            display: &persist_display,
            streamed: &stream.text(),
            res: &res,
            err: err_dyn,
            empty_fallback: "On it — I'll keep working within your rails and keep you posted.",
            started: Some(started),
            skip_user: retry,
            user_msg_id: "",
        });
        if !(self.is_shutting_down() && is_canceled(err_dyn)) {
            self.update_agent_status(workspace_id, code, "idle");
        }
        drop(permit);
        finish(self);
        reply
    }

    /// The loop's Maestro chat transcript (MA's session).
    pub async fn maestro_chat_items(&self, workspace_id: &str) -> Vec<ChatItem> {
        self.session_chat_items(workspace_id, MAESTRO_AGENT_CODE)
            .await
    }

    /// Applies a Maestro re-plan from chat: a plan-change deviation on the
    /// feed, queued onto the running walk (new agents get it in their
    /// prompt; the in-flight one as a steer). Rails are enforced where
    /// actions execute, never by matching the sentence.
    pub fn apply_maestro_steer(&self, workspace_id: &str, agent_code: &str, summary: &str) {
        let summary = summary.trim();
        if summary.is_empty() {
            return;
        }
        self.emit_deviation(
            workspace_id,
            &format!(
                "Re-planned from your chat: {}",
                html_escape(&truncate(summary, 200))
            ),
            "default = the original plan",
            "you steered the loop via the Maestro chat (rails still hold at execution).",
        );
        if let Some(lp) = self.running_loop_for_hand(workspace_id, agent_code) {
            lp.queue_steer(summary);
        }
    }

    /// The persisted Maestro session id ("" before the first message).
    pub fn maestro_session(&self, workspace_id: &str) -> String {
        self.store
            .agent(workspace_id, MAESTRO_AGENT_CODE)
            .map(|a| a.session_id)
            .unwrap_or_default()
    }

    /// The live loop context seeded into a Maestro turn.
    pub fn maestro_context(&self, lp: &Workspace) -> MaestroContext {
        self.maestro_context_for_chat(lp, "")
    }

    pub fn maestro_context_for_chat(&self, lp: &Workspace, chat_code: &str) -> MaestroContext {
        let recent = self.recent_activity(&lp.id, 5);
        let walk = self.running_loop_for_hand(&lp.id, chat_code);
        let mut ctx = MaestroContext {
            goal: lp.goal.clone(),
            mission: self.loop_mission(lp, chat_code),
            state: lp.state.as_str().to_owned(),
            live: walk.is_some(),
            iteration: lp.iteration,
            recent,
            handoff: lp.handoff.clone(),
            ..MaestroContext::default()
        };
        let (g, _) = self.chat_loop_goal(&lp.id, chat_code);
        if !g.is_empty() {
            ctx.goal = g;
        }
        if let Some(h) = walk {
            let s = h.lock();
            if !s.walk_state.is_empty() {
                ctx.state = s.walk_state.as_str().to_owned();
            } else if s.escalated {
                ctx.state = WorkspaceState::Escalated.as_str().to_owned();
            } else if s.paused {
                ctx.state = WorkspaceState::Paused.as_str().to_owned();
            } else if ctx.state == WorkspaceState::Escalated.as_str()
                || ctx.state == WorkspaceState::Paused.as_str()
            {
                ctx.state = WorkspaceState::Coding.as_str().to_owned();
            }
            ctx.iteration = s.walk_iter;
            let g = s.goal.trim();
            if !g.is_empty() {
                ctx.goal = g.to_owned();
            }
        }
        if let Some(pr) = self.store.pr_by_workspace(&lp.id) {
            ctx.pr_thread = format_pr_thread(&pr);
            if !pr.impact.is_empty() {
                let (mut sym, mut api, mut test) = (0, 0, 0);
                for it in &pr.impact {
                    match it.kind.as_str() {
                        "symbol" => sym += 1,
                        "api" => api += 1,
                        "test" => test += 1,
                        _ => {}
                    }
                }
                ctx.impact =
                    format!("{sym} defs · {api} refs · {test} tests (from last PR analysis)");
            }
        }
        if ctx.impact.is_empty()
            && let Some(a) = self.store.latest_analysis(&lp.project_id, &lp.id)
            && !a.impact.is_empty()
        {
            ctx.impact = a.summary;
        }
        ctx
    }

    /// The `[[loop]]` mission: the conductor's standing instructions.
    pub fn loop_mission(&self, ws: &Workspace, chat_code: &str) -> String {
        let (mut loop_id, mut policy_id) = if chat_code.is_empty() {
            (String::new(), String::new())
        } else {
            self.chat_loop_policy(&ws.id, chat_code)
        };
        if loop_id.is_empty() {
            loop_id = ws.loop_id.clone();
        }
        if policy_id.is_empty() {
            policy_id = ws.policy_id.clone();
        }
        let mut scoped = ws.clone();
        scoped.loop_id = loop_id;
        scoped.policy_id = policy_id;
        let g = self.guard_for_workspace(&scoped).unwrap_or_default();
        g.loop_def
            .prompts
            .get("mission")
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    }

    /// Up to `n` recent feed lines, newest first, tags stripped; the durable
    /// feed is reconstructed when the live one is empty.
    pub fn recent_activity(&self, workspace_id: &str, n: usize) -> Vec<String> {
        let mut feed = self.store.feed(workspace_id).unwrap_or_default();
        if feed.is_empty() {
            feed = self.reconstruct_feed(workspace_id);
        }
        let mut out = Vec::with_capacity(n);
        for it in feed {
            if out.len() >= n {
                break;
            }
            let line = strip_tags(&it.text).trim().to_owned();
            if line.is_empty() {
                continue;
            }
            let name = if it.name.is_empty() { "•" } else { &it.name };
            out.push(format!("{name}: {}", truncate(&line, 120)));
        }
        out
    }
}
