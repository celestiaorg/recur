//! The engine's `recur_agent_tools::Host` (Go `engineReporter`): the single
//! seam through which an agent's tool call re-enters the engine as a typed
//! signal. Callbacks arrive from MCP request tasks concurrently with loop
//! walks; the host keeps no state of its own and routes to the running
//! walk's control plane and the store.
//!
//! Ownership is per method (plan §2.4). E1 implements the loop-flow signals
//! below; every other method keeps the trait default (Go's `noopReporter`)
//! until its slice adds it here.

use std::sync::{Arc, Weak};

use recur_agent_tools::{EscalationRequest, Evidence, Finding, Host, HostError};
use recur_domain::{
    CHAT_MODE_LOOP, CHAT_MODE_REVIEW, FINDING_ACTOR_CODER, FINDING_ACTOR_REVIEWER, FindingStatus,
    TestResult, finding_loc, normalize_chat_mode,
};

use crate::Engine;
use crate::bus::{EngineEvent, WorkspaceRef};
use crate::codes::{MAESTRO_AGENT_CODE, normalize_pair_chat_code};
use crate::roles::{role_spec, security_reviewer_role};
use crate::session_config::AgentConfig;

/// The engine's Host. Holds the engine weakly (the engine owns the MCP
/// server that owns this host).
pub struct EngineHost {
    eng: Weak<Engine>,
}

impl EngineHost {
    pub fn new(eng: Weak<Engine>) -> Self {
        Self { eng }
    }

    fn engine(&self) -> Option<Arc<Engine>> {
        self.eng.upgrade()
    }
}

impl Engine {
    /// A chat tab's persisted mode as the dock selection reads it (Go
    /// `WorkspaceChatSelection`'s mode): MA is always loop.
    pub fn chat_selection_mode(&self, workspace_id: &str, code: &str) -> String {
        let code = normalize_pair_chat_code(code);
        if code == MAESTRO_AGENT_CODE {
            return CHAT_MODE_LOOP.to_owned();
        }
        match self.store.session_for_loop_agent(workspace_id, &code) {
            Some(sess) => AgentConfig::parse(&sess.config_json).mode,
            None => String::new(),
        }
    }

    /// Maps an MCP caller to the persisted UpdatedBy actor: reviewer codes
    /// and Review-mode chats are the reviewer, everyone else the coder.
    pub fn finding_update_actor(&self, workspace_id: &str, agent_code: &str) -> &'static str {
        let code = agent_code.trim();
        if code == role_spec(&recur_domain::AgentRole::Reviewer).code
            || code == security_reviewer_role().code
        {
            return FINDING_ACTOR_REVIEWER;
        }
        if normalize_chat_mode(&self.chat_selection_mode(workspace_id, code)) == CHAT_MODE_REVIEW {
            return FINDING_ACTOR_REVIEWER;
        }
        FINDING_ACTOR_CODER
    }

    fn publish_findings_updated(&self, workspace_id: &str) {
        self.bus
            .publish(EngineEvent::FindingsUpdated(WorkspaceRef::new(
                workspace_id,
            )));
    }

    /// Host `report_finding`: stamps the live walk's review iteration (or
    /// the workspace's when no walk runs) and stores the finding.
    pub fn report_finding(&self, workspace_id: &str, f: Finding) -> recur_domain::Finding {
        let iter = match self.running_loop_by_id(workspace_id) {
            Some(lp) => lp.collect_finding(),
            // A chat-dispatched specialist over a settled loop: persist at the
            // workspace's iteration; there is no convergence count to feed.
            None => self
                .store
                .workspace(workspace_id)
                .map_or(1, |ws| if ws.iteration > 0 { ws.iteration } else { 1 }),
        };
        let stored = self.store.add_finding(recur_domain::Finding {
            workspace_id: workspace_id.to_owned(),
            iteration: iter,
            file: f.file,
            line: f.line,
            end_line: f.end_line,
            severity: f.severity,
            message: f.message,
            summary: f.summary.trim().to_owned(),
            status: FindingStatus::Open.as_str().to_owned(),
            ..recur_domain::Finding::default()
        });
        self.publish_findings_updated(workspace_id);
        stored
    }

    /// Host `done`: the first-party Coder may not finish while findings are
    /// open (the refusal lists them); the implementer's summary lands on the
    /// walk that owns the turn.
    pub fn done(
        &self,
        workspace_id: &str,
        agent_code: &str,
        summary: &str,
        notes: &str,
    ) -> Result<(), HostError> {
        if agent_code == role_spec(&recur_domain::AgentRole::Coder).code {
            let open = self.store.open_findings(workspace_id).unwrap_or_default();
            if !open.is_empty() {
                let mut b = format!(
                    "cannot finish: {} open finding(s) remain. Address each, call mcp__recur__update_finding \
(resolved or invalid with reason), then call done again.\n",
                    open.len()
                );
                for (i, f) in open.iter().enumerate() {
                    b.push_str(&format!(
                        "{}. id={} [{}] {} — {}\n",
                        i + 1,
                        f.public_id(),
                        f.severity.as_str(),
                        finding_loc(&f.file, f.line, f.end_line),
                        f.message
                    ));
                    for n in &f.notes {
                        let body = n.body.trim();
                        if body.is_empty() {
                            continue;
                        }
                        b.push_str(&format!("   operator note: {body}\n"));
                    }
                }
                if let Some(lp) = self.running_loop_for_done(workspace_id, agent_code)
                    && lp.accepts_done_summary(agent_code)
                {
                    lp.note_done_notes(notes);
                }
                return Err(HostError(b.trim_end_matches('\n').to_owned()));
            }
        }
        if let Some(lp) = self.running_loop_for_done(workspace_id, agent_code)
            && lp.accepts_done_summary(agent_code)
        {
            lp.note_done_summary(summary, notes);
        }
        Ok(())
    }

    /// Host `update_finding`, attributed to the calling agent.
    pub fn update_finding(
        &self,
        workspace_id: &str,
        agent_code: &str,
        finding_id: &str,
        status: &str,
        reason: &str,
    ) -> Result<recur_domain::Finding, HostError> {
        let actor = self.finding_update_actor(workspace_id, agent_code);
        let f = self
            .store
            .update_finding_status(workspace_id, finding_id, status, reason, actor)
            .map_err(|e| HostError(e.to_string()))?;
        self.publish_findings_updated(workspace_id);
        Ok(f)
    }

    /// Host `report_result`: the Tester's pass/fail on the workspace.
    pub fn report_result(&self, workspace_id: &str, passed: bool, summary: &str) {
        self.store.update_workspace(workspace_id, |l| {
            l.test_result = Some(TestResult {
                passed,
                summary: summary.to_owned(),
                when: "now".into(),
            });
        });
    }

    /// Host `escalate`: recorded on the running walk, which blocks at its
    /// next checkpoint. Dropped when no walk runs.
    pub fn escalate(&self, workspace_id: &str, req: EscalationRequest) {
        if let Some(lp) = self.running_loop_by_id(workspace_id) {
            lp.request_escalation(req);
        }
    }

    /// Host `attach_evidence`: one verify step onto the running walk.
    pub fn attach_evidence(&self, workspace_id: &str, e: Evidence) {
        if let Some(lp) = self.running_loop_by_id(workspace_id) {
            lp.collect_evidence(e);
        }
    }
}

#[async_trait::async_trait]
impl Host for EngineHost {
    fn report_finding(&self, ws: &str, _agent: &str, f: Finding) -> recur_domain::Finding {
        match self.engine() {
            Some(e) => e.report_finding(ws, f),
            None => recur_domain::Finding::default(),
        }
    }

    fn done(&self, ws: &str, agent: &str, summary: &str, notes: &str) -> Result<(), HostError> {
        match self.engine() {
            Some(e) => e.done(ws, agent, summary, notes),
            None => Ok(()),
        }
    }

    fn list_findings(
        &self,
        ws: &str,
        _agent: &str,
        status: &str,
    ) -> Result<Vec<recur_domain::Finding>, HostError> {
        Ok(self
            .engine()
            .and_then(|e| e.store.findings_by_status(ws, status))
            .unwrap_or_default())
    }

    fn update_finding(
        &self,
        ws: &str,
        agent: &str,
        finding_id: &str,
        status: &str,
        reason: &str,
    ) -> Result<recur_domain::Finding, HostError> {
        match self.engine() {
            Some(e) => e.update_finding(ws, agent, finding_id, status, reason),
            None => Ok(recur_domain::Finding::default()),
        }
    }

    fn report_result(&self, ws: &str, _agent: &str, passed: bool, summary: &str) {
        if let Some(e) = self.engine() {
            e.report_result(ws, passed, summary);
        }
    }

    fn escalate(&self, ws: &str, _agent: &str, req: EscalationRequest) {
        if let Some(e) = self.engine() {
            e.escalate(ws, req);
        }
    }

    fn attach_evidence(&self, ws: &str, _agent: &str, ev: Evidence) {
        if let Some(e) = self.engine() {
            e.attach_evidence(ws, ev);
        }
    }
}

#[cfg(test)]
mod tests;
