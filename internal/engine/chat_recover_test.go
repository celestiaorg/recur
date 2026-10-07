package engine

import (
	"context"
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func persistWorkingChat(t *testing.T, eng *Engine, wsID, code string) domain.Session {
	t.Helper()
	eng.ensurePairAgent(wsID, code, "sonnet")
	eng.Store.UpdateAgent(wsID, code, func(a *domain.Agent) {
		a.Status = "working"
	})
	sess, ok := eng.ensureAgentSession(wsID, string(domain.RoleCoder), code, "Agent", domain.ProviderClaudeCode)
	if !ok {
		t.Fatal("ensureAgentSession")
	}
	msgID, err := eng.Store.CreateMessage(sess.ID, domain.Message{
		Role:         "assistant",
		StreamStatus: domain.StreamStreaming,
	})
	if err != nil {
		t.Fatalf("CreateMessage: %v", err)
	}
	if err := eng.Store.AppendContentBlock(msgID, domain.ToolUseBlock(
		"tu-crash", "toolu_crash", "Bash", json.RawMessage(`{"command":"sleep 999"}`),
	)); err != nil {
		t.Fatalf("AppendContentBlock: %v", err)
	}
	if err := eng.Store.SetSessionStatus(sess.ID, "streaming"); err != nil {
		t.Fatalf("SetSessionStatus: %v", err)
	}
	return sess
}

func persistDirectingMaestro(t *testing.T, eng *Engine, wsID string) {
	t.Helper()
	eng.ensureMaestroAgent(wsID, "goal")
	eng.Store.UpdateAgent(wsID, maestroAgentCode, func(a *domain.Agent) {
		a.Status = "directing"
	})
	if _, ok := eng.ensureAgentSession(wsID, string(domain.RoleMaestro), maestroAgentCode, "Maestro", domain.ProviderClaudeCode); !ok {
		t.Fatal("ensureAgentSession")
	}
}

func TestCancelChatHealsZombieWorkingAgent(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "zombie-stop", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)

	if !eng.CancelChat(ws.ID, pairAgentCode) {
		t.Fatal("CancelChat returned false; zombie working agent should heal")
	}
	a, ok := st.Agent(ws.ID, pairAgentCode)
	if !ok || a.Status != "idle" {
		t.Fatalf("agent status=%q, want idle", a.Status)
	}
	items := eng.WorkspaceChatItems(ws.ID, pairAgentCode)
	var sawStop, sawPending bool
	for _, it := range items {
		if it.Kind == "text" && it.Text == chatStoppedReply {
			sawStop = true
		}
		if it.Kind == "tool" && it.ResultPending {
			sawPending = true
		}
	}
	if !sawStop {
		t.Fatalf("transcript=%+v, want %q", items, chatStoppedReply)
	}
	if sawPending {
		t.Fatalf("zombie heal left a spinning tool: %+v", items)
	}
}

func TestRecoverIdlesInterruptedChatAndClosesStream(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "crash-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	sess := persistWorkingChat(t, eng, ws.ID, pairAgentCode)

	eng.Recover()

	a, ok := st.Agent(ws.ID, pairAgentCode)
	if !ok || a.Status != "idle" {
		t.Fatalf("after Recover status=%q, want idle", a.Status)
	}
	reloaded, ok := eng.Store.GetSession(sess.ID)
	if !ok || reloaded.Status != "idle" {
		t.Fatalf("session status=%q, want idle", reloaded.Status)
	}
	items := eng.WorkspaceChatItems(ws.ID, pairAgentCode)
	var sawNote, sawPending bool
	for _, it := range items {
		if it.Kind == "text" && it.Text == chatInterruptedReply {
			sawNote = true
		}
		if it.Kind == "tool" && it.ResultPending {
			sawPending = true
		}
	}
	if !sawNote {
		t.Fatalf("transcript=%+v, want interrupted note", items)
	}
	if sawPending {
		t.Fatalf("Recover left a spinning tool: %+v", items)
	}
	if len(eng.interruptedChats) != 1 || eng.interruptedChats[0].code != pairAgentCode {
		t.Fatalf("interruptedChats=%+v, want [PA]", eng.interruptedChats)
	}
}

func TestAutoResumeContinuesInterruptedChat(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture("sess-resume", "picked up.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "resume-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)
	const cursorResume = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b"
	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) {
		a.SessionID = cursorResume
	})
	eng.RememberWorkspaceChatSelection(ws.ID, pairAgentCode, "cursor/composer-2.5",
		map[string]string{"effort": "high"}, "agent", nil)

	eng.Recover()
	eng.AutoResume()

	waitFor(t, 5*time.Second, "auto-resume chat turn to run", func() bool {
		spec, ok := runner.specWithRole(domain.RoleCoder)
		return ok && strings.Contains(spec.Prompt, "Recur restarted")
	})
	spec, _ := runner.specWithRole(domain.RoleCoder)
	if spec.ResumeSession != cursorResume {
		t.Errorf("ResumeSession=%q, want %s (restart resumes a usable session id; it must not inject a transcript)", spec.ResumeSession, cursorResume)
	}
	if strings.Contains(spec.Prompt, "<transcript>") {
		t.Errorf("AutoResume must --resume, not inject a fork transcript: %q", spec.Prompt)
	}
	if spec.DisplayText != chatResumeDisplay {
		t.Errorf("DisplayText=%q, want %q", spec.DisplayText, chatResumeDisplay)
	}
	if spec.SkipRecordUser {
		t.Errorf("SkipRecordUser = true, want false (no open turn to reuse)")
	}
	if !strings.Contains(spec.Model, "composer") {
		t.Errorf("Model=%q, want persisted cursor composer, not the Settings default", spec.Model)
	}
	waitFor(t, 3*time.Second, "resumed chat to go idle", func() bool {
		a, ok := st.Agent(ws.ID, pairAgentCode)
		return ok && a.Status == "idle"
	})
}

func TestChatResumeDoesNotCancelLiveTurn(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-live", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "resume-idle", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	turn := eng.registerChatTurn(ws.ID, pairAgentCode, cancel)
	if turn == nil {
		t.Fatal("registerChatTurn")
	}
	reply := eng.SendWorkspaceMessage(ws.ID, pairAgentCode, chatResumePrompt, "sonnet", nil, nil, "", chatResumeDisplay)
	if reply != "" {
		t.Errorf("SendWorkspaceMessage(resume) reply = %q, want empty so a live user turn is not displaced", reply)
	}
	if got := eng.liveChatTurn(ws.ID, pairAgentCode); got != turn {
		t.Error("liveChatTurn was displaced by a restart remount")
	}
	select {
	case <-ctx.Done():
		t.Fatal("restart remount cancelled the live user turn")
	default:
	}
}

func TestAutoResumeDropsForeignCursorIdOnClaude(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			return maestroFixture("sess-resume", "picked up.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "resume-mismatch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)
	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) {
		a.SessionID = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b"
	})
	eng.RememberWorkspaceChatSelection(ws.ID, pairAgentCode, "sonnet", nil, "agent", nil)

	eng.Recover()
	eng.AutoResume()

	waitFor(t, 5*time.Second, "auto-resume chat turn to run", func() bool {
		spec, ok := runner.specWithRole(domain.RoleCoder)
		return ok && strings.Contains(spec.Prompt, "Recur restarted")
	})
	spec, _ := runner.specWithRole(domain.RoleCoder)
	if spec.ResumeSession != "" {
		t.Errorf("ResumeSession=%q, want empty (Cursor agent- id cannot resume on Claude)", spec.ResumeSession)
	}
	if !strings.Contains(spec.Prompt, "<transcript>") {
		t.Errorf("foreign resume id should inject history, prompt=%q", spec.Prompt)
	}
}

func TestAutoResumeContinuesMaestroOnSettledLoop(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			return maestroFixture("sess-ma-settled", "continuing.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "settled-ma", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	persistDirectingMaestro(t, eng, ws.ID)
	if _, ok := st.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
		lp.RunStarted = true
		lp.State = domain.StatePROpen
		lp.Phase = "ready"
	}); !ok {
		t.Fatal("UpdateWorkspace")
	}

	eng.Recover()
	if len(eng.interruptedChats) != 1 || eng.interruptedChats[0].code != maestroAgentCode {
		t.Fatalf("interruptedChats=%+v, want [MA]", eng.interruptedChats)
	}
	eng.AutoResume()
	waitFor(t, 5*time.Second, "maestro auto-resume turn to run", func() bool {
		spec, ok := runner.specWithRole(domain.RoleMaestro)
		return ok && strings.Contains(spec.Prompt, "Recur restarted")
	})
	spec, _ := runner.specWithRole(domain.RoleMaestro)
	if spec.DisplayText != chatResumeDisplay {
		t.Errorf("DisplayText=%q, want %q", spec.DisplayText, chatResumeDisplay)
	}
}

func TestMaestroResumeNeedsLoop(t *testing.T) {
	cases := []struct {
		name string
		ws   domain.Workspace
		want bool
	}{
		{name: "queued", ws: domain.Workspace{State: domain.StateQueued}, want: true},
		{name: "coding", ws: domain.Workspace{State: domain.StateCoding}, want: true},
		{name: "paused interrupted", ws: domain.Workspace{State: domain.StatePaused, Phase: phaseInterrupted}, want: true},
		{name: "paused by you", ws: domain.Workspace{State: domain.StatePaused, Phase: "paused by you"}, want: true},
		{name: "answered waiting", ws: domain.Workspace{State: domain.StatePaused, Phase: "answered — waiting to resume"}, want: true},
		{name: "escalated", ws: domain.Workspace{State: domain.StateEscalated}, want: true},
		{name: "retryable failed", ws: domain.Workspace{State: domain.StateFailed, Retryable: true}, want: true},
		{name: "pr_open", ws: domain.Workspace{State: domain.StatePROpen}, want: false},
		{name: "merged", ws: domain.Workspace{State: domain.StateMerged}, want: false},
		{name: "stopped", ws: domain.Workspace{State: domain.StateStopped}, want: false},
		{name: "failed", ws: domain.Workspace{State: domain.StateFailed}, want: false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := maestroResumeNeedsLoop(tc.ws)
			if got != tc.want {
				t.Errorf("maestroResumeNeedsLoop(state=%s phase=%q retryable=%t) = %v, want %v",
					tc.ws.State, tc.ws.Phase, tc.ws.Retryable, got, tc.want)
			}
		})
	}
}

func TestResumeInterruptedMaestroSkipsParkedLoop(t *testing.T) {
	cases := []struct {
		name string
		prep func(t *testing.T, eng *Engine, wsID string)
	}{
		{
			name: "queued",
			prep: func(t *testing.T, eng *Engine, wsID string) {
				t.Helper()
				if _, ok := eng.Store.UpdateWorkspace(wsID, func(lp *domain.Workspace) {
					lp.RunStarted = true
					lp.State = domain.StateQueued
					lp.Phase = "waiting for a slot"
				}); !ok {
					t.Fatal("UpdateWorkspace")
				}
			},
		},
		{
			name: "escalated unanswered",
			prep: func(t *testing.T, eng *Engine, wsID string) {
				t.Helper()
				if _, ok := eng.Store.UpdateWorkspace(wsID, func(lp *domain.Workspace) {
					lp.RunStarted = true
					lp.State = domain.StateEscalated
					lp.Phase = "asking a question"
				}); !ok {
					t.Fatal("UpdateWorkspace")
				}
				eng.Store.AddEscalation(domain.Escalation{
					WorkspaceID: wsID, Question: "A or B?", Status: "open",
				})
			},
		},
		{
			name: "answered waiting to resume",
			prep: func(t *testing.T, eng *Engine, wsID string) {
				t.Helper()
				if _, ok := eng.Store.UpdateWorkspace(wsID, func(lp *domain.Workspace) {
					lp.RunStarted = true
					lp.State = domain.StateEscalated
					lp.Phase = "asking a question"
				}); !ok {
					t.Fatal("UpdateWorkspace")
				}
				eng.Store.AddEscalation(domain.Escalation{
					WorkspaceID: wsID, Question: "A or B?", Status: "answered", Answer: "A",
				})
			},
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			runner := &captureRunner{
				linesFor: func(int) [][]byte {
					return maestroFixture("sess-ma-parked", "should not run")
				},
			}
			eng, st, _, _ := newTestEngine(t, runner)
			projID := addProject(t, st, initRepo(t))
			ws, err := eng.CreateWorkspace(projID, "parked-ma", "")
			if err != nil {
				t.Fatalf("CreateWorkspace: %v", err)
			}
			persistDirectingMaestro(t, eng, ws.ID)
			tc.prep(t, eng, ws.ID)
			eng.Recover()
			eng.resumeInterruptedChat(ws.ID, maestroAgentCode)
			if specs := runner.specsWithRole(domain.RoleMaestro); len(specs) != 0 {
				t.Errorf("resumeInterruptedChat(MA, %s) ran %d maestro specs, want 0", tc.name, len(specs))
			}
		})
	}
}

func TestRecoverDoesNotQueueSpecialistAsChat(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "specialist", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-CO", WorkspaceID: ws.ID, Code: "CO", Name: "Coder",
		Status: "working",
	})

	eng.Recover()
	if len(eng.interruptedChats) != 0 {
		t.Fatalf("interruptedChats=%+v, specialists must not auto-resume as chats", eng.interruptedChats)
	}
	a, ok := st.Agent(ws.ID, "CO")
	if !ok || a.Status != "idle" {
		t.Fatalf("CO status=%q, want idle", a.Status)
	}
}

func TestShutdownLeavesChatWorkingForRecover(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ctxRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "quit-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, pairAgentCode, "long task", "sonnet", nil, nil, "", "")
	}()
	waitFor(t, 3*time.Second, "chat agent working", func() bool {
		a, ok := st.Agent(ws.ID, pairAgentCode)
		return ok && a.Status == "working"
	})

	eng.Shutdown()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("SendWorkspaceMessage did not return after Shutdown")
	}
	a, ok := st.Agent(ws.ID, pairAgentCode)
	if !ok || a.Status != "working" {
		t.Fatalf("after Shutdown status=%q, want working so Recover can resume", a.Status)
	}

	eng2 := New(st)
	t.Cleanup(eng2.Shutdown)
	eng2.SetRunner(&captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("sess-after", "ok")
	}})
	eng2.Recover()
	a, _ = st.Agent(ws.ID, pairAgentCode)
	if a.Status != "idle" {
		t.Fatalf("Recover after quit status=%q, want idle", a.Status)
	}
	if len(eng2.interruptedChats) != 1 {
		t.Fatalf("interruptedChats=%+v, want the quit chat", eng2.interruptedChats)
	}
}

func TestRecoverSkipsMissingWorktree(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "gone-tree", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)
	if err := os.RemoveAll(ws.WorktreePath); err != nil {
		t.Fatalf("RemoveAll worktree: %v", err)
	}

	eng.Recover()
	if len(eng.interruptedChats) != 0 {
		t.Fatalf("interruptedChats=%+v, missing worktree must not auto-resume", eng.interruptedChats)
	}
	a, ok := st.Agent(ws.ID, pairAgentCode)
	if !ok || a.Status != "idle" {
		t.Fatalf("status=%q, want idle even when the worktree is gone", a.Status)
	}
}

func TestResumeInterruptedChatSkipsLiveTurn(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ctxRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "live-skip", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)
	eng.Recover()

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, pairAgentCode, "user follow-up", "sonnet", nil, nil, "", "")
	}()
	waitFor(t, 3*time.Second, "user turn working", func() bool {
		a, ok := st.Agent(ws.ID, pairAgentCode)
		return ok && a.Status == "working"
	})

	eng.resumeInterruptedChat(ws.ID, pairAgentCode)
	if !eng.CancelChat(ws.ID, pairAgentCode) {
		t.Fatal("expected the user turn to still be registered")
	}
	select {
	case reply := <-done:
		if reply != chatStoppedReply {
			t.Fatalf("reply=%q, want user turn stopped (not replaced by auto-resume)", reply)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("user turn did not return after CancelChat")
	}
}

func TestResumeInterruptedChatSkipsClosedTab(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-x", "nope") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "closed-tab", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)
	eng.Recover()
	st.DeleteAgent(ws.ID, pairAgentCode)

	eng.AutoResume()
	time.Sleep(200 * time.Millisecond)
	if _, ok := runner.specWithRole(domain.RoleCoder); ok {
		t.Fatal("closed tab must not be resurrected by auto-resume")
	}
}

func TestRecoverClosesSpecialistPendingTools(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "specialist-tools", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-CO", WorkspaceID: ws.ID, Code: "CO", Name: "Coder",
		Status: "working",
	})
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), "CO", "Coder", domain.ProviderClaudeCode)
	if !ok {
		t.Fatal("ensureAgentSession CO")
	}
	msgID, err := eng.Store.CreateMessage(sess.ID, domain.Message{
		Role:         "assistant",
		StreamStatus: domain.StreamStreaming,
	})
	if err != nil {
		t.Fatalf("CreateMessage: %v", err)
	}
	if err := eng.Store.AppendContentBlock(msgID, domain.ToolUseBlock(
		"tu-co", "toolu_co", "Bash", json.RawMessage(`{"command":"sleep 999"}`),
	)); err != nil {
		t.Fatalf("AppendContentBlock: %v", err)
	}

	eng.Recover()
	if len(eng.interruptedChats) != 0 {
		t.Fatalf("interruptedChats=%+v, CO must not auto-resume as a chat", eng.interruptedChats)
	}
	items := flattenTranscript(mustTranscript(t, eng, sess.ID), "", nil)
	var sawPending bool
	var sawResult bool
	for _, it := range items {
		if it.Kind == "tool" && it.ResultPending {
			sawPending = true
		}
		if it.Kind == "tool" && it.Result != "" {
			sawResult = true
		}
	}
	if sawPending || !sawResult {
		t.Fatalf("specialist tools after Recover: pending=%v result=%v items=%+v", sawPending, sawResult, items)
	}
}

func TestShutdownCompletedTurnGoesIdle(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	eng, st, _, _ := newTestEngine(t, &holdAfterSuccess{started: started, release: release})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "finished-quit", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, pairAgentCode, "almost done", "sonnet", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("turn did not start")
	}

	eng.Shutdown()
	close(release)
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("SendWorkspaceMessage did not return after Shutdown")
	}
	a, ok := st.Agent(ws.ID, pairAgentCode)
	if !ok || a.Status != "idle" {
		t.Fatalf("completed turn during quit status=%q, want idle", a.Status)
	}
}

func TestAutoResumeDefersAgentChatWhenLoopResumes(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			return maestroFixture("sess-loop", "ok")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-and-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)
	eng.RememberWorkspaceChatSelection(ws.ID, pairAgentCode, "sonnet", nil, domain.ChatModeAgent, nil)
	if _, ok := st.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
		lp.RunStarted = true
		lp.State = domain.StateCoding
		lp.Phase = "iteration 1"
		lp.Iteration = 1
		lp.Checkpoint = &domain.Checkpoint{State: domain.StateCoding, Iteration: 1}
	}); !ok {
		t.Fatal("UpdateWorkspace")
	}

	eng.Recover()
	if len(eng.interruptedChats) != 1 {
		t.Fatalf("interruptedChats=%+v, want the PA tab queued", eng.interruptedChats)
	}
	eng.AutoResume()
	waitFor(t, 8*time.Second, "loop to leave paused/interrupted", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && !(got.State == domain.StatePaused && got.Phase == phaseInterrupted)
	})
	time.Sleep(200 * time.Millisecond)
	for _, spec := range runner.specsWithRole(domain.RoleCoder) {
		if strings.Contains(spec.Prompt, "Recur restarted") || spec.DisplayText == chatResumeDisplay {
			t.Fatalf("agent-mode PA must not auto-resume into a worktree the loop owns: %+v", spec)
		}
	}
	a, ok := st.Agent(ws.ID, pairAgentCode)
	if !ok {
		t.Fatal("Agent(PA) missing after AutoResume")
	}
	if a.Status != "idle" {
		t.Errorf("Agent(PA).Status = %q, want idle (run-lock skip, not a refused send)", a.Status)
	}
}

func TestChatResumeBlockedByRunLock(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &captureRunner{})
	cases := []struct {
		name string
		ws   domain.Workspace
		mode string
		want bool
	}{
		{
			name: "agent paused interrupted",
			ws:   domain.Workspace{RunStarted: true, State: domain.StatePaused, Phase: phaseInterrupted},
			mode: domain.ChatModeAgent,
			want: true,
		},
		{
			name: "agent coding",
			ws:   domain.Workspace{RunStarted: true, State: domain.StateCoding},
			mode: domain.ChatModeAgent,
			want: true,
		},
		{
			name: "agent queued",
			ws:   domain.Workspace{RunStarted: true, State: domain.StateQueued},
			mode: domain.ChatModeAgent,
			want: true,
		},
		{
			name: "ask paused interrupted",
			ws:   domain.Workspace{RunStarted: true, State: domain.StatePaused, Phase: phaseInterrupted},
			mode: domain.ChatModeAsk,
			want: false,
		},
		{
			name: "agent no run",
			ws:   domain.Workspace{Type: domain.WorkspaceSolo, State: domain.StatePaused, Phase: phaseInterrupted},
			mode: domain.ChatModeAgent,
			want: false,
		},
		{
			name: "agent human paused",
			ws:   domain.Workspace{RunStarted: true, State: domain.StatePaused, Phase: "paused by you"},
			mode: domain.ChatModeAgent,
			want: false,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := eng.chatResumeBlockedByRunLock(tc.ws, tc.mode)
			if got != tc.want {
				t.Errorf("chatResumeBlockedByRunLock(state=%s phase=%q mode=%s) = %v, want %v",
					tc.ws.State, tc.ws.Phase, tc.mode, got, tc.want)
			}
		})
	}
}

type holdAfterSuccess struct {
	started chan struct{}
	release chan struct{}
}

func (r *holdAfterSuccess) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	select {
	case <-r.started:
	default:
		close(r.started)
	}
	<-r.release
	return Result{Text: "done"}, nil
}

func mustTranscript(t *testing.T, eng *Engine, sessionID string) []domain.Message {
	t.Helper()
	msgs, err := eng.Store.SessionTranscript(sessionID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	return msgs
}
