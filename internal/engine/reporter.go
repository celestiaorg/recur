package engine

import (
	"context"
	"fmt"
	"strings"
	"time"

	"recur/internal/code"
	"recur/internal/domain"
	"recur/internal/mcp"
)

// permissionSurface tells the UI how to ask for a decision. A loop turn runs
// unattended, so it also raises an inbox row; a chat turn was started by a human
// who is probably still looking at the thread.
const (
	permissionSurfaceLoop = "loop"
	permissionSurfaceChat = "chat"
)

// RequestPermission routes a Claude Code permission prompt to the human and
// blocks until it is answered, the turn ends, or the broker's timeout fires.
// This is the engine half of --permission-prompt-tool, used on every turn when
// Settings.BypassPermissions is off.
func (r engineReporter) RequestPermission(ctx context.Context, workspaceID, agentCode string, ask mcp.PermissionAsk) (mcp.PermissionVerdict, error) {
	decision, err := r.eng.perms.request(ctx, domain.PermissionRequest{
		WorkspaceID: workspaceID,
		AgentCode:   agentCode,
		Surface:     r.eng.permissionSurfaceFor(workspaceID, agentCode),
		Tool:        ask.Tool,
		Input:       string(ask.Input),
		Summary:     permissionSummary(ask.Tool, ask.Input),
	})
	if err != nil {
		return mcp.PermissionVerdict{}, err
	}
	return mcp.PermissionVerdict{Allow: decision.Allow, Reason: decision.Reason}, nil
}

// permissionSurfaceFor picks how the UI should ask. An in-flight PA*/MA chat
// turn means a human started (and is probably watching) that thread — even when
// a loop is also running on the same workspace. Unattended loop agents raise the
// floating/inbox surface instead.
func (e *Engine) permissionSurfaceFor(workspaceID, agentCode string) string {
	if e == nil {
		return permissionSurfaceChat
	}
	if _, ok := e.chats.Load(chatTurnKey(workspaceID, agentCode)); ok {
		return permissionSurfaceChat
	}
	if e.runningLoopByID(workspaceID) != nil {
		return permissionSurfaceLoop
	}
	return permissionSurfaceChat
}

// AnswerPermission resolves a pending permission request from the UI. It
// reports false when the request is already gone (answered by another window,
// timed out, or its turn ended), so the caller can drop a stale prompt.
func (e *Engine) AnswerPermission(id string, d domain.PermissionDecision) bool {
	return e.perms.Answer(id, d)
}

// PendingPermissions lists unanswered permission requests so a reloading UI can
// rebuild its queue.
func (e *Engine) PendingPermissions() []domain.PermissionRequest {
	return e.perms.Pending()
}

// engineReporter is the engine's implementation of mcp.Reporter (PRD §8.3): it is
// the SINGLE seam through which an agent's MCP tool call re-enters the engine as a
// typed signal. The live MCP HTTP handler calls these methods; the convergence
// test drives findings through the SAME interface, so the test exercises the real
// engine convergence path (not a parallel mock).
//
// Concurrency: callbacks arrive from agent subprocesses (via the MCP HTTP
// handler) concurrently with the loop goroutine. engineReporter holds no mutable
// state of its own — it looks up the running loop and routes to that loop's
// finding collector (itself mutex-guarded) and to the store (mutex-guarded). The
// engine, never the agent, owns Finding identity/iteration/status.
type engineReporter struct {
	eng *Engine
}

// ReportFinding records one reviewer finding for the loop's CURRENT review
// iteration. The engine stamps the id/iteration/status; the agent supplies only
// file/line/severity/message. It is collected for the iteration's convergence
// decision, persisted (PRD §14), and surfaced in the activity feed.
func (r engineReporter) ReportFinding(workspaceID, agentCode string, f mcp.Finding) domain.Finding {
	lp := r.eng.runningLoopByID(workspaceID)
	iter := 1
	if lp != nil {
		iter = lp.collectFinding() // increments the current-iteration count + returns it
	} else {
		// No running loop: a chat-dispatched specialist (rerun_review /
		// dispatch_specialist over a settled loop). Persist at the loop's current
		// iteration — it must not vanish, but there is no convergence count to feed.
		if ws, ok := r.eng.Store.Workspace(workspaceID); ok && ws.Iteration > 0 {
			iter = ws.Iteration
		}
	}

	df := domain.Finding{
		ID:          fmt.Sprintf("F-%s-%d", workspaceID, time.Now().UnixNano()),
		WorkspaceID: workspaceID,
		Iteration:   iter,
		File:        f.File,
		Line:        f.Line,
		EndLine:     f.EndLine,
		Severity:    f.Severity,
		Message:     f.Message,
		Summary:     strings.TrimSpace(f.Summary),
		Status:      string(domain.FindingOpen),
	}
	stored := r.eng.Store.AddFinding(df)
	// Activity shows the mcp__recur__report_finding tool row (stream KindStep);
	// no duplicate prose feed line here. The Findings left-dock panel refreshes
	// off this event — loop reviews and Review-mode chats alike.
	r.eng.Bus.Publish(Event{Name: "findings.updated", Data: map[string]any{"workspaceId": workspaceID}})
	return stored
}

// Done is the loop implementer "done" MCP signal. For the first-party Coder
// (CO) it refuses while any findings are still open so the agent must close
// them via update_finding first (continue-turn nudge). The configured implementer
// (CO, or WR on the Docs loop) stores a successful summary on the walk that
// owns this turn. Interactive Agent chats (PA…) do not get this tool
// (chatModeShape + MCP allowlist). Convergence itself remains an independent
// re-review decision (PRD §7.3).
func (r engineReporter) Done(workspaceID, agentCode, summary, notes string) error {
	if agentCode == roles[domain.RoleCoder].code {
		open := r.eng.Store.OpenFindings(workspaceID)
		if len(open) > 0 {
			var b strings.Builder
			b.WriteString(fmt.Sprintf(
				"cannot finish: %d open finding(s) remain. Address each, call mcp__recur__update_finding "+
					"(resolved or invalid with reason), then call done again.\n", len(open)))
			for i, f := range open {
				loc := domain.FindingLoc(f.File, f.Line, f.EndLine)
				b.WriteString(fmt.Sprintf("%d. id=%s [%s] %s — %s\n", i+1, f.ID, f.Severity, loc, f.Message))
				for _, n := range f.Notes {
					body := strings.TrimSpace(n.Body)
					if body == "" {
						continue
					}
					b.WriteString(fmt.Sprintf("   operator note: %s\n", body))
				}
			}
			if lp := r.eng.runningLoopForDone(workspaceID, agentCode); lp != nil && lp.acceptsDoneSummary(agentCode) {
				lp.noteDoneNotes(notes)
			}
			return fmt.Errorf("%s", strings.TrimRight(b.String(), "\n"))
		}
	}
	if lp := r.eng.runningLoopForDone(workspaceID, agentCode); lp != nil && lp.acceptsDoneSummary(agentCode) {
		lp.noteDoneSummary(summary, notes)
	}
	return nil
}

// ListFindings returns persisted findings, optionally filtered by status.
func (r engineReporter) ListFindings(workspaceID, agentCode, status string) ([]domain.Finding, error) {
	_ = agentCode
	return r.eng.Store.FindingsByStatus(workspaceID, status), nil
}

// UpdateFinding applies a status change (resolved|invalid) with attribution
// from the calling agent (coder vs reviewer).
func (r engineReporter) UpdateFinding(workspaceID, agentCode, findingID, status, reason string) (domain.Finding, error) {
	f, err := r.eng.Store.UpdateFindingStatus(workspaceID, findingID, status, reason, findingUpdateActor(r.eng, workspaceID, agentCode))
	if err != nil {
		return domain.Finding{}, err
	}
	r.eng.Bus.Publish(Event{Name: "findings.updated", Data: map[string]any{"workspaceId": workspaceID}})
	return f, nil
}

// findingUpdateActor maps an MCP caller to a persisted UpdatedBy actor.
// simplify: custom bundle reviewer codes fall through to coder; attribute via
// the loop reviewer slot / RoleModeReview if that starts to matter.
func findingUpdateActor(eng *Engine, workspaceID, agentCode string) string {
	code := strings.TrimSpace(agentCode)
	if code == roles[domain.RoleReviewer].code || code == securityReviewerRole.code {
		return domain.FindingActorReviewer
	}
	if eng != nil {
		_, _, mode := eng.WorkspaceChatSelection(workspaceID, code)
		if domain.NormalizeChatMode(mode) == domain.ChatModeReview {
			return domain.FindingActorReviewer
		}
	}
	return domain.FindingActorCoder
}

// ReportResult records the Tester pass/fail MCP signal on the loop (ADE checks)
// and surfaces it in the activity feed via the tool row from the stream.
func (r engineReporter) ReportResult(workspaceID, agentCode string, passed bool, summary string) {
	r.eng.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		l.TestResult = &domain.TestResult{
			Passed:  passed,
			Summary: summary,
			When:    "now",
		}
	})
}

// Plan records a Maestro plan / re-plan (PRD §8.3 plan, §8.5 steering). When the
// Maestro re-plans via chat (mcp__recur__plan), the engine applies the change ONLY
// WITHIN THE RAILS, logs a plan-change deviation, and queues the steer onto the
// running loop (D49: new agents get it in Prompt; in-flight agents get a stdin
// user-message inject on the next tool_result) — a rail can never be crossed via
// chat (PRD §8.5). The rails check is enforced HERE in Go (Engine.applyMaestroSteer),
// not by prompt text, so an adversarial chat message cannot talk the loop past a rail.
func (r engineReporter) Plan(workspaceID, agentCode, summary string) {
	r.eng.applyMaestroSteer(workspaceID, agentCode, summary)
}

// Escalate handles the escalate MCP tool (PRD §8.3, §13): an agent (the Maestro)
// asks the human a scoped decision it should not guess on. The MCP HTTP handler
// must return immediately, so this does NOT block — it records the request on the
// running loop, which consumes it at its next safe checkpoint and BLOCKS there
// until the human answers (PRD §13). If the loop is no longer running, the request
// is dropped (a late escalation from a stale token has no loop to pause).
func (r engineReporter) Escalate(workspaceID, agentCode string, req mcp.EscalationRequest) {
	lp := r.eng.runningLoopByID(workspaceID)
	if lp == nil {
		return
	}
	lp.requestEscalation(req)
}

// AttachEvidence records one browser-verification evidence step from the QA agent
// (PRD §8.3 attach_evidence, §10). The MCP HTTP handler must return immediately,
// so this does NOT block — it appends the step to the loop's verify collector
// (mutex-guarded) and surfaces it in the activity feed. The loop goroutine reads
// the accumulated evidence after the QA turn ends and decides verify pass/fail.
// If the loop is no longer running, the late evidence is dropped.
func (r engineReporter) AttachEvidence(workspaceID, agentCode string, e mcp.Evidence) {
	lp := r.eng.runningLoopByID(workspaceID)
	if lp == nil {
		return
	}
	lp.collectEvidence(e)
	// Activity shows the mcp__recur__attach_evidence tool row from the stream.
}

func (r engineReporter) SendBack(workspaceID, agentCode, instruction string) string {
	_ = agentCode
	return r.eng.MaestroSendBack(workspaceID, instruction)
}

// ---- Maestro hands: thin delegations to the engine actions;
// each returns the engine's status/refusal string as the tool result. ----

func (r engineReporter) EditPlan(workspaceID, agentCode, plan string) string {
	_ = agentCode
	return r.eng.EditPlan(workspaceID, plan)
}

func (r engineReporter) RerunReview(workspaceID, agentCode string) string {
	_ = agentCode
	return r.eng.DispatchSpecialistTurn(workspaceID, "reviewer",
		"Re-review the current diff against the base branch and file findings for any real issues.")
}

func (r engineReporter) DispatchSpecialist(workspaceID, agentCode, role, instruction string) string {
	_ = agentCode
	return r.eng.DispatchSpecialistTurn(workspaceID, role, instruction)
}

func (r engineReporter) ExtendIterations(workspaceID, agentCode string, n int) string {
	return r.eng.ExtendLoopIterations(workspaceID, agentCode, n)
}

func (r engineReporter) ExtendRounds(workspaceID, agentCode string, n int) string {
	return r.eng.ExtendLoopRounds(workspaceID, agentCode, n)
}

func (r engineReporter) ReenterPhase(workspaceID, agentCode, phaseID string) string {
	return r.eng.ReenterPhase(workspaceID, agentCode, phaseID)
}

func (r engineReporter) RunPhase(workspaceID, agentCode, phaseID string) string {
	return r.eng.RunOnDemandPhase(workspaceID, agentCode, phaseID)
}

func (r engineReporter) ResumeLoop(workspaceID, agentCode string) string {
	if r.eng.runningLoopByChat(workspaceID, agentCode) != nil {
		return "Refused: the loop is already running."
	}
	if strings.TrimSpace(agentCode) != "" && r.eng.runningLoopByID(workspaceID) != nil {
		ws, ok := r.eng.Store.Workspace(workspaceID)
		if !ok {
			return "Refused: that workspace no longer exists."
		}
		rec := r.eng.chatScopedWorkspace(ws, agentCode)
		rec.Retryable = true
		rec.Checkpoint = nil
		return r.eng.startAdditionalChatWalk(rec)
	}
	return r.eng.Restart(workspaceID)
}

// SubmitPlan persists a structured plan filed via mcp__recur__submit_plan
// — Plan-mode chats and the loop planner alike.
func (r engineReporter) SubmitPlan(workspaceID, agentCode string, p mcp.PlanSubmission) string {
	return r.eng.applySubmittedPlan(workspaceID, agentCode, p)
}

// UpdatePlanStep applies an agent-requested step-status change; the engine
// validates the id and recomputes run progress off the steps.
func (r engineReporter) UpdatePlanStep(workspaceID, agentCode, stepID, status, note string) string {
	return r.eng.applyPlanStepUpdate(workspaceID, agentCode, stepID, status, note)
}

// ListSkills returns the progressive-disclosure catalog (no SKILL.md bodies).
func (r engineReporter) ListSkills(workspaceID, agentCode string) ([]domain.AgentSkill, error) {
	_ = agentCode
	worktree, err := r.skillWorktree(workspaceID)
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), skillsCatalogTimeout)
	defer cancel()
	all, err := code.ListSkills(ctx, worktree)
	if err != nil {
		return nil, err
	}
	out := make([]domain.AgentSkill, 0, len(all))
	for _, s := range all {
		if s.Path == "" || code.SkillDisablesModelInvocation(s.Path) {
			continue
		}
		if !r.eng.skillEnabledForWorkspace(workspaceID, s.Name) {
			continue
		}
		// Catalog is name + description only (activate_skill loads the body).
		out = append(out, domain.AgentSkill{
			Name: s.Name, Description: s.Description, Scope: s.Scope,
		})
	}
	return out, nil
}

// ActivateSkill loads one skill's body for model-driven activation.
func (r engineReporter) ActivateSkill(workspaceID, agentCode, name string) (string, error) {
	_ = agentCode
	worktree, err := r.skillWorktree(workspaceID)
	if err != nil {
		return "", err
	}
	ctx, cancel := context.WithTimeout(context.Background(), skillActivateTimeout)
	defer cancel()
	sk, ok, err := code.SkillByName(ctx, worktree, name)
	if err != nil {
		return "", err
	}
	if !ok || sk.Path == "" {
		return "", fmt.Errorf("unknown skill %q — call list_skills for names", name)
	}
	if !r.eng.skillEnabledForWorkspace(workspaceID, name) {
		return "", fmt.Errorf("skill %q is disabled", name)
	}
	if code.SkillDisablesModelInvocation(sk.Path) {
		return "", fmt.Errorf("skill %q is user-invocable only (disable-model-invocation)", name)
	}
	body := code.ReadSkillBody(sk.Path)
	if strings.TrimSpace(body) == "" {
		return "", fmt.Errorf("skill %q has empty instructions", name)
	}
	return formatSkillContent(sk.Name, sk.Path, body), nil
}

func (r engineReporter) ListTerminals(workspaceID, agentCode string) []mcp.TerminalInfo {
	_ = agentCode
	mgr := r.eng.terminals
	if mgr == nil {
		return nil
	}
	active := mgr.ActiveID(workspaceID)
	infos := mgr.List(workspaceID)
	out := make([]mcp.TerminalInfo, 0, len(infos))
	for _, s := range infos {
		out = append(out, mcp.TerminalInfo{
			ID: s.ID, Title: s.Title, Cwd: s.Cwd, Shell: s.Shell,
			Active: s.ID == active, Exited: s.Exited, ExitCode: s.ExitCode,
		})
	}
	return out
}

func (r engineReporter) ListWorkspaceBriefs() domain.BriefingSnapshot {
	return r.eng.BriefingSnapshot()
}

func (r engineReporter) WorkspaceContext(id string) (domain.WorkspaceContext, error) {
	return r.eng.WorkspaceContext(id)
}

func (r engineReporter) PresentWorkspace(target string) string {
	return r.eng.PresentWorkspace(target)
}

func (r engineReporter) ReadTerminal(workspaceID, agentCode, id string, tailLines int) (string, error) {
	_ = agentCode
	mgr := r.eng.terminals
	if mgr == nil {
		return "", fmt.Errorf("no Observatory terminals in this session")
	}
	sessions := mgr.List(workspaceID)
	if len(sessions) == 0 {
		return "", fmt.Errorf("no live terminals in this workspace")
	}
	id = strings.TrimSpace(id)
	hits := resolveTerminalMentions(terminalMention{qual: id}, sessions, mgr.ActiveID(workspaceID))
	if len(hits) == 0 {
		return "", fmt.Errorf("terminal %q not found — call list_terminals", id)
	}
	info := hits[0]
	if id == "" && len(hits) > 1 {
		return "", fmt.Errorf("several terminals are open — pass id from list_terminals")
	}
	body, err := mgr.ReadPlain(info.ID, tailLines)
	if err != nil {
		return "", err
	}
	title := info.Title
	if title == "" {
		title = "terminal"
	}
	note := terminalExitedNote(info)
	if strings.TrimSpace(body) == "" {
		return fmt.Sprintf("terminal %s (%s%s): idle — no recent output", info.ID, title, note), nil
	}
	return fmt.Sprintf("terminal %s (%s%s):\n%s", info.ID, title, note, body), nil
}

func (r engineReporter) skillWorktree(workspaceID string) (string, error) {
	ws, ok := r.eng.Store.Workspace(workspaceID)
	if !ok {
		return "", fmt.Errorf("workspace not found")
	}
	return ws.WorktreePath, nil
}

const loopMapSep = "\x1e"

func (e *Engine) loopMapKey(rec domain.Workspace) string {
	c := strings.TrimSpace(rec.LoopChatCode)
	if c == "" {
		return rec.ID
	}
	if _, taken := e.loops.Load(rec.ID); taken {
		return rec.ID + loopMapSep + c
	}
	return rec.ID
}

func (e *Engine) forEachRunningLoop(workspaceID string, fn func(*runningLoop)) {
	if v, ok := e.loops.Load(workspaceID); ok {
		fn(v.(*runningLoop))
	}
	prefix := workspaceID + loopMapSep
	e.loops.Range(func(k, v any) bool {
		if ks, ok := k.(string); ok && strings.HasPrefix(ks, prefix) {
			fn(v.(*runningLoop))
		}
		return true
	})
}

// runningLoopByID returns a live *Loop on this workspace, or nil.
func (e *Engine) runningLoopByID(workspaceID string) *Loop {
	if v, ok := e.loops.Load(workspaceID); ok {
		return v.(*runningLoop).loop
	}
	var found *Loop
	e.forEachRunningLoop(workspaceID, func(rl *runningLoop) {
		if found == nil {
			found = rl.loop
		}
	})
	return found
}

// runningLoopForDone returns the walk that owns this done call.
// Ambiguous same-role claims return nil (goal fallback, not a sibling's summary).
// simplify: include ChatCode in the MCP binding to split overlapping implementers.
func (e *Engine) runningLoopForDone(workspaceID, agentCode string) *Loop {
	var claimed, only *Loop
	n, claims := 0, 0
	e.forEachRunningLoop(workspaceID, func(rl *runningLoop) {
		n++
		only = rl.loop
		if rl.loop.holdsDoneTurn(agentCode) {
			claims++
			claimed = rl.loop
		}
	})
	if claims == 1 {
		return claimed
	}
	if n == 1 {
		return only
	}
	return nil
}

func (e *Engine) runningLoopByChat(workspaceID, code string) *Loop {
	code = strings.TrimSpace(code)
	if v, ok := e.loops.Load(workspaceID + loopMapSep + code); ok {
		return v.(*runningLoop).loop
	}
	if v, ok := e.loops.Load(workspaceID); ok {
		lp := v.(*runningLoop).loop
		if lp.ChatCode == "" || lp.ChatCode == code {
			return lp
		}
	}
	return nil
}

// runningLoopForHand is the Maestro-hand target: this chat's walk when
// agentCode is set, else any walk on the workspace (UI / tests).
func (e *Engine) runningLoopForHand(workspaceID, agentCode string) *Loop {
	if strings.TrimSpace(agentCode) != "" {
		return e.runningLoopByChat(workspaceID, agentCode)
	}
	return e.runningLoopByID(workspaceID)
}

func (e *Engine) runningLoopRecordByChat(workspaceID, code string) *runningLoop {
	code = strings.TrimSpace(code)
	if v, ok := e.loops.Load(workspaceID + loopMapSep + code); ok {
		return v.(*runningLoop)
	}
	v, ok := e.loops.Load(workspaceID)
	if !ok {
		return nil
	}
	rl := v.(*runningLoop)
	if rl.loop.ChatCode == code {
		return rl
	}
	if rl.loop.ChatCode != "" {
		return nil
	}
	ws, found := e.Store.Workspace(workspaceID)
	if !found || loopOwnerCode(ws) != code {
		return nil
	}
	return rl
}

// stopLoopByChat cancels the walk owned by this chat tab, if any. Other
// chats' walks on the same workspace keep running. A legacy workspace-keyed
// walk (empty ChatCode) stops only when this tab is the conductor — closing
// a sibling Agent tab must not kill the run.
func (e *Engine) stopLoopByChat(workspaceID, code string) {
	rl := e.runningLoopRecordByChat(workspaceID, code)
	if rl == nil {
		return
	}
	rl.loop.send(Command{Kind: CmdStop})
	rl.cancel()
}

// idleWalkByChat cancels this tab's walk and parks it idle (composer Stop).
// Checkpoint stays so the conductor can reenter_phase. Closing a tab still
// uses stopLoopByChat (StateStopped).
func (e *Engine) idleWalkByChat(workspaceID, code string) bool {
	if rl := e.runningLoopRecordByChat(workspaceID, code); rl != nil {
		rl.loop.requestIdle()
		rl.cancel()
		return true
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok || loopOwnerCode(ws) != code {
		return false
	}
	if ws.State != domain.StateQueued {
		return false
	}
	e.dispatchMu.Lock()
	defer e.dispatchMu.Unlock()
	if rl := e.runningLoopRecordByChat(workspaceID, code); rl != nil {
		rl.loop.requestIdle()
		rl.cancel()
		return true
	}
	ws, ok = e.Store.Workspace(workspaceID)
	if !ok || loopOwnerCode(ws) != code || ws.State != domain.StateQueued {
		return false
	}
	e.setWorkspaceState(workspaceID, domain.StatePaused, "idle")
	return true
}
