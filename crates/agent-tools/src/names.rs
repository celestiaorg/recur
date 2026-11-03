//! Recur host tool names. The bare names are what MCP `tools/list` advertises;
//! [`mcp`] holds the `mcp__recur__<name>` forms runner allowlists use
//! (`--allowedTools`). acceptEdits/dontAsk do not auto-approve MCP tools, so
//! each role that needs one lists it explicitly.

/// How runner allowlists name Recur host tools.
pub const RECUR_MCP_PREFIX: &str = "mcp__recur__";

macro_rules! tool_names {
    ($($ident:ident = $name:literal),* $(,)?) => {
        $(pub const $ident: &str = $name;)*

        /// `mcp__recur__<name>` allowlist names.
        pub mod mcp {
            $(pub const $ident: &str = concat!("mcp__recur__", $name);)*
        }
    };
}

tool_names! {
    REPORT_FINDING = "report_finding",
    LIST_FINDINGS = "list_findings",
    UPDATE_FINDING = "update_finding",
    DONE = "done",
    REPORT_RESULT = "report_result",
    PLAN = "plan",
    ESCALATE = "escalate",
    ATTACH_EVIDENCE = "attach_evidence",
    SEND_BACK = "send_back",
    // Maestro hands
    EDIT_PLAN = "edit_plan",
    RERUN_REVIEW = "rerun_review",
    DISPATCH_SPECIALIST = "dispatch_specialist",
    EXTEND_ITERATIONS = "extend_iterations",
    EXTEND_ROUNDS = "extend_rounds",
    REENTER_PHASE = "reenter_phase",
    RUN_PHASE = "run_phase",
    RESUME_LOOP = "resume_loop",
    // Structured plan capture
    SUBMIT_PLAN = "submit_plan",
    UPDATE_PLAN_STEP = "update_plan_step",
    // Claude Code calls this itself for --permission-prompt-tool, so it is never
    // in a role's --allowedTools.
    PERMISSION_REQUEST = "permission_request",
    // Agent Skills progressive disclosure
    LIST_SKILLS = "list_skills",
    ACTIVATE_SKILL = "activate_skill",
    ASK_USER = "ask_user",
    SWITCH_MODE = "switch_mode",
    CREATE_ARTIFACT = "create_artifact",
    LIST_ARTIFACTS = "list_artifacts",
    READ_ARTIFACT = "read_artifact",
    PRESENT_ARTIFACT = "present_artifact",
    RENAME_SESSION = "rename_session",
    RENAME_WORKSPACE = "rename_workspace",
    RENAME_BRANCH = "rename_branch",
    OPEN_PR = "open_pr",
    CREATE_WORKSPACE = "create_workspace",
    MERGE_CHILD = "merge_child",
    READ_CHILD = "read_child",
    ARCHIVE_CHILD = "archive_child",
    MESSAGE_CHILD = "message_child",
    LIST_TERMINALS = "list_terminals",
    READ_TERMINAL = "read_terminal",
    // Ask Recur coordinator
    LIST_WORKSPACE_BRIEFS = "list_workspace_briefs",
    GET_WORKSPACE_CONTEXT = "get_workspace_context",
    PRESENT_WORKSPACE = "present_workspace",
}
