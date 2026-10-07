package engine

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/store"
)

// spawnChild creates a child of caller via the tool, waits for its first turn,
// commits helper.txt in it, and returns it settled.
func spawnChild(t *testing.T, eng *Engine, st *store.Store, caller domain.Workspace, title string) domain.Workspace {
	t.Helper()
	rep := engineReporter{eng: eng}
	reply := rep.CreateWorkspace(caller.ID, "PA2", CreateWorkspaceRequest{Title: title, Prompt: "Do it."})
	if !strings.HasPrefix(reply, "Created subagent ") {
		t.Fatalf("reply = %q", reply)
	}
	eng.scheduleWG.Wait()
	var child domain.Workspace
	for _, w := range st.Workspaces() {
		if w.Goal == title {
			child = w
		}
	}
	if child.ParentID != caller.ID || !isChatChild(child) || child.WorktreePath == "" || child.FanOut == nil || child.FanOut.Chat != "PA2" {
		t.Fatalf("child = %+v, want ParentID=%s with worktree and FanOut row", child, caller.ID)
	}
	pr, _ := st.Project(caller.ProjectID)
	if err := os.WriteFile(filepath.Join(child.WorktreePath, "helper.txt"), []byte("child\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := eng.gitManager(pr.RepoPath).Commit(context.Background(), child.WorktreePath, "child work", "t", "t@x"); err != nil {
		t.Fatal(err)
	}
	if !eng.childSettledForChat(child.ID) {
		t.Fatalf("child after its first turn must count as settled: state=%q pending=%v", child.State, child.ScheduledPromptPending)
	}
	return child
}

func TestSpawnedChildPromptExecutesLocally(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Implement the feature")
	var spec AgentSpec
	for i := 0; ; i++ {
		s, ok := runner.specAt(i)
		if !ok {
			break
		}
		if s.Worktree == child.WorktreePath {
			spec = s
		}
	}
	if spec.AppendSystemPrompt == "" {
		t.Fatal("child first turn not captured")
	}
	if !spec.ChatChild {
		t.Error("child first turn should set ChatChild")
	}
	if strings.Contains(spec.AppendSystemPrompt, "beyond a very simple task") {
		t.Errorf("spawned child must not inherit parent delegation preference, got %q", spec.AppendSystemPrompt)
	}
	if !strings.Contains(spec.AppendSystemPrompt, "execute the assigned task here") {
		t.Errorf("spawned child should be told to execute locally, got %q", spec.AppendSystemPrompt)
	}
	if strings.Contains(spec.AppendSystemPrompt, mcpCreateWorkspace) {
		t.Errorf("spawned child must not be pointed at create_workspace, got %q", spec.AppendSystemPrompt)
	}
	if !strings.Contains(spec.AppendSystemPrompt, "Do not call "+mcpAskUser) {
		t.Errorf("spawned child should be told not to ask_user, got %q", spec.AppendSystemPrompt)
	}
	if !slices.Contains(spec.AllowedTools, mcpRenameBranch) {
		t.Errorf("spawned child missing %s; tools = %v", mcpRenameBranch, spec.AllowedTools)
	}
	for _, tl := range []string{mcpAskUser, mcpOpenPR, mcpCreateWorkspace, mcpMergeChild, mcpReadChild, mcpArchiveChild, mcpMessageChild} {
		if slices.Contains(spec.AllowedTools, tl) {
			t.Errorf("spawned child must not allow %s; tools = %v", tl, spec.AllowedTools)
		}
	}
}

func TestStandaloneWorkspaceKeepsFanOutTools(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("solo", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	yes := true
	reply := engineReporter{eng: eng}.CreateWorkspace(caller.ID, "PA2", CreateWorkspaceRequest{
		Title: "Independent", Prompt: "Do it.", Standalone: &yes,
	})
	if !strings.HasPrefix(reply, "Created workspace ") {
		t.Fatalf("reply = %q", reply)
	}
	eng.scheduleWG.Wait()
	var spawned domain.Workspace
	for _, w := range st.Workspaces() {
		if w.Goal == "Independent" {
			spawned = w
		}
	}
	if spawned.ID == "" || spawned.ParentID != "" || spawned.WorktreePath == "" {
		t.Fatalf("standalone = %+v, want top-level with worktree", spawned)
	}
	var spec AgentSpec
	for i := 0; ; i++ {
		s, ok := runner.specAt(i)
		if !ok {
			break
		}
		if s.Worktree == spawned.WorktreePath {
			spec = s
		}
	}
	if spec.AppendSystemPrompt == "" {
		t.Fatal("standalone first turn not captured")
	}
	if spec.ChatChild {
		t.Error("standalone first turn must not set ChatChild")
	}
	for _, tl := range []string{mcpAskUser, mcpOpenPR, mcpCreateWorkspace, mcpMergeChild, mcpReadChild, mcpArchiveChild, mcpMessageChild} {
		if !slices.Contains(spec.AllowedTools, tl) {
			t.Errorf("standalone missing %s; tools = %v", tl, spec.AllowedTools)
		}
	}
}

func TestChildReportsToParentThenMergesOnRequest(t *testing.T) {
	old := fanOutPoll
	fanOutPoll = 50 * time.Millisecond
	t.Cleanup(func() { fanOutPoll = old })
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Add helper")

	// The watcher re-engages the parent with the child's report.
	var report AgentSpec
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		for i := 0; ; i++ {
			spec, ok := runner.specAt(i)
			if !ok {
				break
			}
			if spec.Worktree == caller.WorktreePath && (spec.ChatCode == "PA2" || strings.HasSuffix(spec.AgentID, "-PA2")) && strings.Contains(spec.Prompt, child.ID) {
				report = spec
			}
		}
		if report.Prompt != "" {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if report.Prompt == "" {
		t.Fatal("creator tab PA2 never received the child report turn")
	}
	// The watcher may report before this test's commit lands, so the file
	// count is not asserted here; TestReadChildAndArchiveWithoutMerge covers it.
	for _, want := range []string{"Done.", "committed", "read_child " + child.ID, "merge_child " + child.ID, "archive_child " + child.ID} {
		if !strings.Contains(report.Prompt, want) {
			t.Errorf("notice missing %q:\n%s", want, report.Prompt)
		}
	}
	if strings.Contains(report.Prompt, "helper.txt") || len(report.Prompt) > 900 {
		t.Errorf("notice should be brief and leave the file list to read_child (%d chars):\n%s", len(report.Prompt), report.Prompt)
	}
	waitFor(t, 5*time.Second, "child outcome settled after report delivery", func() bool {
		after, _ := st.Workspace(child.ID)
		return after.FanOut != nil && after.FanOut.Outcome == childOutcomeSettled
	})

	rep := engineReporter{eng: eng}
	ctx := context.Background()
	pr, _ := st.Project(proj)
	git := eng.gitManager(pr.RepoPath)
	if err := os.WriteFile(filepath.Join(caller.WorktreePath, "wip.txt"), []byte("wip\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if msg := rep.MergeChild(caller.ID, pairAgentCode, child.ID); !strings.Contains(msg, "uncommitted") {
		t.Fatalf("dirty parent merge = %q, want refusal", msg)
	}
	if _, err := git.Commit(ctx, caller.WorktreePath, "parent wip", "t", "t@x"); err != nil {
		t.Fatal(err)
	}
	if msg := rep.MergeChild(caller.ID, pairAgentCode, child.ID); !strings.HasPrefix(msg, "Merged subagent") {
		t.Fatalf("clean parent merge = %q", msg)
	}
	if _, err := os.Stat(filepath.Join(caller.WorktreePath, "helper.txt")); err != nil {
		t.Fatalf("child work not in parent worktree: %v", err)
	}
	if len(gitpkg.StatusMap(ctx, caller.WorktreePath)) != 0 {
		t.Fatal("parent worktree dirty after merge")
	}
	if after, ok := st.Workspace(child.ID); ok && (!after.Archived || after.FanOut.Outcome != childOutcomeMerged) {
		t.Fatalf("child after merge = archived:%v outcome:%q", after.Archived, after.FanOut.Outcome)
	}
	if msg := rep.MergeChild(caller.ID, pairAgentCode, child.ID); !strings.Contains(msg, "not a live child") {
		t.Fatalf("second merge = %q", msg)
	}
	eng.teardownWG.Wait()
	if git.BaseBranchExists(ctx, child.Branch) {
		t.Fatalf("merged child branch %s must be deleted", child.Branch)
	}
}

func TestReadChildAndArchiveWithoutMerge(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Research says X.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Research")
	rep := engineReporter{eng: eng}

	report := rep.ReadChild(caller.ID, pairAgentCode, child.ID)
	for _, want := range []string{"Research says X.", "helper.txt", "```diff", "+child"} {
		if !strings.Contains(report, want) {
			t.Errorf("read_child missing %q:\n%s", want, report)
		}
	}
	if msg := rep.ReadChild(caller.ID, pairAgentCode, caller.ID); !strings.Contains(msg, "not a child") {
		t.Errorf("read_child on self = %q", msg)
	}
	if _, err := rep.ReadChildArtifact(caller.ID, pairAgentCode, "nope", "a1"); err == nil {
		t.Error("read_artifact on a non-child must fail")
	}
	if msg := rep.ArchiveChild(caller.ID, pairAgentCode, child.ID); !strings.HasPrefix(msg, "Archived subagent") || !strings.Contains(msg, "discarding 1 unmerged commit") {
		t.Fatalf("archive = %q", msg)
	}
	eng.teardownWG.Wait()
	pr, _ := st.Project(proj)
	if eng.gitManager(pr.RepoPath).BaseBranchExists(context.Background(), child.Branch) {
		t.Fatalf("archived child branch %s must be deleted regardless of the project setting", child.Branch)
	}
	after, ok := st.Workspace(child.ID)
	if !ok || !after.Archived || after.FanOut.Outcome != childOutcomeClosed || after.State != domain.StateStopped {
		t.Fatalf("archived child = %+v", after)
	}
	if _, err := os.Stat(filepath.Join(caller.WorktreePath, "helper.txt")); err == nil {
		t.Fatal("archive_child must not merge")
	}
	if !strings.Contains(rep.ReadChild(caller.ID, pairAgentCode, child.ID), "archived") {
		t.Error("read_child on an archived child should still report")
	}
}

func TestReadChildIncludesFindingsPlanAndUncommitted(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Filed 1 findings") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Review it")
	st.AddFinding(domain.Finding{
		ID: "f1", WorkspaceID: child.ID, File: "x.go", Line: 10,
		Severity: domain.SevHigh, Message: "nil deref", Status: string(domain.FindingOpen),
	})
	st.UpdateWorkspace(child.ID, func(w *domain.Workspace) {
		w.Plan = &domain.Plan{Summary: "Ship the fix", Body: "full plan body", Steps: []domain.PlanStep{{ID: "S1", Title: "patch x.go"}}}
	})
	if err := os.WriteFile(filepath.Join(child.WorktreePath, "dirty.txt"), []byte("dirty\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	child, _ = st.Workspace(child.ID)
	report := engineReporter{eng: eng}.ReadChild(caller.ID, pairAgentCode, child.ID)
	for _, want := range []string{"nil deref", "x.go:10", "Plan:", "full plan body", "patch x.go", "Uncommitted", "dirty.txt", "+dirty"} {
		if !strings.Contains(report, want) {
			t.Errorf("read_child missing %q:\n%s", want, report)
		}
	}
}

func TestWantChildDefaults(t *testing.T) {
	yes, no := true, false
	caller := domain.Workspace{ProjectID: "p", Branch: "b", WorktreePath: "/wt"}
	scheduled := caller
	scheduled.ScheduleID = "sched"
	bare := domain.Workspace{ProjectID: "p"}
	nested := caller
	nested.ParentID = "p0"
	for _, tc := range []struct {
		name   string
		caller domain.Workspace
		req    CreateWorkspaceRequest
		mode   string
		child  bool
		refuse bool
	}{
		{"agent chat defaults to child", caller, CreateWorkspaceRequest{}, domain.ChatModeAgent, true, false},
		{"scheduled caller defaults standalone", scheduled, CreateWorkspaceRequest{}, domain.ChatModeAgent, false, false},
		{"loop mode defaults standalone", caller, CreateWorkspaceRequest{}, domain.ChatModeLoop, false, false},
		{"base_branch implies standalone", caller, CreateWorkspaceRequest{BaseBranch: "main"}, domain.ChatModeAgent, false, false},
		{"explicit standalone", caller, CreateWorkspaceRequest{Standalone: &yes}, domain.ChatModeAgent, false, false},
		{"explicit child on scheduled caller", scheduled, CreateWorkspaceRequest{Standalone: &no}, domain.ChatModeAgent, true, false},
		{"other project", caller, CreateWorkspaceRequest{ProjectID: "q"}, domain.ChatModeAgent, false, true},
		{"caller without worktree", bare, CreateWorkspaceRequest{}, domain.ChatModeAgent, false, true},
		{"chat child cannot nest", nested, CreateWorkspaceRequest{}, domain.ChatModeAgent, false, true},
		{"chat child explicit child refused", nested, CreateWorkspaceRequest{Standalone: &no}, domain.ChatModeAgent, false, true},
		{"chat child standalone refused", nested, CreateWorkspaceRequest{Standalone: &yes}, domain.ChatModeAgent, false, true},
	} {
		pid := tc.req.ProjectID
		if pid == "" {
			pid = tc.caller.ProjectID
		}
		child, reason := wantChild(tc.caller, tc.req, tc.mode, pid)
		if child != tc.child || (reason != "") != tc.refuse {
			t.Errorf("%s: child=%v reason=%q, want child=%v refuse=%v", tc.name, child, reason, tc.child, tc.refuse)
		}
	}
}

func TestChatChildAutoResumesInterruptedFirstTurn(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &captureRunner{})
	dir := t.TempDir()
	agent := domain.Agent{Code: pairAgentCode, Status: "working"}
	child := domain.Workspace{WorktreePath: dir, ScheduledPromptPending: true, ParentID: "parent", FanOut: &domain.ChildFanOut{Chat: "PA"}}
	if !eng.shouldAutoResumeChat(child, agent) {
		t.Error("interrupted subagent first turn must auto-resume")
	}
	scheduled := domain.Workspace{WorktreePath: dir, ScheduledPromptPending: true, ScheduleID: "s1"}
	if eng.shouldAutoResumeChat(scheduled, agent) {
		t.Error("interrupted scheduled first turn must stay excluded")
	}
}

func TestChildNotSettledWhileAskUserPending(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Need a decision")
	if !eng.childSettledForChat(child.ID) {
		t.Fatal("child should be settled before ask_user")
	}
	eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: child.ID, AgentCode: pairAgentCode, Question: "which one?",
	}, nil)
	if eng.childSettledForChat(child.ID) {
		t.Error("parked ask_user must keep the child from looking finished to its parent")
	}
}

func TestRemovingParentRemovesItsChildren(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Orphan check")
	if _, err := eng.RemoveWorkspace(caller.ID); err != nil {
		t.Fatal(err)
	}
	eng.teardownWG.Wait()
	after, ok := st.Workspace(child.ID)
	if !ok || !after.Archived {
		t.Fatalf("child %s left as an orphan after parent removal: %+v", child.ID, after)
	}
	pr, _ := st.Project(proj)
	if eng.gitManager(pr.RepoPath).BaseBranchExists(context.Background(), child.Branch) {
		t.Fatalf("orphaned child branch %s survived parent removal", child.Branch)
	}
}

func TestSiblingChildrenFromOneTurnShareAWave(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	rep := engineReporter{eng: eng}
	create := func(title string) domain.Workspace {
		if r := rep.CreateWorkspace(caller.ID, "PA2", CreateWorkspaceRequest{Title: title, Prompt: "Do it."}); !strings.HasPrefix(r, "Created subagent ") {
			t.Fatalf("create %s = %q", title, r)
		}
		for _, w := range st.Workspaces() {
			if w.Goal == title {
				return w
			}
		}
		t.Fatalf("child %s not found", title)
		return domain.Workspace{}
	}
	// One live parent turn creates two children: same wave.
	turn := eng.registerChatTurn(caller.ID, "PA2", func() {})
	a, b := create("Engine"), create("CLI")
	eng.unregisterChatTurn(caller.ID, "PA2", turn)
	if a.FanOut.Wave == 0 || a.FanOut.Wave != b.FanOut.Wave {
		t.Fatalf("siblings waves = %d, %d; want equal and non-zero", a.FanOut.Wave, b.FanOut.Wave)
	}
	// A later turn opens a new wave.
	turn2 := eng.registerChatTurn(caller.ID, "PA2", func() {})
	c := create("Docs")
	eng.unregisterChatTurn(caller.ID, "PA2", turn2)
	if c.FanOut.Wave != a.FanOut.Wave+1 {
		t.Fatalf("next-turn child wave = %d, want %d", c.FanOut.Wave, a.FanOut.Wave+1)
	}
	eng.scheduleWG.Wait()
	var cards []domain.FeedItem
	for _, f := range st.Feed(caller.ID) {
		if f.Card == domain.FeedCardFanOut {
			cards = append(cards, f)
		}
	}
	if len(cards) != 2 {
		t.Fatalf("fan-out cards = %d, want 2 (one per wave): %+v", len(cards), cards)
	}
	var wave1 fanOutWave
	for _, f := range cards {
		var w fanOutWave
		if err := json.Unmarshal([]byte(f.Data), &w); err != nil {
			t.Fatal(err)
		}
		if w.Wave == a.FanOut.Wave {
			wave1 = w
		}
	}
	if len(wave1.Children) != 2 || wave1.Title != "Subagents" {
		t.Fatalf("wave card = %+v, want both siblings under one Subagents card", wave1)
	}
}

func TestChildAwaitingAutoResumeIsNotSettled(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Restart me")
	// Recover healed the agent to idle and cleared the pending flag; AutoResume
	// has launched the tab but its turn has not registered yet.
	eng.resumingChats.Store(chatTurnKey(child.ID, pairAgentCode), struct{}{})
	if eng.childSettledForChat(child.ID) {
		t.Fatal("a child awaiting its auto-resumed turn must not count as settled")
	}
	eng.resumingChats.Delete(chatTurnKey(child.ID, pairAgentCode))
	if !eng.childSettledForChat(child.ID) {
		t.Fatal("child must settle once the resumed turn is done")
	}
}

func TestReadChildOnRunningChildSaysRunning(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Working…") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Slow one")
	// Simulate a live turn on the child.
	turn := eng.registerChatTurn(child.ID, pairAgentCode, func() {})
	report := engineReporter{eng: eng}.ReadChild(caller.ID, pairAgentCode, child.ID)
	eng.unregisterChatTurn(child.ID, pairAgentCode, turn)
	if !strings.Contains(report, "STILL RUNNING") || strings.Contains(report, "has finished") || strings.Contains(report, "merge_child "+child.ID) {
		t.Fatalf("read_child on a running child must say running and offer no verbs:\n%s", report)
	}
}

func TestChatChildReportsCreateFailureWithoutWorktree(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	parent, err := st.AddWorkspace(domain.Workspace{ID: "parent", Goal: "p", Branch: "b", WorktreePath: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	child, err := st.AddWorkspace(domain.Workspace{
		ID: "child", ParentID: parent.ID, Goal: "failed spawn", LastError: "no terminals",
		Phase: "failed", FanOut: &domain.ChildFanOut{Chat: "PA"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if !eng.chatChildReadyToReport(child) {
		t.Fatal("failed create with no worktree must still be reportable")
	}
	report := eng.childReport(parent.ID, child, false)
	if !strings.Contains(report, "no terminals") {
		t.Fatalf("report missing LastError:\n%s", report)
	}
}

func TestRemovingChildStampsClosedOutcome(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Spike")
	if _, err := eng.RemoveWorkspace(child.ID); err != nil {
		t.Fatal(err)
	}
	after, ok := st.Workspace(child.ID)
	if !ok || !after.Archived || after.FanOut == nil || after.FanOut.Outcome != childOutcomeClosed {
		t.Fatalf("removed child = archived:%v outcome:%v", ok, after.FanOut)
	}
}

func TestDeletingParentRemovesItsChildren(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Orphan delete")
	if err := eng.DeleteWorkspace(caller.ID); err != nil {
		t.Fatal(err)
	}
	eng.teardownWG.Wait()
	after, ok := st.Workspace(child.ID)
	if !ok || !after.Archived {
		t.Fatalf("child %s left as an orphan after parent delete: %+v", child.ID, after)
	}
}

func TestChildReportPreservesParentModel(t *testing.T) {
	old := fanOutPoll
	fanOutPoll = 50 * time.Millisecond
	t.Cleanup(func() { fanOutPoll = old })
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	eng.ensurePairAgent(caller.ID, "PA2", "")
	eng.RememberWorkspaceChatSelection(caller.ID, "PA2", "cursor/composer-2.5", map[string]string{"effort": "high"}, "agent", nil)
	child := spawnChild(t, eng, st, caller, "Model check")
	var report AgentSpec
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		for i := 0; ; i++ {
			spec, ok := runner.specAt(i)
			if !ok {
				break
			}
			if spec.Worktree == caller.WorktreePath && strings.Contains(spec.Prompt, child.ID) {
				report = spec
			}
		}
		if report.Prompt != "" {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if report.Prompt == "" {
		t.Fatal("parent never received the child report turn")
	}
	if !strings.Contains(report.Model, "composer") {
		t.Errorf("report Model=%q, want persisted cursor composer", report.Model)
	}
}

func TestWorkspaceDiscardRiskIncludesChildren(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Risk check")
	risk := eng.WorkspaceDiscardRisk(caller.ID)
	if !strings.Contains(risk, "branch will be deleted") {
		t.Fatalf("parent discard risk = %q, want child branch warning", risk)
	}
	turn := eng.registerChatTurn(child.ID, pairAgentCode, func() {})
	risk = eng.WorkspaceDiscardRisk(caller.ID)
	eng.unregisterChatTurn(child.ID, pairAgentCode, turn)
	if !strings.Contains(risk, "running child") {
		t.Fatalf("parent discard risk = %q, want running child warning", risk)
	}
}

func TestReportKeepsOutcomeSetDuringParentTurn(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Closed mid-turn")
	// The parent's report turn archived the child before reportChild returned.
	st.UpdateWorkspace(child.ID, func(w *domain.Workspace) { w.FanOut.Outcome = childOutcomeClosed })
	eng.markChildReported(caller.ID, child.ID)
	if after, _ := st.Workspace(child.ID); after.FanOut.Outcome != childOutcomeClosed {
		t.Fatalf("outcome = %q, want closed preserved", after.FanOut.Outcome)
	}
	st.UpdateWorkspace(child.ID, func(w *domain.Workspace) { w.FanOut.Outcome = childOutcomeReportPending })
	eng.markChildReported(caller.ID, child.ID)
	if after, _ := st.Workspace(child.ID); after.FanOut.Outcome != childOutcomeSettled {
		t.Fatalf("outcome = %q, want pending advanced to settled", after.FanOut.Outcome)
	}
}

func TestRearmDoesNotResendDeliveredReport(t *testing.T) {
	old := fanOutPoll
	fanOutPoll = 50 * time.Millisecond
	t.Cleanup(func() { fanOutPoll = old })
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Delivered once")
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if after, _ := st.Workspace(child.ID); after.FanOut != nil && after.FanOut.Outcome == childOutcomeSettled {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if !eng.noticeDelivered(caller.ID, "PA2", child) {
		t.Fatal("delivered notice not found on the creator tab")
	}
	// Simulate a crash between delivery and the settled stamp, then a restart.
	st.UpdateWorkspace(child.ID, func(w *domain.Workspace) { w.FanOut.Outcome = childOutcomeReportPending })
	runs := len(runner.specs)
	eng.rearmChatChildren()
	time.Sleep(300 * time.Millisecond)
	if after, _ := st.Workspace(child.ID); after.FanOut.Outcome != childOutcomeSettled {
		t.Fatalf("outcome after rearm = %q, want settled without a resend", after.FanOut.Outcome)
	}
	if n := len(runner.specs); n != runs {
		t.Fatalf("rearm resent a delivered report: runs %d -> %d", runs, n)
	}
}

// TestReportedChildReportsAgainAfterNewTurn: a child the human retries after
// it already reported (a usage limit) must wake the parent again.
func TestReportedChildReportsAgainAfterNewTurn(t *testing.T) {
	old := fanOutPoll
	fanOutPoll = 50 * time.Millisecond
	t.Cleanup(func() { fanOutPoll = old })
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Hit the limit")
	waitFor(t, 10*time.Second, "first report delivered", func() bool {
		after, _ := st.Workspace(child.ID)
		return after.FanOut != nil && after.FanOut.Outcome == childOutcomeSettled && eng.noticeCount(caller.ID, "PA2", child) == 1
	})
	eng.SendWorkspaceMessage(child.ID, pairAgentCode, "Try again.", "", nil, nil, "", "")
	waitFor(t, 10*time.Second, "second report delivered", func() bool {
		after, _ := st.Workspace(child.ID)
		return after.FanOut.Outcome == childOutcomeSettled && eng.noticeCount(caller.ID, "PA2", child) == 2
	})
}

type messageFromReportRunner struct {
	captureRunner
	onRun func(AgentSpec)
}

func (r *messageFromReportRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.onRun(spec)
	return r.captureRunner.Run(ctx, spec, emit)
}

// TestMessageFinishedChildStartsNewTurn: message_child on a finished child,
// from the parent's report turn or a later turn, resumes it and the parent
// gets another report each time.
func TestMessageFinishedChildStartsNewTurn(t *testing.T) {
	old := fanOutPoll
	fanOutPoll = 50 * time.Millisecond
	t.Cleanup(func() { fanOutPoll = old })
	var eng *Engine
	var parentID string
	var parentRuns atomic.Int32
	runner := &messageFromReportRunner{captureRunner: captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}}
	runner.onRun = func(spec AgentSpec) {
		if spec.WorkspaceID != parentID || parentRuns.Add(1) != 1 {
			return
		}
		for _, w := range eng.Store.Workspaces() {
			if w.ParentID == parentID {
				if got := (engineReporter{eng: eng}).MessageChild(parentID, "PA2", w.ID, "Also add docs."); !strings.Contains(got, "new turn") {
					t.Errorf("MessageChild(report-pending) = %q, want new turn", got)
				}
			}
		}
	}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	parentID = caller.ID
	eng.ensurePairAgent(caller.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, caller, "Resume me")
	waitFor(t, 10*time.Second, "report after the report-turn message", func() bool {
		after, _ := st.Workspace(child.ID)
		return after.FanOut.Outcome == childOutcomeSettled && eng.noticeCount(caller.ID, "PA2", child) == 2
	})
	if got := (engineReporter{eng: eng}).MessageChild(caller.ID, "PA2", child.ID, "One more thing."); !strings.Contains(got, "new turn") {
		t.Fatalf("MessageChild(settled) = %q, want new turn", got)
	}
	waitFor(t, 10*time.Second, "report after the settled message", func() bool {
		after, _ := st.Workspace(child.ID)
		return after.FanOut.Outcome == childOutcomeSettled && eng.noticeCount(caller.ID, "PA2", child) == 3
	})
	eng.setChatChildOutcome(caller.ID, child.ID, "conflict: README.md", "")
	if got := (engineReporter{eng: eng}).MessageChild(caller.ID, "PA2", child.ID, "Rebase and fix the conflicts."); !strings.Contains(got, "new turn") {
		t.Fatalf("MessageChild(conflict) = %q, want new turn", got)
	}
	waitFor(t, 10*time.Second, "report after the conflict message", func() bool {
		after, _ := st.Workspace(child.ID)
		return after.FanOut.Outcome == childOutcomeSettled && eng.noticeCount(caller.ID, "PA2", child) == 4
	})
}

// TestStopDuringQueuedReportIsHonored: Stop while the notice waits behind a
// live parent turn must not make the watcher retry it as a fresh agent turn.
func TestStopDuringQueuedReportIsHonored(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	caller, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	child := domain.Workspace{ID: "proj.kid", ProjectID: proj, ParentID: caller.ID, Goal: "Kid", Branch: "kid",
		FanOut: &domain.ChildFanOut{Chat: "PA", Wave: 1}}
	if _, err := st.AddWorkspace(child); err != nil {
		t.Fatal(err)
	}
	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(caller.ID, "PA", "do the thing", "claude-code/sonnet", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not start")
	}
	reported := make(chan bool, 1)
	go func() { reported <- eng.reportChild(caller.ID, child) }()
	time.Sleep(150 * time.Millisecond)
	if !eng.CancelChat(caller.ID, "PA") {
		t.Fatal("CancelChat found no live turn")
	}
	close(release)
	select {
	case ok := <-reported:
		if !ok {
			t.Fatal("reportChild must give up (not retry) after a human Stop")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("reportChild did not return")
	}
	<-firstDone
	if n := runner.runCount(); n != 1 {
		t.Fatalf("Run called %d times, want 1 (stopped notice must not start a turn)", n)
	}
	after, _ := st.Workspace(child.ID)
	if after.FanOut.Outcome != childOutcomeSettled {
		t.Fatalf("outcome = %q, want settled so the watcher stops", after.FanOut.Outcome)
	}
	if !strings.Contains(after.FanOut.Note, "cancelled by Stop") || !strings.Contains(after.FanOut.Note, "read_child "+child.ID) {
		t.Fatalf("child row note = %q, want the Stop explanation the card renders", after.FanOut.Note)
	}
	var card domain.FeedItem
	for _, f := range st.Feed(caller.ID) {
		if f.Card == domain.FeedCardFanOut {
			card = f
		}
	}
	if !strings.Contains(card.Data, "cancelled by Stop") {
		t.Fatalf("fan-out card data lacks the note: %s", card.Data)
	}
}

func TestMessageChildFromChatRefuses(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte { return maestroFixture("child", "Done.") }}
	eng, st, _, _ := newTestEngine(t, runner)
	proj := addProject(t, st, initRepo(t))
	parent, err := eng.CreateWorkspace(proj, "parent", "")
	if err != nil {
		t.Fatal(err)
	}
	other, err := eng.CreateWorkspace(proj, "other", "")
	if err != nil {
		t.Fatal(err)
	}
	eng.ensurePairAgent(parent.ID, pairAgentCode, "")
	child := spawnChild(t, eng, st, parent, "Settled child")
	rep := engineReporter{eng: eng}

	if got := rep.MessageChild(parent.ID, pairAgentCode, other.ID, "steer"); !strings.Contains(got, "not a child") {
		t.Errorf("MessageChild(unrelated) = %q, want not a child", got)
	}
	if got := rep.MessageChild("", pairAgentCode, other.ID, "steer"); !strings.Contains(got, "not a child") {
		t.Errorf("MessageChild(empty parent, root) = %q, want not a child", got)
	}
	if got := rep.MessageChild(other.ID, pairAgentCode, child.ID, "steer"); !strings.Contains(got, "not a child") {
		t.Errorf("MessageChild(wrong parent) = %q, want not a child", got)
	}
	if msg := rep.ArchiveChild(parent.ID, pairAgentCode, child.ID); !strings.HasPrefix(msg, "Archived") {
		t.Fatalf("ArchiveChild = %q", msg)
	}
	if got := rep.MessageChild(parent.ID, pairAgentCode, child.ID, "steer"); !strings.Contains(got, "not a child") {
		t.Errorf("MessageChild(archived) = %q, want not a child", got)
	}
}

func TestMessageChildFromChatInjectsLiveTurn(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	parent, err := eng.CreateWorkspace(projID, "msg-parent", "")
	if err != nil {
		t.Fatal(err)
	}
	child, err := eng.CreateWorkspace(projID, "msg-child", "")
	if err != nil {
		t.Fatal(err)
	}
	st.UpdateWorkspace(child.ID, func(w *domain.Workspace) { w.ParentID = parent.ID })

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(child.ID, "PA", "do the thing", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("child turn did not start")
	}

	steer := "please stop and fix tests"
	got := engineReporter{eng: eng}.MessageChild(parent.ID, pairAgentCode, child.ID, steer)
	if !strings.Contains(got, "mid-turn") {
		t.Errorf("MessageChild = %q, want mid-turn ack", got)
	}

	close(release)
	select {
	case reply := <-firstDone:
		if reply == chatInjectedSentinel {
			t.Errorf("child SendWorkspaceMessage = %q, want the original turn reply", reply)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("child turn did not finish after release")
	}

	if n := runner.runCount(); n != 1 {
		t.Errorf("Run called %d times, want 1 (message_child must inject, not start a second Run)", n)
	}
	injected := runner.allInjected()
	if len(injected) != 1 {
		t.Fatalf("injected = %v, want one payload", injected)
	}
	p, display := decodeInjectPlain(injected[0])
	if display == "" {
		display = p
	}
	wantDisplay := "From parent " + parent.ID + ": " + steer
	if display != wantDisplay {
		t.Errorf("inject display = %q, want %q (prompt=%q)", display, wantDisplay, p)
	}
	if !strings.Contains(p, parentSteerPrefix) || !strings.Contains(p, steer) {
		t.Errorf("inject prompt = %q, want parent-steer prefix + message", p)
	}
}

func TestMessageChildFromChatSteersRunningLoop(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	parent, err := st.AddWorkspace(domain.Workspace{ID: "msg-loop-parent", Goal: "parent"})
	if err != nil {
		t.Fatal(err)
	}
	child, err := st.AddWorkspace(domain.Workspace{
		ID: "msg-loop-child", ParentID: parent.ID, Goal: "fan-out", ParentStepIDs: []string{"S1"},
	})
	if err != nil {
		t.Fatal(err)
	}
	lp := &Loop{ID: child.ID}
	eng.loops.Store(child.ID, &runningLoop{loop: lp})
	t.Cleanup(func() { eng.loops.Delete(child.ID) })

	steer := "skip the flaky test"
	got := engineReporter{eng: eng}.MessageChild(parent.ID, "MA", child.ID, steer)
	if !strings.Contains(got, "loop") {
		t.Errorf("MessageChild = %q, want running-loop ack", got)
	}
	if lp.pendingSteer != steer {
		t.Errorf("pendingSteer = %q, want %q", lp.pendingSteer, steer)
	}
}
