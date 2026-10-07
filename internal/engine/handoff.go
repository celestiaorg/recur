package engine

import (
	"context"
	"fmt"
	"html"
	"strings"

	"recur/internal/domain"
)

// Implementer handoff: a worker hands back not just what was done but
// deviations, concerns, and unfinished work.
// The Coder files it as done(notes); the engine threads it to the Reviewer's
// briefing for the same round, to the conductor's context, and — when the plan
// is structured — to a bounded Maestro replan turn before the review.

const handoffMaxChars = 4000

// accumulateHandoff appends next onto prev, ignoring empty next so a later
// empty parent turn cannot wipe child notes. Caps at handoffMaxChars.
func accumulateHandoff(prev, next string) string {
	next = truncate(strings.TrimSpace(next), handoffMaxChars)
	if next == "" {
		return prev
	}
	if prev == "" {
		return next
	}
	return truncate(prev+"\n"+next, handoffMaxChars)
}

// recordHandoff stores the round's done-notes on the loop and the workspace,
// and surfaces them on the feed. lastHandoff is this landing (replan); boardHandoff
// accumulates child notes within one board pass. An empty note clears
// lastHandoff but keeps boardHandoff — a parent fallback must not wipe children.
// Call resetBoardHandoff at the start of an independent coding round.
func (l *Loop) recordHandoff(rs roleSpec, notes string) {
	notes = truncate(strings.TrimSpace(notes), handoffMaxChars)
	l.lastHandoff = notes
	if notes != "" {
		l.boardHandoff = accumulateHandoff(l.boardHandoff, notes)
		l.emitFeed(rs.code, rs.name, "Handoff: "+html.EscapeString(truncate(notes, feedInputLimit)))
	}
	stored := l.boardHandoff
	if stored == "" {
		stored = notes
	}
	l.eng.Store.UpdateWorkspace(l.ID, func(ws *domain.Workspace) { ws.Handoff = stored })
}

// resetBoardHandoff starts a new coding round's notes. Child notes still
// accumulate within a board pass; the next independent round replaces them.
func (l *Loop) resetBoardHandoff() {
	l.boardHandoff = ""
	l.lastHandoff = ""
	if l.eng != nil {
		l.eng.Store.UpdateWorkspace(l.ID, func(ws *domain.Workspace) { ws.Handoff = "" })
	}
}

// handoffSection is the Reviewer briefing block for this round's handoff.
func handoffSection(notes string) string {
	notes = strings.TrimSpace(notes)
	if notes == "" {
		return ""
	}
	return "\n\nThe implementer handed off with these notes (deviations from the plan, concerns, " +
		"unfinished work). Judge them: a deviation the plan did not allow, or work left undone " +
		"that the definition of done requires, is a finding. A concern that is real is a finding " +
		"at the severity it deserves:\n\n" + notes
}

// maestroReplanTools is the replan turn's tool set: read-only investigation
// plus the plan hands. No escalate — a replan that cannot decide leaves the
// plan as is; the review round still runs.
var maestroReplanTools = []string{"Read", "Grep", "Glob", mcpSubmitPlan, mcpUpdatePlanStep, mcpEditPlan, mcpListFindings}

func maestroReplanSystemPrompt(task, plan, handoff string) string {
	return "You are the Maestro — the conductor of ONE autonomous coding loop. The implementer " +
		"just finished a turn and handed off with notes. Decide whether the PLAN needs revising " +
		"before the Reviewer judges the diff. Revise when a step turned out to be too big (split it), " +
		"wrong (drop or rewrite it), or missing (add it). You revise HOW the work is decomposed, never " +
		"WHAT counts as done: the user's task, constraints, and acceptance criteria stay as written — " +
		"do not drop or soften a requirement to fit what landed. If the notes say a requirement " +
		"itself is unachievable, leave the plan and say so; the Reviewer and the human decide. Use " +
		"Read/Grep/Glob only to check what the notes claim. Investigate, do not edit code.\n\n" +
		"To revise: call mcp__recur__submit_plan with the FULL plan (summary, body, every step in " +
		"order — finished steps keep their titles so their status survives; steps other child " +
		"workspaces are working RIGHT NOW keep their titles too, they close when they land). To change one step's " +
		"status, mcp__recur__update_plan_step. If the plan still holds, reply in one sentence and call " +
		"no tools. Be brief.\n\n" +
		"Task: " + task + "\n\nCurrent plan:\n" + plan + "\n\nImplementer handoff:\n" + handoff
}

// runReplanner is the between-rounds replan hook: when the implementer's
// handoff is non-empty, the Maestro gets one bounded turn to revise the plan.
// The plan tools write the stored plan as they are called, so a turn that
// fails AFTER filing a revision has still revised it: the loop adopts whatever
// the store holds either way (never two plans in play) and says so. The review
// round always runs. Returns true when the plan changed (the Coder's next
// prompt then carries the new plan).
func (l *Loop) runReplanner(ctx context.Context, handoff string) bool {
	handoff = strings.TrimSpace(handoff)
	if handoff == "" || strings.TrimSpace(l.plan) == "" {
		return false
	}
	before, ok := l.eng.Store.Workspace(l.ID)
	if !ok {
		return false
	}
	ctx, cancel := l.turnContext(ctx)
	defer cancel()

	rs := l.slotRoleSpec("planner", roleSpec{
		role: domain.RoleMaestro, code: maestroAgentCode, name: "Maestro",
		allowedTools: maestroReplanTools,
	})
	rs.allowedTools = maestroReplanTools
	agentID := l.ID + "-" + rs.code
	set := l.eng.settings()
	model, flags := l.roleModel(rs, set.MaestroModel().String(), set.MaestroFlags())
	l.eng.updateAgentStatus(l.ID, rs.code, "replanning")

	planText := l.plan
	if before.Plan != nil && len(before.Plan.Steps) > 0 {
		planText = before.Plan.Board()
	}

	token, mcpURL, err := l.eng.registerMCP(l.ID, rs.code, l.ChatCode, rs.allowedTools, model)
	if err != nil {
		l.eng.updateAgentStatus(l.ID, rs.code, "idle")
		return false
	}
	defer l.eng.unregisterMCP(token)

	spec := AgentSpec{
		WorkspaceID:    l.ID,
		ChatCode:       l.ChatCode,
		AgentID:        agentID,
		Role:           domain.RoleMaestro,
		Model:          model,
		Flags:          flags,
		SystemPrompt:   maestroReplanSystemPrompt(l.Goal, planText, handoff),
		AllowedTools:   rs.allowedTools,
		Prompt:         "Review the implementer handoff against the plan. Revise the plan only if it needs it.",
		Worktree:       l.Worktree,
		PermissionMode: set.PermissionMode(),
		MCPURL:         mcpURL,
	}
	l.applyGoalImages(&spec)
	release, ok := l.acquireTurnSlot(ctx)
	if !ok {
		l.eng.updateAgentStatus(l.ID, rs.code, "idle")
		return false
	}
	defer release()

	res, err := l.eng.runAgent(ctx, spec, func(ev ParsedEvent) {
		if ev.Kind == KindStep && !ev.nested() {
			l.eng.appendAgentStep(l.ID, rs.code, domain.AgentStep{
				Kind: stepKind(displayToolFor(ev).Name), Text: displayToolFor(ev).Name + " " + ev.ToolInput,
			})
		}
	})
	l.eng.publishResource()
	l.eng.updateAgentStatus(l.ID, rs.code, "idle")
	failed := err != nil || res.IsError

	after, ok := l.eng.Store.Workspace(l.ID)
	if !ok || !planExecutionChanged(before, after) {
		if failed {
			l.emitFeed(rs.code, rs.name, "Replan turn failed — keeping the current plan.")
		} else {
			l.emitFeed(rs.code, rs.name, "Plan still holds.")
		}
		return false
	}
	l.plan = truncate(after.DefinitionOfDone, planMaxPersistChars)
	l.planStructured = after.Plan != nil && len(after.Plan.Steps) > 0
	n := 0
	if after.Plan != nil {
		n = len(after.Plan.Steps)
	}
	if failed {
		l.emitFeed(rs.code, rs.name, fmt.Sprintf("Replan turn failed after revising the plan — the revision it filed stands (%d steps).", n))
	} else {
		l.emitFeed(rs.code, rs.name, fmt.Sprintf("Plan revised after the handoff (%d steps).", n))
	}
	return true
}

// planExecutionChanged reports a replan that changed prose OR structured
// execution metadata (files, depends_on, titles). Plan.Text() / DefinitionOfDone
// omit those, so a files-only submit_plan must still count as a revision.
func planExecutionChanged(before, after domain.Workspace) bool {
	if strings.TrimSpace(after.DefinitionOfDone) != strings.TrimSpace(before.DefinitionOfDone) {
		return true
	}
	return !planShapeEqual(before.Plan, after.Plan)
}

func planShapeEqual(a, b *domain.Plan) bool {
	if a == nil && b == nil {
		return true
	}
	if a == nil || b == nil || len(a.Steps) != len(b.Steps) {
		return false
	}
	if strings.TrimSpace(a.Summary) != strings.TrimSpace(b.Summary) || strings.TrimSpace(a.Body) != strings.TrimSpace(b.Body) {
		return false
	}
	for i := range a.Steps {
		if a.Steps[i].Title != b.Steps[i].Title ||
			!stringSliceEqual(a.Steps[i].Files, b.Steps[i].Files) ||
			!stringSliceEqual(a.Steps[i].DependsOn, b.Steps[i].DependsOn) {
			return false
		}
	}
	return true
}

func stringSliceEqual(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
