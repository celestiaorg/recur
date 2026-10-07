package engine

import (
	"strings"
	"sync"
	"testing"

	"recur/internal/domain"
)

func TestParseChatInboxID(t *testing.T) {
	ws, code, ok := ParseChatInboxID(chatInboxID("demo.golden-burrito", "PA2"))
	if !ok || ws != "demo.golden-burrito" || code != "PA2" {
		t.Fatalf("PA2: ws=%q code=%q ok=%v", ws, code, ok)
	}
	ws, code, ok = ParseChatInboxID(chatInboxID("LOOP-2218", "MA"))
	if !ok || ws != "LOOP-2218" || code != "MA" {
		t.Fatalf("MA: ws=%q code=%q ok=%v", ws, code, ok)
	}
	if _, _, ok = ParseChatInboxID("ib-ask-x"); ok {
		t.Fatal("non-chat id should not parse")
	}
	if _, _, ok = ParseChatInboxID("ib-chat-only"); ok {
		t.Fatal("missing code should not parse")
	}
	if _, _, ok = ParseChatInboxID("ib-chat-ws-CO"); ok {
		t.Fatal("specialist code should not parse")
	}
}

func TestSyncChatInboxHealsFromUnseen(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "sync-inbox", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)

	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) { a.Unseen = true })
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("inbox row should not exist before SyncChatInbox")
	}
	eng.SyncChatInbox()
	if it, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); !ok || !it.Unread || it.Kind != "chat" {
		t.Fatalf("SyncChatInbox missing row: %+v ok=%v", it, ok)
	}

	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) { a.Unseen = false })
	eng.SyncChatInbox()
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("SyncChatInbox left a stale chat row")
	}
}

func TestSyncChatInboxDoesNotInferLoopFinished(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "sync-settled", "")
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
	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) { a.Unseen = true })
	eng.SyncChatInbox()
	it, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode))
	if !ok {
		t.Fatal("SyncChatInbox missing row")
	}
	if !strings.Contains(it.Title, "finished a reply") {
		t.Fatalf("SyncChatInbox title = %q, want finished a reply (not Loop finished)", it.Title)
	}
}

func TestEmitChatInboxSkipsPendingAskUser(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ask-skip", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	q := eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: ws.ID, AgentCode: pairAgentCode, Question: "which one?",
	}, nil)
	eng.updateAgentStatus(ws.ID, pairAgentCode, "working")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("chat inbox row should not stack on a parked ask_user")
	}
	if _, ok := inboxByID(eng, "ib-ask-"+q.ID); !ok {
		t.Fatal("ask_user inbox row should still be present")
	}
}

func TestMarkAllChatsSeenClearsInbox(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "mark-all", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.ensurePairAgent(ws.ID, "PA2", "")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "working")
	eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
	eng.updateAgentStatus(ws.ID, "PA2", "working")
	eng.updateAgentStatus(ws.ID, "PA2", "idle")
	if st.UnseenChatCount() != 2 {
		t.Fatalf("UnseenChatCount=%d, want 2", st.UnseenChatCount())
	}
	inboxBefore := len(sink.byName("inbox.updated"))
	attBefore := len(sink.byName("attention.updated"))
	eng.MarkAllChatsSeen()
	if st.UnseenChatCount() != 0 {
		t.Fatalf("UnseenChatCount after MarkAllChatsSeen = %d, want 0", st.UnseenChatCount())
	}
	for _, it := range st.Inbox() {
		if it.Kind == "chat" {
			t.Fatalf("chat inbox row still present: %+v", it)
		}
	}
	if got := len(sink.byName("inbox.updated")) - inboxBefore; got != 1 {
		t.Errorf("MarkAllChatsSeen inbox.updated count = %d, want 1", got)
	}
	if got := len(sink.byName("attention.updated")) - attBefore; got != 1 {
		t.Errorf("MarkAllChatsSeen attention.updated count = %d, want 1", got)
	}
	last := sink.byName("attention.updated")
	data, _ := last[len(last)-1].Data.(map[string]any)
	if data["clearAllUnseen"] != true {
		t.Errorf("MarkAllChatsSeen attention.updated clearAllUnseen = %v, want true", data["clearAllUnseen"])
	}
}

func TestUpdateAgentStatusLastEventMatchesStore(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "status-mark-all-race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	for i := 0; i < 50; i++ {
		eng.SetWorkspaceChatFocus("", "")
		st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) {
			a.Unseen = false
			a.Status = "working"
		})
		start := make(chan struct{})
		var wg sync.WaitGroup
		wg.Add(2)
		go func() {
			defer wg.Done()
			<-start
			eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
		}()
		go func() {
			defer wg.Done()
			<-start
			eng.MarkAllChatsSeen()
		}()
		close(start)
		wg.Wait()
		a, ok := st.Agent(ws.ID, pairAgentCode)
		if !ok {
			t.Fatal("agent missing")
		}
		var lastUnseen *bool
		for _, ev := range sink.byName("attention.updated") {
			d, _ := ev.Data.(map[string]any)
			if d["clearAllUnseen"] == true {
				u := false
				lastUnseen = &u
				continue
			}
			if d["code"] != pairAgentCode || d["workspaceId"] != ws.ID {
				continue
			}
			u, _ := d["tabUnseen"].(bool)
			lastUnseen = &u
		}
		if !a.Unseen && lastUnseen != nil && *lastUnseen {
			t.Fatalf("iter %d: store Unseen=false but last event tabUnseen=true", i)
		}
	}
}

func TestUpdateAgentStatusFocusLastEventMatchesStore(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "status-focus-race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	for i := 0; i < 50; i++ {
		eng.SetWorkspaceChatFocus("", "")
		st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) {
			a.Unseen = false
			a.Status = "working"
		})
		start := make(chan struct{})
		var wg sync.WaitGroup
		wg.Add(2)
		go func() {
			defer wg.Done()
			<-start
			eng.updateAgentStatus(ws.ID, pairAgentCode, "idle")
		}()
		go func() {
			defer wg.Done()
			<-start
			eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)
		}()
		close(start)
		wg.Wait()
		a, ok := st.Agent(ws.ID, pairAgentCode)
		if !ok {
			t.Fatal("agent missing")
		}
		if a.Unseen {
			t.Fatalf("iter %d: focused tab left unseen", i)
		}
		var last *bool
		for _, ev := range sink.byName("attention.updated") {
			d, _ := ev.Data.(map[string]any)
			if d["code"] != pairAgentCode || d["workspaceId"] != ws.ID {
				continue
			}
			u, _ := d["tabUnseen"].(bool)
			last = &u
		}
		if last != nil && *last {
			t.Fatalf("iter %d: last tabUnseen=true after mark-seen, store Unseen=%v", i, a.Unseen)
		}
	}
}

func TestMarkAllChatsSeenLastEventMatchesStore(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "mark-all-race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	for i := 0; i < 50; i++ {
		eng.SetWorkspaceChatFocus("", "")
		st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) { a.Unseen = true })
		start := make(chan struct{})
		var wg sync.WaitGroup
		wg.Add(2)
		go func() {
			defer wg.Done()
			<-start
			eng.flagChatUnseen(ws.ID, pairAgentCode, false)
		}()
		go func() {
			defer wg.Done()
			<-start
			eng.MarkAllChatsSeen()
		}()
		close(start)
		wg.Wait()
		a, ok := st.Agent(ws.ID, pairAgentCode)
		if !ok {
			t.Fatal("agent missing")
		}
		var lastUnseen *bool
		for _, ev := range sink.byName("attention.updated") {
			d, _ := ev.Data.(map[string]any)
			if d["clearAllUnseen"] == true {
				u := false
				lastUnseen = &u
				continue
			}
			if d["code"] != pairAgentCode || d["workspaceId"] != ws.ID {
				continue
			}
			u, _ := d["tabUnseen"].(bool)
			lastUnseen = &u
		}
		if !a.Unseen && lastUnseen != nil && *lastUnseen {
			t.Fatalf("iter %d: store Unseen=false but last event tabUnseen=true", i)
		}
	}
}

func TestCloseWorkspaceChatPublishesAttentionWithoutInboxRow(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "close-unseen", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.ensurePairAgent(ws.ID, "PA2", "")
	_ = eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: ws.ID, AgentCode: "PA2", Question: "which?",
	}, nil)
	st.UpdateAgent(ws.ID, "PA2", func(a *domain.Agent) { a.Unseen = true })
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, "PA2")); ok {
		t.Fatal("ask_user park should suppress the chat inbox row")
	}
	before := len(sink.byName("attention.updated"))
	_ = eng.CloseWorkspaceChat(ws.ID, "PA2")
	if got := len(sink.byName("attention.updated")) - before; got != 1 {
		t.Errorf("CloseWorkspaceChat attention.updated count = %d, want 1 (Unseen tab, no inbox row)", got)
	}
}

func TestMarkWorkspaceChatSeenRemovesOrphanRow(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "orphan-row", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	id := chatInboxID(ws.ID, pairAgentCode)
	st.AddInboxItem(domain.InboxItem{ID: id, Kind: "chat", Unread: true, Target: chatInboxTarget(ws.ID, pairAgentCode)})
	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) { a.Unseen = false })
	eng.MarkWorkspaceChatSeen(ws.ID, pairAgentCode)
	if _, ok := inboxByID(eng, id); ok {
		t.Fatal("MarkWorkspaceChatSeen left an orphan chat row (Unseen already false)")
	}

	st.AddInboxItem(domain.InboxItem{ID: id, Kind: "chat", Unread: true, Target: chatInboxTarget(ws.ID, pairAgentCode)})
	st.DeleteAgent(ws.ID, pairAgentCode)
	eng.MarkWorkspaceChatSeen(ws.ID, pairAgentCode)
	if _, ok := inboxByID(eng, id); ok {
		t.Fatal("MarkWorkspaceChatSeen left an orphan chat row (agent deleted)")
	}
}

func TestCloseWorkspaceChatClearsInboxRow(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "close-inbox", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	eng.ensurePairAgent(ws.ID, "PA2", "")
	eng.updateAgentStatus(ws.ID, "PA2", "working")
	eng.updateAgentStatus(ws.ID, "PA2", "idle")
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, "PA2")); !ok {
		t.Fatal("expected PA2 inbox row before CloseWorkspaceChat")
	}
	_ = eng.CloseWorkspaceChat(ws.ID, "PA2")
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, "PA2")); ok {
		t.Fatal("CloseWorkspaceChat left the PA2 inbox row")
	}
}

func TestEmitChatInboxDeclinesWhenSeen(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "emit-seen", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) { a.Unseen = false })
	if eng.emitChatInbox(ws.ID, pairAgentCode, false) {
		t.Fatal("emitChatInbox() = true, want false when Unseen is already clear")
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("emitChatInbox wrote a row for a seen tab")
	}
}

func TestSyncChatInboxSkipsPendingAskWithoutSpuriousPublish(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "sync-ask", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	_ = eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: ws.ID, AgentCode: pairAgentCode, Question: "which?",
	}, nil)
	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) { a.Unseen = true })
	before := len(sink.byName("inbox.updated"))
	eng.SyncChatInbox()
	if got := len(sink.byName("inbox.updated")) - before; got != 0 {
		t.Errorf("SyncChatInbox inbox.updated count = %d, want 0 when emit declines", got)
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("SyncChatInbox created a chat row on top of a parked ask_user")
	}
}

func TestMarkAllChatsSeenDropsOrphanRows(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	st.AddInboxItem(domain.InboxItem{ID: "ib-chat-ghost-PA2", Kind: "chat", Unread: true, Target: "chat:ghost:PA2"})
	st.AddInboxItem(domain.InboxItem{ID: "ib-keep", Kind: "pause", Unread: true, Target: "workspace:x"})
	eng.MarkAllChatsSeen()
	if _, ok := inboxByID(eng, "ib-chat-ghost-PA2"); ok {
		t.Fatal("MarkAllChatsSeen left an orphan chat row")
	}
	if _, ok := inboxByID(eng, "ib-keep"); !ok {
		t.Fatal("MarkAllChatsSeen removed a non-chat inbox row")
	}
}

func TestPRChatHostUnseenLandsInInbox(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	pr := domain.PR{
		ID: "gh-host-inbox", ProjectID: projID, Number: 88, Source: domain.SourceGitHub,
		Title: "Imported checkout", Branch: "feat/in", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	_ = eng.ListWorkspaceChats(host.ID)

	eng.updateAgentStatus(host.ID, pairAgentCode, "working")
	eng.updateAgentStatus(host.ID, pairAgentCode, "idle")

	it, ok := inboxByID(eng, chatInboxID(host.ID, pairAgentCode))
	if !ok || !it.Unread || it.Kind != "chat" {
		t.Fatalf("host unseen inbox row: %+v ok=%v", it, ok)
	}
	if it.Target != chatInboxTarget(host.ID, pairAgentCode) {
		t.Errorf("target = %q", it.Target)
	}
	if !strings.Contains(it.Sub, "#88") {
		t.Errorf("sub = %q, want PR number", it.Sub)
	}
	if !sink.has("inbox.updated") {
		t.Fatal("expected inbox.updated")
	}
	c := eng.AttentionCounts()
	if c.Unseen != 1 || c.ByWorkspace[host.ID] != 1 {
		t.Fatalf("counts %+v", c)
	}
	if c.ByProject[projID] != 0 {
		t.Errorf("ByProject[%s]=%d, want 0 (host has no sidebar row)", projID, c.ByProject[projID])
	}
}
