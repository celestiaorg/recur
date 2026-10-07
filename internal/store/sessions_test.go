package store

import (
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/domain"
)

// TestMigrationRunnerBaselineAndSessions asserts a fresh DB reports the latest
// schema version and that a pre-migration-runner v1 database (all v1 tables, no
// __migrations row) migrates in place to the session schema without losing data.
func TestMigrationRunnerBaselineAndSessions(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "recur.db")

	s := openTemp(t, dir) // seeds demo data
	v, err := s.db.schemaVersion()
	if err != nil {
		t.Fatalf("schemaVersion: %v", err)
	}
	if v != latestSchemaVersion() {
		t.Fatalf("fresh DB schema version = %d, want %d", v, latestSchemaVersion())
	}
	// The session tables must exist on a fresh migrated DB.
	if _, err := s.db.sql.Exec(`SELECT 1 FROM session LIMIT 1`); err != nil {
		t.Fatalf("session table missing after migrate: %v", err)
	}
	seededWorkspaces := len(s.Workspaces())
	s.Close()

	// Simulate a REAL pre-rename (Loop-era) database: put the schema back to the
	// loops/loop_id names and un-record the rename migration, exactly as an existing
	// dev DB looks before it is opened by the unified build. Reopening must re-apply
	// migration 4 in place and converge to the workspace schema with data intact —
	// the "add a temp migration, don't recreate my stuff" path.
	legacy, err := openDB(path)
	if err != nil {
		t.Fatalf("reopen raw: %v", err)
	}
	stmts := []string{
		`ALTER TABLE workspaces RENAME TO loops`,
		`ALTER TABLE agents RENAME COLUMN workspace_id TO loop_id`,
		`ALTER TABLE feed RENAME COLUMN workspace_id TO loop_id`,
		`ALTER TABLE escalations RENAME COLUMN workspace_id TO loop_id`,
		// maestro_chat was dropped by a later migration (0006), but a real pre-4 DB
		// had it and 0004 renames its column — reconstruct it in loop-era form so the
		// forced re-apply of 0004 below finds it (0006 stays recorded, won't re-run).
		`CREATE TABLE maestro_chat (loop_id TEXT PRIMARY KEY, doc_json TEXT NOT NULL)`,
		`ALTER TABLE events RENAME COLUMN workspace_id TO loop_id`,
		`ALTER TABLE findings RENAME COLUMN workspace_id TO loop_id`,
		`ALTER TABLE analysis RENAME COLUMN workspace_id TO loop_id`,
		`DELETE FROM __migrations WHERE version = 4`,
	}
	for _, q := range stmts {
		if _, err := legacy.sql.Exec(q); err != nil {
			t.Fatalf("simulate legacy (%q): %v", q, err)
		}
	}
	legacy.close()

	s2, err := open(path)
	if err != nil {
		t.Fatalf("reopen after legacy simulation: %v", err)
	}
	defer s2.Close()
	v2, err := s2.db.schemaVersion()
	if err != nil || v2 != latestSchemaVersion() {
		t.Fatalf("re-migrated schema version = %d (err %v), want %d", v2, err, latestSchemaVersion())
	}
	if got := len(s2.Workspaces()); got != seededWorkspaces {
		t.Errorf("in-place rename lost data: got %d workspaces, want %d", got, seededWorkspaces)
	}
	// The renamed table + column are queryable under the new names.
	if _, err := s2.db.sql.Exec(`SELECT 1 FROM workspaces LIMIT 1`); err != nil {
		t.Errorf("workspaces table missing after in-place rename: %v", err)
	}
	if _, err := s2.db.sql.Exec(`SELECT workspace_id FROM feed LIMIT 1`); err != nil {
		t.Errorf("feed.workspace_id missing after in-place rename: %v", err)
	}
}

func TestDropMessageCostUSD(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	s.Close()

	path := filepath.Join(dir, "recur.db")
	legacy, err := openDB(path)
	if err != nil {
		t.Fatalf("openDB: %v", err)
	}
	for _, q := range []string{
		`ALTER TABLE message ADD COLUMN cost_usd REAL`,
		`DELETE FROM __migrations WHERE version = 13`,
	} {
		if _, err := legacy.sql.Exec(q); err != nil {
			legacy.close()
			t.Fatalf("simulate pre-0013 (%q): %v", q, err)
		}
	}
	legacy.close()

	s2 := openTemp(t, dir)
	var n int
	if err := s2.db.sql.QueryRow(`SELECT COUNT(*) FROM pragma_table_info('message') WHERE name = 'cost_usd'`).Scan(&n); err != nil {
		t.Fatalf("pragma_table_info: %v", err)
	}
	if n != 0 {
		t.Fatalf("cost_usd still on message after 0013")
	}
}

// TestSessionTranscriptRoundTrip covers the core S1 promise: a session's messages
// and typed content blocks persist and reload in order, across a store reopen,
// through the single load path.
func TestSessionTranscriptRoundTrip(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)

	sess, err := s.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1",
		Kind:        "maestro",
		Provider:    domain.ProviderClaudeCode,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if sess.ID == "" || sess.ContextMode != domain.ContextModeProvider || sess.Status != "idle" {
		t.Fatalf("session defaults wrong: %+v", sess)
	}
	if s.SessionHasMessages(sess.ID) {
		t.Fatal("new session should have no messages")
	}

	// A user turn (one text block).
	if _, err := s.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("fix the flaky test")},
	}); err != nil {
		t.Fatalf("append user message: %v", err)
	}
	if !s.SessionHasMessages(sess.ID) {
		t.Fatal("SessionHasMessages = false after append")
	}

	// An assistant turn streamed incrementally: text, a tool call, then completion.
	msgID, err := s.CreateMessage(sess.ID, domain.Message{
		Role: "assistant", ModelID: "claude-opus-4-8", Provider: domain.ProviderClaudeCode,
	})
	if err != nil {
		t.Fatalf("create assistant message: %v", err)
	}
	if err := s.AppendContentBlock(msgID, domain.TextBlock("Looking at it.")); err != nil {
		t.Fatalf("append text block: %v", err)
	}
	if err := s.AppendContentBlock(msgID, domain.ToolUseBlock("tu-int-1", "toolu_abc", "Read", json.RawMessage(`{"file_path":"x_test.go"}`))); err != nil {
		t.Fatalf("append tool_use: %v", err)
	}
	if err := s.CompleteMessage(msgID, domain.StreamComplete, domain.Message{
		InputTokens: 1200, OutputTokens: 340,
	}); err != nil {
		t.Fatalf("complete message: %v", err)
	}
	// The tool_result comes back on a following user message.
	if _, err := s.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.ToolResultBlock("tu-int-1", "package x", false)},
	}); err != nil {
		t.Fatalf("append tool_result message: %v", err)
	}

	assert := func(t *testing.T, st *Store, label string) {
		t.Helper()
		msgs, err := st.SessionTranscript(sess.ID)
		if err != nil {
			t.Fatalf("%s transcript: %v", label, err)
		}
		if len(msgs) != 3 {
			t.Fatalf("%s: got %d messages, want 3", label, len(msgs))
		}
		if msgs[0].Role != "user" || msgs[1].Role != "assistant" || msgs[2].Role != "user" {
			t.Fatalf("%s: message order wrong: %s/%s/%s", label, msgs[0].Role, msgs[1].Role, msgs[2].Role)
		}
		asst := msgs[1]
		if asst.ModelID != "claude-opus-4-8" || asst.StreamStatus != domain.StreamComplete {
			t.Errorf("%s: assistant stamp wrong: model=%q status=%q", label, asst.ModelID, asst.StreamStatus)
		}
		if asst.InputTokens != 1200 || asst.OutputTokens != 340 {
			t.Errorf("%s: usage not persisted: %+v", label, asst)
		}
		if len(asst.Content) != 2 || asst.Content[0].Type != domain.BlockText || asst.Content[1].Type != domain.BlockToolUse {
			t.Fatalf("%s: assistant blocks wrong: %+v", label, asst.Content)
		}
		tu, ok := asst.Content[1].AsToolUse()
		if !ok || tu.ID != "tu-int-1" || tu.ProviderToolUseID != "toolu_abc" || tu.Name != "Read" {
			t.Errorf("%s: tool_use payload wrong: %+v ok=%v", label, tu, ok)
		}
		tr, ok := msgs[2].Content[0].AsToolResult()
		if !ok || tr.ToolUseID != "tu-int-1" || tr.Result != "package x" {
			t.Errorf("%s: tool_result payload wrong: %+v ok=%v", label, tr, ok)
		}
	}

	assert(t, s, "in memory")
	s.Close()

	s2 := openTemp(t, dir)
	assert(t, s2, "after reopen")
}

// TestSessionForWorkspaceKind covers reusing a loop's session across turns.
func TestSessionForWorkspaceKind(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)

	if _, ok := s.SessionForWorkspaceKind("LOOP-9", "maestro"); ok {
		t.Fatalf("no session should exist yet")
	}
	created, err := s.CreateSession(domain.Session{WorkspaceID: "LOOP-9", Kind: "maestro", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	got, ok := s.SessionForWorkspaceKind("LOOP-9", "maestro")
	if !ok || got.ID != created.ID {
		t.Fatalf("SessionForWorkspaceKind = %+v ok=%v, want id %q", got, ok, created.ID)
	}

	// Ref + status updates persist.
	if err := s.UpdateSessionRef(created.ID, "claude-sess-xyz"); err != nil {
		t.Fatalf("UpdateSessionRef: %v", err)
	}
	if err := s.SetSessionStatus(created.ID, "streaming"); err != nil {
		t.Fatalf("SetSessionStatus: %v", err)
	}
	if err := s.UpdateSessionProvider(created.ID, domain.ProviderCursor); err != nil {
		t.Fatalf("UpdateSessionProvider: %v", err)
	}
	s.Close()

	s2 := openTemp(t, dir)
	reloaded, ok := s2.GetSession(created.ID)
	if !ok || reloaded.ProviderSessionRef != "claude-sess-xyz" || reloaded.Status != "streaming" || reloaded.Provider != domain.ProviderCursor {
		t.Fatalf("session updates not persisted: %+v ok=%v", reloaded, ok)
	}
}

func TestSessionForAppKindAndStreamingRecovery(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)

	if _, ok := s.SessionForAppKind(domain.SessionKindApp); ok {
		t.Fatal("SessionForAppKind() found a session, want none")
	}
	created, err := s.CreateSession(domain.Session{
		Kind: domain.SessionKindApp, Provider: domain.ProviderClaudeCode, Status: "streaming",
		ConfigJSON: `{"code":"APP","name":"Ask Recur"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if created.WorkspaceID != "" {
		t.Errorf("CreateSession app workspace_id = %q, want empty", created.WorkspaceID)
	}
	got, ok := s.SessionForAppKind(domain.SessionKindApp)
	if !ok || got.ID != created.ID {
		t.Fatalf("SessionForAppKind() = %+v ok=%v, want id %q", got, ok, created.ID)
	}
	live := s.StreamingAppSessions()
	if len(live) != 1 || live[0].ID != created.ID {
		t.Fatalf("StreamingAppSessions() = %+v, want [%s]", live, created.ID)
	}
	if err := s.SetSessionStatus(created.ID, "idle"); err != nil {
		t.Fatalf("SetSessionStatus: %v", err)
	}
	if n := len(s.StreamingAppSessions()); n != 0 {
		t.Errorf("StreamingAppSessions() after idle = %d, want 0", n)
	}
}

func TestAttentionCountsAppUnseen(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	before := s.AttentionCounts()
	created, err := s.CreateSession(domain.Session{
		Kind: domain.SessionKindApp, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"APP","unseen":true}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	got := s.AttentionCounts()
	if got.AppUnseen != 1 {
		t.Errorf("AppUnseen = %d, want 1 (session %s)", got.AppUnseen, created.ID)
	}
	if got.Dock != before.Dock+1 {
		t.Errorf("Dock = %d, want %d (app unseen counted once)", got.Dock, before.Dock+1)
	}
	if err := s.UpdateSessionConfigJSON(created.ID, `{"code":"APP"}`); err != nil {
		t.Fatal(err)
	}
	cleared := s.AttentionCounts()
	if cleared.AppUnseen != 0 {
		t.Errorf("AppUnseen after clear = %d, want 0", cleared.AppUnseen)
	}
}

func TestSessionHeadersAndLatestEventTS(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	a, err := s.CreateSession(domain.Session{WorkspaceID: "W-A", Kind: "pair", Provider: domain.ProviderClaudeCode, ConfigJSON: `{"code":"PA"}`})
	if err != nil {
		t.Fatalf("CreateSession A: %v", err)
	}
	_, err = s.CreateSession(domain.Session{WorkspaceID: "W-B", Kind: "pair", Provider: domain.ProviderClaudeCode, ConfigJSON: `{"code":"PA"}`})
	if err != nil {
		t.Fatalf("CreateSession B: %v", err)
	}
	headers := s.SessionHeadersByWorkspace()
	if len(headers["W-A"]) == 0 || headers["W-A"][0].ID != a.ID {
		t.Errorf("SessionHeadersByWorkspace()[W-A] = %+v, want session %s", headers["W-A"], a.ID)
	}
	if _, ok := headers["W-B"]; !ok {
		t.Error("SessionHeadersByWorkspace() missing W-B")
	}
	if _, err := s.AppendEvent(domain.StoredEvent{WorkspaceID: "W-A", Type: "note", TS: 99}); err != nil {
		t.Fatalf("AppendEvent: %v", err)
	}
	ts := s.LatestEventTSByWorkspace()
	if ts["W-A"] != 99 {
		t.Errorf("LatestEventTSByWorkspace()[W-A] = %d, want 99", ts["W-A"])
	}
}

func TestReassignWorkspaceSessions(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	sess, err := s.CreateSession(domain.Session{
		WorkspaceID: "W-host", Kind: "pair", Provider: domain.ProviderClaudeCode, ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if err := s.ReassignWorkspaceSessions("W-host", "W-live", nil); err != nil {
		t.Fatalf("ReassignWorkspaceSessions: %v", err)
	}
	got, ok := s.SessionForLoopAgent("W-live", "PA")
	if !ok || got.ID != sess.ID {
		t.Fatalf("SessionForLoopAgent(W-live) = %+v ok=%v, want %s", got, ok, sess.ID)
	}
	if _, still := s.SessionForLoopAgent("W-host", "PA"); still {
		t.Error("source workspace still has the session")
	}
}

func TestReassignWorkspaceSessionsRemapsCollidingCodes(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	host, err := s.CreateSession(domain.Session{
		WorkspaceID: "W-host", Kind: "pair", Provider: domain.ProviderClaudeCode, ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession host: %v", err)
	}
	if _, err := s.AppendMessage(host.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("host chat")},
	}); err != nil {
		t.Fatalf("AppendMessage host: %v", err)
	}
	live, err := s.CreateSession(domain.Session{
		WorkspaceID: "W-live", Kind: "pair", Provider: domain.ProviderClaudeCode, ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession live: %v", err)
	}
	if _, err := s.AppendMessage(live.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("live chat")},
	}); err != nil {
		t.Fatalf("AppendMessage live: %v", err)
	}
	if err := s.ReassignWorkspaceSessions("W-host", "W-live", map[string]string{"PA": "PA2"}); err != nil {
		t.Fatalf("ReassignWorkspaceSessions: %v", err)
	}
	gotLive, ok := s.SessionForLoopAgent("W-live", "PA")
	if !ok || gotLive.ID != live.ID {
		t.Fatalf("SessionForLoopAgent(W-live, PA) = %+v ok=%v, want %s", gotLive, ok, live.ID)
	}
	gotHost, ok := s.SessionForLoopAgent("W-live", "PA2")
	if !ok || gotHost.ID != host.ID {
		t.Fatalf("SessionForLoopAgent(W-live, PA2) = %+v ok=%v, want %s", gotHost, ok, host.ID)
	}
	if !strings.Contains(gotHost.ConfigJSON, `"code":"PA2"`) {
		t.Errorf("hoisted config = %s, want remapped PA2", gotHost.ConfigJSON)
	}
}

func TestReassignWorkspaceSessionsDropsEmptyDestCollision(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	host, err := s.CreateSession(domain.Session{
		WorkspaceID: "W-host", Kind: "pair", Provider: domain.ProviderClaudeCode, ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession host: %v", err)
	}
	if _, err := s.AppendMessage(host.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("host chat")},
	}); err != nil {
		t.Fatalf("AppendMessage host: %v", err)
	}
	if _, err := s.CreateSession(domain.Session{
		WorkspaceID: "W-live", Kind: "pair", Provider: domain.ProviderClaudeCode, ConfigJSON: `{"code":"PA"}`,
	}); err != nil {
		t.Fatalf("CreateSession live: %v", err)
	}
	if err := s.ReassignWorkspaceSessions("W-host", "W-live", nil); err != nil {
		t.Fatalf("ReassignWorkspaceSessions: %v", err)
	}
	got, ok := s.SessionForLoopAgent("W-live", "PA")
	if !ok || got.ID != host.ID {
		t.Fatalf("SessionForLoopAgent(W-live, PA) = %+v ok=%v, want hoisted %s", got, ok, host.ID)
	}
}

// TestToolResultUniqueIndex covers the one-result-per-call idempotency backstop:
// a second tool_result for the same internal tool_use id in the same message is
// rejected by the unique partial index.
func TestToolResultUniqueIndex(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	sess, err := s.CreateSession(domain.Session{Kind: "task", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	msgID, err := s.CreateMessage(sess.ID, domain.Message{Role: "user"})
	if err != nil {
		t.Fatalf("CreateMessage: %v", err)
	}
	if err := s.AppendContentBlock(msgID, domain.ToolResultBlock("tu-1", "first", false)); err != nil {
		t.Fatalf("first tool_result: %v", err)
	}
	if err := s.AppendContentBlock(msgID, domain.ToolResultBlock("tu-1", "duplicate", false)); err == nil {
		t.Fatalf("expected the unique index to reject a duplicate tool_result for tu-1")
	}
}

func TestPatchToolUseInput(t *testing.T) {
	s := openTemp(t, t.TempDir())
	sess, err := s.CreateSession(domain.Session{Kind: "task", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatal(err)
	}
	msgID, err := s.CreateMessage(sess.ID, domain.Message{Role: "assistant"})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.AppendContentBlock(msgID, domain.ToolUseBlock(
		"tu-ask", "call-1", "mcp__recur__ask_user", json.RawMessage(`{}`),
	)); err != nil {
		t.Fatal(err)
	}
	want := json.RawMessage(`{"question":"Ship it?"}`)
	if err := s.PatchToolUseInput("tu-ask", want); err != nil {
		t.Fatal(err)
	}
	msgs, err := s.SessionTranscript(sess.ID)
	if err != nil || len(msgs) != 1 || len(msgs[0].Content) != 1 {
		t.Fatalf("transcript: %+v err=%v", msgs, err)
	}
	tu, ok := msgs[0].Content[0].AsToolUse()
	if !ok || string(tu.Input) != string(want) {
		t.Fatalf("patched input = %s ok=%v", tu.Input, ok)
	}
	got, ok := s.LatestToolUseByNameSuffix(sess.ID, "__ask_user")
	if !ok || got.ID != "tu-ask" || string(got.Input) != string(want) {
		t.Fatalf("LatestToolUseByNameSuffix = %+v ok=%v", got, ok)
	}
}

// TestPatchSessionConfigTitleKeepsModeModelFlags: tab-title upgrades must not
// replace the whole config_json (that used to wipe dock selection mid-turn).
func TestPatchSessionConfigTitleKeepsModeModelFlags(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA2","name":"Agent","model":"sonnet","flags":{"effort":"high"},"mode":"review"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if err := st.PatchSessionConfigTitle(sess.ID, "Review the diff"); err != nil {
		t.Fatalf("PatchSessionConfigTitle: %v", err)
	}
	got, ok := st.SessionForLoopAgent("LOOP-1", "PA2")
	if !ok {
		t.Fatal("session missing after title patch")
	}
	if !strings.Contains(got.ConfigJSON, `"mode":"review"`) {
		t.Errorf("mode lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"model":"sonnet"`) {
		t.Errorf("model lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"effort":"high"`) {
		t.Errorf("flags lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"title":"Review the diff"`) {
		t.Errorf("title missing: %s", got.ConfigJSON)
	}
}

// TestPatchSessionConfigSimplifyKeepsModeModelFlags: composer Simplify toggles
// must json_set only $.simplify so a concurrent dock write cannot lose mode/model.
func TestPatchSessionConfigSimplifyKeepsModeModelFlags(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA2","name":"Agent","model":"sonnet","flags":{"effort":"high"},"mode":"review","title":"Review the diff"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if err := st.PatchSessionConfigSimplify(sess.ID, false); err != nil {
		t.Fatalf("PatchSessionConfigSimplify: %v", err)
	}
	got, ok := st.SessionForLoopAgent("LOOP-1", "PA2")
	if !ok {
		t.Fatal("session missing after simplify patch")
	}
	if !strings.Contains(got.ConfigJSON, `"mode":"review"`) {
		t.Errorf("mode lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"model":"sonnet"`) {
		t.Errorf("model lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"effort":"high"`) {
		t.Errorf("flags lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"title":"Review the diff"`) {
		t.Errorf("title lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"simplify":false`) {
		t.Errorf("simplify missing: %s", got.ConfigJSON)
	}
	if err := st.PatchSessionConfigTitle(sess.ID, "Still reviewing"); err != nil {
		t.Fatalf("PatchSessionConfigTitle: %v", err)
	}
	got, ok = st.SessionForLoopAgent("LOOP-1", "PA2")
	if !ok {
		t.Fatal("session missing after title patch")
	}
	if !strings.Contains(got.ConfigJSON, `"simplify":false`) {
		t.Errorf("simplify lost after title patch: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"title":"Still reviewing"`) {
		t.Errorf("title missing: %s", got.ConfigJSON)
	}
}

// TestPatchSessionConfigMergeKeepsSimplify: a dock model/mode write must
// json_patch without restoring a stale simplify value.
func TestPatchSessionConfigMergeKeepsSimplify(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA","name":"Agent","model":"sonnet","mode":"agent","simplify":false}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if err := st.PatchSessionConfigMerge(sess.ID, `{"model":"opus","mode":"review","flags":{"effort":"high"}}`); err != nil {
		t.Fatalf("PatchSessionConfigMerge: %v", err)
	}
	got, ok := st.SessionForLoopAgent("LOOP-1", "PA")
	if !ok {
		t.Fatal("session missing after merge")
	}
	if !strings.Contains(got.ConfigJSON, `"simplify":false`) {
		t.Errorf("simplify lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"mode":"review"`) {
		t.Errorf("mode missing: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"model":"opus"`) {
		t.Errorf("model missing: %s", got.ConfigJSON)
	}
	if err := st.PatchSessionConfigSimplifyIfUnset(sess.ID, true); err != nil {
		t.Fatalf("PatchSessionConfigSimplifyIfUnset: %v", err)
	}
	got, ok = st.SessionForLoopAgent("LOOP-1", "PA")
	if !ok {
		t.Fatal("session missing after if-unset")
	}
	if !strings.Contains(got.ConfigJSON, `"simplify":false`) {
		t.Errorf("if-unset overwrote explicit false: %s", got.ConfigJSON)
	}
}

// TestPatchSessionConfigFlagsReplacesObject: json_patch would keep omitted
// flag keys; json_set must replace the whole flags object.
func TestPatchSessionConfigFlagsReplacesObject(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA","model":"sonnet","flags":{"effort":"high","thinking":"on"},"simplify":false}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if err := st.PatchSessionConfigFlags(sess.ID, `{"effort":"low"}`); err != nil {
		t.Fatalf("PatchSessionConfigFlags: %v", err)
	}
	got, ok := st.SessionForLoopAgent("LOOP-1", "PA")
	if !ok {
		t.Fatal("session missing after flags patch")
	}
	if !strings.Contains(got.ConfigJSON, `"simplify":false`) {
		t.Errorf("simplify lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"effort":"low"`) {
		t.Errorf("effort missing: %s", got.ConfigJSON)
	}
	if strings.Contains(got.ConfigJSON, `"thinking"`) {
		t.Errorf("stale flag retained: %s", got.ConfigJSON)
	}
}

// TestPatchSessionConfigDockReplacesFlagsKeepsSimplify: one UPDATE must
// swap model+flags together without restoring a concurrent simplify json_set.
func TestPatchSessionConfigDockReplacesFlagsKeepsSimplify(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA","model":"sonnet","mode":"agent","flags":{"effort":"high","thinking":"on"},"simplify":false}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if err := st.PatchSessionConfigDock(sess.ID, `{"model":"opus","mode":"review"}`, `{"effort":"low"}`); err != nil {
		t.Fatalf("PatchSessionConfigDock: %v", err)
	}
	got, ok := st.SessionForLoopAgent("LOOP-1", "PA")
	if !ok {
		t.Fatal("session missing after dock patch")
	}
	if !strings.Contains(got.ConfigJSON, `"simplify":false`) {
		t.Errorf("simplify lost: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"model":"opus"`) {
		t.Errorf("model missing: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"mode":"review"`) {
		t.Errorf("mode missing: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"effort":"low"`) {
		t.Errorf("effort missing: %s", got.ConfigJSON)
	}
	if strings.Contains(got.ConfigJSON, `"thinking"`) {
		t.Errorf("stale flag retained: %s", got.ConfigJSON)
	}
}

// TestPatchSessionConfigMergeRemovesForkInjectKeepsSimplify: RFC 7396 null
// deletes forkInject without restoring a stale simplify value.
func TestPatchSessionConfigMergeRemovesForkInjectKeepsSimplify(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA","name":"Agent","forkInject":true,"simplify":false}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if err := st.PatchSessionConfigMerge(sess.ID, `{"forkInject":null}`); err != nil {
		t.Fatalf("PatchSessionConfigMerge: %v", err)
	}
	got, ok := st.SessionForLoopAgent("LOOP-1", "PA")
	if !ok {
		t.Fatal("session missing after merge")
	}
	if strings.Contains(got.ConfigJSON, "forkInject") {
		t.Errorf("forkInject still present: %s", got.ConfigJSON)
	}
	if !strings.Contains(got.ConfigJSON, `"simplify":false`) {
		t.Errorf("simplify lost: %s", got.ConfigJSON)
	}
}

// TestMessageDurationRoundTrip pins the 0007 column: a turn's wall time must
// survive a write/read cycle, or the transcript loses the run time on reload.
func TestMessageDurationRoundTrip(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	id, err := st.CreateMessage(sess.ID, domain.Message{Role: "assistant"})
	if err != nil {
		t.Fatalf("CreateMessage: %v", err)
	}
	if err := st.CompleteMessage(id, domain.StreamComplete, domain.Message{
		DurationMS: 8460,
	}); err != nil {
		t.Fatalf("CompleteMessage: %v", err)
	}

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	if len(msgs) != 1 {
		t.Fatalf("got %d messages", len(msgs))
	}
	if msgs[0].DurationMS != 8460 {
		t.Errorf("DurationMS = %d, want 8460", msgs[0].DurationMS)
	}
}

// TestMessageContextWindowRoundTrip pins the 0009 column for the chat context meter.
func TestMessageContextWindowRoundTrip(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	id, err := st.CreateMessage(sess.ID, domain.Message{Role: "assistant"})
	if err != nil {
		t.Fatalf("CreateMessage: %v", err)
	}
	if err := st.CompleteMessage(id, domain.StreamComplete, domain.Message{
		InputTokens: 120, CacheReadTokens: 18000, CacheCreationTokens: 500,
		DurationMS: 100, ContextWindow: 200_000,
	}); err != nil {
		t.Fatalf("CompleteMessage: %v", err)
	}

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	if len(msgs) != 1 {
		t.Fatalf("got %d messages", len(msgs))
	}
	m := msgs[0]
	if m.InputTokens != 120 || m.CacheReadTokens != 18000 || m.CacheCreationTokens != 500 {
		t.Errorf("tokens = %+v", m)
	}
	if m.ContextWindow != 200_000 {
		t.Errorf("ContextWindow = %d, want 200000", m.ContextWindow)
	}
}

// TestSessionFirstUserPrompt skips tool_result carriers and prefers display_text
// so tab titles can stay off the full transcript scan.
func TestSessionFirstUserPrompt(t *testing.T) {
	st := openTemp(t, t.TempDir())
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "LOOP-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if got := st.SessionFirstUserPrompt(sess.ID); got != "" {
		t.Fatalf("empty session prompt = %q", got)
	}

	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.ToolResultBlock("tu-1", strings.Repeat("diff ", 200), false)},
	}); err != nil {
		t.Fatalf("append tool_result: %v", err)
	}
	if got := st.SessionFirstUserPrompt(sess.ID); got != "" {
		t.Fatalf("tool_result-only prompt = %q", got)
	}

	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlockDisplay("wire prompt with @files", "review the flaky suite"),
		},
	}); err != nil {
		t.Fatalf("append user: %v", err)
	}
	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("ok")},
	}); err != nil {
		t.Fatalf("append assistant: %v", err)
	}
	if got := st.SessionFirstUserPrompt(sess.ID); got != "review the flaky suite" {
		t.Fatalf("first prompt = %q, want display_text", got)
	}
}

// TestLastAssistantTextPicksLastTextInLatestSession walks assistant text blocks
// newest-first in the newest session and ignores older sessions.
func TestLastAssistantTextPicksLastTextInLatestSession(t *testing.T) {
	st := openTemp(t, t.TempDir())
	if got := st.LastAssistantText("WS-snippet"); got != "" {
		t.Fatalf("empty workspace = %q", got)
	}

	old, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-snippet", Kind: "coder", Provider: domain.ProviderClaudeCode,
		CreatedAt: 1,
	})
	if err != nil {
		t.Fatalf("CreateSession old: %v", err)
	}
	if _, err := st.AppendMessage(old.ID, domain.Message{
		Role:    "assistant",
		Content: []domain.ContentBlock{domain.TextBlock("old session last line")},
	}); err != nil {
		t.Fatalf("append old: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-snippet", Kind: "coder", Provider: domain.ProviderClaudeCode,
		CreatedAt: 2,
	})
	if err != nil {
		t.Fatalf("CreateSession new: %v", err)
	}
	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.TextBlock("I'll load the Recur triage skill and Linear tools."),
			domain.ToolUseBlock("t1", "tu-1", "Skill", nil),
			domain.TextBlock("No matching issues in ACME Triage."),
			domain.TextBlock(""),
		},
	}); err != nil {
		t.Fatalf("append new: %v", err)
	}
	got := st.LastAssistantText("WS-snippet")
	if got != "No matching issues in ACME Triage." {
		t.Fatalf("LastAssistantText = %q", got)
	}

	ui, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-snippet", Kind: "coder", Provider: domain.ProviderClaudeCode,
		CreatedAt: 3,
	})
	if err != nil {
		t.Fatalf("CreateSession display: %v", err)
	}
	if _, err := st.AppendMessage(ui.ID, domain.Message{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.TextBlockDisplay("WIRE do not show this in the snippet", "All tests passed on the nightly run."),
		},
	}); err != nil {
		t.Fatalf("append display: %v", err)
	}
	got = st.LastAssistantText("WS-snippet")
	if got != "All tests passed on the nightly run." {
		t.Fatalf("LastAssistantText display = %q", got)
	}
}

// TestForkSessionCopiesJoinRowsUpToForkPoint is the fork contract:
// the child shares message rows through the fork message (no content copy)
// and new appends on either session stay isolated.
func TestForkSessionCopiesJoinRowsUpToForkPoint(t *testing.T) {
	st := openTemp(t, t.TempDir())
	parent, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-1", Kind: "maestro",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA","title":"orig"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	u1, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("first")},
	})
	if err != nil {
		t.Fatalf("append user1: %v", err)
	}
	a1, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("ok one")},
	})
	if err != nil {
		t.Fatalf("append asst1: %v", err)
	}
	if _, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("second")},
	}); err != nil {
		t.Fatalf("append user2: %v", err)
	}
	if _, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("ok two")},
	}); err != nil {
		t.Fatalf("append asst2: %v", err)
	}

	if st.SessionMessageIsTip(parent.ID, a1.ID) {
		t.Fatal("first assistant message should not be the tip after later turns")
	}

	child, err := st.ForkSession(parent.ID, a1.ID, "coder", `{"code":"PA2","title":"fork"}`)
	if err != nil {
		t.Fatalf("ForkSession: %v", err)
	}
	if child.ParentSessionID != parent.ID || child.ForkMessageID != a1.ID {
		t.Fatalf("lineage: %+v", child)
	}
	if child.Kind != "coder" || child.ProviderSessionRef != "" || child.Status != "idle" {
		t.Fatalf("child defaults: %+v", child)
	}
	if child.WorkspaceID != "WS-1" || child.Provider != domain.ProviderClaudeCode {
		t.Fatalf("child should inherit workspace/provider: %+v", child)
	}
	if !st.SessionMessageIsTip(child.ID, a1.ID) {
		t.Fatal("fork message should be the child's tip")
	}

	got, err := st.SessionTranscript(child.ID)
	if err != nil {
		t.Fatalf("child transcript: %v", err)
	}
	if len(got) != 2 || got[0].ID != u1.ID || got[1].ID != a1.ID {
		t.Fatalf("child should share the first two messages, got %+v", idsOf(got))
	}
	if got[0].Content[0].Type != domain.BlockText {
		t.Fatalf("shared message should still have content: %+v", got[0])
	}

	if _, err := st.AppendMessage(child.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("from fork")},
	}); err != nil {
		t.Fatalf("append on child: %v", err)
	}
	if _, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("from parent")},
	}); err != nil {
		t.Fatalf("append on parent: %v", err)
	}

	parentMsgs, _ := st.SessionTranscript(parent.ID)
	childMsgs, _ := st.SessionTranscript(child.ID)
	if len(parentMsgs) != 5 {
		t.Fatalf("parent len = %d, want 5 (4 original + 1 new)", len(parentMsgs))
	}
	if len(childMsgs) != 3 {
		t.Fatalf("child len = %d, want 3 (2 inherited + 1 new)", len(childMsgs))
	}
	if lastText(parentMsgs) != "from parent" || lastText(childMsgs) != "from fork" {
		t.Fatalf("sessions leaked: parent=%q child=%q", lastText(parentMsgs), lastText(childMsgs))
	}

	if st.SessionContainsMessage(parent.ID, "no-such-msg") {
		t.Fatal("SessionContainsMessage(unknown) = true")
	}
	if !st.SessionContainsMessage(parent.ID, a1.ID) {
		t.Fatal("SessionContainsMessage(a1) = false")
	}
	if _, err := st.ForkSession(parent.ID, "no-such-msg", "coder", "{}"); err == nil {
		t.Fatal("fork at unknown message should fail")
	}

	moved, err := st.ForkSessionToWorkspace(parent.ID, a1.ID, "WS-2", "coder", `{"code":"PA"}`)
	if err != nil {
		t.Fatalf("ForkSessionToWorkspace: %v", err)
	}
	if moved.WorkspaceID != "WS-2" {
		t.Fatalf("ForkSessionToWorkspace WorkspaceID = %q, want WS-2", moved.WorkspaceID)
	}
	movedMsgs, err := st.SessionTranscript(moved.ID)
	if err != nil {
		t.Fatalf("moved transcript: %v", err)
	}
	if len(movedMsgs) != 2 || movedMsgs[0].ID != u1.ID || movedMsgs[1].ID != a1.ID {
		t.Fatalf("moved should share the first two messages, got %+v", idsOf(movedMsgs))
	}
	if _, err := st.ForkSessionToWorkspace(parent.ID, a1.ID, "", "coder", "{}"); err == nil {
		t.Fatal("ForkSessionToWorkspace with empty dest should fail")
	}
}

func TestDeleteSessionDropsOrphansAndKeepsForks(t *testing.T) {
	st := openTemp(t, t.TempDir())
	parent, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	shared, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("keep")},
	})
	if err != nil {
		t.Fatalf("append shared: %v", err)
	}
	onlyParent, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("gone")},
	})
	if err != nil {
		t.Fatalf("append parent-only: %v", err)
	}
	child, err := st.ForkSession(parent.ID, shared.ID, "coder", `{"code":"PA2"}`)
	if err != nil {
		t.Fatalf("ForkSession: %v", err)
	}
	if err := st.DeleteSession(parent.ID); err != nil {
		t.Fatalf("DeleteSession: %v", err)
	}
	if _, ok := st.GetSession(parent.ID); ok {
		t.Fatal("parent session still present")
	}
	got, err := st.SessionTranscript(child.ID)
	if err != nil {
		t.Fatalf("child transcript: %v", err)
	}
	if len(got) != 1 || got[0].ID != shared.ID {
		t.Fatalf("child should keep the shared message, got %+v", idsOf(got))
	}
	var n int
	if err := st.db.sql.QueryRow(`SELECT COUNT(*) FROM message WHERE id = ?`, onlyParent.ID).Scan(&n); err != nil {
		t.Fatalf("count parent-only: %v", err)
	}
	if n != 0 {
		t.Fatalf("parent-only message survived, count=%d", n)
	}
}

func TestDeleteClosedChatDropsOwnedAndStripsOrder(t *testing.T) {
	st := openTemp(t, t.TempDir())
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "WS-1", Goal: "delete-closed", ChatTabOrder: []string{"PA", "PA2"},
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	keep, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("keep session: %v", err)
	}
	drop, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA2"}`,
	})
	if err != nil {
		t.Fatalf("drop session: %v", err)
	}
	owned, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"CO","ownerCode":"PA2"}`,
	})
	if err != nil {
		t.Fatalf("owned session: %v", err)
	}
	if _, err := st.AppendMessage(drop.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("throw away")},
	}); err != nil {
		t.Fatalf("append drop: %v", err)
	}
	if err := st.DeleteClosedChat("WS-1", "PA2"); err != nil {
		t.Fatalf("DeleteClosedChat: %v", err)
	}
	if _, ok := st.GetSession(drop.ID); ok {
		t.Fatal("conductor session still present")
	}
	if _, ok := st.GetSession(owned.ID); ok {
		t.Fatal("owned specialist session still present")
	}
	if _, ok := st.GetSession(keep.ID); !ok {
		t.Fatal("unrelated session was deleted")
	}
	got, ok := st.Workspace("WS-1")
	if !ok {
		t.Fatal("workspace missing")
	}
	if strings.Contains(strings.Join(got.ChatTabOrder, ","), "PA2") {
		t.Fatalf("ChatTabOrder still has deleted code: %v", got.ChatTabOrder)
	}
	if len(got.ChatTabOrder) != 1 || got.ChatTabOrder[0] != "PA" {
		t.Fatalf("ChatTabOrder = %v, want [PA]", got.ChatTabOrder)
	}
}

func TestWriteSessionContextAndForkCopiesIt(t *testing.T) {
	st := openTemp(t, t.TempDir())
	parent, err := st.CreateSession(domain.Session{
		WorkspaceID: "WS-1", Kind: "coder",
		ContextMode: domain.ContextModeProvider, Provider: domain.ProviderCursor,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	early, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("ancient")},
	})
	if err != nil {
		t.Fatalf("append early: %v", err)
	}
	compacted, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("done with the old file")},
	})
	if err != nil {
		t.Fatalf("append compacted: %v", err)
	}
	live, err := st.AppendMessage(parent.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("keep going")},
	})
	if err != nil {
		t.Fatalf("append live: %v", err)
	}

	ctx, err := st.WriteSessionContext(parent.ID, "older turns folded", compacted.ID)
	if err != nil {
		t.Fatalf("WriteSessionContext: %v", err)
	}
	got, ok := st.ActiveSessionContext(parent.ID)
	if !ok {
		t.Fatal("ActiveSessionContext(parent) missing after write")
	}
	if got.ID != ctx.ID || got.Summary != "older turns folded" || got.LastCompactedMessageID != compacted.ID {
		t.Errorf("ActiveSessionContext(parent) = %+v, want id=%s summary=%q watermark=%s",
			got, ctx.ID, "older turns folded", compacted.ID)
	}
	sess, ok := st.GetSession(parent.ID)
	if !ok || sess.ActiveContextID != ctx.ID {
		t.Errorf("GetSession.ActiveContextID = %q, want %s", sess.ActiveContextID, ctx.ID)
	}

	child, err := st.ForkSession(parent.ID, live.ID, "coder", `{"code":"PA2"}`)
	if err != nil {
		t.Fatalf("ForkSession at live: %v", err)
	}
	if child.ActiveContextID == "" || child.ActiveContextID == ctx.ID {
		t.Errorf("ForkSession.ActiveContextID = %q, want a new context id (not parent %s)",
			child.ActiveContextID, ctx.ID)
	}
	childCtx, ok := st.ActiveSessionContext(child.ID)
	if !ok {
		t.Fatal("ActiveSessionContext(child) missing after fork through watermark")
	}
	if childCtx.Summary != "older turns folded" || childCtx.LastCompactedMessageID != compacted.ID {
		t.Errorf("ActiveSessionContext(child) = %+v, want summary=%q watermark=%s",
			childCtx, "older turns folded", compacted.ID)
	}
	if childCtx.SessionID != child.ID {
		t.Errorf("child context SessionID = %q, want %s", childCtx.SessionID, child.ID)
	}

	before, err := st.ForkSession(parent.ID, early.ID, "coder", `{"code":"PA3"}`)
	if err != nil {
		t.Fatalf("ForkSession before compact: %v", err)
	}
	if before.ActiveContextID != "" {
		t.Errorf("ForkSession before watermark ActiveContextID = %q, want empty", before.ActiveContextID)
	}
	if _, ok := st.ActiveSessionContext(before.ID); ok {
		t.Error("ActiveSessionContext(fork-before-compact) should be missing")
	}

	if _, err := st.WriteSessionContext(parent.ID, "", compacted.ID); err == nil {
		t.Error("WriteSessionContext empty summary should fail")
	}
}

func idsOf(msgs []domain.Message) []string {
	out := make([]string, len(msgs))
	for i, m := range msgs {
		out[i] = m.ID
	}
	return out
}

func lastText(msgs []domain.Message) string {
	if len(msgs) == 0 || len(msgs[len(msgs)-1].Content) == 0 {
		return ""
	}
	p, ok := msgs[len(msgs)-1].Content[0].AsText()
	if !ok {
		return ""
	}
	return p.Text
}
