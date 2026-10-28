//! Durable agent sessions and their config (Go `mirror.go`
//! `ensureAgentSession` / `agentIdentity` / `ensureRoutedSession`,
//! `chat_errors.go` `chatSessionMeta`, the session-config half of
//! `workspace_chat.go`, `app_chat.go` `appSessionByCode`, and the agent
//! records those need: `ensurePairAgent`, `ensureMaestroAgent`).

use recur_agent::{Flags, TurnRequest};
use recur_domain::{
    APP_AGENT_CODE, Agent, AgentRole, CHAT_MODE_ASK, CHAT_MODE_LOOP, CONTEXT_MODE_APP,
    CONTEXT_MODE_PROVIDER, PROVIDER_CLAUDE_CODE, SESSION_KIND_APP, Session, builtin_provider,
    normalize_chat_mode, parse_model_ref,
};
use serde::Deserialize;

use crate::Engine;
use crate::codes::{MAESTRO_AGENT_CODE, is_pair_chat_code, normalize_pair_chat_code};
use crate::roles::role_spec;
use crate::session_config::AgentConfig;

/// A specialist role code (CO, RV, PL, …): neither the conductor nor a
/// pair chat.
pub fn is_specialist_chat_code(code: &str) -> bool {
    !code.is_empty() && code != MAESTRO_AGENT_CODE && !is_pair_chat_code(code)
}

/// An app chat tab code; empty is the primary APP tab.
pub fn app_code(code: &str) -> String {
    let code = code.trim();
    if code.is_empty() {
        APP_AGENT_CODE.to_owned()
    } else {
        code.to_owned()
    }
}

fn config_json(cfg: &AgentConfig) -> String {
    serde_json::to_string(cfg).unwrap_or_else(|_| "{}".into())
}

impl Engine {
    /// The durable session for a (workspace, agent code), created on first
    /// use and reused across the agent's turns. A later turn on another
    /// provider rewrites `session.provider` so the row matches who ran.
    /// `None` only when the store has no database.
    pub fn ensure_agent_session(
        &self,
        workspace_id: &str,
        kind: &str,
        code: &str,
        name: &str,
        provider: &str,
    ) -> Option<Session> {
        self.ensure_owned_agent_session(workspace_id, kind, code, name, provider, "")
    }

    /// [`Engine::ensure_agent_session`] for a specialist owned by a conductor
    /// tab (its transcript is per owner).
    pub fn ensure_owned_agent_session(
        &self,
        workspace_id: &str,
        kind: &str,
        code: &str,
        name: &str,
        provider: &str,
        owner: &str,
    ) -> Option<Session> {
        let owner = owner.trim();
        let _g = self.session_ensure_mu.lock();
        let found = if is_specialist_chat_code(code) {
            self.store.session_for_chat_agent(workspace_id, owner, code)
        } else {
            self.store.session_for_loop_agent(workspace_id, code)
        };
        if let Some(mut sess) = found {
            if !provider.is_empty() && sess.provider != provider {
                let _ = self.store.update_session_provider(&sess.id, provider);
                sess.provider = provider.to_owned();
            }
            return Some(sess);
        }
        let specialist = is_specialist_chat_code(code);
        let cfg = AgentConfig {
            code: code.to_owned(),
            name: name.to_owned(),
            owner_code: if specialist {
                owner.to_owned()
            } else {
                String::new()
            },
            ..AgentConfig::default()
        };
        let context_mode = if !specialist && !provider.is_empty() && !builtin_provider(provider) {
            CONTEXT_MODE_APP
        } else {
            CONTEXT_MODE_PROVIDER
        };
        self.store
            .create_session(Session {
                workspace_id: workspace_id.to_owned(),
                kind: kind.to_owned(),
                context_mode: context_mode.to_owned(),
                provider: provider.to_owned(),
                config_json: config_json(&cfg),
                ..Session::default()
            })
            .ok()
    }

    /// The avatar code and display name of the agent a turn drives: the
    /// agent id suffix (else the role), named from the stored agent record
    /// (so the security Reviewer reads "Security Reviewer"), else the role
    /// table.
    pub fn agent_identity(&self, req: &TurnRequest) -> (String, String) {
        let prefix = format!("{}-", req.workspace_id);
        let mut code = req
            .agent_id
            .strip_prefix(&prefix)
            .unwrap_or(&req.agent_id)
            .to_owned();
        if code.is_empty() || code == req.agent_id {
            code = req.role.as_str().to_owned();
        }
        let mut name = self
            .store
            .agent(&req.workspace_id, &code)
            .map(|a| a.name)
            .unwrap_or_default();
        if name.is_empty() {
            name = role_spec(&req.role).name;
        }
        (code, name)
    }

    /// The session a routed turn mirrors into: an app tab's own session
    /// (never "the newest app session"), the newest session of another app
    /// kind, or the workspace agent session.
    pub(crate) fn ensure_routed_session(
        &self,
        req: &TurnRequest,
        provider: &str,
    ) -> Option<Session> {
        let mut code = req.route.code.clone();
        let mut name = req.route.name.clone();
        if code.is_empty() {
            (code, name) = self.agent_identity(req);
        }
        if !req.route.workspace_id.is_empty() {
            return self.ensure_agent_session(
                &req.route.workspace_id,
                &req.route.kind,
                &code,
                &name,
                provider,
            );
        }
        let found = if req.route.kind == SESSION_KIND_APP {
            self.app_session_by_code(&code)
        } else {
            self.store.session_for_app_kind(&req.route.kind)
        };
        if let Some(mut sess) = found {
            if !provider.is_empty() && sess.provider != provider {
                let _ = self.store.update_session_provider(&sess.id, provider);
                sess.provider = provider.to_owned();
            }
            return Some(sess);
        }
        self.store
            .create_session(Session {
                kind: req.route.kind.clone(),
                context_mode: CONTEXT_MODE_PROVIDER.into(),
                provider: provider.to_owned(),
                config_json: config_json(&AgentConfig {
                    code,
                    name,
                    mode: CHAT_MODE_ASK.into(),
                    ..AgentConfig::default()
                }),
                ..Session::default()
            })
            .ok()
    }

    /// The newest open app-kind session for one Ask Recur tab. Legacy rows
    /// without a code are the primary APP tab; closed sessions are skipped
    /// so a fresh tab never resurrects their transcript.
    pub fn app_session_by_code(&self, code: &str) -> Option<Session> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Meta {
            closed: bool,
        }
        let code = app_code(code);
        let list = self
            .store
            .sessions_for_app_kind(SESSION_KIND_APP)
            .unwrap_or_default();
        list.into_iter().rev().find(|s| {
            if serde_json::from_str::<Meta>(&s.config_json).is_ok_and(|m| m.closed) {
                return false;
            }
            let c = AgentConfig::parse(&s.config_json).code;
            let c = if c.is_empty() {
                APP_AGENT_CODE.to_owned()
            } else {
                c
            };
            c == code
        })
    }

    /// The role, name and provider of a chat tab's durable session, the
    /// provider taken from the tab's own model (never the Settings default,
    /// which would clobber `session.provider`).
    pub fn chat_session_meta(&self, workspace_id: &str, code: &str) -> (String, String, String) {
        let maestro = code == MAESTRO_AGENT_CODE;
        let mut name = if maestro { "Maestro" } else { "Agent" }.to_owned();
        let mut provider = PROVIDER_CLAUDE_CODE.to_owned();
        if let Some(a) = self.store.agent(workspace_id, code) {
            if !a.name.is_empty() {
                name = a.name;
            }
            let r = parse_model_ref(&a.model);
            if !r.provider.is_empty() {
                provider = r.provider;
            }
        }
        let kind = if maestro {
            AgentRole::Maestro
        } else {
            AgentRole::Coder
        };
        (kind.as_str().to_owned(), name, provider)
    }

    fn session_config(&self, workspace_id: &str, code: &str) -> Option<AgentConfig> {
        self.store
            .session_for_loop_agent(workspace_id, code)
            .map(|s| AgentConfig::parse(&s.config_json))
    }

    /// The last dock selection of a coding-agent tab: session config first
    /// (updated every send), else the agent record's model. Empty model =
    /// never sent; empty mode = the dock defaults to agent. MA's model comes
    /// from Settings and its mode is fixed.
    pub fn workspace_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> (String, Flags, String) {
        let code = normalize_pair_chat_code(code);
        if code == MAESTRO_AGENT_CODE {
            return (String::new(), Flags::new(), CHAT_MODE_LOOP.to_owned());
        }
        let mut mode = String::new();
        if let Some(cfg) = self.session_config(workspace_id, &code) {
            if !cfg.model.trim().is_empty() {
                return (cfg.model, cfg.flags, cfg.mode);
            }
            mode = cfg.mode;
        }
        if let Some(a) = self.store.agent(workspace_id, &code)
            && !a.model.trim().is_empty()
        {
            return (a.model, Flags::new(), mode);
        }
        (String::new(), Flags::new(), mode)
    }

    /// The effective mode for a turn: an explicit dock value (normalized),
    /// else the tab's persisted mode, else agent.
    pub fn resolve_chat_mode(&self, workspace_id: &str, code: &str, mode: &str) -> String {
        let m = mode.trim();
        if !m.is_empty() {
            return normalize_chat_mode(m).to_owned();
        }
        normalize_chat_mode(&self.chat_mode_for(workspace_id, code)).to_owned()
    }

    /// A tab's persisted mode ("" when never set).
    pub fn chat_mode_for(&self, workspace_id: &str, code: &str) -> String {
        self.session_config(workspace_id, &normalize_pair_chat_code(code))
            .map(|c| c.mode)
            .unwrap_or_default()
    }

    /// This tab's `[[loop]]` and `[[policy]]` ids.
    pub fn chat_loop_policy(&self, workspace_id: &str, code: &str) -> (String, String) {
        self.session_config(workspace_id, code)
            .map(|c| (c.loop_id.trim().to_owned(), c.policy_id.trim().to_owned()))
            .unwrap_or_default()
    }

    /// This tab's goal and its prompt-action label.
    pub fn chat_loop_goal(&self, workspace_id: &str, code: &str) -> (String, String) {
        if code.trim().is_empty() {
            return (String::new(), String::new());
        }
        self.session_config(workspace_id, code)
            .map(|c| (c.goal.trim().to_owned(), c.goal_display.trim().to_owned()))
            .unwrap_or_default()
    }

    /// This tab engaged its own run.
    pub fn chat_loop_started(&self, workspace_id: &str, code: &str) -> bool {
        self.session_config(workspace_id, code)
            .is_some_and(|c| c.loop_started)
    }

    /// Stamps a tab as a loop: loop/policy/goal (when given), started, and
    /// mode loop. Only the conductor (MA) and pair tabs carry a loop.
    #[allow(clippy::too_many_arguments)]
    pub fn stamp_chat_loop(
        &self,
        workspace_id: &str,
        code: &str,
        loop_id: &str,
        policy_id: &str,
        goal: &str,
        display: &str,
        started: bool,
    ) {
        if code != MAESTRO_AGENT_CODE && !is_pair_chat_code(code) {
            return;
        }
        if code == MAESTRO_AGENT_CODE {
            self.ensure_maestro_agent(workspace_id, "");
        } else {
            self.ensure_pair_agent(workspace_id, code, "");
        }
        let (kind, name, provider) = self.chat_session_meta(workspace_id, code);
        let Some(sess) = self.ensure_agent_session(workspace_id, &kind, code, &name, &provider)
        else {
            return;
        };
        let mut cfg = AgentConfig::parse(&sess.config_json);
        if !loop_id.trim().is_empty() {
            cfg.loop_id = loop_id.to_owned();
        }
        if !policy_id.trim().is_empty() {
            cfg.policy_id = policy_id.to_owned();
        }
        let g = goal.trim();
        if !g.is_empty() {
            cfg.goal = g.to_owned();
            cfg.goal_display = display.trim().to_owned();
        }
        if started {
            cfg.loop_started = true;
        }
        cfg.mode = CHAT_MODE_LOOP.into();
        if cfg.code.is_empty() {
            cfg.code = code.to_owned();
        }
        let _ = self
            .store
            .update_session_config_json(&sess.id, &config_json(&cfg));
    }

    /// The tab's agent record, created idle when missing. New records start
    /// idle: listing and creating tabs call this, and "working" would light
    /// the busy indicator with no turn in flight.
    pub fn ensure_pair_agent(&self, workspace_id: &str, code: &str, model: &str) {
        self.ensure_pair_agent_with_status(workspace_id, code, model, "idle");
    }

    /// [`Engine::ensure_pair_agent`] for the send path: a new record starts
    /// "working" so the first `agent.updated` is live.
    pub fn ensure_pair_agent_run(&self, workspace_id: &str, code: &str, model: &str) {
        self.ensure_pair_agent_with_status(workspace_id, code, model, "working");
    }

    pub fn ensure_pair_agent_with_status(
        &self,
        workspace_id: &str,
        code: &str,
        model: &str,
        status: &str,
    ) {
        let code = normalize_pair_chat_code(code);
        if self.store.agent(workspace_id, &code).is_some() {
            return;
        }
        let status = if status.is_empty() { "idle" } else { status };
        self.upsert_agent(
            workspace_id,
            Agent {
                id: format!("{workspace_id}-{code}"),
                workspace_id: workspace_id.to_owned(),
                code,
                name: "Agent".into(),
                role: "codes alongside you".into(),
                model: model.to_owned(),
                status: status.to_owned(),
                color: "var(--cyan)".into(),
                cur: -1,
                ..Agent::default()
            },
        );
    }

    /// The conductor's agent record, created "directing" when missing (keeps
    /// an existing record and its session id).
    pub fn ensure_maestro_agent(&self, workspace_id: &str, goal: &str) {
        self.ensure_maestro_agent_with_status(workspace_id, goal, "directing");
    }

    pub fn ensure_maestro_agent_with_status(&self, workspace_id: &str, goal: &str, status: &str) {
        if self.store.agent(workspace_id, MAESTRO_AGENT_CODE).is_some() {
            return;
        }
        let status = if status.is_empty() { "idle" } else { status };
        self.put_agent(
            workspace_id,
            Agent {
                id: format!("{workspace_id}-{MAESTRO_AGENT_CODE}"),
                workspace_id: workspace_id.to_owned(),
                code: MAESTRO_AGENT_CODE.into(),
                name: "Maestro".into(),
                role: "orchestrates the loop".into(),
                model: self.settings().maestro_model().id,
                status: status.to_owned(),
                color: "var(--amber)".into(),
                task: goal.to_owned(),
                cur: -1,
                ..Agent::default()
            },
        );
    }

    /// Coding-agent tabs of a workspace, ordered PA, PA2, ….
    pub fn pair_chat_agents(&self, workspace_id: &str) -> Vec<Agent> {
        let mut out: Vec<Agent> = self
            .store
            .agents(workspace_id)
            .into_iter()
            .filter(|a| is_pair_chat_code(&a.code))
            .collect();
        out.sort_by_key(|a| pair_chat_code_num(&a.code));
        out
    }
}

/// PA is 1, PAn is n (0 when malformed).
pub fn pair_chat_code_num(code: &str) -> i64 {
    if code == crate::codes::PAIR_AGENT_CODE {
        return 1;
    }
    code.strip_prefix("PA")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}
