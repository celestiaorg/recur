package engine

import (
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	"recur/internal/mcp"
)

// TestMaestroHands covers the engine-mediated actions through
// the SAME typed Reporter seam the live MCP handler uses: edit_plan persists +
// logs a deviation; dispatch_specialist runs a bounded reviewer whose findings
// persist WITHOUT a running loop; extend_iterations refuses honestly when the
// loop is not running (the engine names the reason).
func TestMaestroHands(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte {
		return [][]byte{
			[]byte(`{"type":"system","subtype":"init","session_id":"hand-1","model":"m","mcp_servers":[]}`),
			[]byte(`{"type":"result","subtype":"success","session_id":"hand-1","duration_ms":5,"is_error":false,"num_turns":1,"result":"done","total_cost_usd":0.01}`),
		}
	}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	// A settled LOOP with a worktree but no running goroutine (CreateWorkspace
	// coerces "loop" to solo — loops normally come from StartWorkspace — so flip
	// the type on the record; the hands only read Type + WorktreePath).
	ws, err := eng.CreateWorkspace(projID, "hands", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) { l.Type = domain.WorkspaceLoop })
	rep := engineReporter{eng: eng}

	t.Run("edit_plan persists + deviates", func(t *testing.T) {
		status := rep.EditPlan(ws.ID, "MA", "Ship only the config change; defer the UI.")
		if !strings.Contains(status, "Plan updated") {
			t.Fatalf("EditPlan status: %q", status)
		}
		got, _ := st.Workspace(ws.ID)
		if got.DefinitionOfDone != "Ship only the config change; defer the UI." {
			t.Errorf("DefinitionOfDone not persisted: %q", got.DefinitionOfDone)
		}
		var sawDeviation bool
		for _, ev := range sink.byName("workspace.event") {
			m, _ := ev.Data.(map[string]any)
			item, _ := m["event"].(domain.FeedItem)
			if item.IsDeviation && strings.Contains(item.Text, "Plan edited") {
				sawDeviation = true
			}
		}
		if !sawDeviation {
			t.Error("plan edit must log a deviation")
		}
		if s := rep.EditPlan("nope", "MA", "x"); !strings.Contains(s, "Refused") {
			t.Errorf("unknown workspace should refuse, got %q", s)
		}
	})

	t.Run("dispatch_specialist runs; findings persist without a running loop", func(t *testing.T) {
		status := rep.DispatchSpecialist(ws.ID, "MA", "reviewer", "look at the config handling")
		if !strings.Contains(status, "dispatched") {
			t.Fatalf("DispatchSpecialist status: %q", status)
		}
		waitFor(t, 3*time.Second, "dispatched turn ran", func() bool {
			s, ok := runner.specAt(0)
			return ok && s.Role == domain.RoleReviewer
		})
		s0, _ := runner.specAt(0)
		if s0.Worktree != ws.WorktreePath {
			t.Errorf("dispatched turn in wrong dir: %q", s0.Worktree)
		}
		hasBash := false
		for _, tl := range s0.AllowedTools {
			if tl == "Edit" || tl == "Write" {
				t.Errorf("dispatched reviewer must be read-only, has %q", tl)
			}
			if tl == "Bash" {
				hasBash = true
			}
		}
		if !hasBash {
			t.Errorf("dispatched reviewer needs Bash for lint/test, got %v", s0.AllowedTools)
		}
		// The mode is machine-global (D63) — a dispatched turn tracks the setting
		// like every other turn, rather than deriving anything from its tools.
		if want := st.Settings().PermissionMode(); s0.PermissionMode != want {
			t.Errorf("dispatched reviewer permission mode %q, want the global %q", s0.PermissionMode, want)
		}
		// A finding filed by the dispatched reviewer (no running loop) persists.
		rep.ReportFinding(ws.ID, "RV", mcp.Finding{File: "a.go", Line: 3, Severity: domain.SevHigh, Message: "bug"})
		if got := st.OpenFindingsCount(ws.ID); got != 1 {
			t.Errorf("dispatched finding must persist without a running loop, got %d", got)
		}
	})

	t.Run("extend_iterations refuses with the reason when not running", func(t *testing.T) {
		status := rep.ExtendIterations(ws.ID, "MA", 2)
		if !strings.Contains(status, "Refused") || !strings.Contains(status, "not running") {
			t.Errorf("expected a named refusal, got %q", status)
		}
	})
}

func TestEditPlanClearsApprovalAndKeepsSummary(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return [][]byte{
			[]byte(`{"type":"system","subtype":"init","session_id":"edit-1","model":"m","mcp_servers":[]}`),
			[]byte(`{"type":"result","subtype":"success","session_id":"edit-1","duration_ms":5,"is_error":false,"num_turns":1,"result":"done","total_cost_usd":0.01}`),
		}
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "edit-plan", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) { l.Type = domain.WorkspaceLoop })
	rep := engineReporter{eng: eng}
	_ = rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary: "Curated headline",
		Body:    "Original body.",
		Steps:   []string{"Do the thing"},
	})
	if s := eng.ApprovePlan(ws.ID); s != "Plan approved." {
		t.Fatalf("ApprovePlan = %q", s)
	}
	status := eng.EditPlan(ws.ID, "A substantially different plan body.")
	if !strings.Contains(status, "Plan updated") {
		t.Fatalf("EditPlan = %q", status)
	}
	got, _ := st.Workspace(ws.ID)
	if got.Plan == nil {
		t.Fatal("plan missing")
	}
	if got.Plan.Approved {
		t.Error("EditPlan should reset Approved")
	}
	if got.Plan.Summary != "Curated headline" {
		t.Errorf("Summary = %q, want curated headline kept", got.Plan.Summary)
	}
	if got.Plan.Body != "A substantially different plan body." {
		t.Errorf("Body = %q, want the rewritten plan", got.Plan.Body)
	}
}

func TestMaestroHandsScopeToCallingChat(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("scope", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scope-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) { l.Type = domain.WorkspaceLoop })
	ws, _ = st.Workspace(ws.ID)

	sibling := &Loop{ID: ws.ID, ChatCode: "PA2"}
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: sibling})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	if ctx := eng.maestroContextForChat(ws, "PA"); ctx.Live {
		t.Fatal("idle chat must not inherit a sibling walk's liveness")
	}
	if ctx := eng.maestroContextForChat(ws, "PA2"); !ctx.Live {
		t.Fatal("live chat must see its own walk")
	}

	if s := eng.ExtendLoopIterations(ws.ID, "PA", 2); !strings.Contains(s, "not running") {
		t.Fatalf("idle extend_iterations = %q, want not running", s)
	}
	if s := eng.ExtendLoopIterations(ws.ID, "PA2", 2); !strings.Contains(s, "Granted") {
		t.Fatalf("live extend_iterations = %q, want Granted", s)
	}
	if sibling.grantedExtra != 2 {
		t.Fatalf("grantedExtra = %d, want 2 on the calling chat's walk", sibling.grantedExtra)
	}

	g, reason := eng.resolveGuard(ws.LoopID, ws.PolicyID, ws.Autonomy)
	if reason != "" {
		t.Fatalf("resolveGuard: %s", reason)
	}
	ids := g.Loop.ReenterIDs()
	if len(ids) == 0 {
		t.Fatal("no reenter ids")
	}
	phase := ids[0]
	if s := eng.ReenterPhase(ws.ID, "PA2", phase); !strings.Contains(s, "idle only") {
		t.Fatalf("live reenter = %q, want idle only", s)
	}

	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	if s := eng.ReenterPhase(ws.ID, "PA", phase); !strings.Contains(s, "loop slot") {
		t.Fatalf("idle reenter with sibling live = %q, want slot refusal (not sibling idle-only)", s)
	}

	eng.applyMaestroSteer(ws.ID, "PA", "steer only this chat")
	if sibling.pendingSteer != "" {
		t.Fatalf("idle chat steered sibling: %q", sibling.pendingSteer)
	}
}

func newHandLoop(workspaceID, chatCode string) *Loop {
	lp := &Loop{ID: workspaceID, ChatCode: chatCode, cmds: make(chan Command, 8)}
	lp.cond = sync.NewCond(&lp.mu)
	return lp
}

func TestExtendLoopRoundsAnswersSuffixedLoop(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("cap", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cap-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StateEscalated
		l.Checkpoint = &domain.Checkpoint{PhaseID: "verify"}
	})
	st.AddEscalation(domain.Escalation{ID: "esc-cap", WorkspaceID: ws.ID, Status: "open", Question: "cap"})

	root := newHandLoop(ws.ID, "PA")
	root.escalated = true
	eng.loops.Store(ws.ID, &runningLoop{loop: root})
	t.Cleanup(func() { eng.loops.Delete(ws.ID) })

	cap := newHandLoop(ws.ID, "PA2")
	cap.escalated = true
	cap.roundCapEsc = true
	cap.currentPhase = bundles.LoopPhase{ID: "converge", Uses: bundles.VerbConverge}
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: cap})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	got := eng.ExtendLoopRounds(ws.ID, "PA2", 0)
	if !strings.Contains(got, "Granted") {
		t.Fatalf("ExtendLoopRounds = %q, want Granted", got)
	}
	if cap.answer == nil || cap.escalated {
		t.Fatal("suffixed walk must receive the cap answer")
	}
	if root.answer != nil || !root.escalated {
		t.Fatal("root sibling must not receive the cap answer")
	}
}

func TestMaestroContextStateIsChatLocal(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("ctx", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ctx-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StateCoding
		l.Iteration = 9
	})
	ws, _ = st.Workspace(ws.ID)

	cap := newHandLoop(ws.ID, "PA2")
	cap.escalated = true
	cap.roundCapEsc = true
	cap.walkState = domain.StateEscalated
	cap.walkIter = 4
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: cap})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	ctx := eng.maestroContextForChat(ws, "PA2")
	if !ctx.Live || ctx.State != string(domain.StateEscalated) {
		t.Fatalf("live cap chat: Live=%v State=%q, want escalated", ctx.Live, ctx.State)
	}
	if ctx.Iteration != 4 {
		t.Fatalf("Iteration = %d, want 4 from this walk", ctx.Iteration)
	}
	fresh := newHandLoop(ws.ID, "PA3")
	fresh.walkIter = 0
	fresh.Goal = "chat-local goal"
	eng.loops.Store(ws.ID+loopMapSep+"PA3", &runningLoop{loop: fresh})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA3") })
	zero := eng.maestroContextForChat(ws, "PA3")
	if zero.Iteration != 0 {
		t.Fatalf("Iteration = %d, want 0 from this walk (not workspace 9)", zero.Iteration)
	}
	if zero.Goal != "chat-local goal" {
		t.Fatalf("Goal = %q, want chat-local goal", zero.Goal)
	}
	idle := eng.maestroContextForChat(ws, "PA")
	if idle.Live || idle.State != string(domain.StateCoding) {
		t.Fatalf("idle sibling: Live=%v State=%q, want coding and not live", idle.Live, idle.State)
	}
}

func TestReenterPhaseUsesCallingChatGraph(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("reenter", "OK.")
	}})
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "reenter-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StatePaused
		l.LoopChatCode = "PA"
		l.LoopID = ""
	})
	eng.RememberChatLoopPolicy(ws.ID, "PA2", "test.loops.verify", "")

	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()

	if s := eng.ReenterPhase(ws.ID, "PA2", "verify"); strings.Contains(s, "not on this loop") {
		t.Fatalf("calling chat's graph must allow verify, got %q", s)
	} else if !strings.Contains(s, "Loop restarting") {
		t.Fatalf("idle reenter = %q, want Loop restarting", s)
	}
	got, _ := st.Workspace(ws.ID)
	if got.LoopID != "test.loops.verify" {
		t.Fatalf("LoopID = %q, want test.loops.verify", got.LoopID)
	}
	if got.LoopChatCode != "PA2" {
		t.Fatalf("LoopChatCode = %q, want PA2", got.LoopChatCode)
	}
}

func TestResumeWakesSuffixedLoop(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("resume", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "resume-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	lp := newHandLoop(ws.ID, "PA2")
	lp.paused = true
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: lp})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	eng.Resume(ws.ID)
	if lp.paused {
		t.Fatal("Resume must wake a chat-suffixed paused walk")
	}
}

func TestExtendLoopRoundsDoesNotAnswerSiblingEscalation(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("cap-sib", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cap-sib", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StateEscalated
	})
	st.AddEscalation(domain.Escalation{ID: "esc-a", WorkspaceID: ws.ID, AgentCode: "PA", Status: "open", Question: "A cap"})
	st.AddEscalation(domain.Escalation{ID: "esc-b", WorkspaceID: ws.ID, AgentCode: "PA2", Status: "open", Question: "B cap"})

	root := newHandLoop(ws.ID, "PA")
	root.escalated = true
	eng.loops.Store(ws.ID+loopMapSep+"PA", &runningLoop{loop: root})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA") })

	cap := newHandLoop(ws.ID, "PA2")
	cap.escalated = true
	cap.roundCapEsc = true
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: cap})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	got := eng.ExtendLoopRounds(ws.ID, "PA2", 0)
	if !strings.Contains(got, "Granted") {
		t.Fatalf("ExtendLoopRounds = %q, want Granted", got)
	}
	if cap.answer == nil || cap.escalated {
		t.Fatal("PA2 must receive the cap answer")
	}
	if root.answer != nil || !root.escalated {
		t.Fatal("PA sibling must stay blocked on its own escalation")
	}
	esc, ok := st.AnswerEscalationFor(ws.ID, "PA", "later", "")
	if !ok || esc.Question != "A cap" {
		t.Fatalf("PA escalation AnswerEscalationFor = ok=%v %+v, want still open question A", ok, esc)
	}
}

func TestChatScopedWorkspaceRestoresGoal(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("goal", "OK.")
	}})
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "goal-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StatePaused
		l.LoopChatCode = "PA"
		l.Goal = "owner goal"
		l.GoalDisplay = "Linear ENG-123 · title"
		l.Checkpoint = &domain.Checkpoint{PhaseID: "planning"}
	})
	ws, _ = st.Workspace(ws.ID)
	eng.stampChatLoop(ws.ID, "PA2", "test.loops.verify", "", "chat goal", "", true)

	rec := eng.chatScopedWorkspace(ws, "PA2")
	if rec.Goal != "chat goal" {
		t.Fatalf("chatScopedWorkspace Goal = %q, want chat goal", rec.Goal)
	}
	if rec.GoalDisplay != "" {
		t.Fatalf("chatScopedWorkspace GoalDisplay = %q, want empty after a different goal", rec.GoalDisplay)
	}
	if rec.Checkpoint != nil && rec.Checkpoint.PhaseID == "planning" {
		t.Fatalf("chatScopedWorkspace Checkpoint = %+v, want not sibling planning", rec.Checkpoint)
	}

	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	if s := eng.ReenterPhase(ws.ID, "PA2", "verify"); !strings.Contains(s, "Loop restarting") {
		t.Fatalf("idle reenter = %q, want Loop restarting", s)
	}
	got, _ := st.Workspace(ws.ID)
	if got.Goal != "chat goal" {
		t.Fatalf("persisted Goal = %q, want chat goal", got.Goal)
	}
	if got.GoalDisplay != "" {
		t.Fatalf("persisted GoalDisplay = %q, want empty after a different goal", got.GoalDisplay)
	}
}

func TestReenterPhaseDoesNotMutateQueuedRun(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("queued-reenter", "OK.")
	}})
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "queued-reenter", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StateQueued
		l.LoopChatCode = "PA"
		l.LoopID = "owner.loop"
		l.Checkpoint = &domain.Checkpoint{PhaseID: "converge"}
		l.Goal = "queued owner goal"
	})
	eng.RememberChatLoopPolicy(ws.ID, "PA2", "test.loops.verify", "")
	eng.stampChatLoop(ws.ID, "PA2", "test.loops.verify", "", "second chat goal", "", true)

	if s := eng.ReenterPhase(ws.ID, "PA2", "verify"); !strings.Contains(s, "queued") {
		t.Fatalf("queued reenter = %q, want queued refusal", s)
	}
	got, _ := st.Workspace(ws.ID)
	if got.LoopChatCode != "PA" {
		t.Fatalf("LoopChatCode = %q, want PA (queued owner)", got.LoopChatCode)
	}
	if got.LoopID != "owner.loop" {
		t.Fatalf("LoopID = %q, want owner.loop", got.LoopID)
	}
	if got.Checkpoint == nil || got.Checkpoint.PhaseID != "converge" {
		t.Fatalf("Checkpoint = %+v, want converge", got.Checkpoint)
	}
	if got.Goal != "queued owner goal" {
		t.Fatalf("Goal = %q, want queued owner goal", got.Goal)
	}
}

func TestAnswerEscalationDeliversToOwningChat(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("esc-own", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "esc-own", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StateEscalated
		l.Group = domain.GroupNeeds
	})
	st.AddEscalation(domain.Escalation{ID: "esc-old", WorkspaceID: ws.ID, Status: "answered", Question: "stale", Answer: "done"})
	st.AddEscalation(domain.Escalation{ID: "esc-a", WorkspaceID: ws.ID, AgentCode: "PA", Status: "open", Question: "A"})
	st.AddEscalation(domain.Escalation{ID: "esc-b", WorkspaceID: ws.ID, AgentCode: "PA2", Status: "open", Question: "B"})

	root := newHandLoop(ws.ID, "PA")
	root.escalated = true
	eng.loops.Store(ws.ID+loopMapSep+"PA", &runningLoop{loop: root})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA") })

	sib := newHandLoop(ws.ID, "PA2")
	sib.escalated = true
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: sib})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	answered, ok := eng.AnswerEscalation(ws.ID, "Stop", "")
	if !ok || answered.Question != "A" {
		t.Fatalf("AnswerEscalation = ok=%v %+v, want question A", ok, answered)
	}
	if root.answer == nil || root.escalated {
		t.Fatal("owning PA walk must receive the answer")
	}
	if sib.answer != nil || !sib.escalated {
		t.Fatal("PA2 must stay blocked on its own question")
	}
	got, _ := st.Workspace(ws.ID)
	if got.Group != domain.GroupNeeds {
		t.Fatalf("Group = %s, want needs while PA2 is still open", got.Group)
	}
}

func TestAnswerEscalationForRoutesToNamedChat(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("esc-named", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "esc-named", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StateEscalated
		l.Group = domain.GroupNeeds
	})
	st.AddEscalation(domain.Escalation{ID: "esc-a", WorkspaceID: ws.ID, AgentCode: "PA", Status: "open", Question: "A"})
	st.AddEscalation(domain.Escalation{ID: "esc-b", WorkspaceID: ws.ID, AgentCode: "PA2", Status: "open", Question: "B"})

	root := newHandLoop(ws.ID, "PA")
	root.escalated = true
	eng.loops.Store(ws.ID+loopMapSep+"PA", &runningLoop{loop: root})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA") })

	sib := newHandLoop(ws.ID, "PA2")
	sib.escalated = true
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: sib})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	answered, ok := eng.AnswerEscalationFor(ws.ID, "PA2", "Stop", "")
	if !ok || answered.Question != "B" {
		t.Fatalf("AnswerEscalationFor(PA2) = ok=%v %+v, want question B", ok, answered)
	}
	if sib.answer == nil || sib.escalated {
		t.Fatal("named PA2 walk must receive the answer")
	}
	if root.answer != nil || !root.escalated {
		t.Fatal("PA must stay blocked on its own question")
	}
}

func TestRestartRefusesWhenSiblingLive(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("restart-sib", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "restart-sib", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StateStopped
	})
	sib := newHandLoop(ws.ID, "PA2")
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: sib})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	if s := eng.Restart(ws.ID); !strings.Contains(s, "already running") {
		t.Fatalf("Restart with sibling live = %q, want already running", s)
	}
}

func TestResumeLoopStartsWalkWhenSiblingLive(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("resume-sib", "OK.")
	}})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "resume-sib", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StateStopped
		l.Checkpoint = &domain.Checkpoint{PhaseID: "converge"}
	})
	st.UpsertAgent(ws.ID, domain.Agent{ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", Name: "Agent"})
	ws, _ = st.Workspace(ws.ID)
	sib := newHandLoop(ws.ID, "PA2")
	eng.loops.Store(ws.ID+loopMapSep+"PA2", &runningLoop{loop: sib})
	t.Cleanup(func() { eng.loops.Delete(ws.ID + loopMapSep + "PA2") })

	got := engineReporter{eng: eng}.ResumeLoop(ws.ID, "PA")
	if !strings.Contains(got, "restarting") {
		t.Fatalf("ResumeLoop with sibling live = %q, want restarting", got)
	}
	t.Cleanup(func() {
		if rl := eng.runningLoopRecordByChat(ws.ID, "PA"); rl != nil {
			rl.loop.send(Command{Kind: CmdStop})
			rl.cancel()
		}
	})
	if eng.runningLoopByChat(ws.ID, "PA") == nil {
		t.Fatal("ResumeLoop must start this chat's walk")
	}
	if lp := eng.runningLoopByChat(ws.ID, "PA"); lp.startPhase != "" {
		t.Fatalf("startPhase = %q, want empty (fresh restart, not sibling checkpoint)", lp.startPhase)
	}
	if eng.runningLoopByChat(ws.ID, "PA2") != sib {
		t.Fatal("sibling walk must stay")
	}
}

func TestIdleRunPhaseDoesNotAdoptSiblingCheckpoint(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("phase-cp", "OK.")
	}})
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "phase-cp", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StatePaused
		l.LoopChatCode = "PA"
		l.Checkpoint = &domain.Checkpoint{PhaseID: "converge"}
	})
	eng.RememberChatLoopPolicy(ws.ID, "PA2", "test.loops.verify", "")

	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	if s := eng.RunOnDemandPhase(ws.ID, "PA2", "ondemand"); !strings.Contains(s, "Loop restarting") {
		t.Fatalf("idle run_phase = %q, want Loop restarting", s)
	}
	got, _ := st.Workspace(ws.ID)
	if got.Checkpoint == nil || got.Checkpoint.PhaseID != "ondemand" {
		t.Fatalf("Checkpoint = %+v, want ondemand", got.Checkpoint)
	}
	if got.Checkpoint.ResumePhaseID != "" {
		t.Fatalf("ResumePhaseID = %q, want empty (do not resume a sibling's parked phase)", got.Checkpoint.ResumePhaseID)
	}
}

func TestReenterPhaseSerializesIdleOwner(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("reenter-lock", "OK.")
	}})
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "reenter-lock", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StatePaused
		l.LoopChatCode = "PA"
	})
	eng.RememberChatLoopPolicy(ws.ID, "PA2", "test.loops.verify", "")
	g, reason := eng.resolveGuard("", "", domain.AutonomyBalanced)
	if reason != "" {
		t.Fatalf("resolveGuard: %s", reason)
	}
	ids := g.Loop.ReenterIDs()
	if len(ids) == 0 {
		t.Fatal("no reenter ids")
	}

	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()

	var a, b string
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		a = eng.ReenterPhase(ws.ID, "PA", ids[0])
	}()
	go func() {
		defer wg.Done()
		b = eng.ReenterPhase(ws.ID, "PA2", "verify")
	}()
	wg.Wait()

	restarting := 0
	if strings.Contains(a, "Loop restarting") {
		restarting++
	}
	if strings.Contains(b, "Loop restarting") {
		restarting++
	}
	if restarting != 1 {
		t.Fatalf("restarting count=%d a=%q b=%q, want exactly one (the other must refuse)", restarting, a, b)
	}
	got, _ := st.Workspace(ws.ID)
	if strings.Contains(a, "Loop restarting") && got.LoopChatCode != "PA" {
		t.Fatalf("LoopChatCode = %q, want PA (the caller that was accepted)", got.LoopChatCode)
	}
	if strings.Contains(b, "Loop restarting") && got.LoopChatCode != "PA2" {
		t.Fatalf("LoopChatCode = %q, want PA2 (the caller that was accepted)", got.LoopChatCode)
	}
}

func TestReenterPhaseSameChatDoesNotDoubleDispatch(t *testing.T) {
	gate := make(chan struct{})
	runner := newCountingRunner()
	runner.gate = gate
	eng, st, _, _ := newTestEngine(t, runner)
	t.Cleanup(func() { close(gate) })
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "reenter-same", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.State = domain.StatePaused
		l.LoopChatCode = "PA"
	})
	st.UpsertAgent(ws.ID, domain.Agent{ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", Name: "Agent"})
	g, reason := eng.resolveGuard("", "", domain.AutonomyBalanced)
	if reason != "" {
		t.Fatalf("resolveGuard: %s", reason)
	}
	ids := g.Loop.ReenterIDs()
	if len(ids) == 0 {
		t.Fatal("no reenter ids")
	}

	entered := make(chan struct{})
	eng.promoteHook = func() {
		select {
		case <-entered:
		default:
			close(entered)
		}
		time.Sleep(40 * time.Millisecond)
	}

	var a, b string
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		a = eng.ReenterPhase(ws.ID, "PA", ids[0])
	}()
	select {
	case <-entered:
	case <-time.After(5 * time.Second):
		t.Fatal("first ReenterPhase never entered dispatch")
	}
	go func() {
		defer wg.Done()
		b = eng.ReenterPhase(ws.ID, "PA", ids[0])
	}()
	wg.Wait()

	restarting := 0
	if strings.Contains(a, "Loop restarting") {
		restarting++
	}
	if strings.Contains(b, "Loop restarting") {
		restarting++
	}
	if restarting != 1 {
		t.Fatalf("restarting count=%d a=%q b=%q, want exactly one", restarting, a, b)
	}
	n := 0
	if eng.runningLoopByID(ws.ID) != nil {
		n++
	}
	if _, ok := eng.loops.Load(ws.ID + loopMapSep + "PA"); ok {
		n++
	}
	if n != 1 {
		t.Fatalf("live walks for this chat = %d, want 1", n)
	}
}
