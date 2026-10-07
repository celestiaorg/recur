package engine

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/mcp"
	"recur/internal/store"
)

// TestSendWorkspaceMessage covers the workspace coding-agent chat: it runs in the
// workspace worktree, does NOT override Claude Code's system prompt, gets the
// editing tool set, honors the requested model, and persists its transcript.
func TestSendWorkspaceMessage(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("ws-sess-1", "Edited the file.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	reply := eng.SendWorkspaceMessage(ws.ID, "", "edit the file", "opus", nil, nil, "", "")
	if !strings.Contains(reply, "Edited the file") {
		t.Fatalf("reply: got %q", reply)
	}

	// Find the coder chat by Role.
	s0, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no coding-agent chat turn ran")
	}
	if s0.SystemPrompt != "" {
		t.Errorf("workspace chat must NOT override Claude Code's system prompt, got %q", s0.SystemPrompt)
	}
	if s0.Model != "opus" {
		t.Errorf("model not passed through: got %q, want opus", s0.Model)
	}
	if s0.Worktree != ws.WorktreePath {
		t.Errorf("turn ran in the wrong dir: got %q, want %q", s0.Worktree, ws.WorktreePath)
	}
	has := func(tool string) bool {
		return slices.Contains(s0.AllowedTools, tool)
	}
	if !has("Edit") || !has("Write") {
		t.Errorf("workspace agent must be able to edit; tools = %v", s0.AllowedTools)
	}
	// Agent mode wires findings checklist + skills MCP; not done (loop handback)
	// or escalate/plan hands.
	if !has(mcpListFindings) || !has(mcpUpdateFinding) {
		t.Errorf("agent chat needs findings MCP tools; tools = %v", s0.AllowedTools)
	}
	if !has(mcpListSkills) || !has(mcpActivateSkill) {
		t.Errorf("agent chat needs skills MCP tools; tools = %v", s0.AllowedTools)
	}
	if !has(mcpListTerminals) || !has(mcpReadTerminal) {
		t.Errorf("agent chat needs terminal MCP tools; tools = %v", s0.AllowedTools)
	}
	if !has(mcpOpenPR) {
		t.Errorf("agent chat needs open_pr MCP tool; tools = %v", s0.AllowedTools)
	}
	if !has(mcpCreateWorkspace) {
		t.Errorf("agent chat needs create_workspace MCP tool; tools = %v", s0.AllowedTools)
	}
	if !has(mcpMessageChild) {
		t.Errorf("agent chat needs message_child MCP tool; tools = %v", s0.AllowedTools)
	}
	if s0.MCPURL == "" {
		t.Error("agent chat should wire the per-agent Recur MCP endpoint")
	}
	for _, tl := range s0.AllowedTools {
		if tl == mcpEscalate || tl == mcpReportFinding || tl == mcpDone {
			t.Errorf("agent chat must not carry %q", tl)
		}
	}

	// Transcript persisted for reload — via the session store (single source of truth).
	chat := contentItems(eng.WorkspaceChatItems(ws.ID, ""))
	if len(chat) != 2 || chat[0].Text != "edit the file" || chat[0].Role != "user" || chat[1].Role != "assistant" {
		t.Fatalf("transcript not persisted: %+v", chat)
	}
	if got := st.LastAssistantText(ws.ID); !strings.Contains(got, "Edited the file") {
		t.Errorf("LastAssistantText = %q, want reply text", got)
	}

	// Dock flags travel with the turn (not only Coder settings defaults).
	_ = eng.SendWorkspaceMessage(ws.ID, "", "with flags", "claude-code/sonnet", map[string]string{"effort": "max", "context": "1m"}, nil, "", "")
	coderTurns := runner.specsWithRole(domain.RoleCoder)
	if len(coderTurns) < 2 {
		t.Fatal("flags turn did not run")
	}
	s1 := coderTurns[len(coderTurns)-1]
	if s1.Flags["effort"] != "max" || s1.Flags["context"] != "1m" {
		t.Errorf("flags not passed through: got %#v", s1.Flags)
	}
	if s0.MCPURL == "" || s0.MCPURL != s1.MCPURL {
		t.Errorf("follow-up MCP URL should reuse the first turn's token: first=%q second=%q", s0.MCPURL, s1.MCPURL)
	}
	if s1.RefreshStandingInstructions {
		t.Error("ordinary follow-up must not RefreshStandingInstructions")
	}

	selModel, selFlags, _ := eng.WorkspaceChatSelection(ws.ID, "")
	if selModel != "claude-code/sonnet" {
		t.Errorf("WorkspaceChatSelection model = %q, want claude-code/sonnet", selModel)
	}
	if selFlags["effort"] != "max" || selFlags["context"] != "1m" {
		t.Errorf("WorkspaceChatSelection flags = %#v", selFlags)
	}
}

// TestRenameNudgeFirstWorkspaceSession: the first PA* message in a new workspace
// asks the agent to rename workspace, branch, and chat — and sets a provisional
// Goal from the first message until rename_workspace.
func TestRenameNudgeFirstWorkspaceSession(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("nudge-%d", call), "OK.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.Goal != defaultWorkspaceTitle {
		t.Fatalf("create goal=%q, want %q", ws.Goal, defaultWorkspaceTitle)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "", "add a dark-mode toggle", "", nil, nil, "", "")
	still, _ := st.Workspace(ws.ID)
	if still.Goal != "add a dark-mode toggle" {
		t.Fatalf("provisionalWorkspaceTitle from first message: Goal=%q, want %q", still.Goal, "add a dark-mode toggle")
	}

	specs := runner.specsWithAgentID(ws.ID + "-PA")
	if len(specs) < 1 {
		t.Fatal("expected a chat turn")
	}
	got := specs[0].AppendSystemPrompt
	for _, want := range []string{
		"first session in this workspace",
		"mcp__recur__rename_workspace",
		"mcp__recur__rename_branch",
		"mcp__recur__rename_session",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("first-session prompt missing %q, got %q", want, got)
		}
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "", "and persist the preference", "", nil, nil, "", "")
	specs = runner.specsWithAgentID(ws.ID + "-PA")
	if len(specs) < 2 {
		t.Fatal("expected follow-up turn")
	}
	if strings.Contains(specs[1].AppendSystemPrompt, "first session") ||
		strings.Contains(specs[1].AppendSystemPrompt, "first message in this chat") {
		t.Errorf("follow-up should not nudge rename, got %q", specs[1].AppendSystemPrompt)
	}
	afterSecond, _ := st.Workspace(ws.ID)
	if afterSecond.Goal != "add a dark-mode toggle" {
		t.Fatalf("second message changed provisional Goal=%q, want %q", afterSecond.Goal, "add a dark-mode toggle")
	}

	labeled, err := eng.CreateWorkspace(projID, "keep this name", "")
	if err != nil {
		t.Fatalf("CreateWorkspace labeled: %v", err)
	}
	_ = eng.SendWorkspaceMessage(labeled.ID, "", "something else entirely", "", nil, nil, "", "")
	kept, _ := st.Workspace(labeled.ID)
	if kept.Goal != "keep this name" {
		t.Fatalf("explicit name overwritten without MCP: got %q", kept.Goal)
	}
	namedSpecs := runner.specsWithAgentID(labeled.ID + "-PA")
	if len(namedSpecs) < 1 {
		t.Fatal("expected labeled workspace chat turn")
	}
	namedPrompt := namedSpecs[0].AppendSystemPrompt
	if strings.Contains(namedPrompt, "rename_workspace") || strings.Contains(namedPrompt, "first session in this workspace") {
		t.Errorf("named workspace must not nudge rename_workspace, got %q", namedPrompt)
	}
	if !strings.Contains(namedPrompt, "mcp__recur__rename_session") {
		t.Errorf("named workspace first message should still nudge chat rename, got %q", namedPrompt)
	}
}

func TestProvisionalWorkspaceTitle(t *testing.T) {
	longWord := strings.Repeat("w", workspaceTitleLimit+20)
	tests := []struct {
		name  string
		input string
		want  string
	}{
		{name: "empty", input: "", want: ""},
		{name: "whitespace", input: "  \n\t  ", want: ""},
		{name: "default title", input: defaultWorkspaceTitle, want: ""},
		{name: "short", input: "add a dark-mode toggle", want: "add a dark-mode toggle"},
		{name: "first line", input: "  fix the flaky test  \nmore context below", want: "fix the flaky test"},
		{name: "eight words", input: "one two three four five six seven eight", want: "one two three four five six seven eight"},
		{name: "nine words", input: "one two three four five six seven eight nine", want: "one two three four five six seven eight"},
		{name: "collapse space", input: "  add   dark   mode  ", want: "add dark mode"},
		{name: "long word", input: longWord, want: longWord[:workspaceTitleLimit]},
		{name: "leading attachment", input: AttachmentMarker("img1") + " fix the flaky test", want: "fix the flaky test"},
		{name: "attachment own line", input: AttachmentMarker("img1") + "\nfix the flaky test", want: "fix the flaky test"},
		{name: "trailing attachment", input: "look at this " + AttachmentMarker("img1"), want: "look at this"},
		{name: "image only", input: AttachmentMarker("img1"), want: ""},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := provisionalWorkspaceTitle(tt.input)
			if got != tt.want {
				t.Errorf("provisionalWorkspaceTitle(%q) = %q, want %q", tt.input, got, tt.want)
			}
		})
	}
}

func TestProvisionalWorkspaceTitleFromFirstMessage(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("prov", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	msg := "one two three four five six seven eight nine ten"
	_ = eng.SendWorkspaceMessage(ws.ID, "", msg, "", nil, nil, "", "")
	got, _ := st.Workspace(ws.ID)
	want := "one two three four five six seven eight"
	if got.Goal != want {
		t.Fatalf("provisionalWorkspaceTitle(%q) Goal=%q, want %q", msg, got.Goal, want)
	}

	_ = eng.RenameWorkspace(ws.ID, "Dark mode")
	named, _ := st.Workspace(ws.ID)
	if named.Goal != "Dark mode" {
		t.Fatalf("RenameWorkspace Goal=%q, want %q", named.Goal, "Dark mode")
	}
}

func TestApplyProvisionalTitlesBeforeSend(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "", nil, domain.ChatModeAgent, nil)

	msg := "add a dark-mode toggle"
	eng.ApplyProvisionalTitles(ws.ID, "PA", msg)
	got, _ := st.Workspace(ws.ID)
	if got.Goal != msg {
		t.Fatalf("Goal=%q, want %q", got.Goal, msg)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != msg {
		t.Fatalf("tab title = %+v, want %q", tabs, msg)
	}
	if nudge := eng.renameNudgePrompt(ws.ID, "PA", domain.ChatModeAgent, msg); nudge != firstWorkspaceRenamePrompt {
		t.Fatalf("nudge after early title = %q, want first-workspace prompt", nudge)
	}
}

func TestEarlyTitlesFromRemovedQueuedMessageAreRederived(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("early", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "", nil, domain.ChatModeAgent, nil)

	eng.ApplyProvisionalTitles(ws.ID, "PA", "queued then removed")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "the real first message", "", nil, nil, "", "")

	got, _ := st.Workspace(ws.ID)
	if got.Goal != "the real first message" {
		t.Fatalf("Goal=%q, want the sent message", got.Goal)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "the real first message" {
		t.Fatalf("tab title = %+v, want the sent message", tabs)
	}
	specs := runner.specsWithAgentID(ws.ID + "-PA")
	if len(specs) < 1 || !strings.Contains(specs[0].AppendSystemPrompt, "mcp__recur__rename_workspace") {
		t.Fatalf("first send should get the first-workspace nudge, got %+v", specs)
	}
}

func TestProvisionalTitleStripsLeadingAttachment(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("attach-title", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	marker := AttachmentMarker("img_mu1fovfk_9oabcd")
	_ = eng.SendWorkspaceMessage(ws.ID, "", marker+" the sidebar chip leaked", "", nil, []domain.ChatAttachment{{
		ID: "img_mu1fovfk_9oabcd", Name: "shot.png", Mime: "image/png", Data: png,
	}}, "", "")
	got, _ := st.Workspace(ws.ID)
	if strings.Contains(got.Goal, "⟦attachment:") || strings.Contains(got.Short, "⟦attachment:") {
		t.Fatalf("workspace title leaked marker: Goal=%q Short=%q", got.Goal, got.Short)
	}
	if got.Goal != "the sidebar chip leaked" {
		t.Fatalf("Goal=%q, want the sidebar chip leaked", got.Goal)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "the sidebar chip leaked" {
		t.Fatalf("tab title = %+v, want the sidebar chip leaked", tabs)
	}
}

// TestRenameNudgeFirstChatOnly: a later tab's first message asks only for
// rename_session, not workspace/branch.
func TestRenameNudgeFirstChatOnly(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("tab-%d", call), "OK.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "named workspace", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "task A", "", nil, nil, "", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	_ = eng.SendWorkspaceMessage(ws.ID, tabB.Code, "task B", "", nil, nil, "", "")

	pa1 := runner.specsWithAgentID(ws.ID + "-PA")
	if len(pa1) < 1 {
		t.Fatal("expected PA turn")
	}
	if strings.Contains(pa1[0].AppendSystemPrompt, "rename_workspace") ||
		strings.Contains(pa1[0].AppendSystemPrompt, "first session in this workspace") {
		t.Errorf("named workspace first PA message must not nudge workspace/branch rename, got %q", pa1[0].AppendSystemPrompt)
	}

	pa2 := runner.specsWithAgentID(ws.ID + "-" + tabB.Code)
	if len(pa2) < 1 {
		t.Fatal("expected PA2 turn")
	}
	got := pa2[0].AppendSystemPrompt
	if !strings.Contains(got, "first message in this chat") || !strings.Contains(got, "mcp__recur__rename_session") {
		t.Errorf("later-tab first message should nudge chat rename, got %q", got)
	}
	if strings.Contains(got, "rename_workspace") || strings.Contains(got, "first session in this workspace") {
		t.Errorf("later tab must not get the all-three nudge, got %q", got)
	}
}

func TestSanitizeWorkspaceTitle(t *testing.T) {
	cases := []struct{ in, want string }{
		{`"Dark mode toggle"`, "Dark mode toggle"},
		{"Dark mode toggle.", "Dark mode toggle"},
		{"Dark mode toggle\n(with more)", "Dark mode toggle"},
		{"  ", ""},
		{defaultWorkspaceTitle, ""},
	}
	for _, tc := range cases {
		if got := sanitizeWorkspaceTitle(tc.in); got != tc.want {
			t.Errorf("sanitizeWorkspaceTitle(%q)=%q, want %q", tc.in, got, tc.want)
		}
	}
}

// TestRenameWorkspaceAndBranch: MCP-style engine helpers set Goal/Short and
// rename the live git branch, painting workspace.updated.
func TestRenameWorkspaceAndBranch(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	msg := eng.RenameWorkspace(ws.ID, "Dark mode toggle")
	if !strings.Contains(msg, "Dark mode toggle") {
		t.Fatalf("RenameWorkspace = %q", msg)
	}
	named, ok := st.Workspace(ws.ID)
	if !ok || named.Goal != "Dark mode toggle" || named.Short != "Dark mode toggle" {
		t.Fatalf("after rename: ok=%v goal=%q short=%q", ok, named.Goal, named.Short)
	}
	sawGoal := false
	for _, ev := range sink.byName("workspace.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == ws.ID && d["goal"] == "Dark mode toggle" {
			sawGoal = true
			break
		}
	}
	if !sawGoal {
		t.Error("workspace.updated did not carry the new goal")
	}

	_ = eng.ListWorkspaceChats(ws.ID)

	oldBranch := named.Branch
	msg = eng.RenameWorkspaceBranch(ws.ID, "add-dark-mode")
	if !strings.Contains(msg, "recur/add-dark-mode") {
		t.Fatalf("RenameWorkspaceBranch = %q (old %q)", msg, oldBranch)
	}
	renamed, _ := st.Workspace(ws.ID)
	if renamed.Branch != "recur/add-dark-mode" {
		t.Fatalf("stored branch = %q, want recur/add-dark-mode", renamed.Branch)
	}
	ctx := context.Background()
	live, err := gitpkg.CurrentBranch(ctx, renamed.WorktreePath)
	if err != nil || live != "recur/add-dark-mode" {
		t.Fatalf("live branch = %q err=%v", live, err)
	}

	again := eng.RenameChatSession(ws.ID, "PA", "Auth refresh")
	if !strings.Contains(again, "Auth refresh") {
		t.Fatalf("RenameChatSession = %q", again)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "Auth refresh" {
		t.Fatalf("chat tab after rename = %+v", tabs)
	}
}

func TestRenameWorkspaceRejectsInvalidTitle(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	for _, title := range []string{"...", defaultWorkspaceTitle, strings.Repeat(".", 80)} {
		msg := eng.RenameWorkspace(ws.ID, title)
		if !strings.Contains(msg, "invalid title") {
			t.Errorf("RenameWorkspace(%q) = %q, want invalid title", title, msg)
		}
		got, _ := st.Workspace(ws.ID)
		if got.Goal != "scratch" {
			t.Errorf("RenameWorkspace(%q) mutated Goal=%q, want scratch", title, got.Goal)
		}
	}

	if _, ok := st.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
		lp.RunStarted = true
		lp.Goal = "ship the original long task"
	}); !ok {
		t.Fatal("UpdateWorkspace RunStarted")
	}
	msg := eng.RenameWorkspace(ws.ID, "Short title")
	if !strings.Contains(msg, "Renamed workspace") {
		t.Errorf("RenameWorkspace on HasRun = %q, want success", msg)
	}
	got, _ := st.Workspace(ws.ID)
	if got.Goal != "Short title" {
		t.Errorf("RenameWorkspace HasRun Goal=%q, want %q", got.Goal, "Short title")
	}
}

func TestRenameWorkspaceBranchRefusesActiveRun(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	old := ws.Branch
	if _, ok := st.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
		lp.RunStarted = true
		lp.State = domain.StateCoding
	}); !ok {
		t.Fatal("UpdateWorkspace coding")
	}
	msg := eng.RenameWorkspaceBranch(ws.ID, "nope")
	if !strings.Contains(msg, "loop is running") {
		t.Errorf("RenameWorkspaceBranch active run = %q, want running refusal", msg)
	}
	got, _ := st.Workspace(ws.ID)
	if got.Branch != old {
		t.Errorf("RenameWorkspaceBranch under run-lock Branch=%q, want %q", got.Branch, old)
	}

	if _, ok := st.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
		lp.State = domain.StatePlanning
	}); !ok {
		t.Fatal("UpdateWorkspace planning")
	}
	msg = eng.RenameWorkspaceBranch(ws.ID, "during-planning")
	if !strings.Contains(msg, "recur/during-planning") {
		t.Errorf("RenameWorkspaceBranch planning = %q, want success", msg)
	}

	if _, ok := st.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
		lp.State = domain.StatePaused
	}); !ok {
		t.Fatal("UpdateWorkspace paused")
	}
	msg = eng.RenameWorkspaceBranch(ws.ID, "after-pause")
	if !strings.Contains(msg, "recur/after-pause") {
		t.Errorf("RenameWorkspaceBranch paused = %q, want success", msg)
	}
}

// Single live loop: RenameChatSession from PL with empty ownerChat retitles
// the loop's ChatCode tab via runningLoopForDone (unique walk).
func TestRenameChatSessionPlannerRetitlesLoopTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	loopPA := &Loop{ID: ws.ID, ChatCode: "PA"}
	eng.loops.Store(ws.ID, &runningLoop{loop: loopPA})
	t.Cleanup(func() { eng.loops.Delete(ws.ID) })

	msg := eng.RenameChatSession(ws.ID, "PL", "Dark mode")
	if !strings.Contains(msg, "Dark mode") {
		t.Fatalf("RenameChatSession(ws, PL, Dark mode) = %q, want success", msg)
	}
	var paTitle string
	for _, tab := range eng.ListWorkspaceChats(ws.ID) {
		if tab.Code == "PA" {
			paTitle = tab.Title
		}
	}
	if paTitle != "Dark mode" {
		t.Errorf("RenameChatSession(ws, PL, Dark mode) PA title = %q, want Dark mode", paTitle)
	}
}

// Two live loops both claiming PL (overlapping planners): ownership comes from
// the calling turn's conductor tab, not from bindInject inference.
func TestRenameChatSessionRoutesPlannerToOwningLoopTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	tab2 := eng.CreateWorkspaceChat(ws.ID)
	if tab2.Code != "PA2" {
		t.Fatalf("CreateWorkspaceChat code = %q, want PA2", tab2.Code)
	}
	if _, ok := st.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
		lp.RunStarted = true
		lp.LoopChatCode = "PA"
		lp.State = domain.StatePlanning
	}); !ok {
		t.Fatal("UpdateWorkspace LoopChatCode")
	}

	loopPA := &Loop{ID: ws.ID, ChatCode: "PA"}
	loopPA2 := &Loop{ID: ws.ID, ChatCode: tab2.Code}
	eng.loops.Store(ws.ID, &runningLoop{loop: loopPA})
	eng.loops.Store(ws.ID+loopMapSep+tab2.Code, &runningLoop{loop: loopPA2})
	t.Cleanup(func() {
		eng.loops.Delete(ws.ID)
		eng.loops.Delete(ws.ID + loopMapSep + tab2.Code)
	})
	_, clearPA := loopPA.bindInject("PL")
	_, clearPA2 := loopPA2.bindInject("PL")
	t.Cleanup(clearPA)
	t.Cleanup(clearPA2)

	msg := engineReporter{eng: eng}.RenameSession(ws.ID, "PL", tab2.Code, "Dark mode")
	if !strings.Contains(msg, "Dark mode") {
		t.Fatalf("RenameSession(PL, PA2, Dark mode) = %q, want success", msg)
	}
	var paTitle, pa2Title string
	for _, tab := range eng.ListWorkspaceChats(ws.ID) {
		switch tab.Code {
		case "PA":
			paTitle = tab.Title
		case tab2.Code:
			pa2Title = tab.Title
		}
	}
	if pa2Title != "Dark mode" {
		t.Errorf("RenameChatSession(ws, PL, Dark mode) PA2 title = %q, want Dark mode", pa2Title)
	}
	if paTitle == "Dark mode" {
		t.Errorf("RenameChatSession(ws, PL, Dark mode) PA title = %q, want unchanged (not the workspace owner)", paTitle)
	}
}

// TestEnsurePairAgentIdleOnListCreate: opening/listing chat tabs must not mark
// the agent "working" — that falsely lights sidebar + chat busy with no turn.
func TestEnsurePairAgentIdleOnListCreate(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "idle-tabs", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	a, ok := st.Agent(ws.ID, "PA")
	if !ok {
		t.Fatal("PA agent missing after ListWorkspaceChats")
	}
	if a.Status != "idle" {
		t.Fatalf("PA status after list = %q, want idle", a.Status)
	}
	tab := eng.CreateWorkspaceChat(ws.ID)
	b, ok := st.Agent(ws.ID, tab.Code)
	if !ok {
		t.Fatalf("agent %s missing after CreateWorkspaceChat", tab.Code)
	}
	if b.Status != "idle" {
		t.Fatalf("%s status after create = %q, want idle", tab.Code, b.Status)
	}
}

// TestMultiWorkspaceChats: a solo workspace can host multiple independent coding
// chat tabs (PA, PA2, …) with separate transcripts and resume ids.
func TestMultiWorkspaceChats(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			// Unique session ids per call.
			return maestroFixture(fmt.Sprintf("sess-%d", call), "Done.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "multi-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("ListWorkspaceChats initial = %+v, want [PA]", tabs)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "task A", "sonnet", nil, nil, "", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	if tabB.Code != "PA2" {
		t.Fatalf("CreateWorkspaceChat code = %q, want PA2", tabB.Code)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "task B", "opus", nil, nil, "", "")

	chatA := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	chatB := contentItems(eng.WorkspaceChatItems(ws.ID, "PA2"))
	if len(chatA) < 2 || chatA[0].Text != "task A" {
		t.Fatalf("chat A transcript: %+v", chatA)
	}
	if len(chatB) < 2 || chatB[0].Text != "task B" {
		t.Fatalf("chat B transcript: %+v", chatB)
	}
	if chatA[0].Text == chatB[0].Text {
		t.Fatal("chats should not share transcripts")
	}

	paTurns := runner.specsWithAgentID(ws.ID + "-PA")
	pa2Turns := runner.specsWithAgentID(ws.ID + "-PA2")
	if len(paTurns) < 1 || len(pa2Turns) < 1 {
		t.Fatalf("coder turns: PA=%d PA2=%d", len(paTurns), len(pa2Turns))
	}
	if paTurns[0].ResumeSession != "" {
		t.Errorf("first PA turn should not resume, got %q", paTurns[0].ResumeSession)
	}
	if pa2Turns[0].ResumeSession != "" {
		t.Errorf("first PA2 turn should not resume, got %q", pa2Turns[0].ResumeSession)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "follow up A", "sonnet", nil, nil, "", "")
	paTurns = runner.specsWithAgentID(ws.ID + "-PA")
	if len(paTurns) < 2 {
		t.Fatal("expected PA follow-up turn")
	}
	if paTurns[1].ResumeSession == "" {
		t.Errorf("PA follow-up should resume prior session, got empty")
	}
	if paTurns[1].AgentID != ws.ID+"-PA" {
		t.Errorf("follow-up agent = %q", paTurns[1].AgentID)
	}

	tabs = eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 2 {
		t.Fatalf("ListWorkspaceChats after create = %+v", tabs)
	}
	// First-message fallback titles (truncated user text until MCP rename).
	if tabs[0].Title == "New chat" || tabs[1].Title == "New chat" {
		t.Errorf("expected named tabs after first messages, got %q / %q", tabs[0].Title, tabs[1].Title)
	}

	tabs = eng.CloseWorkspaceChat(ws.ID, "PA2")
	if len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("after close PA2: %+v", tabs)
	}
	// Closed tab's history remains readable via session store / recents.
	if got := contentItems(eng.WorkspaceChatItems(ws.ID, "PA2")); len(got) < 2 {
		t.Fatalf("closed chat history should remain: %+v", got)
	}
	closed := eng.ListClosedWorkspaceChats(ws.ID)
	if len(closed) != 1 || closed[0].Code != "PA2" {
		t.Fatalf("ListClosedWorkspaceChats = %+v, want [PA2]", closed)
	}
	tabs = eng.ReopenWorkspaceChat(ws.ID, "PA2")
	if len(tabs) != 2 {
		t.Fatalf("after reopen PA2: %+v", tabs)
	}
	var reopened bool
	for _, tab := range tabs {
		if tab.Code == "PA2" {
			reopened = true
		}
	}
	if !reopened {
		t.Fatalf("reopened tabs missing PA2: %+v", tabs)
	}
	if got := contentItems(eng.WorkspaceChatItems(ws.ID, "PA2")); len(got) < 2 {
		t.Fatalf("reopened chat should keep history: %+v", got)
	}
	if leftover := eng.ListClosedWorkspaceChats(ws.ID); len(leftover) != 0 {
		t.Fatalf("reopened chat should leave recents, got %+v", leftover)
	}

	tabs = eng.CloseWorkspaceChat(ws.ID, "PA2")
	if len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("after close PA2 again: %+v", tabs)
	}

	// Closing the last tab resets to a fresh empty chat on a new code.
	tabs = eng.CloseWorkspaceChat(ws.ID, "PA")
	if len(tabs) != 1 || tabs[0].Title != "New chat" || tabs[0].Code == "PA" || tabs[0].Code == "PA2" {
		t.Fatalf("close last tab: %+v", tabs)
	}
	if got := contentItems(eng.WorkspaceChatItems(ws.ID, tabs[0].Code)); len(got) != 0 {
		t.Fatalf("fresh tab should be empty, got %+v", got)
	}
}

func TestDeleteClosedWorkspaceChat(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("del-%d", call), "Done.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "delete-closed", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "keep me", "sonnet", nil, nil, "", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	_ = eng.SendWorkspaceMessage(ws.ID, tabB.Code, "throw away", "sonnet", nil, nil, "", "")
	if tabs := eng.CloseWorkspaceChat(ws.ID, tabB.Code); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("close: %+v", tabs)
	}
	if closed := eng.ListClosedWorkspaceChats(ws.ID); len(closed) != 1 || closed[0].Code != tabB.Code {
		t.Fatalf("recents before delete: %+v", eng.ListClosedWorkspaceChats(ws.ID))
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.ChatTabOrder = []string{"PA", tabB.Code}
	})
	if openGone, err := eng.DeleteClosedWorkspaceChat(ws.ID, "PA"); err == nil {
		t.Fatal("delete open tab should fail")
	} else if len(openGone) != 1 || openGone[0].Code != tabB.Code {
		t.Fatalf("delete open tab remaining: %+v err=%v", openGone, err)
	}
	eng.loops.Store(ws.ID+loopMapSep+tabB.Code, &runningLoop{loop: &Loop{ID: ws.ID, ChatCode: tabB.Code}})
	if _, err := eng.DeleteClosedWorkspaceChat(ws.ID, tabB.Code); err == nil {
		t.Fatal("delete while loop running should fail")
	}
	eng.loops.Delete(ws.ID + loopMapSep + tabB.Code)
	eng.chats.Store(chatTurnKey(ws.ID, tabB.Code), &chatTurn{})
	if _, err := eng.DeleteClosedWorkspaceChat(ws.ID, tabB.Code); err == nil {
		t.Fatal("delete while live should fail")
	}
	eng.chats.Delete(chatTurnKey(ws.ID, tabB.Code))
	leftover, err := eng.DeleteClosedWorkspaceChat(ws.ID, tabB.Code)
	if err != nil || len(leftover) != 0 {
		t.Fatalf("delete closed: %+v err=%v", leftover, err)
	}
	if rec, _ := st.Workspace(ws.ID); strings.Contains(strings.Join(rec.ChatTabOrder, ","), tabB.Code) {
		t.Fatalf("ChatTabOrder still has deleted code: %v", rec.ChatTabOrder)
	}
	if got := contentItems(eng.WorkspaceChatItems(ws.ID, tabB.Code)); len(got) != 0 {
		t.Fatalf("deleted chat should be gone, got %+v", got)
	}
	reopened := eng.ReopenWorkspaceChat(ws.ID, tabB.Code)
	if len(reopened) != 1 || reopened[0].Code != "PA" {
		t.Fatalf("reopen after delete should not restore %s: %+v", tabB.Code, reopened)
	}
}

func TestDeleteClosedWorkspaceChatSerializesWithReopen(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("del-race-%d", call), "Done.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "delete-race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "keep me", "sonnet", nil, nil, "", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	_ = eng.SendWorkspaceMessage(ws.ID, tabB.Code, "throw away", "sonnet", nil, nil, "", "")
	if tabs := eng.CloseWorkspaceChat(ws.ID, tabB.Code); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("close: %+v", tabs)
	}

	var delErr error
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		_, delErr = eng.DeleteClosedWorkspaceChat(ws.ID, tabB.Code)
	}()
	go func() {
		defer wg.Done()
		_ = eng.ReopenWorkspaceChat(ws.ID, tabB.Code)
	}()
	wg.Wait()

	_, agentOpen := st.Agent(ws.ID, tabB.Code)
	_, sessOK := st.SessionForLoopAgent(ws.ID, tabB.Code)
	if agentOpen && !sessOK {
		t.Fatal("reopen left an open tab whose transcript was deleted")
	}
	if delErr == nil && agentOpen {
		t.Fatal("delete succeeded while the chat is still open")
	}
	if delErr != nil && !sessOK {
		t.Fatalf("delete failed and the session is gone: %v", delErr)
	}
}

func TestDeleteClosedWorkspaceChatInvalidatesScheduledModeContinue(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("del-cont-%d", call), "Done.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "delete-continue", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "keep me", "sonnet", nil, nil, "", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	_ = eng.SendWorkspaceMessage(ws.ID, tabB.Code, "throw away", "sonnet", nil, nil, "", "")
	if tabs := eng.CloseWorkspaceChat(ws.ID, tabB.Code); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("close: %+v", tabs)
	}
	before := len(runner.specsWithRole(domain.RoleCoder))
	turn := &chatTurn{cancel: func() {}}
	eng.noteModeContinue(ws.ID, tabB.Code, domain.ChatModeAgent, domain.ChatModePlan, turn)
	eng.maybeContinueAfterModeSwitch(ws.ID, tabB.Code, turn)
	if _, err := eng.DeleteClosedWorkspaceChat(ws.ID, tabB.Code); err != nil {
		t.Fatalf("delete: %v", err)
	}
	time.Sleep(200 * time.Millisecond)
	if _, ok := st.Agent(ws.ID, tabB.Code); ok {
		t.Fatal("scheduled remount reopened the deleted chat")
	}
	if _, ok := st.SessionForLoopAgent(ws.ID, tabB.Code); ok {
		t.Fatal("scheduled remount recreated the deleted session")
	}
	if n := len(runner.specsWithRole(domain.RoleCoder)); n != before {
		t.Fatalf("scheduled remount started a turn after delete, coder turns=%d want %d", n, before)
	}
}

func TestModeContinueEpochIsCapturedAtNoteTime(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("epoch-note-%d", call), "Done.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "epoch-note", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "keep me", "sonnet", nil, nil, "", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	_ = eng.SendWorkspaceMessage(ws.ID, tabB.Code, "throw away", "sonnet", nil, nil, "", "")
	if tabs := eng.CloseWorkspaceChat(ws.ID, tabB.Code); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("close: %+v", tabs)
	}
	before := len(runner.specsWithRole(domain.RoleCoder))
	turn := &chatTurn{cancel: func() {}}
	eng.noteModeContinue(ws.ID, tabB.Code, domain.ChatModeAgent, domain.ChatModePlan, turn)
	eng.invalidateContinueGens(ws.ID, tabB.Code)
	eng.maybeContinueAfterModeSwitch(ws.ID, tabB.Code, turn)
	time.Sleep(200 * time.Millisecond)
	if _, ok := st.Agent(ws.ID, tabB.Code); ok {
		t.Fatal("stale remount reopened the chat after a post-note epoch bump")
	}
	if n := len(runner.specsWithRole(domain.RoleCoder)); n != before {
		t.Fatalf("stale remount started a turn, coder turns=%d want %d", n, before)
	}
}

func TestRemoveWorkspaceInvalidatesScheduledModeContinue(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("rm-cont-%d", call), "Done.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "remove-continue", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "keep me", "sonnet", nil, nil, "", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	_ = eng.SendWorkspaceMessage(ws.ID, tabB.Code, "throw away", "sonnet", nil, nil, "", "")
	if tabs := eng.CloseWorkspaceChat(ws.ID, tabB.Code); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("close: %+v", tabs)
	}
	beforeItems := contentItems(eng.WorkspaceChatItems(ws.ID, tabB.Code))
	beforeTurns := len(runner.specsWithRole(domain.RoleCoder))
	turn := &chatTurn{cancel: func() {}}
	eng.noteModeContinue(ws.ID, tabB.Code, domain.ChatModeAgent, domain.ChatModePlan, turn)
	eng.maybeContinueAfterModeSwitch(ws.ID, tabB.Code, turn)
	if _, err := eng.RemoveWorkspace(ws.ID); err != nil {
		t.Fatalf("RemoveWorkspace: %v", err)
	}
	time.Sleep(200 * time.Millisecond)
	if _, ok := st.Agent(ws.ID, tabB.Code); ok {
		t.Fatal("scheduled remount reopened a chat on the archived workspace")
	}
	if n := len(runner.specsWithRole(domain.RoleCoder)); n != beforeTurns {
		t.Fatalf("scheduled remount started a turn after remove, coder turns=%d want %d", n, beforeTurns)
	}
	got := contentItems(eng.WorkspaceChatItems(ws.ID, tabB.Code))
	if len(got) != len(beforeItems) {
		t.Fatalf("archived transcript mutated by stale remount: before %d after %d", len(beforeItems), len(got))
	}
}

func TestRemoveWorkspaceWaitsForChatLifePrologue(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "life-wait", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	unlock, held := eng.lockChatLife(ws.ID, "PA")
	if !held {
		t.Fatal("lockChatLife refused a live workspace")
	}
	done := make(chan error, 1)
	go func() {
		_, err := eng.RemoveWorkspace(ws.ID)
		done <- err
	}()
	select {
	case err := <-done:
		t.Fatalf("RemoveWorkspace returned while prologue held the lifecycle lock: %v", err)
	case <-time.After(80 * time.Millisecond):
	}
	got, ok := st.Workspace(ws.ID)
	if !ok || got.Archived {
		t.Fatal("workspace archived while a chat prologue held the lifecycle lock")
	}
	unlock()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("RemoveWorkspace after prologue: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("RemoveWorkspace did not finish after prologue released")
	}
	got, ok = st.Workspace(ws.ID)
	if !ok || !got.Archived {
		t.Fatal("expected archive after prologue released")
	}
}

func TestPersistChatFailureSkipsCommittedTeardown(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fail-archive", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	before := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	eng.deleting.Store(ws.ID, struct{}{})
	eng.persistChatFailure(ws.ID, "PA", formatModeSwitchResumePrompt(domain.ChatModeAgent, domain.ChatModePlan),
		formatModeSwitchDisplay(domain.ChatModePlan), "I can't run right now: mcp bind failed", 0, Result{}, false, "")
	got := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if len(got) == len(before) {
		t.Fatal("transient deleting tombstone must not drop a failure note on a workspace that may remain")
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) { w.Archived = true })
	afterDelete := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	eng.persistChatFailure(ws.ID, "PA", formatModeSwitchResumePrompt(domain.ChatModeAgent, domain.ChatModeAsk),
		formatModeSwitchDisplay(domain.ChatModeAsk), "I can't find that workspace anymore — it may have been removed.", 0, Result{}, false, "")
	got = contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if len(got) != len(afterDelete) {
		t.Fatalf("dead-end note persisted on an archived workspace: before %d after %d", len(afterDelete), len(got))
	}
}

func TestDropChatLifeInvalidatesLiveContinueGen(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &captureRunner{})
	turn := &chatTurn{cancel: func() {}}
	eng.noteModeContinue("ws-epoch", "PA", domain.ChatModeAgent, domain.ChatModePlan, turn)
	var gen uint64
	eng.modeContinueMu.Lock()
	if p, ok := eng.modeContinues[modeContinueKey("ws-epoch", "PA")]; ok {
		gen = p.epoch
	}
	eng.modeContinueMu.Unlock()
	if gen == 0 || !eng.continueGenLive(gen) {
		t.Fatalf("noted remount gen %d should be live", gen)
	}
	eng.dropChatLifeForWorkspace("ws-epoch")
	if eng.continueGenLive(gen) {
		t.Fatal("teardown should reclaim the remount gen")
	}
	eng.continueGenMu.Lock()
	n := len(eng.liveContinues)
	eng.continueGenMu.Unlock()
	if n != 0 {
		t.Fatalf("liveContinues leaked %d entries after teardown", n)
	}
}

func TestLockChatLifeRetiresIdleSlots(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &captureRunner{})
	unlock, ok := eng.lockChatLife("ws-idle", "PA")
	if !ok {
		t.Fatal("lockChatLife refused an idle workspace")
	}
	eng.chatLifeMu.Lock()
	_, held := eng.chatLife[chatLifeKey("ws-idle", "PA")]
	eng.chatLifeMu.Unlock()
	if !held {
		t.Fatal("held slot missing from registry")
	}
	unlock()
	eng.chatLifeMu.Lock()
	_, idle := eng.chatLife[chatLifeKey("ws-idle", "PA")]
	eng.chatLifeMu.Unlock()
	if idle {
		t.Fatal("idle lifecycle lock was not retired")
	}
}

func TestLockChatLifeRefusesDeletingWorkspace(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &captureRunner{})
	eng.deleting.Store("ws-block", struct{}{})
	unlock, ok := eng.lockChatLife("ws-block", "PA")
	defer unlock()
	if ok {
		t.Fatal("lockChatLife should refuse a workspace marked deleting")
	}
	eng.chatLifeMu.Lock()
	_, held := eng.chatLife[chatLifeKey("ws-block", "PA")]
	eng.chatLifeMu.Unlock()
	if held {
		t.Fatal("refused lockChatLife left a lifecycle slot")
	}
}

func TestLockChatLifeRefusesHoistingWorkspace(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &captureRunner{})
	eng.hoisting.Store("ws-hoist", struct{}{})
	unlock, ok := eng.lockChatLife("ws-hoist", "PA")
	defer unlock()
	if ok {
		t.Fatal("lockChatLife should refuse a workspace marked hoisting")
	}
	eng.chatLifeMu.Lock()
	_, held := eng.chatLife[chatLifeKey("ws-hoist", "PA")]
	eng.chatLifeMu.Unlock()
	if held {
		t.Fatal("refused lockChatLife left a lifecycle slot")
	}
}

func TestDeleteClosedWorkspaceChatDropsOwnedSpecialists(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("del-own-%d", call), "Done.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "delete-owned", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "keep me", "sonnet", nil, nil, "", "")
	tabB := eng.CreateWorkspaceChat(ws.ID)
	_ = eng.SendWorkspaceMessage(ws.ID, tabB.Code, "conductor", "sonnet", nil, nil, "", "")
	co, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "coder", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"CO","name":"Coder","ownerCode":"` + tabB.Code + `"}`,
	})
	if err != nil {
		t.Fatalf("CO session: %v", err)
	}
	if _, err := st.AppendMessage(co.ID, domain.Message{
		Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("old coder turn")},
	}); err != nil {
		t.Fatalf("CO message: %v", err)
	}
	if tabs := eng.CloseWorkspaceChat(ws.ID, tabB.Code); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("close: %+v", tabs)
	}
	if leftover, err := eng.DeleteClosedWorkspaceChat(ws.ID, tabB.Code); err != nil || len(leftover) != 0 {
		t.Fatalf("delete: %+v err=%v", leftover, err)
	}
	if _, ok := st.SessionForChatAgent(ws.ID, tabB.Code, "CO"); ok {
		t.Fatal("deleted chat still owns a CO transcript")
	}
	fresh := eng.CreateWorkspaceChat(ws.ID)
	if _, ok := st.SessionForChatAgent(ws.ID, fresh.Code, "CO"); ok {
		t.Fatal("new chat inherited the deleted chat's CO session")
	}
}

// TestClosePairTabWhenLoopExists: closing the last pair tab on a run seeds a
// fresh empty chat — not the Maestro conductor, which would make the next
// Review & fix reopen the closed loop.
func TestClosePairTabWhenLoopExists(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-tabs", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID) // seeds PA
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = "PA"
	})

	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("engaged tabs = %+v, want [PA] (loop is a mode, not a singleton MA tab)", tabs)
	}

	tabs = eng.CloseWorkspaceChat(ws.ID, "PA")
	if len(tabs) != 1 || tabs[0].Code == "MA" {
		t.Fatalf("close last pair tab: %+v, want a pair tab, not MA", tabs)
	}
	if tabs[0].Title != "New chat" {
		t.Fatalf("fresh tab title = %q, want New chat", tabs[0].Title)
	}
	if rec, ok := st.Workspace(ws.ID); !ok || rec.LoopChatCode != "" {
		t.Fatalf("LoopChatCode after close = %q, want empty", rec.LoopChatCode)
	}
}

// TestCreateWorkspaceChatKeepsMaestroTab: a New Task loop (MA conductor) plus
// "+" must keep the Loop tab and append the new chat — not replace it.
func TestCreateWorkspaceChatKeepsMaestroTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "maestro-peer", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = "MA"
	})
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 1 || got[0] != "MA" {
		t.Fatalf("Maestro-only strip = %v, want [MA]", got)
	}
	created := eng.CreateWorkspaceChat(ws.ID)
	if created.Code == "" || created.Code == "MA" {
		t.Fatalf("CreateWorkspaceChat = %+v, want a pair tab", created)
	}
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 2 || got[0] != "MA" || got[1] != created.Code {
		t.Fatalf("after new tab = %v, want [MA %s]", got, created.Code)
	}
}

// TestListWorkspaceChatsLoopOnly: a run with no pair agents lists just Loop.
func TestListWorkspaceChatsLoopOnly(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-only", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = "MA"
	})
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Code != "MA" {
		t.Fatalf("ListWorkspaceChats = %+v, want [MA]", tabs)
	}
}

func TestCloseMaestroChatTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "close-maestro", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = "MA"
	})
	_ = eng.CreateWorkspaceChat(ws.ID)
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 2 || got[0] != "MA" {
		t.Fatalf("before close = %v, want [MA PA]", got)
	}

	tabs := eng.CloseWorkspaceChat(ws.ID, "MA")
	if got := tabCodes(tabs); len(got) != 1 || got[0] == "MA" {
		t.Fatalf("CloseWorkspaceChat(MA) = %v, want the pair tab only", got)
	}
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 1 || got[0] == "MA" {
		t.Fatalf("List after close = %v, want Maestro gone", got)
	}
	if rec, ok := st.Workspace(ws.ID); !ok || rec.LoopChatCode != "" {
		t.Fatalf("LoopChatCode after close MA = %q, want empty", rec.LoopChatCode)
	}
}

func TestCloseLastMaestroChatTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "close-last-maestro", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = "MA"
	})
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 1 || got[0] != "MA" {
		t.Fatalf("before close = %v, want [MA]", got)
	}

	tabs := eng.CloseWorkspaceChat(ws.ID, "MA")
	if got := tabCodes(tabs); len(got) != 1 || got[0] == "MA" {
		t.Fatalf("CloseWorkspaceChat last MA = %v, want a fresh pair tab", got)
	}
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 1 || got[0] == "MA" {
		t.Fatalf("List after close last MA = %v, want Maestro gone", got)
	}
}

func tabCodes(tabs []domain.WorkspaceChatTab) []string {
	out := make([]string, len(tabs))
	for i, t := range tabs {
		out[i] = t.Code
	}
	return out
}

func TestApplyChatTabOrder(t *testing.T) {
	pa := domain.WorkspaceChatTab{Code: "PA"}
	pa2 := domain.WorkspaceChatTab{Code: "PA2"}
	pa3 := domain.WorkspaceChatTab{Code: "PA3"}
	ma := domain.WorkspaceChatTab{Code: "MA"}
	def := []domain.WorkspaceChatTab{ma, pa, pa2, pa3}

	tests := []struct {
		name  string
		tabs  []domain.WorkspaceChatTab
		order []string
		want  []string
	}{
		{name: "empty order keeps default", tabs: def, order: nil, want: []string{"MA", "PA", "PA2", "PA3"}},
		{name: "single tab ignores order", tabs: []domain.WorkspaceChatTab{pa}, order: []string{"PA2", "PA"}, want: []string{"PA"}},
		{name: "saved permutation", tabs: def, order: []string{"PA2", "MA", "PA3", "PA"}, want: []string{"PA2", "MA", "PA3", "PA"}},
		{name: "closed codes skipped", tabs: []domain.WorkspaceChatTab{ma, pa, pa3}, order: []string{"PA2", "PA3", "MA", "PA"}, want: []string{"PA3", "MA", "PA"}},
		{name: "new pair tab appends", tabs: def, order: []string{"PA2", "MA", "PA"}, want: []string{"PA2", "MA", "PA", "PA3"}},
		{name: "new Loop still leads", tabs: def, order: []string{"PA2", "PA", "PA3"}, want: []string{"MA", "PA2", "PA", "PA3"}},
		{name: "duplicate order codes ignored", tabs: []domain.WorkspaceChatTab{pa, pa2}, order: []string{"PA2", "PA2", "PA"}, want: []string{"PA2", "PA"}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := tabCodes(applyChatTabOrder(tt.tabs, tt.order))
			if len(got) != len(tt.want) {
				t.Fatalf("applyChatTabOrder(%v, %v) = %v, want %v", tabCodes(tt.tabs), tt.order, got, tt.want)
			}
			for i := range got {
				if got[i] != tt.want[i] {
					t.Fatalf("applyChatTabOrder(%v, %v) = %v, want %v", tabCodes(tt.tabs), tt.order, got, tt.want)
				}
			}
		})
	}
}

// TestReorderWorkspaceChats: drag-drop order persists on the workspace and
// ListWorkspaceChats returns it; a new tab still lands at the end; a rejected
// permutation leaves the strip unchanged.
func TestReorderWorkspaceChats(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("ord-sess-%d", call), "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "reorder-chats", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	if tab := eng.CreateWorkspaceChat(ws.ID); tab.Code != "PA2" {
		t.Fatalf("CreateWorkspaceChat = %+v, want PA2", tab)
	}
	if tab := eng.CreateWorkspaceChat(ws.ID); tab.Code != "PA3" {
		t.Fatalf("CreateWorkspaceChat = %+v, want PA3", tab)
	}

	tabs, err := eng.ReorderWorkspaceChats(ws.ID, []string{"PA3", "PA", "PA2"})
	if err != nil {
		t.Fatalf("ReorderWorkspaceChats: %v", err)
	}
	if got := tabCodes(tabs); len(got) != 3 || got[0] != "PA3" || got[1] != "PA" || got[2] != "PA2" {
		t.Fatalf("ReorderWorkspaceChats = %v, want [PA3 PA PA2]", got)
	}
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 3 || got[0] != "PA3" || got[1] != "PA" || got[2] != "PA2" {
		t.Fatalf("ListWorkspaceChats after reorder = %v, want [PA3 PA PA2]", got)
	}

	created := eng.CreateWorkspaceChat(ws.ID)
	if created.Code != "PA4" {
		t.Fatalf("CreateWorkspaceChat after reorder = %+v, want PA4", created)
	}
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 4 || got[3] != "PA4" {
		t.Fatalf("ListWorkspaceChats after new tab = %v, want [… PA4]", got)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA3", "keep this tab", "sonnet", nil, nil, "", "")

	before := tabCodes(eng.ListWorkspaceChats(ws.ID))
	if _, err := eng.ReorderWorkspaceChats(ws.ID, []string{"PA", "PA2"}); err == nil {
		t.Fatal("ReorderWorkspaceChats incomplete permutation: want error")
	}
	if _, err := eng.ReorderWorkspaceChats(ws.ID, []string{"PA", "PA2", "PA3", "PA4", "PA9"}); err == nil {
		t.Fatal("ReorderWorkspaceChats unknown tab: want error")
	}
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != len(before) {
		t.Fatalf("ListWorkspaceChats after failed reorder = %v, want %v", got, before)
	} else {
		for i := range got {
			if got[i] != before[i] {
				t.Fatalf("ListWorkspaceChats after failed reorder = %v, want %v", got, before)
			}
		}
	}

	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = "MA"
	})
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); got[0] != "MA" {
		t.Fatalf("ListWorkspaceChats after Maestro conductor = %v, want MA first (peer, not a replacement)", got)
	}
	if _, err := eng.ReorderWorkspaceChats(ws.ID, []string{"PA", "MA", "PA3", "PA2", "PA4"}); err != nil {
		t.Fatalf("ReorderWorkspaceChats with Loop: %v", err)
	}
	if got := tabCodes(eng.ListWorkspaceChats(ws.ID)); len(got) != 5 || got[0] != "PA" || got[1] != "MA" {
		t.Fatalf("ListWorkspaceChats after moving Loop = %v, want [PA MA …]", got)
	}

	closed := eng.CloseWorkspaceChat(ws.ID, "PA3")
	if got := tabCodes(closed); len(got) != 4 || got[0] != "PA" || got[1] != "MA" {
		t.Fatalf("CloseWorkspaceChat = %v, want PA then MA still leading the rest", got)
	}
	reopened := eng.ReopenWorkspaceChat(ws.ID, "PA3")
	if got := tabCodes(reopened); len(got) != 5 || got[2] != "PA3" {
		t.Fatalf("ReopenWorkspaceChat = %v, want PA3 back in its saved slot", got)
	}

	if _, err := eng.ReorderWorkspaceChats("no-such-ws", []string{"PA"}); err == nil {
		t.Fatal("ReorderWorkspaceChats unknown workspace: want error")
	}
}

// TestWorkspaceChatModes: the dock's mode shapes the turn.
// Ask is read-only (scoped git + dontAsk pin); Agent restores edits + bare-Bash
// bypass from Settings. Mode persists per tab.
func TestWorkspaceChatModes(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("mode-sess-%d", call), "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	set := st.Settings()
	set.BypassPermissions = true
	st.SetSettings(set)

	ws, err := eng.CreateWorkspace(projID, "modes", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	lastCoder := func() AgentSpec {
		specs := runner.specsWithRole(domain.RoleCoder)
		if len(specs) == 0 {
			t.Fatal("no coder turn ran")
		}
		return specs[len(specs)-1]
	}
	has := func(spec AgentSpec, tool string) bool {
		return slices.Contains(spec.AllowedTools, tool)
	}

	// Ask mode is read-only through its tool list, not through a stricter
	// permission mode: permissions are machine-global, so every mode gets the
	// same one from Settings.
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "what does this repo do?", "sonnet", nil, nil, "ask", "")
	s := lastCoder()
	if has(s, "Edit") || has(s, "Write") || has(s, "Bash") || has(s, "NotebookEdit") {
		t.Errorf("ask mode must be read-only; tools = %v", s.AllowedTools)
	}
	if !has(s, "Read") || !has(s, "Bash(git diff:*)") {
		t.Errorf("ask mode should keep read/explore + git-read tools; tools = %v", s.AllowedTools)
	}
	if s.PermissionMode != "bypassPermissions" {
		t.Errorf("ask mode should follow Settings, got %q", s.PermissionMode)
	}
	if s.SystemPrompt != "" {
		t.Errorf("ask mode must keep the provider system prompt, got --system-prompt %q", s.SystemPrompt)
	}
	if s.AppendSystemPrompt == "" || !strings.Contains(s.AppendSystemPrompt, "ASK mode") {
		t.Errorf("ask mode should append an honest read-only note, got %q", s.AppendSystemPrompt)
	}
	if strings.Contains(s.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("ask mode should not inject simplify rails, got %q", s.AppendSystemPrompt)
	}
	if strings.Contains(s.AppendSystemPrompt, commentHygieneNeedle) {
		t.Errorf("ask mode should not inject comment hygiene, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "list_skills") {
		t.Errorf("ask mode should mention skills tools, got %q", s.AppendSystemPrompt)
	}
	if !has(s, mcpListSkills) || !has(s, mcpActivateSkill) {
		t.Errorf("ask mode needs skills MCP tools; tools = %v", s.AllowedTools)
	}
	if !has(s, mcpListTerminals) || !has(s, mcpReadTerminal) {
		t.Errorf("ask mode needs terminal MCP tools; tools = %v", s.AllowedTools)
	}
	if !has(s, mcpRenameSession) || !has(s, mcpRenameWorkspace) {
		t.Errorf("ask mode needs rename session/workspace MCP tools; tools = %v", s.AllowedTools)
	}
	if has(s, mcpRenameBranch) {
		t.Errorf("ask mode must not get rename_branch; tools = %v", s.AllowedTools)
	}
	if has(s, mcpOpenPR) {
		t.Errorf("ask mode must not get open_pr; tools = %v", s.AllowedTools)
	}
	if has(s, mcpCreateWorkspace) {
		t.Errorf("ask mode must not get create_workspace; tools = %v", s.AllowedTools)
	}
	if has(s, mcpMessageChild) {
		t.Errorf("ask mode must not get message_child; tools = %v", s.AllowedTools)
	}
	if !has(s, mcpCreateArtifact) || !has(s, mcpListArtifacts) || !has(s, mcpReadArtifact) || !has(s, mcpPresentArtifact) {
		t.Errorf("ask mode should keep artifact MCP tools; tools = %v", s.AllowedTools)
	}
	if !strings.Contains(s.AppendSystemPrompt, "create_artifact") {
		t.Errorf("ask mode should tell the model artifacts are allowed, got %q", s.AppendSystemPrompt)
	}
	// Ask needs MCP for skills and for permission_request when bypass is off.
	if s.MCPURL == "" {
		t.Error("ask mode should wire Recur MCP for skills and permission prompts")
	}

	// Mode persists on the tab and is reported to the dock.
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != "ask" {
		t.Errorf("persisted mode = %q, want ask", mode)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Mode != "ask" {
		t.Errorf("tab mode = %+v, want ask", tabs)
	}

	// Empty mode on the next send reuses the persisted one.
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "and the tests?", "sonnet", nil, nil, "", "")
	if s := lastCoder(); has(s, "Edit") {
		t.Errorf("persisted ask mode not applied: tools=%v", s.AllowedTools)
	}

	// Switching the dock to agent restores edit tools + findings MCP + Settings bypass.
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "now fix it", "sonnet", nil, nil, "agent", "")
	s = lastCoder()
	if !has(s, "Edit") || !has(s, "Write") {
		t.Errorf("agent mode must restore edit tools; tools = %v", s.AllowedTools)
	}
	if !has(s, mcpListFindings) || !has(s, mcpUpdateFinding) {
		t.Errorf("agent mode needs findings checklist MCP tools; tools = %v", s.AllowedTools)
	}
	if has(s, mcpDone) {
		t.Errorf("agent mode must not get done (loop specialist handback); tools = %v", s.AllowedTools)
	}
	if !has(s, mcpListSkills) || !has(s, mcpActivateSkill) {
		t.Errorf("agent mode needs skills MCP tools; tools = %v", s.AllowedTools)
	}
	if !has(s, mcpListTerminals) || !has(s, mcpReadTerminal) {
		t.Errorf("agent mode needs terminal MCP tools; tools = %v", s.AllowedTools)
	}
	if !has(s, mcpRenameSession) || !has(s, mcpRenameWorkspace) || !has(s, mcpRenameBranch) {
		t.Errorf("agent mode needs rename MCP tools; tools = %v", s.AllowedTools)
	}
	if !has(s, mcpOpenPR) {
		t.Errorf("agent mode needs open_pr MCP tool; tools = %v", s.AllowedTools)
	}
	if s.MCPURL == "" {
		t.Error("agent mode should wire the per-agent Recur MCP endpoint")
	}
	if s.PermissionMode != "bypassPermissions" {
		t.Errorf("agent mode with bare Bash + Bypass on must bypassPermissions, got %q", s.PermissionMode)
	}
	// Findings checklist must not be an always-on append (models treat it as a
	// startup ritual). A short skills tip is fine — load-on-demand tools.
	if strings.Contains(s.AppendSystemPrompt, "list_findings") ||
		strings.Contains(s.AppendSystemPrompt, "update_finding") {
		t.Errorf("agent mode must not append a findings checklist note, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "list_skills") {
		t.Errorf("agent mode should mention skills tools, got %q", s.AppendSystemPrompt)
	}
	// Agent must lead with an identity note — Ask/Plan/Review already do, and
	// without it models misread mode-literacy's dock-flip clause as "I'm read-only".
	if !strings.Contains(s.AppendSystemPrompt, "AGENT mode") {
		t.Errorf("agent mode should append an AGENT identity note, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "Do not ask the user to switch to Agent") {
		t.Errorf("agent mode should tell the model not to request a dock flip, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "beyond a very simple task") {
		t.Errorf("agent mode should strongly prefer child workspaces, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "Prefer acting on reasonable defaults") {
		t.Errorf("mode literacy should restrain eager ask_user, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "Never use Claude's AskUserQuestion") {
		t.Errorf("mode literacy should steer away from AskUserQuestion, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "remounts this turn immediately") {
		t.Errorf("mode literacy should say switch_mode remounts immediately, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "Do not use Cursor's built-in SwitchMode") {
		t.Errorf("mode literacy should capitalize Do not use Cursor SwitchMode, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("agent mode should inject simplify by default, got %q", s.AppendSystemPrompt)
	}
	if !strings.Contains(s.AppendSystemPrompt, commentHygieneNeedle) {
		t.Errorf("agent mode should inject comment hygiene, got %q", s.AppendSystemPrompt)
	}
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != "agent" {
		t.Errorf("persisted mode after switch = %q, want agent", mode)
	}
}

// TestSendPersistsModeBeforeTurnRuns: mode, model, and flags must be in session
// config_json while the turn is still running — not only after runAgent returns —
// so a mid-turn reload (or DB inspection) sees the dock selection correctly.
func TestSendPersistsModeBeforeTurnRuns(t *testing.T) {
	var eng *Engine
	var wsID string
	runner := &captureRunner{}
	runner.linesFor = func(call int) [][]byte {
		if spec, ok := runner.specAt(call); ok && spec.Role == domain.RoleCoder {
			model, flags, mode := eng.WorkspaceChatSelection(wsID, "PA2")
			if mode != "review" {
				t.Errorf("mode during turn = %q, want review", mode)
			}
			if model != "sonnet" {
				t.Errorf("model during turn = %q, want sonnet", model)
			}
			if flags["effort"] != "high" {
				t.Errorf("flags during turn = %#v, want effort=high", flags)
			}
		}
		return maestroFixture(fmt.Sprintf("mid-%d", call), "OK.")
	}
	e, st, _, _ := newTestEngine(t, runner)
	eng = e
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "mid-turn mode", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	wsID = ws.ID
	tab := eng.CreateWorkspaceChat(ws.ID)
	if tab.Code != "PA2" {
		t.Fatalf("CreateWorkspaceChat code = %q, want PA2", tab.Code)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "review please", "sonnet", map[string]string{"effort": "high"}, nil, "review", "")
	model, flags, mode := eng.WorkspaceChatSelection(ws.ID, "PA2")
	if mode != "review" || model != "sonnet" || flags["effort"] != "high" {
		t.Errorf("after turn: model=%q flags=%#v mode=%q", model, flags, mode)
	}
}

func TestRememberWorkspaceChatPromptGroup(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("grp-%d", call), "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "prompt group", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, "agent", nil)
	eng.RememberWorkspaceChatPromptGroup(ws.ID, "PA", "ops")
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].PromptGroup != "ops" {
		t.Errorf("ListWorkspaceChats PromptGroup = %+v, want ops", tabs)
	}
}

// A new chat has an Agent row but no session until the first dock Remember.
// Group stamp must create that session so it is not lost when it races selection.
func TestRememberWorkspaceChatPromptGroupWithoutSelection(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("grp-ns-%d", call), "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "prompt group no selection", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	if _, ok := st.SessionForLoopAgent(ws.ID, "PA"); ok {
		t.Fatal("SessionForLoopAgent(PA) existed before RememberWorkspaceChatPromptGroup")
	}
	eng.RememberWorkspaceChatPromptGroup(ws.ID, "PA", "ops")
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].PromptGroup != "ops" {
		t.Errorf("RememberWorkspaceChatPromptGroup without selection PromptGroup = %+v, want ops", tabs)
	}
}

func pairAgentSessions(t *testing.T, st *store.Store, workspaceID, code string) []domain.Session {
	t.Helper()
	var out []domain.Session
	for _, sess := range st.SessionsForLoop(workspaceID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
		if cfg.Code == code {
			out = append(out, sess)
		}
	}
	return out
}

// Concurrent selection + group Remember must share one session so neither
// promptGroup nor the dock pick is written onto an orphaned duplicate row.
func TestRememberWorkspaceChatPromptGroupConcurrentWithSelection(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("grp-race-%d", call), "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "prompt group race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, "agent", nil)
	}()
	go func() {
		defer wg.Done()
		eng.RememberWorkspaceChatPromptGroup(ws.ID, "PA", "ops")
	}()
	wg.Wait()
	got := pairAgentSessions(t, st, ws.ID, "PA")
	if len(got) != 1 {
		t.Errorf("pairAgentSessions(PA) = %d, want 1", len(got))
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].PromptGroup != "ops" {
		t.Errorf("ListWorkspaceChats PromptGroup = %+v, want ops", tabs)
	}
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != "agent" {
		t.Errorf("WorkspaceChatSelection mode = %q, want agent", mode)
	}
}

// TestRememberWorkspaceChatSelectionWithoutSend: switching the dock to Review
// (or any mode) must persist before the first turn — otherwise a reload falls
// back to agent and a quick send can overwrite the intended mode.
func TestRememberWorkspaceChatSelectionWithoutSend(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("mem-%d", call), "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "remember mode", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	// Ensure the PA agent row exists (ListWorkspaceChats does this for the UI).
	_ = eng.ListWorkspaceChats(ws.ID)

	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", map[string]string{"effort": "high", "thinking": "on"}, "review", nil)
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != "review" {
		t.Errorf("WorkspaceChatSelection mode = %q, want review", mode)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Mode != "review" {
		t.Errorf("ListWorkspaceChats mode = %+v, want review", tabs)
	}
	model, flags, _ := eng.WorkspaceChatSelection(ws.ID, "PA")
	if model != "sonnet" {
		t.Errorf("persisted model = %q, want sonnet", model)
	}
	if flags["effort"] != "high" || flags["thinking"] != "on" {
		t.Errorf("persisted flags = %#v, want effort=high thinking=on", flags)
	}

	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", map[string]string{"effort": "low"}, "review", nil)
	_, flags, _ = eng.WorkspaceChatSelection(ws.ID, "PA")
	if flags["effort"] != "low" {
		t.Errorf("updated flags = %#v, want effort=low", flags)
	}
	if _, ok := flags["thinking"]; ok {
		t.Errorf("stale flag retained: %#v", flags)
	}

	// Mode-only flip creates a durable session but must not appear in Activity
	// until there is a transcript.
	if views := eng.WorkspaceSessions(ws.ID, "PA", ""); len(views) != 0 {
		t.Errorf("WorkspaceSessions = %d, want 0 empty (mode-only) sessions", len(views))
	}

	// Loop persists on a coding tab like every other mode.
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, "loop", nil)
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != "loop" {
		t.Errorf("loop remember mode = %q, want loop", mode)
	}

	// Non-pair codes coerce to PA — must not invent an RV agent/session.
	eng.RememberWorkspaceChatSelection(ws.ID, "RV", "opus", nil, "ask", nil)
	if _, ok := st.Agent(ws.ID, "RV"); ok {
		t.Error("Remember with RV must not create an RV agent")
	}
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != "ask" {
		t.Errorf("Remember RV should coerce to PA; mode = %q, want ask", mode)
	}

	// Missing workspace / closed tab must not resurrect rows.
	eng.RememberWorkspaceChatSelection("no-such-ws", "PA", "sonnet", nil, "agent", nil)
	if _, ok := st.Agent("no-such-ws", "PA"); ok {
		t.Error("Remember on missing workspace must not create an agent")
	}
	tab2 := eng.CreateWorkspaceChat(ws.ID)
	eng.RememberWorkspaceChatSelection(ws.ID, tab2.Code, "sonnet", nil, "plan", nil)
	_ = eng.CloseWorkspaceChat(ws.ID, tab2.Code)
	if _, ok := st.Agent(ws.ID, tab2.Code); ok {
		t.Fatalf("agent %s still present after close", tab2.Code)
	}
	eng.RememberWorkspaceChatSelection(ws.ID, tab2.Code, "sonnet", nil, "review", nil)
	if _, ok := st.Agent(ws.ID, tab2.Code); ok {
		t.Errorf("Remember after CloseWorkspaceChat resurrected agent %s", tab2.Code)
	}
}

func TestHumanDockModeChangeRefreshesStanding(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("dock-%d", call), "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "dock mode change", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "edit it", "sonnet", nil, nil, domain.ChatModeAgent, "")
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, domain.ChatModeReview, nil)
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "what changed?", "sonnet", nil, nil, domain.ChatModeReview, "")
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) < 2 {
		t.Fatalf("specs = %d, want 2", len(specs))
	}
	first, second := specs[0], specs[len(specs)-1]
	if first.RefreshStandingInstructions {
		t.Error("first turn RefreshStandingInstructions = true, want false")
	}
	if !second.RefreshStandingInstructions {
		t.Error("human dock mode change must RefreshStandingInstructions")
	}
	if !strings.Contains(second.AppendSystemPrompt, "REVIEW mode") {
		t.Errorf("review follow-up append = %q, want REVIEW mode notes", second.AppendSystemPrompt)
	}
}

type failCallRunner struct {
	failAt int
	*captureRunner
}

func (r *failCallRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	call := len(r.specs)
	r.mu.Unlock()
	if call == r.failAt {
		r.mu.Lock()
		r.specs = append(r.specs, spec)
		r.mu.Unlock()
		return Result{}, fmt.Errorf("provider unavailable")
	}
	return r.captureRunner.Run(ctx, spec, emit)
}

func TestFailedTurnDoesNotStampStandingMode(t *testing.T) {
	inner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("failstand-%d", call), "OK.") },
	}
	runner := &failCallRunner{failAt: 1, captureRunner: inner}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "failed standing", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "edit it", "sonnet", nil, nil, domain.ChatModeAgent, "")
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, domain.ChatModeReview, nil)
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "what changed?", "sonnet", nil, nil, domain.ChatModeReview, "")
	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("missing PA session")
	}
	var cfg agentConfig
	if err := json.Unmarshal([]byte(sess.ConfigJSON), &cfg); err != nil {
		t.Fatalf("config: %v", err)
	}
	if cfg.StandingMode != domain.ChatModeAgent {
		t.Errorf("StandingMode after failed review turn = %q, want %q", cfg.StandingMode, domain.ChatModeAgent)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "retry review", "sonnet", nil, nil, domain.ChatModeReview, "")
	specs := inner.specsWithRole(domain.RoleCoder)
	if len(specs) < 3 {
		t.Fatalf("specs = %d, want 3", len(specs))
	}
	retry := specs[len(specs)-1]
	if !retry.RefreshStandingInstructions {
		t.Error("retry after failed mode change must RefreshStandingInstructions")
	}
}

func TestLegacyStandingModeRefreshesAskToAgent(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("legacy-%d", call), "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "legacy standing", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "explain this", "sonnet", nil, nil, domain.ChatModeAsk, "")
	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("missing PA session")
	}
	var cfg agentConfig
	if err := json.Unmarshal([]byte(sess.ConfigJSON), &cfg); err != nil {
		t.Fatalf("config: %v", err)
	}
	cfg.StandingMode = ""
	raw, err := json.Marshal(cfg)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if err := st.UpdateSessionConfigJSON(sess.ID, string(raw)); err != nil {
		t.Fatalf("UpdateSessionConfigJSON: %v", err)
	}
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, domain.ChatModeAgent, nil)
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "now edit it", "sonnet", nil, nil, domain.ChatModeAgent, "")
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) < 2 {
		t.Fatalf("specs = %d, want 2", len(specs))
	}
	second := specs[len(specs)-1]
	if !second.RefreshStandingInstructions {
		t.Error("legacy Ask session switching to Agent must RefreshStandingInstructions")
	}
}

// TestRememberWorkspaceChatSelectionStampsProvider: a Cursor dock pick must
// persist session.Provider=cursor (not the old create-time claude-code default).
// Switching to Claude updates the row and drops a Cursor agent- resume id.
func TestRememberWorkspaceChatSelectionStampsProvider(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("prov-%d", call), "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "remember provider", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)

	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "cursor/composer-2.5", nil, "agent", nil)
	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("RememberWorkspaceChatSelection did not create a session")
	}
	if sess.Provider != domain.ProviderCursor {
		t.Errorf("RememberWorkspaceChatSelection(cursor/composer-2.5) session.Provider = %q, want %q",
			sess.Provider, domain.ProviderCursor)
	}

	const cursorAgentID = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b"
	eng.setAgentSession(ws.ID, "PA", cursorAgentID)
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "", nil, "ask", nil)
	if a, ok := st.Agent(ws.ID, "PA"); !ok || a.SessionID != cursorAgentID {
		t.Errorf("mode-only RememberWorkspaceChatSelection SessionID = %q, want %q", a.SessionID, cursorAgentID)
	}
	sess, _ = st.SessionForLoopAgent(ws.ID, "PA")
	if sess.Provider != domain.ProviderCursor {
		t.Errorf("mode-only RememberWorkspaceChatSelection session.Provider = %q, want %q",
			sess.Provider, domain.ProviderCursor)
	}

	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, "agent", nil)
	sess, _ = st.SessionForLoopAgent(ws.ID, "PA")
	if sess.Provider != domain.ProviderClaudeCode {
		t.Errorf("RememberWorkspaceChatSelection(sonnet) session.Provider = %q, want %q",
			sess.Provider, domain.ProviderClaudeCode)
	}
	if a, ok := st.Agent(ws.ID, "PA"); !ok || a.SessionID != cursorAgentID {
		t.Errorf("RememberWorkspaceChatSelection must not drop a resume id (AutoResume needs it); SessionID = %q", a.SessionID)
	}
}

// TestReviewModeChatFilesFindings: Review mode
// is Ask's read-only scoping plus structured output — the turn gets the
// per-agent Recur MCP with report_finding allowlisted, and a filed finding
// persists on the workspace (no run required) and announces findings.updated.
func TestReviewModeChatFilesFindings(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("rev-%d", call), "Reviewed.") },
	}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "review me", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "review my diff", "sonnet", nil, nil, "review", "")
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) == 0 {
		t.Fatal("no review turn ran")
	}
	s := specs[len(specs)-1]
	hasBash, hasFindingTool, hasListFindings, hasUpdateFinding := false, false, false, false
	for _, tl := range s.AllowedTools {
		if tl == "Edit" || tl == "Write" || tl == "NotebookEdit" {
			t.Errorf("review mode must not allow edits; tools = %v", s.AllowedTools)
		}
		if tl == "Bash" {
			hasBash = true
		}
		if tl == "mcp__recur__report_finding" {
			hasFindingTool = true
		}
		if tl == mcpListFindings {
			hasListFindings = true
		}
		if tl == mcpUpdateFinding {
			hasUpdateFinding = true
		}
		if tl == mcpRenameBranch {
			t.Errorf("review mode must not get rename_branch; tools = %v", s.AllowedTools)
		}
		if tl == mcpOpenPR {
			t.Errorf("review mode must not get open_pr; tools = %v", s.AllowedTools)
		}
	}
	if !hasBash {
		t.Errorf("review mode needs Bash for lint/test, got %v", s.AllowedTools)
	}
	if !hasFindingTool {
		t.Errorf("review mode needs report_finding, got %v", s.AllowedTools)
	}
	if !hasListFindings {
		t.Errorf("review mode needs list_findings (read-only), got %v", s.AllowedTools)
	}
	if !hasUpdateFinding {
		t.Errorf("review mode needs update_finding to close prior rows, got %v", s.AllowedTools)
	}
	if s.MCPURL == "" {
		t.Error("review mode should wire the per-agent Recur MCP endpoint")
	}
	// Machine-global (D63): the mode follows the setting, not the mode's tools.
	if want := st.Settings().PermissionMode(); s.PermissionMode != want {
		t.Errorf("review mode permission mode %q, want the global %q", s.PermissionMode, want)
	}
	if !strings.Contains(s.AppendSystemPrompt, "REVIEW mode") {
		t.Errorf("review mode should append its role note, got %q", s.AppendSystemPrompt)
	}
	if strings.Contains(s.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("review mode should not inject simplify rails, got %q", s.AppendSystemPrompt)
	}
	if strings.Contains(s.AppendSystemPrompt, commentHygieneNeedle) {
		t.Errorf("review mode should not inject comment hygiene, got %q", s.AppendSystemPrompt)
	}

	// A filed finding persists on the run-less workspace via the same reporter
	// seam the live MCP handler drives, and the findings panel is notified.
	rep := engineReporter{eng: eng}
	rep.ReportFinding(ws.ID, "PA", mcp.Finding{File: "main.go", Line: 3, Severity: domain.SevHigh, Message: "nil deref"})
	fs := st.Findings(ws.ID)
	if len(fs) != 1 || fs[0].File != "main.go" || fs[0].Status != "open" || fs[0].Iteration != 1 {
		t.Fatalf("finding not persisted as expected: %+v", fs)
	}
	saw := false
	for _, ev := range sink.byName("findings.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == ws.ID {
			saw = true
		}
	}
	if !saw {
		t.Error("expected findings.updated on the bus")
	}
}

func TestRemapChatsOffDisabledProviders(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	ws, err := st.AddWorkspace(domain.Workspace{ID: "WS-remap", Goal: "remap"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", Model: "cursor/composer-2.5",
	})
	set := st.Settings()
	set.DisabledProviders = []string{domain.ProviderCursor}
	set.CursorAPIKey = "sk-x"
	set = set.NormalizeProviders()
	eng.RemapChatsOffDisabledProviders(set)
	a, ok := st.Agent(ws.ID, "PA")
	if !ok {
		t.Fatal("PA agent missing")
	}
	if a.Model != "claude-code/claude-sonnet-5-5" {
		t.Errorf("RemapChatsOffDisabledProviders() PA.Model = %q, want claude-code/claude-sonnet-5-5", a.Model)
	}
}

func TestRemapChatsOffDisabledProvidersKeepsSimplify(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	ws, err := st.AddWorkspace(domain.Workspace{ID: "WS-remap-simplify", Goal: "remap"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", Model: "cursor/composer-2.5",
	})
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), "PA", "Agent", "")
	if !ok {
		t.Fatal("ensureAgentSession")
	}
	if err := st.PatchSessionConfigMerge(sess.ID, `{"model":"cursor/composer-2.5"}`); err != nil {
		t.Fatalf("PatchSessionConfigMerge: %v", err)
	}
	if err := st.PatchSessionConfigSimplify(sess.ID, false); err != nil {
		t.Fatalf("PatchSessionConfigSimplify: %v", err)
	}
	set := st.Settings()
	set.DisabledProviders = []string{domain.ProviderCursor}
	set.CursorAPIKey = "sk-x"
	set = set.NormalizeProviders()
	eng.RemapChatsOffDisabledProviders(set)
	got, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("session missing after remap")
	}
	if !strings.Contains(got.ConfigJSON, `"simplify":false`) {
		t.Errorf("simplify lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"model":"claude-code/claude-sonnet-5-5"`) {
		t.Errorf("model not remapped: %s", got.ConfigJSON)
	}
}

func TestRemapChatsOffDisabledProvidersUsesSettingsFallback(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	ws, err := st.AddWorkspace(domain.Workspace{ID: "WS-remap-claude-off", Goal: "remap"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", Model: "claude-code/sonnet",
	})
	set := st.Settings()
	set.DisabledProviders = []string{domain.ProviderClaudeCode}
	set.CursorAPIKey = "sk-x"
	set.ModelAgent = "cursor/composer-2.5"
	eng.RemapChatsOffDisabledProviders(set)
	a, ok := st.Agent(ws.ID, "PA")
	if !ok {
		t.Fatal("PA agent missing")
	}
	if a.Model != "cursor/composer-2.5" {
		t.Errorf("RemapChatsOffDisabledProviders() PA.Model = %q, want cursor/composer-2.5", a.Model)
	}
}

func TestRemapChatsOffDisabledProvidersLeavesEmptyPick(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	ws, err := st.AddWorkspace(domain.Workspace{ID: "WS-remap-empty", Goal: "remap"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", Model: "",
	})
	set := st.Settings()
	set.DisabledProviders = []string{domain.ProviderClaudeCode}
	set.CursorAPIKey = "sk-x"
	set.ModelAgent = "cursor/composer-2.5"
	eng.RemapChatsOffDisabledProviders(set)
	a, ok := st.Agent(ws.ID, "PA")
	if !ok {
		t.Fatal("PA agent missing")
	}
	if a.Model != "" {
		t.Errorf("RemapChatsOffDisabledProviders() PA.Model = %q, want empty (follows the mode default)", a.Model)
	}
}

func TestRemapChatsOffRemovedCompatibleProvider(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	ws, err := st.AddWorkspace(domain.Workspace{ID: "WS-remap-removed", Goal: "remap"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", Model: "alpha/m1",
	})
	set := st.Settings()
	set.DisabledProviders = []string{domain.ProviderClaudeCode}
	set.CompatibleProviders = []domain.CompatibleProvider{{
		ID: "beta", BaseURL: "https://b.example",
		Models: []domain.CompatibleModel{{ID: "m2"}},
	}}
	set.ModelAgent = "beta/m2"
	eng.RemapChatsOffDisabledProviders(set)
	a, ok := st.Agent(ws.ID, "PA")
	if !ok {
		t.Fatal("PA agent missing")
	}
	if a.Model != "beta/m2" {
		t.Errorf("RemapChatsOffDisabledProviders() PA.Model = %q, want beta/m2", a.Model)
	}
}

// TestSendPersistsProvisionalChatTitle: listing tabs after the first send must
// read config_json.title, not flatten the transcript.
func TestSendPersistsProvisionalChatTitle(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("title-sess", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "title-tab", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "add a dark-mode toggle", "sonnet", nil, nil, "", "")
	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("PA session missing")
	}
	if !strings.Contains(sess.ConfigJSON, `"title":"add a dark-mode toggle"`) {
		t.Fatalf("provisional title missing: %s", sess.ConfigJSON)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "add a dark-mode toggle" {
		t.Fatalf("ListWorkspaceChats title = %+v", tabs)
	}
}

// TestChatTabTitleFromFirstPromptWithoutStoredTitle: older sessions never
// wrote config_json.title — listing must still name the tab from the first
// user line without SessionTranscript.
func TestChatTabTitleFromFirstPromptWithoutStoredTitle(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "legacy-title", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "coder", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA","name":"Agent"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("review the flaky suite")},
	}); err != nil {
		t.Fatalf("AppendMessage: %v", err)
	}
	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.ToolResultBlock("tu-1", strings.Repeat("diff ", 4000), false)},
	}); err != nil {
		t.Fatalf("AppendMessage tool_result: %v", err)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "review the flaky suite" {
		t.Fatalf("legacy title = %+v", tabs)
	}
}

// TestSendDoesNotRenameLegacyUntitledTab: older sessions have an empty
// config title. A follow-up must not stamp that send as the permanent label.
func TestSendDoesNotRenameLegacyUntitledTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("legacy-follow", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "legacy-follow", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "coder", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA","name":"Agent"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("review the flaky suite")},
	}); err != nil {
		t.Fatalf("AppendMessage: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "and also fix the flake", "sonnet", nil, nil, "", "")
	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("PA session missing after send")
	}
	if strings.Contains(sess.ConfigJSON, "and also fix the flake") {
		t.Fatalf("follow-up stamped as title: %s", sess.ConfigJSON)
	}
	tabs := eng.ListWorkspaceChats(ws.ID)
	if len(tabs) != 1 || tabs[0].Title != "review the flaky suite" {
		t.Fatalf("legacy follow-up title = %+v config=%s", tabs, sess.ConfigJSON)
	}
}

// TestChatTabTitleCapsStoredAndDerivedAlike: persist the first line, then
// display through one byte-cap so CJK / long Latin match the derived path.
func TestChatTabTitleCapsStoredAndDerivedAlike(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("cjk-title", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cjk-title", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	first := strings.Repeat("漢", 20)
	want := truncate(first, chatTabTitleLimit)
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", first, "sonnet", nil, nil, "", "")
	stored := eng.ListWorkspaceChats(ws.ID)
	if len(stored) != 1 || stored[0].Title != want {
		t.Fatalf("stored title = %+v want %q", stored, want)
	}

	ws2, err := eng.CreateWorkspace(projID, "cjk-derived", "")
	if err != nil {
		t.Fatalf("CreateWorkspace 2: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws2.ID)
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws2.ID, Kind: "coder", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA","name":"Agent"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock(first)},
	}); err != nil {
		t.Fatalf("AppendMessage: %v", err)
	}
	derived := eng.ListWorkspaceChats(ws2.ID)
	if len(derived) != 1 || derived[0].Title != want {
		t.Fatalf("derived title = %+v want %q", derived, want)
	}
}

// TestWorkspaceSessionsSkipsPairAndMaestro: the Loop feed only needs specialist
// transcripts. Pair/MA rows used to be flattened on every workspace switch and
// then dropped by the UI.
func TestWorkspaceSessionsSkipsPairAndMaestro(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("pair-sess", "done") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "sessions-skip", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "do the pair work", "sonnet", nil, nil, "", "")

	ma, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "maestro", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"MA","name":"Maestro"}`,
	})
	if err != nil {
		t.Fatalf("MA session: %v", err)
	}
	if _, err := st.AppendMessage(ma.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("steer the loop")},
	}); err != nil {
		t.Fatalf("MA message: %v", err)
	}

	rv, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "reviewer", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"RV","name":"Reviewer"}`,
	})
	if err != nil {
		t.Fatalf("RV session: %v", err)
	}
	if _, err := st.AppendMessage(rv.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("looks good")},
	}); err != nil {
		t.Fatalf("RV message: %v", err)
	}

	views := eng.WorkspaceSessions(ws.ID, "MA", "")
	if len(views) != 1 || views[0].Code != "RV" {
		t.Fatalf("WorkspaceSessions = %+v, want [RV]", views)
	}
}

// TestSendWorkspaceMessageSteersLiveTurn: a second SendWorkspaceMessage while
// the first Run is in flight must inject onto that turn, not queue a second Run.
func TestSendWorkspaceMessageSteersLiveTurn(t *testing.T) {
	for _, model := range []string{"cursor/composer-2.5", "claude-code/sonnet", "codex/gpt-5.4"} {
		t.Run(model, func(t *testing.T) {
			started := make(chan struct{})
			release := make(chan struct{})
			runner := &holdInjectRunner{started: started, release: release}
			eng, st, _, _ := newTestEngine(t, runner)
			projID := addProject(t, st, initRepo(t))
			ws, err := eng.CreateWorkspace(projID, "steer "+model, "")
			if err != nil {
				t.Fatalf("CreateWorkspace: %v", err)
			}

			firstDone := make(chan string, 1)
			go func() {
				firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", model, nil, nil, "", "")
			}()
			select {
			case <-started:
			case <-time.After(3 * time.Second):
				t.Fatal("first turn did not start")
			}

			got := eng.SendWorkspaceMessage(ws.ID, "PA", "please stop and fix tests", model, nil, nil, "", "")
			if got != chatInjectedSentinel {
				t.Errorf("SendWorkspaceMessage(follow-up) = %q, want injected sentinel %q", got, chatInjectedSentinel)
			}

			close(release)
			select {
			case reply := <-firstDone:
				if reply == chatInjectedSentinel {
					t.Errorf("first SendWorkspaceMessage = %q, want the original turn reply (not inject sentinel)", reply)
				}
			case <-time.After(3 * time.Second):
				t.Fatal("first turn did not finish after release")
			}

			if n := runner.runCount(); n != 1 {
				t.Errorf("Run called %d times, want 1 (follow-up must inject, not start a second Run)", n)
			}
			spec, ok := runner.lastSpec()
			if !ok {
				t.Fatal("missing chat spec")
			}
			if spec.Inject == nil {
				t.Error("workspace chat spec.Inject = nil, want armed follow-up channel")
			}
			if !spec.InjectPlain {
				t.Error("workspace chat spec.InjectPlain = false, want true (no Maestro prefix)")
			}
			injected := runner.allInjected()
			if len(injected) != 1 {
				t.Fatalf("injected = %v, want one payload", injected)
			}
			p, user := decodeInjectPlain(injected[0])
			if user == "" {
				user = p
			}
			if user != "please stop and fix tests" {
				t.Errorf("inject user = %q, want %q (prompt=%q)", user, "please stop and fix tests", p)
			}
		})
	}
}

// TestSendWorkspaceFollowUpInjectAckDoesNotHoldChatLife: Claude InjectPlain
// acks only after a tool_result. A follow-up must drop lockChatLife across that
// wait, or SwitchChatMode → RememberWorkspaceChatSelection deadlocks.
func TestSendWorkspaceFollowUpInjectAckDoesNotHoldChatLife(t *testing.T) {
	started := make(chan struct{})
	gotInject := make(chan struct{})
	ackWait := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release, gotInject: gotInject, ackWait: ackWait}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "inject-life", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "sonnet", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not start")
	}

	followDone := make(chan string, 1)
	go func() {
		followDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "please stop and fix tests", "sonnet", nil, nil, "", "")
	}()
	select {
	case <-gotInject:
	case <-time.After(3 * time.Second):
		t.Fatal("follow-up did not reach inject")
	}

	switched := make(chan string, 1)
	go func() {
		switched <- eng.SwitchChatMode(ws.ID, "PA", domain.ChatModePlan)
	}()
	select {
	case msg := <-switched:
		if !strings.Contains(msg, "plan") {
			t.Errorf("SwitchChatMode = %q", msg)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("SwitchChatMode deadlocked on chat life lock held across inject ack")
	}

	close(ackWait)
	select {
	case got := <-followDone:
		if got != chatInjectedSentinel {
			t.Errorf("follow-up = %q, want injected sentinel", got)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("follow-up did not finish after inject ack")
	}

	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not finish")
	}
}

func TestWorkspaceSessionsScoped(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	ws, err := st.AddWorkspace(domain.Workspace{ID: "WS-scoped"})
	if err != nil {
		t.Fatal(err)
	}
	for _, spec := range []struct{ owner, code string }{
		{"", "CO"}, {"PA", "CO"}, {"MA", "CO"},
		{"PA2", "CO"}, {"PA2", "RV"}, {"PA3", "CO"},
		{"PA2", "PA"}, {"PA2", "MA"}, {"PA2", "empty"},
	} {
		cfg, err := json.Marshal(agentConfig{Code: spec.code, OwnerCode: spec.owner})
		if err != nil {
			t.Fatal(err)
		}
		sess, err := st.CreateSession(domain.Session{
			ID: spec.owner + "-" + spec.code, WorkspaceID: ws.ID,
			Kind: "coder", Provider: domain.ProviderClaudeCode, ConfigJSON: string(cfg),
		})
		if err != nil {
			t.Fatal(err)
		}
		if spec.code == "empty" {
			continue
		}
		if _, err := st.AppendMessage(sess.ID, domain.Message{
			Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock(sess.ID)},
		}); err != nil {
			t.Fatal(err)
		}
	}
	for _, tc := range []struct {
		owner, code string
		want        []string
	}{
		{"", "", []string{"-CO"}},
		{"PA", "", []string{"-CO", "PA-CO"}},
		{"MA", "", []string{"-CO", "MA-CO"}},
		{"PA2", "", []string{"PA2-CO", "PA2-RV"}},
		{"PA2", "CO", []string{"PA2-CO"}},
		{"PA3", "CO", []string{"PA3-CO"}},
		{"PA2", "QA", nil},
	} {
		t.Run(tc.owner+"/"+tc.code, func(t *testing.T) {
			views := eng.WorkspaceSessions(ws.ID, tc.owner, tc.code)
			if len(views) != len(tc.want) {
				t.Fatalf("sessions = %+v, want %v", views, tc.want)
			}
			got := make(map[string]bool)
			for _, view := range views {
				got[view.ID] = true
				if len(view.Items) != 1 || view.Items[0].Text != view.ID {
					t.Errorf("transcript = %+v, want text %q", view.Items, view.ID)
				}
			}
			for _, id := range tc.want {
				if !got[id] {
					t.Errorf("sessions = %v, missing %s", got, id)
				}
			}
		})
	}
}

func TestSendWorkspaceMessageConcurrentFollowUpDoesNotCancelFirst(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cursor race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstIn := make(chan struct{})
	secondIn := make(chan struct{})
	proceed := make(chan struct{})
	var n atomic.Int32
	testBeforeChatClaim = func() {
		switch n.Add(1) {
		case 1:
			close(firstIn)
			<-proceed
		case 2:
			close(secondIn)
		}
	}
	t.Cleanup(func() { testBeforeChatClaim = nil })

	replies := make(chan string, 2)
	go func() {
		replies <- eng.SendWorkspaceMessage(ws.ID, "PA", "first prompt", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-firstIn:
	case <-time.After(3 * time.Second):
		t.Fatal("first send never entered prologue")
	}
	go func() {
		replies <- eng.SendWorkspaceMessage(ws.ID, "PA", "second prompt", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-secondIn:
	case <-time.After(3 * time.Second):
		t.Fatal("second send never took a FIFO ticket")
	}
	close(proceed)
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("Cursor turn did not start")
	}

	var injected string
	select {
	case injected = <-replies:
	case <-time.After(3 * time.Second):
		t.Fatal("expected one send to return the inject sentinel before Run finished")
	}
	if injected != chatInjectedSentinel {
		t.Errorf("quick send = %q, want injected sentinel", injected)
	}

	close(release)
	select {
	case <-replies:
	case <-time.After(3 * time.Second):
		t.Fatal("remaining send did not finish")
	}
	if n := runner.runCount(); n != 1 {
		t.Errorf("Run called %d times, want 1 (follow-up must inject, not Swap-cancel)", n)
	}
	spec, ok := runner.lastSpec()
	if !ok {
		t.Fatal("missing Cursor chat spec")
	}
	if !strings.Contains(spec.Prompt, "first prompt") {
		t.Errorf("Prompt = %q, want first prompt to own the run", spec.Prompt)
	}
	got := runner.allInjected()
	if len(got) != 1 {
		t.Fatalf("injected = %v, want one payload", got)
	}
	p, user := decodeInjectPlain(got[0])
	if user == "" {
		user = p
	}
	if user != "second prompt" {
		t.Errorf("inject user = %q, want %q (prompt=%q)", user, "second prompt", p)
	}
}

func TestSendWorkspaceMessageFollowUpDoesNotRestartAfterStop(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cursor stop race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	blocked := make(chan struct{})
	proceed := make(chan struct{})
	var once sync.Once
	testBeforeChatClaim = func() {
		if eng.liveChatTurn(ws.ID, "PA") == nil {
			return
		}
		once.Do(func() { close(blocked) })
		<-proceed
	}
	t.Cleanup(func() { testBeforeChatClaim = nil })

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not start")
	}
	followDone := make(chan string, 1)
	go func() {
		followDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "late steer", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-blocked:
	case <-time.After(3 * time.Second):
		t.Fatal("follow-up never entered prologue")
	}
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false")
	}
	close(proceed)
	select {
	case got := <-followDone:
		if got != chatStoppedReply {
			t.Errorf("follow-up = %q, want %q (must not start a replacement run)", got, chatStoppedReply)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("follow-up did not return after Stop")
	}
	if n := runner.runCount(); n != 1 {
		t.Errorf("Run called %d times, want 1", n)
	}
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not finish after CancelChat")
	}
}

func TestSendWorkspaceMessageInjectsDisplayText(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cursor display", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not start")
	}

	got := eng.SendWorkspaceMessage(ws.ID, "PA", "WIRE_PROMPT", "cursor/composer-2.5", nil, nil, "", "Fix findings")
	if got != chatInjectedSentinel {
		t.Errorf("SendWorkspaceMessage(follow-up) = %q, want injected sentinel", got)
	}
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not finish")
	}
	injected := runner.allInjected()
	if len(injected) != 1 {
		t.Fatalf("injected = %v, want one encoded payload", injected)
	}
	p, d := decodeInjectPlain(injected[0])
	if d != "Fix findings" {
		t.Errorf("decoded inject display=%q, want Fix findings", d)
	}
	if !strings.Contains(p, "WIRE_PROMPT") {
		t.Errorf("decoded inject prompt=%q, want to contain WIRE_PROMPT", p)
	}
	if !strings.Contains(p, "Recur workspace") {
		t.Errorf("decoded inject prompt=%q, want current workspace surface", p)
	}
}

func TestSendWorkspaceMessageInjectsCurrentSurface(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cursor surface", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not start")
	}

	wire := chatSurfaceOpen + `{"surface":"github_pr","prNumber":9}` + chatSurfaceClose + "\nreview this"
	got := eng.SendWorkspaceMessage(ws.ID, "PA", wire, "cursor/composer-2.5", nil, nil, "", "")
	if got != chatInjectedSentinel {
		t.Errorf("SendWorkspaceMessage(follow-up) = %q, want injected sentinel", got)
	}
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not finish")
	}
	injected := runner.allInjected()
	if len(injected) != 1 {
		t.Fatalf("injected = %v, want one payload", injected)
	}
	p, user := decodeInjectPlain(injected[0])
	if user == "" {
		user = p
	}
	if user != "review this" {
		t.Errorf("inject user = %q, want %q", user, "review this")
	}
	if !strings.Contains(p, "GitHub pull request") {
		t.Errorf("inject prompt=%q, want current GitHub PR surface", p)
	}
}

func TestSendWorkspaceMessageInjectSkipsForkPreamble(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cursor fork inject", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "only turn", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not start")
	}
	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("session missing")
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	cfg.ForkInject = true
	raw, err := json.Marshal(cfg)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if err := st.UpdateSessionConfigJSON(sess.ID, string(raw)); err != nil {
		t.Fatalf("UpdateSessionConfigJSON: %v", err)
	}

	got := eng.SendWorkspaceMessage(ws.ID, "PA", "steer later", "cursor/composer-2.5", nil, nil, "", "")
	if got != chatInjectedSentinel {
		t.Errorf("follow-up = %q, want injected sentinel", got)
	}
	if n := runner.runCount(); n != 1 {
		t.Errorf("Run called %d times, want 1", n)
	}
	injected := runner.allInjected()
	if len(injected) != 1 {
		t.Fatalf("injected = %v, want one payload", injected)
	}
	p, user := decodeInjectPlain(injected[0])
	if user == "" {
		user = p
	}
	if user != "steer later" {
		t.Errorf("inject user = %q, want %q", user, "steer later")
	}
	if strings.Contains(p, forkPreambleLead) {
		t.Errorf("live inject must not re-prepend fork history, prompt=%q", p)
	}
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not finish")
	}
}

func TestSendWorkspaceMessageDoesNotInjectAfterCancel(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cursor stop", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not start")
	}
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false")
	}
	if eng.tryInjectChat(ws.ID, "PA", "late steer") {
		t.Fatal("tryInjectChat after CancelChat must not acknowledge a discarded steer")
	}
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not finish after CancelChat")
	}
}

func TestSendWorkspaceMessageDoesNotInjectAfterResult(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	secondStarted := make(chan struct{})
	secondRelease := make(chan struct{})
	runner := &holdAfterResultRunner{
		started: started, release: release,
		secondStarted: secondStarted, secondRelease: secondRelease,
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cursor result", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not emit result")
	}
	followDone := make(chan string, 1)
	go func() {
		followDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "late steer", "cursor/composer-2.5", nil, nil, "", "")
	}()
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first Cursor turn did not finish")
	}
	select {
	case <-secondStarted:
	case <-time.After(3 * time.Second):
		t.Fatal("replacement turn did not start after the first turn unregistered")
	}
	a, ok := st.Agent(ws.ID, "PA")
	if !ok || a.Status != "working" {
		t.Errorf("status=%q, want working while the replacement run is live", a.Status)
	}
	if got := runner.specResume(1); got != "agent-done" {
		t.Errorf("replacement ResumeSession=%q, want agent-done (refresh after predecessor unregisters)", got)
	}
	close(secondRelease)
	select {
	case got := <-followDone:
		if got == chatInjectedSentinel {
			t.Fatal("follow-up after KindResult must not return the inject sentinel")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("follow-up did not finish")
	}
}

func TestSendWorkspaceMessageDoesNotSwapHealthyTurnWithoutInject(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdUntilRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "no swap", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "claude-code/sonnet", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not start")
	}
	followDone := make(chan string, 1)
	go func() {
		followDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "steer please", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-followDone:
		t.Fatal("Cursor follow-up returned while Claude turn still running — Swap-cancelled?")
	case <-time.After(80 * time.Millisecond):
	}
	if n := runner.runCount(); n != 1 {
		t.Errorf("Run called %d times, want 1 while first turn is live", n)
	}
	if n := runner.cancelCount(); n != 0 {
		t.Errorf("first Run cancelled %d times, want 0 (must wait, not Swap)", n)
	}
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not finish")
	}
	select {
	case got := <-followDone:
		if got == chatInjectedSentinel {
			t.Fatal("follow-up into a non-inject turn must not return the inject sentinel")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("follow-up did not finish after first turn ended")
	}
	if n := runner.runCount(); n != 2 {
		t.Errorf("Run called %d times, want 2 (follow-up starts after first ends)", n)
	}
	if n := runner.cancelCount(); n != 0 {
		t.Errorf("Run cancelled %d times, want 0", n)
	}
}

func TestFreshSessionDoesNotInjectIntoLiveTurn(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fresh live", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not start")
	}

	followDone := make(chan string, 1)
	go func() {
		followDone <- eng.SendWorkspaceTurn(ws.ID, "PA", "WIRE review", "cursor/composer-2.5", nil, nil, "review", "Re-review", true)
	}()
	select {
	case got := <-followDone:
		if got == chatInjectedSentinel {
			t.Fatal("fresh_session follow-up injected into the live turn")
		}
		t.Fatalf("fresh_session follow-up returned %q before the live turn ended", got)
	case <-time.After(80 * time.Millisecond):
	}
	if n := runner.runCount(); n != 1 {
		t.Errorf("Run called %d times, want 1 while first turn is live", n)
	}
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not finish")
	}
	select {
	case got := <-followDone:
		if got == chatInjectedSentinel {
			t.Fatal("fresh_session follow-up returned inject sentinel")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("fresh_session follow-up did not finish")
	}
	specs := runner.allSpecs()
	if len(specs) != 2 {
		t.Fatalf("Run specs = %d, want 2 (isolated prompt starts a new Run)", len(specs))
	}
	if specs[1].ResumeSession != "" {
		t.Errorf("isolated replacement ResumeSession=%q, want empty", specs[1].ResumeSession)
	}
}

func TestModeSwitchContinueRegistersAfterFIFOClaim(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "continue fifo", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstIn := make(chan struct{})
	contIn := make(chan struct{})
	proceed := make(chan struct{})
	var n atomic.Int32
	testBeforeChatClaim = func() {
		switch n.Add(1) {
		case 1:
			close(firstIn)
			<-proceed
		case 2:
			close(contIn)
		}
	}
	t.Cleanup(func() { testBeforeChatClaim = nil })

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "user prompt", "cursor/composer-2.5", nil, nil, "", "")
	}()
	select {
	case <-firstIn:
	case <-time.After(3 * time.Second):
		t.Fatal("first send never entered prologue")
	}

	epoch := eng.allocContinueGen(ws.ID, "PA")
	contDone := make(chan string, 1)
	go func() {
		contDone <- eng.sendWorkspaceTurn(ws.ID, "PA",
			formatModeSwitchResumePrompt(domain.ChatModeAgent, domain.ChatModePlan),
			"cursor/composer-2.5", nil, nil, domain.ChatModePlan,
			formatModeSwitchDisplay(domain.ChatModePlan), false, epoch)
	}()
	select {
	case <-contIn:
	case <-time.After(3 * time.Second):
		t.Fatal("continuation never entered prologue")
	}
	deadline := time.Now().Add(80 * time.Millisecond)
	for time.Now().Before(deadline) {
		if eng.liveChatTurn(ws.ID, "PA") != nil {
			t.Fatal("continuation registered before FIFO claim")
		}
		time.Sleep(5 * time.Millisecond)
	}
	close(proceed)
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not start")
	}
	close(release)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first send did not finish")
	}
	select {
	case <-contDone:
	case <-time.After(3 * time.Second):
		t.Fatal("continuation deadlocked waiting on a later ticket")
	}
}

// holdInjectRunner holds Run until release, capturing Inject payloads so a
// concurrent SendWorkspaceMessage can be asserted to steer rather than spawn.
type holdInjectRunner struct {
	started   chan struct{}
	release   chan struct{}
	gotInject chan struct{}
	ackWait   chan struct{}

	mu          sync.Mutex
	startedOnce sync.Once
	injectOnce  sync.Once
	runs        int
	specs       []AgentSpec
	injected    []string
}

func (r *holdInjectRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	r.runs++
	r.specs = append(r.specs, spec)
	r.mu.Unlock()
	r.startedOnce.Do(func() { close(r.started) })

	ack := func() {
		if spec.InjectAck == nil {
			return
		}
		if r.ackWait != nil {
			select {
			case <-r.ackWait:
			case <-r.release:
			case <-ctx.Done():
				spec.InjectAck <- false
				return
			}
		}
		spec.InjectAck <- true
	}
	note := func(msg string) {
		msg = strings.TrimSpace(msg)
		if msg == "" {
			return
		}
		r.mu.Lock()
		r.injected = append(r.injected, msg)
		r.mu.Unlock()
		r.injectOnce.Do(func() {
			if r.gotInject != nil {
				close(r.gotInject)
			}
		})
		ack()
	}
	collect := func() {
		if spec.Inject == nil {
			return
		}
		for {
			select {
			case msg, ok := <-spec.Inject:
				if !ok {
					return
				}
				note(msg)
			default:
				return
			}
		}
	}

	select {
	case msg := <-spec.Inject:
		note(msg)
		collect()
		select {
		case <-r.release:
		case <-ctx.Done():
			return Result{}, ctx.Err()
		}
	case <-r.release:
		collect()
	case <-ctx.Done():
		return Result{}, ctx.Err()
	}
	if emit != nil {
		emit(ParsedEvent{Kind: KindResult, Result: &Result{Subtype: "success", SessionID: "cursor-steer", Text: "done"}})
	}
	return Result{Subtype: "success", SessionID: "cursor-steer", Text: "done"}, nil
}

func (r *holdInjectRunner) runCount() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.runs
}

func (r *holdInjectRunner) lastSpec() (AgentSpec, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if len(r.specs) == 0 {
		return AgentSpec{}, false
	}
	return r.specs[len(r.specs)-1], true
}

func (r *holdInjectRunner) allInjected() []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]string(nil), r.injected...)
}

func (r *holdInjectRunner) allSpecs() []AgentSpec {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]AgentSpec(nil), r.specs...)
}

// holdAfterResultRunner emits KindResult then waits, so a concurrent follow-up
// can be asserted not to inject into a finished turn.
type holdAfterResultRunner struct {
	started       chan struct{}
	release       chan struct{}
	secondStarted chan struct{}
	secondRelease chan struct{}

	mu          sync.Mutex
	startedOnce sync.Once
	secondOnce  sync.Once
	runs        int
	specs       []AgentSpec
}

func (r *holdAfterResultRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	r.runs++
	r.specs = append(r.specs, spec)
	n := r.runs
	r.mu.Unlock()
	if emit != nil {
		emit(ParsedEvent{Kind: KindResult, Result: &Result{Subtype: "success", SessionID: "agent-done", Text: "done"}})
	}
	if n == 1 {
		r.startedOnce.Do(func() { close(r.started) })
		select {
		case <-r.release:
		case <-ctx.Done():
			return Result{}, ctx.Err()
		}
		return Result{Subtype: "success", SessionID: "agent-done", Text: "done"}, nil
	}
	if r.secondStarted != nil {
		r.secondOnce.Do(func() { close(r.secondStarted) })
		select {
		case <-r.secondRelease:
		case <-ctx.Done():
			return Result{}, ctx.Err()
		}
	}
	return Result{Subtype: "success", SessionID: "agent-done", Text: "done"}, nil
}

func (r *holdAfterResultRunner) specResume(i int) string {
	r.mu.Lock()
	defer r.mu.Unlock()
	if i < 0 || i >= len(r.specs) {
		return ""
	}
	return r.specs[i].ResumeSession
}

// holdUntilRunner holds Run until release without honouring Inject, so a
// concurrent Cursor follow-up can be asserted not to Swap-cancel the live turn.
type holdUntilRunner struct {
	started chan struct{}
	release chan struct{}

	mu          sync.Mutex
	startedOnce sync.Once
	runs        int
	cancels     int
}

func (r *holdUntilRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	r.runs++
	r.mu.Unlock()
	r.startedOnce.Do(func() { close(r.started) })
	select {
	case <-r.release:
	case <-ctx.Done():
		r.mu.Lock()
		r.cancels++
		r.mu.Unlock()
		return Result{}, ctx.Err()
	}
	if emit != nil {
		emit(ParsedEvent{Kind: KindResult, Result: &Result{Subtype: "success", SessionID: "hold", Text: "done"}})
	}
	return Result{Subtype: "success", SessionID: "hold", Text: "done"}, nil
}

func (r *holdUntilRunner) runCount() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.runs
}

func (r *holdUntilRunner) cancelCount() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.cancels
}

func BenchmarkWorkspaceSessionsScoped(b *testing.B) {
	b.Setenv("RECUR_DATA_DIR", b.TempDir())
	st, err := store.New()
	if err != nil {
		b.Fatal(err)
	}
	b.Cleanup(func() { st.Close() })
	ws, err := st.AddWorkspace(domain.Workspace{ID: "WS-bench"})
	if err != nil {
		b.Fatal(err)
	}
	for i := range 50 {
		owner := "PA3"
		if i == 0 {
			owner = "PA2"
		}
		sess, err := st.CreateSession(domain.Session{
			WorkspaceID: ws.ID, Kind: "coder", Provider: domain.ProviderClaudeCode,
			ConfigJSON: fmt.Sprintf(`{"code":"CO%d","ownerCode":%q}`, i, owner),
		})
		if err != nil {
			b.Fatal(err)
		}
		if _, err := st.AppendMessage(sess.ID, domain.Message{
			Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock(strings.Repeat("transcript ", 1000))},
		}); err != nil {
			b.Fatal(err)
		}
	}
	eng := &Engine{Store: st}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		eng.WorkspaceSessions(ws.ID, "PA2", "")
	}
}

// TestSubagentNoticeQueuesAsOwnTurn: a hidden-display notice (a subagent
// report) sent while a turn is live must NOT steer it; it waits and runs as a
// second Run on the same tab, so two children finishing back to back each get
// a full turn.
func TestSubagentNoticeQueuesAsOwnTurn(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	runner := &holdInjectRunner{started: started, release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "notice", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	model := "claude-code/sonnet"
	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", model, nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not start")
	}
	noticeDone := make(chan string, 1)
	go func() {
		noticeDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "Subagent x has finished.", model, nil, nil, "", displayHiddenSentinel+"x finished (ws.x)")
	}()
	time.Sleep(150 * time.Millisecond)
	if n := runner.runCount(); n != 1 {
		t.Fatalf("Run called %d times before release, want 1 (notice must wait, not start yet)", n)
	}
	close(release)
	for _, ch := range []chan string{firstDone, noticeDone} {
		select {
		case reply := <-ch:
			if reply == chatInjectedSentinel {
				t.Errorf("reply = injected sentinel; the notice must not steer the live turn")
			}
		case <-time.After(3 * time.Second):
			t.Fatal("a turn did not finish after release")
		}
	}
	if n := runner.runCount(); n != 2 {
		t.Errorf("Run called %d times, want 2 (notice runs as its own turn)", n)
	}
	runner.mu.Lock()
	injected := append([]string(nil), runner.injected...)
	runner.mu.Unlock()
	if len(injected) != 0 {
		t.Errorf("notice was injected as a steer: %q", injected)
	}
}
