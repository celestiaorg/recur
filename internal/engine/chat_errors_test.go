package engine

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestFormatChatAgentErrorIncludesDetail(t *testing.T) {
	got := formatChatAgentError(errors.New("cursor run: invalid_argument: bad image"), Result{})
	if got != "Agent error: cursor run: invalid_argument: bad image" {
		t.Fatalf("got %q", got)
	}
	got = formatChatAgentError(nil, Result{})
	if got != "I hit an error trying to respond just now. Try again." {
		t.Fatalf("fallback got %q", got)
	}
}

func TestFormatChatAgentErrorDeadlineExceeded(t *testing.T) {
	got := formatChatAgentError(context.DeadlineExceeded, Result{})
	if got != chatTurnTimedOutReply {
		t.Fatalf("got %q, want %q", got, chatTurnTimedOutReply)
	}
}

func TestFormatChatAgentErrorPrefersProviderStderr(t *testing.T) {
	got := formatChatAgentError(
		errors.New("cursor run: boom"),
		Result{Stderr: "You've hit your org's monthly spend limit"},
	)
	if got != "Agent error: You've hit your org's monthly spend limit" {
		t.Fatalf("got %q", got)
	}
}

func TestFormatChatAgentErrorCodexLoginExpired(t *testing.T) {
	stderr := "\x1b[2m2026-09-10T15:43:08.630243Z\x1b[0m \x1b[31mERROR\x1b[0m \x1b[2mcodex_login::auth::manager\x1b[0m \x1b[2m-\x1b[0m Failed to refresh token: 401 Unauthorized {\n  \"error\": \"invalid_grant\"\n}"
	want := "Agent error: " + codexLoginExpiredMsg
	got := formatChatAgentError(errors.New("codex app-server exited"), Result{Stderr: stderr})
	if got != want {
		t.Fatalf("stderr path: got %q, want %q", got, want)
	}
	got = formatChatAgentError(errors.New("codex thread: boom\n"+stderr), Result{})
	if got != want {
		t.Fatalf("err path: got %q, want %q", got, want)
	}
}

func TestFormatChatAgentErrorTidiesTracingDump(t *testing.T) {
	stderr := "\x1b[31mERROR\x1b[0m workspace::tools - workspace tool failed to start {\n  \"code\": 1\n}"
	got := formatChatAgentError(errors.New("codex run: boom"), Result{Stderr: stderr})
	if got != "Agent error: workspace tool failed to start" {
		t.Fatalf("got %q", got)
	}
}

func TestFormatChatAgentErrorKeepsProviderJSON(t *testing.T) {
	stderr := `API Error: 400 {"error":{"message":"model does not support images"}}`
	got := formatChatAgentError(errors.New("codex run: boom"), Result{Stderr: stderr})
	if got != "Agent error: "+stderr {
		t.Fatalf("got %q", got)
	}
}

func TestFormatChatAgentErrorCodexLogin401(t *testing.T) {
	stderr := "ERROR codex_login::auth::manager - 401 Unauthorized"
	got := formatChatAgentError(errors.New("codex app-server exited"), Result{Stderr: stderr})
	if got != "Agent error: "+codexLoginExpiredMsg {
		t.Fatalf("got %q", got)
	}
}

func TestFormatChatAgentErrorPrefersRunnerErrOverAssistantText(t *testing.T) {
	// Bridge crash mid-turn: Result.Text is partial assistant prose; err is real.
	got := formatChatAgentError(
		errors.New("cursor run ended without a result: bridge crashed spawning a shell"),
		Result{IsError: true, Text: "I'll check the failure output and load any relevant Recur skills."},
	)
	if got != "Agent error: cursor run ended without a result: bridge crashed spawning a shell" {
		t.Fatalf("got %q", got)
	}
}

func TestFormatChatAgentErrorHidesCursorBridgeStartupDump(t *testing.T) {
	tail := "recur: starting cursor-sdk-bridge bin=/tmp/cursor-sdk-bridge workspace=/tmp/caramel-pho\n" +
		`cursor-sdk-bridge ready {"authTokenFile":"/tmp/auth-token","host":"127.0.0.1","pid":20136,"port":54632,"protocol":"connect"}` + "\n" +
		"cursor-sdk-bridge SIGTERM received, shutting down"
	got := formatChatAgentError(errors.New(cursorNoResultMessage(tail, " (bridge log: /tmp/bridge.log)")), Result{})
	if strings.Contains(got, "authTokenFile") || strings.Contains(got, "starting cursor-sdk-bridge") {
		t.Fatalf("formatChatAgentError dumped bridge startup: %q", got)
	}
	if !strings.HasPrefix(got, chatAgentErrorPrefix) || !strings.Contains(got, "stopped mid-turn") {
		t.Fatalf("formatChatAgentError(...) = %q, want Agent error about mid-turn stop", got)
	}
}

func TestFinalizeChatReplyPersistsSynthesizedError(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	code := pairAgentCode
	eng.ensurePairAgent(ws.ID, code, "cursor/composer-2.5")
	if _, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), code, "Agent", "cursor"); !ok {
		t.Fatal("ensure session")
	}

	out := eng.finalizeChatReply(ws.ID, code, "hi", "", "", Result{}, errors.New("cursor run: boom"), "Done.", time.Time{}, false, "")
	if out != "Agent error: cursor run: boom" {
		t.Fatalf("out=%q", out)
	}
	items := contentItems(eng.WorkspaceChatItems(ws.ID, code))
	if len(items) == 0 || items[len(items)-1].Text != out {
		t.Fatalf("session items=%+v, want trailing %q", items, out)
	}
}

func TestFinalizeChatReplyPersistsErrorAfterPartialStream(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	code := pairAgentCode
	eng.ensurePairAgent(ws.ID, code, "cursor/composer-2.5")
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), code, "Agent", "cursor")
	if !ok {
		t.Fatal("ensure session")
	}
	if _, err := eng.Store.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("write a helper")},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := eng.Store.AppendMessage(sess.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("Sure — I'll start by")},
	}); err != nil {
		t.Fatal(err)
	}

	out := eng.finalizeChatReply(
		ws.ID, code, "write a helper", "", "Sure — I'll start by",
		Result{IsError: true, Stderr: "rate limit exceeded"}, nil, "Done.", time.Time{}, false, "",
	)
	want := "Agent error: rate limit exceeded"
	if out != want {
		t.Fatalf("out=%q, want %q", out, want)
	}
	items := contentItems(eng.WorkspaceChatItems(ws.ID, code))
	if len(items) < 3 {
		t.Fatalf("items=%+v, want user + partial + error", items)
	}
	if items[len(items)-1].Text != want {
		t.Fatalf("trailing=%q, want %q (items=%+v)", items[len(items)-1].Text, want, items)
	}
	// Must not duplicate the user prompt when the mirror already recorded it.
	users := 0
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" {
			users++
		}
	}
	if users != 1 {
		t.Fatalf("user bubbles=%d, want 1 (items=%+v)", users, items)
	}
}

func TestFinalizeChatReplyPersistsPreMirrorFailure(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	code := pairAgentCode
	eng.ensurePairAgent(ws.ID, code, "cursor/composer-2.5")
	// No session yet — simulates runnerFor failing before mirroringRunner.Run.

	out := eng.finalizeChatReply(
		ws.ID, code, "paste this image", "", "",
		Result{}, errors.New("cursor: API key not configured"), "Done.", time.Time{}, false, "",
	)
	want := "Agent error: cursor: API key not configured"
	if out != want {
		t.Fatalf("out=%q", out)
	}
	items := contentItems(eng.WorkspaceChatItems(ws.ID, code))
	if len(items) != 2 {
		t.Fatalf("items=%+v, want user + error", items)
	}
	if items[0].Role != "user" || items[0].Text != "paste this image" {
		t.Fatalf("user item=%+v", items[0])
	}
	if items[1].Text != want {
		t.Fatalf("error item=%+v", items[1])
	}
}

func TestPersistChatFailureRetryDoesNotDuplicateUser(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	code := pairAgentCode
	eng.ensurePairAgent(ws.ID, code, "cursor/composer-2.5")
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), code, "Agent", "cursor")
	if !ok {
		t.Fatal("ensure session")
	}
	if _, err := eng.Store.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("do the thing")},
	}); err != nil {
		t.Fatal(err)
	}

	eng.persistChatFailure(ws.ID, code, "do the thing", chatRetryDisplaySentinel,
		"Agent error: You've hit your individual spend limit", 0, Result{}, false, "")
	items := contentItems(eng.WorkspaceChatItems(ws.ID, code))
	users := 0
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" {
			users++
		}
	}
	if users != 1 {
		t.Fatalf("user bubbles=%d, want 1 (items=%+v)", users, items)
	}
	if items[len(items)-1].Text != "Agent error: You've hit your individual spend limit" {
		t.Fatalf("trailing=%+v", items[len(items)-1])
	}
}

func TestPersistChatFailureWrappedDisplayDoesNotDuplicate(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	code := pairAgentCode
	eng.ensurePairAgent(ws.ID, code, "cursor/composer-2.5")
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), code, "Agent", "cursor")
	if !ok {
		t.Fatal("ensure session")
	}
	composer := "/go-code-review look at auth"
	wire := `<skill_content name="go-code-review">body</skill_content>` + "\n" + composer
	if _, err := eng.Store.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlockDisplay(wire, composer)},
	}); err != nil {
		t.Fatal(err)
	}

	eng.persistChatFailure(ws.ID, code, wire, composer, "Agent error: boom", 0, Result{}, false, "")
	items := contentItems(eng.WorkspaceChatItems(ws.ID, code))
	users := 0
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" {
			users++
			if it.Text != composer {
				t.Fatalf("user bubble=%q, want composer %q", it.Text, composer)
			}
		}
	}
	if users != 1 {
		t.Fatalf("user bubbles=%d, want 1 (items=%+v)", users, items)
	}
}

func TestPersistChatFailureResumeRecordsNewTurn(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	code := pairAgentCode
	eng.ensurePairAgent(ws.ID, code, "sonnet")
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), code, "Agent", domain.ProviderClaudeCode)
	if !ok {
		t.Fatal("ensure session")
	}
	if _, err := eng.Store.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("previous")},
	}); err != nil {
		t.Fatal(err)
	}

	const wantID = "11111111-1111-1111-1111-111111111111"
	eng.persistChatFailure(ws.ID, code, chatResumePrompt, chatResumeDisplay,
		"Agent error: boom", 1500, Result{}, false, wantID)
	items := eng.WorkspaceChatItems(ws.ID, code)
	var sawResume bool
	var footerID string
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" && it.Text == chatResumeDisplay {
			sawResume = true
		}
		if it.Kind == "turn" {
			footerID = it.MessageID
		}
	}
	if !sawResume {
		t.Fatalf("WorkspaceChatItems missing restart user row when skipUser=false, items=%+v", items)
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	got := turnUserMessageID(msgs, footerID)
	var resumeID string
	for _, m := range msgs {
		if m.Role == "user" && userMessageIsChatPrompt(m) {
			resumeID = m.ID
		}
	}
	if got != wantID {
		t.Errorf("turnUserMessageID(%q) = %s, want %s (checkpoint key)", footerID, got, wantID)
	}
	if resumeID != wantID {
		t.Errorf("restart user id = %s, want %s (checkpoint key)", resumeID, wantID)
	}

	const wantID2 = "22222222-2222-2222-2222-222222222222"
	eng.persistChatFailure(ws.ID, code, chatResumePrompt, chatResumeDisplay,
		"Agent error: boom again", 800, Result{}, false, wantID2)
	msgs, err = st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript after repeat: %v", err)
	}
	var lastUser, lastFooter string
	for _, m := range msgs {
		if m.Role == "user" && userMessageIsChatPrompt(m) {
			lastUser = m.ID
		}
		if m.DurationMS > 0 {
			lastFooter = m.ID
		}
	}
	if lastUser != wantID2 {
		t.Errorf("last user id = %s, want %s (same resume text must still insert the new checkpoint key)", lastUser, wantID2)
	}
	if got := turnUserMessageID(msgs, lastFooter); got != wantID2 {
		t.Errorf("turnUserMessageID(%q) = %s, want %s", lastFooter, got, wantID2)
	}
}

func TestFailedChatErrorNoteOwnsFooter(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &durationErrorRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	out := eng.SendWorkspaceMessage(ws.ID, "PA", "long task", "sonnet", nil, nil, "", "")
	if !strings.HasPrefix(out, chatAgentErrorPrefix) {
		t.Fatalf("reply=%q, want an Agent error", out)
	}

	items := eng.WorkspaceChatItems(ws.ID, "PA")
	errorAt, footerAt, footers := -1, -1, 0
	for i, it := range items {
		if it.Kind == "text" && strings.HasPrefix(it.Text, chatAgentErrorPrefix) {
			errorAt = i
		}
		if it.Kind == "turn" {
			footers++
			footerAt = i
			if it.DurationMS != 1500 {
				t.Errorf("turn footer DurationMS = %d, want 1500 on the error note", it.DurationMS)
			}
			if it.InputTokens != 200 || it.CacheReadTokens != 15000 {
				t.Errorf("turn footer tokens in=%d cacheRead=%d, want 200/15000", it.InputTokens, it.CacheReadTokens)
			}
		}
	}
	if footers != 1 {
		t.Fatalf("turn footers = %d, want 1 (items=%+v)", footers, items)
	}
	if errorAt < 0 || footerAt != errorAt+1 {
		t.Fatalf("error at %d footer at %d, want footer immediately after error (items=%+v)", errorAt, footerAt, items)
	}

	sess, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok {
		t.Fatal("SessionForLoopAgent: no session for PA")
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	var sawUsage bool
	for _, m := range msgs {
		if m.InputTokens == 200 && m.DurationMS == 0 {
			sawUsage = true
			break
		}
	}
	if !sawUsage {
		t.Errorf("SessionTranscript missing KindResult usage (InputTokens=200 DurationMS=0); msgs=%+v", msgs)
	}
}

func TestFailedChatKindResultDoesNotDuplicateFooter(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &kindResultErrorRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	out := eng.SendWorkspaceMessage(ws.ID, "PA", "long task", "sonnet", nil, nil, "", "")
	if !strings.HasPrefix(out, chatAgentErrorPrefix) {
		t.Fatalf("reply=%q, want an Agent error", out)
	}

	items := eng.WorkspaceChatItems(ws.ID, "PA")
	var footers int
	for _, it := range items {
		if it.Kind == "turn" {
			footers++
		}
	}
	if footers != 1 {
		t.Fatalf("turn footers = %d, want 1 after KindResult (items=%+v)", footers, items)
	}
}

func TestRetryStopDoesNotInheritPriorTurnFooter(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &sequentialChatRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	first := eng.SendWorkspaceMessage(ws.ID, "PA", "long task", "sonnet", nil, nil, "", "")
	if first != "first reply" {
		t.Fatalf("SendWorkspaceMessage first reply = %q, want %q", first, "first reply")
	}

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, "PA", "", "sonnet", nil, nil, "", chatRetryDisplaySentinel)
	}()

	waitFor(t, 3*time.Second, "chat agent working", func() bool {
		a, ok := st.Agent(ws.ID, "PA")
		return ok && a.Status == "working"
	})

	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false; no retry turn was registered")
	}

	select {
	case reply := <-done:
		if reply != chatStoppedReply {
			t.Fatalf("retry reply=%q, want %q", reply, chatStoppedReply)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("SendWorkspaceMessage retry did not return after CancelChat")
	}

	items := eng.WorkspaceChatItems(ws.ID, "PA")
	var footers int
	var stopAt, lastFooter int
	stopAt, lastFooter = -1, -1
	for i, it := range items {
		if it.Kind == "text" && it.Text == chatStoppedReply {
			stopAt = i
		}
		if it.Kind == "turn" {
			footers++
			lastFooter = i
		}
	}
	if footers != 2 {
		t.Fatalf("turn footers = %d, want 2 (prior turn + this Stop) items=%+v", footers, items)
	}
	if stopAt < 0 || lastFooter != stopAt+1 {
		t.Fatalf("stop at %d last footer at %d, want footer immediately after Stop (items=%+v)", stopAt, lastFooter, items)
	}
}
