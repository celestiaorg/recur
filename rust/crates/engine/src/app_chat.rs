//! Ask Recur (Go `app_chat.go`): app-scoped coordinator chat tabs (APP,
//! AP2, …) over an app-kind session each, turns with the three briefing
//! tools, unseen/focus for app tabs, and `present_workspace` target
//! validation with its brief card.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use recur_agent::{Flags, SessionRoute, TurnRequest};
use recur_agent_tools::names::mcp;
use recur_domain::{
    APP_AGENT_CODE, AgentRole, BRIEF_ACTION_OPEN, BriefAction, CHAT_MODE_ASK,
    CONTEXT_MODE_PROVIDER, ChatItem, PROVIDER_CLAUDE_CODE, PresentWorkspaceResult,
    SESSION_KIND_APP, Session, WorkspaceBrief, WorkspaceChatTab, parse_model_ref,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::attention::chat_inbox::first_user_chat_title;
use crate::bus::{AgentUpdated, AppChatTab, EngineEvent};
use crate::codes::{is_app_chat_code, is_chat_attention_code};
use crate::session_config::AgentConfig;
use crate::turn::AgentSpec;
use crate::turn::finalize::{CHAT_STOPPED_REPLY, ChatOutcome, is_canceled};
use crate::turn::sessions::app_code;
use crate::turn::stream::ChatStream;
use crate::turn::transcript::flatten_transcript;

/// The wedged-turn bound of one Ask Recur turn.
pub const APP_CHAT_TIMEOUT: Duration = Duration::from_secs(120);

/// The coordinator's tools.
pub const APP_CHAT_TOOLS: [&str; 3] = [
    mcp::LIST_WORKSPACE_BRIEFS,
    mcp::GET_WORKSPACE_CONTEXT,
    mcp::PRESENT_WORKSPACE,
];

pub const APP_CHAT_APPEND_PROMPT: &str = "You are Ask Recur, Recur's app-scoped coordinator. You answer questions about what needs attention, what is running, and where the human left off. Always call mcp__recur__list_workspace_briefs before making a factual status claim — your transcript is conversational memory, not current truth. Use mcp__recur__get_workspace_context for one workspace's detail and mcp__recur__present_workspace to pin a workspace/chat/question/PR card in this transcript (workspace:<id>, chat:<id>:<code>, question:<id>, pr:<id>). present_workspace does not switch the UI — the human opens the card. You cannot edit files, mutate workspaces, answer questions, approve permissions, retry runs, or message workspace agents. If the user asks you to change code, tell them to open that workspace's Agent chat.";

const APP_CHAT_AGENT_ID_PREFIX: &str = "app-";

/// The `agent.updated` agent id of an app tab: an empty workspace plus this
/// prefix lights the chip without a fake workspace agent row.
pub fn app_chat_agent_id(code: &str) -> String {
    format!("{APP_CHAT_AGENT_ID_PREFIX}{}", app_code(code))
}

fn app_chat_tab_name(code: &str) -> &'static str {
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

/// A session config as Go's `map[string]any` (keys sorted on write).
fn config_map(raw: &str) -> BTreeMap<String, Value> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn config_json(m: &BTreeMap<String, Value>) -> String {
    recur_domain::json::to_string(m).unwrap_or_else(|_| "{}".into())
}

fn agent_config_json(cfg: &AgentConfig) -> String {
    recur_domain::json::to_string(cfg).unwrap_or_else(|_| "{}".into())
}

fn present_json(out: &PresentWorkspaceResult) -> String {
    match recur_domain::json::to_string(out) {
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
        self.stamp_app_chat_title(&code, text);
        let allowed: Vec<String> = APP_CHAT_TOOLS.iter().map(|s| (*s).to_owned()).collect();
        let mut req = TurnRequest {
            agent_id: code.clone(),
            role: AgentRole::Maestro,
            model: model.clone(),
            flags,
            append_system_prompt: APP_CHAT_APPEND_PROMPT.into(),
            allowed_tools: Some(allowed.clone()),
            prompt: text.to_owned(),
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
                "I'm at capacity right now — every agent slot is busy. Try again in a moment."
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
            display: "",
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
