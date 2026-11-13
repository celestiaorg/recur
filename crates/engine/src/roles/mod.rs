//! The app-owned agent roles (PRD §8.2): scoped tool sets, avatar codes, and
//! every specialist and Maestro prompt. The prompt text is a contract (agents,
//! stored transcripts and tests read it).

mod tools;

pub use tools::{
    loop_flow_tools, map_role_tool, mode_tools, model_for_slot, role_allowlist,
    role_wants_playwright, slot_role_spec,
};

use recur_agent_tools::names::mcp;
use recur_domain::{AgentRole, Finding, FindingStatus, finding_loc};
use recur_store::Store;

use crate::text::truncate;

/// One app-owned role: tool scope, avatar code, display name, and the
/// `[[role]]` model slot / mode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleSpec {
    pub role: AgentRole,
    /// 2-letter avatar (MA/CO/RV/TE/QA/PR).
    pub code: String,
    pub name: String,
    /// The `--allowedTools` scope.
    pub allowed_tools: Vec<String>,
    /// `[[role]].model_slot` (ask|plan|agent|review|loop).
    pub model_slot: String,
    /// `[[role]].mode`; the model slot defaults from it.
    pub mode: String,
}

/// The Playwright MCP server, allowed whole by wildcard for the QA role.
pub const MCP_PLAYWRIGHT_ALL: &str = "mcp__playwright__*";

/// Read/explore built-ins every role gets.
pub const READ_EXPLORE_TOOLS: [&str; 6] = ["Read", "Glob", "Grep", "LSP", "WebFetch", "WebSearch"];
/// Edit built-ins (never on read-only roles).
pub const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "Bash", "NotebookEdit"];
/// Read-only git Bash rules, in both permission-rule dialects so whichever
/// the installed CLI honors pre-approves them.
pub const GIT_READ_TOOLS: [&str; 6] = [
    "Bash(git diff:*)",
    "Bash(git diff *)",
    "Bash(git log:*)",
    "Bash(git log *)",
    "Bash(git show:*)",
    "Bash(git show *)",
];
/// Agent Skills progressive disclosure.
pub const SKILL_MCP_TOOLS: [&str; 2] = [mcp::LIST_SKILLS, mcp::ACTIVATE_SKILL];

fn strings(parts: &[&[&str]]) -> Vec<String> {
    parts
        .iter()
        .flat_map(|p| p.iter().map(|s| (*s).to_owned()))
        .collect()
}

/// The conductor allowlist: mode=loop plus ask_user. Retired hands
/// (rerun_review / resume_loop / extend_iterations) stay implemented for old
/// calls but are not allowed.
pub fn maestro_tools() -> Vec<String> {
    let mut t = mode_tools(recur_bundles::ROLE_MODE_LOOP);
    t.push(mcp::ASK_USER.to_owned());
    t
}

/// The Maestro planning turn's tools: read-only investigation plus
/// escalate and the structured plan capture. No Edit/Write/Bash.
pub fn maestro_plan_tools() -> Vec<String> {
    strings(&[&[
        "Read",
        "Grep",
        "Glob",
        mcp::ESCALATE,
        mcp::SUBMIT_PLAN,
        mcp::LIST_FINDINGS,
        mcp::CREATE_ARTIFACT,
        mcp::LIST_ARTIFACTS,
        mcp::READ_ARTIFACT,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
    ]])
}

/// The static role table. Roles outside it yield the zero spec.
pub fn role_spec(role: &AgentRole) -> RoleSpec {
    match role {
        AgentRole::Coder => RoleSpec {
            role: AgentRole::Coder,
            code: "CO".into(),
            name: "Coder".into(),
            allowed_tools: strings(&[
                &READ_EXPLORE_TOOLS,
                &EDIT_TOOLS,
                &[
                    mcp::DONE,
                    mcp::LIST_FINDINGS,
                    mcp::UPDATE_FINDING,
                    mcp::UPDATE_PLAN_STEP,
                    mcp::ESCALATE,
                    mcp::CREATE_ARTIFACT,
                    mcp::LIST_ARTIFACTS,
                    mcp::READ_ARTIFACT,
                    mcp::PRESENT_ARTIFACT,
                ],
                &SKILL_MCP_TOOLS,
            ]),
            ..RoleSpec::default()
        },
        AgentRole::Reviewer => RoleSpec {
            role: AgentRole::Reviewer,
            code: "RV".into(),
            name: "Reviewer".into(),
            allowed_tools: reviewer_tools(),
            ..RoleSpec::default()
        },
        AgentRole::Tester => RoleSpec {
            role: AgentRole::Tester,
            code: "TE".into(),
            name: "Tester".into(),
            allowed_tools: strings(&[
                &READ_EXPLORE_TOOLS,
                &EDIT_TOOLS,
                &[mcp::REPORT_RESULT, mcp::LIST_FINDINGS],
                &SKILL_MCP_TOOLS,
            ]),
            ..RoleSpec::default()
        },
        AgentRole::Qa => RoleSpec {
            role: AgentRole::Qa,
            code: "QA".into(),
            name: "QA / Browser".into(),
            allowed_tools: role_allowlist(&first_party_qa_role()),
            ..RoleSpec::default()
        },
        _ => RoleSpec::default(),
    }
}

fn reviewer_tools() -> Vec<String> {
    strings(&[
        &READ_EXPLORE_TOOLS,
        &GIT_READ_TOOLS,
        &[
            "Bash",
            mcp::REPORT_FINDING,
            mcp::LIST_FINDINGS,
            mcp::UPDATE_FINDING,
        ],
        &SKILL_MCP_TOOLS,
    ])
}

/// The rail #5 extra reviewer: a distinct code (SR) so reviewer ≠ coder
/// holds structurally, with the Reviewer's read-only tools.
pub fn security_reviewer_role() -> RoleSpec {
    RoleSpec {
        role: AgentRole::Reviewer,
        code: "SR".into(),
        name: "Security Reviewer".into(),
        allowed_tools: reviewer_tools(),
        ..RoleSpec::default()
    }
}

/// The QA role's bundle shape (ask mode plus evidence/Playwright/result).
pub fn first_party_qa_role() -> recur_bundles::Role {
    recur_bundles::Role {
        mode: recur_bundles::ROLE_MODE_ASK.into(),
        tools: vec![
            "attach_evidence".into(),
            "playwright".into(),
            "report_result".into(),
        ],
        ..recur_bundles::Role::default()
    }
}

pub fn security_reviewer_system_prompt(task: &str) -> String {
    format!(
        "{}{task}",
        concat!(
            "You are an extra Security Reviewer in an autonomous coding loop. The diff ",
            "touches authentication or secret-handling code. Review it READ-ONLY (Read, Grep, ",
            "`git diff`) with a security lens: hardcoded credentials/secrets, weak token ",
            "handling, missing auth checks, secrets logged or committed, insecure defaults. ",
            "For EACH real issue call mcp__recur__report_finding with file, line (and optional ",
            "end_line for a range), severity, a clear message, and optional summary (markdown ",
            "for the end-user digest). Do NOT restate or list filed findings in your closing ",
            "reply: the app summarizes still-open filings. If the security-sensitive changes ",
            "are sound, report no findings.\n\nTask: ",
        )
    )
}

pub fn qa_system_prompt(task: &str, app_url: &str, shot_dir: &str) -> String {
    format!(
        "You are the QA / Browser agent in an autonomous coding loop. The app is \
running and reachable at {app_url}. Using the Playwright browser tools, \
drive the app like a real user to verify the change accomplishes the task's \
acceptance criteria. For EACH meaningful step: take a screenshot saved under \
{shot_dir}, then call mcp__recur__attach_evidence with a one-line description \
of what you did/observed, the screenshot_path, and passed=true if the step \
behaved correctly (passed=false if it did not). Do not modify any source files. \
When you have exercised the acceptance criteria, stop.\n\nTask to verify: {task}"
    )
}

/// The URL / screenshot / evidence handshake appended when a role prompt
/// replaces [`qa_system_prompt`].
pub fn qa_runtime_extra(app_url: &str, shot_dir: &str) -> String {
    format!(
        "\n\nThe app is running and reachable at {app_url}. For each meaningful step: \
take a screenshot saved under {shot_dir}, then call mcp__recur__attach_evidence \
with a one-line description of what you did/observed, the screenshot_path, and \
passed=true if the step behaved correctly (passed=false if it did not). When you \
have exercised the acceptance criteria, call mcp__recur__report_result and stop."
    )
}

/// The Maestro's plan as a block for a Coder prompt ("" when none). The
/// phrasing is the same on every pass so a resume cache prefix stays stable.
pub fn plan_section(plan: &str) -> String {
    let plan = plan.trim();
    if plan.is_empty() {
        return String::new();
    }
    format!(
        "\n\nThe Maestro investigated the repo and prepared this plan. Follow it on the first \
coding pass; on a fix or rework turn the reviewer findings or rework instruction in \
your task message take precedence: the plan is background context:\n\n{plan}"
    )
}

pub fn coder_system_prompt(task: &str, plan: &str, structured: bool) -> String {
    let step_note = if structured {
        " The plan's steps are numbered; as you complete (or deliberately skip) each one, \
call mcp__recur__update_plan_step with its id (S1, S2, …) so progress stays honest."
    } else {
        ""
    };
    format!(
        "{}{step_note}\n\nTask: {task}{}",
        concat!(
            "You are the Coder agent in an autonomous coding loop. You are working in ",
            "an isolated git worktree. Make the minimal, correct code changes needed to ",
            "accomplish the task below (Read/Glob/Grep/LSP/Edit/Write/Bash; WebFetch/",
            "WebSearch for docs when needed). Do not commit: the orchestrator commits ",
            "your changes. When fixing reviewer findings, mark each with ",
            "mcp__recur__update_finding (resolved, or invalid with a reason) before done: ",
            "mcp__recur__list_findings lists what is still open. No TODOs, no stubs, no partial ",
            "implementations, no disabled tests: finish the work or hand off honestly. When the change ",
            "is complete and no findings remain open, call mcp__recur__done with a one-line summary: ",
            "and notes ONLY when something deviated: what you did differently from the plan, what you ",
            "could not finish and why, a step that should be split, or a concern the Reviewer cannot ",
            "see from the diff. Then stop.",
        ),
        plan_section(plan)
    )
}

fn finding_line(i: usize, id: &str, f: &Finding) -> String {
    format!(
        "{}. id={id} [{}] {}: {}\n",
        i + 1,
        f.severity.as_str(),
        finding_loc(&f.file, f.line, f.end_line),
        f.message
    )
}

pub fn coder_fix_prompt(task: &str, findings: &[Finding]) -> String {
    let mut b = String::new();
    b.push_str("You are the Coder agent. A reviewer found issues with the current diff. ");
    b.push_str("Fix every finding below with minimal, correct edits. For each finding, call ");
    b.push_str("mcp__recur__update_finding with its id: status resolved if you fixed it, or ");
    b.push_str(
        "invalid with a clear reason if it is wrong/out of scope. Then call mcp__recur__done.\n\n",
    );
    b.push_str("Original task: ");
    b.push_str(task);
    b.push_str("\n\nReviewer findings to fix:\n");
    for (i, f) in findings.iter().enumerate() {
        let id = match f.public_id() {
            "" => "?",
            id => id,
        };
        b.push_str(&finding_line(i, id, f));
    }
    b
}

pub fn coder_conflict_prompt(task: &str, files: &[String]) -> String {
    let mut b = String::new();
    b.push_str(
        "You are the Coder agent. A sibling workspace's branch was merged into this worktree ",
    );
    b.push_str(
        "and git left merge conflicts. Resolve EVERY conflict marker in the files below so both ",
    );
    b.push_str(
        "sides' intent is kept and the code builds; run the relevant tests. Do not run git merge, ",
    );
    b.push_str("rebase, reset, or commit: the orchestrator commits and that concludes the merge. ");
    b.push_str("Then call mcp__recur__done.\n\nOriginal task: ");
    b.push_str(task);
    b.push_str("\n\nConflicted files:\n");
    for f in files {
        b.push_str("- ");
        b.push_str(f);
        b.push('\n');
    }
    b
}

pub fn coder_open_findings_nudge_prompt(task: &str, open: &[Finding]) -> String {
    let mut b = String::new();
    b.push_str("You stopped while reviewer findings are still open. You are NOT done. ");
    b.push_str("Address each open finding below, mark it resolved or invalid ");
    b.push_str(
        "(invalid requires a reason) via mcp__recur__update_finding, then call mcp__recur__done.\n\n",
    );
    b.push_str("Original task: ");
    b.push_str(task);
    b.push_str("\n\nOpen findings:\n");
    for (i, f) in open.iter().enumerate() {
        b.push_str(&finding_line(i, f.public_id(), f));
    }
    b
}

pub fn coder_resume_prompt(task: &str, iter: i64, session_restored: bool) -> String {
    let mut b = String::new();
    if session_restored {
        b.push_str("You were interrupted mid-task (the app restarted) and have just been ");
        b.push_str("resumed. Your earlier work in this worktree is still there. Re-check the ");
        b.push_str("current state of the files you were editing, finish the remaining work, ");
        b.push_str("then call mcp__recur__done.");
    } else {
        b.push_str("You are the Coder agent resuming an interrupted task (iteration ");
        b.push_str(&iter.to_string());
        b.push_str("). Work from a previous run may already be committed in this worktree. ");
        b.push_str("FIRST inspect what is already there (git log/git diff against the base ");
        b.push_str("branch, plus the relevant files) so you do not redo or undo it, THEN make ");
        b.push_str("only the changes still missing and call mcp__recur__done.");
    }
    b.push_str("\n\nTask: ");
    b.push_str(task);
    b
}

pub const CODER_FIX_DISPLAY: &str = "Fix open reviewer findings";
pub const CODER_RESUME_DISPLAY: &str = "Resume coding";

/// The cockpit label for a Coder prompt built above ("" for a bare goal).
pub fn coder_dispatch_display(prompt: &str) -> &'static str {
    if prompt.starts_with("You are the Coder agent. A reviewer found issues")
        || prompt.starts_with("You stopped while reviewer findings")
    {
        CODER_FIX_DISPLAY
    } else if prompt.starts_with("You were interrupted mid-task")
        || prompt.starts_with("You are the Coder agent resuming an interrupted task")
    {
        CODER_RESUME_DISPLAY
    } else {
        ""
    }
}

pub fn maestro_plan_system_prompt(task: &str) -> String {
    format!(
        "{}{task}",
        concat!(
            "You are the Maestro: the orchestrator of ONE autonomous coding loop. Before any ",
            "code is written, investigate the repository and produce a THOROUGH implementation plan ",
            "for the task below. The plan is the ONLY brief the Coder agent will receive: that agent ",
            "has no other context and no memory of your reasoning, so write it in enough detail that a competent but unfamiliar engineer ",
            "could execute it with no further judgment calls.\n\n",
            "First, use Read/Grep/Glob to actually look at the codebase: find the specific files, ",
            "functions, and call sites the task touches; note existing conventions/patterns nearby so ",
            "the change fits in; check for related tests. Do NOT guess at file paths or symbol names: ",
            "look them up. You have no Edit/Write/Bash: investigate the repo, never modify it. ",
            "You may mcp__recur__create_artifact for diagrams or mockups that belong on the plan ",
            "(they write outside the git worktree).\n\n",
            "Then call mcp__recur__submit_plan. Put the DETAIL in the body argument: that body is the ",
            "ONLY brief the Coder will receive, so do not leave the thorough plan only in chat. Shape:\n\n",
            "summary: one or two sentences on what this change does and why.\n\n",
            "body (markdown):\n",
            "Relevant files:\n- <path>: <what's there / why it's relevant>\n- … (every file you expect ",
            "the Coder to touch or must read for context; be specific, e.g. `internal/engine/loop.go` ",
            "not \"the engine\")\n\n",
            "Approach: how to make the change. Name functions/types and what changes. Be concrete ",
            "enough that an unfamiliar engineer could execute with no further judgment calls.\n\n",
            "Edge cases / gotchas: anything that could easily be missed or broken by this change ",
            "(existing callers, tests, concurrency, error paths), or \"none identified\" if truly none.\n\n",
            "Definition of done: the precise, observable condition(s) that mean this task is complete ",
            "and correct (what should build/pass/behave, not just \"code compiles\").\n\n",
            "steps: ordered checklist, each one actionable sentence naming the file/function and the ",
            "change (as many as the task needs: a small task may need 2, a large one 10+). Do not ",
            "duplicate the checklist in body; body holds the detail, steps are the living todos. ",
            "Prefer the object form {title, files, depends_on}: files = the repo-relative paths the step ",
            "touches, depends_on = ids (S1, S2, … by position) that must land first. Steps with disjoint ",
            "files and no unmet dependency run IN PARALLEL in isolated child workspaces and are merged ",
            "back; steps that share a file ride together in one workspace, and steps with no files share ",
            "one workspace. Declare files honestly and split work so independent steps do not share files.\n\n",
            "attachments: ids of any artifacts you created (create_artifact) or user-attached images ",
            "(⟦attachment:id⟧) that belong to this plan. Omit if none.\n\n",
            "Be concrete over concise: prefer one extra correct, specific sentence to a vague one. If ",
            "the task is genuinely ambiguous in a way you must not guess on, call mcp__recur__escalate ",
            "with a scoped question and options instead of planning.\n\n",
            "Prefer submit_plan over any built-in create-plan tool your environment exposes; Recur ",
            "cannot consume that tool into the structured plan.\n\n",
            "Task: ",
        )
    )
}

/// The live loop context seeded into a Maestro chat turn (PRD §8.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaestroContext {
    pub goal: String,
    /// The `[[loop]]` prompt: the conductor charter.
    pub mission: String,
    pub state: String,
    /// This chat's walk is live (persisted paused is not enough).
    pub live: bool,
    pub iteration: i64,
    /// Recent activity-feed lines, newest first, tags stripped.
    pub recent: Vec<String>,
    /// The observatory blast-radius one-liner.
    pub impact: String,
    /// Conversation + line notes on the open PR.
    pub pr_thread: String,
    /// The implementer's latest done-notes.
    pub handoff: String,
}

pub fn maestro_system_prompt(ctx: &MaestroContext) -> String {
    let mut b = String::new();
    b.push_str("You are the Maestro: the conductor of ONE Loop. ");
    b.push_str("The human steers this loop by talking to you (they never talk to the specialist agents directly). ");
    b.push_str("You orchestrate only: do not do the loop's phase work yourself. The engine already runs the specialists ");
    b.push_str("on this loop's graph. Read/Grep/Glob only to answer the human, never to stand in for a specialist. ");
    b.push_str("dispatch_specialist is a one-off on a settled loop, not the walk. ");
    b.push_str("Answer where the run stands honestly and concisely. ");
    b.push_str("Your actions are engine-mediated tools; the engine validates and executes them:\n");
    b.push_str("- mcp__recur__plan: steer a RUNNING walk with a short re-plan summary. It does not start specialists: if the walk is idle, paused, or settled, plan is a no-op.\n");
    b.push_str(
        "- mcp__recur__message_child: steer one running child workspace; plan steers this loop.\n",
    );
    b.push_str("- mcp__recur__edit_plan: rewrite the loop's definition of done (logged as a deviation, steered into turns).\n");
    b.push_str(
        "- mcp__recur__dispatch_specialist: run a one-off reviewer|security pass over a settled loop.\n",
    );
    b.push_str("When the human wants work to continue, pick by whether the walk is live, not by the words “keep going” / “keep iterating”:\n");
    b.push_str("- no live walk (stopped/failed/settled, or paused without “(live walk)”): mcp__recur__reenter_phase (usually converge). Do this even if recent activity still shows the cap question: that question is closed once State is not escalated. Do not claim you steered.\n");
    b.push_str("- live walk escalated at the round cap: mcp__recur__extend_rounds. Refused after the loop has stopped, and for agent questions or other waits.\n");
    b.push_str("- live walk paused at a checkpoint: do not reenter_phase; Resume continues this walk. mcp__recur__plan only steers.\n");
    b.push_str("- any other live walk: do not restart; mcp__recur__plan only steers.\n");
    b.push_str("- mcp__recur__run_phase: on-demand phases only (e.g. security).\n");
    b.push_str("- mcp__recur__submit_plan / mcp__recur__update_plan_step: rewrite the STRUCTURED plan (headline + detailed body + ordered steps + optional attachments), or advance one step's status (steps are numbered S1, S2, …).\n");
    b.push_str("- mcp__recur__ask_user: ask the human a clarifying question (ends this turn; answer arrives later as a user message; does not pause the workspace). Never use Claude's AskUserQuestion: it cannot collect answers in Recur.\n");
    b.push_str("- mcp__recur__create_artifact / list_artifacts / read_artifact / present_artifact: standalone analytical deliverables (HTML may be self-contained interactive demos under the preview sandbox).\n");
    b.push_str("When the human asks for something one of these tools does, CALL IT: don't tell them to do it ");
    b.push_str(
        "themselves. If the engine refuses, relay the named reason. You have no Edit/Write tools: ",
    );
    b.push_str("judgment and orchestration, not code edits. Be brief and direct.\n\n");
    b.push_str("Loop context:\n");
    let mission = ctx.mission.trim();
    let goal = ctx.goal.trim();
    if !mission.is_empty() {
        b.push_str("- This loop (the engine walks the graph; you orchestrate only): ");
        b.push_str(mission);
        b.push('\n');
    }
    if !goal.is_empty() && goal != mission {
        b.push_str("- Task: ");
        b.push_str(goal);
        b.push('\n');
    }
    b.push_str("- State: ");
    b.push_str(&ctx.state);
    if ctx.live {
        b.push_str(" (live walk)");
    }
    if ctx.iteration > 0 {
        b.push_str(&format!(" (iteration {})", ctx.iteration));
    }
    if !ctx.impact.is_empty() {
        b.push_str("\n- Blast radius: ");
        b.push_str(&ctx.impact);
    }
    let h = ctx.handoff.trim();
    if !h.is_empty() {
        b.push_str("\n- Implementer's last handoff (deviations / concerns / unfinished): ");
        b.push_str(&truncate(h, 1200));
    }
    if !ctx.recent.is_empty() {
        b.push_str("\n- Recent activity:\n");
        for r in &ctx.recent {
            b.push_str("  · ");
            b.push_str(r);
            b.push('\n');
        }
    }
    if !ctx.pr_thread.is_empty() {
        b.push_str("\nOpen PR review thread (conversation and line notes). ");
        b.push_str("Answer if the human is asking a question. If they want work done, call ");
        b.push_str("mcp__recur__reenter_phase on an allowed phase (usually converge). ");
        b.push_str("Do not invent a second rework machine or a PR-responder specialist.\n");
        b.push_str(&ctx.pr_thread);
    }
    b
}

pub fn reviewer_system_prompt(task: &str, plan: &str) -> String {
    let base = format!(
        "{}{task}",
        concat!(
            "You are the Reviewer agent in an autonomous coding loop. Review the current ",
            "diff against the task below. Use Read/Glob/Grep/LSP, `git diff`, Bash for ",
            "lint/test/typecheck, and WebFetch/WebSearch when you need external docs: do ",
            "not modify any files. For EACH real problem you find (a bug, a missing case, a ",
            "regression, a quality issue), call the mcp__recur__report_finding tool with the ",
            "file, line (and optional end_line for a multi-line span), severity ",
            "(info/low/medium/high/critical), a clear message, and optional summary (markdown ",
            "shown to the end user in the turn digest; omit to use message). On a re-review, ",
            "call mcp__recur__update_finding to mark a prior finding resolved (fixed) or ",
            "invalid (wrong/out of scope: reason required). Before filing, call ",
            "mcp__recur__list_findings with status \"all\". Never re-file a finding the human ",
            "ignored or marked invalid (updatedBy human), even reworded or at moved lines, and ",
            "do not duplicate one that is still open. On a re-review, verify each open finding against the ",
            "current code and mark it resolved once the issue is gone; if a resolved finding's ",
            "fix did not hold, file it again as a new finding. If AGENTS.md has a `## Reviews` ",
            "section, follow it on what not to flag. Do NOT restate ",
            "or list filed findings in your closing reply: the app summarizes still-open ",
            "filings for the user; keep closing prose brief. If the diff is correct and ",
            "complete, report no findings.\n\nTask: ",
        )
    );
    let plan = plan.trim();
    if plan.is_empty() {
        return base;
    }
    format!(
        "{base}{}{plan}",
        concat!(
            "\n\nBefore any code was written, the Maestro investigated this repo and wrote ",
            "the plan below: it names the specific files/edge-cases in scope and, most importantly, ",
            "the DEFINITION OF DONE for this task. Judge the diff's completeness against that ",
            "definition of done (not just \"does it compile\"), and use the plan's scope to tell a real ",
            "problem apart from a change that was never asked for:\n\n",
        )
    )
}

/// The Reviewer's prompt for an iteration plus the findings the Coder
/// disputed.
pub fn reviewer_prompt_for(
    store: &Store,
    workspace_id: &str,
    goal: &str,
    plan: &str,
    iter: i64,
) -> String {
    reviewer_system_prompt(goal, plan) + &coder_disputed_findings_section(store, workspace_id, iter)
}

/// Findings the Coder marked invalid in the previous review iteration, so
/// the Reviewer can re-judge them. Scoped to one iteration so history does
/// not re-anchor every later review.
pub fn coder_disputed_findings_section(store: &Store, workspace_id: &str, iter: i64) -> String {
    let prev = iter - 1;
    if prev < 1 {
        return String::new();
    }
    let invalid = store
        .findings_by_status(workspace_id, FindingStatus::Invalid.as_str())
        .unwrap_or_default();
    let disputed: Vec<&Finding> = invalid
        .iter()
        .filter(|f| f.updated_by == recur_domain::FINDING_ACTOR_CODER && f.iteration == prev)
        .collect();
    if disputed.is_empty() {
        return String::new();
    }
    let mut b = String::new();
    b.push_str("\n\nThe Coder disputed the following prior findings as invalid. Re-judge each ");
    b.push_str("against the CURRENT diff. If a dispute is wrong and the issue still stands, ");
    b.push_str("call mcp__recur__report_finding again with enough detail for the next fix pass ");
    b.push_str("(include why the coder's reason does not hold):\n");
    for (i, f) in disputed.iter().enumerate() {
        b.push_str(&format!(
            "{}. [{}] {}: {}\n",
            i + 1,
            f.severity.as_str(),
            finding_loc(&f.file, f.line, f.end_line),
            f.message
        ));
        if !f.status_reason.is_empty() {
            b.push_str(&format!("   Coder reason: {}\n", f.status_reason));
        }
    }
    b
}

#[cfg(test)]
mod tests;
