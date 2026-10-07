package engine

import (
	"strings"
	"testing"

	"recur/internal/domain"
	"recur/internal/store"
)

func TestSendAppMessageUsesAskCoordinatorShape(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("app-sess-1", "Needs you: LOOP-2218.") },
	}
	eng, st, sink, _ := newTestEngine(t, runner)

	reply := eng.SendAppMessage("", "What needs me?", "sonnet", nil)
	if !strings.Contains(reply, "Needs you: LOOP-2218") {
		t.Fatalf("reply: got %q", reply)
	}

	spec, ok := runner.specWithRole(domain.RoleMaestro)
	if !ok {
		t.Fatal("Ask Recur turn did not run")
	}
	if spec.AgentID != domain.AppAgentCode {
		t.Errorf("AgentID = %q, want APP", spec.AgentID)
	}
	if spec.Route.Kind != domain.SessionKindApp || spec.Route.Code != domain.AppAgentCode {
		t.Errorf("Route = %+v, want app/APP", spec.Route)
	}
	if !strings.Contains(spec.AppendSystemPrompt, "Ask Recur") {
		t.Errorf("AppendSystemPrompt missing coordinator instructions: %q", spec.AppendSystemPrompt)
	}
	wantTools := map[string]bool{
		mcpListWorkspaceBriefs: true,
		mcpGetWorkspaceContext: true,
		mcpPresentWorkspace:    true,
	}
	if len(spec.AllowedTools) != 3 {
		t.Errorf("AllowedTools = %v, want the three briefing tools", spec.AllowedTools)
	}
	for _, tl := range spec.AllowedTools {
		if !wantTools[tl] {
			t.Errorf("unexpected tool %q", tl)
		}
		if tl == "Edit" || tl == "Write" || tl == mcpAskUser || tl == mcpEscalate {
			t.Errorf("coordinator must not get %q", tl)
		}
	}
	cwd, err := store.AppChatDir()
	if err != nil {
		t.Fatal(err)
	}
	if spec.Worktree != cwd {
		t.Errorf("Worktree = %q, want app-chat dir %q", spec.Worktree, cwd)
	}

	sess, ok := st.SessionForAppKind(domain.SessionKindApp)
	if !ok || sess.WorkspaceID != "" {
		t.Fatalf("app session = %+v ok=%v, want workspace_id empty", sess, ok)
	}
	if sink.has("workspace.presented") {
		t.Error("SendAppMessage must not present a workspace by itself")
	}
}

func TestSendAppMessageEmptyIsNoop(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, &ScriptedRunner{})
	if got := eng.SendAppMessage("", "  ", "sonnet", nil); got != "" {
		t.Errorf("empty prompt reply = %q, want empty", got)
	}
	if sink.has("agent.updated") {
		t.Error("empty prompt must not publish agent.updated")
	}
}

func TestSendAppMessagePublishesRunningStatus(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("app-sess-run", "ok") },
	}
	eng, _, sink, _ := newTestEngine(t, runner)

	reply := eng.SendAppMessage("", "What needs me?", "sonnet", nil)
	if !strings.Contains(reply, "ok") {
		t.Fatalf("reply: got %q", reply)
	}
	assertAppChatStatusSeq(t, sink, domain.AppAgentCode)

	tab := eng.NewAppChat()
	if tab.Code != "AP2" {
		t.Fatalf("new tab code = %q, want AP2", tab.Code)
	}
	eng.SendAppMessage("AP2", "and another", "sonnet", nil)
	assertAppChatStatusSeq(t, sink, "AP2")
}

func assertAppChatStatusSeq(t *testing.T, sink *recordingSink, code string) {
	t.Helper()
	wantID := "app-" + code
	var statuses []string
	for _, ev := range sink.byName("agent.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["agentId"] != wantID {
			continue
		}
		if d["workspaceId"] != "" {
			t.Errorf("%s workspaceId = %v, want empty", code, d["workspaceId"])
		}
		if d["surface"] != "app" {
			t.Errorf("%s surface = %v, want app", code, d["surface"])
		}
		if d["code"] != code {
			t.Errorf("%s code = %v, want %s", code, d["code"], code)
		}
		st, _ := d["status"].(string)
		statuses = append(statuses, st)
	}
	if len(statuses) < 2 || statuses[0] != "working" || statuses[len(statuses)-1] != "idle" {
		t.Errorf("%s agent.updated statuses = %v, want working … idle", code, statuses)
	}
}

func TestPresentWorkspaceValidatesTargets(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddProject(domain.Project{ID: "p1", Name: "Storefront"}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "W-live", ProjectID: "p1", Short: "Auth"}); err != nil {
		t.Fatal(err)
	}

	if got := eng.PresentWorkspace(""); !strings.Contains(got, "Refused") {
		t.Errorf("empty target: %q", got)
	}
	if got := eng.PresentWorkspace("workspace:missing"); !strings.Contains(got, "Refused") {
		t.Errorf("missing workspace: %q", got)
	}
	if got := eng.PresentWorkspace("chat:W-live"); !strings.Contains(got, "Refused") {
		t.Errorf("malformed chat: %q", got)
	}
	if got := eng.PresentWorkspace("workspace:W-live"); !strings.Contains(got, `"ok":true`) || !strings.Contains(got, "workspace:W-live") {
		t.Errorf("live workspace: %q", got)
	}
	if got := eng.PresentWorkspace("workspace:W-live:plan"); !strings.Contains(got, `"ok":true`) {
		t.Errorf("workspace plan panel: %q", got)
	}
	if !strings.Contains(eng.PresentWorkspace("workspace:W-live"), `"title":"Auth"`) {
		t.Errorf("live workspace missing brief title: %q", eng.PresentWorkspace("workspace:W-live"))
	}
	if sink.has("workspace.presented") {
		t.Error("present_workspace must not switch the UI via workspace.presented")
	}
}

func TestAppChatUnseenAndFocus(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	eng.flagAppChatUnseen("")
	c := st.AttentionCounts()
	if c.AppUnseen != 1 {
		t.Errorf("AppUnseen = %d, want 1", c.AppUnseen)
	}
	if c.Dock < 1 {
		t.Errorf("Dock = %d, want app unseen included", c.Dock)
	}
	eng.SetAppChatFocus(true, "")
	c = st.AttentionCounts()
	if c.AppUnseen != 0 {
		t.Errorf("focused Ask Recur still unseen: %+v", c)
	}
}

func TestAppChatTabsLifecycle(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})

	tabs := eng.ListAppChats()
	if len(tabs) != 1 || tabs[0].Code != domain.AppAgentCode {
		t.Fatalf("initial tabs = %+v, want just APP", tabs)
	}

	tab := eng.NewAppChat()
	if tab.Code != "AP2" {
		t.Fatalf("new tab code = %q, want AP2", tab.Code)
	}
	if tabs = eng.ListAppChats(); len(tabs) != 2 || tabs[1].Code != "AP2" {
		t.Fatalf("tabs after new = %+v", tabs)
	}

	// Per-tab unseen: both tabs flagged counts twice; seen clears one.
	eng.flagAppChatUnseen("AP2")
	eng.flagAppChatUnseen("")
	if c := st.AttentionCounts(); c.AppUnseen != 2 {
		t.Errorf("AppUnseen = %d, want 2", c.AppUnseen)
	}
	eng.MarkAppChatSeen("AP2")
	if c := st.AttentionCounts(); c.AppUnseen != 1 {
		t.Errorf("AppUnseen after seen = %d, want 1", c.AppUnseen)
	}

	// Closing a secondary tab removes it and drops its unseen from the rollup.
	eng.flagAppChatUnseen("AP2")
	if tabs = eng.CloseAppChat("AP2"); len(tabs) != 1 || tabs[0].Code != domain.AppAgentCode {
		t.Fatalf("tabs after close = %+v", tabs)
	}
	if c := st.AttentionCounts(); c.AppUnseen != 1 {
		t.Errorf("AppUnseen after close = %d, want 1 (APP only)", c.AppUnseen)
	}

	// Closing the primary resets it to a fresh chat (same code, new session)
	// and drops its unseen; extra-tab codes are never reused.
	if tabs = eng.CloseAppChat(domain.AppAgentCode); len(tabs) != 1 || tabs[0].Code != domain.AppAgentCode || tabs[0].Unseen {
		t.Fatalf("APP close should reset to a fresh primary; tabs = %+v", tabs)
	}
	if c := st.AttentionCounts(); c.AppUnseen != 0 {
		t.Errorf("AppUnseen after primary close = %d, want 0", c.AppUnseen)
	}
	if next := eng.NewAppChat(); next.Code != "AP3" {
		t.Errorf("next tab code = %q, want AP3 (AP2 not reused)", next.Code)
	}
}

func TestCancelAppMessageInterruptsLiveTurn(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	for _, code := range []string{"APP", "AP2"} {
		cancelled := false
		turn := eng.registerChatTurn("", code, func() { cancelled = true })
		if turn == nil {
			t.Fatalf("registerChatTurn(%q) returned nil", code)
		}
		// Regression: liveChatTurn used to reject empty workspace IDs, so the
		// Stop button never reached an Ask Recur turn.
		eng.CancelAppMessage(code)
		if !cancelled {
			t.Errorf("CancelAppMessage(%q) did not cancel the live turn", code)
		}
		eng.unregisterChatTurn("", code, turn)
	}
	// Other empty-workspace lookups stay invalid.
	if eng.liveChatTurn("", "PA") != nil || eng.liveChatTurn("", "MA") != nil {
		t.Error("empty-workspace lookup must stay nil for workspace codes")
	}
}

func TestAppChatSelectionPersistsAskMode(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	eng.RememberAppChatSelection("", "sonnet", map[string]string{"effort": "low"})
	model, flags := eng.AppChatSelection("")
	if model != "sonnet" {
		t.Errorf("model = %q, want sonnet", model)
	}
	if flags["effort"] != "low" {
		t.Errorf("flags = %v", flags)
	}
}
