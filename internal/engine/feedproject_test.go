package engine

import (
	"encoding/json"
	"sort"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

// TestFeedProjectionParity is S2's gate: the reload-path feed projection (agent
// tool/note rows from mirrored sessions + outcomes/deviations from the events
// table) must reproduce the live feed that streamed while the loop ran. It drives
// a real converging loop to a PR, then compares the live in-memory feed against
// ReconstructFeed as a multiset of (agentCode, kind, text, toolResult).
//
// Multiset (not exact order) because a hermetic scripted run fires many events in
// the same millisecond, so the chronological merge's tie-break is not meaningful
// here; ordering is by real wall-clock time in production, where turns span
// seconds. What must hold is that no row is lost, invented, or misattributed.
func TestFeedProjectionParity(t *testing.T) {
	repo := initRepo(t)
	runner := &headlineRunner{
		reviewFindings: []int{2, 0}, // converge on iteration 2
		evidence: []stepEvidence{
			{step: "Booted the app and opened /", shot: "/shots/01.png", passed: true},
			{step: "Exercised the feature", shot: "/shots/02.png", passed: true},
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := verifyDevProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "Add a verifiable, reviewable feature", "", "", "test.loops.verify", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 30*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	live := st.Feed(id)
	if len(live) == 0 {
		t.Fatal("live feed is empty")
	}
	projected := eng.ReconstructFeed(id)

	key := func(it domain.FeedItem) string {
		return it.AgentCode + "\x00" + it.Kind + "\x00" + it.Text + "\x00" + it.ToolResult
	}
	multiset := func(items []domain.FeedItem) []string {
		out := make([]string, 0, len(items))
		for _, it := range items {
			out = append(out, key(it))
		}
		sort.Strings(out)
		return out
	}
	liveMS, projMS := multiset(live), multiset(projected)

	if len(liveMS) != len(projMS) {
		t.Errorf("feed row count: live=%d projected=%d", len(liveMS), len(projMS))
	}
	// Report the first divergence with context.
	max := len(liveMS)
	if len(projMS) > max {
		max = len(projMS)
	}
	for i := 0; i < max; i++ {
		var l, p string
		if i < len(liveMS) {
			l = liveMS[i]
		}
		if i < len(projMS) {
			p = projMS[i]
		}
		if l != p {
			t.Fatalf("feed projection diverges at %d:\n live: %q\n proj: %q", i, l, p)
		}
	}

	// Every kind must be represented on both sides (the projection genuinely merges
	// both sources — session-derived tool rows and event-derived outcomes).
	kinds := func(items []domain.FeedItem) map[string]int {
		m := map[string]int{}
		for _, it := range items {
			m[it.Kind]++
		}
		return m
	}
	lk, pk := kinds(live), kinds(projected)
	for _, want := range []string{domain.FeedKindTool, domain.FeedKindOutcome} {
		if lk[want] == 0 {
			t.Errorf("precondition: live feed had no %q rows to test", want)
		}
		if pk[want] != lk[want] {
			t.Errorf("kind %q: live=%d projected=%d", want, lk[want], pk[want])
		}
	}
}

// ReconstructFeed must route persisted Cursor/Codex tool_use blocks through
// displayTool, matching the live onAgentEvent path. Without that, a stored
// `delete` + `{"path":…}` dumps as JSON instead of `Delete: /wt/out.txt`.
func TestReconstructFeedCanonicalizesCursorDelete(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "feed-delete", "")
	if err != nil {
		t.Fatal(err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID,
		Kind:        string(domain.RoleCoder),
		Provider:    domain.ProviderCursor,
		ConfigJSON:  `{"code":"CO","name":"Coder"}`,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := st.AppendMessage(sess.ID, domain.Message{
		Role:     "assistant",
		Provider: domain.ProviderCursor,
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "c1", "delete", json.RawMessage(argsDelete)),
		},
	}); err != nil {
		t.Fatal(err)
	}

	feed := eng.ReconstructFeed(ws.ID)
	var row *domain.FeedItem
	for i := range feed {
		if feed[i].Kind == domain.FeedKindTool {
			row = &feed[i]
			break
		}
	}
	if row == nil {
		t.Fatalf("no tool row in %+v", feed)
	}
	if strings.Contains(row.Text, "{") {
		t.Errorf("reload dumped JSON: %s", row.Text)
	}
	if !strings.Contains(row.Text, "Delete") || !strings.Contains(row.Text, "/wt/out.txt") {
		t.Errorf("reload row = %q, want canonical Delete + path", row.Text)
	}
}

func TestStepKindDeleteIsEdit(t *testing.T) {
	if got := stepKind("Delete"); got != "edit" {
		t.Errorf("stepKind(Delete) = %q, want edit (file-mutation tint)", got)
	}
}
