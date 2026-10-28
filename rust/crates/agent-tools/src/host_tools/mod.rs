//! Recur's host tools: one engine callback each, with the name, description,
//! JSON schema and result text agents rely on (as Go; JSON results are
//! compatible, not byte-identical).

mod artifacts;
mod briefing;
mod chat;
mod children;
mod findings;
mod hands;
mod loop_flow;
mod plans;
mod skills;
mod terminals;

pub use artifacts::{CreateArtifactTool, ListArtifactsTool, PresentArtifactTool, ReadArtifactTool};
pub use briefing::{GetWorkspaceContextTool, ListWorkspaceBriefsTool, PresentWorkspaceTool};
pub use chat::{
    AskUserTool, OpenPrTool, PermissionRequestTool, RenameBranchTool, RenameSessionTool,
    RenameWorkspaceTool, SwitchModeTool, marshal_permission_verdict,
};
pub use children::{
    ArchiveChildTool, CreateWorkspaceTool, MergeChildTool, MessageChildTool, ReadChildTool,
};
pub use findings::{ListFindingsTool, ReportFindingTool, UpdateFindingTool};
pub use hands::{
    DispatchSpecialistTool, EditPlanTool, ExtendIterationsTool, ExtendRoundsTool, ReenterPhaseTool,
    RerunReviewTool, ResumeLoopTool, RunPhaseTool,
};
pub use loop_flow::{
    AttachEvidenceTool, DoneTool, EscalateTool, PlanTool, ReportResultTool, SendBackTool,
};
pub use plans::{SubmitPlanTool, UpdatePlanStepTool};
pub use skills::{ActivateSkillTool, ListSkillsTool};
pub use terminals::{ListTerminalsTool, ReadTerminalTool};

use recur_domain::Severity;
use serde::Serialize;

use crate::json::{Obj, Shape, strict_unmarshal};
use crate::tool::ToolError;

pub(crate) const EMPTY_OBJECT_SCHEMA: &str = r#"{"type":"object","properties":{}}"#;

/// Go `schemaSummaryOnly`.
pub(crate) fn schema_summary_only(field: &str) -> String {
    format!(
        "{{\n  \"type\": \"object\",\n  \"properties\": {{\"{field}\": {{\"type\": \"string\", \"description\": \"A short summary\"}}}},\n  \"required\": [\"{field}\"]\n}}"
    )
}

pub(crate) fn strict(args: &str, shape: &'static Shape) -> Result<Obj, ToolError> {
    strict_unmarshal(args, shape).map_err(ToolError)
}

pub(crate) fn marshal<T: Serialize + ?Sized>(v: &T) -> Result<String, ToolError> {
    serde_json::to_string(v).map_err(|e| ToolError(e.to_string()))
}

pub(crate) fn xml_escape_catalog(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(crate) fn parse_severity(s: &str) -> Option<Severity> {
    let sev = Severity::from(crate::gostr::to_lower(s.trim()).as_str());
    sev.is_known().then_some(sev)
}

pub(crate) fn is_zero(n: &i64) -> bool {
    *n == 0
}
