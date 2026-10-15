package engine

import (
	"encoding/base64"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

// TestLoopModeEngagesRunOnWorkspace: Loop mode on a chat tab engages the
// autopilot on this workspace (one chat = one loop). Agent-mode sends on the
// loop's own tab refuse while the run is active; sibling tabs keep working.
// A second Loop tab stays here and does not inherit the live run.
func TestLoopModeEngagesRunOnWorkspace(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("engage-sess", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.HasRun() {
		t.Fatal("fresh hand-driven workspace must not report a run")
	}

	// Fill every loop slot so the engaged run parks QUEUED — deterministic: no
	// loop goroutine races the assertions below, and queued counts as active
	// for the run-lock (it may start at any moment).
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()

	// The dock always lists tabs before send, which seeds the PA agent row.
	if tabs := eng.ListWorkspaceChats(ws.ID); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("tabs before engage = %+v, want [PA]", tabs)
	}

	reply := eng.SendWorkspaceMessage(ws.ID, "PA", "ship dark mode", "", nil, nil, "loop", "")
	if strings.Contains(reply, "Follow along") || strings.Contains(reply, "already has a loop") ||
		strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("engage reply = %q", reply)
	}

	got, _ := st.Workspace(ws.ID)
	if !got.HasRun() || !got.RunStarted {
		t.Fatalf("run not engaged: %+v", got)
	}
	if got.State != domain.StateQueued {
		t.Fatalf("state = %q, want queued", got.State)
	}
	if got.Goal != "scratch" {
		t.Fatalf("goal = %q, want the existing workspace name", got.Goal)
	}

	// The tab you sent from stays the loop chat — no singleton MA injection.
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Code != "PA" || tabs[0].Mode != domain.ChatModeLoop {
		t.Fatalf("tabs after engage = %+v, want [PA] in loop mode", tabs)
	}
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != domain.ChatModeLoop {
		t.Errorf("PA selection mode = %q, want loop", mode)
	}

	lock := eng.SendWorkspaceMessage(ws.ID, "PA", "edit something", "", nil, nil, "agent", "")
	if !strings.Contains(lock, "loop is running") {
		t.Fatalf("agent-mode send on the loop tab should be locked, got %q", lock)
	}
	ask := eng.SendWorkspaceMessage(ws.ID, "PA", "what changed?", "", nil, nil, "ask", "")
	if strings.Contains(ask, "loop is running") {
		t.Fatalf("ask mode must not be locked, got %q", ask)
	}
	sibling := eng.CreateWorkspaceChat(ws.ID)
	if free := eng.SendWorkspaceMessage(ws.ID, sibling.Code, "edit something", "", nil, nil, "agent", ""); strings.Contains(free, "loop is running") {
		t.Fatalf("agent-mode send on a sibling tab must not be locked, got %q", free)
	}

	// A second Loop tab is its own loop on this workspace.
	tabB := eng.CreateWorkspaceChat(ws.ID)
	again := eng.SendWorkspaceMessage(ws.ID, tabB.Code, "converge the reviews", "", nil, nil, "loop", "")
	if strings.Contains(again, "already running") || strings.Contains(again, "another workspace") ||
		strings.Contains(again, "I couldn't engage") {
		t.Fatalf("second Loop chat = %q", again)
	}
	if n := len(st.Workspaces()); n != 1 {
		t.Fatalf("second Loop chat created %d workspaces, want 1", n)
	}
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, tabB.Code); mode != domain.ChatModeLoop {
		t.Errorf("second tab mode = %q, want loop", mode)
	}
	if !eng.chatLoopStarted(ws.ID, tabB.Code) {
		t.Fatal("second Loop chat should be marked started")
	}
}

// A tab whose own loop settled is not locked while a sibling tab's loop runs.
func TestSettledLoopTabNotLockedBySiblingLoop(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("settled-sess", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	eng.stampChatLoop(ws.ID, "PA", "", "", "", "", true)
	owner := eng.CreateWorkspaceChat(ws.ID)
	if _, ok := st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = owner.Code
		w.State = domain.StateQueued
	}); !ok {
		t.Fatal("UpdateWorkspace")
	}

	if lock := eng.SendWorkspaceMessage(ws.ID, owner.Code, "edit something", "", nil, nil, "agent", ""); !strings.Contains(lock, "loop is running") {
		t.Fatalf("agent-mode send on the owner tab should be locked, got %q", lock)
	}
	if free := eng.SendWorkspaceMessage(ws.ID, "PA", "edit something", "", nil, nil, "agent", ""); strings.Contains(free, "loop is running") {
		t.Fatalf("settled loop tab must not be locked by a sibling's loop, got %q", free)
	}
}

// A settled workspace still holding another graph's checkpoint must not idle
// a new Loop tab as "phase missing". Review & fix (start=review) opens on
// the reviewer from this chat's entry.
func TestSecondLoopChatStartsFreshWalkFromEntry(t *testing.T) {
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if _, ok := st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopID = "test.loops.verify"
		w.LoopChatCode = "PA"
		w.State = domain.StatePaused
		w.Phase = "idle"
		w.Checkpoint = &domain.Checkpoint{PhaseID: "pr"}
	}); !ok {
		t.Fatal("UpdateWorkspace")
	}
	eng.ensurePairAgent(ws.ID, "PA", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	if tabB.Code == "PA" || tabB.Code == "" {
		t.Fatalf("second tab = %+v, want a new PA* code", tabB)
	}
	eng.RememberChatLoopPolicy(ws.ID, tabB.Code, "test.loops.converge", "test.policies.fail")
	reply := eng.SendWorkspaceMessage(ws.ID, tabB.Code, "fix the open findings", "", nil, nil, "loop", "")
	if strings.Contains(reply, "already has a loop") || strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("second Loop chat = %q", reply)
	}

	waitFor(t, 15*time.Second, "fresh converge walk to review or idle", func() bool {
		got, _ := st.Workspace(ws.ID)
		if got.Phase == "phase missing" {
			return true
		}
		runner.mu.Lock()
		n := runner.reviewRuns
		runner.mu.Unlock()
		return n > 0 || got.State == domain.StateReviewing
	})
	got, _ := st.Workspace(ws.ID)
	if got.Phase == "phase missing" {
		t.Fatal("second Loop chat reused the sibling checkpoint and idled")
	}
	runner.mu.Lock()
	reviews := runner.reviewRuns
	runner.mu.Unlock()
	if reviews < 1 && got.State != domain.StateReviewing {
		t.Fatalf("reviewer never ran: state=%s phase=%s", got.State, got.Phase)
	}
}

func TestLoopChatRenameNudgeFirstSession(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("loop-nudge", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
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

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "add a dark-mode toggle", "", nil, nil, "loop", "")
	if specs := runner.specsWithRole(domain.RoleMaestro); len(specs) != 0 {
		t.Fatalf("first engage ran a conductor turn: %+v", specs[0].Prompt)
	}
	named, _ := st.Workspace(ws.ID)
	if named.Goal != "add a dark-mode toggle" {
		t.Errorf("provisional workspace title = %q, want add a dark-mode toggle", named.Goal)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "add a dark-mode toggle" {
		t.Errorf("tab title = %+v, want add a dark-mode toggle", tabs)
	}
}

func TestLoopModePlannerRenameNudge(t *testing.T) {
	goal := "add a dark-mode toggle"
	tests := []struct {
		name      string
		wsName    string
		wantNudge []string
		wantTools []string
		noNudge   []string
		noTools   []string
	}{
		{
			name:   "unnamed",
			wsName: "",
			wantNudge: []string{
				"first session in this workspace",
				mcpRenameWorkspace, mcpRenameBranch, mcpRenameSession,
			},
			wantTools: []string{mcpRenameWorkspace, mcpRenameBranch, mcpRenameSession},
		},
		{
			name:      "named",
			wsName:    "scratch",
			wantNudge: []string{"first message in this chat"},
			wantTools: []string{mcpRenameSession},
			noNudge:   []string{"first session in this workspace", mcpRenameWorkspace, mcpRenameBranch},
			noTools:   []string{mcpRenameWorkspace, mcpRenameBranch},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			runner := &captureRunner{
				linesFor: func(int) [][]byte { return maestroFixture("loop-planner-nudge", "OK.") },
			}
			eng, st, _, _ := newTestEngine(t, runner)
			projID := addProject(t, st, initRepo(t))
			ws, err := eng.CreateWorkspace(projID, tt.wsName, "")
			if err != nil {
				t.Fatalf("CreateWorkspace: %v", err)
			}
			reply := eng.SendWorkspaceMessage(ws.ID, "PA", goal, "", nil, nil, "loop", "")
			if strings.Contains(reply, "I couldn't engage") {
				t.Fatalf("SendWorkspaceMessage loop engage = %q, want success", reply)
			}

			var spec AgentSpec
			waitFor(t, 10*time.Second, "planner spec with rename nudge", func() bool {
				for _, s := range runner.specsWithRole(domain.RoleMaestro) {
					if s.AppendSystemPrompt != "" {
						spec = s
						return true
					}
				}
				return false
			})
			for _, want := range tt.wantNudge {
				if !strings.Contains(spec.AppendSystemPrompt, want) {
					t.Errorf("runPlanner AppendSystemPrompt missing %q, got %q", want, spec.AppendSystemPrompt)
				}
			}
			for _, refuse := range tt.noNudge {
				if strings.Contains(spec.AppendSystemPrompt, refuse) {
					t.Errorf("runPlanner AppendSystemPrompt has %q, want absent; got %q", refuse, spec.AppendSystemPrompt)
				}
			}
			for _, tool := range tt.wantTools {
				if !hasTool(spec, tool) {
					t.Errorf("runPlanner AllowedTools missing %q, got %v", tool, spec.AllowedTools)
				}
			}
			for _, tool := range tt.noTools {
				if hasTool(spec, tool) {
					t.Errorf("runPlanner AllowedTools has %q, want absent; got %v", tool, spec.AllowedTools)
				}
			}
		})
	}
}

func TestLoopEngageSkipsConductorTurn(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("loop-engage", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
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

	mission := "Review the current worktree, then fix and re-review until the policy bar passes."
	reply := eng.SendWorkspaceMessage(ws.ID, "PA", mission, "", nil, nil, "loop", "Review & fix")
	if reply != "" {
		t.Errorf("engage reply = %q, want empty (no conductor turn)", reply)
	}
	if specs := runner.specsWithRole(domain.RoleMaestro); len(specs) != 0 {
		t.Fatalf("first engage ran a conductor turn: %q", specs[0].Prompt)
	}
	got, _ := st.Workspace(ws.ID)
	if !got.HasRun() {
		t.Fatal("walk did not start")
	}
	if g, _ := eng.chatLoopGoal(ws.ID, "PA"); g != mission {
		t.Errorf("chatLoopGoal = %q, want wire %q", g, mission)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "Review & fix" {
		t.Errorf("tab title = %+v, want Review & fix", tabs)
	}
	items := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if len(items) != 1 || items[0].Role != "user" || items[0].Text != "Review & fix" {
		t.Fatalf("engage transcript = %+v, want one user bubble Review & fix", items)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "where are we?", "", nil, nil, "loop", "")
	specs := runner.specsWithRole(domain.RoleMaestro)
	if len(specs) != 1 {
		t.Fatalf("follow-up Maestro specs = %d, want 1", len(specs))
	}
	follow := specs[0]
	if !strings.Contains(follow.Prompt, "where are we?") {
		t.Errorf("follow-up Maestro Prompt = %q, want where are we?", follow.Prompt)
	}
	if !strings.Contains(follow.SystemPrompt, "do not do the loop's phase work yourself") {
		t.Errorf("follow-up SystemPrompt missing conductor split, got %q", follow.SystemPrompt)
	}
}

func TestLoopEngagePersistsImageAttachments(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("loop-img", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
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

	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	marker := AttachmentMarker("img1")
	reply := eng.SendWorkspaceMessage(ws.ID, "PA", "look at this "+marker, "", nil, []domain.ChatAttachment{{
		ID: "img1", Name: "shot.png", Mime: "image/png", Data: png,
	}}, "loop", "")
	if reply != "" {
		t.Fatalf("engage reply = %q, want empty", reply)
	}

	items := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if len(items) != 1 || items[0].Role != "user" {
		t.Fatalf("engage transcript = %+v, want one user bubble", items)
	}
	if !strings.Contains(items[0].Text, marker) {
		t.Errorf("bubble text = %q, want attachment marker for inline thumbs", items[0].Text)
	}
	if len(items[0].Attachments) != 1 || items[0].Attachments[0].ID != "img1" || items[0].Attachments[0].URL == "" {
		t.Fatalf("attachments = %+v, want img1 with a url", items[0].Attachments)
	}
	named, _ := st.Workspace(ws.ID)
	if strings.Contains(named.Goal, "⟦attachment:") {
		t.Errorf("workspace goal leaked marker: %q", named.Goal)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "look at this" {
		t.Errorf("tab title = %+v, want look at this", tabs)
	}
}

func TestRetryLoopModeDoesNotEngageOrBypassRunLock(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("retry-loop", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "sonnet", nil, nil, "agent", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "ignored", "sonnet", nil, nil, "loop", chatRetryDisplaySentinel)
	got, _ := st.Workspace(ws.ID)
	if got.HasRun() {
		t.Fatal("loop-mode retry must not engage a run")
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
	reply := eng.SendWorkspaceMessage(ws.ID, "PA", "ship dark mode", "", nil, nil, "loop", "")
	if strings.Contains(reply, "Follow along") || strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("engage reply = %q", reply)
	}

	lock := eng.SendWorkspaceMessage(ws.ID, "PA", "ignored", "", nil, nil, "loop", chatRetryDisplaySentinel)
	if !strings.Contains(lock, "loop is running") {
		t.Fatalf("loop-mode retry should be run-locked, got %q", lock)
	}
}

func TestCloseLoopThenNewReviewFixIsFresh(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("reengage", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
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

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "review the worktree", "", nil, nil, "loop", "Review & fix")
	co, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "coder", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"CO","name":"Coder","ownerCode":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CO session: %v", err)
	}
	if _, err := st.AppendMessage(co.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("old coder turn")},
	}); err != nil {
		t.Fatalf("CO message: %v", err)
	}

	tabs := eng.CloseWorkspaceChat(ws.ID, "PA")
	if len(tabs) != 1 || tabs[0].Code == "MA" || tabs[0].Code == "PA" {
		t.Fatalf("after close = %+v, want a fresh pair tab, not MA/PA", tabs)
	}
	fresh := tabs[0].Code
	if rec, _ := st.Workspace(ws.ID); rec.LoopChatCode != "" {
		t.Fatalf("LoopChatCode after close = %q, want empty", rec.LoopChatCode)
	}
	if _, ok := st.SessionForChatAgent(ws.ID, "PA", "CO"); !ok {
		t.Fatal("closed PA must still own its CO transcript")
	}
	closed := eng.ListClosedWorkspaceChats(ws.ID)
	if len(closed) != 1 || closed[0].Code != "PA" {
		t.Fatalf("closed chats = %+v, want [PA]", closed)
	}

	reply := eng.SendWorkspaceMessage(ws.ID, fresh, "review the worktree", "", nil, nil, "loop", "Review & fix")
	if reply != "" {
		t.Errorf("re-engage reply = %q, want empty", reply)
	}
	items := contentItems(eng.WorkspaceChatItems(ws.ID, fresh))
	if len(items) != 1 || items[0].Role != "user" || items[0].Text != "Review & fix" {
		t.Fatalf("fresh transcript = %+v, want one new Review & fix bubble", items)
	}
	old := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if len(old) != 1 || old[0].Text != "Review & fix" {
		t.Fatalf("closed PA transcript = %+v, want the original bubble", old)
	}
	if rec, _ := st.Workspace(ws.ID); rec.LoopChatCode != fresh {
		t.Fatalf("LoopChatCode after re-engage = %q, want %s", rec.LoopChatCode, fresh)
	}
	if _, ok := st.SessionForChatAgent(ws.ID, fresh, "CO"); ok {
		t.Fatal("new tab inherited the previous loop's CO session")
	}
}

func TestFailedEngageDoesNotMarkTabStarted(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	eng.RememberChatLoopPolicy(ws.ID, "PA", "no.such.loop", "")
	reply := eng.SendWorkspaceMessage(ws.ID, "PA", "review and fix", "", nil, nil, "loop", "Review & fix")
	if !strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("reply = %q, want engage error", reply)
	}
	if eng.chatLoopStarted(ws.ID, "PA") {
		t.Fatal("failed engage marked LoopStarted")
	}
	if got, _ := eng.chatLoopGoal(ws.ID, "PA"); got != "review and fix" {
		t.Fatalf("chatLoopGoal after refused launch = %q, want persisted goal", got)
	}
	got, _ := st.Workspace(ws.ID)
	if got.HasRun() {
		t.Fatal("failed engage started a run")
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
	eng.RememberChatLoopPolicy(ws.ID, "PA", "test.loops.converge", "test.policies.fail")
	reply = eng.SendWorkspaceMessage(ws.ID, "PA", "review and fix", "", nil, nil, "loop", "Review & fix")
	if strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("retry after correcting the loop = %q", reply)
	}
	if !eng.chatLoopStarted(ws.ID, "PA") {
		t.Fatal("retry after refused launch did not engage")
	}
	got, _ = st.Workspace(ws.ID)
	if !got.HasRun() {
		t.Fatal("retry after refused launch did not start a run")
	}
}

func TestFirstEngageStampsOwnerBeforeDispatch(t *testing.T) {
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	tab := eng.CreateWorkspaceChat(ws.ID)
	if tab.Code == "" || tab.Code == "PA" {
		t.Fatalf("second tab = %+v, want PA2+", tab)
	}
	eng.RememberChatLoopPolicy(ws.ID, tab.Code, "test.loops.converge", "test.policies.fail")
	reply := eng.SendWorkspaceMessage(ws.ID, tab.Code, "fix the open findings", "", nil, nil, "loop", "Review & fix")
	if strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("engage = %q", reply)
	}
	waitFor(t, 15*time.Second, "reviewer ran", func() bool {
		runner.mu.Lock()
		n := runner.reviewRuns
		runner.mu.Unlock()
		return n > 0
	})
	for _, spec := range runner.capturedSpecs() {
		if spec.ChatCode != tab.Code {
			t.Fatalf("spec role=%s ChatCode=%q, want %s", spec.Role, spec.ChatCode, tab.Code)
		}
	}
}

func TestSpecialistSessionsKeyedByOwner(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	a, ok := eng.ensureOwnedAgentSession(ws.ID, "coder", "CO", "Coder", "", "PA")
	if !ok {
		t.Fatal("PA CO")
	}
	b, ok := eng.ensureOwnedAgentSession(ws.ID, "coder", "CO", "Coder", "", "PA2")
	if !ok {
		t.Fatal("PA2 CO")
	}
	if a.ID == b.ID {
		t.Fatal("two conductor tabs must not share a CO session")
	}
	got, ok := st.SessionForChatAgent(ws.ID, "PA", "CO")
	if !ok || got.ID != a.ID {
		t.Fatalf("PA lookup = %v %s, want %s", ok, got.ID, a.ID)
	}
	got, ok = st.SessionForChatAgent(ws.ID, "PA2", "CO")
	if !ok || got.ID != b.ID {
		t.Fatalf("PA2 lookup = %v %s, want %s", ok, got.ID, b.ID)
	}
}

func TestStartChatLoopWalkDoesNotRestoreClosedTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = pairAgentCode
		w.State = domain.StatePaused
		w.Phase = "idle"
	})
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	if tabB.Code == "" || tabB.Code == pairAgentCode {
		t.Fatalf("second tab = %+v, want a new PA* code", tabB)
	}
	_ = eng.CloseWorkspaceChat(ws.ID, tabB.Code)
	if _, ok := st.Agent(ws.ID, tabB.Code); ok {
		t.Fatal("closed tab still has an agent")
	}
	eng.startChatLoopWalk(ws.ID, tabB.Code, "", "", "fix it")
	if rec, _ := st.Workspace(ws.ID); rec.LoopChatCode == tabB.Code {
		t.Fatalf("LoopChatCode restored closed tab %q", rec.LoopChatCode)
	}
	if eng.runningLoopByChat(ws.ID, tabB.Code) != nil {
		t.Fatal("closed tab walk registered")
	}
}

func TestStartChatLoopWalkCloseAfterUnlockDoesNotStartLegacyWalk(t *testing.T) {
	t.Run("immediate dispatch", func(t *testing.T) {
		eng, st, ws := startChatLoopWalkCloseFixture(t)
		eng.startWalkHook = func() {
			_ = eng.CloseWorkspaceChat(ws.ID, pairAgentCode)
		}
		eng.startChatLoopWalk(ws.ID, pairAgentCode, "", "", "fix it")
		assertClosedTabDidNotStartLegacyWalk(t, eng, st, ws.ID)
	})
	t.Run("queued then drain", func(t *testing.T) {
		eng, st, ws := startChatLoopWalkCloseFixture(t)
		slots := 0
		for eng.gov.TryAcquireLoop() {
			slots++
		}
		t.Cleanup(func() {
			for i := 0; i < slots; i++ {
				eng.gov.ReleaseLoop()
			}
		})
		eng.startWalkHook = func() {
			_ = eng.CloseWorkspaceChat(ws.ID, pairAgentCode)
		}
		eng.startChatLoopWalk(ws.ID, pairAgentCode, "", "", "fix it")
		if rec, _ := st.Workspace(ws.ID); rec.State == domain.StateQueued {
			t.Fatal("closed tab must not be requeued")
		}
		if slots > 0 {
			eng.gov.ReleaseLoop()
			slots--
		}
		eng.drainQueued()
		assertClosedTabDidNotStartLegacyWalk(t, eng, st, ws.ID)
	})
}

func TestStartChatLoopWalkClearsStaleGoalDisplay(t *testing.T) {
	eng, st, ws := startChatLoopWalkCloseFixture(t)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.Goal = "Linear ENG-123\n\nFull issue body"
		w.GoalDisplay = "Linear ENG-123 · title"
	})
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	t.Cleanup(func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	})
	eng.startChatLoopWalk(ws.ID, pairAgentCode, "", "", "fix it")
	got, _ := st.Workspace(ws.ID)
	if got.Goal != "fix it" {
		t.Errorf("startChatLoopWalk Goal = %q, want %q", got.Goal, "fix it")
	}
	if got.GoalDisplay != "" {
		t.Errorf("startChatLoopWalk GoalDisplay = %q, want empty after a different goal", got.GoalDisplay)
	}
}

func startChatLoopWalkCloseFixture(t *testing.T) (*Engine, *store.Store, domain.Workspace) {
	t.Helper()
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = pairAgentCode
		w.State = domain.StatePaused
		w.Phase = "idle"
	})
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	return eng, st, ws
}

func assertClosedTabDidNotStartLegacyWalk(t *testing.T, eng *Engine, st *store.Store, id string) {
	t.Helper()
	if rec, ok := st.Workspace(id); !ok || rec.LoopChatCode != "" {
		t.Fatalf("LoopChatCode = %q, want empty", rec.LoopChatCode)
	}
	if rec, _ := st.Workspace(id); rec.State == domain.StateFailed {
		t.Fatalf("closed tab start marked failed: %s", rec.Phase)
	}
	if rec, _ := st.Workspace(id); rec.State == domain.StatePlanning || rec.State == domain.StateCoding {
		t.Fatalf("closed tab walk started: state=%s", rec.State)
	}
	if rec, _ := st.Workspace(id); rec.State == domain.StateQueued {
		t.Fatal("closed tab must not stay queued for drain")
	}
	if _, running := eng.loops.Load(id); running {
		t.Fatal("closed tab must not start a legacy empty-code walk")
	}
	if eng.runningLoopByChat(id, pairAgentCode) != nil {
		t.Fatal("closed tab walk registered")
	}
}

func TestLoopGoalImagesUsesEngageTurnNotFirstChat(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("engage-img", "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "see "+AttachmentMarker("imgA"), "sonnet", nil, []domain.ChatAttachment{{
		ID: "imgA", Name: "a.png", Mime: "image/png", Data: png,
	}}, "agent", "")

	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	t.Cleanup(func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	})
	reply := eng.SendWorkspaceMessage(ws.ID, "PA", "now "+AttachmentMarker("imgB"), "", nil, []domain.ChatAttachment{{
		ID: "imgB", Name: "b.png", Mime: "image/png", Data: png,
	}}, "loop", "")
	if strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("engage = %q", reply)
	}
	imgs := eng.loopGoalImages(ws.ID, "PA")
	if len(imgs) != 1 || imgs[0].ID != "imgB" {
		t.Fatalf("loopGoalImages = %+v, want [imgB] from the engage turn", imgs)
	}
}

func TestReengageKeepsPromptActionGoalDisplay(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	installFixturePack(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = pairAgentCode
		w.State = domain.StatePaused
		w.Phase = "idle"
	})
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	t.Cleanup(func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	})

	tab := eng.CreateWorkspaceChat(ws.ID)
	wire := "The human already wrote this change. Review the current worktree, then fix and re-review."
	eng.SendWorkspaceMessage(ws.ID, tab.Code, wire, "", nil, nil, "loop", "Review & fix")

	rec, _ := st.Workspace(ws.ID)
	if rec.Goal != wire || rec.GoalDisplay != "Review & fix" {
		t.Fatalf("workspace goal = %q display = %q, want the wire labelled Review & fix", rec.Goal, rec.GoalDisplay)
	}
	if got := newLoop(eng, eng.chatScopedWorkspace(rec, tab.Code)).goalLabel(); got != "Review & fix" {
		t.Errorf("loop goalLabel = %q, want Review & fix", got)
	}
}
