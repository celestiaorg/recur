package engine

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func installFreshSessionPrompts(t *testing.T) {
	t.Helper()
	dataDir := os.Getenv("RECUR_DATA_DIR")
	root := filepath.Join(dataDir, "Bundles", "ChatTest")
	if err := os.MkdirAll(filepath.Join(root, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	body := "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE\",\"display\":\"Act\"}'\n"
	if err := os.WriteFile(filepath.Join(root, "src", "act.sh"), []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	toml := `[bundle]
name = "ChatTest"
id = "recur.chattest"
version = "1.0.0"

[[prompt]]
id = "recur.chattest.re-review"
name = "Re-review"
slash = "re-review"
fresh_session = true
interpreter = "shebang"
script = "src/act.sh"

[[prompt]]
id = "recur.chattest.fix-findings"
name = "Fix Findings"
slash = "fix-findings"
interpreter = "shebang"
script = "src/act.sh"
`
	if err := os.WriteFile(filepath.Join(root, "bundle.toml"), []byte(toml), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestExpandComposerFreshSession(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	installFreshSessionPrompts(t)

	got, err := eng.ExpandComposer(context.Background(), "/re-review", expandOpts{ignoreDisabled: true})
	if err != nil {
		t.Fatal(err)
	}
	if !got.FreshSession {
		t.Errorf("ExpandComposer(/re-review) FreshSession = false, want true")
	}

	fix, err := eng.ExpandComposer(context.Background(), "/fix-findings", expandOpts{ignoreDisabled: true})
	if err != nil {
		t.Fatal(err)
	}
	if fix.FreshSession {
		t.Errorf("ExpandComposer(/fix-findings) FreshSession = true, want false")
	}
}

func TestFreshSessionPromptSkipsResume(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "ok")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	installFreshSessionPrompts(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fresh-review", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hello", "sonnet", nil, nil, "", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "/re-review", "sonnet", nil, nil, "review", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "/re-review", "sonnet", nil, nil, "review", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "look at auth", "sonnet", nil, nil, "review", "")

	turns := runner.specsWithAgentID(ws.ID + "-PA")
	if len(turns) != 4 {
		t.Fatalf("SendWorkspaceMessage turns = %d, want 4", len(turns))
	}
	if turns[0].ResumeSession != "" {
		t.Errorf("SendWorkspaceMessage(hello) ResumeSession = %q, want empty", turns[0].ResumeSession)
	}
	if turns[1].ResumeSession != "" {
		t.Errorf("SendWorkspaceMessage(/re-review) ResumeSession = %q, want empty (isolate)", turns[1].ResumeSession)
	}
	if turns[2].ResumeSession != "" {
		t.Errorf("SendWorkspaceMessage(/re-review #2) ResumeSession = %q, want empty (isolate)", turns[2].ResumeSession)
	}
	if turns[3].ResumeSession != "sess-3" {
		t.Errorf("SendWorkspaceMessage(follow-up) ResumeSession = %q, want sess-3", turns[3].ResumeSession)
	}
}

func TestFreshSessionCTAArgSkipsResume(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "ok")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fresh-cta", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hello", "sonnet", nil, nil, "", "")
	_ = eng.SendWorkspaceTurn(ws.ID, "PA", "WIRE review", "sonnet", nil, nil, "review", "Re-review", true)

	turns := runner.specsWithAgentID(ws.ID + "-PA")
	if len(turns) != 2 {
		t.Fatalf("SendWorkspaceTurn turns = %d, want 2", len(turns))
	}
	if turns[1].ResumeSession != "" {
		t.Errorf("SendWorkspaceTurn(freshSession) ResumeSession = %q, want empty", turns[1].ResumeSession)
	}
	if turns[1].Prompt != "WIRE review" {
		t.Errorf("SendWorkspaceTurn(freshSession) Prompt = %q, want WIRE review", turns[1].Prompt)
	}
}

func TestFreshSessionSkipsForkInject(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "ok")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	installFreshSessionPrompts(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fresh-fork", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "first question", "sonnet", nil, nil, "", "")
	tip := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if tip == "" {
		t.Fatal("first turn footer missing message id")
	}
	tab := eng.ForkWorkspaceChat(ws.ID, "PA", tip)
	if tab.Code != "PA2" {
		t.Fatalf("ForkWorkspaceChat code = %q, want PA2", tab.Code)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "/re-review", "sonnet", nil, nil, "review", "")
	specs := runner.specsWithAgentID(ws.ID + "-PA2")
	if len(specs) != 1 {
		t.Fatalf("PA2 turns = %d, want 1", len(specs))
	}
	if specs[0].ResumeSession != "" {
		t.Errorf("isolated fork send ResumeSession = %q, want empty", specs[0].ResumeSession)
	}
	if strings.Contains(specs[0].Prompt, forkPreambleLead) {
		t.Errorf("isolated fork send Prompt contains fork preamble, want only the action wire: %q", specs[0].Prompt)
	}
	if !strings.Contains(specs[0].Prompt, "WIRE") {
		t.Errorf("isolated fork send Prompt = %q, want WIRE", specs[0].Prompt)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "explain finding 2", "sonnet", nil, nil, "review", "")
	specs = runner.specsWithAgentID(ws.ID + "-PA2")
	if len(specs) != 2 {
		t.Fatalf("PA2 turns after follow-up = %d, want 2", len(specs))
	}
	if specs[1].ResumeSession != "sess-2" {
		t.Errorf("SendWorkspaceMessage(follow-up after isolated fork review) ResumeSession = %q, want sess-2", specs[1].ResumeSession)
	}
	if strings.Contains(specs[1].Prompt, forkPreambleLead) {
		t.Errorf("follow-up Prompt contains fork preamble, want resume of isolated session: %q", specs[1].Prompt)
	}
}

func TestFreshSessionRetryAfterPreInitFailureStaysIsolated(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	n := 0
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			n++
			if call == 1 {
				return nil
			}
			return maestroFixture("sess-"+strconv.Itoa(n), "ok")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	installFreshSessionPrompts(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fresh-retry", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hello", "sonnet", nil, nil, "", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "/re-review", "sonnet", nil, nil, "review", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "", "sonnet", nil, nil, "review", chatRetryDisplaySentinel)

	turns := runner.specsWithAgentID(ws.ID + "-PA")
	if len(turns) != 3 {
		t.Fatalf("SendWorkspaceMessage turns = %d, want 3", len(turns))
	}
	if turns[1].ResumeSession != "" {
		t.Errorf("SendWorkspaceMessage(/re-review) ResumeSession = %q, want empty", turns[1].ResumeSession)
	}
	if turns[2].ResumeSession != "" {
		t.Errorf("SendWorkspaceMessage(retry after isolated pre-init failure) ResumeSession = %q, want empty", turns[2].ResumeSession)
	}
}

func TestFreshSessionNativeHistoryDropsPriorTurns(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	n := 0
	runner := &captureRunner{
		linesFor: func(int) [][]byte {
			n++
			return maestroFixture("sess-"+strconv.Itoa(n), "ok")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	installFreshSessionPrompts(t)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fresh-native", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hello from last week", "sonnet", nil, nil, "", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "/re-review", "sonnet", nil, nil, "review", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "look at auth", "sonnet", nil, nil, "review", "")

	turns := runner.specsWithAgentID(ws.ID + "-PA")
	if len(turns) != 3 {
		t.Fatalf("SendWorkspaceMessage turns = %d, want 3", len(turns))
	}
	if turns[1].HistoryAfterID == "" {
		t.Fatal("isolated /re-review HistoryAfterID = empty, want cutoff")
	}
	if turns[2].HistoryAfterID != turns[1].HistoryAfterID {
		t.Errorf("follow-up HistoryAfterID = %q, want isolated cutoff %q", turns[2].HistoryAfterID, turns[1].HistoryAfterID)
	}

	msgs, err := st.SessionTranscript(turns[1].RecurSessionID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	raw, _ := json.Marshal(harnessHistory(msgs, turns[1]))
	s := string(raw)
	if strings.Contains(s, "hello from last week") {
		t.Errorf("isolated harnessHistory leaked prior turn: %s", s)
	}
	if !strings.Contains(s, "WIRE") {
		t.Errorf("isolated harnessHistory missing isolated prompt: %s", s)
	}

	follow, _ := json.Marshal(harnessHistory(msgs, turns[2]))
	fs := string(follow)
	if strings.Contains(fs, "hello from last week") {
		t.Errorf("follow-up harnessHistory leaked prior turn: %s", fs)
	}
	if !strings.Contains(fs, "look at auth") {
		t.Errorf("follow-up harnessHistory missing follow-up: %s", fs)
	}

	items := eng.WorkspaceChatItems(ws.ID, "PA")
	visible := false
	for _, it := range items {
		if it.Role == "user" && strings.Contains(it.Text, "hello from last week") {
			visible = true
			break
		}
	}
	if !visible {
		t.Fatal("visible transcript dropped hello from last week")
	}

	tip := turnMessageID(eng.WorkspaceChatItems(ws.ID, "PA"))
	if tip == "" {
		t.Fatal("turn footer missing")
	}
	tab := eng.ForkWorkspaceChat(ws.ID, "PA", tip)
	if tab.Code == "" {
		t.Fatal("ForkWorkspaceChat failed")
	}
	child, ok := st.SessionForLoopAgent(ws.ID, tab.Code)
	if !ok {
		t.Fatal("fork session missing")
	}
	if got := nativeHistoryAfterID(child.ConfigJSON); got != "" {
		t.Errorf("fork NativeHistoryAfter = %q, want empty so copied history is kept", got)
	}
}
