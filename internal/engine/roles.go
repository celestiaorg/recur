package engine

import (
	"fmt"
	"strings"

	"recur/internal/domain"
)

// roleSpec defines an app-owned agent role: its scoped tool set, its avatar code
// for the UI, and a system-prompt builder (PRD §8.2). Roles are app-owned and
// versioned in the repo, not user-authored. M3 wires the Coder + Reviewer (the
// convergence loop); the Tester row is scaffolded for M5 (its tools listed so the
// seam is obvious, but the engine does not spawn it yet).
type roleSpec struct {
	role         domain.AgentRole
	code         string   // 2-letter avatar (MA/CO/RV/TE/QA/PR)
	name         string   // display name
	allowedTools []string // --allowedTools scope (headless ref §8.2)
	modelSlot    string   // [[role]].model_slot (ask|plan|agent|review|loop)
	mode         string   // [[role]].mode — model_slot defaults from this
}

// MCP tool names, scoped as mcp__recur__<tool> for --allowedTools (headless ref
// §5). acceptEdits/dontAsk do NOT auto-approve MCP tools, so each role that needs
// one must list it explicitly here (the engine, not a prompt, scopes access).
const (
	mcpDone            = "mcp__recur__done"
	mcpReportFinding   = "mcp__recur__report_finding"
	mcpListFindings    = "mcp__recur__list_findings"
	mcpUpdateFinding   = "mcp__recur__update_finding"
	mcpReportResult    = "mcp__recur__report_result"
	mcpPlan            = "mcp__recur__plan"
	mcpEscalate        = "mcp__recur__escalate"
	mcpAskUser         = "mcp__recur__ask_user"
	mcpSwitchMode      = "mcp__recur__switch_mode"
	mcpCreateArtifact  = "mcp__recur__create_artifact"
	mcpListArtifacts   = "mcp__recur__list_artifacts"
	mcpReadArtifact    = "mcp__recur__read_artifact"
	mcpPresentArtifact = "mcp__recur__present_artifact"
	mcpRenameSession   = "mcp__recur__rename_session"
	mcpRenameWorkspace = "mcp__recur__rename_workspace"
	mcpRenameBranch    = "mcp__recur__rename_branch"
	mcpOpenPR          = "mcp__recur__open_pr"
	mcpCreateWorkspace = "mcp__recur__create_workspace"
	mcpMergeChild      = "mcp__recur__merge_child"
	mcpReadChild       = "mcp__recur__read_child"
	mcpArchiveChild    = "mcp__recur__archive_child"
	mcpMessageChild    = "mcp__recur__message_child"
	mcpAttachEvidence  = "mcp__recur__attach_evidence"
	// Maestro hands: actions the Maestro REQUESTS and the
	// engine validates + executes within the rails.
	mcpEditPlan           = "mcp__recur__edit_plan"
	mcpRerunReview        = "mcp__recur__rerun_review"
	mcpDispatchSpecialist = "mcp__recur__dispatch_specialist"
	mcpExtendIterations   = "mcp__recur__extend_iterations"
	mcpExtendRounds       = "mcp__recur__extend_rounds"
	mcpReenterPhase       = "mcp__recur__reenter_phase"
	mcpRunPhase           = "mcp__recur__run_phase"
	mcpResumeLoop         = "mcp__recur__resume_loop"
	// Structured plan capture: Plan-mode chats and
	// the loop planner file the plan as summary + ordered steps; the Maestro
	// marks steps done as the run advances.
	mcpSubmitPlan     = "mcp__recur__submit_plan"
	mcpUpdatePlanStep = "mcp__recur__update_plan_step"
	// Agent Skills progressive disclosure (model-driven catalog + activation)
	mcpListSkills          = "mcp__recur__list_skills"
	mcpActivateSkill       = "mcp__recur__activate_skill"
	mcpListTerminals       = "mcp__recur__list_terminals"
	mcpReadTerminal        = "mcp__recur__read_terminal"
	mcpListWorkspaceBriefs = "mcp__recur__list_workspace_briefs"
	mcpGetWorkspaceContext = "mcp__recur__get_workspace_context"
	mcpPresentWorkspace    = "mcp__recur__present_workspace"
)

// maestroTools is the conductor allowlist (mode=loop plus ask_user). Retired
// hands (rerun_review / resume_loop / extend_iterations) stay implemented for
// old MCP calls but are not in --allowedTools.
var maestroTools = append(ModeTools("loop"), mcpAskUser)

// Browser (Playwright) MCP tools the QA role may call. The Playwright MCP server
// (configured as the "playwright" server in --mcp-config, see playwrightMCPConfig)
// exposes its tools as mcp__playwright__*. We allow the whole server via the
// wildcard (headless ref §5: "Wildcard mcp__<server>__* allows the whole server")
// so the QA agent can navigate/click/snapshot/screenshot without us enumerating
// every Playwright tool. The QA role gets NO Read/Edit/Write/Bash — it drives the
// app, it does not touch source (PRD §8.2).
const mcpPlaywrightAll = "mcp__playwright__*"

// Shared built-in tool groups. Glob/LSP fill the gaps agents hit when only Grep
// is available; WebFetch/WebSearch let them look up docs/APIs without shelling
// out. Edit/Write stay role-gated (Reviewer must never get them).
var (
	readExploreTools = []string{"Read", "Glob", "Grep", "LSP", "WebFetch", "WebSearch"}
	editTools        = []string{"Edit", "Write", "Bash", "NotebookEdit"}

	// gitReadTools are the read-only git Bash dialects (diff/log/show). The
	// headless docs and PRD §8.2 disagree on permission-rule syntax (space-glob
	// `Bash(git diff *)` vs colon `Bash(git diff:*)`), and which one the installed
	// CLI honors decides whether these reads are pre-approved — so BOTH dialects
	// are listed and a read-only role never ends up prompting for `git diff`
	// regardless of which form the CLI accepts.
	gitReadTools = []string{
		"Bash(git diff:*)", "Bash(git diff *)",
		"Bash(git log:*)", "Bash(git log *)",
		"Bash(git show:*)", "Bash(git show *)",
	}
)

// roles is the static role table (PRD §8.2). M3 wires the Coder + the Reviewer:
// the Coder edits the worktree and signals completion via mcp__recur__done; the
// Reviewer is READ-ONLY on code and files structured findings via
// mcp__recur__report_finding. Tool scoping is enforced in Go, not in prompts (§9).
var roles = map[domain.AgentRole]roleSpec{
	domain.RoleCoder: {
		role: domain.RoleCoder,
		code: "CO",
		name: "Coder",
		// update_plan_step lets the Coder advance the structured plan's steps as
		// it lands them; the engine validates ids.
		// list/update_finding: coder checklist before done (convergence still
		// decided by an independent re-review — PRD §7.3).
		allowedTools: append(append(append(append([]string{}, readExploreTools...), editTools...),
			mcpDone, mcpListFindings, mcpUpdateFinding, mcpUpdatePlanStep, mcpEscalate,
			mcpCreateArtifact, mcpListArtifacts, mcpReadArtifact, mcpPresentArtifact), skillMCPTools...),
	},
	domain.RoleReviewer: {
		role: domain.RoleReviewer,
		code: "RV",
		name: "Reviewer",
		// Read-only on code (PRD §8.2): explore + git inspection (both Bash
		// dialects) + bare Bash for lint/test. The scoped git rules are kept
		// alongside bare Bash so the common read commands are pre-approved and
		// don't prompt when BypassPermissions is off. No Edit/Write —
		// ClaudeRunner passes --disallowedTools for the omitted edit builtins;
		// the prompt says not to modify files. list_findings is read-only and
		// available to every findings-aware agent (dedupe / see open rows).
		// update_finding lets a re-review close or dispute prior rows.
		allowedTools: append(append(append(append([]string{}, readExploreTools...), gitReadTools...),
			"Bash", mcpReportFinding, mcpListFindings, mcpUpdateFinding), skillMCPTools...),
	},
	domain.RoleTester: {
		role: domain.RoleTester,
		code: "TE",
		name: "Tester",
		allowedTools: append(append(append(append([]string{}, readExploreTools...), editTools...),
			mcpReportResult, mcpListFindings), skillMCPTools...),
	},
	domain.RoleQA: {
		role: domain.RoleQA,
		code: "QA",
		name: "QA / Browser",
		// Ask-mode tools (can read) plus evidence / Playwright / report_result.
		// Edit/Write stay off — it drives the app, it does not patch source.
		allowedTools: RoleAllowlist(firstPartyQARole()),
	},
}

// securityReviewerRole is the EXTRA security-focused reviewer spawned by rail #5
// when a committed diff touches auth/secrets (PRD §9). It is a SEPARATE agent from
// both the Coder and the normal Reviewer — a distinct agent code (SR) so rail #2
// (reviewer ≠ coder) holds structurally — read-only on code, filing findings via
// the same MCP report_finding tool. Defined separately (not in the roles map) so a
// caller can't accidentally request it as a base role.
var securityReviewerRole = roleSpec{
	role:         domain.RoleReviewer,
	code:         "SR",
	name:         "Security Reviewer",
	allowedTools: roles[domain.RoleReviewer].allowedTools,
}

// securityReviewerSystemPrompt builds the extra security reviewer's prompt (rail
// #5, PRD §9). It is focused on auth/secret handling specifically. As with the
// normal reviewer, the rail itself is NOT in the prompt — Go decides to spawn it;
// the prompt only tells it HOW and WHAT to scrutinize.
func securityReviewerSystemPrompt(task string) string {
	return "You are an extra Security Reviewer in an autonomous coding loop. The diff " +
		"touches authentication or secret-handling code. Review it READ-ONLY (Read, Grep, " +
		"`git diff`) with a security lens: hardcoded credentials/secrets, weak token " +
		"handling, missing auth checks, secrets logged or committed, insecure defaults. " +
		"For EACH real issue call mcp__recur__report_finding with file, line (and optional " +
		"end_line for a range), severity, a clear message, and optional summary (markdown " +
		"for the end-user digest). Do NOT restate or list filed findings in your closing " +
		"reply — the app summarizes still-open filings. If the security-sensitive changes " +
		"are sound, report no findings.\n\nTask: " + task
}

// qaSystemPrompt builds the QA/Browser agent's role prompt (PRD §8.2, §10). It
// drives the RUNNING app at appURL with the Playwright MCP (navigate/click/type/
// snapshot/screenshot) like a real user, exercising the task's acceptance
// criteria, and records EACH step via mcp__recur__attach_evidence with a short
// description, a screenshot path, and pass/fail. It does NOT touch source code —
// the engine scopes its tools to the browser MCP + attach_evidence only (§9), so
// the "no source edits" property is enforced in Go, not relied on from the prompt.
// shotDir is the directory the agent should save screenshots into
// (workspaces/<workspaceID>/screenshots/).
func qaSystemPrompt(task, appURL, shotDir string) string {
	return "You are the QA / Browser agent in an autonomous coding loop. The app is " +
		"running and reachable at " + appURL + ". Using the Playwright browser tools, " +
		"drive the app like a real user to verify the change accomplishes the task's " +
		"acceptance criteria. For EACH meaningful step: take a screenshot saved under " +
		shotDir + ", then call mcp__recur__attach_evidence with a one-line description " +
		"of what you did/observed, the screenshot_path, and passed=true if the step " +
		"behaved correctly (passed=false if it did not). Do not modify any source files. " +
		"When you have exercised the acceptance criteria, stop.\n\nTask to verify: " + task
}

// qaRuntimeExtra is the engine-derived URL / screenshot / evidence handshake
// appended when a role prompt replaces the qaSystemPrompt fallback.
func qaRuntimeExtra(appURL, shotDir string) string {
	return "\n\nThe app is running and reachable at " + appURL +
		". For each meaningful step: take a screenshot saved under " + shotDir +
		", then call mcp__recur__attach_evidence with a one-line description of what " +
		"you did/observed, the screenshot_path, and passed=true if the step behaved " +
		"correctly (passed=false if it did not). When you have exercised the " +
		"acceptance criteria, call mcp__recur__report_result and stop."
}

// planSection formats the Maestro's planning-turn output (if any) as a "Maestro's
// plan" block appended to a Coder prompt, so the Coder — a "lesser" agent with no
// memory of the Maestro's own investigation — gets that thorough brief verbatim
// instead of just the one-line task goal. Returns "" when no plan was produced
// (planning is best-effort and can degrade to a no-op; PRD §7.2), so every prompt
// builder below degrades gracefully to task-only when there is nothing to attach.
//
// Phrasing is identical on the first coding pass and later fix/rework turns so
// a --resume cache prefix stays byte-stable. The user prompt (coderFixPrompt /
// coderReworkPrompt) is what tells a later turn that findings take precedence.
func planSection(plan string) string {
	plan = strings.TrimSpace(plan)
	if plan == "" {
		return ""
	}
	return "\n\nThe Maestro investigated the repo and prepared this plan. Follow it on the first " +
		"coding pass; on a fix or rework turn the reviewer findings or rework instruction in " +
		"your task message take precedence — the plan is background context:\n\n" + plan
}

// coderSystemPrompt builds the Coder's role prompt for a task (PRD §8.2). It is
// deliberately small and honest: the Coder edits the worktree to accomplish the
// task; the ENGINE (not the agent) commits the result (PRD §11), so the prompt
// does not ask the agent to commit. When done it calls mcp__recur__done. When the
// Maestro produced a planning-turn plan for this loop, it is appended (via
// planSection) so the Coder gets the thorough brief, not just the bare task string.
// The plan text is the same on every iteration so --resume / thread/resume can
// cache it. structured adds the update_plan_step instruction when the plan
// carries numbered steps.
func coderSystemPrompt(task, plan string, structured bool) string {
	stepNote := ""
	if structured {
		stepNote = " The plan's steps are numbered; as you complete (or deliberately skip) each one, " +
			"call mcp__recur__update_plan_step with its id (S1, S2, …) so progress stays honest."
	}
	return "You are the Coder agent in an autonomous coding loop. You are working in " +
		"an isolated git worktree. Make the minimal, correct code changes needed to " +
		"accomplish the task below (Read/Glob/Grep/LSP/Edit/Write/Bash; WebFetch/" +
		"WebSearch for docs when needed). Do not commit — the orchestrator commits " +
		"your changes. When fixing reviewer findings, mark each with " +
		"mcp__recur__update_finding (resolved, or invalid with a reason) before done — " +
		"mcp__recur__list_findings lists what is still open. No TODOs, no stubs, no partial " +
		"implementations, no disabled tests: finish the work or hand off honestly. When the change " +
		"is complete and no findings remain open, call mcp__recur__done with a one-line summary — " +
		"and notes ONLY when something deviated: what you did differently from the plan, what you " +
		"could not finish and why, a step that should be split, or a concern the Reviewer cannot " +
		"see from the diff. Then stop." +
		stepNote + "\n\nTask: " + task + planSection(plan)
}

// coderFixPrompt builds the Coder's prompt when re-entering the loop to fix the
// previous iteration's reviewer findings (the convergence loop, PRD §7.3). The
// findings are passed as concrete instructions, NOT guardrails. The Maestro's
// original plan (if any) is NOT repeated here — coderSystemPrompt already put it
// in the system prompt for this same turn (via planSection), and duplicating a
// thorough, multi-KB plan into the user prompt too would double its token cost on
// every convergence iteration for no benefit (the model already has it above).
func coderFixPrompt(task string, findings []domain.Finding) string {
	var b strings.Builder
	b.WriteString("You are the Coder agent. A reviewer found issues with the current diff. ")
	b.WriteString("Fix every finding below with minimal, correct edits. For each finding, call ")
	b.WriteString("mcp__recur__update_finding with its id: status resolved if you fixed it, or ")
	b.WriteString("invalid with a clear reason if it is wrong/out of scope. Then call mcp__recur__done.\n\n")
	b.WriteString("Original task: ")
	b.WriteString(task)
	b.WriteString("\n\nReviewer findings to fix:\n")
	for i, f := range findings {
		loc := domain.FindingLoc(f.File, f.Line, f.EndLine)
		id := f.PublicID()
		if id == "" {
			id = "?"
		}
		b.WriteString(fmt.Sprintf("%d. id=%s [%s] %s — %s\n", i+1, id, f.Severity, loc, f.Message))
	}
	return b.String()
}

// coderConflictPrompt is the Coder's turn after a child-branch merge (fan-out)
// left conflicts: resolve them, keep both sides' intent, then done. The engine
// commit concludes the in-progress merge.
func coderConflictPrompt(task string, files []string) string {
	var b strings.Builder
	b.WriteString("You are the Coder agent. A sibling workspace's branch was merged into this worktree ")
	b.WriteString("and git left merge conflicts. Resolve EVERY conflict marker in the files below so both ")
	b.WriteString("sides' intent is kept and the code builds; run the relevant tests. Do not run git merge, ")
	b.WriteString("rebase, reset, or commit — the orchestrator commits and that concludes the merge. ")
	b.WriteString("Then call mcp__recur__done.\n\nOriginal task: ")
	b.WriteString(task)
	b.WriteString("\n\nConflicted files:\n")
	for _, f := range files {
		b.WriteString("- ")
		b.WriteString(f)
		b.WriteString("\n")
	}
	return b.String()
}

// coderOpenFindingsNudgePrompt re-enters the Coder when it stopped with findings
// still open (post-exit guard — done may have been skipped).
func coderOpenFindingsNudgePrompt(task string, open []domain.Finding) string {
	var b strings.Builder
	b.WriteString("You stopped while reviewer findings are still open. You are NOT done. ")
	b.WriteString("Address each open finding below, mark it resolved or invalid ")
	b.WriteString("(invalid requires a reason) via mcp__recur__update_finding, then call mcp__recur__done.\n\n")
	b.WriteString("Original task: ")
	b.WriteString(task)
	b.WriteString("\n\nOpen findings:\n")
	for i, f := range open {
		loc := domain.FindingLoc(f.File, f.Line, f.EndLine)
		b.WriteString(fmt.Sprintf("%d. id=%s [%s] %s — %s\n", i+1, f.PublicID(), f.Severity, loc, f.Message))
	}
	return b.String()
}

// coderResumePrompt builds the Coder's prompt for its first turn after the loop was
// interrupted mid-run and resumed from a checkpoint (D55). When the prior claude
// session was restored (sessionRestored), the agent already has the whole task and its
// own partial work in context, so the prompt is a short continuation directive rather
// than a restatement that would invite it to start over. Without a restored session it
// has to be told to inspect what is already in the worktree first.
func coderResumePrompt(task string, iter int, sessionRestored bool) string {
	var b strings.Builder
	if sessionRestored {
		b.WriteString("You were interrupted mid-task (the app restarted) and have just been ")
		b.WriteString("resumed. Your earlier work in this worktree is still there. Re-check the ")
		b.WriteString("current state of the files you were editing, finish the remaining work, ")
		b.WriteString("then call mcp__recur__done.")
	} else {
		b.WriteString("You are the Coder agent resuming an interrupted task (iteration ")
		b.WriteString(fmt.Sprintf("%d", iter))
		b.WriteString("). Work from a previous run may already be committed in this worktree. ")
		b.WriteString("FIRST inspect what is already there (git log/git diff against the base ")
		b.WriteString("branch, plus the relevant files) so you do not redo or undo it, THEN make ")
		b.WriteString("only the changes still missing and call mcp__recur__done.")
	}
	b.WriteString("\n\nTask: ")
	b.WriteString(task)
	return b.String()
}

// Coder dispatch bubbles in the Loop cockpit should match human prompt-actions
// (/fix-findings → "Fix open reviewer findings"): the model still gets the full
// wire, the session card shows the short label.
const (
	coderFixDisplay    = "Fix open reviewer findings"
	coderResumeDisplay = "Resume coding"
)

// coderDispatchDisplay is the UI label for a Coder user-prompt built by the
// helpers above. Empty when Prompt is already human-facing (the bare goal).
func coderDispatchDisplay(prompt string) string {
	switch {
	case strings.HasPrefix(prompt, "You are the Coder agent. A reviewer found issues"):
		return coderFixDisplay
	case strings.HasPrefix(prompt, "You stopped while reviewer findings"):
		return coderFixDisplay
	case strings.HasPrefix(prompt, "You were interrupted mid-task"):
		return coderResumeDisplay
	case strings.HasPrefix(prompt, "You are the Coder agent resuming an interrupted task"):
		return coderResumeDisplay
	default:
		return ""
	}
}

// maestroPlanTools is the Maestro's PLANNING-turn tool set (PRD §7.2) — distinct
// from maestroTools (the chat role, which gets NO file tools). To write a plan a
// "lesser" Coder agent can execute verbatim, the Maestro must first ORIENT itself in
// the ACTUAL repo: read the relevant files, find the existing patterns/conventions,
// locate the exact functions/call sites involved. So the planning turn gets read-only
// investigation tools (Read/Grep/Glob, mirroring the Reviewer's read-only scope) PLUS
// mcp__recur__escalate and mcp__recur__submit_plan (the STRUCTURED plan capture).
// It still gets NO Edit/Write/Bash — it investigates
// and plans, it never edits (§8.2); that stays enforced in Go, not in the prompt.
var maestroPlanTools = []string{"Read", "Grep", "Glob", mcpEscalate, mcpSubmitPlan, mcpListFindings,
	mcpCreateArtifact, mcpListArtifacts, mcpReadArtifact, mcpListSkills, mcpActivateSkill}

// maestroPlanSystemPrompt builds the Maestro's PLANNING-turn system prompt (PRD
// §7.2): at the start of a loop the Maestro investigates the repo and produces a
// THOROUGH plan document BEFORE any code is written — the Coder that executes it is a
// "lesser" agent with no memory of this reasoning, so the plan must be detailed and
// concrete enough to follow like an instruction sheet, not a vague 2-line summary. It does judgment,
// not edits (no Edit/Write/Bash — only Read/Grep/Glob to investigate). It must
// respond with the plan as TEXT (the engine surfaces it to the activity feed,
// persists it, and hands it to the Coder as its task brief). If the task is
// genuinely ambiguous in a way it must not guess on, it calls mcp__recur__escalate
// instead. Guardrails are NOT in the prompt — enforced in Go (§9).
func maestroPlanSystemPrompt(task string) string {
	return "You are the Maestro — the orchestrator of ONE autonomous coding loop. Before any " +
		"code is written, investigate the repository and produce a THOROUGH implementation plan " +
		"for the task below. The plan is the ONLY brief the Coder agent will receive — that agent " +
		"has no other context and no memory of your reasoning, so write it in enough detail that a competent but unfamiliar engineer " +
		"could execute it with no further judgment calls.\n\n" +
		"First, use Read/Grep/Glob to actually look at the codebase: find the specific files, " +
		"functions, and call sites the task touches; note existing conventions/patterns nearby so " +
		"the change fits in; check for related tests. Do NOT guess at file paths or symbol names — " +
		"look them up. You have no Edit/Write/Bash — investigate the repo, never modify it. " +
		"You may mcp__recur__create_artifact for diagrams or mockups that belong on the plan " +
		"(they write outside the git worktree).\n\n" +
		"Then call mcp__recur__submit_plan. Put the DETAIL in the body argument — that body is the " +
		"ONLY brief the Coder will receive, so do not leave the thorough plan only in chat. Shape:\n\n" +
		"summary: one or two sentences on what this change does and why.\n\n" +
		"body (markdown):\n" +
		"Relevant files:\n- <path> — <what's there / why it's relevant>\n- … (every file you expect " +
		"the Coder to touch or must read for context; be specific, e.g. `internal/engine/loop.go` " +
		"not \"the engine\")\n\n" +
		"Approach: how to make the change — name functions/types and what changes. Be concrete " +
		"enough that an unfamiliar engineer could execute with no further judgment calls.\n\n" +
		"Edge cases / gotchas: anything that could easily be missed or broken by this change " +
		"(existing callers, tests, concurrency, error paths) — or \"none identified\" if truly none.\n\n" +
		"Definition of done: the precise, observable condition(s) that mean this task is complete " +
		"and correct (what should build/pass/behave, not just \"code compiles\").\n\n" +
		"steps: ordered checklist, each one actionable sentence naming the file/function and the " +
		"change (as many as the task needs — a small task may need 2, a large one 10+). Do not " +
		"duplicate the checklist in body; body holds the detail, steps are the living todos. " +
		"Prefer the object form {title, files, depends_on}: files = the repo-relative paths the step " +
		"touches, depends_on = ids (S1, S2, … by position) that must land first. Steps with disjoint " +
		"files and no unmet dependency run IN PARALLEL in isolated child workspaces and are merged " +
		"back; steps that share a file ride together in one workspace, and steps with no files share " +
		"one workspace. Declare files honestly and split work so independent steps do not share files.\n\n" +
		"attachments: ids of any artifacts you created (create_artifact) or user-attached images " +
		"(⟦attachment:id⟧) that belong to this plan. Omit if none.\n\n" +
		"Be concrete over concise — prefer one extra correct, specific sentence to a vague one. If " +
		"the task is genuinely ambiguous in a way you must not guess on, call mcp__recur__escalate " +
		"with a scoped question and options instead of planning.\n\n" +
		"Prefer submit_plan over any built-in create-plan tool your environment exposes; Recur " +
		"cannot consume that tool into the structured plan.\n\n" +
		"Task: " + task
}

// maestroSystemPrompt builds the Maestro chat role's system prompt (PRD §8.2,
// §8.5). The Maestro is the per-loop conductor the human steers the loop
// through (never the specialists directly). It orchestrates; the engine walks
// this loop's graph and runs whatever specialists the phases name. Seeded with
// the live loop CONTEXT (mission, state, recent activity) so its answers are
// grounded. CRUCIALLY, the guardrails are NOT in this prompt — they are
// enforced in Go (PRD §9): the prompt only tells the Maestro what it does, the
// engine decides what it is allowed to do. A request to cross a rail is
// rejected by the engine, not by prompt text.
func maestroSystemPrompt(ctx MaestroContext) string {
	var b strings.Builder
	b.WriteString("You are the Maestro — the conductor of ONE Loop. ")
	b.WriteString("The human steers this loop by talking to you (they never talk to the specialist agents directly). ")
	b.WriteString("You orchestrate only: do not do the loop's phase work yourself. The engine already runs the specialists ")
	b.WriteString("on this loop's graph. Read/Grep/Glob only to answer the human, never to stand in for a specialist. ")
	b.WriteString("dispatch_specialist is a one-off on a settled loop, not the walk. ")
	b.WriteString("Answer where the run stands honestly and concisely. ")
	b.WriteString("Your actions are engine-mediated tools — the engine validates and executes them:\n")
	b.WriteString("- mcp__recur__plan: steer a RUNNING walk with a short re-plan summary. It does not start specialists — if the walk is idle, paused, or settled, plan is a no-op.\n")
	b.WriteString("- mcp__recur__message_child: steer one running child workspace; plan steers this loop.\n")
	b.WriteString("- mcp__recur__edit_plan: rewrite the loop's definition of done (logged as a deviation, steered into turns).\n")
	b.WriteString("- mcp__recur__dispatch_specialist: run a one-off reviewer|security pass over a settled loop.\n")
	b.WriteString("When the human wants work to continue, pick by whether the walk is live — not by the words “keep going” / “keep iterating”:\n")
	b.WriteString("- no live walk (stopped/failed/settled, or paused without “(live walk)”): mcp__recur__reenter_phase (usually converge). Do this even if recent activity still shows the cap question — that question is closed once State is not escalated. Do not claim you steered.\n")
	b.WriteString("- live walk escalated at the round cap: mcp__recur__extend_rounds. Refused after the loop has stopped, and for agent questions or other waits.\n")
	b.WriteString("- live walk paused at a checkpoint: do not reenter_phase; Resume continues this walk. mcp__recur__plan only steers.\n")
	b.WriteString("- any other live walk: do not restart; mcp__recur__plan only steers.\n")
	b.WriteString("- mcp__recur__run_phase: on-demand phases only (e.g. security).\n")
	b.WriteString("- mcp__recur__submit_plan / mcp__recur__update_plan_step: rewrite the STRUCTURED plan (headline + detailed body + ordered steps + optional attachments), or advance one step's status (steps are numbered S1, S2, …).\n")
	b.WriteString("- mcp__recur__ask_user: ask the human a clarifying question (ends this turn; answer arrives later as a user message; does not pause the workspace). Never use Claude's AskUserQuestion — it cannot collect answers in Recur.\n")
	b.WriteString("- mcp__recur__create_artifact / list_artifacts / read_artifact / present_artifact: standalone analytical deliverables (HTML may be self-contained interactive demos under the preview sandbox).\n")
	b.WriteString("When the human asks for something one of these tools does, CALL IT — don't tell them to do it ")
	b.WriteString("themselves. If the engine refuses, relay the named reason. You have no Edit/Write tools: ")
	b.WriteString("judgment and orchestration, not code edits. Be brief and direct.\n\n")
	b.WriteString("Loop context:\n")
	mission := strings.TrimSpace(ctx.Mission)
	goal := strings.TrimSpace(ctx.Goal)
	if mission != "" {
		b.WriteString("- This loop (the engine walks the graph; you orchestrate only): ")
		b.WriteString(mission)
		b.WriteString("\n")
	}
	if goal != "" && goal != mission {
		b.WriteString("- Task: ")
		b.WriteString(goal)
		b.WriteString("\n")
	}
	b.WriteString("- State: ")
	b.WriteString(ctx.State)
	if ctx.Live {
		b.WriteString(" (live walk)")
	}
	if ctx.Iteration > 0 {
		b.WriteString(fmt.Sprintf(" (iteration %d)", ctx.Iteration))
	}
	if ctx.Impact != "" {
		b.WriteString("\n- Blast radius: ")
		b.WriteString(ctx.Impact)
	}
	if h := strings.TrimSpace(ctx.Handoff); h != "" {
		b.WriteString("\n- Implementer's last handoff (deviations / concerns / unfinished): ")
		b.WriteString(truncate(h, 1200))
	}
	if len(ctx.Recent) > 0 {
		b.WriteString("\n- Recent activity:\n")
		for _, r := range ctx.Recent {
			b.WriteString("  · ")
			b.WriteString(r)
			b.WriteString("\n")
		}
	}
	if ctx.PRThread != "" {
		b.WriteString("\nOpen PR review thread (conversation and line notes). ")
		b.WriteString("Answer if the human is asking a question. If they want work done, call ")
		b.WriteString("mcp__recur__reenter_phase on an allowed phase (usually converge). ")
		b.WriteString("Do not invent a second rework machine or a PR-responder specialist.\n")
		b.WriteString(ctx.PRThread)
	}
	return b.String()
}

// MaestroContext is the live loop context seeded into the Maestro chat turn (PRD
// §8.5): the goal, the current coarse state + iteration, and a few recent activity
// lines. It is built by the engine from the store at message time, so each turn is
// grounded in the loop's real situation.
type MaestroContext struct {
	Goal      string
	Mission   string // [[loop]] prompt — conductor charter when the human talks
	State     string
	Live      bool // this chat's walk is live — persisted paused is not enough
	Iteration int
	Recent    []string // a few recent activity-feed lines, newest first, tags stripped
	Impact    string   // observatory blast-radius one-liner, when available
	PRThread  string   // conversation + line notes on the open PR, when any
	Handoff   string   // implementer's latest done-notes (deviations, concerns), when any
}

// reviewerSystemPrompt builds the Reviewer's role prompt (PRD §8.2). It is
// read-only on code and files structured findings through the MCP tool. The
// "must not edit" / "separate from the coder" properties are ENFORCED in Go (tool
// scoping + rail #2), not relied on from the prompt (§9) — the prompt only tells
// the reviewer HOW to report, not that a rail exists.
//
// plan is the Maestro's planning-turn brief for this loop (PRD §7.2), if any — the
// same thorough, plan-mode-style document (relevant files, edge cases, and a
// concrete DEFINITION OF DONE) already threaded into the Coder's system prompt via
// coderSystemPrompt/planSection. Convergence (PRD §7.3) is decided off the
// Reviewer's findings, so without it the Reviewer can only judge the diff against
// the bare one-line task/goal string and has no way to know what "done" concretely
// means for this task — it could file a finding that was never actually in scope,
// or (worse) approve a diff that skipped a requirement the plan called out. Passing
// "" degrades cleanly to the task-only prompt (planning is best-effort; PRD §7.2).
func reviewerSystemPrompt(task, plan string) string {
	base := "You are the Reviewer agent in an autonomous coding loop. Review the current " +
		"diff against the task below. Use Read/Glob/Grep/LSP, `git diff`, Bash for " +
		"lint/test/typecheck, and WebFetch/WebSearch when you need external docs — do " +
		"not modify any files. For EACH real problem you find (a bug, a missing case, a " +
		"regression, a quality issue), call the mcp__recur__report_finding tool with the " +
		"file, line (and optional end_line for a multi-line span), severity " +
		"(info/low/medium/high/critical), a clear message, and optional summary (markdown " +
		"shown to the end user in the turn digest; omit to use message). On a re-review, " +
		"call mcp__recur__update_finding to mark a prior finding resolved (fixed) or " +
		"invalid (wrong/out of scope — reason required). Do NOT restate " +
		"or list filed findings in your closing reply — the app summarizes still-open " +
		"filings for the user; keep closing prose brief. If the diff is correct and " +
		"complete, report no findings.\n\nTask: " + task
	plan = strings.TrimSpace(plan)
	if plan == "" {
		return base
	}
	return base + "\n\nBefore any code was written, the Maestro investigated this repo and wrote " +
		"the plan below — it names the specific files/edge-cases in scope and, most importantly, " +
		"the DEFINITION OF DONE for this task. Judge the diff's completeness against that " +
		"definition of done (not just \"does it compile\"), and use the plan's scope to tell a real " +
		"problem apart from a change that was never asked for:\n\n" + plan
}

// reviewerPromptFor is the Reviewer's system prompt for this iteration, plus
// any findings the Coder disputed so the Reviewer can re-judge them.
func reviewerPromptFor(l *Loop, iter int) string {
	return reviewerSystemPrompt(l.Goal, l.plan) + coderDisputedFindingsSection(l, iter)
}

// coderDisputedFindingsSection appends findings the Coder marked invalid (with
// reasons) so the Reviewer can re-judge them and re-report if still valid.
// Scoped to the previous review iteration only — the findings the Coder was just
// asked to fix. Unscoped history would re-anchor every later review (and reworks)
// on stale invalid rows and grow unboundedly across dispute→re-report cycles.
func coderDisputedFindingsSection(l *Loop, iter int) string {
	if l == nil || l.eng == nil {
		return ""
	}
	prev := iter - 1
	if prev < 1 {
		return ""
	}
	invalid := l.eng.Store.FindingsByStatus(l.ID, string(domain.FindingInvalid))
	var disputed []domain.Finding
	for _, f := range invalid {
		if f.UpdatedBy == domain.FindingActorCoder && f.Iteration == prev {
			disputed = append(disputed, f)
		}
	}
	if len(disputed) == 0 {
		return ""
	}
	var b strings.Builder
	b.WriteString("\n\nThe Coder disputed the following prior findings as invalid. Re-judge each ")
	b.WriteString("against the CURRENT diff. If a dispute is wrong and the issue still stands, ")
	b.WriteString("call mcp__recur__report_finding again with enough detail for the next fix pass ")
	b.WriteString("(include why the coder's reason does not hold):\n")
	for i, f := range disputed {
		loc := domain.FindingLoc(f.File, f.Line, f.EndLine)
		b.WriteString(fmt.Sprintf("%d. [%s] %s — %s\n", i+1, f.Severity, loc, f.Message))
		if f.StatusReason != "" {
			b.WriteString(fmt.Sprintf("   Coder reason: %s\n", f.StatusReason))
		}
	}
	return b.String()
}
