package engine

import (
	"encoding/base64"
	"encoding/json"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestForkWorkspaceChatCopiesTranscriptAndIsolatesSends(t *testing.T) {
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "first question", "sonnet", nil, nil, "", "")
	firstTurn := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if firstTurn == "" {
		t.Fatal("first turn footer missing message id")
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "second question", "sonnet", nil, nil, "", "")

	parentItems := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if len(parentItems) < 4 {
		t.Fatalf("parent transcript: %+v", parentItems)
	}

	tab := eng.ForkWorkspaceChat(ws.ID, "PA", firstTurn)
	if tab.Code != "PA2" {
		t.Fatalf("fork tab = %+v, want PA2", tab)
	}
	if tab.Title == "" || tab.Title == "New chat" {
		t.Errorf("forked tab should keep a title, got %q", tab.Title)
	}

	childItems := contentItems(eng.WorkspaceChatItems(ws.ID, "PA2"))
	if len(childItems) != 2 {
		t.Fatalf("child should have the first turn only, got %+v", childItems)
	}
	if childItems[0].Text != "first question" {
		t.Fatalf("child user text = %q", childItems[0].Text)
	}

	child := eng.ForkWorkspaceChat(ws.ID, "PA", "no-such-message")
	if child.Code != "" {
		t.Fatalf("unknown fork point should return empty tab, got %+v", child)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "branch from first", "sonnet", nil, nil, "", "")
	after := contentItems(eng.WorkspaceChatItems(ws.ID, "PA2"))
	if lastUser(after) != "branch from first" {
		t.Fatalf("child follow-up missing: %+v", after)
	}
	parentAfter := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if lastUser(parentAfter) != "second question" {
		t.Fatalf("parent should be unchanged, last user = %q", lastUser(parentAfter))
	}

	specs := runner.specsWithAgentID(ws.ID + "-PA2")
	if len(specs) != 1 {
		t.Fatalf("PA2 turns = %d, want 1", len(specs))
	}
	// Mid-history fork: no provider resume, history injected into the prompt.
	if specs[0].ResumeSession != "" || specs[0].ForkSession {
		t.Fatalf("mid-history fork should inject, not resume: resume=%q fork=%v", specs[0].ResumeSession, specs[0].ForkSession)
	}
	if !strings.Contains(specs[0].Prompt, "<transcript>") || !strings.Contains(specs[0].Prompt, `<message role="user">`) {
		t.Fatalf("injected prompt missing structured transcript: %q", specs[0].Prompt)
	}
	if !strings.Contains(specs[0].Prompt, "first question") || !strings.Contains(specs[0].Prompt, "branch from first") {
		t.Fatalf("injected prompt missing history or new text: %q", specs[0].Prompt)
	}
	if specs[0].DisplayText != "branch from first" {
		t.Fatalf("DisplayText = %q, want the user's new message", specs[0].DisplayText)
	}
}

func TestForkWorkspaceChatTipInjectsHistory(t *testing.T) {
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-tip", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "only turn", "sonnet", nil, nil, "", "")
	tip := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if tip == "" {
		t.Fatal("missing tip message id")
	}
	parentSess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || parentSess.Provider != domain.ProviderClaudeCode {
		t.Fatalf("parent session: %+v ok=%v", parentSess, ok)
	}

	tab := eng.ForkWorkspaceChat(ws.ID, "PA", tip)
	if tab.Code != "PA2" {
		t.Fatalf("fork tab = %+v", tab)
	}
	childSess, ok := st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok {
		t.Fatal("child session missing")
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(childSess.ConfigJSON), &cfg)
	if !cfg.ForkInject {
		t.Fatalf("tip fork should set forkInject, cfg=%+v", cfg)
	}
	if a, ok := st.Agent(ws.ID, "PA2"); ok && a.SessionID != "" {
		t.Fatalf("fork must not snapshot parent resume id, agent=%+v", a)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "continue", "sonnet", nil, nil, "", "")
	specs := runner.specsWithAgentID(ws.ID + "-PA2")
	if len(specs) != 1 {
		t.Fatalf("PA2 turns = %d", len(specs))
	}
	if specs[0].ResumeSession != "" || specs[0].ForkSession {
		t.Fatalf("tip fork should inject, not resume: resume=%q fork=%v", specs[0].ResumeSession, specs[0].ForkSession)
	}
	if !strings.Contains(specs[0].Prompt, "<transcript>") || !strings.Contains(specs[0].Prompt, "only turn") {
		t.Fatalf("tip fork should inject history: %q", specs[0].Prompt)
	}

	childSess, ok = st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok {
		t.Fatal("child session gone after send")
	}
	cfg = agentConfig{}
	_ = json.Unmarshal([]byte(childSess.ConfigJSON), &cfg)
	if cfg.ForkInject {
		t.Fatalf("forkInject should clear after the first send carried the preamble; cfg=%s", childSess.ConfigJSON)
	}
}

func TestForkCursorSessionToClaudeInjectsHistory(t *testing.T) {
	// Defense in depth: a Cursor agent-<uuid> on Agent.SessionID must never
	// be snapshotted for Claude --resume, even if the session row still says
	// claude-code (the old create-time default).
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-cursor-to-claude", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "only turn", "sonnet", nil, nil, "", "")
	tip := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if tip == "" {
		t.Fatal("missing tip message id")
	}
	const cursorAgentID = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b"
	eng.setAgentSession(ws.ID, "PA", cursorAgentID)

	tab := eng.ForkWorkspaceChat(ws.ID, "PA", tip)
	if tab.Code != "PA2" {
		t.Fatalf("fork tab = %+v", tab)
	}
	if a, ok := st.Agent(ws.ID, "PA2"); ok && a.SessionID == cursorAgentID {
		t.Fatalf("tip fork must not snapshot a Cursor agent id: %q", a.SessionID)
	}
	childSess, ok := st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok {
		t.Fatal("child session missing")
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(childSess.ConfigJSON), &cfg)
	if !cfg.ForkInject {
		t.Fatalf("fork should set forkInject, cfg=%+v", cfg)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "continue on claude", "sonnet", nil, nil, "", "")
	specs := runner.specsWithAgentID(ws.ID + "-PA2")
	if len(specs) != 1 {
		t.Fatalf("PA2 turns = %d", len(specs))
	}
	if specs[0].ResumeSession != "" || specs[0].ForkSession {
		t.Fatalf("cursor→claude fork should inject, not resume: resume=%q fork=%v", specs[0].ResumeSession, specs[0].ForkSession)
	}
	if !strings.Contains(specs[0].Prompt, "<transcript>") || !strings.Contains(specs[0].Prompt, "only turn") {
		t.Fatalf("injected prompt missing parent history: %q", specs[0].Prompt)
	}
	if strings.Contains(specs[0].Prompt, cursorAgentID) {
		t.Fatalf("prompt leaked cursor agent id: %q", specs[0].Prompt)
	}
}

func TestRetryAfterCursorSwitchInjectsInsteadOfResuming(t *testing.T) {
	// A Cursor→Claude dock switch leaves Agent.SessionID = agent-<uuid> (it is
	// deliberately not cleared, so AutoResume can --resume a usable id). Hitting
	// Retry then must not hand that id to claude -p --resume, which crashes with
	// "not a UUID and does not match any session title". Instead the retry must
	// drop the foreign id AND replay the transcript so Claude keeps the context.
	// The retry path used to return before sanitizing or injecting.
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "retry-after-switch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "sonnet", nil, nil, "", "")
	const cursorAgentID = "agent-6a931db4-d388-4fb7-afc3-b97178f2b82b"
	eng.setAgentSession(ws.ID, "PA", cursorAgentID)

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "", "sonnet", nil, nil, "", chatRetryDisplaySentinel)
	specs := runner.specsWithAgentID(ws.ID + "-PA")
	if len(specs) < 2 {
		t.Fatalf("expected first send + retry specs, got %d", len(specs))
	}
	last := specs[len(specs)-1]
	if last.ResumeSession != "" {
		t.Fatalf("retry after cursor→claude switch must not resume the cursor id: %q", last.ResumeSession)
	}
	if strings.Contains(last.Prompt, cursorAgentID) {
		t.Fatalf("retry prompt leaked cursor agent id: %q", last.Prompt)
	}
	if !strings.Contains(last.Prompt, "<transcript>") || !strings.Contains(last.Prompt, "do the thing") {
		t.Fatalf("retry on the new provider must replay context, got prompt: %q", last.Prompt)
	}
	if a, ok := st.Agent(ws.ID, "PA"); ok && a.SessionID == cursorAgentID {
		t.Fatalf("retry must clear the stale cursor agent id, still %q", a.SessionID)
	}
}

func TestRetryAfterCodexSwitchDropsUUIDResume(t *testing.T) {
	// Codex↔Claude share the UUID id shape, so resumeFitsProvider cannot flag a
	// Codex id as foreign to Claude — the dock switch marks it stale via
	// ForkInject. A retry must honor that flag: drop the Codex UUID (Claude would
	// resume the wrong/nonexistent session) and replay the transcript instead.
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "retry-after-codex", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "do the thing", "sonnet", nil, nil, "", "")
	// A Codex thread id: UUID-shaped, indistinguishable from a Claude id.
	const codexThreadID = "7b1c9f20-2c1a-4d3e-9a11-0f6e2b8c4d5a"
	eng.setAgentSession(ws.ID, "PA", codexThreadID)
	// The switch marks the id stale (rememberPairChatSelection does this on a
	// real Codex→Claude dock change; set the flag directly here).
	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("session missing")
	}
	_ = st.PatchSessionConfigMerge(sess.ID, `{"forkInject":true}`)

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "", "sonnet", nil, nil, "", chatRetryDisplaySentinel)
	specs := runner.specsWithAgentID(ws.ID + "-PA")
	last := specs[len(specs)-1]
	if last.ResumeSession != "" {
		t.Fatalf("retry after codex→claude switch must not resume the codex id: %q", last.ResumeSession)
	}
	if !strings.Contains(last.Prompt, "<transcript>") || !strings.Contains(last.Prompt, "do the thing") {
		t.Fatalf("retry on the new provider must replay context, got prompt: %q", last.Prompt)
	}
}

func TestRetryOfInjectedWireKeepsStdinAndConsumesFlag(t *testing.T) {
	// A fork/provider-switch send injects the transcript, then fails without a
	// session id, so ForkInject stays armed. Retry replays that already-injected
	// wire. It must NOT prepend a second preamble, but must still ride stdin and
	// report it carries the preamble — so once a session id lands the one-shot
	// flag is consumed and the next ordinary send resumes instead of replaying.
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-x", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "retry-injected", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "only turn", "sonnet", nil, nil, "", "")
	tip := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if tip == "" {
		t.Fatal("missing tip")
	}
	if tab := eng.ForkWorkspaceChat(ws.ID, "PA", tip); tab.Code != "PA2" {
		t.Fatalf("fork = %+v", tab)
	}

	first := &AgentSpec{Prompt: "continue", Model: "sonnet"}
	if !eng.applyChatFork(ws.ID, "PA2", first, false) {
		t.Fatal("first send should carry the preamble")
	}
	if !strings.HasPrefix(first.Prompt, forkPreambleLead) {
		t.Fatalf("first prompt should open with the preamble: %q", first.Prompt)
	}

	// The turn failed (no session id) — ForkInject stays armed. Retry replays the
	// injected wire verbatim.
	retry := &AgentSpec{Prompt: first.Prompt, Model: "sonnet"}
	if !eng.applyChatFork(ws.ID, "PA2", retry, false) {
		t.Fatal("retry of an injected wire must report it carries the preamble")
	}
	if strings.Count(retry.Prompt, forkPreambleLead) != 1 {
		t.Fatalf("retry must not nest the preamble: %q", retry.Prompt)
	}
	if !retry.PromptOnStdin {
		t.Fatal("retry of a (possibly huge) injected wire must ride stdin, not argv")
	}

	// A session id lands: finishChatFork consumes the one-shot flag.
	eng.finishChatFork(ws.ID, "PA2", true, Result{SessionID: "sess-x"})
	child, ok := st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok {
		t.Fatal("child session gone")
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(child.ConfigJSON), &cfg)
	if cfg.ForkInject {
		t.Fatal("a successful retry must consume forkInject")
	}

	// The next ordinary send resumes the new session instead of replaying again.
	next := &AgentSpec{Prompt: "next", Model: "sonnet", ResumeSession: "sess-x"}
	if eng.applyChatFork(ws.ID, "PA2", next, false) {
		t.Fatal("next send should resume, not inject")
	}
	if next.ResumeSession != "sess-x" {
		t.Fatalf("next send must keep resuming the valid session: %q", next.ResumeSession)
	}
	if strings.Contains(next.Prompt, forkPreambleLead) {
		t.Fatalf("next send must not replay history: %q", next.Prompt)
	}
}

func TestForkChatImageSendHidesInjectedTranscript(t *testing.T) {
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-image", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "look at this screenshot", "sonnet", nil, nil, "", "")
	parent, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("parent session missing")
	}
	if _, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "", "Read", []byte(`{"path":"attachments.go"}`)),
		},
	}); err != nil {
		t.Fatalf("append tool_use: %v", err)
	}
	if _, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.ToolResultBlock("t1", `var re = "⟦attachment:other⟧"`, false),
		},
	}); err != nil {
		t.Fatalf("append tool_result: %v", err)
	}
	asst, err := st.AppendMessage(parent.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("done reading")},
	})
	if err != nil {
		t.Fatalf("append assistant: %v", err)
	}

	tab := eng.ForkWorkspaceChat(ws.ID, "PA", asst.ID)
	if tab.Code != "PA2" {
		t.Fatalf("fork tab = %+v, want PA2", tab)
	}

	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	marker := AttachmentMarker("shot1")
	composer := "this is better but still needs work " + marker + " cues from the inbox"
	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", composer, "sonnet", nil, []domain.ChatAttachment{{
		ID: "shot1", Name: "shot.png", Mime: "image/png", Data: png,
	}}, "", "")

	after := contentItems(eng.WorkspaceChatItems(ws.ID, "PA2"))
	got := lastUser(after)
	if strings.Contains(got, forkPreambleLead) || strings.Contains(got, "<transcript>") ||
		strings.Contains(got, "tool_use") || strings.Contains(got, "attachment:other") {
		t.Fatalf("forked user bubble leaked injected transcript: %q", got)
	}
	if !strings.Contains(got, "this is better but still needs work") || !strings.Contains(got, "cues from the inbox") {
		t.Fatalf("forked user bubble missing composer: %q", got)
	}
}

// TestForkCursorChatThenSwitchToClaudeInjectsHistory is the user repro:
// Cursor chat → fork → switch the forked tab to Claude → send. The first
// chat must store provider=cursor; the fork inherits that; Claude's first
// send injects the transcript instead of --resume on a Cursor agent- id.
func TestForkCursorChatThenSwitchToClaudeInjectsHistory(t *testing.T) {
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-cursor-then-claude", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "only turn", "cursor/composer-2.5", nil, nil, "", "")
	parent, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("parent session missing")
	}
	if parent.Provider != domain.ProviderCursor {
		t.Fatalf("SendWorkspaceMessage(cursor/composer-2.5) session.Provider = %q, want %q",
			parent.Provider, domain.ProviderCursor)
	}
	tip := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if tip == "" {
		t.Fatal("missing tip message id")
	}
	const cursorAgentID = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b"
	eng.setAgentSession(ws.ID, "PA", cursorAgentID)

	tab := eng.ForkWorkspaceChat(ws.ID, "PA", tip)
	if tab.Code != "PA2" {
		t.Fatalf("ForkWorkspaceChat = %+v, want PA2", tab)
	}
	child, ok := st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok {
		t.Fatal("child session missing")
	}
	if child.Provider != domain.ProviderCursor {
		t.Errorf("forked session.Provider = %q, want %q (copied from Cursor parent)",
			child.Provider, domain.ProviderCursor)
	}
	if a, ok := st.Agent(ws.ID, "PA2"); ok && a.SessionID == cursorAgentID {
		t.Fatalf("tip fork must not snapshot a Cursor agent id: %q", a.SessionID)
	}

	eng.RememberWorkspaceChatSelection(ws.ID, "PA2", "sonnet", nil, "agent", nil)
	child, _ = st.SessionForLoopAgent(ws.ID, "PA2")
	if child.Provider != domain.ProviderClaudeCode {
		t.Errorf("RememberWorkspaceChatSelection(sonnet) on fork session.Provider = %q, want %q",
			child.Provider, domain.ProviderClaudeCode)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "continue on claude", "sonnet", nil, nil, "", "")
	specs := runner.specsWithAgentID(ws.ID + "-PA2")
	if len(specs) != 1 {
		t.Fatalf("PA2 turns = %d, want 1", len(specs))
	}
	if specs[0].ResumeSession != "" || specs[0].ForkSession {
		t.Fatalf("cursor→claude fork should inject, not resume: resume=%q fork=%v",
			specs[0].ResumeSession, specs[0].ForkSession)
	}
	if !strings.Contains(specs[0].Prompt, "<transcript>") || !strings.Contains(specs[0].Prompt, "only turn") {
		t.Fatalf("injected prompt missing parent history: %q", specs[0].Prompt)
	}
}

func TestForkHistoryPreambleIncludesToolResults(t *testing.T) {
	got := forkHistoryPreamble([]domain.Message{
		{Role: "user", Content: []domain.ContentBlock{domain.TextBlock("hello")}},
		{Role: "assistant", Content: []domain.ContentBlock{
			domain.ThinkingBlock("look at the tree", "", 0),
			domain.ToolUseBlock("t1", "", "Bash", []byte(`{"command":"ls"}`)),
		}},
		{Role: "user", Content: []domain.ContentBlock{domain.ToolResultBlock("t1", "huge diff", false)}},
		{Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("hi back")}},
	})
	if !strings.Contains(got, "<transcript>") {
		t.Fatalf("preamble missing <transcript>: %q", got)
	}
	if !strings.Contains(got, "hello") || !strings.Contains(got, "hi back") {
		t.Fatalf("preamble missing chat text: %q", got)
	}
	if !strings.Contains(got, `<tool_use name="Bash">`) || !strings.Contains(got, `<result><![CDATA[huge diff]]></result>`) {
		t.Fatalf("preamble should nest tool result inside use: %q", got)
	}
	if strings.Contains(got, `id=`) || strings.Contains(got, `tool_use_id=`) || strings.Contains(got, `<tool_result`) {
		t.Fatalf("preamble leaked pairing ids: %q", got)
	}
	if strings.Contains(got, "<thinking") || strings.Contains(got, "look at the tree") {
		t.Fatalf("preamble should omit thinking: %q", got)
	}
}

func TestForkHistoryPreambleUsesLastCompactSummary(t *testing.T) {
	got := forkHistoryPreamble([]domain.Message{
		{Role: "user", Content: []domain.ContentBlock{domain.TextBlock("ancient prompt")}},
		{Role: "assistant", Content: []domain.ContentBlock{
			domain.ToolUseBlock("t0", "", "Read", []byte(`{"path":"roles.go"}`)),
		}},
		{Role: "user", Content: []domain.ContentBlock{domain.ToolResultBlock("t0", strings.Repeat("old file ", 200), false)}},
		{Role: "assistant", Content: []domain.ContentBlock{
			domain.TextBlock("done with the old file"),
			domain.CompactBlock(domain.CompactDone, "auto", "older turns folded into a short brief", 800_000),
		}},
		{Role: "user", Content: []domain.ContentBlock{domain.TextBlock("keep going")}},
		{Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("post-compact reply")}},
	})
	if !strings.Contains(got, "older turns folded into a short brief") {
		t.Fatalf("forkHistoryPreamble() missing compact summary: %q", got)
	}
	if !strings.Contains(got, "keep going") || !strings.Contains(got, "post-compact reply") {
		t.Fatalf("forkHistoryPreamble() dropped turns after compact: %q", got)
	}
	if strings.Contains(got, "ancient prompt") || strings.Contains(got, "old file") ||
		strings.Contains(got, "done with the old file") {
		t.Fatalf("forkHistoryPreamble() kept pre-compact history: %q", got)
	}
}

func TestForkHistoryPreambleUsesSessionContext(t *testing.T) {
	watermark := "msg-compact"
	got := forkHistoryPreambleCtx([]domain.Message{
		{ID: "msg-old", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("ancient prompt")}},
		{ID: watermark, Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("done with the old file")}},
		{ID: "msg-live", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("keep going")}},
		{ID: "msg-asst", Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("post-compact reply")}},
	}, &domain.SessionContext{
		Summary:                "older turns folded into a short brief",
		LastCompactedMessageID: watermark,
	})
	if !strings.Contains(got, "older turns folded into a short brief") {
		t.Fatalf("forkHistoryPreambleCtx() missing session_context summary: %q", got)
	}
	if !strings.Contains(got, "keep going") || !strings.Contains(got, "post-compact reply") {
		t.Fatalf("forkHistoryPreambleCtx() dropped turns after watermark: %q", got)
	}
	if strings.Contains(got, "ancient prompt") || strings.Contains(got, "done with the old file") {
		t.Fatalf("forkHistoryPreambleCtx() kept pre-compact history: %q", got)
	}
}

func TestForkHistoryPreambleUsesLastCompactInWatermarkMessage(t *testing.T) {
	watermark := "msg-compact"
	got := forkHistoryPreambleCtx([]domain.Message{
		{ID: "msg-old", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("ancient prompt")}},
		{ID: watermark, Role: "assistant", Content: []domain.ContentBlock{
			domain.TextBlock("after first compact"),
			domain.CompactBlock(domain.CompactDone, "auto", "first fold", 100_000),
			domain.TextBlock("covered by the later compact"),
			domain.CompactBlock(domain.CompactDone, "auto", "stale second chip", 200_000),
			domain.TextBlock("live tail in the same turn"),
		}},
		{ID: "msg-live", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("keep going")}},
	}, &domain.SessionContext{
		Summary:                "latest fold covering both",
		LastCompactedMessageID: watermark,
	})
	if !strings.Contains(got, "latest fold covering both") {
		t.Fatalf("forkHistoryPreambleCtx() missing latest session_context summary: %q", got)
	}
	if !strings.Contains(got, "live tail in the same turn") || !strings.Contains(got, "keep going") {
		t.Fatalf("forkHistoryPreambleCtx() dropped the post-latest-compact tail: %q", got)
	}
	if strings.Contains(got, "ancient prompt") || strings.Contains(got, "after first compact") ||
		strings.Contains(got, "covered by the later compact") || strings.Contains(got, "first fold") ||
		strings.Contains(got, "stale second chip") {
		t.Fatalf("forkHistoryPreambleCtx() kept pre-latest-compact history: %q", got)
	}
}

func TestForkHistoryPreambleKeepsTailWhenLaterCompactHasNoSummary(t *testing.T) {
	watermark := "msg-compact"
	got := forkHistoryPreambleCtx([]domain.Message{
		{ID: "msg-old", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("ancient prompt")}},
		{ID: watermark, Role: "assistant", Content: []domain.ContentBlock{
			domain.CompactBlock(domain.CompactDone, "auto", "first fold", 100_000),
			domain.TextBlock("still live after empty second compact"),
			domain.CompactBlock(domain.CompactDone, "auto", "", 200_000),
		}},
		{ID: "msg-live", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("keep going")}},
	}, &domain.SessionContext{
		Summary:                "first fold",
		LastCompactedMessageID: watermark,
	})
	if !strings.Contains(got, "first fold") {
		t.Fatalf("forkHistoryPreambleCtx() missing active summary: %q", got)
	}
	if !strings.Contains(got, "still live after empty second compact") || !strings.Contains(got, "keep going") {
		t.Fatalf("forkHistoryPreambleCtx() dropped content after the active summary: %q", got)
	}
	if strings.Contains(got, "ancient prompt") {
		t.Fatalf("forkHistoryPreambleCtx() kept pre-compact history: %q", got)
	}
}

func TestForkHistoryPreambleKeepsHistoryWhenCompactHasNoSummary(t *testing.T) {
	got := forkHistoryPreamble([]domain.Message{
		{Role: "user", Content: []domain.ContentBlock{domain.TextBlock("please read roles.go")}},
		{Role: "assistant", Content: []domain.ContentBlock{
			domain.CompactBlock(domain.CompactDone, "auto", "", 167219),
			domain.TextBlock("ok"),
		}},
	})
	if !strings.Contains(got, "please read roles.go") {
		t.Fatalf("forkHistoryPreamble() dropped history when compact had no summary: %q", got)
	}
}

func TestForkSendInjectsCompactSummaryNotPreCompactHistory(t *testing.T) {
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-compact", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "ancient prompt", "cursor/composer-2.5", nil, nil, "", "")
	parent, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("parent session missing")
	}
	if _, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.TextBlock("done with the old file"),
			domain.CompactBlock(domain.CompactDone, "auto", "older turns folded into a short brief", 800_000),
		},
	}); err != nil {
		t.Fatalf("AppendMessage: %v", err)
	}
	if _, err := st.AppendMessage(parent.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("keep going")},
	}); err != nil {
		t.Fatalf("AppendMessage keep going: %v", err)
	}
	post, err := st.AppendMessage(parent.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("post-compact reply")},
	})
	if err != nil {
		t.Fatalf("AppendMessage post-compact: %v", err)
	}

	tab := eng.ForkWorkspaceChat(ws.ID, "PA", post.ID)
	if tab.Code != "PA2" {
		t.Fatalf("ForkWorkspaceChat = %+v, want PA2", tab)
	}
	eng.RememberWorkspaceChatSelection(ws.ID, "PA2", "sonnet", nil, "agent", nil)
	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "continue on claude", "sonnet", nil, nil, "", "")
	specs := runner.specsWithAgentID(ws.ID + "-PA2")
	if len(specs) != 1 {
		t.Fatalf("PA2 turns = %d, want 1", len(specs))
	}
	prompt := specs[0].Prompt
	if !strings.Contains(prompt, "older turns folded into a short brief") {
		t.Fatalf("injected prompt missing compact summary: %q", prompt)
	}
	if !strings.Contains(prompt, "keep going") || !strings.Contains(prompt, "post-compact reply") {
		t.Fatalf("injected prompt dropped turns after compact: %q", prompt)
	}
	if strings.Contains(prompt, "ancient prompt") || strings.Contains(prompt, "done with the old file") {
		t.Fatalf("injected prompt kept pre-compact history: %q", prompt)
	}
}

func TestForkSendInjectsSessionContextNotPreCompactHistory(t *testing.T) {
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-session-context", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "ancient prompt", "cursor/composer-2.5", nil, nil, "", "")
	parent, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("parent session missing")
	}
	compacted, err := st.AppendMessage(parent.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("done with the old file")},
	})
	if err != nil {
		t.Fatalf("AppendMessage compacted: %v", err)
	}
	if _, err := st.WriteSessionContext(parent.ID, "older turns folded into a short brief", compacted.ID); err != nil {
		t.Fatalf("WriteSessionContext: %v", err)
	}
	if _, err := st.AppendMessage(parent.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("keep going")},
	}); err != nil {
		t.Fatalf("AppendMessage keep going: %v", err)
	}
	post, err := st.AppendMessage(parent.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("post-compact reply")},
	})
	if err != nil {
		t.Fatalf("AppendMessage post-compact: %v", err)
	}

	tab := eng.ForkWorkspaceChat(ws.ID, "PA", post.ID)
	if tab.Code != "PA2" {
		t.Fatalf("ForkWorkspaceChat = %+v, want PA2", tab)
	}
	eng.RememberWorkspaceChatSelection(ws.ID, "PA2", "sonnet", nil, "agent", nil)
	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "continue on claude", "sonnet", nil, nil, "", "")
	specs := runner.specsWithAgentID(ws.ID + "-PA2")
	if len(specs) != 1 {
		t.Fatalf("PA2 turns = %d, want 1", len(specs))
	}
	prompt := specs[0].Prompt
	if !strings.Contains(prompt, "older turns folded into a short brief") {
		t.Fatalf("injected prompt missing session_context summary: %q", prompt)
	}
	if !strings.Contains(prompt, "keep going") || !strings.Contains(prompt, "post-compact reply") {
		t.Fatalf("injected prompt dropped turns after watermark: %q", prompt)
	}
	if strings.Contains(prompt, "ancient prompt") || strings.Contains(prompt, "done with the old file") {
		t.Fatalf("injected prompt kept pre-compact history: %q", prompt)
	}
}

func TestForkHistoryPreambleClipsHugeToolResults(t *testing.T) {
	huge := strings.Repeat("z", 20<<10)
	got := forkHistoryPreamble([]domain.Message{
		{Role: "user", Content: []domain.ContentBlock{domain.TextBlock("please read the file")}},
		{Role: "assistant", Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "", "Read", []byte(`{"path":"big.go"}`)),
		}},
		{Role: "user", Content: []domain.ContentBlock{domain.ToolResultBlock("t1", huge, false)}},
		{Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("done reading")}},
	})
	if !strings.Contains(got, "please read the file") || !strings.Contains(got, "done reading") {
		t.Fatalf("preamble dropped chat text: %q", got)
	}
	if strings.Contains(got, huge) {
		t.Fatalf("preamble left the full tool result intact")
	}
	if !strings.Contains(got, "…(truncated)…") {
		t.Fatalf("preamble missing tool truncation: %q", got)
	}
}

func turnMessageID(items []domain.ChatItem) string {
	for _, it := range items {
		if it.Kind == "turn" {
			return it.MessageID
		}
	}
	return ""
}

func lastUser(items []domain.ChatItem) string {
	for i := len(items) - 1; i >= 0; i-- {
		if items[i].Role == "user" && items[i].Kind == "text" {
			return items[i].Text
		}
	}
	return ""
}

func TestForkWorkspaceChatToWorkspaceCopiesTranscript(t *testing.T) {
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "reply "+strconv.Itoa(n))
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-to-ws", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "first question", "sonnet", nil, nil, "", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "second question", "sonnet", nil, nil, "", "")
	firstTurn := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if firstTurn == "" {
		t.Fatal("first turn footer missing message id")
	}

	dest, err := eng.ForkWorkspaceChatToWorkspace(ws.ID, "PA", firstTurn)
	if err != nil {
		t.Fatalf("ForkWorkspaceChatToWorkspace: %v", err)
	}
	if dest.ID == "" || dest.ID == ws.ID {
		t.Fatalf("ForkWorkspaceChatToWorkspace dest = %+v, want a new workspace", dest)
	}
	if dest.ProjectID != projID {
		t.Errorf("ForkWorkspaceChatToWorkspace ProjectID = %q, want %q", dest.ProjectID, projID)
	}
	if dest.Goal != "first question" {
		t.Errorf("ForkWorkspaceChatToWorkspace Goal = %q, want first question", dest.Goal)
	}

	parentItems := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if lastUser(parentItems) != "second question" {
		t.Fatalf("parent last user = %q, want second question", lastUser(parentItems))
	}
	childItems := contentItems(eng.WorkspaceChatItems(dest.ID, "PA"))
	if len(childItems) != 2 {
		t.Fatalf("dest PA should have the first turn only, got %+v", childItems)
	}
	if childItems[0].Text != "first question" {
		t.Fatalf("dest user text = %q, want first question", childItems[0].Text)
	}

	before := len(st.Workspaces())
	if _, err := eng.ForkWorkspaceChatToWorkspace(ws.ID, "PA", "no-such-message"); err == nil {
		t.Fatal("unknown fork point should error")
	}
	if got := len(st.Workspaces()); got != before {
		t.Fatalf("failed fork leaked a workspace: before=%d after=%d", before, got)
	}

	waitFor(t, 5*time.Second, "dest worktree", func() bool {
		got, ok := st.Workspace(dest.ID)
		return ok && got.WorktreePath != "" && got.Phase == "editing"
	})

	_ = eng.SendWorkspaceMessage(dest.ID, "PA", "branch elsewhere", "sonnet", nil, nil, "", "")
	after := contentItems(eng.WorkspaceChatItems(dest.ID, "PA"))
	if lastUser(after) != "branch elsewhere" {
		t.Fatalf("dest follow-up missing: %+v", after)
	}
	if lastUser(contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))) != "second question" {
		t.Fatalf("parent should be unchanged after dest send")
	}

	specs := runner.specsWithAgentID(dest.ID + "-PA")
	if len(specs) != 1 {
		t.Fatalf("dest PA turns = %d, want 1", len(specs))
	}
	if specs[0].ResumeSession != "" || specs[0].ForkSession {
		t.Fatalf("mid-history dest fork should inject, not resume: resume=%q fork=%v",
			specs[0].ResumeSession, specs[0].ForkSession)
	}
	if !strings.Contains(specs[0].Prompt, "<transcript>") || !strings.Contains(specs[0].Prompt, "first question") {
		t.Fatalf("injected prompt missing parent history: %q", specs[0].Prompt)
	}
}

func TestForkWorkspaceChatToWorkspaceRefusesPRChatHost(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("host-1", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-3", ProjectID: "acme", Number: 3, Source: domain.SourceGitHub,
		Title: "Docs", Branch: "docs", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	_ = eng.SendWorkspaceMessage(host.ID, "PA", "what changed?", "sonnet", nil, nil, domain.ChatModeReview, "")
	tip := turnMessageID(eng.WorkspaceChatItems(host.ID, "PA"))
	if tip == "" {
		t.Fatal("host chat missing message id")
	}
	before := len(st.Workspaces())
	if _, err := eng.ForkWorkspaceChatToWorkspace(host.ID, "PA", tip); err == nil {
		t.Fatal("ForkWorkspaceChatToWorkspace(host) error = nil, want refusal")
	} else if !strings.Contains(err.Error(), "create a workspace") {
		t.Errorf("ForkWorkspaceChatToWorkspace(host) error = %v, want create-a-workspace", err)
	}
	if got := len(st.Workspaces()); got != before {
		t.Errorf("host fork leaked a workspace: before=%d after=%d", before, got)
	}
}

func TestApplyChatForkInjectsOnce(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-once", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-once", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "only turn", "sonnet", nil, nil, "", "")
	tip := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if tip == "" {
		t.Fatal("missing tip")
	}
	if tab := eng.ForkWorkspaceChat(ws.ID, "PA", tip); tab.Code != "PA2" {
		t.Fatalf("fork = %+v", tab)
	}

	first := &AgentSpec{Prompt: "first", Model: "sonnet"}
	eng.applyChatFork(ws.ID, "PA2", first, false)
	if !strings.Contains(first.Prompt, "<transcript>") || !strings.Contains(first.Prompt, "only turn") {
		t.Fatalf("first send should inject: %q", first.Prompt)
	}
	if !first.PromptOnStdin {
		t.Fatal("injected prompt should ride stdin, not argv")
	}
	child, ok := st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok {
		t.Fatal("child session missing")
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(child.ConfigJSON), &cfg)
	if !cfg.ForkInject {
		t.Fatal("forkInject must stay set until a provider session id lands")
	}

	_, err = st.AppendMessage(child.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock(first.Prompt)},
	})
	if err != nil {
		t.Fatalf("AppendMessage: %v", err)
	}
	nested := &AgentSpec{Prompt: "again", Model: "sonnet"}
	eng.applyChatFork(ws.ID, "PA2", nested, false)
	if strings.Count(nested.Prompt, forkPreambleLead) != 1 {
		t.Fatalf("re-inject after a failed first send must not nest the preamble: %q", nested.Prompt)
	}

	eng.finishChatFork(ws.ID, "PA2", false, Result{SessionID: "sess-ok"})
	child, ok = st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok {
		t.Fatal("child session gone")
	}
	cfg = agentConfig{}
	_ = json.Unmarshal([]byte(child.ConfigJSON), &cfg)
	if !cfg.ForkInject {
		t.Fatal("finishChatFork must not clear forkInject when the turn did not inject")
	}
	eng.finishChatFork(ws.ID, "PA2", true, Result{SessionID: "sess-ok"})
	child, ok = st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok {
		t.Fatal("child session gone")
	}
	cfg = agentConfig{}
	_ = json.Unmarshal([]byte(child.ConfigJSON), &cfg)
	if cfg.ForkInject {
		t.Fatal("forkInject should clear after a session id is recorded")
	}

	second := &AgentSpec{Prompt: "second", Model: "sonnet"}
	eng.applyChatFork(ws.ID, "PA2", second, false)
	if strings.Contains(second.Prompt, "<transcript>") {
		t.Fatalf("second send after a live session must not inject: %q", second.Prompt)
	}
	if second.Prompt != "second" {
		t.Fatalf("second prompt = %q, want unchanged", second.Prompt)
	}
}

func TestResumeFitsProviderAllowlist(t *testing.T) {
	const (
		cursorID = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b"
		claudeID = "sess-1"
		codexID  = "0192f3c0-1111-7222-8333-444455556666"
	)
	cases := []struct {
		id, provider string
		want         bool
	}{
		{"", domain.ProviderClaudeCode, true},
		{cursorID, domain.ProviderCursor, true},
		{cursorID, domain.ProviderClaudeCode, false},
		{cursorID, domain.ProviderCodex, false},
		{claudeID, domain.ProviderClaudeCode, true},
		{claudeID, domain.ProviderCursor, false},
		{claudeID, domain.ProviderCodex, true}, // titles and UUIDs share a shape
		{codexID, domain.ProviderCodex, true},
		{codexID, domain.ProviderCursor, false},
		{claudeID, "unknown-provider", false},
	}
	for _, tc := range cases {
		if got := resumeFitsProvider(tc.id, tc.provider); got != tc.want {
			t.Errorf("resumeFitsProvider(%q, %q) = %v, want %v", tc.id, tc.provider, got, tc.want)
		}
	}
}

func TestRememberClaudeToCodexDropsResumeAndMarksInject(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-switch", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "switch-codex", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "only turn", "sonnet", nil, nil, "", "")
	eng.setAgentSession(ws.ID, "PA", "sess-claude")
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "codex/gpt-5", nil, "agent", nil)
	if a, ok := st.Agent(ws.ID, "PA"); !ok || a.SessionID != "sess-claude" {
		t.Fatalf("Remember must keep the resume id for AutoResume, agent=%+v", a)
	}
	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("session missing")
	}
	if sess.Provider != domain.ProviderCodex {
		t.Fatalf("session.Provider = %q, want %q", sess.Provider, domain.ProviderCodex)
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if !cfg.ForkInject {
		t.Fatalf("Claude→Codex dock pick should mark the next user send to inject, cfg=%+v", cfg)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "continue on codex", "codex/gpt-5", nil, nil, "", "")
	specs := runner.specsWithAgentID(ws.ID + "-PA")
	if len(specs) < 2 {
		t.Fatalf("turns = %d, want at least the original plus the Codex send", len(specs))
	}
	last := specs[len(specs)-1]
	if last.ResumeSession != "" {
		t.Fatalf("Codex send should not resume a Claude id: %q", last.ResumeSession)
	}
	if !strings.Contains(last.Prompt, "<transcript>") || !strings.Contains(last.Prompt, "only turn") {
		t.Fatalf("Codex send should inject history: %q", last.Prompt)
	}
}

func TestForkWorkspaceChatToWorkspaceCopiesDirtyTree(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-dirty", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork-dirty", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.WorktreePath == "" {
		t.Fatal("source worktree missing")
	}
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "committed.txt"), []byte("on branch\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, ws.WorktreePath, "add", "committed.txt")
	runGitIn(t, ws.WorktreePath, "commit", "-m", "branch work")
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "dirty.txt"), []byte("uncommitted\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "edit the tree", "sonnet", nil, nil, "", "")
	tip := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if tip == "" {
		t.Fatal("missing tip")
	}
	srcBase := ws.BaseBranch
	dest, err := eng.ForkWorkspaceChatToWorkspace(ws.ID, "PA", tip)
	if err != nil {
		t.Fatalf("ForkWorkspaceChatToWorkspace: %v", err)
	}

	waitFor(t, 5*time.Second, "dest dirty overlay", func() bool {
		got, ok := st.Workspace(dest.ID)
		if !ok || got.WorktreePath == "" || got.Phase != "editing" {
			return false
		}
		if srcBase != "" && got.BaseBranch != srcBase {
			return false
		}
		_, err := os.Stat(filepath.Join(got.WorktreePath, "dirty.txt"))
		return err == nil
	})
	got, ok := st.Workspace(dest.ID)
	if !ok {
		t.Fatal("dest gone")
	}
	if srcBase != "" && got.BaseBranch != srcBase {
		t.Errorf("dest.BaseBranch = %q, want source merge-base %q", got.BaseBranch, srcBase)
	}
	if b, err := os.ReadFile(filepath.Join(got.WorktreePath, "committed.txt")); err != nil || string(b) != "on branch\n" {
		t.Fatalf("dest missing committed branch file: %s (%v)", b, err)
	}
	if b, err := os.ReadFile(filepath.Join(got.WorktreePath, "dirty.txt")); err != nil || string(b) != "uncommitted\n" {
		t.Fatalf("dest missing uncommitted overlay: %s (%v)", b, err)
	}
}

// A mid-turn rename_session (agent titling its own tab) must not flip the
// session's provider to the Settings coder default; otherwise the next send
// looks like a provider switch and re-injects the whole transcript.
func TestRenameChatSessionKeepsProviderNoForkInject(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-rename", "ok") },
	})
	set := st.Settings()
	set.ModelAgent = "cursor/composer-2.5"
	st.SetSettings(set)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "rename-provider", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "first", "claude-code/sonnet", nil, nil, "", "")
	if msg := eng.RenameChatSession(ws.ID, "PA", "Titled"); !strings.Contains(msg, "Titled") {
		t.Fatalf("RenameChatSession = %q", msg)
	}
	sess, _ := st.SessionForLoopAgent(ws.ID, "PA")
	if sess.Provider != domain.ProviderClaudeCode {
		t.Fatalf("rename flipped provider to %q", sess.Provider)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "follow-up", "claude-code/sonnet", nil, nil, "", "")
	sess, _ = st.SessionForLoopAgent(ws.ID, "PA")
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if cfg.ForkInject {
		t.Fatalf("follow-up after rename set forkInject: %s", sess.ConfigJSON)
	}
}
