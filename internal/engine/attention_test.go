package engine

import (
	"errors"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
)

// TestUnseenChatAttention: a background PA turn finishing raises unseen and an
// inbox row; focusing the tab clears both; the focused tab finishing does not
// raise unseen. Dock rollup is unseen + non-chat inbox unread.
func TestUnseenChatAttention(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "attention", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID) // ensures default PA tab
	eng.ensurePairAgent(ws.ID, "PA2", "")

	// Background tab finishes → unseen.
	eng.updateAgentStatus(ws.ID, "PA2", "working")
	eng.updateAgentStatus(ws.ID, "PA2", "idle")
	if a, ok := st.Agent(ws.ID, "PA2"); !ok || !a.Unseen {
		t.Fatalf("PA2 after idle: unseen=%v ok=%v, want true", a.Unseen, ok)
	}
	if got := st.UnseenChatCount(); got != 1 {
		t.Fatalf("UnseenChatCount=%d, want 1", got)
	}
	if !sink.has("attention.updated") {
		t.Fatalf("expected attention.updated, got names from sink")
	}
	if !sink.has("inbox.updated") {
		t.Fatalf("expected inbox.updated for unseen chat row, got names from sink")
	}
	if it, ok := inboxByID(eng, chatInboxID(ws.ID, "PA2")); !ok || !it.Unread || it.Kind != "chat" {
		t.Fatalf("unseen PA2 inbox row: %+v ok=%v", it, ok)
	}

	counts := eng.AttentionCounts()
	if counts.Unseen != 1 || counts.ByWorkspace[ws.ID] != 1 || counts.ByProject[projID] != 1 {
		t.Fatalf("AttentionCounts=%+v", counts)
	}

	// Focus clears unseen.
	eng.SetWorkspaceChatFocus(ws.ID, "PA2")
	if a, _ := st.Agent(ws.ID, "PA2"); a.Unseen {
		t.Fatal("PA2 still unseen after focus")
	}
	if st.UnseenChatCount() != 0 {
		t.Fatalf("UnseenChatCount after focus = %d", st.UnseenChatCount())
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, "PA2")); ok {
		t.Fatal("PA2 inbox row still present after focus")
	}

	// Focused tab finishing does not raise unseen.
	eng.updateAgentStatus(ws.ID, "PA2", "working")
	eng.updateAgentStatus(ws.ID, "PA2", "idle")
	if a, _ := st.Agent(ws.ID, "PA2"); a.Unseen {
		t.Fatal("focused PA2 should not become unseen on idle")
	}

	// Specialist idle must never set unseen.
	st.UpsertAgent(ws.ID, domain.Agent{ID: ws.ID + "-CO", WorkspaceID: ws.ID, Code: "CO", Status: "working"})
	eng.updateAgentStatus(ws.ID, "CO", "idle")
	if a, _ := st.Agent(ws.ID, "CO"); a.Unseen {
		t.Fatal("CO specialist must not set Unseen")
	}

	// Dock = unseen + non-chat inbox unread (chat row is the unseen tab).
	st.MarkAllInboxRead()
	st.AddInboxItem(domain.InboxItem{ID: "ib-att", Group: "needs", Title: "needs you", Unread: true, Time: "now"})
	eng.SetWorkspaceChatFocus(ws.ID, "PA2") // PA is background
	eng.updateAgentStatus(ws.ID, pairAgentCode, "working")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
	c := eng.AttentionCounts()
	if c.InboxUnread != 2 || c.Unseen != 1 || c.Dock != 2 {
		t.Fatalf("dock rollup: %+v (want inbox=2 unseen=1 dock=2)", c)
	}
	if it, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); !ok || it.Target != chatInboxTarget(ws.ID, pairAgentCode) {
		t.Fatalf("PA inbox row: %+v ok=%v", it, ok)
	}

	tabs := eng.ListWorkspaceChats(ws.ID)
	var paUnseen, pa2Unseen bool
	var foundPA, foundPA2 bool
	for _, tab := range tabs {
		switch tab.Code {
		case pairAgentCode:
			foundPA = true
			paUnseen = tab.Unseen
		case "PA2":
			foundPA2 = true
			pa2Unseen = tab.Unseen
		}
	}
	if !foundPA || !paUnseen {
		t.Fatalf("ListWorkspaceChats PA unseen=%v found=%v", paUnseen, foundPA)
	}
	if !foundPA2 || pa2Unseen {
		t.Fatalf("ListWorkspaceChats PA2 unseen=%v found=%v (should be cleared)", pa2Unseen, foundPA2)
	}

	inboxBefore := len(st.Inbox())
	eng.updateAgentStatus(ws.ID, "PA2", "working")
	eng.SetWorkspaceChatFocus("", "") // leave so PA2 can go unseen
	eng.updateAgentStatus(ws.ID, "PA2", "idle")
	if len(st.Inbox()) != inboxBefore+1 {
		t.Fatalf("unseen chat must create an inbox row: before=%d after=%d", inboxBefore, len(st.Inbox()))
	}
}

// TestUnseenChatAttentionSurface: selected-tab finishes still raise unseen when
// the OS window is unfocused or a modal occludes the chat; restoring visibility
// marks the selected tab seen (D-attention).
func TestUnseenChatAttentionSurface(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "surface", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)

	// App unfocused → finish on selected tab raises unseen.
	eng.SetAppFocused(false)
	eng.updateAgentStatus(ws.ID, pairAgentCode, "working")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
	if a, ok := st.Agent(ws.ID, pairAgentCode); !ok || !a.Unseen {
		t.Fatalf("SetAppFocused(false) idle: unseen=%v ok=%v, want true", a.Unseen, ok)
	}
	if got := eng.AttentionCounts().Dock; got != 1 {
		t.Fatalf("AttentionCounts().Dock after unfocused idle = %d, want 1", got)
	}

	// Refocus clears unseen for the still-selected tab.
	eng.SetAppFocused(true)
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("SetAppFocused(true) should mark selected tab seen")
	}
	if got := eng.AttentionCounts().Dock; got != 0 {
		t.Fatalf("AttentionCounts().Dock after refocus = %d, want 0", got)
	}

	// UI occlusion (e.g. New workspace dialog) → same as unfocused.
	eng.SetChatUIOccluded(true)
	eng.updateAgentStatus(ws.ID, pairAgentCode, "working")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
	if a, _ := st.Agent(ws.ID, pairAgentCode); !a.Unseen {
		t.Fatal("SetChatUIOccluded(true) idle: want unseen")
	}

	eng.SetChatUIOccluded(false)
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("SetChatUIOccluded(false) should mark selected tab seen")
	}

	// SetWorkspaceChatFocus must not clear unseen while occluded.
	eng.SetChatUIOccluded(true)
	eng.updateAgentStatus(ws.ID, pairAgentCode, "working")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
	eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)
	if a, _ := st.Agent(ws.ID, pairAgentCode); !a.Unseen {
		t.Fatal("SetWorkspaceChatFocus while occluded must not clear unseen")
	}
	eng.SetChatUIOccluded(false)
}

// TestLoopFinishedRaisesUnseen: a walk that settles while the human is not
// looking marks the conductor tab unseen and upserts a "Loop finished" inbox
// row (workspace/project badges + inbox). Stop and composer-Stop do not.
func TestLoopFinishedRaisesUnseen(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-done", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.stampChatLoop(ws.ID, pairAgentCode, "", "", "", "", true)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.LoopChatCode = pairAgentCode
	})

	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: pairAgentCode, eng: eng,
		exitState: domain.StatePaused, exitPhase: "idle",
	})

	if a, ok := st.Agent(ws.ID, pairAgentCode); !ok || !a.Unseen {
		t.Fatalf("PA after loop finish: unseen=%v ok=%v, want true", a.Unseen, ok)
	}
	it, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode))
	if !ok || !it.Unread || it.Kind != "chat" {
		t.Fatalf("loop-finished inbox: %+v ok=%v", it, ok)
	}
	if !strings.Contains(it.Title, "Loop") || !strings.Contains(it.Title, "finished") {
		t.Fatalf("inbox title = %q, want Loop finished", it.Title)
	}
	if strings.Contains(it.Title, "reply") {
		t.Fatalf("inbox title = %q, should not say finished a reply", it.Title)
	}
	c := eng.AttentionCounts()
	if c.Unseen != 1 || c.ByWorkspace[ws.ID] != 1 || c.ByProject[projID] != 1 {
		t.Fatalf("AttentionCounts=%+v", c)
	}
	if !sink.has("attention.updated") || !sink.has("inbox.updated") {
		t.Fatal("expected attention.updated and inbox.updated")
	}

	eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("focus should clear loop-finished unseen")
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("inbox row still present after focus")
	}
}

func TestLoopFinishedSkipsStopAndIdleCancel(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-stop", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)

	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.LoopChatCode = pairAgentCode
	})
	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: pairAgentCode, eng: eng,
		exitState: domain.StateStopped, exitPhase: "stopped",
	})
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("stopped loop must not raise unseen")
	}

	lp := &Loop{
		ID: ws.ID, ChatCode: pairAgentCode, eng: eng,
		exitState: domain.StatePaused, exitPhase: "idle",
	}
	lp.requestIdle()
	eng.maybeFlagLoopFinished(lp)
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("composer Stop idle must not raise unseen")
	}

	stopped := &Loop{
		ID: ws.ID, ChatCode: pairAgentCode, eng: eng,
		exitState: domain.StatePROpen, exitPhase: "PR open — ready for your review",
	}
	stopped.mu.Lock()
	stopped.stopped = true
	stopped.mu.Unlock()
	eng.maybeFlagLoopFinished(stopped)
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("CmdStop after settle must not raise unseen")
	}

	eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)
	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: pairAgentCode, eng: eng,
		exitState: domain.StatePROpen, exitPhase: "PR open — ready for your review",
	})
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("focused tab must not raise unseen on PR-open")
	}
}

func TestLoopFinishedUsesWalkExitNotWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-sibling", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)

	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.State = domain.StateCoding
		w.Phase = "implementing"
	})
	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: pairAgentCode, eng: eng,
		exitState: domain.StatePROpen, exitPhase: "PR open — ready for your review",
	})
	if a, ok := st.Agent(ws.ID, pairAgentCode); !ok || !a.Unseen {
		t.Fatal("settled walk must badge even if a sibling overwrote workspace state")
	}
	it, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode))
	if !ok || !strings.Contains(it.Title, "Loop") || strings.Contains(it.Title, "reply") {
		t.Fatalf("inbox title = %q, want Loop finished", it.Title)
	}

	eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.State = domain.StatePaused
		w.Phase = "idle"
	})
	eng.SetWorkspaceChatFocus("", "")
	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: pairAgentCode, eng: eng,
		exitState: domain.StateCoding, exitPhase: "implementing",
	})
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("in-flight walk must not badge from a sibling's settled workspace state")
	}
}

func TestLoopFinishedVersusCloseLeavesNoOrphanInbox(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-close-race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = maestroAgentCode
	})
	eng.SetWorkspaceChatFocus("", "")
	for i := 0; i < 50; i++ {
		eng.ensureMaestroAgentWithStatus(ws.ID, ws.Goal, "idle")
		st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
			w.LoopChatCode = maestroAgentCode
		})
		st.UpdateAgent(ws.ID, maestroAgentCode, func(a *domain.Agent) { a.Unseen = false })
		eng.clearChatInbox(ws.ID, maestroAgentCode, false)
		start := make(chan struct{})
		var wg sync.WaitGroup
		wg.Add(2)
		go func() {
			defer wg.Done()
			<-start
			eng.maybeFlagLoopFinished(&Loop{
				ID: ws.ID, ChatCode: maestroAgentCode, eng: eng,
				exitState: domain.StatePaused, exitPhase: "idle",
			})
		}()
		go func() {
			defer wg.Done()
			<-start
			_ = eng.CloseWorkspaceChat(ws.ID, maestroAgentCode)
		}()
		close(start)
		wg.Wait()
		if _, ok := st.Agent(ws.ID, maestroAgentCode); ok {
			continue
		}
		if _, ok := inboxByID(eng, chatInboxID(ws.ID, maestroAgentCode)); ok {
			t.Fatalf("iter %d: closed MA left an inbox row", i)
		}
	}
}

func TestLoopFinishedDoesNotRecreateClosedTab(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-closed", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	st.DeleteAgent(ws.ID, pairAgentCode)

	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: pairAgentCode, eng: eng,
		exitState: domain.StatePaused, exitPhase: "idle",
	})
	if _, ok := st.Agent(ws.ID, pairAgentCode); ok {
		t.Fatal("maybeFlagLoopFinished must not recreate a closed conductor tab")
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("closed tab must not get an inbox row")
	}
}

func TestLoopFinishedLegacyEmptyChatCode(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "legacy-ma-done", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = maestroAgentCode
	})
	eng.ensureMaestroAgentWithStatus(ws.ID, ws.Goal, "idle")
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.LoopChatCode = ""
	})

	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: "", eng: eng,
		exitState: domain.StatePaused, exitPhase: "idle",
	})
	if a, ok := st.Agent(ws.ID, maestroAgentCode); !ok || !a.Unseen {
		t.Fatalf("legacy MA after loop finish: unseen=%v ok=%v, want true", a.Unseen, ok)
	}
	it, ok := inboxByID(eng, chatInboxID(ws.ID, maestroAgentCode))
	if !ok || !strings.Contains(it.Title, "Loop") || strings.Contains(it.Title, "reply") {
		t.Fatalf("legacy MA inbox: %+v ok=%v", it, ok)
	}
}

func TestLoopFinishedLegacyEmptyDoesNotFollowPAOwner(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "legacy-ma-pa-owner", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	eng.ensureMaestroAgentWithStatus(ws.ID, ws.Goal, "idle")
	_ = eng.ListWorkspaceChats(ws.ID)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = pairAgentCode
	})

	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: "", eng: eng,
		exitState: domain.StatePaused, exitPhase: "idle",
	})
	if a, ok := st.Agent(ws.ID, maestroAgentCode); !ok || !a.Unseen {
		t.Fatalf("legacy MA after loop finish: unseen=%v ok=%v, want true", a.Unseen, ok)
	}
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("legacy MA walk must not badge the later PA owner")
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("PA must not get a Loop finished row for a legacy MA walk")
	}
	it, ok := inboxByID(eng, chatInboxID(ws.ID, maestroAgentCode))
	if !ok || !strings.Contains(it.Title, "Loop") || strings.Contains(it.Title, "reply") {
		t.Fatalf("legacy MA inbox: %+v ok=%v", it, ok)
	}

	eng.SetWorkspaceChatFocus(ws.ID, maestroAgentCode)
	st.DeleteAgent(ws.ID, maestroAgentCode)
	eng.SetWorkspaceChatFocus("", "")
	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: "", eng: eng,
		exitState: domain.StatePaused, exitPhase: "idle",
	})
	if _, ok := st.Agent(ws.ID, maestroAgentCode); ok {
		t.Fatal("closed legacy MA must not be recreated")
	}
	if a, _ := st.Agent(ws.ID, pairAgentCode); a.Unseen {
		t.Fatal("closed legacy MA must not badge PA")
	}
}

func TestLoopFinishedLegacyEmptyClosedMA(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "legacy-ma-closed", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.DeleteAgent(ws.ID, maestroAgentCode)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.LoopChatCode = ""
		w.RunStarted = true
	})

	eng.maybeFlagLoopFinished(&Loop{
		ID: ws.ID, ChatCode: "", eng: eng,
		exitState: domain.StatePaused, exitPhase: "idle",
	})
	if _, ok := st.Agent(ws.ID, maestroAgentCode); ok {
		t.Fatal("closed legacy MA must not be recreated")
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, maestroAgentCode)); ok {
		t.Fatal("closed legacy MA must not get an inbox row")
	}
}

func TestDispatchDoesNotRecreateClosedMaestro(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "closed-ma-dispatch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = maestroAgentCode
		w.State = domain.StateQueued
	})
	eng.ensureMaestroAgentWithStatus(ws.ID, ws.Goal, "idle")
	stale, ok := st.Workspace(ws.ID)
	if !ok || stale.LoopChatCode != maestroAgentCode {
		t.Fatal("need a pre-close snapshot that still owns MA")
	}
	_ = eng.CloseWorkspaceChat(ws.ID, maestroAgentCode)
	if _, ok := st.Agent(ws.ID, maestroAgentCode); ok {
		t.Fatal("MA should be gone after close")
	}
	if rec, ok := st.Workspace(ws.ID); !ok || rec.LoopChatCode != "" {
		t.Fatalf("LoopChatCode after close = %q, want empty", rec.LoopChatCode)
	}
	if !eng.gov.TryAcquireLoop() {
		t.Fatal("loop slot")
	}
	if err := eng.dispatch(stale); !errors.Is(err, errLoopOwnerGone) {
		t.Fatalf("dispatch = %v, want owner gone", err)
	}
	eng.gov.ReleaseLoop()
	if _, running := eng.loops.Load(ws.ID); running {
		t.Fatal("closed conductor must not run")
	}
	if _, ok := st.Agent(ws.ID, maestroAgentCode); ok {
		t.Fatal("dispatch must not recreate a closed MA tab")
	}
	if rec, ok := st.Workspace(ws.ID); !ok || rec.LoopChatCode != "" {
		t.Fatalf("LoopChatCode after dispatch = %q, want empty", rec.LoopChatCode)
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, maestroAgentCode)); ok {
		t.Fatal("closed MA must not get an inbox row")
	}
}

func TestDispatchDoesNotRunClosedQueuedPair(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "closed-pa-dispatch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = pairAgentCode
		w.State = domain.StateQueued
	})
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	stale, ok := st.Workspace(ws.ID)
	if !ok || stale.LoopChatCode != pairAgentCode {
		t.Fatal("need a pre-close snapshot that still owns PA")
	}
	_ = eng.CloseWorkspaceChat(ws.ID, pairAgentCode)
	if rec, ok := st.Workspace(ws.ID); !ok || rec.LoopChatCode != "" {
		t.Fatalf("LoopChatCode after close = %q, want empty", rec.LoopChatCode)
	}
	if rec, _ := st.Workspace(ws.ID); rec.State == domain.StateQueued {
		t.Fatal("close of a queued conductor must not leave it queued")
	}
	if !eng.gov.TryAcquireLoop() {
		t.Fatal("loop slot")
	}
	if err := eng.dispatch(stale); !errors.Is(err, errLoopOwnerGone) {
		t.Fatalf("dispatch = %v, want owner gone", err)
	}
	eng.gov.ReleaseLoop()
	if _, running := eng.loops.Load(ws.ID); running {
		t.Fatal("closed PA must not run")
	}
}

func TestDispatchDoesNotRunDeletedPairAgent(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "restored-pa-dispatch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = pairAgentCode
		w.State = domain.StatePaused
	})
	st.DeleteAgent(ws.ID, pairAgentCode)
	stale, ok := st.Workspace(ws.ID)
	if !ok || stale.LoopChatCode != pairAgentCode {
		t.Fatal("need LoopChatCode still naming the deleted PA")
	}
	if !eng.gov.TryAcquireLoop() {
		t.Fatal("loop slot")
	}
	if err := eng.dispatch(stale); !errors.Is(err, errLoopOwnerGone) {
		t.Fatalf("dispatch = %v, want owner gone", err)
	}
	eng.gov.ReleaseLoop()
	if _, running := eng.loops.Load(ws.ID); running {
		t.Fatal("deleted PA must not run")
	}
}

func TestUpsertDoesNotResurrectMAWhenPAOwns(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "pa-owns-closed-ma", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = maestroAgentCode
	})
	eng.ensureMaestroAgentWithStatus(ws.ID, ws.Goal, "idle")
	eng.ensurePairAgent(ws.ID, pairAgentCode, "")
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleMaestro), maestroAgentCode, "Maestro", "")
	if !ok {
		t.Fatal("ensureAgentSession")
	}
	newSessionMirror(st, sess.ID, "", sess.Provider).recordUser("", "old MA turn", "", nil)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.LoopChatCode = pairAgentCode
	})
	_ = eng.CloseWorkspaceChat(ws.ID, maestroAgentCode)
	if _, ok := st.Agent(ws.ID, maestroAgentCode); ok {
		t.Fatal("MA should be gone after close")
	}

	eng.upsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-" + maestroAgentCode, WorkspaceID: ws.ID, Code: maestroAgentCode,
		Name: "Maestro", Status: "planning",
	})
	if _, ok := st.Agent(ws.ID, maestroAgentCode); ok {
		t.Fatal("planner upsert must not recreate a closed MA while PA owns")
	}
	for _, tab := range eng.ListWorkspaceChats(ws.ID) {
		if tab.Code == maestroAgentCode {
			t.Fatal("closed MA must not reappear in the strip")
		}
	}
}

func TestOrdinaryReplyOnSettledLoopIsNotLoopFinished(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "loop-reply", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.stampChatLoop(ws.ID, pairAgentCode, "", "", "", "", true)
	st.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.State = domain.StatePaused
		w.Phase = "idle"
		w.LoopChatCode = pairAgentCode
	})
	eng.SetWorkspaceChatFocus("", "")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "working")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
	it, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode))
	if !ok {
		t.Fatal("expected inbox row for ordinary unseen reply")
	}
	if !strings.Contains(it.Title, "finished a reply") {
		t.Fatalf("inbox title = %q, want finished a reply", it.Title)
	}
}

func TestDocsLoopFinishedRaisesUnseen(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "Write the README", "", "", "test.loops.docs", "test.policies.fail")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 15*time.Second, "docs loop unseen inbox", func() bool {
		a, ok := st.Agent(id, maestroAgentCode)
		if !ok || !a.Unseen {
			return false
		}
		it, iok := inboxByID(eng, chatInboxID(id, maestroAgentCode))
		return iok && it.Unread && strings.Contains(it.Title, "Loop") && strings.Contains(it.Title, "finished")
	})
	it, ok := inboxByID(eng, chatInboxID(id, maestroAgentCode))
	if !ok || !it.Unread || !strings.Contains(it.Title, "Loop") || !strings.Contains(it.Title, "finished") {
		t.Fatalf("docs loop inbox: %+v ok=%v", it, ok)
	}
	c := eng.AttentionCounts()
	if c.Unseen < 1 || c.ByWorkspace[id] < 1 || c.ByProject[projID] < 1 {
		t.Fatalf("docs loop AttentionCounts=%+v", c)
	}
}
