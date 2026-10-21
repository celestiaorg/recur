//! The tool sets. `host_tools()` order is the MCP `tools/list` order.

use std::sync::Arc;

use crate::host_tools::*;
use crate::tool::Tool;

/// Recur's host tools. MCP wraps `execute`; the compatible harness calls the
/// same tools in-process. Coding tools are not included.
pub fn host_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReportFindingTool),
        Arc::new(ListFindingsTool),
        Arc::new(UpdateFindingTool),
        Arc::new(DoneTool),
        Arc::new(ReportResultTool),
        Arc::new(PlanTool),
        Arc::new(EscalateTool),
        Arc::new(AttachEvidenceTool),
        Arc::new(EditPlanTool),
        Arc::new(RerunReviewTool),
        Arc::new(DispatchSpecialistTool),
        Arc::new(ExtendIterationsTool),
        Arc::new(ExtendRoundsTool),
        Arc::new(ReenterPhaseTool),
        Arc::new(RunPhaseTool),
        Arc::new(ResumeLoopTool),
        Arc::new(SubmitPlanTool),
        Arc::new(UpdatePlanStepTool),
        Arc::new(SendBackTool),
        Arc::new(PermissionRequestTool),
        Arc::new(ListSkillsTool),
        Arc::new(ActivateSkillTool),
        Arc::new(AskUserTool),
        Arc::new(SwitchModeTool),
        Arc::new(CreateArtifactTool),
        Arc::new(ListArtifactsTool),
        Arc::new(ReadArtifactTool),
        Arc::new(PresentArtifactTool),
        Arc::new(ListWorkspaceBriefsTool),
        Arc::new(GetWorkspaceContextTool),
        Arc::new(PresentWorkspaceTool),
        Arc::new(RenameSessionTool),
        Arc::new(RenameWorkspaceTool),
        Arc::new(RenameBranchTool),
        Arc::new(OpenPrTool),
        Arc::new(CreateWorkspaceTool),
        Arc::new(MergeChildTool),
        Arc::new(ReadChildTool),
        Arc::new(ArchiveChildTool),
        Arc::new(MessageChildTool),
        Arc::new(ListTerminalsTool),
        Arc::new(ReadTerminalTool),
    ]
}

/// The native-harness file/shell/todo set (not registered on MCP). Glob and
/// Grep need `recur-code`'s ripgrep, hence the `code` feature.
#[cfg(feature = "code")]
pub fn coding_tools() -> Vec<Arc<dyn Tool>> {
    use crate::coding::{
        bash::BashTool, edit::EditTool, glob::GlobTool, grep::GrepTool, read::ReadTool,
        todos::TodoTool, write::WriteTool,
    };
    vec![
        Arc::new(ReadTool),
        Arc::new(EditTool),
        Arc::new(WriteTool),
        Arc::new(GlobTool),
        Arc::new(GrepTool),
        Arc::new(BashTool),
        Arc::new(TodoTool),
    ]
}
