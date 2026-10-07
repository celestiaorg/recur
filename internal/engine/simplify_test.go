package engine

import (
	"strings"
	"testing"

	"recur/internal/domain"
	"recur/internal/mcp"
)

func boolPtr(v bool) *bool { return &v }

// rememberDockSimplify persists the composer chip on the same session as
// mode/model/flags (one RememberWorkspaceChatSelection write).
func rememberDockSimplify(eng *Engine, workspaceID, code string, on bool) {
	eng.RememberWorkspaceChatSelection(workspaceID, code, "sonnet", nil, domain.ChatModeAgent, boolPtr(on))
}

func TestSimplifyOffSkipsInject(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-off", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	set := st.Settings()
	set.Simplify = boolPtr(false)
	st.SetSettings(set)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.SendWorkspaceMessage(ws.ID, "", "edit the file", "", nil, nil, domain.ChatModeAgent, "")
	s, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no coding turn")
	}
	if strings.Contains(s.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("simplifyChatAppend(agent, settings off) AppendSystemPrompt = %q, want no SIMPLIFY MODE", s.AppendSystemPrompt)
	}
}

func TestSimplifyPlanInjects(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-plan", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.SendWorkspaceMessage(ws.ID, "", "plan a cache", "", nil, nil, domain.ChatModePlan, "")
	s, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no plan turn")
	}
	if !strings.Contains(s.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("simplifyChatAppend(plan, default on) AppendSystemPrompt = %q, want SIMPLIFY MODE", s.AppendSystemPrompt)
	}
}

func TestSimplifySessionOffBeatsSettingsOn(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-sess-off", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	rememberDockSimplify(eng, ws.ID, pairAgentCode, false)
	eng.SendWorkspaceMessage(ws.ID, "", "edit the file", "", nil, nil, domain.ChatModeAgent, "")
	s, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no coding turn")
	}
	if strings.Contains(s.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("simplifyChatAppend(session off, settings on) AppendSystemPrompt = %q, want no SIMPLIFY MODE", s.AppendSystemPrompt)
	}
}

func TestSimplifySessionOnBeatsSettingsOff(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-sess-on", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	set := st.Settings()
	set.Simplify = boolPtr(false)
	st.SetSettings(set)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	rememberDockSimplify(eng, ws.ID, pairAgentCode, true)
	eng.SendWorkspaceMessage(ws.ID, "", "edit the file", "", nil, nil, domain.ChatModeAgent, "")
	s, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no coding turn")
	}
	if !strings.Contains(s.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("simplifyChatAppend(session on, settings off) AppendSystemPrompt = %q, want SIMPLIFY MODE", s.AppendSystemPrompt)
	}
}

func TestSimplifyTabsIndependent(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-tabs", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	rememberDockSimplify(eng, ws.ID, pairAgentCode, false)
	tab2 := eng.CreateWorkspaceChat(ws.ID)
	if tab2.Code == "" {
		t.Fatal("CreateWorkspaceChat returned empty tab")
	}
	eng.SendWorkspaceMessage(ws.ID, pairAgentCode, "edit a", "", nil, nil, domain.ChatModeAgent, "")
	eng.SendWorkspaceMessage(ws.ID, tab2.Code, "edit b", "", nil, nil, domain.ChatModeAgent, "")
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) < 2 {
		t.Fatalf("specsWithRole(coder) len = %d, want >= 2", len(specs))
	}
	if strings.Contains(specs[0].AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("PA session-off AppendSystemPrompt = %q, want no SIMPLIFY MODE", specs[0].AppendSystemPrompt)
	}
	if !strings.Contains(specs[1].AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("PA2 settings-default AppendSystemPrompt = %q, want SIMPLIFY MODE", specs[1].AppendSystemPrompt)
	}
}

func TestSimplifySettingsChangeDoesNotFlipStartedChat(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-sticky", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.SendWorkspaceMessage(ws.ID, "", "first", "", nil, nil, domain.ChatModeAgent, "")
	set := st.Settings()
	set.Simplify = boolPtr(false)
	st.SetSettings(set)
	eng.SendWorkspaceMessage(ws.ID, "", "second", "", nil, nil, domain.ChatModeAgent, "")
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) < 2 {
		t.Fatalf("specsWithRole(coder) len = %d, want >= 2", len(specs))
	}
	if !strings.Contains(specs[1].AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("simplifyChatAppend(stamped on, settings now off) AppendSystemPrompt = %q, want SIMPLIFY MODE", specs[1].AppendSystemPrompt)
	}
}

func TestSimplifyCoderFollowsMaestroSession(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-ma", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	if got := eng.simplifyCoderAppend(ws.ID); !strings.Contains(got, "SIMPLIFY MODE") {
		t.Errorf("simplifyCoderAppend(unset MA, settings on) = %q, want SIMPLIFY MODE", got)
	}
	eng.stampMaestroSimplifyIfUnset(ws.ID)
	eng.RememberWorkspaceChatSimplify(ws.ID, maestroAgentCode, false)
	if got := eng.simplifyCoderAppend(ws.ID); got != "" {
		t.Errorf("simplifyCoderAppend(MA off) = %q, want empty", got)
	}
	eng.RememberWorkspaceChatSimplify(ws.ID, maestroAgentCode, true)
	if got := eng.simplifyCoderAppend(ws.ID); !strings.Contains(got, "SIMPLIFY MODE") {
		t.Errorf("simplifyCoderAppend(MA on) = %q, want SIMPLIFY MODE", got)
	}
}

func TestSimplifyLoopInheritsPairChatOff(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-inherit", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	rememberDockSimplify(eng, ws.ID, pairAgentCode, false)
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	if queued, err := eng.StartRunOnWorkspace(ws.ID, "ship it"); err != nil {
		t.Fatalf("StartRunOnWorkspace() err = %v, want nil", err)
	} else if !queued {
		t.Fatal("StartRunOnWorkspace() queued = false, want true (slots held)")
	}
	if got := eng.simplifyCoderAppend(ws.ID); got != "" {
		t.Errorf("simplifyCoderAppend(loop inherited PA off) = %q, want empty", got)
	}
	if s := eng.ChatSimplify(ws.ID, maestroAgentCode); s == nil || *s {
		t.Errorf("ChatSimplify(MA) = %v, want false", s)
	}
}

func TestBeginStartWorkspaceStampsSimplifyChip(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-begin", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	ws, err := eng.BeginStartWorkspace(projID, "ship it", "", domain.AutonomyBalanced, boolPtr(false))
	if err != nil {
		t.Fatal(err)
	}
	if s := eng.ChatSimplify(ws.ID, maestroAgentCode); s == nil || *s {
		t.Errorf("ChatSimplify(MA) = %v, want false (create chip, not settings default)", s)
	}
}

func TestLoopEngageStampsMaestroBeforeStart(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-engage", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	rememberDockSimplify(eng, ws.ID, pairAgentCode, false)
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	if tabs := eng.ListWorkspaceChats(ws.ID); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("tabs before engage = %+v, want [PA]", tabs)
	}
	reply := eng.SendWorkspaceMessage(ws.ID, pairAgentCode, "ship it", "", nil, nil, domain.ChatModeLoop, "")
	if strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("engage reply = %q", reply)
	}
	got, _ := st.Workspace(ws.ID)
	if !got.HasRun() || got.State != domain.StateQueued {
		t.Fatalf("run after engage: hasRun=%v state=%q, want queued", got.HasRun(), got.State)
	}
	if s := eng.ChatSimplify(ws.ID, maestroAgentCode); s == nil || *s {
		t.Errorf("ChatSimplify(MA after engage) = %v, want false", s)
	}
}

func TestRememberPairChatSelectionKeepsSimplify(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-dock", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	rememberDockSimplify(eng, ws.ID, pairAgentCode, false)
	eng.RememberWorkspaceChatSelection(ws.ID, pairAgentCode, "sonnet", map[string]string{"effort": "high"}, "review", nil)
	if s := eng.ChatSimplify(ws.ID, pairAgentCode); s == nil || *s {
		t.Errorf("ChatSimplify after dock write = %v, want false", s)
	}
	_, _, mode := eng.WorkspaceChatSelection(ws.ID, pairAgentCode)
	if mode != domain.ChatModeReview {
		t.Errorf("WorkspaceChatSelection mode = %q, want review", mode)
	}
}

func TestInheritSimplifyFollowsPlanAuthorNotNewestTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-author", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	eng.ensurePairAgent(ws.ID, "PA2", "")
	rememberDockSimplify(eng, ws.ID, pairAgentCode, false)
	rememberDockSimplify(eng, ws.ID, "PA2", true)
	_ = engineReporter{eng: eng}.SubmitPlan(ws.ID, pairAgentCode, mcp.PlanSubmission{
		Summary: "ship it",
		Steps:   []string{"one"},
	})
	got, _ := st.Workspace(ws.ID)
	if got.Plan == nil || got.Plan.ChatCode != pairAgentCode {
		t.Fatalf("Plan.ChatCode = %v, want %s", got.Plan, pairAgentCode)
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
	if queued, err := eng.StartRunOnWorkspace(ws.ID, "ship it"); err != nil {
		t.Fatalf("StartRunOnWorkspace() err = %v, want nil", err)
	} else if !queued {
		t.Fatal("StartRunOnWorkspace() queued = false, want true (slots held)")
	}
	if s := eng.ChatSimplify(ws.ID, maestroAgentCode); s == nil || *s {
		t.Errorf("ChatSimplify(MA) = %v, want false (plan author PA, not newest PA2)", s)
	}
}

func TestStampMaestroSimplifyIfUnsetDoesNotOverwrite(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-stamp", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.stampMaestroSimplifyIfUnset(ws.ID)
	eng.RememberWorkspaceChatSimplify(ws.ID, maestroAgentCode, false)
	eng.stampMaestroSimplifyIfUnset(ws.ID)
	if s := eng.ChatSimplify(ws.ID, maestroAgentCode); s == nil || *s {
		t.Errorf("ChatSimplify(MA) = %v, want false", s)
	}
}

func TestStampMaestroSimplifyIfUnsetFreezesWhenUnset(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-stamp-unset", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	if s := eng.ChatSimplify(ws.ID, maestroAgentCode); s != nil {
		t.Fatalf("ChatSimplify(MA) = %v, want unset", s)
	}
	eng.stampMaestroSimplifyIfUnset(ws.ID)
	if s := eng.ChatSimplify(ws.ID, maestroAgentCode); s == nil {
		t.Fatal("stamp should freeze MA simplify")
	}
}

func TestRememberWorkspaceChatSimplifyDoesNotCreatePairSession(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-nocreate", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	eng.RememberWorkspaceChatSimplify(ws.ID, pairAgentCode, false)
	if _, ok := st.SessionForLoopAgent(ws.ID, pairAgentCode); ok {
		t.Fatal("RememberWorkspaceChatSimplify minted a PA session; dock selection should create it")
	}
}

func TestRememberWorkspaceChatSimplifyDoesNotCreateMaestroSession(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-nocreate-ma", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.RememberWorkspaceChatSimplify(ws.ID, maestroAgentCode, false)
	if _, ok := st.SessionForLoopAgent(ws.ID, maestroAgentCode); ok {
		t.Fatal("RememberWorkspaceChatSimplify minted an MA session; stamp/send should create it")
	}
}

func TestRememberWorkspaceChatSimplifyDoesNotResurrectClosedTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("sm-close", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatal(err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	tab2 := eng.CreateWorkspaceChat(ws.ID)
	eng.RememberWorkspaceChatSimplify(ws.ID, tab2.Code, false)
	_ = eng.CloseWorkspaceChat(ws.ID, tab2.Code)
	if _, ok := st.Agent(ws.ID, tab2.Code); ok {
		t.Fatalf("agent %s still present after close", tab2.Code)
	}
	eng.RememberWorkspaceChatSimplify(ws.ID, tab2.Code, true)
	if _, ok := st.Agent(ws.ID, tab2.Code); ok {
		t.Errorf("RememberWorkspaceChatSimplify after CloseWorkspaceChat resurrected agent %s", tab2.Code)
	}
	// Simulate the check-then-act window: persist after Close has already
	// deleted the agent (as if the public existence check had already passed).
	eng.rememberChatSimplify(ws.ID, tab2.Code, true)
	if _, ok := st.Agent(ws.ID, tab2.Code); ok {
		t.Errorf("rememberChatSimplify after CloseWorkspaceChat resurrected agent %s", tab2.Code)
	}

	eng.RememberWorkspaceChatSimplify("no-such-ws", "PA", true)
	if _, ok := st.Agent("no-such-ws", "PA"); ok {
		t.Error("RememberWorkspaceChatSimplify on missing workspace created an agent")
	}

	eng.stampMaestroSimplifyIfUnset(ws.ID)
	eng.RememberWorkspaceChatSimplify(ws.ID, maestroAgentCode, false)
	if s := eng.ChatSimplify(ws.ID, maestroAgentCode); s == nil || *s {
		t.Errorf("RememberWorkspaceChatSimplify(MA) ChatSimplify = %v, want false", s)
	}
}
