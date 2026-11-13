//! The Maestro's wrap-up of a settled walk: one bounded, tool-less turn over
//! the whole run (task, plan, every handoff and done summary, commits, recent
//! activity) whose prose lands under the "Completed" fact (or is patched onto
//! the PR-opened fact).

use std::sync::Arc;
use std::time::Duration;

use recur_agent::TurnRequest;
use recur_domain::{PERMISSION_MODE_DENY, WorkspaceState, parse_model_ref};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::attention::chat_inbox::loop_settled;
use crate::bus::{EngineEvent, WorkspaceEvent};
use crate::codes::MAESTRO_AGENT_CODE;
use crate::loop_handle::LoopHandle;
use crate::loops::WrapUpInputs;
use crate::loops::maestro::strip_tags;
use crate::text::{html_escape, truncate};
use crate::turn::AgentSpec;

pub const WRAP_UP_TIMEOUT: Duration = Duration::from_secs(60);
pub const WRAP_UP_ACTIVITY_N: usize = 24;
pub const WRAP_UP_BODY_MAX: usize = 1200;
pub const WRAP_UP_LINE_MAX: usize = 160;

pub const WRAP_UP_SYSTEM_PROMPT: &str = "You are the Maestro wrapping up ONE finished workflow for the operator. You see the whole run: the task, the plan (every step / child), every implementer handoff, and recent activity. Do not describe a single Coder turn as if it were the workflow. Reply with ONLY the wrap-up, plain text, 3–8 short lines: root cause if this was a bug or ticket, what landed across the whole run, and anything unfinished or notable. No greeting, no markdown fences, no tools.";

/// The compact wall-clock label for a finished loop.
pub fn format_loop_elapsed(d: Duration) -> String {
    // Round to the second, halves away from zero.
    let sec = ((d.as_nanos() + 500_000_000) / 1_000_000_000) as u64;
    if sec < 60 {
        return format!("{sec}s");
    }
    let mut m = sec / 60;
    let s = sec % 60;
    if m >= 60 {
        let h = m / 60;
        m %= 60;
        if m == 0 {
            return format!("{h}h");
        }
        return format!("{h}h {m}m");
    }
    if s == 0 {
        return format!("{m}m");
    }
    format!("{m}m {s:02}s")
}

/// A completion fact the wrap-up attaches to.
pub fn is_completion_fact(text: &str) -> bool {
    if text.contains("loop-pr-link") {
        return true;
    }
    strip_tags(text).trim().starts_with("Completed")
}

/// The cockpit body under "Completed": escaped model prose.
pub fn loop_wrap_up_html(prose: &str) -> String {
    let s = sanitize_wrap_prose(prose);
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    let s = html_escape(&truncate(s, WRAP_UP_BODY_MAX)).replace('\n', "<br>");
    format!(r#"<div class="loop-wrap">{s}</div>"#)
}

/// Strips a markdown fence and wrapping quotes from a model reply.
pub fn sanitize_wrap_prose(raw: &str) -> String {
    let mut s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    let owned;
    if let Some(rest) = s.strip_prefix("```") {
        let mut t = rest;
        if let Some(i) = t.find('\n') {
            t = &t[i + 1..];
        }
        if let Some(j) = t.rfind("```") {
            t = &t[..j];
        }
        owned = t.trim().to_owned();
        s = &owned;
    }
    s.trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_owned()
}

impl Engine {
    /// The Maestro's wrap-up for a walk that settled (not stopped, not idled by
    /// composer Stop). Children skip it: the parent covers the whole fan-out. A
    /// PR-open walk already posted its Completed fact, so the wrap is patched
    /// onto it.
    pub(crate) async fn emit_settled_wrap_up(&self, h: &Arc<LoopHandle>, w: &WrapUpInputs) {
        if h.wants_idle() || h.is_stopped() {
            return;
        }
        let (state, phase) = h.exit();
        if !loop_settled(&state, &phase) || !w.parent_id.is_empty() {
            return;
        }
        let wrap = self.maestro_wrap_html(h, w).await;
        if state == WorkspaceState::PrOpen {
            if !wrap.is_empty() {
                self.attach_loop_wrap(&w.id, &wrap);
            }
            return;
        }
        let elapsed = w.started_at.map(|t| t.elapsed()).unwrap_or_default();
        let line = if elapsed.is_zero() {
            "Completed.".to_owned()
        } else {
            format!("Completed in {}.", format_loop_elapsed(elapsed))
        };
        self.emit_workspace_event_item(
            &w.id,
            recur_domain::FeedItem {
                agent_code: "MA".into(),
                name: "Maestro".into(),
                text: format!("{line}{wrap}"),
                chat_code: h.chat_code(),
                ..recur_domain::FeedItem::default()
            },
        );
    }

    async fn maestro_wrap_html(&self, h: &Arc<LoopHandle>, w: &WrapUpInputs) -> String {
        let cancel = CancellationToken::new();
        let set = self.settings();
        let r = set.maestro_model();
        let model = r.to_string();
        let provider = self.providers.for_model(&model);
        let mut flags = set.maestro_flags();
        if flags.is_empty()
            && let Some(p) = &provider
        {
            flags = p.oneshot_flags(&parse_model_ref(&model).id);
        }
        let mut req = TurnRequest {
            workspace_id: w.id.clone(),
            model,
            flags,
            system_prompt: WRAP_UP_SYSTEM_PROMPT.into(),
            prompt: self.wrap_up_prompt(h, w),
            worktree: w.worktree.clone().into(),
            permission_mode: PERMISSION_MODE_DENY.into(),
            tools: Some(String::new()),
            max_turns: 1,
            skip_thinking: true,
            ..TurnRequest::default()
        };
        if provider.is_some_and(|p| !p.caps().can_disable_tools) {
            req.allowed_tools = Some(vec!["Read".into(), "Glob".into(), "Grep".into()]);
        }
        self.update_agent_status(&w.id, MAESTRO_AGENT_CODE, "summarizing");
        let turn = cancel.child_token();
        let timer = {
            let t = turn.clone();
            tokio::spawn(async move {
                tokio::time::sleep(WRAP_UP_TIMEOUT).await;
                t.cancel();
            })
        };
        let out = self.run_agent(&turn, AgentSpec::new(req)).await;
        timer.abort();
        self.publish_resource();
        self.update_agent_status(&w.id, MAESTRO_AGENT_CODE, "idle");
        match out {
            Ok(res) if !res.is_error => loop_wrap_up_html(&res.text),
            _ => String::new(),
        }
    }

    /// The wrap-up prompt: the whole run, not one Coder turn.
    pub(crate) fn wrap_up_prompt(&self, h: &LoopHandle, w: &WrapUpInputs) -> String {
        let mut b = String::from("Task:\n");
        b.push_str(w.goal.trim());
        b.push('\n');
        if let Some(ws) = self.store.workspace(&w.id)
            && let Some(p) = &ws.plan
            && !p.steps.is_empty()
        {
            b.push_str("\nPlan:\n");
            b.push_str(&p.board());
            b.push('\n');
        }
        let handoff = w.review_handoff.trim();
        if !handoff.is_empty() {
            b.push_str("\nHandoffs (every landing, including children):\n");
            b.push_str(handoff);
            b.push('\n');
        }
        let done = h.wrap_up_done();
        if !done.is_empty() {
            b.push_str("\nImplementer done() notes (every turn, not one Coder):\n");
            b.push_str(&done);
            b.push('\n');
        }
        if let Some(pr) = self.store.pr_by_workspace(&w.id) {
            let subs =
                recur_forge::model::commit_subjects(pr.commits.as_deref().unwrap_or_default());
            if !subs.is_empty() {
                b.push_str("\nCommits:\n");
                for s in subs {
                    b.push_str("- ");
                    b.push_str(&s);
                    b.push('\n');
                }
            }
        }
        let recent = self.recent_activity(&w.id, WRAP_UP_ACTIVITY_N);
        if !recent.is_empty() {
            b.push_str("\nRecent activity (newest first):\n");
            for r in recent {
                b.push_str("- ");
                b.push_str(&truncate(&r, WRAP_UP_LINE_MAX));
                b.push('\n');
            }
        }
        b.push_str("\nWrite the wrap-up.");
        b
    }

    /// Appends the wrap to the latest Completed / PR-opened fact.
    pub fn attach_loop_wrap(&self, workspace_id: &str, wrap_html: &str) {
        let wrap_html = wrap_html.trim();
        if workspace_id.is_empty() || wrap_html.is_empty() {
            return;
        }
        for mut it in self.store.feed(workspace_id).unwrap_or_default() {
            if !is_completion_fact(&it.text) || it.text.contains(r#"class="loop-wrap""#) {
                continue;
            }
            it.text.push_str(wrap_html);
            self.store.upsert_feed(workspace_id, it.clone());
            let ev = EngineEvent::WorkspaceEvent(WorkspaceEvent {
                event: it,
                workspace_id: workspace_id.to_owned(),
            });
            self.persist_event(workspace_id, "", &ev);
            self.bus.publish(ev);
            return;
        }
    }
}
