//! The engine seam: every Recur host tool call becomes one typed call here,
//! attributed to a workspace and agent. This is the single path structured
//! signals re-enter the engine; tests drive findings through the same interface
//! the live MCP handler calls.
//!
//! Default bodies do nothing, so an engine slice that has not
//! ported a method yet behaves exactly like a Recur with no engine attached.
//! Implementations must be safe for concurrent use: MCP requests from agent
//! processes run concurrently with the loop task.

use recur_domain::{
    AgentSkill, Artifact, BriefingSnapshot, Finding as StoredFinding, Severity, WorkspaceContext,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

/// A host failure. Its text is the tool result the agent reads.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct HostError(pub String);

impl HostError {
    pub fn new(msg: impl Into<String>) -> Self {
        HostError(msg.into())
    }
}

/// One Observatory terminal as advertised to the agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    pub title: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub shell: String,
    pub active: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub exited: bool,
    /// Set when exited, including 0 (clean exit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
}

/// A submit_plan payload. MCP callers omit `step_ids` (the engine assigns
/// S1…Sn); the Plan panel passes ids aligned with `steps` so a retitle keeps
/// status and note.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanSubmission {
    pub summary: String,
    pub body: String,
    pub steps: Vec<String>,
    pub step_ids: Vec<String>,
    pub attachment_ids: Vec<String>,
    /// Aligned with `steps`.
    pub step_meta: Vec<PlanStepMeta>,
}

/// Optional per-step detail a planner may file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanStepMeta {
    pub files: Vec<String>,
    pub depends_on: Vec<String>,
    /// The object form ({title, files, depends_on}): its lists are
    /// authoritative even when empty. A bare string title leaves them unset.
    pub explicit: bool,
}

/// The create_workspace tool input. Empty `project_id` means the caller's
/// project, empty `base_branch` the project default, empty `mode` agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateWorkspaceRequest {
    pub project_id: String,
    pub title: String,
    pub prompt: String,
    pub base_branch: String,
    pub mode: String,
    pub loop_id: String,
    pub policy_id: String,
    /// `None` = default: a child of the caller unless the caller is a
    /// scheduled run, mode is loop, or `base_branch` is set.
    pub standalone: Option<bool>,
}

/// An escalate payload: a scoped question, reasoned options, one-read context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EscalationRequest {
    pub question: String,
    pub options: Vec<EscalationOption>,
    pub context: String,
}

/// An ask_user payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AskUserRequest {
    pub question: String,
    pub options: Vec<EscalationOption>,
    pub context: String,
}

/// A create_artifact payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateArtifactRequest {
    pub title: String,
    pub kind: String,
    pub content: String,
}

/// One reasoned choice: `tag` is a short label (simplest, recommended, …),
/// `desc` the trade-off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EscalationOption {
    pub text: String,
    pub tag: String,
    pub desc: String,
}

/// A report_finding payload. No engine identity or iteration: the engine
/// stamps those so an agent cannot forge them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Finding {
    pub file: String,
    /// 1-based start; 0 if not line-specific.
    pub line: i64,
    /// Inclusive end; 0 means a single line.
    pub end_line: i64,
    pub severity: Severity,
    pub message: String,
    /// Optional user-facing digest; empty means use `message`.
    pub summary: String,
}

/// One browser-verification step from attach_evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evidence {
    pub step: String,
    pub screenshot_path: String,
    pub passed: bool,
}

/// A permission_request: the tool the CLI wants to run, its arguments as raw
/// JSON (echoed back untouched on approval) and the correlating tool_use id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionAsk {
    pub tool: String,
    pub input: String,
    pub tool_use_id: String,
}

/// The answer to a [`PermissionAsk`]. `reason` reaches the model on refusal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionVerdict {
    pub allow: bool,
    pub reason: String,
}

/// The engine callback seam. Sync methods complete before the tool result is
/// written (report_finding returns the stored row; note_tool_args runs before
/// the handler).
#[async_trait::async_trait]
pub trait Host: Send + Sync {
    // ---- Loop flow

    /// Records one reviewer finding for the loop's current iteration and
    /// returns the stored row (with its id) for the tool result.
    fn report_finding(&self, _ws: &str, _agent: &str, _f: Finding) -> StoredFinding {
        StoredFinding::default()
    }

    /// A Coder/Tester turn finished. Err keeps the turn going (open findings
    /// remain); the engine still decides convergence after re-review.
    fn done(&self, _ws: &str, _agent: &str, _summary: &str, _notes: &str) -> Result<(), HostError> {
        Ok(())
    }

    /// Persisted findings, filtered by status (open|resolved|ignored|invalid|all).
    fn list_findings(
        &self,
        _ws: &str,
        _agent: &str,
        _status: &str,
    ) -> Result<Vec<StoredFinding>, HostError> {
        Ok(Vec::new())
    }

    /// Sets one finding's status (resolved|invalid; invalid needs a reason).
    fn update_finding(
        &self,
        _ws: &str,
        _agent: &str,
        _finding_id: &str,
        _status: &str,
        _reason: &str,
    ) -> Result<StoredFinding, HostError> {
        Ok(StoredFinding::default())
    }

    /// A Tester's pass/fail result.
    fn report_result(&self, _ws: &str, _agent: &str, _passed: bool, _summary: &str) {}

    /// A Maestro/agent plan summary.
    fn plan(&self, _ws: &str, _agent: &str, _summary: &str) {}

    /// Raises an escalation; the loop blocks at its next checkpoint, not here.
    fn escalate(&self, _ws: &str, _agent: &str, _e: EscalationRequest) {}

    /// One QA evidence step for the current verify run.
    fn attach_evidence(&self, _ws: &str, _agent: &str, _e: Evidence) {}

    /// The verbatim tools/call arguments for transcript backfill (empty when
    /// the call carried none). Must not block.
    fn note_tool_args(&self, _ws: &str, _agent: &str, _tool: &str, _args: &str) {}

    // ---- Chat

    /// Parks a clarifying question and returns at once (interactive agents only).
    async fn ask_user(&self, _ws: &str, _agent: &str, _req: AskUserRequest) -> String {
        "ask_user unavailable".into()
    }

    /// Changes the calling chat's mode (privilege-safe).
    async fn switch_mode(&self, _ws: &str, _agent: &str, _mode: &str) -> String {
        "ignored".into()
    }

    async fn create_artifact(
        &self,
        _ws: &str,
        _agent: &str,
        _req: CreateArtifactRequest,
    ) -> Result<Artifact, HostError> {
        Err(HostError::new("no artifacts"))
    }

    async fn list_artifacts(&self, _ws: &str, _agent: &str) -> Result<Vec<Artifact>, HostError> {
        Ok(Vec::new())
    }

    /// One artifact's metadata and text body (PNG/JPEG flagged, not inlined).
    async fn read_artifact(
        &self,
        _ws: &str,
        _agent: &str,
        _artifact_id: &str,
    ) -> Result<String, HostError> {
        Err(HostError::new("no artifacts"))
    }

    async fn present_artifact(&self, _ws: &str, _agent: &str, _artifact_id: &str) -> String {
        "ignored".into()
    }

    /// Sets the chat title. `owner_chat` is the calling turn's conductor tab
    /// when known; empty means infer from the running walk.
    async fn rename_session(
        &self,
        _ws: &str,
        _agent: &str,
        _owner_chat: &str,
        _title: &str,
    ) -> String {
        "ignored".into()
    }

    async fn rename_workspace(&self, _ws: &str, _agent: &str, _title: &str) -> String {
        "ignored".into()
    }

    async fn rename_branch(&self, _ws: &str, _agent: &str, _name: &str) -> String {
        "ignored".into()
    }

    async fn open_pr(
        &self,
        _ws: &str,
        _agent: &str,
        _title: &str,
        _body: &str,
        _draft: bool,
    ) -> String {
        "ignored".into()
    }

    async fn create_workspace(
        &self,
        _ws: &str,
        _agent: &str,
        _req: CreateWorkspaceRequest,
    ) -> String {
        "ignored".into()
    }

    async fn merge_child(&self, _ws: &str, _agent: &str, _child_id: &str) -> String {
        String::new()
    }

    async fn read_child(&self, _ws: &str, _agent: &str, _child_id: &str) -> String {
        String::new()
    }

    async fn archive_child(&self, _ws: &str, _agent: &str, _child_id: &str) -> String {
        String::new()
    }

    async fn message_child(
        &self,
        _ws: &str,
        _agent: &str,
        _child_id: &str,
        _message: &str,
    ) -> String {
        String::new()
    }

    async fn read_child_artifact(
        &self,
        _ws: &str,
        _agent: &str,
        _child_id: &str,
        _artifact_id: &str,
    ) -> Result<String, HostError> {
        Ok(String::new())
    }

    /// Asks the human whether this agent may run one tool and waits for the
    /// answer. The only long-blocking call: it must return when `cancel` fires
    /// (client disconnected or turn ended) and impose its own timeout. Err only
    /// when no verdict could be reached; a refusal is a verdict.
    async fn request_permission(
        &self,
        _cancel: &CancellationToken,
        _ws: &str,
        _agent: &str,
        _ask: PermissionAsk,
    ) -> Result<PermissionVerdict, HostError> {
        Ok(PermissionVerdict {
            allow: false,
            reason: "no permission broker is attached".into(),
        })
    }

    // ---- Conductor hands: the agent requests, the engine validates and
    // returns success or the named refusal.

    async fn send_back(&self, _ws: &str, _agent: &str, _instruction: &str) -> String {
        "ignored".into()
    }

    async fn edit_plan(&self, _ws: &str, _agent: &str, _plan: &str) -> String {
        "ignored".into()
    }

    async fn rerun_review(&self, _ws: &str, _agent: &str) -> String {
        "ignored".into()
    }

    async fn dispatch_specialist(
        &self,
        _ws: &str,
        _agent: &str,
        _role: &str,
        _instruction: &str,
    ) -> String {
        "ignored".into()
    }

    async fn extend_iterations(&self, _ws: &str, _agent: &str, _n: i64) -> String {
        "ignored".into()
    }

    async fn extend_rounds(&self, _ws: &str, _agent: &str, _n: i64) -> String {
        "ignored".into()
    }

    async fn reenter_phase(&self, _ws: &str, _agent: &str, _phase_id: &str) -> String {
        "ignored".into()
    }

    async fn run_phase(&self, _ws: &str, _agent: &str, _phase_id: &str) -> String {
        "ignored".into()
    }

    async fn resume_loop(&self, _ws: &str, _agent: &str) -> String {
        "ignored".into()
    }

    // ---- Plans

    async fn submit_plan(&self, _ws: &str, _agent: &str, _p: PlanSubmission) -> String {
        "ignored".into()
    }

    async fn update_plan_step(
        &self,
        _ws: &str,
        _agent: &str,
        _step_id: &str,
        _status: &str,
        _note: &str,
    ) -> String {
        "ignored".into()
    }

    // ---- Skills, terminals

    /// Skills with disable-model-invocation are omitted.
    async fn list_skills(&self, _ws: &str, _agent: &str) -> Result<Vec<AgentSkill>, HostError> {
        Ok(Vec::new())
    }

    async fn activate_skill(
        &self,
        _ws: &str,
        _agent: &str,
        _name: &str,
    ) -> Result<String, HostError> {
        Err(HostError::new("no skills"))
    }

    /// `None` serializes as `null`.
    async fn list_terminals(&self, _ws: &str, _agent: &str) -> Option<Vec<TerminalInfo>> {
        None
    }

    /// ANSI-stripped recent output. Empty id reads the focused tab;
    /// `tail_lines` 0 means the default tail.
    async fn read_terminal(
        &self,
        _ws: &str,
        _agent: &str,
        _id: &str,
        _tail_lines: i64,
    ) -> Result<String, HostError> {
        Err(HostError::new("no terminals"))
    }

    // ---- Ask Recur (app coordinator only)

    async fn list_workspace_briefs(&self) -> BriefingSnapshot {
        BriefingSnapshot::default()
    }

    async fn workspace_context(&self, _id: &str) -> Result<WorkspaceContext, HostError> {
        Err(HostError::new("no briefing"))
    }

    /// Validates a target and returns a JSON card payload.
    async fn present_workspace(&self, _target: &str) -> String {
        "ignored".into()
    }
}

/// Every method keeps its default body.
pub struct NoopHost;

impl Host for NoopHost {}
