package engine

import (
	"context"
	"fmt"
	"html"
	"strings"

	"recur/internal/bundles"
	"recur/internal/domain"
)

// Maestro hands: actions the Maestro chat REQUESTS via MCP and
// the ENGINE validates + executes within the rails. Each returns a short status
// string that flows back as the tool result — success, or the named reason the
// engine refused (the refusal also lands on the feed, so chat and feed agree).

// EditPlan rewrites a loop's persisted plan. It is the shared
// implementation behind the mcp__recur__edit_plan tool AND the plan editor in the
// loop UI. The edit is logged as a plan-change deviation and, when the loop is
// running, steered into subsequent specialist turns (D49). When a structured
// Plan exists, the new text becomes Plan.Body and the prose mirror is rebuilt
// from Plan.Text() so Coders keep the living checklist.
func (e *Engine) EditPlan(workspaceID, plan string) string {
	plan = strings.TrimSpace(plan)
	if plan == "" {
		return "Refused: the new plan is empty."
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	if !ws.HasRun() {
		return "Refused: only a workspace with a loop has a plan to edit."
	}

	e.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		if l.Plan != nil {
			p := *l.Plan
			p.Body = plan
			if strings.TrimSpace(p.Summary) == "" {
				p.Summary = firstParagraph(plan)
			}
			p.Approved = false
			capPlanForPersist(&p)
			l.Plan = &p
			l.DefinitionOfDone = truncate(p.Text(), planMaxPersistChars)
			if prog, ok := runProgress(l.State, l.Plan); ok {
				l.Progress = prog
			}
			return
		}
		l.DefinitionOfDone = truncate(plan, planMaxPersistChars)
	})
	e.emitDeviation(workspaceID,
		"Plan edited: "+html.EscapeString(truncate(plan, 200)),
		"default = the previous plan",
		"the plan was rewritten (chat or plan editor); specialists are steered with the new plan.")
	if lp := e.runningLoopByID(workspaceID); lp != nil {
		steer := plan
		if fresh, ok := e.Store.Workspace(workspaceID); ok && strings.TrimSpace(fresh.DefinitionOfDone) != "" {
			steer = fresh.DefinitionOfDone
		}
		lp.queueSteer("The plan (definition of done) has been UPDATED. Work to this plan now:\n" + steer)
	}
	if fresh, ok := e.Store.Workspace(workspaceID); ok {
		e.publishLoopUpdated(fresh.ID, fresh.State, fresh.Phase, fresh.Iteration)
	}
	e.touchWorkspaceActivity(workspaceID, "Plan edited", e.now())
	return "Plan updated — logged as a deviation and steered into subsequent turns."
}

// DispatchSpecialistTurn runs ONE bounded specialist turn (reviewer|security) over
// the loop's worktree with a concrete instruction. It refuses while the loop is
// actively running: a running loop already reviews every iteration, and an ad-hoc
// reviewer would double-count findings into the convergence decision. On a settled
// loop (planning/pr_open/paused/stopped/failed) findings persist at the loop's
// current iteration via the no-running-loop ReportFinding path.
func (e *Engine) DispatchSpecialistTurn(workspaceID, role, instruction string) string {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	if ws.WorktreePath == "" {
		return "Refused: the workspace has no worktree to inspect."
	}
	if e.runningLoopByID(workspaceID) != nil {
		return "Refused: the loop is running — it already reviews every iteration; wait for it to settle."
	}

	rs := roles[domain.RoleReviewer]
	rs.mode = bundles.RoleModeReview
	sysPrompt := dispatchReviewerSystemPrompt(instruction)
	if role == "security" {
		rs = securityReviewerRole
		rs.mode = bundles.RoleModeReview
		sysPrompt = securityReviewerSystemPrompt(instruction)
		if cat, err := e.discoverLoopCatalog(); err == nil {
			if r, ok := bundles.FindRole(cat.Roles, "recur.roles.security-reviewer"); ok {
				rs.modelSlot = r.ModelSlot
				if r.Mode != "" {
					rs.mode = r.Mode
				}
			}
		}
	}

	set := e.settings()
	model, flags := modelForSlot(set, rs, set.ReviewerModel().String(), set.ReviewerFlags())
	agentID := ws.ID + "-" + rs.code
	e.upsertAgent(ws.ID, domain.Agent{
		ID: agentID, WorkspaceID: ws.ID, Code: rs.code, Name: rs.name,
		Role: "dispatched from chat", Model: domain.ParseModelRef(model).ID,
		Status: "working", Color: "var(--indigo)", Task: truncate(instruction, 120), Cur: -1,
	})

	go func() {
		defer e.updateAgentStatus(ws.ID, rs.code, "idle")

		ctx, cancel := context.WithTimeout(context.Background(), set.AgentTurnTimeout())
		defer cancel()
		release, gotSlot := e.acquireAgent(ctx.Done())
		if !gotSlot {
			e.emitWorkspaceEvent(ws.ID, rs.code, rs.name, "Could not start — every agent slot is busy.")
			return
		}
		defer release()

		token, mcpURL, err := e.registerMCP(ws.ID, rs.code, loopOwnerCode(ws), rs.allowedTools, model)
		if err != nil {
			e.emitWorkspaceEvent(ws.ID, rs.code, rs.name, "Could not start — "+err.Error())
			return
		}
		defer e.unregisterMCP(token)

		spec := AgentSpec{
			WorkspaceID:    ws.ID,
			ChatCode:       loopOwnerCode(ws),
			AgentID:        agentID,
			Role:           domain.RoleReviewer,
			Model:          model,
			Flags:          flags,
			SystemPrompt:   sysPrompt,
			AllowedTools:   rs.allowedTools,
			Prompt:         instruction,
			Worktree:       ws.WorktreePath,
			PermissionMode: set.PermissionMode(),
			MCPURL:         mcpURL,
		}
		res, err := e.runAgent(ctx, spec, func(ev ParsedEvent) {
			if ev.Kind == KindInit {
				e.setAgentSession(ws.ID, rs.code, ev.SessionID)
			}
		})
		e.publishResource()

		if err != nil || res.IsError {
			e.emitWorkspaceEvent(ws.ID, rs.code, rs.name, "Dispatched turn failed: "+html.EscapeString(firstNonEmptyStr(res.Text, fmt.Sprint(err))))
			return
		}
		open := e.Store.OpenFindingsCount(ws.ID)
		e.emitWorkspaceEvent(ws.ID, rs.code, rs.name, fmt.Sprintf(
			"Dispatched turn finished — %s now open.", plural(open, "finding", "findings")))
	}()

	return fmt.Sprintf("%s dispatched over the worktree — findings will appear as they are filed.", rs.name)
}

// dispatchReviewerSystemPrompt is the ad-hoc (chat-dispatched) reviewer prompt: a
// focused read-only review with structured findings, without a loop iteration.
func dispatchReviewerSystemPrompt(task string) string {
	return "You are a code Reviewer dispatched into a workspace. Review READ-ONLY " +
		"(Read, Grep, `git diff`/`git log`) — you cannot edit files. For EACH real issue " +
		"call mcp__recur__report_finding with file, line (optional end_line for a range), severity, and a clear message. " +
		"On a re-review, mark prior findings resolved or invalid with mcp__recur__update_finding. " +
		"Do not file style nits as findings. If the work is sound, report no findings.\n\n" +
		"Instruction: " + task
}

// ExtendLoopIterations grants a RUNNING loop extra fix iterations, clamped to the
// autonomy level's extension allowance (the same arithmetic a cap escalation
// grants — rail #4 stays the engine's).
func (e *Engine) ExtendLoopIterations(workspaceID, agentCode string, n int) string {
	lp := e.runningLoopForHand(workspaceID, agentCode)
	if lp == nil {
		return "Refused: the loop is not running — there is no iteration budget to extend."
	}
	granted := n
	if allow := capExtension(lp.Autonomy); granted > allow {
		granted = allow
	}
	if granted < 1 {
		return "Refused: nothing to grant."
	}
	lp.grantExtension(granted)
	e.emitWorkspaceEvent(workspaceID, "MA", "Maestro", fmt.Sprintf(
		"Extended the convergence budget by %d iteration(s) (autonomy allows at most %d per grant).",
		granted, capExtension(lp.Autonomy)))
	if granted < n {
		return fmt.Sprintf("Granted %d extra iteration(s) — clamped from %d by the autonomy level's allowance.", granted, n)
	}
	return fmt.Sprintf("Granted %d extra iteration(s).", granted)
}

// ExtendLoopRounds is the human keep-iterating grant at on_max_rounds escalate.
func (e *Engine) ExtendLoopRounds(workspaceID, agentCode string, n int) string {
	lp := e.runningLoopForHand(workspaceID, agentCode)
	if lp == nil {
		return "Refused: the loop is not running — there is no round budget to extend."
	}
	lp.mu.Lock()
	atCap := lp.escalated && lp.roundCapEsc
	lp.mu.Unlock()
	if !atCap {
		return "Refused: extend_rounds is only legal when the live converge is escalated at the round cap."
	}
	allow := lp.policyExtendRounds()
	if allow < 1 {
		allow = 1
	}
	granted := n
	if granted < 1 {
		granted = allow
	}
	if granted > allow {
		granted = allow
	}
	if _, ok := e.answerEscalationTo(workspaceID, lp, "", EscalationAnswer{
		Choice: "Keep iterating", Grant: granted,
	}); !ok {
		return "Refused: that cap question was already answered."
	}
	e.emitWorkspaceEvent(workspaceID, "MA", "Maestro", fmt.Sprintf(
		"Extended the convergence budget by %d round(s) (policy allows at most %d per grant).",
		granted, allow))
	return fmt.Sprintf("Granted %d extra round(s).", granted)
}

// ReenterPhase starts a new graph walk from phaseID. Idle only.
func (e *Engine) ReenterPhase(workspaceID, agentCode, phaseID string) string {
	if lp := e.runningLoopForHand(workspaceID, agentCode); lp != nil {
		lp.mu.Lock()
		paused := lp.paused
		lp.mu.Unlock()
		if paused {
			return "Refused: reenter_phase is idle only — this walk is still running (paused at a checkpoint). Resume continues it."
		}
		return "Refused: reenter_phase is idle only."
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	rec := e.chatScopedWorkspace(ws, agentCode)
	g, reason := e.resolveGuard(rec.LoopID, rec.PolicyID, rec.Autonomy)
	if reason != "" {
		return "Refused: " + reason
	}
	phaseID = strings.TrimSpace(phaseID)
	allowed := false
	for _, id := range g.Loop.ReenterIDs() {
		if id == phaseID {
			allowed = true
			break
		}
	}
	if !allowed {
		return "Refused: " + phaseID + " is not on this loop's reenter list."
	}
	rec.Checkpoint = &domain.Checkpoint{PhaseID: phaseID}
	rec.Retryable = true
	return e.startScopedWalk(rec)
}

// RunOnDemandPhase runs a dispatch=on_demand phase, then resumes the parked walk.
func (e *Engine) RunOnDemandPhase(workspaceID, agentCode, phaseID string) string {
	phaseID = strings.TrimSpace(phaseID)
	lp := e.runningLoopForHand(workspaceID, agentCode)
	if lp != nil {
		ph, ok := lp.guard.Loop.PhaseByID(phaseID)
		if !ok || ph.Dispatch != "on_demand" {
			return "Refused: run_phase only accepts on-demand phase ids."
		}
		if cur, cok := lp.walkPhase(); cok {
			switch cur.Uses {
			case bundles.VerbConverge, bundles.VerbVerify:
				return "Refused: cannot nest run_phase inside a live " + cur.Uses + " phase."
			case bundles.VerbEnsurePR, bundles.VerbArtifact:
				return "Refused: the current phase is finishing — wait until it settles, then run_phase."
			case bundles.VerbPlan:
				lp.mu.Lock()
				gated := lp.paused || lp.escalated
				lp.mu.Unlock()
				if !gated {
					return "Refused: cannot nest run_phase inside a live plan phase."
				}
			}
		}
		lp.queueOnDemand(ph.ID)
		return "Queued — will run at the next graph checkpoint, then resume the parked walk."
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	rec := e.chatScopedWorkspace(ws, agentCode)
	g, reason := e.resolveGuard(rec.LoopID, rec.PolicyID, rec.Autonomy)
	if reason != "" {
		return "Refused: " + reason
	}
	ph, ok := g.Loop.PhaseByID(phaseID)
	if !ok || ph.Dispatch != "on_demand" {
		return "Refused: run_phase only accepts on-demand phase ids."
	}
	resume := ""
	// simplify: workspace checkpoint is shared; persist per-chat when two idle
	// walks must independently resume a parked phase.
	owner := strings.TrimSpace(agentCode)
	if rec.Checkpoint != nil && e.runningLoopByID(workspaceID) == nil &&
		(owner == "" || owner == strings.TrimSpace(ws.LoopChatCode)) {
		resume = rec.Checkpoint.PhaseID
	}
	rec.Checkpoint = &domain.Checkpoint{PhaseID: ph.ID, ResumePhaseID: resume}
	rec.Retryable = true
	return e.startScopedWalk(rec)
}

// chatScopedWorkspace rebuilds the run record from this chat's [[loop]]/[[policy]].
func (e *Engine) chatScopedWorkspace(ws domain.Workspace, agentCode string) domain.Workspace {
	code := strings.TrimSpace(agentCode)
	if code == "" {
		return ws
	}
	owner := strings.TrimSpace(ws.LoopChatCode)
	ws.LoopChatCode = code
	if lid, pid := e.chatLoopPolicy(ws.ID, code); lid != "" || pid != "" {
		if lid != "" {
			ws.LoopID = lid
		}
		if pid != "" {
			ws.PolicyID = pid
		}
	}
	ws.ReplaceGoal(e.chatLoopGoal(ws.ID, code))
	// simplify: workspace checkpoint is shared; persist per-chat when two idle
	// walks must independently resume a parked phase. Until then, drop a sibling's
	// (or off-graph) phase so walkLoop starts at this [[loop]] entry.
	if ws.Checkpoint != nil {
		owned := owner == "" || owner == code
		if !owned {
			ws.Checkpoint = nil
		} else if g, reason := e.resolveGuard(ws.LoopID, ws.PolicyID, ws.Autonomy); reason == "" {
			if _, ok := g.Loop.PhaseByID(strings.TrimSpace(ws.Checkpoint.PhaseID)); !ok {
				ws.Checkpoint = nil
			}
		}
	}
	return ws
}

// startScopedWalk starts this chat's walk. A live sibling stays on its key;
// an idle workspace persists this chat as owner so Resume uses the right graph.
func (e *Engine) startScopedWalk(rec domain.Workspace) string {
	e.dispatchMu.Lock()
	defer e.dispatchMu.Unlock()
	if strings.TrimSpace(rec.LoopChatCode) != "" && e.runningLoopByID(rec.ID) != nil {
		if e.runningLoopByChat(rec.ID, rec.LoopChatCode) != nil {
			return "Refused: reenter_phase is idle only."
		}
		return e.startAdditionalChatWalk(rec)
	}
	if reason := e.idleWalkRefuse(rec.ID); reason != "" {
		return reason
	}
	e.Store.UpdateWorkspace(rec.ID, func(l *domain.Workspace) {
		if c := strings.TrimSpace(rec.LoopChatCode); c != "" {
			l.LoopChatCode = c
		}
		if id := strings.TrimSpace(rec.LoopID); id != "" {
			l.LoopID = id
		}
		if id := strings.TrimSpace(rec.PolicyID); id != "" {
			l.PolicyID = id
		}
		l.ReplaceGoal(rec.Goal, rec.GoalDisplay)
		l.Checkpoint = rec.Checkpoint
		l.Retryable = rec.Retryable
	})
	return e.dispatchIdleWalkLocked(rec.ID)
}
