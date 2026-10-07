package engine

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

func saveTestSchedule(t *testing.T, eng *Engine, projectID string) domain.ScheduledPrompt {
	t.Helper()
	eng.nowFn = func() time.Time { return time.Date(2026, 9, 8, 8, 0, 0, 0, time.UTC) }
	p, err := eng.SaveScheduledPrompt(domain.ScheduledPrompt{
		ProjectID: projectID, Name: "Check tests", Prompt: "Find missing tests", BaseBranch: "main",
		Mode: "ask", Model: "claude-code/sonnet", Cadence: "daily", Time: "09:00", TimeZone: "UTC",
	})
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func scheduleRuns(t *testing.T, eng *Engine) []domain.ScheduledPromptRun {
	t.Helper()
	runs, err := eng.Store.ScheduledPromptRuns()
	if err != nil {
		t.Fatal(err)
	}
	return runs
}

func TestScheduledPromptRunsOnceInFreshWorkspace(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("scheduled-session", "Tests checked.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	due := time.UnixMilli(p.NextRunAt)
	for i := 0; i < 2; i++ {
		eng.scheduleMu.Lock()
		err := eng.tickScheduledPrompts(due, false)
		eng.scheduleMu.Unlock()
		if err != nil {
			t.Fatal(err)
		}
	}
	eng.scheduleWG.Wait()
	runs := scheduleRuns(t, eng)
	if len(runs) != 1 {
		t.Fatalf("runs = %d, want 1", len(runs))
	}
	if runs[0].Status != "finished" {
		t.Errorf("run = %+v, want finished", runs[0])
	}
	ws, ok := st.Workspace(runs[0].WorkspaceID)
	if !ok || ws.WorktreePath == "" || ws.ScheduledPromptPending {
		t.Fatalf("workspace = %+v, want allocated and settled", ws)
	}
	if ws.ScheduleID != p.ID {
		t.Errorf("ScheduleID = %q, want %s", ws.ScheduleID, p.ID)
	}
	spec, ok := runner.specWithRole(domain.RoleCoder)
	if !ok || spec.Worktree != ws.WorktreePath {
		t.Errorf("runner spec = %+v, want run in %s", spec, ws.WorktreePath)
	}
	for _, tool := range spec.AllowedTools {
		if tool == "Edit" || tool == "Write" {
			t.Errorf("Ask scheduled run exposed %s", tool)
		}
	}
	id, err := eng.RunScheduledPromptNow(p.ID)
	if err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	if id == ws.ID {
		t.Errorf("Run now reused workspace %s", id)
	}
}

func TestScheduledPromptMissedPausedAndDeleted(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	due := time.UnixMilli(p.NextRunAt)
	if err := eng.tickScheduledPrompts(due, true); err != nil {
		t.Fatal(err)
	}
	runs := scheduleRuns(t, eng)
	if len(runs) != 1 || runs[0].Status != "missed" || runs[0].WorkspaceID != "" {
		t.Fatalf("startup runs = %+v, want one missed run", runs)
	}
	if err := eng.tickScheduledPrompts(due.AddDate(0, 0, 1).Add(2*time.Minute), false); err != nil {
		t.Fatal(err)
	}
	runs = scheduleRuns(t, eng)
	if len(runs) != 2 || runs[1].Status != "missed" {
		t.Fatalf("wake runs = %+v, want second missed run", runs)
	}
	if err := eng.PauseScheduledPrompt(p.ID, true); err != nil {
		t.Fatal(err)
	}
	if err := eng.tickScheduledPrompts(due.AddDate(0, 0, 1), false); err != nil {
		t.Fatal(err)
	}
	if got := len(scheduleRuns(t, eng)); got != 2 {
		t.Errorf("paused runs = %d, want 2", got)
	}
	if err := eng.DeleteScheduledPrompt(p.ID); err != nil {
		t.Fatal(err)
	}
	if err := eng.tickScheduledPrompts(due.AddDate(0, 0, 2), false); err != nil {
		t.Fatal(err)
	}
	if got := len(scheduleRuns(t, eng)); got != 0 {
		t.Errorf("deleted runs = %d, want 0", got)
	}
}

func TestScheduledPromptOverlapAndRestart(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	r := domain.ScheduledPromptRun{ID: "previous", ScheduleID: p.ID, DueAt: p.NextRunAt - 86400000, Status: "running"}
	if _, err := st.ClaimScheduledPrompt(p, r); err != nil {
		t.Fatal(err)
	}
	if _, err := eng.RunScheduledPromptNow(p.ID); err == nil {
		t.Error("Run now accepted overlapping execution")
	}
	if err := eng.tickScheduledPrompts(time.UnixMilli(p.NextRunAt), false); err != nil {
		t.Fatal(err)
	}
	runs := scheduleRuns(t, eng)
	if len(runs) != 2 || runs[1].Status != "skipped" {
		t.Fatalf("overlap runs = %+v, want skipped", runs)
	}
	if err := eng.tickScheduledPrompts(time.UnixMilli(p.NextRunAt), true); err != nil {
		t.Fatal(err)
	}
	runs = scheduleRuns(t, eng)
	if runs[0].Status != "interrupted" {
		t.Errorf("recovered run = %+v, want interrupted", runs[0])
	}
	claimed, err := st.ClaimScheduledPrompt(p, r)
	if err != nil || claimed {
		t.Errorf("claim after recovery = %v, %v, want false, nil", claimed, err)
	}
}

func TestScheduledPromptInterruptedOverlap(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	ws, err := eng.CreateWorkspace(p.ProjectID, "Needs you", "main")
	if err != nil {
		t.Fatal(err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.State = domain.StateEscalated
		w.Phase = "asking a question"
	})
	r := domain.ScheduledPromptRun{ID: "previous", ScheduleID: p.ID, DueAt: p.NextRunAt - 86400000, Status: "running"}
	if _, err := st.ClaimScheduledPrompt(p, r); err != nil {
		t.Fatal(err)
	}
	if err := st.LinkScheduledPromptRun(r.ID, ws.ID); err != nil {
		t.Fatal(err)
	}
	if err := eng.tickScheduledPrompts(eng.now(), true); err != nil {
		t.Fatal(err)
	}
	if got := scheduleRuns(t, eng); len(got) != 1 || got[0].Status != "interrupted" {
		t.Fatalf("startup runs = %+v, want interrupted", got)
	}
	if _, err := eng.RunScheduledPromptNow(p.ID); err == nil {
		t.Error("Run now launched over an interrupted run still waiting for input")
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) { w.State = domain.StateCoding; w.Phase = "coding" })
	if _, err := eng.RunScheduledPromptNow(p.ID); err == nil {
		t.Error("Run now launched over a resumed interrupted run")
	}
	if err := eng.tickScheduledPrompts(time.UnixMilli(p.NextRunAt), false); err != nil {
		t.Fatal(err)
	}
	runs := scheduleRuns(t, eng)
	if len(runs) != 2 || runs[1].Status != "skipped" {
		t.Fatalf("overlap after interrupt = %+v, want skipped", runs)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) { w.State = domain.StatePaused; w.Phase = "idle" })
	if _, err := eng.RunScheduledPromptNow(p.ID); err != nil {
		t.Errorf("idle interrupted workspace blocked Run now: %v", err)
	}
}

func TestScheduledPromptSetupFailure(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	proj := addRunProject(t, st, domain.Project{ID: "schedule-fail", Name: "Fail", RepoPath: initRepo(t), BaseBranch: "main", SetupCmd: "exit 1"})
	p := saveTestSchedule(t, eng, proj.ID)
	id, err := eng.RunScheduledPromptNow(p.ID)
	if err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	runs := scheduleRuns(t, eng)
	if len(runs) != 1 || runs[0].Status != "failed" || runs[0].Detail == "" || runs[0].WorkspaceID != id {
		t.Errorf("setup failure = %+v, want failure with workspace and detail", runs)
	}
}

func TestScheduledPromptRecoveryDoesNotResumeChat(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	proj := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(proj, "Interrupted scheduled run", "main")
	if err != nil {
		t.Fatal(err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) { w.ScheduledPromptPending = true })
	eng.Recover()
	if len(eng.interruptedChats) != 0 {
		t.Errorf("scheduled recovery queued %+v, want no auto-resume", eng.interruptedChats)
	}
	ws, _ = st.Workspace(ws.ID)
	if ws.ScheduledPromptPending {
		t.Error("recovery did not release marker for subsequent manual chat turns")
	}
}

func TestScheduledPromptValidation(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	for _, mutate := range []func(*domain.ScheduledPrompt){
		func(p *domain.ScheduledPrompt) { p.Prompt = " " },
		func(p *domain.ScheduledPrompt) { p.ProjectID = "missing" },
		func(p *domain.ScheduledPrompt) { p.BaseBranch = "missing" },
		func(p *domain.ScheduledPrompt) { p.Mode = "invalid" },
		func(p *domain.ScheduledPrompt) { p.Model = "invalid/model" },
		func(p *domain.ScheduledPrompt) { p.TimeZone = "Local" },
	} {
		bad := p
		mutate(&bad)
		if _, err := eng.SaveScheduledPrompt(bad); err == nil {
			t.Errorf("SaveScheduledPrompt accepted %+v", bad)
		}
	}
}

func TestScheduledPromptProviderFailure(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &durationErrorRunner{})
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	if _, err := eng.RunScheduledPromptNow(p.ID); err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	runs := scheduleRuns(t, eng)
	if len(runs) != 1 || runs[0].Status != "failed" || runs[0].Detail == "" {
		t.Errorf("provider failure = %+v, want failed with detail", runs)
	}
}

func TestScheduledPromptRunNowAtDueTimeConsumesOccurrence(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("scheduled", "Done") }})
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	due := time.UnixMilli(p.NextRunAt)
	eng.nowFn = func() time.Time { return due }
	if _, err := eng.RunScheduledPromptNow(p.ID); err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	if err := eng.tickScheduledPrompts(due, false); err != nil {
		t.Fatal(err)
	}
	saved, err := eng.scheduledPrompt(p.ID)
	if err != nil {
		t.Fatal(err)
	}
	if saved.NextRunAt <= due.UnixMilli() || len(scheduleRuns(t, eng)) != 1 {
		t.Errorf("run-now at due time left schedule %+v, want next day and one run", saved)
	}
}

func TestScheduledPromptShutdownJoinsWorker(t *testing.T) {
	for _, mode := range []string{domain.ChatModeAsk, domain.ChatModeLoop} {
		t.Run(mode, func(t *testing.T) {
			eng, st, _, _ := newTestEngine(t, &ctxRunner{})
			p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
			p.Mode = mode
			if _, err := eng.SaveScheduledPrompt(p); err != nil {
				t.Fatal(err)
			}
			if err := eng.StartScheduledPrompts(); err != nil {
				t.Fatal(err)
			}
			id, err := eng.RunScheduledPromptNow(p.ID)
			if err != nil {
				t.Fatal(err)
			}
			active := func() bool {
				return eng.liveChatTurn(id, pairAgentCode) != nil || eng.runningLoopByID(id) != nil
			}
			waitFor(t, 5*time.Second, "scheduled execution starts", active)
			eng.Shutdown()
			select {
			case <-eng.scheduleDone:
			default:
				t.Error("Shutdown left scheduler running")
			}
			if active() {
				t.Error("Shutdown left scheduled execution running")
			}
			if _, err := eng.RunScheduledPromptNow(p.ID); err == nil {
				t.Error("Run now accepted after shutdown")
			}
		})
	}
}

func TestScheduledPromptRemountBlocksOverlap(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("scheduled", "Done") }})
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	id, err := eng.RunScheduledPromptNow(p.ID)
	if err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	gen := eng.allocContinueGen(id, pairAgentCode)
	r := scheduleRuns(t, eng)[0]
	r.Status = "waiting"
	if err := st.UpdateScheduledPromptRun(r); err != nil {
		t.Fatal(err)
	}
	st.UpdateWorkspace(id, func(w *domain.Workspace) { w.ScheduledPromptPending = true })
	if _, err := eng.RunScheduledPromptNow(p.ID); err == nil {
		t.Error("Run now accepted while a mode remount was pending")
	}
	if err := eng.tickScheduledPrompts(eng.now(), false); err != nil {
		t.Fatal(err)
	}
	if got := scheduleRuns(t, eng)[0].Status; got != "waiting" {
		t.Errorf("pending remount status = %s, want waiting", got)
	}
	eng.dropContinueGen(gen)
	if err := eng.tickScheduledPrompts(eng.now(), false); err != nil {
		t.Fatal(err)
	}
	ws, _ := st.Workspace(id)
	if got := scheduleRuns(t, eng)[0].Status; got != "finished" || ws.ScheduledPromptPending {
		t.Errorf("settled remount status = %s, pending = %v, want finished and false", got, ws.ScheduledPromptPending)
	}
}

func TestScheduledPromptPlanAndLoop(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("scheduled", "Plan ready") }}
	eng, st, _, _ := newTestEngine(t, runner)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	p.Mode, p.Cadence, p.Cron = domain.ChatModePlan, "cron", "*/15 9-17 * * MON-FRI"
	p.Flags, p.Simplify = map[string]string{"effort": "low"}, boolPtr(false)
	var err error
	if p, err = eng.SaveScheduledPrompt(p); err != nil {
		t.Fatal(err)
	}
	if _, err = eng.RunScheduledPromptNow(p.ID); err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	if runs := scheduleRuns(t, eng); len(runs) != 1 || runs[0].Status != "finished" {
		t.Fatalf("plan runs = %+v", runs)
	}

	spec, ok := runner.specWithRole(domain.RoleCoder)
	if !ok || spec.Flags["effort"] != "low" || strings.Contains(spec.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Fatalf("scheduled plan options = %+v", spec)
	}
	planRun := scheduleRuns(t, eng)[0]
	if on := eng.ChatSimplify(planRun.WorkspaceID, pairAgentCode); on == nil || *on {
		t.Fatalf("saved Simplify off = %v", on)
	}

	// Keep the real Loop kickoff queued so lifecycle assertions are deterministic.
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	p.ID, p.Mode = "", domain.ChatModeLoop
	p.Name, p.Prompt = "Short name", "Do the full task\nwith extra instructions"
	if p, err = eng.SaveScheduledPrompt(p); err != nil {
		t.Fatal(err)
	}
	if p.LoopID == "" || p.PolicyID == "" {
		t.Fatal("loop and policy were not resolved")
	}
	id, err := eng.RunScheduledPromptNow(p.ID)
	if err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	ws, _ := st.Workspace(id)
	if !ws.HasRun() || ws.State != domain.StateQueued {
		t.Fatalf("loop not engaged: %+v", ws)
	}
	if ws.Goal != p.Name || ws.Short != p.Name {
		t.Fatalf("display title = %q / %q, want %q", ws.Goal, ws.Short, p.Name)
	}
	if got, _ := eng.chatLoopGoal(id, pairAgentCode); got != p.Prompt {
		t.Fatalf("loop goal = %q, want full prompt", got)
	}
	if model, flags, mode := eng.WorkspaceChatSelection(id, pairAgentCode); model != p.Model || mode != domain.ChatModeLoop || flags["effort"] != "low" {
		t.Fatalf("selection = %s, %s", model, mode)
	}
	if on := eng.ChatSimplify(id, maestroAgentCode); on == nil || *on {
		t.Fatalf("loop did not inherit Simplify off: %v", on)
	}
	for _, state := range []domain.WorkspaceState{domain.StateQueued, domain.StatePaused, domain.StateEscalated} {
		st.UpdateWorkspace(id, func(w *domain.Workspace) { w.State = state; w.Phase = "needs input" })
		if _, err := eng.RunScheduledPromptNow(p.ID); err == nil {
			t.Errorf("allowed overlap in %s", state)
		}
	}
	st.UpdateWorkspace(id, func(w *domain.Workspace) { w.State = domain.StatePaused; w.Phase = "artifact" })
	if eng.scheduledChatActive(id) {
		t.Error("artifact pause kept the schedule busy")
	}
	st.UpdateWorkspace(id, func(w *domain.Workspace) { w.State = domain.StatePaused; w.Phase = "idle" })
	if err := eng.tickScheduledPrompts(eng.now(), false); err != nil {
		t.Fatal(err)
	}
	runs := scheduleRuns(t, eng)
	for _, run := range runs {
		if run.ScheduleID == p.ID && run.Status != "finished" {
			t.Fatalf("completed loop = %+v", run)
		}
	}
}

func TestScheduledPromptCronStartup(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("scheduled", "Done") }})
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	p.Cadence, p.Cron = "cron", "@reboot"
	if _, err := eng.SaveScheduledPrompt(p); err != nil {
		t.Fatal(err)
	}
	if err := eng.tickScheduledPrompts(eng.now(), false); err != nil {
		t.Fatal(err)
	}
	if len(scheduleRuns(t, eng)) != 0 {
		t.Fatal("startup schedule ran on ordinary tick")
	}
	if err := eng.StartScheduledPrompts(); err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	if err := eng.StartScheduledPrompts(); err != nil {
		t.Fatal(err)
	}
	runs := scheduleRuns(t, eng)
	if len(runs) != 1 || runs[0].Status != "finished" {
		t.Fatalf("startup runs = %+v", runs)
	}
}

func TestScheduledPromptLoopRecovery(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	proj := addProject(t, st, initRepo(t))
	for _, state := range []domain.WorkspaceState{domain.StateQueued, domain.StateCoding} {
		ws, err := eng.CreateWorkspace(proj, "Interrupted loop", "main")
		if err != nil {
			t.Fatal(err)
		}
		st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) { w.RunStarted = true; w.State = state; w.ScheduledPromptPending = true })
		eng.Recover()
		got, _ := st.Workspace(ws.ID)
		if got.State != domain.StatePaused || got.Phase == phaseInterrupted || got.ScheduledPromptPending {
			t.Fatalf("scheduled loop eligible for automatic replay: %+v", got)
		}
	}
}

func TestScheduledPromptLoopCompletesGraph(t *testing.T) {
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	p.Mode, p.LoopID, p.PolicyID = domain.ChatModeLoop, "test.loops.converge", "test.policies.fail"
	p.Name, p.Prompt = "Short name", "Do the full task\nwith extra instructions"
	if _, err := eng.SaveScheduledPrompt(p); err != nil {
		t.Fatal(err)
	}
	id, err := eng.RunScheduledPromptNow(p.ID)
	if err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	waitFor(t, 15*time.Second, "scheduled loop completes graph", func() bool {
		w, _ := st.Workspace(id)
		return w.Phase == "idle" && eng.runningLoopByID(id) == nil
	})
	if err := eng.tickScheduledPrompts(eng.now(), false); err != nil {
		t.Fatal(err)
	}
	runs := scheduleRuns(t, eng)
	if len(runs) != 1 || runs[0].Status != "finished" {
		t.Fatalf("loop outcome = %+v", runs)
	}
	ws, _ := st.Workspace(id)
	if ws.ScheduledPromptPending || ws.LoopID != p.LoopID {
		t.Fatalf("completed workspace = %+v", ws)
	}
	if ws.Goal != p.Name {
		t.Fatalf("display title = %q, want %q", ws.Goal, p.Name)
	}
	sawPrompt := false
	for _, spec := range runner.capturedSpecs() {
		if strings.Contains(spec.Prompt, "with extra instructions") {
			sawPrompt = true
			break
		}
	}
	if !sawPrompt {
		t.Fatal("planner/coder did not receive the full multiline prompt")
	}
}

func TestCreateWorkspaceFromChat(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("spawned", "Triaged.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "triage", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	rememberDockSimplify(eng, caller.ID, pairAgentCode, false)
	rep := engineReporter{eng: eng}
	req := CreateWorkspaceRequest{Title: "ACME-12: Fix login redirect", Prompt: "Fix the redirect described in ACME-12."}
	reply := rep.CreateWorkspace(caller.ID, pairAgentCode, req)
	if !strings.HasPrefix(reply, "Created ") {
		t.Fatalf("create_workspace reply = %q", reply)
	}
	eng.scheduleWG.Wait()
	var spawned domain.Workspace
	for _, w := range st.Workspaces() {
		if w.Goal == req.Title {
			spawned = w
		}
	}
	if spawned.ID == "" || spawned.ID == caller.ID || spawned.ProjectID != proj || spawned.WorktreePath == "" || spawned.ScheduledPromptPending {
		t.Fatalf("spawned workspace = %+v, want fresh allocated worktree in caller's project", spawned)
	}
	if spawned.ScheduleID != "" {
		t.Errorf("create_workspace stamped ScheduleID = %q", spawned.ScheduleID)
	}
	spec, ok := runner.specWithRole(domain.RoleCoder)
	if !ok || spec.Worktree != spawned.WorktreePath || spec.Prompt != req.Prompt {
		t.Fatalf("spawned turn = %+v, want prompt in the new worktree", spec)
	}
	if on := eng.ChatSimplify(spawned.ID, pairAgentCode); on == nil || *on {
		t.Errorf("spawned chat Simplify = %v, want inherited off", on)
	}
	if again := rep.CreateWorkspace(caller.ID, pairAgentCode, req); !strings.Contains(again, "already exists") {
		t.Errorf("duplicate title reply = %q, want dedupe", again)
	}
	if n := len(st.Workspaces()); n != 2 {
		t.Errorf("workspaces = %d, want caller + one spawned", n)
	}
	for _, bad := range []CreateWorkspaceRequest{
		{Title: "x", Prompt: "y", ProjectID: "missing"},
		{Title: "x", Prompt: "y", BaseBranch: "missing"},
		{Title: "x", Prompt: "y", Mode: "loop", LoopID: "no.such.loop"},
	} {
		if r := rep.CreateWorkspace(caller.ID, pairAgentCode, bad); !strings.HasPrefix(r, "create_workspace:") {
			t.Errorf("CreateWorkspace(%+v) = %q, want refusal", bad, r)
		}
	}
}

func TestCreateWorkspaceFromChatChildWorkspaceModel(t *testing.T) {
	cases := []struct {
		setting    string
		wantSubstr string
		notSubstr  string
	}{
		{setting: "", wantSubstr: "composer"},
		{setting: domain.ChildWorkspaceModelParent, wantSubstr: "composer"},
		{setting: domain.ChildWorkspaceModelMode, wantSubstr: "sonnet", notSubstr: "composer"},
	}
	for _, tc := range cases {
		name := tc.setting
		if name == "" {
			name = "empty"
		}
		t.Run(name, func(t *testing.T) {
			runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("spawned", "Triaged.") }}
			eng, st, _, _ := newTestEngine(t, runner)
			proj := addProject(t, st, initRepo(t))
			caller, err := eng.CreateWorkspace(proj, "triage", "")
			if err != nil {
				t.Fatal(err)
			}
			eng.ensurePairAgent(caller.ID, pairAgentCode, "")
			eng.RememberWorkspaceChatSelection(caller.ID, pairAgentCode, "cursor/composer-2.5", nil, "agent", nil)
			set := st.Settings()
			set.ModelAgent = "claude-code/sonnet"
			set.ChildWorkspaceModel = tc.setting
			st.SetSettings(set)
			req := CreateWorkspaceRequest{Title: "ACME-12: model pick", Prompt: "Fix it."}
			reply := engineReporter{eng: eng}.CreateWorkspace(caller.ID, pairAgentCode, req)
			if !strings.HasPrefix(reply, "Created ") {
				t.Fatalf("create_workspace reply = %q", reply)
			}
			eng.scheduleWG.Wait()
			spec, ok := runner.specWithRole(domain.RoleCoder)
			if !ok {
				t.Fatal("spawned turn not captured")
			}
			if !strings.Contains(spec.Model, tc.wantSubstr) {
				t.Errorf("CreateWorkspaceFromChat(childWorkspaceModel=%q) Model = %q, want containing %q", tc.setting, spec.Model, tc.wantSubstr)
			}
			if tc.notSubstr != "" && strings.Contains(spec.Model, tc.notSubstr) {
				t.Errorf("CreateWorkspaceFromChat(childWorkspaceModel=%q) Model = %q, want not containing %q", tc.setting, spec.Model, tc.notSubstr)
			}
		})
	}
}

func TestScheduledPromptOnceRunNowStaysExhaustedOnResume(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("once", "Done") }})
	tomorrow := time.Date(2026, 9, 10, 9, 0, 0, 0, time.UTC)
	eng.nowFn = func() time.Time { return time.Date(2026, 9, 8, 8, 0, 0, 0, time.UTC) }
	p, err := eng.SaveScheduledPrompt(domain.ScheduledPrompt{
		ProjectID: addProject(t, st, initRepo(t)), Name: "Once", Prompt: "ping", BaseBranch: "main",
		Mode: "ask", Model: "claude-code/sonnet", Cadence: "once", Date: "2026-09-10", Time: "09:00", TimeZone: "UTC",
	})
	if err != nil {
		t.Fatal(err)
	}
	eng.nowFn = func() time.Time { return tomorrow.Add(-time.Hour) }
	if _, err := eng.RunScheduledPromptNow(p.ID); err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	if err := eng.PauseScheduledPrompt(p.ID, true); err != nil {
		t.Fatal(err)
	}
	if err := eng.PauseScheduledPrompt(p.ID, false); err != nil {
		t.Fatal(err)
	}
	saved, err := eng.scheduledPrompt(p.ID)
	if err != nil {
		t.Fatal(err)
	}
	if saved.NextRunAt != 0 {
		t.Fatalf("resumed one-time NextRunAt = %d, want 0", saved.NextRunAt)
	}
}

func TestCreateWorkspaceFromChatClearsPendingWithoutRunRow(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("spawned", "Triaged.") }})
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "triage", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	installFixturePack(t)
	rep := engineReporter{eng: eng}
	req := CreateWorkspaceRequest{
		Title: "ACME-12: Fix login", Prompt: "Fix the redirect.\nInclude tests.",
		Mode: domain.ChatModeLoop, LoopID: "test.loops.converge", PolicyID: "test.policies.fail",
	}
	reply := rep.CreateWorkspace(caller.ID, pairAgentCode, req)
	if !strings.HasPrefix(reply, "Created ") {
		t.Fatalf("create_workspace reply = %q", reply)
	}
	eng.scheduleWG.Wait()
	var spawned domain.Workspace
	for _, w := range st.Workspaces() {
		if w.Goal == req.Title {
			spawned = w
		}
	}
	if spawned.ID == "" || !spawned.ScheduledPromptPending {
		t.Fatalf("queued create_workspace = %+v, want pending true", spawned)
	}
	if runs := scheduleRuns(t, eng); len(runs) != 0 {
		t.Fatalf("create_workspace wrote schedule runs: %+v", runs)
	}
	if got, _ := eng.chatLoopGoal(spawned.ID, pairAgentCode); got != req.Prompt {
		t.Fatalf("loop goal = %q, want full prompt", got)
	}
	st.UpdateWorkspace(spawned.ID, func(w *domain.Workspace) { w.State = domain.StatePaused; w.Phase = "idle" })
	if err := eng.tickScheduledPrompts(eng.now(), false); err != nil {
		t.Fatal(err)
	}
	got, _ := st.Workspace(spawned.ID)
	if got.ScheduledPromptPending {
		t.Fatal("tick left ScheduledPromptPending set with no schedule-run row")
	}
}

func TestStampScheduledWorkspaceOrigins(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	proj := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(proj, "legacy run", "main")
	if err != nil {
		t.Fatal(err)
	}
	p := saveTestSchedule(t, eng, proj)
	r := domain.ScheduledPromptRun{ID: "run", ScheduleID: p.ID, DueAt: 100, Status: "finished"}
	if _, err := st.ClaimScheduledPrompt(p, r); err != nil {
		t.Fatal(err)
	}
	if err := st.LinkScheduledPromptRun(r.ID, ws.ID); err != nil {
		t.Fatal(err)
	}
	eng.stampScheduledWorkspaceOrigins()
	got, ok := st.Workspace(ws.ID)
	if !ok || got.ScheduleID != p.ID {
		t.Fatalf("ScheduleID = %q, want %s", got.ScheduleID, p.ID)
	}
}

func TestScheduledWorkspaceKeepPrunesOldest(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("scheduled-session", "Tests checked.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	set := st.Settings()
	set.ScheduledWorkspaceKeep = 1
	st.SetSettings(set)
	due := time.UnixMilli(p.NextRunAt)
	eng.scheduleMu.Lock()
	if err := eng.tickScheduledPrompts(due, false); err != nil {
		t.Fatal(err)
	}
	eng.scheduleMu.Unlock()
	eng.scheduleWG.Wait()
	first := scheduleRuns(t, eng)[0].WorkspaceID
	if _, err := eng.RunScheduledPromptNow(p.ID); err != nil {
		t.Fatal(err)
	}
	eng.scheduleWG.Wait()
	runs := scheduleRuns(t, eng)
	if len(runs) != 2 {
		t.Fatalf("runs = %d, want 2", len(runs))
	}
	old, ok := st.Workspace(first)
	if !ok || !old.Archived {
		t.Fatalf("oldest scheduled workspace = %+v, want archived", old)
	}
	newer, ok := st.Workspace(runs[1].WorkspaceID)
	if !ok || newer.Archived || newer.ScheduleID != p.ID {
		t.Fatalf("kept workspace = %+v, want live with ScheduleID", newer)
	}
}

func scheduledKeepPair(t *testing.T, keep int) (*Engine, *store.Store, string, domain.Workspace, domain.Workspace) {
	t.Helper()
	eng, st, _, _ := newTestEngine(t, nil)
	p := saveTestSchedule(t, eng, addProject(t, st, initRepo(t)))
	set := st.Settings()
	set.ScheduledWorkspaceKeep = keep
	st.SetSettings(set)
	older, err := eng.CreateWorkspace(p.ProjectID, "older", "main")
	if err != nil {
		t.Fatal(err)
	}
	newer, err := eng.CreateWorkspace(p.ProjectID, "newer", "main")
	if err != nil {
		t.Fatal(err)
	}
	st.UpdateWorkspace(older.ID, func(w *domain.Workspace) { w.ScheduleID = p.ID })
	st.UpdateWorkspace(newer.ID, func(w *domain.Workspace) { w.ScheduleID = p.ID })
	older, _ = st.Workspace(older.ID)
	newer, _ = st.Workspace(newer.ID)
	return eng, st, p.ID, older, newer
}

func TestScheduledKeepSkipsParkedSoloChat(t *testing.T) {
	eng, st, id, older, _ := scheduledKeepPair(t, 1)
	_ = eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: older.ID, AgentCode: "PA2", Question: "color?",
	}, nil)
	eng.PruneScheduledWorkspaces(id)
	got, ok := st.Workspace(older.ID)
	if !ok || got.Archived {
		t.Errorf("PruneScheduledWorkspaces(%s) Archived parked %s = %+v, want live", id, older.ID, got)
	}
	if n := len(eng.PendingAskUsers()); n != 1 {
		t.Errorf("PendingAskUsers() = %d, want 1", n)
	}
}

func TestScheduledKeepSkipsSoloOpenPR(t *testing.T) {
	eng, st, id, older, _ := scheduledKeepPair(t, 1)
	if _, err := st.AddPR(domain.PR{
		ID: "gh-42", WorkspaceID: older.ID, Number: 42, Source: domain.SourceGitHub, Draft: true, State: "In review",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if older.State == domain.StatePROpen {
		t.Fatalf("older.State = %q, want idle solo", older.State)
	}
	eng.PruneScheduledWorkspaces(id)
	got, ok := st.Workspace(older.ID)
	if !ok || got.Archived {
		t.Errorf("PruneScheduledWorkspaces(%s) Archived open-PR %s = %+v, want live", id, older.ID, got)
	}
}

func TestScheduledKeepSkipsDirtyWorktree(t *testing.T) {
	eng, st, id, older, _ := scheduledKeepPair(t, 1)
	if older.WorktreePath == "" {
		t.Fatal("CreateWorkspace WorktreePath = empty, want allocated")
	}
	path := filepath.Join(older.WorktreePath, "scratch.txt")
	if err := os.WriteFile(path, []byte("keep me\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	eng.PruneScheduledWorkspaces(id)
	got, ok := st.Workspace(older.ID)
	if !ok || got.Archived {
		t.Errorf("PruneScheduledWorkspaces(%s) Archived dirty %s = %+v, want live", id, older.ID, got)
	}
	if _, err := os.Stat(path); err != nil {
		t.Errorf("os.Stat(%s) = %v, want uncommitted file kept", path, err)
	}
}

func TestScheduledKeepPrunesArtifactCompletion(t *testing.T) {
	eng, st, id, older, newer := scheduledKeepPair(t, 1)
	st.UpdateWorkspace(older.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.State = domain.StatePaused
		w.Phase = "artifact"
	})
	eng.PruneScheduledWorkspaces(id)
	got, ok := st.Workspace(older.ID)
	if !ok || !got.Archived {
		t.Errorf("PruneScheduledWorkspaces(%s) Archived artifact %s = %+v, want archived", id, older.ID, got)
	}
	kept, ok := st.Workspace(newer.ID)
	if !ok || kept.Archived {
		t.Errorf("PruneScheduledWorkspaces(%s) kept %s = %+v, want live", id, newer.ID, kept)
	}
}
