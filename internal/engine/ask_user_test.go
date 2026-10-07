package engine

import (
	"context"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/mcp"
)

func TestAskUserParksWithoutBlocking(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	rep := engineReporter{eng: eng}

	done := make(chan string, 1)
	go func() {
		done <- rep.AskUser("W1", "PA", mcp.AskUserRequest{
			Question: "Which color?",
			Options:  []mcp.EscalationOption{{Text: "Blue"}},
		})
	}()

	select {
	case msg := <-done:
		if !strings.Contains(msg, "Question delivered") {
			t.Fatalf("park result = %q", msg)
		}
	case <-time.After(500 * time.Millisecond):
		t.Fatal("AskUser blocked; park must return immediately")
	}

	pending := eng.PendingAskUsers()
	if len(pending) != 1 {
		t.Fatalf("pending = %d, want 1", len(pending))
	}
	if pending[0].Question != "Which color?" {
		t.Fatalf("question = %q", pending[0].Question)
	}
	if pending[0].AgentCode != "PA" {
		t.Fatalf("agent = %q", pending[0].AgentCode)
	}
}

func TestAskUserSurvivesUnregisterMCP(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	_ = eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: "W1", AgentCode: "PA", Question: "still here?",
	}, nil)
	token, _, err := eng.registerMCP("W1", "PA", "PA", []string{}, "sonnet")
	if err != nil {
		t.Fatal(err)
	}
	eng.unregisterMCP(token)
	if len(eng.PendingAskUsers()) != 1 {
		t.Fatalf("parked ask cleared on unregisterMCP: %+v", eng.PendingAskUsers())
	}
}

func TestAnswerAskUserRemovesPending(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	q := eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: "W1", AgentCode: "PA", Question: "color?",
	}, nil)
	if !eng.AnswerAskUser(q.ID, "Blue", "", nil) {
		t.Fatal("AnswerAskUser returned false")
	}
	if len(eng.PendingAskUsers()) != 0 {
		t.Fatalf("still pending after answer: %+v", eng.PendingAskUsers())
	}
}

func TestFormatAskUserResumePrompt(t *testing.T) {
	got := formatAskUserResumePrompt(
		domain.UserQuestion{Question: "Which color?", Context: "pick one"},
		domain.UserQuestionAnswer{Choice: "Blue"},
	)
	if !strings.Contains(got, "Which color?") || !strings.Contains(got, "Blue") {
		t.Fatalf("resume prompt missing pieces: %q", got)
	}
	if !strings.Contains(got, "Continue with this answer") {
		t.Fatalf("resume prompt missing continue instruction: %q", got)
	}
}

// TestResumeAfterAskUserKeepsDockProvider: answering ask_user must resume with the
// tab's persisted dock model/provider — not Settings' role default. Empty model
// on that path used to fall back to Claude while Agent.SessionID still held a
// Cursor agent-… id, so claude -p --resume rejected it.
func TestResumeAfterAskUserKeepsDockProvider(t *testing.T) {
	const cursorAgentID = "agent-192c0959-3abe-403c-a3e6-aeb25a687b0c"
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(cursorAgentID, "OK.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	set := st.Settings()
	// Plan/Ask fallbacks are Claude — the bug is falling through to these.
	set.ModelPlan = "claude-code/opus"
	set.ModelAsk = "claude-code/sonnet"
	set.ModelAgent = "claude-code/sonnet"
	st.SetSettings(set)

	ws, err := eng.CreateWorkspace(projID, "ask-resume", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	// Dock picked Cursor + Plan; that selection must survive ask_user resume.
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "plan the work", "cursor/composer-2.5",
		map[string]string{"fast": "true"}, nil, domain.ChatModePlan, "")
	eng.setAgentSession(ws.ID, "PA", cursorAgentID)

	before := len(runner.specsWithRole(domain.RoleCoder))
	eng.resumeAfterAskUser(
		domain.UserQuestion{WorkspaceID: ws.ID, AgentCode: "PA", Question: "Which depth?"},
		domain.UserQuestionAnswer{Choice: "Write the full comparison plan"},
		nil,
	)
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) <= before {
		t.Fatalf("resumeAfterAskUser did not run a chat turn; coder specs=%d", len(specs))
	}
	got := specs[len(specs)-1]
	if got.Provider != domain.ProviderCursor {
		t.Errorf("resumeAfterAskUser provider = %q, want %q (dock Cursor, not Settings Claude)",
			got.Provider, domain.ProviderCursor)
	}
	if got.Model != "composer-2.5" {
		t.Errorf("resumeAfterAskUser model = %q, want composer-2.5", got.Model)
	}
	if got.ResumeSession != cursorAgentID {
		t.Errorf("resumeAfterAskUser ResumeSession = %q, want %q", got.ResumeSession, cursorAgentID)
	}
	if !strings.Contains(got.Prompt, "Which depth?") || !strings.Contains(got.Prompt, "Write the full comparison plan") {
		t.Errorf("resumeAfterAskUser prompt missing Q/A: %q", got.Prompt)
	}
	if got.AppendSystemPrompt == "" || !strings.Contains(got.AppendSystemPrompt, "PLAN mode") {
		t.Errorf("resumeAfterAskUser should keep Plan mode, append=%q", got.AppendSystemPrompt)
	}
}

func TestParkChatTurnQuietFinalize(t *testing.T) {
	cancelled := make(chan struct{}, 1)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	addSoloWorkspace(t, st, "W1")
	turn := eng.registerChatTurn("W1", "PA", func() { cancelled <- struct{}{} })
	if turn == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	if !eng.ParkChatTurn("W1", "PA") {
		t.Fatal("ParkChatTurn returned false")
	}
	select {
	case <-cancelled:
	case <-time.After(200 * time.Millisecond):
		t.Fatal("ParkChatTurn did not cancel")
	}
	if !eng.chatTurnWasParked("W1", "PA") {
		t.Fatal("expected turn marked parked")
	}
	out := eng.finalizeChatReply("W1", "PA", "hi", "", "partial", Result{}, context.Canceled, "Done.", time.Time{}, false, "")
	if out != "partial" {
		t.Fatalf("parked finalize = %q, want streamed text (quiet)", out)
	}
}

func TestParkChatTurnIfSkipsNewerTurn(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	addSoloWorkspace(t, st, "W1")
	oldCancelled := false
	// Stale asking-turn pointer (not the live registration).
	old := &chatTurn{cancel: func() { oldCancelled = true }}
	newCancelled := false
	newer := eng.registerChatTurn("W1", "PA", func() { newCancelled = true })
	if newer == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	// Answering a parked ask that captured `old` must not cancel `newer`.
	if eng.parkChatTurnIf("W1", "PA", old) {
		t.Fatal("parkChatTurnIf should no-op when expect is not the live turn")
	}
	if oldCancelled || newCancelled {
		t.Fatalf("cancelled old=%v new=%v, want neither", oldCancelled, newCancelled)
	}
	if !eng.parkChatTurnIf("W1", "PA", newer) {
		t.Fatal("parkChatTurnIf should cancel the matching live turn")
	}
	if !newCancelled {
		t.Fatal("expected newer turn cancelled")
	}
}

func TestTimedOutFinalizeNotStoppedByUser(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	addSoloWorkspace(t, st, "W1")
	turn := eng.registerChatTurn("W1", "PA", func() {})
	if turn == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	turn.mu.Lock()
	turn.timedOut = true
	turn.mu.Unlock()
	out := eng.finalizeChatReply("W1", "PA", "hi", "", "", Result{}, context.Canceled, "Done.", time.Time{}, false, "")
	if out != chatTurnTimedOutReply {
		t.Fatalf("timed-out finalize = %q, want %q", out, chatTurnTimedOutReply)
	}
	if out == chatStoppedReply {
		t.Fatalf("timed-out finalize reported as user stop: %q", out)
	}
}

func TestClearAskUsersForWorkspace(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	_ = eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: "W1", AgentCode: "PA", Question: "gone?",
	}, nil)
	_ = eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: "W2", AgentCode: "PA", Question: "keep?",
	}, nil)
	eng.clearAskUsersForWorkspace("W1")
	pending := eng.PendingAskUsers()
	if len(pending) != 1 || pending[0].WorkspaceID != "W2" {
		t.Fatalf("pending after clear = %+v", pending)
	}
}

func TestResumeAfterAskUserClearsReservation(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ask-resume-clear", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	q := eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: ws.ID, AgentCode: "PA", Question: "color?",
	}, nil)
	got, _, ok := eng.askUsers.Answer(q.ID, domain.UserQuestionAnswer{Choice: "Blue"})
	if !ok {
		t.Fatal("Answer returned false")
	}
	if !eng.askUsers.workspaceBusy(ws.ID) {
		t.Fatal("resume reservation missing after Answer")
	}
	eng.resumeAfterAskUser(got, domain.UserQuestionAnswer{Choice: "Blue"}, nil)
	if eng.askUsers.workspaceBusy(ws.ID) {
		t.Fatal("resume reservation leaked after resumeAfterAskUser")
	}
}
