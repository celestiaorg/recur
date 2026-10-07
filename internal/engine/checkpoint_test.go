package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// Keep a realistic number of completed turns so per-footer Git processes
// cannot hide behind a tiny transcript in performance checks.
func BenchmarkMarkCheckpoints(b *testing.B) {
	ws := domain.Workspace{ID: "bench", WorktreePath: initRepo(b)}
	head, err := exec.Command("git", "-C", ws.WorktreePath, "rev-parse", "HEAD").Output()
	if err != nil {
		b.Fatal(err)
	}
	var refs strings.Builder
	var msgs []domain.Message
	var items []domain.ChatItem
	for i := range 50 {
		user, footer := fmt.Sprintf("user%d", i), fmt.Sprintf("footer%d", i)
		for _, phase := range []string{"start", "end"} {
			fmt.Fprintf(&refs, "update %s %s\n", checkpointRef(ws.ID, user, phase), strings.TrimSpace(string(head)))
		}
		msgs = append(msgs,
			domain.Message{ID: user, Role: "user", Content: []domain.ContentBlock{domain.TextBlock("question")}},
			domain.Message{ID: footer, Role: "assistant", DurationMS: 1},
		)
		items = append(items, domain.ChatItem{Kind: "turn", MessageID: footer})
	}
	cmd := exec.Command("git", "-C", ws.WorktreePath, "update-ref", "--stdin")
	cmd.Stdin = strings.NewReader(refs.String())
	if out, err := cmd.CombinedOutput(); err != nil {
		b.Fatalf("checkpoint refs: %v\n%s", err, out)
	}
	eng := &Engine{}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		eng.markCheckpoints(ws, msgs, items)
	}
}

func TestMarkCheckpoints(t *testing.T) {
	ws := domain.Workspace{ID: "mark", WorktreePath: initRepo(t)}
	idx := t.TempDir()
	var msgs []domain.Message
	var items []domain.ChatItem
	var want []bool
	for _, tc := range []struct {
		user                string
		start, end, changed bool
	}{
		{"changed", true, true, true},
		{"unchanged", true, true, false},
		{"missing-end", true, false, true},
		{"missing-start", false, true, true},
	} {
		for _, phase := range []string{"start", "end"} {
			if phase == "end" && tc.changed {
				if err := os.WriteFile(filepath.Join(ws.WorktreePath, "edit.txt"), []byte(tc.user), 0o644); err != nil {
					t.Fatal(err)
				}
			}
			if (phase == "start" && !tc.start) || (phase == "end" && !tc.end) {
				continue
			}
			if _, err := gitpkg.Checkpoint(context.Background(), ws.WorktreePath, idx, checkpointRef(ws.ID, tc.user, phase)); err != nil {
				t.Fatal(err)
			}
		}
		msgs = append(msgs, domain.Message{
			ID: tc.user, Role: "user", Content: []domain.ContentBlock{domain.TextBlock("question")},
		})
		// A tool result carried in a user message is not a new human prompt.
		msgs = append(msgs, domain.Message{
			ID: tc.user + "-tool", Role: "user", Content: []domain.ContentBlock{{Type: domain.BlockToolResult}},
		})
		// Continuation footers still belong to the same opening prompt.
		for i := range 2 {
			footer := fmt.Sprintf("%s-footer%d", tc.user, i)
			msgs = append(msgs, domain.Message{ID: footer, Role: "assistant", DurationMS: 1})
			items = append(items, domain.ChatItem{Kind: "turn", MessageID: footer})
			want = append(want, tc.start && tc.end && tc.changed)
		}
	}
	items = append(items,
		domain.ChatItem{Kind: "text", MessageID: "changed-footer0"},
		domain.ChatItem{Kind: "turn", MessageID: "unknown"},
	)
	want = append(want, false, false)
	(&Engine{}).markCheckpoints(ws, msgs, items)
	for i, item := range items {
		if item.Checkpoint != want[i] {
			t.Errorf("%s %s checkpoint = %v, want %v", item.Kind, item.MessageID, item.Checkpoint, want[i])
		}
	}
}

// editingRunner writes a file into the worktree mid-turn, like an agent would.
type editingRunner struct {
	captureRunner
	write func(spec AgentSpec)
}

func (r *editingRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if r.write != nil {
		r.write(spec)
	}
	return r.captureRunner.Run(ctx, spec, emit)
}

// Each turn is checkpointed start/end; the footer's diff is that turn's edits,
// and reset restores the worktree and truncates the transcript at the footer.
func TestChatCheckpointTurnDiffAndReset(t *testing.T) {
	turn := 0
	runner := &editingRunner{captureRunner: captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-ckpt", "ok") },
	}}
	runner.write = func(spec AgentSpec) {
		turn++
		name := "turn1.txt"
		if turn == 2 {
			name = "turn2.txt"
		}
		_ = os.WriteFile(filepath.Join(spec.Worktree, name), []byte("agent\n"), 0o644)
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ckpt", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	// A pre-existing untracked file must survive reset, still untracked.
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "mine.txt"), []byte("user\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "first", "sonnet", nil, nil, "", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "second", "sonnet", nil, nil, "", "")

	var footers []domain.ChatItem
	for _, it := range eng.WorkspaceChatItems(ws.ID, "PA") {
		if it.Kind == "turn" {
			footers = append(footers, it)
		}
	}
	if len(footers) != 2 || !footers[0].Checkpoint || !footers[1].Checkpoint {
		t.Fatalf("footers = %+v", footers)
	}

	turn2, err := eng.WorkspaceChatTurnDiff(ws.ID, "PA", footers[1].MessageID)
	if err != nil {
		t.Fatalf("TurnDiff: %v", err)
	}
	if len(turn2.Files) != 1 || turn2.Files[0].Path != "turn2.txt" || turn2.Diff.Add != 1 || len(turn2.Hash) != 7 {
		t.Fatalf("turn 2 diff = %+v", turn2)
	}

	if err := eng.ResetWorkspaceChat(ws.ID, "PA", footers[0].MessageID); err != nil {
		t.Fatalf("Reset: %v", err)
	}
	if _, err := os.Stat(filepath.Join(ws.WorktreePath, "turn2.txt")); !os.IsNotExist(err) {
		t.Fatal("turn2.txt should be gone after reset to turn 1")
	}
	for _, keep := range []string{"turn1.txt", "mine.txt"} {
		if _, err := os.Stat(filepath.Join(ws.WorktreePath, keep)); err != nil {
			t.Fatalf("%s should survive reset: %v", keep, err)
		}
	}
	out, err := exec.Command("git", "-C", ws.WorktreePath, "status", "--porcelain").Output()
	if err != nil || string(out) != "?? mine.txt\n?? turn1.txt\n" {
		t.Fatalf("status after reset = %q (%v)", out, err)
	}
	items := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if len(items) != 2 || items[0].Text != "first" {
		t.Fatalf("transcript after reset = %+v", items)
	}
	sess, _ := st.SessionForLoopAgent(ws.ID, "PA")
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if !cfg.ForkInject {
		t.Fatal("reset should arm forkInject so the next send continues from the truncated transcript")
	}
	if a, _ := st.Agent(ws.ID, "PA"); a.SessionID != "" {
		t.Fatalf("provider session should be cleared, got %q", a.SessionID)
	}
}

// A turn that did not touch the worktree still has start/end snapshots, but
// the footer must not offer a turn diff — it would be empty.
func TestChatCheckpointHiddenWhenTurnDidNotEdit(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-ckpt-empty", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ckpt-empty", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "just thinking", "sonnet", nil, nil, "", "")
	found := false
	for _, it := range eng.WorkspaceChatItems(ws.ID, "PA") {
		if it.Kind != "turn" {
			continue
		}
		found = true
		if it.Checkpoint {
			t.Fatal("empty turn should not offer checkpoint actions")
		}
	}
	if !found {
		t.Fatal("expected a turn footer")
	}
}

// Reset works on turns that never edited files: later messages go, and the
// worktree (including the user's manual edits) is left alone.
func TestChatResetWithoutEditsKeepsWorktree(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-reset-chat", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "reset-chat", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "first", "sonnet", nil, nil, "", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "typo", "sonnet", nil, nil, "", "")
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "mine.txt"), []byte("user\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	var footers []string
	for _, it := range eng.WorkspaceChatItems(ws.ID, "PA") {
		if it.Kind == "turn" {
			footers = append(footers, it.MessageID)
		}
	}
	if len(footers) != 2 {
		t.Fatalf("footers = %v", footers)
	}
	if err := eng.ResetWorkspaceChat(ws.ID, "PA", footers[0]); err != nil {
		t.Fatalf("Reset: %v", err)
	}
	if _, err := os.Stat(filepath.Join(ws.WorktreePath, "mine.txt")); err != nil {
		t.Fatalf("manual edit should survive a conversation-only reset: %v", err)
	}
	items := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	if len(items) != 2 || items[0].Text != "first" {
		t.Fatalf("transcript after reset = %+v", items)
	}
}

// /review stamps a per-tab review checkpoint; /re-review on the same tab (with
// a live provider session) gets since→now, elsewhere it stays a fresh full pass.
func TestReviewCheckpointDeltaOnlyOnSameChat(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-review", "ok") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "review-ckpt", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if s, n := eng.ReviewCheckpoint(ws.ID, "PA", "re-review"); s != "" || n != "" {
		t.Fatalf("re-review before any review should be full: %q %q", s, n)
	}
	if s, n := eng.ReviewCheckpoint(ws.ID, "PA", "review"); s != "" || n != "" {
		t.Fatalf("/review is always full: %q %q", s, n)
	}
	// No provider session on this tab yet → nothing to resume → full pass.
	if s, _ := eng.ReviewCheckpoint(ws.ID, "PA", "re-review"); s != "" {
		t.Fatalf("re-review without a live session should be full, got since=%q", s)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "review it", "sonnet", nil, nil, "", "")
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "fix.txt"), []byte("fixed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	since, now := eng.ReviewCheckpoint(ws.ID, "PA", "re-review")
	if since == "" || now == "" || since == now {
		t.Fatalf("re-review on the reviewed tab should be a delta: %q → %q", since, now)
	}
	out, _ := exec.Command("git", "-C", ws.WorktreePath, "diff", "--name-only", since, now).Output()
	if string(out) != "fix.txt\n" {
		t.Fatalf("delta = %q", out)
	}
	// A different tab never reviewed → full.
	if s, _ := eng.ReviewCheckpoint(ws.ID, "PA2", "re-review"); s != "" {
		t.Fatalf("other tab should be full, got since=%q", s)
	}
}

// An interrupted turn and AutoResume continuation share one checkpoint pair:
// start stays the pre-crash snapshot, end is the continuation's tree.
func TestChatCheckpointResumeReusesStart(t *testing.T) {
	runner := &editingRunner{captureRunner: captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-ckpt-restart", "ok") },
	}}
	runner.write = func(spec AgentSpec) {
		_ = os.WriteFile(filepath.Join(spec.Worktree, "post.txt"), []byte("after\n"), 0o644)
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ckpt-restart", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "sonnet")
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), pairAgentCode, "Agent", domain.ProviderClaudeCode)
	if !ok {
		t.Fatal("ensureAgentSession")
	}
	user, err := st.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("do the work")},
	})
	if err != nil || user.ID == "" {
		t.Fatalf("AppendMessage user: id=%q err=%v", user.ID, err)
	}
	eng.takeCheckpoint(ws, user.ID, "start")
	ctx := context.Background()
	start := checkpointRef(ws.ID, user.ID, "start")
	startOID := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, start)[start]
	if startOID == "" {
		t.Fatalf("takeCheckpoint start: ListCheckpoints(%s) empty", start)
	}
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "mid.txt"), []byte("mid\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)

	eng.Recover()
	eng.AutoResume()
	waitFor(t, 5*time.Second, "auto-resume chat turn to run", func() bool {
		spec, ok := runner.specWithRole(domain.RoleCoder)
		return ok && spec.SkipRecordUser
	})
	spec, _ := runner.specWithRole(domain.RoleCoder)
	if !spec.SkipRecordUser {
		t.Errorf("SkipRecordUser = false, want true so restart reuses the original checkpoint pair")
	}
	waitFor(t, 3*time.Second, "resumed chat to go idle", func() bool {
		a, ok := st.Agent(ws.ID, pairAgentCode)
		return ok && a.Status == "idle"
	})

	refs := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, gitpkg.CheckpointRefPrefix+ws.ID+"/")
	if refs[start] != startOID {
		t.Errorf("checkpoint start oid = %s, want %s (pre-crash start)", refs[start], startOID)
	}
	end := checkpointRef(ws.ID, user.ID, "end")
	if refs[end] == "" {
		t.Fatalf("checkpoint end missing for %s", end)
	}

	var footer domain.ChatItem
	items := eng.WorkspaceChatItems(ws.ID, pairAgentCode)
	for _, it := range items {
		if it.Role == "user" && (it.Text == chatResumeDisplay || strings.Contains(it.Text, "Recur restarted")) {
			t.Errorf("transcript has resume user bubble %q", it.Text)
		}
		if it.Kind == "turn" {
			footer = it
		}
	}
	if footer.MessageID == "" {
		t.Fatalf("no turn footer after resume, items=%+v", items)
	}
	diff, err := eng.WorkspaceChatTurnDiff(ws.ID, pairAgentCode, footer.MessageID)
	if err != nil {
		t.Fatalf("WorkspaceChatTurnDiff: %v", err)
	}
	got := map[string]bool{}
	for _, f := range diff.Files {
		got[f.Path] = true
	}
	for _, path := range []string{"mid.txt", "post.txt"} {
		if !got[path] {
			t.Errorf("WorkspaceChatTurnDiff files = %v, want %s (start→end spanning the restart)", got, path)
		}
	}
}

// A crash after a completed turn (or before the next user row is mirrored)
// must not reuse that turn's checkpoint pair — AutoResume opens a distinct turn.
func TestChatCheckpointResumeDoesNotStealCompletedTurn(t *testing.T) {
	turn := 0
	runner := &editingRunner{captureRunner: captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-ckpt-steal", "ok") },
	}}
	runner.write = func(spec AgentSpec) {
		turn++
		name := "first.txt"
		if turn > 1 {
			name = "resume.txt"
		}
		_ = os.WriteFile(filepath.Join(spec.Worktree, name), []byte("agent\n"), 0o644)
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ckpt-steal", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, pairAgentCode, "first", "sonnet", nil, nil, "", "")

	sess, ok := st.SessionForLoopAgent(ws.ID, pairAgentCode)
	if !ok {
		t.Fatal("SessionForLoopAgent")
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	userID := lastChatPromptID(st, ws.ID, pairAgentCode)
	if userID == "" {
		t.Fatalf("lastChatPromptID(%s, %s) empty after completed send, msgs=%d", ws.ID, pairAgentCode, len(msgs))
	}
	var footerID string
	for _, it := range eng.WorkspaceChatItems(ws.ID, pairAgentCode) {
		if it.Kind == "turn" {
			footerID = it.MessageID
		}
	}
	if footerID == "" {
		t.Fatalf("completed turn missing footer, items=%+v", eng.WorkspaceChatItems(ws.ID, pairAgentCode))
	}
	ctx := context.Background()
	end := checkpointRef(ws.ID, userID, "end")
	endOID := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, end)[end]
	if endOID == "" {
		t.Fatalf("completed turn missing end checkpoint %s", end)
	}

	persistWorkingChat(t, eng, ws.ID, pairAgentCode)
	eng.Recover()
	eng.AutoResume()
	waitFor(t, 5*time.Second, "auto-resume chat turn to run", func() bool {
		specs := runner.specsWithRole(domain.RoleCoder)
		return len(specs) >= 2 && !specs[len(specs)-1].SkipRecordUser
	})
	specs := runner.specsWithRole(domain.RoleCoder)
	resume := specs[len(specs)-1]
	if resume.SkipRecordUser {
		t.Errorf("SkipRecordUser = true, want false so a completed turn keeps its end checkpoint")
	}
	waitFor(t, 3*time.Second, "resumed chat to go idle", func() bool {
		a, ok := st.Agent(ws.ID, pairAgentCode)
		return ok && a.Status == "idle"
	})

	gotEnd := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, end)[end]
	if gotEnd != endOID {
		t.Errorf("completed turn end oid = %s, want %s (must not be overwritten by AutoResume)", gotEnd, endOID)
	}
	diff, err := eng.WorkspaceChatTurnDiff(ws.ID, pairAgentCode, footerID)
	if err != nil {
		t.Fatalf("WorkspaceChatTurnDiff: %v", err)
	}
	for _, f := range diff.Files {
		if f.Path == "resume.txt" {
			t.Errorf("completed turn diff includes %s; AutoResume stole the previous end checkpoint", f.Path)
		}
	}
}

// Graceful quit snapshots an interim end after cancel; AutoResume must still
// reuse that turn's start and replace the end.
func TestChatCheckpointResumeReusesInterimEnd(t *testing.T) {
	runner := &editingRunner{captureRunner: captureRunner{
		linesFor: func(int) [][]byte { return maestroFixture("sess-ckpt-interim", "ok") },
	}}
	runner.write = func(spec AgentSpec) {
		_ = os.WriteFile(filepath.Join(spec.Worktree, "post.txt"), []byte("after\n"), 0o644)
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ckpt-interim", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	eng.ensurePairAgent(ws.ID, pairAgentCode, "sonnet")
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), pairAgentCode, "Agent", domain.ProviderClaudeCode)
	if !ok {
		t.Fatal("ensureAgentSession")
	}
	user, err := st.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("do the work")},
	})
	if err != nil || user.ID == "" {
		t.Fatalf("AppendMessage user: id=%q err=%v", user.ID, err)
	}
	eng.takeCheckpoint(ws, user.ID, "start")
	ctx := context.Background()
	start := checkpointRef(ws.ID, user.ID, "start")
	startOID := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, start)[start]
	if startOID == "" {
		t.Fatalf("takeCheckpoint start: ListCheckpoints(%s) empty", start)
	}
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "mid.txt"), []byte("mid\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	eng.takeCheckpoint(ws, user.ID, "end")
	end := checkpointRef(ws.ID, user.ID, "end")
	endBefore := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, end)[end]
	if endBefore == "" {
		t.Fatalf("takeCheckpoint end: ListCheckpoints(%s) empty", end)
	}
	persistWorkingChat(t, eng, ws.ID, pairAgentCode)

	eng.Recover()
	eng.AutoResume()
	waitFor(t, 5*time.Second, "auto-resume chat turn to run", func() bool {
		spec, ok := runner.specWithRole(domain.RoleCoder)
		return ok && spec.SkipRecordUser
	})
	spec, _ := runner.specWithRole(domain.RoleCoder)
	if !spec.SkipRecordUser {
		t.Errorf("SkipRecordUser = false, want true so graceful-quit interim end is reused")
	}
	waitFor(t, 3*time.Second, "resumed chat to go idle", func() bool {
		a, ok := st.Agent(ws.ID, pairAgentCode)
		return ok && a.Status == "idle"
	})

	refs := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, gitpkg.CheckpointRefPrefix+ws.ID+"/")
	if refs[start] != startOID {
		t.Errorf("checkpoint start oid = %s, want %s (pre-crash start)", refs[start], startOID)
	}
	if refs[end] == "" || refs[end] == endBefore {
		t.Errorf("checkpoint end oid = %s, want replaced (was %s)", refs[end], endBefore)
	}
}
