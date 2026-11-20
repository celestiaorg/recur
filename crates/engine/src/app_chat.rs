//! Ask Recur: app-scoped coordinator chat tabs (APP, AP2, …) over an app-kind
//! session each, turns with the three briefing tools, unseen/focus for app
//! tabs, and `present_workspace` target validation with its brief card.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use recur_agent::attach::DISPLAY_HIDDEN_SENTINEL;
use recur_agent::display::summarize_tool_input;
use recur_agent::{Flags, SessionRoute, TurnRequest};
use recur_agent_tools::names::mcp;
use recur_domain::{
    APP_AGENT_CODE, AgentRole, BRIEF_ACTION_OPEN, BriefAction, CHAT_MODE_ASK,
    CONTEXT_MODE_PROVIDER, ChatItem, PROVIDER_CLAUDE_CODE, PresentWorkspaceResult,
    SESSION_KIND_APP, Session, WorkspaceBrief, WorkspaceChatTab, WorkspaceOverview,
    parse_model_ref,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::attention::chat_inbox::first_user_chat_title;
use crate::bus::{AgentUpdated, AppChatTab, EngineEvent};
use crate::chat::children::describe_child_state;
use crate::codes::{is_app_chat_code, is_chat_attention_code};
use crate::session_config::AgentConfig;
use crate::turn::AgentSpec;
use crate::turn::finalize::{CHAT_STOPPED_REPLY, ChatOutcome, is_canceled};
use crate::turn::sessions::app_code;
use crate::turn::stream::ChatStream;
use crate::turn::transcript::flatten_transcript;

mod reports;
#[cfg(test)]
mod tests;

pub(crate) use reports::{APP_REPORT_ARMED, app_report_tab};

/// Tool calls read_workspace lists.
const RECENT_TOOL_STEPS: usize = 15;

/// The wedged-turn bound of one Ask Recur turn.
pub const APP_CHAT_TIMEOUT: Duration = Duration::from_secs(120);

/// The coordinator's tools.
pub const APP_CHAT_TOOLS: [&str; 8] = [
    mcp::LIST_WORKSPACE_BRIEFS,
    mcp::GET_WORKSPACE_CONTEXT,
    mcp::READ_WORKSPACE,
    mcp::PRESENT_WORKSPACE,
    mcp::CREATE_WORKSPACE,
    mcp::MESSAGE_WORKSPACE,
    mcp::ANSWER_QUESTION,
    mcp::CREATE_PROJECT,
];

pub const APP_CHAT_APPEND_PROMPT: &str = "You are Ask Recur, Recur's app-scoped coordinator: the human's foreman across every project and workspace, so they can run all their work from here. You answer what needs attention, what is running, and where the human left off, and you direct the workspace agents that do the work. Always call mcp__recur__list_workspace_briefs before making a factual status claim: your transcript is conversational memory, not current truth. Use mcp__recur__get_workspace_context for one workspace's detail. For progress or how-is-it-going questions, call mcp__recur__read_workspace first: a live snapshot with its latest reply, plan, findings, changed files and recent tool steps. Use mcp__recur__present_workspace to pin a workspace/chat/question/PR card in this transcript (workspace:<id>, chat:<id>:<code>, question:<id>, pr:<id>); it does not switch the UI, the human opens the card. To start work, call mcp__recur__create_workspace with project_id (a project name works; an unknown one lists every project) and a prompt that gives the agent everything it needs to work alone. When the human asks for a project Recur does not have yet, call mcp__recur__create_project: it starts a new git repo (or clones clone_url) beside the other projects unless they name a path, then create_workspace in it. To redirect, follow up on, or restart a workspace's agent, call mcp__recur__message_workspace. When the human answers a workspace's pending question, relay it with mcp__recur__answer_question; never answer one on your own judgment. Create and message only for what the human asked. You do not edit files or run commands yourself, and you cannot approve permissions, merge, archive, or open PRs: workspace agents do the work, and permissions stay with the human.";

const APP_CHAT_AGENT_ID_PREFIX: &str = "app-";

/// The `agent.updated` agent id of an app tab: an empty workspace plus this
/// prefix lights the chip without a fake workspace agent row.
pub fn app_chat_agent_id(code: &str) -> String {
    format!("{APP_CHAT_AGENT_ID_PREFIX}{}", app_code(code))
}

pub(crate) fn app_chat_tab_name(code: &str) -> &'static str {
    if code == APP_AGENT_CODE {
        "Ask Recur"
    } else {
        "New chat"
    }
}

/// A session's tab label: its title, else its name, else its code.
pub fn session_title(cfg: &AgentConfig) -> String {
    let t = cfg.title.trim();
    if !t.is_empty() {
        return t.to_owned();
    }
    let n = cfg.name.trim();
    if !n.is_empty() {
        return n.to_owned();
    }
    cfg.code.clone()
}

/// A session config as a map, keys sorted on write.
fn config_map(raw: &str) -> BTreeMap<String, Value> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn config_json(m: &BTreeMap<String, Value>) -> String {
    serde_json::to_string(m).unwrap_or_else(|_| "{}".into())
}

fn agent_config_json(cfg: &AgentConfig) -> String {
    serde_json::to_string(cfg).unwrap_or_else(|_| "{}".into())
}

fn present_json(out: &PresentWorkspaceResult) -> String {
    match serde_json::to_string(out) {
        Ok(s) => s,
        Err(_) if out.ok => format!("Presented {}.", out.target),
        Err(_) if !out.error.is_empty() => out.error.clone(),
        Err(_) => "Refused: could not present that target.".into(),
    }
}

fn split_once_or_all(s: &str) -> (&str, &str, bool) {
    match s.split_once(':') {
        Some((a, b)) => (a, b, true),
        None => (s, "", false),
    }
}

impl Engine {
    fn publish_app_chat_status(&self, code: &str, status: &str) {
        let code = app_code(code);
        self.bus.publish(EngineEvent::AgentUpdated(AgentUpdated {
            agent_id: app_chat_agent_id(&code),
            code: Some(code),
            status: status.to_owned(),
            surface: Some("app".into()),
            workspace_id: String::new(),
        }));
    }

    /// One Ask Recur tab's persistent transcript.
    pub fn app_chat_items(&self, code: &str) -> Vec<ChatItem> {
        let Some(sess) = self.app_session_by_code(code) else {
            return Vec::new();
        };
        let Ok(Some(msgs)) = self.store.session_transcript(&sess.id) else {
            return Vec::new();
        };
        let cfg = AgentConfig::parse(&sess.config_json);
        flatten_transcript(&self.providers, &msgs, &cfg.model, &cfg.flags)
    }

    /// The open Ask Recur tabs, primary first, in creation order. APP is
    /// always present: with no open session it is the fresh chat the next
    /// send creates (closing the last tab resets it).
    pub fn list_app_chats(&self) -> Vec<WorkspaceChatTab> {
        let mut tabs = vec![WorkspaceChatTab {
            code: APP_AGENT_CODE.into(),
            title: "Ask Recur".into(),
            mode: CHAT_MODE_ASK.into(),
            ..WorkspaceChatTab::default()
        }];
        for sess in self
            .store
            .sessions_for_app_kind(SESSION_KIND_APP)
            .unwrap_or_default()
        {
            let raw = config_map(&sess.config_json);
            if raw.get("closed").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let ac = AgentConfig::parse(&sess.config_json);
            let code = if ac.code.is_empty() {
                APP_AGENT_CODE.to_owned()
            } else {
                ac.code.clone()
            };
            let unseen = raw.get("unseen").and_then(Value::as_bool).unwrap_or(false);
            if code == APP_AGENT_CODE {
                let mut title = session_title(&ac);
                if title == APP_AGENT_CODE {
                    title = "Ask Recur".into();
                }
                tabs[0].title = title;
                tabs[0].unseen = unseen;
                tabs[0].updated_at = sess.updated_at;
                continue;
            }
            tabs.push(WorkspaceChatTab {
                code,
                title: session_title(&ac),
                mode: CHAT_MODE_ASK.into(),
                unseen,
                updated_at: sess.updated_at,
                ..WorkspaceChatTab::default()
            });
        }
        tabs
    }

    /// Opens a fresh Ask Recur tab. Codes AP2, AP3, … are never reused, so a
    /// closed tab's history cannot leak into a new one.
    pub fn new_app_chat(&self) -> WorkspaceChatTab {
        let mut max_n = 1;
        for sess in self
            .store
            .sessions_for_app_kind(SESSION_KIND_APP)
            .unwrap_or_default()
        {
            let cfg = AgentConfig::parse(&sess.config_json);
            if let Some(n) = cfg
                .code
                .strip_prefix("AP")
                .and_then(|n| n.parse::<i64>().ok())
                && n > max_n
            {
                max_n = n;
            }
        }
        let code = format!("AP{}", max_n + 1);
        let Some(sess) = self.ensure_app_session(&code, "") else {
            return WorkspaceChatTab::default();
        };
        WorkspaceChatTab {
            code,
            title: "New chat".into(),
            mode: CHAT_MODE_ASK.into(),
            updated_at: sess.updated_at,
            ..WorkspaceChatTab::default()
        }
    }

    /// Closes an app tab (history stays). Closing APP resets it: the next
    /// send creates a new session under the same code. Returns the open
    /// tabs.
    pub fn close_app_chat(&self, code: &str) -> Vec<WorkspaceChatTab> {
        let code = app_code(code);
        if let Some(sess) = self.app_session_by_code(&code) {
            let mut cfg = config_map(&sess.config_json);
            cfg.insert("closed".into(), Value::Bool(true));
            cfg.remove("unseen");
            let _ = self
                .store
                .update_session_config_json(&sess.id, &config_json(&cfg));
            self.publish_attention("", &code, false);
        }
        self.list_app_chats()
    }

    /// A tab's last model and flags (its mode is always ask).
    pub fn app_chat_selection(&self, code: &str) -> (String, Flags) {
        let set = self.settings();
        match self.app_session_by_code(code) {
            Some(sess) => {
                let cfg = AgentConfig::parse(&sess.config_json);
                if cfg.model.is_empty() {
                    (set.ask_model().to_string(), set.ask_flags())
                } else {
                    (cfg.model, cfg.flags)
                }
            }
            None => (set.ask_model().to_string(), set.ask_flags()),
        }
    }

    /// Persists a tab's model picker.
    pub fn remember_app_chat_selection(&self, code: &str, model: &str, flags: Option<&Flags>) {
        let code = app_code(code);
        let model = model.trim();
        let Some(sess) = self.ensure_app_session(&code, model) else {
            return;
        };
        let mut cfg = AgentConfig::parse(&sess.config_json);
        cfg.code = code.clone();
        if cfg.name.is_empty() {
            cfg.name = app_chat_tab_name(&code).into();
        }
        cfg.mode = CHAT_MODE_ASK.into();
        if !model.is_empty() {
            cfg.model = model.to_owned();
            cfg.flags = flags.cloned().unwrap_or_default();
            let p = parse_model_ref(model).provider;
            if !p.is_empty() && sess.provider != p {
                let _ = self.store.update_session_provider(&sess.id, &p);
            }
        }
        let _ = self
            .store
            .update_session_config_json(&sess.id, &agent_config_json(&cfg));
    }

    fn ensure_app_session(&self, code: &str, model: &str) -> Option<Session> {
        let code = app_code(code);
        if let Some(sess) = self.app_session_by_code(&code) {
            return Some(sess);
        }
        let mut provider = parse_model_ref(model).provider;
        if provider.is_empty() {
            provider = PROVIDER_CLAUDE_CODE.into();
        }
        let cfg = AgentConfig {
            name: app_chat_tab_name(&code).into(),
            mode: CHAT_MODE_ASK.into(),
            model: model.to_owned(),
            code,
            ..AgentConfig::default()
        };
        self.store
            .create_session(Session {
                kind: SESSION_KIND_APP.into(),
                context_mode: CONTEXT_MODE_PROVIDER.into(),
                provider,
                config_json: agent_config_json(&cfg),
                ..Session::default()
            })
            .ok()
    }

    /// Labels a tab from its first user line, so the strip reads by topic.
    fn stamp_app_chat_title(&self, code: &str, text: &str) {
        let Some(sess) = self.app_session_by_code(code) else {
            return;
        };
        if !AgentConfig::parse(&sess.config_json)
            .title
            .trim()
            .is_empty()
        {
            return;
        }
        match self.store.session_transcript(&sess.id) {
            Ok(m) if m.as_ref().is_none_or(Vec::is_empty) => {}
            _ => return,
        }
        let title = first_user_chat_title(text);
        if title.is_empty()
            || self
                .store
                .patch_session_config_title(&sess.id, &title)
                .is_err()
        {
            return;
        }
        self.bus.publish(EngineEvent::AppChatTab(AppChatTab {
            code: code.to_owned(),
            title,
        }));
    }

    /// One Ask Recur turn on a tab. The agent-slot wait is cancelable.
    pub async fn send_app_message(
        &self,
        code: &str,
        text: &str,
        model: &str,
        flags: Option<&Flags>,
    ) -> String {
        self.send_app_message_viewing(code, text, "", model, flags)
            .await
    }

    /// [`Engine::send_app_message`] while the human has `viewing` open in the Inbox
    /// overview ("" = none): the turn is told so, so "this" means that workspace.
    pub async fn send_app_message_viewing(
        &self,
        code: &str,
        text: &str,
        viewing: &str,
        model: &str,
        flags: Option<&Flags>,
    ) -> String {
        let mut wire = self.attach_ref_mentions(text, text).await;
        if let Some(note) = self.viewing_note(viewing) {
            wire = format!("{note}{wire}");
        }
        let display = if wire == text { "" } else { text };
        self.run_app_turn(code, &wire, display, model, flags).await
    }

    fn viewing_note(&self, id: &str) -> Option<String> {
        let ws = self.store.workspace(id).filter(|w| !w.archived)?;
        let project = self
            .store
            .project(&ws.project_id)
            .map(|p| p.name)
            .unwrap_or_default();
        Some(format!(
            "<system_instruction>\nThe human has workspace {id} ({:?} in {project}) open in the Inbox. Unless they name another workspace, \"this\" and \"it\" mean that workspace: look with mcp__recur__read_workspace. Message its agent with mcp__recur__message_workspace only when the human clearly addresses this workspace; ask which workspace when a request does not say.\n</system_instruction>\n\n",
            crate::briefing::workspace_title(&ws)
        ))
    }

    /// An Ask Recur turn; `display` labels the user bubble (a hidden-marker
    /// display is an engine notice, which never titles the tab).
    async fn run_app_turn(
        &self,
        code: &str,
        text: &str,
        display: &str,
        model: &str,
        flags: Option<&Flags>,
    ) -> String {
        let code = app_code(code);
        let text = text.trim();
        if text.is_empty() {
            return String::new();
        }
        let cwd = self.data_dir.join("app-chat");
        if let Err(e) = std::fs::create_dir_all(&cwd) {
            return format!("I can't run right now: {e}");
        }
        let set = self.settings();
        let model = if model.is_empty() {
            set.ask_model().to_string()
        } else {
            model.to_owned()
        };
        let flags = flags.cloned().unwrap_or_else(|| set.ask_flags());
        self.remember_app_chat_selection(&code, &model, Some(&flags));
        let notice = display.starts_with(DISPLAY_HIDDEN_SENTINEL);
        if !notice {
            self.stamp_app_chat_title(&code, if display.is_empty() { text } else { display });
        }
        // An engine notice carries workspace output with no human present: no answering
        // questions or creating projects on its say-so.
        let allowed: Vec<String> = APP_CHAT_TOOLS
            .iter()
            .filter(|t| !notice || ![mcp::ANSWER_QUESTION, mcp::CREATE_PROJECT].contains(t))
            .map(|s| (*s).to_owned())
            .collect();
        let mut req = TurnRequest {
            agent_id: code.clone(),
            role: AgentRole::Maestro,
            model: model.clone(),
            flags,
            append_system_prompt: APP_CHAT_APPEND_PROMPT.into(),
            allowed_tools: Some(allowed.clone()),
            prompt: text.to_owned(),
            display_text: display.to_owned(),
            worktree: cwd,
            permission_mode: set.permission_mode().into(),
            include_partials: true,
            route: SessionRoute {
                kind: SESSION_KIND_APP.into(),
                code: code.clone(),
                name: app_chat_tab_name(&code).into(),
                ..SessionRoute::default()
            },
            ..TurnRequest::default()
        };
        let (token, url) = match self.register_mcp("", &code, &code, &allowed, &model).await {
            Ok(t) => t,
            Err(e) => return format!("I can't run right now: {e}"),
        };
        req.mcp_url = url;
        let cancel = CancellationToken::new();
        let Some(turn) = self.register_chat_turn("", &code, cancel.clone()) else {
            self.unregister_mcp(&token);
            return "I can't start Ask Recur right now.".into();
        };
        turn.arm(APP_CHAT_TIMEOUT);
        let finish = |e: &Engine| {
            e.unregister_chat_turn("", &code, &turn);
            cancel.cancel();
            e.unregister_mcp(&token);
        };
        let Some(permit) = self.acquire_agent(&cancel).await else {
            let reply = if cancel.is_cancelled() {
                CHAT_STOPPED_REPLY.to_owned()
            } else {
                "I'm at capacity right now: every agent slot is busy. Try again in a moment."
                    .to_owned()
            };
            finish(self);
            return reply;
        };
        self.publish_app_chat_status(&code, "working");
        let stream = ChatStream::new(self.arc(), "", &code);
        let started = Instant::now();
        let mut spec = AgentSpec::new(req);
        spec.emit = Some(stream.emit());
        let out = self.run_agent(&cancel, spec).await;
        self.publish_resource();
        let (res, err) = match &out {
            Ok(r) => (r.clone(), None),
            Err(e) => ((*e.partial).clone(), Some(e)),
        };
        let err_dyn = err.map(|e| e as &(dyn std::error::Error + 'static));
        let reply = self.finalize_chat_reply(ChatOutcome {
            workspace_id: "",
            code: &code,
            prompt: text,
            display,
            streamed: &stream.text(),
            res: &res,
            err: err_dyn,
            empty_fallback: "I couldn't produce a reply.",
            started: Some(started),
            skip_user: false,
            user_msg_id: "",
        });
        let canceled = is_canceled(err_dyn);
        if !canceled && !self.is_app_chat_focused(&code) {
            self.flag_app_chat_unseen(&code);
        }
        // A cancelled unwind during quit stays live so Recover can resume;
        // anything else goes idle.
        if !(self.is_shutting_down() && canceled) {
            self.publish_app_chat_status(&code, "idle");
        }
        drop(permit);
        finish(self);
        reply
    }

    /// Interrupts a tab's in-flight Ask Recur turn.
    pub async fn cancel_app_message(&self, code: &str) {
        self.cancel_chat("", &app_code(code)).await;
    }

    /// Records which app tab is visible (`visible` false: the panel hid).
    pub fn set_app_chat_focus(&self, visible: bool, code: &str) {
        let code = app_code(code);
        let attentive = {
            let mut f = self.chat_focus.lock();
            if visible {
                f.workspace_id.clear();
                f.code = code.clone();
            } else if is_app_chat_code(&f.code) && f.workspace_id.is_empty() {
                f.workspace_id.clear();
                f.code.clear();
            }
            visible && f.attentive()
        };
        if attentive {
            self.mark_app_chat_seen(&code);
        }
    }

    fn is_app_chat_focused(&self, code: &str) -> bool {
        let f = self.chat_focus.lock();
        f.attentive() && f.code == app_code(code) && f.workspace_id.is_empty()
    }

    /// Clears one app tab's unseen flag.
    pub fn mark_app_chat_seen(&self, code: &str) {
        let code = app_code(code);
        if self.set_app_chat_unseen(&code, false) {
            self.publish_attention("", &code, false);
        }
    }

    fn flag_app_chat_unseen(&self, code: &str) {
        let code = app_code(code);
        if self.is_app_chat_focused(&code) {
            return;
        }
        if self.set_app_chat_unseen(&code, true) {
            self.publish_attention("", &code, true);
        }
    }

    /// Persists one tab's unseen flag; true when it changed.
    fn set_app_chat_unseen(&self, code: &str, unseen: bool) -> bool {
        let Some(sess) = self.ensure_app_session(code, "") else {
            return false;
        };
        let mut cfg = config_map(&sess.config_json);
        let cur = cfg.get("unseen").and_then(Value::as_bool).unwrap_or(false);
        if cur == unseen {
            return false;
        }
        if unseen {
            cfg.insert("unseen".into(), Value::Bool(true));
        } else {
            cfg.remove("unseen");
        }
        let _ = self
            .store
            .update_session_config_json(&sess.id, &config_json(&cfg));
        true
    }

    /// The read_workspace tool: read_child for any workspace, against its
    /// project's base branch, plus its latest chat's recent tool steps.
    pub async fn read_workspace_from_app(&self, id: &str, with_diff: bool) -> String {
        let Some(ws) = self.store.workspace(id) else {
            return format!("read_workspace: no workspace {id}");
        };
        let mut b = format!("Workspace {} ({:?}, branch {}) ", ws.id, ws.goal, ws.branch);
        if !ws.archived && self.workspace_working(id) {
            b.push_str("is RUNNING: this is a live snapshot, not its final result.\n\n");
        } else {
            b.push_str(&format!("has {}.\n\n", describe_child_state(&ws)));
        }
        let base = self
            .store
            .project(&ws.project_id)
            .map(|p| (p.id, p.base_branch));
        self.push_workspace_report(&mut b, &ws, base, with_diff)
            .await;
        let pr = self.pr_status_line(id);
        if !pr.is_empty() {
            b.push_str(&format!("{pr}\n\n"));
        }
        let steps = self.recent_tool_steps(id);
        if !steps.is_empty() {
            b.push_str("Recent tool steps, oldest first:\n");
            for s in steps {
                b.push_str(&format!("- {s}\n"));
            }
        }
        b
    }

    /// The Inbox overview: plan, agents, open asks and PR (no briefing
    /// snapshot or transcript read), the latest reply, the last tool steps and
    /// failing checks. Changed files are [`Engine::workspace_changes`].
    pub fn workspace_overview(&self, id: &str) -> crate::Result<WorkspaceOverview> {
        let Some(ws) = self.store.workspace(id).filter(|w| !w.archived) else {
            return Err(crate::EngineError(format!("workspace {id} not found")));
        };
        let mut context = recur_domain::WorkspaceContext::default();
        context.brief.workspace_id = ws.id.clone();
        self.fill_workspace_detail(&ws, &mut context);
        let failing_checks = self
            .workspace_pr(&ws)
            .and_then(|p| p.checks)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.failed())
            .collect();
        let mut open_findings: Vec<_> = self
            .store
            .findings(id)
            .into_iter()
            .filter(|f| f.status == "open")
            .collect();
        let rank = |f: &recur_domain::Finding| {
            ["critical", "high", "medium", "low", "info"]
                .iter()
                .position(|s| *s == f.severity.as_str())
                .unwrap_or(5)
        };
        open_findings.sort_by_key(rank);
        Ok(WorkspaceOverview {
            context,
            running: self.workspace_working(id),
            last_reply: self.store.last_assistant_text(id).trim().to_owned(),
            recent_steps: self.recent_tool_steps(id),
            failing_checks,
            open_findings,
            plan_summary: ws
                .plan
                .as_ref()
                .map(|p| p.summary.trim().to_owned())
                .unwrap_or_default(),
            plan_body: ws
                .plan
                .as_ref()
                .map(|p| p.body.trim().to_owned())
                .unwrap_or_default(),
        })
    }

    /// Files committed against the project's base branch, with line counts.
    pub async fn workspace_changes(&self, id: &str) -> recur_domain::WorkspaceChanges {
        let Some(ws) = self.store.workspace(id).filter(|w| !w.archived) else {
            return Default::default();
        };
        let Some(proj) = self.store.project(&ws.project_id) else {
            return Default::default();
        };
        if ws.worktree_path.is_empty() {
            return Default::default();
        }
        let files = tokio::time::timeout(
            Duration::from_secs(30),
            self.git_manager(&proj.repo_path)
                .changed_numstat(&ws.worktree_path, &proj.base_branch),
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(path, add, del)| recur_domain::ChangedFile { path, add, del })
        .collect();
        recur_domain::WorkspaceChanges {
            base_branch: proj.base_branch,
            files,
        }
    }

    /// The Inbox's Stop: interrupts every live chat turn on the workspace and
    /// stops its workflow.
    pub async fn stop_workspace_from_inbox(&self, id: &str) {
        let prefix = crate::codes::chat_turn_key(id, "");
        let codes: Vec<String> = self
            .live_chat_turns()
            .into_iter()
            .filter_map(|(k, _)| k.strip_prefix(&prefix).map(str::to_owned))
            .collect();
        for code in codes {
            self.cancel_chat(id, &code).await;
        }
        if self.running_loop_by_id(id).is_some() {
            self.stop(id).await;
        }
    }

    /// The last tool calls of the workspace's most recently active chat, oldest first.
    fn recent_tool_steps(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .store
            .recent_tool_uses(id, RECENT_TOOL_STEPS)
            .into_iter()
            .map(|t| {
                let input = summarize_tool_input(&t.name, &t.input);
                if input.is_empty() {
                    t.name
                } else {
                    format!("{} {input}", t.name)
                }
            })
            .collect();
        out.reverse();
        out
    }

    /// Validates a navigation target and returns its JSON card payload. It
    /// does not switch the shell: the transcript renders the card.
    pub fn present_workspace(&self, target: &str) -> String {
        let target = target.trim();
        if target.is_empty() {
            return present_json(&PresentWorkspaceResult {
                error: "Refused: target is required (workspace:<id>, chat:<id>:<code>, question:<id>, or pr:<id>).".into(),
                ..PresentWorkspaceResult::default()
            });
        }
        let mut out = PresentWorkspaceResult {
            target: target.to_owned(),
            kind: split_once_or_all(target).0.to_owned(),
            ..PresentWorkspaceResult::default()
        };
        if let Err(e) = self.validate_present_target(target) {
            out.error = format!("Refused: {e}");
            return present_json(&out);
        }
        out.ok = true;
        out.brief = self.brief_for_present_target(target);
        present_json(&out)
    }

    /// The Overview card of a validated target, so the transcript renders it
    /// without a second round trip; a settled workspace still gets a
    /// minimal identity card.
    fn brief_for_present_target(&self, target: &str) -> Option<WorkspaceBrief> {
        let (kind, rest, _) = split_once_or_all(target);
        let rest = rest.trim();
        let ws_id = match kind {
            "workspace" | "loop" | "chat" => split_once_or_all(rest).0.to_owned(),
            "pr" => self
                .store
                .prs()
                .into_iter()
                .find(|p| p.id == rest)
                .map(|p| p.workspace_id)
                .unwrap_or_default(),
            "question" => self
                .pending_ask_users()
                .into_iter()
                .find(|q| q.id == rest)
                .map(|q| q.workspace_id)
                .unwrap_or_default(),
            _ => String::new(),
        };
        let ws_id = ws_id.trim();
        if ws_id.is_empty() {
            return None;
        }
        if let Some(b) = self
            .briefing_snapshot()
            .workspaces
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.workspace_id == ws_id)
        {
            return Some(b);
        }
        let ws = self.store.workspace(ws_id).filter(|w| !w.archived)?;
        let mut title = ws.short.trim().to_owned();
        if title.is_empty() {
            title = ws.goal.trim().to_owned();
        }
        if title.is_empty() {
            title = ws.id.clone();
        }
        let mut b = WorkspaceBrief {
            workspace_id: ws.id.clone(),
            project_id: ws.project_id.clone(),
            title,
            branch: ws.branch.clone(),
            state: ws.state.as_str().to_owned(),
            phase: ws.phase.clone(),
            primary: BriefAction {
                label: "Open".into(),
                target: target.to_owned(),
                kind: BRIEF_ACTION_OPEN.into(),
            },
            ..WorkspaceBrief::default()
        };
        if let Some(p) = self.store.project(&ws.project_id) {
            let name = p.name.trim();
            b.project_name = if name.is_empty() {
                p.id.clone()
            } else {
                name.to_owned()
            };
        }
        Some(b)
    }

    fn validate_present_target(&self, target: &str) -> Result<(), &'static str> {
        let (kind, rest, _) = split_once_or_all(target);
        let rest = rest.trim();
        let live = |id: &str| self.store.workspace(id).is_some_and(|w| !w.archived);
        match kind {
            "workspace" | "loop" => {
                if live(split_once_or_all(rest).0) {
                    Ok(())
                } else {
                    Err("that workspace is not available")
                }
            }
            "chat" => {
                let (ws_id, code, ok) = split_once_or_all(rest);
                if !ok {
                    return Err("chat target must be chat:<workspaceId>:<code>");
                }
                if !live(ws_id) {
                    return Err("that workspace is not available");
                }
                if !is_chat_attention_code(code) {
                    return Err("unknown chat tab");
                }
                Ok(())
            }
            "question" => {
                if self.pending_ask_users().iter().any(|q| q.id == rest) {
                    Ok(())
                } else {
                    Err("that question is no longer pending")
                }
            }
            "pr" => {
                if self.store.prs().iter().any(|p| p.id == rest) {
                    Ok(())
                } else {
                    Err("that pull request is not available")
                }
            }
            _ => Err("unknown target kind"),
        }
    }
}
