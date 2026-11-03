//! Chat modes: the mode is a property of the chat session, not the workspace. A
//! mode shapes the turn's tools and prompt only; permissions are machine-global
//! (Settings), so Ask is read-only because it allowlists no edit tools, not
//! because it runs under a stricter mode.

use recur_agent_tools::names::mcp;
use recur_domain::{
    CHAT_MODE_ASK, CHAT_MODE_PLAN, CHAT_MODE_REVIEW, Settings, normalize_chat_mode,
};

use crate::composer::skills::{SKILLS_TOOLS_APPEND_PROMPT, join_append_prompt};
use crate::roles::{EDIT_TOOLS, GIT_READ_TOOLS, READ_EXPLORE_TOOLS, SKILL_MCP_TOOLS};

/// Ask-mode turns, appended to the provider's own system prompt. Read-only
/// means the git worktree; artifacts write outside it.
pub const ASK_APPEND_PROMPT: &str = "This chat is in ASK mode: read-only on the git worktree. You may read files, search, and run read-only git commands (diff/log/show). Do not modify repository files or attempt edits. Answer questions and explain code. If the user asks you to change the repository, tell them to switch this chat to Agent mode.";

/// Plan-mode turns: investigate read-only, produce a concrete plan.
pub const PLAN_APPEND_PROMPT: &str = "This chat is in PLAN mode: read-only on the git worktree. Investigate the repository (read files, search, read-only git) and produce a thorough, concrete implementation plan for what the user asks: specific files, functions, steps, edge cases, and a definition of done. Do not modify repository files: plan, don't edit.";

/// [`PLAN_APPEND_PROMPT`] with submit_plan wired: the detailed plan must be
/// filed on the tool, not left only in chat.
pub const PLAN_SUBMIT_APPEND_PROMPT: &str = "This chat is in PLAN mode: read-only on the git worktree. Investigate the repository (read files, search, read-only git) and produce a thorough, concrete implementation plan for what the user asks: specific files, functions, steps, edge cases, and a definition of done. Do not modify repository files: plan, don't edit. When the plan is ready, call mcp__recur__submit_plan with: summary (one or two sentence headline), body (the FULL detailed plan as markdown: relevant files, approach, edge cases, definition of done; other agents execute from body, not from this chat), steps (ordered checklist, each one actionable sentence), and attachments (ids of any create_artifact files or user images ⟦attachment:id⟧ that belong to the plan). Then give the user a readable version in chat. Prefer mcp__recur__submit_plan over any built-in create-plan tool: only submit_plan updates Recur's plan panel and is what later agents see.";

/// Read-only modes may create artifacts: they live under the workspace data
/// dir, not the git worktree.
pub const ARTIFACTS_OK_APPEND_PROMPT: &str = "You may create standalone artifacts (HTML reports, diagrams, tables) with mcp__recur__create_artifact: they are stored under the workspace data dir, not the git worktree.";
pub const ARTIFACTS_PREFER_ASK_APPEND_PROMPT: &str = "You may create standalone artifacts (HTML reports, diagrams, tables) with mcp__recur__create_artifact: they are stored under the workspace data dir, not the git worktree. Prefer an HTML artifact over dumping a large report into chat, then call mcp__recur__present_artifact.";
pub const ARTIFACTS_BESIDE_PLAN_APPEND_PROMPT: &str = "You may create standalone artifacts (HTML reports, diagrams, tables) with mcp__recur__create_artifact: they are stored under the workspace data dir, not the git worktree. Artifacts are in addition to, never instead of, mcp__recur__submit_plan.";
pub const ARTIFACTS_BESIDE_REVIEW_APPEND_PROMPT: &str = "You may create standalone artifacts (HTML reports, diagrams, tables) with mcp__recur__create_artifact: they are stored under the workspace data dir, not the git worktree. Artifacts are in addition to, never instead of, mcp__recur__report_finding.";

/// Agent-mode identity note. Ask/Plan/Review each lead with their mode;
/// without the same here, mode literacy's "flip the dock to Agent" clause
/// stalls models that think it applies to them.
pub const AGENT_APPEND_PROMPT: &str = "This chat is in AGENT mode: you can read and edit. Do not ask the user to switch to Agent: you already have edit tools. Proceed with the task. Strongly prefer child workspaces for anything beyond a very simple task (one-file tweak, one-line fix, short answer). Subagents in Recur are child workspaces: call mcp__recur__create_workspace (one child per task, an isolated worktree off this branch). Your turn does not wait for it: when the child finishes you get a new turn with its report, then read_child, merge_child (land its commits here), or archive_child (close it). mcp__recur__message_child (workspace_id + message) redirects a running child, or gives a finished (unmerged) child a new turn instead of replacing it. Native subagent, Task, or worktree tools are unavailable in Recur: do not look for them.";

/// Agent identity for a create_workspace child.
pub const AGENT_CHILD_APPEND_PROMPT: &str = "This chat is in AGENT mode: you can read and edit. Do not ask the user to switch to Agent: you already have edit tools. Proceed with the task. You are a child workspace: execute the assigned task here. Native subagent, Task, or worktree tools are unavailable in Recur: do not look for them.";

pub const AGENT_CHILD_NO_ASK_PROMPT: &str =
    "Do not call mcp__recur__ask_user: pick a reasonable default and finish the assigned task.";

/// Chat modes and switch_mode's privilege rules, plus ask_user restraint
/// (PA* chats only).
pub const MODE_LITERACY_APPEND_PROMPT: &str = "Chat modes: Ask (read-only Q&A), Plan (read-only → structured plan), Review (read-only review + findings), Agent (read+edit coding). The dock mode is user-controlled. You may call mcp__recur__switch_mode only within privilege rules: Agent may switch to ask/plan/review; ask/plan/review may switch among each other; never switch ask/plan/review up to Agent: ask the user to flip the dock instead. A successful switch_mode remounts this turn immediately with the new tools. Do not use Cursor's built-in SwitchMode, do not ask the user to switch, and do not keep working with the old toolset after the call. Prefer acting on reasonable defaults; call mcp__recur__ask_user only when blocked on a decision only the human can make (ends this turn; answer arrives later: not escalate). Never use Claude's AskUserQuestion: it cannot collect answers in Recur; use mcp__recur__ask_user.";

/// Review-mode turns: a read-only review that files structured findings.
pub const REVIEW_APPEND_PROMPT: &str = "This chat is in REVIEW mode: read-only on the git worktree. Review what the user points you at (default: the looking-at subject in the surface note: a worktree vs its base, or the open pull request). You may run Bash for lint/test/typecheck. For EACH real problem, a bug, a missing case, a regression, a quality issue, call the mcp__recur__report_finding tool with the file, line (optional end_line for a range), severity (info/low/medium/high/critical), a clear message, and optional summary (markdown shown to the end user in the turn digest; omit to use message). On a re-review, call mcp__recur__update_finding to mark a prior finding resolved (fixed) or invalid (wrong/out of scope: reason required). Do NOT restate or list filed findings in your closing reply: the app summarizes still-open filings for the user; keep closing prose brief (e.g. \"Filed N findings\" or \"No issues\"). If the code is sound, say so and file no findings. Do not modify repository files: review, don't fix.";

/// Recur tools on PA* chats (the Maestro has its own set). rename_branch and
/// open_pr are Agent-only (git / remote mutations); artifacts write outside
/// the worktree so they stay on every mode.
pub const INTERACTIVE_MCP_TOOLS: [&str; 10] = [
    mcp::ASK_USER,
    mcp::SWITCH_MODE,
    mcp::CREATE_ARTIFACT,
    mcp::LIST_ARTIFACTS,
    mcp::READ_ARTIFACT,
    mcp::PRESENT_ARTIFACT,
    mcp::RENAME_SESSION,
    mcp::RENAME_WORKSPACE,
    mcp::LIST_TERMINALS,
    mcp::READ_TERMINAL,
];

fn own(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).to_owned()).collect()
}

/// The read-only chat set: explore + read-only git. No edits, no bare Bash.
pub fn ask_tools() -> Vec<String> {
    let mut v = own(&READ_EXPLORE_TOOLS);
    v.extend(own(&GIT_READ_TOOLS));
    v
}

/// Ask's set plus bare Bash for lint/test (the scoped git rules stay so
/// those reads are pre-approved).
pub fn review_tools() -> Vec<String> {
    let mut v = ask_tools();
    v.push("Bash".into());
    v
}

/// The workspace coding agent's base set: explore + edit.
pub fn pair_tools() -> Vec<String> {
    let mut v = own(&READ_EXPLORE_TOOLS);
    v.extend(own(&EDIT_TOOLS));
    v
}

/// `tools` without `name`.
pub fn omit_tool(tools: &[&str], name: &str) -> Vec<String> {
    tools
        .iter()
        .filter(|t| **t != name)
        .map(|t| (*t).to_owned())
        .collect()
}

/// How one chat mode shapes an agent turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatTurnShape {
    pub tools: Vec<String>,
    /// The turn's permission mode.
    pub permission: String,
    /// Appended to the provider's own system prompt.
    pub append_prompt: String,
    /// Wire the per-agent Recur MCP endpoint into the turn.
    pub mcp: bool,
}

/// Maps a chat mode onto its turn shape. The permission mode is the same
/// for every mode. Loop never reaches here. `child` is a workspace with a
/// parent (no ask_user, no workspace fan-out tools).
pub fn chat_mode_shape(mode: &str, set: &Settings, child: bool) -> ChatTurnShape {
    let permission = set.permission_mode().to_owned();
    let interactive = if child {
        omit_tool(&INTERACTIVE_MCP_TOOLS, mcp::ASK_USER)
    } else {
        own(&INTERACTIVE_MCP_TOOLS)
    };
    let skills = own(&SKILL_MCP_TOOLS);
    let triple = |a: &str, b: &str| {
        join_append_prompt(
            &join_append_prompt(&join_append_prompt(a, b), SKILLS_TOOLS_APPEND_PROMPT),
            MODE_LITERACY_APPEND_PROMPT,
        )
    };
    match normalize_chat_mode(mode) {
        CHAT_MODE_ASK => {
            let mut tools = ask_tools();
            tools.push(mcp::LIST_FINDINGS.into());
            tools.extend(skills);
            tools.extend(interactive);
            ChatTurnShape {
                tools,
                permission,
                append_prompt: triple(ASK_APPEND_PROMPT, ARTIFACTS_PREFER_ASK_APPEND_PROMPT),
                mcp: true,
            }
        }
        CHAT_MODE_PLAN => {
            let mut tools = ask_tools();
            tools.extend(own(&[mcp::SUBMIT_PLAN, mcp::LIST_FINDINGS]));
            tools.extend(skills);
            tools.extend(interactive);
            ChatTurnShape {
                tools,
                permission,
                append_prompt: triple(
                    PLAN_SUBMIT_APPEND_PROMPT,
                    ARTIFACTS_BESIDE_PLAN_APPEND_PROMPT,
                ),
                mcp: true,
            }
        }
        CHAT_MODE_REVIEW => {
            let mut tools = review_tools();
            tools.extend(own(&[
                mcp::REPORT_FINDING,
                mcp::LIST_FINDINGS,
                mcp::UPDATE_FINDING,
            ]));
            tools.extend(skills);
            tools.extend(interactive);
            ChatTurnShape {
                tools,
                permission,
                append_prompt: triple(REVIEW_APPEND_PROMPT, ARTIFACTS_BESIDE_REVIEW_APPEND_PROMPT),
                mcp: true,
            }
        }
        // Agent: coding chat + the findings checklist (list/update). No done
        // (a loop-specialist signal) and no findings note (models then call
        // list_findings every turn). The identity note must lead.
        _ => {
            let mut tools = pair_tools();
            tools.extend(own(&[mcp::LIST_FINDINGS, mcp::UPDATE_FINDING]));
            tools.extend(skills);
            tools.extend(interactive);
            tools.push(mcp::RENAME_BRANCH.into());
            let append_prompt = if child {
                join_append_prompt(
                    &join_append_prompt(
                        &join_append_prompt(AGENT_CHILD_APPEND_PROMPT, SKILLS_TOOLS_APPEND_PROMPT),
                        MODE_LITERACY_APPEND_PROMPT,
                    ),
                    AGENT_CHILD_NO_ASK_PROMPT,
                )
            } else {
                tools.extend(own(&[
                    mcp::OPEN_PR,
                    mcp::CREATE_WORKSPACE,
                    mcp::MERGE_CHILD,
                    mcp::READ_CHILD,
                    mcp::ARCHIVE_CHILD,
                    mcp::MESSAGE_CHILD,
                ]));
                join_append_prompt(
                    &join_append_prompt(AGENT_APPEND_PROMPT, SKILLS_TOOLS_APPEND_PROMPT),
                    MODE_LITERACY_APPEND_PROMPT,
                )
            };
            ChatTurnShape {
                tools,
                permission,
                append_prompt,
                mcp: true,
            }
        }
    }
}
