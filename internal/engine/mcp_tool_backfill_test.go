package engine

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestNoteToolArgsBackfillsEmptyToolUse(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "mcp-backfill", "")
	if err != nil {
		t.Fatal(err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "coder", Provider: domain.ProviderCodex,
	})
	if err != nil {
		t.Fatal(err)
	}
	a, _ := st.Agent(ws.ID, "PA")
	a.SessionID = sess.ID
	st.UpsertAgent(ws.ID, a)

	msgID, err := st.CreateMessage(sess.ID, domain.Message{Role: "assistant"})
	if err != nil {
		t.Fatal(err)
	}
	if err := st.AppendContentBlock(msgID, domain.ToolUseBlock(
		"tu-skill", "item-1", "mcp__recur__activate_skill", json.RawMessage(`{}`),
	)); err != nil {
		t.Fatal(err)
	}

	want := json.RawMessage(`{"name":"go-testing"}`)
	engineReporter{eng: eng}.NoteToolArgs(ws.ID, "PA", "activate_skill", want)

	tu, ok := st.LatestToolUseByNameSuffix(sess.ID, "__activate_skill")
	if !ok || string(tu.Input) != string(want) {
		t.Fatalf("immediate backfill = %+v ok=%v", tu, ok)
	}

	deadline := time.Now().Add(500 * time.Millisecond)
	var sawInput bool
	for time.Now().Before(deadline) {
		for _, ev := range sink.byName("session.tool_input") {
			d, _ := ev.Data.(map[string]any)
			if d["id"] == "item-1" && strings.Contains(fmtString(d["input"]), "go-testing") {
				sawInput = true
				break
			}
		}
		if sawInput {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if !sawInput {
		t.Fatal("expected session.tool_input with activate_skill args")
	}
}

func TestNoteToolArgsSkipsNonEmptyAndEmptyPayload(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "mcp-backfill-skip", "")
	if err != nil {
		t.Fatal(err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "coder", Provider: domain.ProviderCodex,
	})
	if err != nil {
		t.Fatal(err)
	}
	a, _ := st.Agent(ws.ID, "PA")
	a.SessionID = sess.ID
	st.UpsertAgent(ws.ID, a)

	msgID, err := st.CreateMessage(sess.ID, domain.Message{Role: "assistant"})
	if err != nil {
		t.Fatal(err)
	}
	existing := json.RawMessage(`{"mode":"plan"}`)
	if err := st.AppendContentBlock(msgID, domain.ToolUseBlock(
		"tu-mode", "item-2", "mcp__recur__switch_mode", existing,
	)); err != nil {
		t.Fatal(err)
	}
	rep := engineReporter{eng: eng}
	rep.NoteToolArgs(ws.ID, "PA", "switch_mode", json.RawMessage(`{"mode":"ask"}`))
	tu, _ := st.LatestToolUseByNameSuffix(sess.ID, "__switch_mode")
	if string(tu.Input) != string(existing) {
		t.Fatalf("must not overwrite non-empty args: %s", tu.Input)
	}
	rep.NoteToolArgs(ws.ID, "PA", "ask_user", json.RawMessage(`{}`))
}

func fmtString(v any) string {
	s, _ := v.(string)
	return s
}
