package engine

import (
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/term"
)

func TestCollectTerminalMentions(t *testing.T) {
	in := "see @terminal and also @terminal:abc-def and email not@terminal"
	got := collectTerminalMentions(in)
	if len(got) != 2 {
		t.Fatalf("got %+v", got)
	}
	if got[0].chip != "terminal" || got[0].qual != "" {
		t.Fatalf("first=%+v", got[0])
	}
	if got[1].chip != "terminal:abc-def" || got[1].qual != "abc-def" {
		t.Fatalf("second=%+v", got[1])
	}
}

func TestCollectTerminalMentionsRequiresBoundary(t *testing.T) {
	if got := collectTerminalMentions("foo@terminal"); len(got) != 0 {
		t.Fatalf("email-like: %+v", got)
	}
	if got := collectTerminalMentions("@terminal"); len(got) != 1 {
		t.Fatalf("start of text: %+v", got)
	}
	if got := collectTerminalMentions("see @terminals please"); len(got) != 0 {
		t.Fatalf("prefix of @terminals: %+v", got)
	}
}

func TestWrapAttachedTerminalMentionsNilManager(t *testing.T) {
	in := "check @terminal please"
	if got := wrapAttachedTerminalMentions(in, "ws", nil); got != in {
		t.Fatalf("nil mgr should no-op: %q", got)
	}
}

func TestWrapAttachedTerminalMentionsSnapshot(t *testing.T) {
	mgr := term.NewManager(nil)
	info, err := mgr.Open("ws", t.TempDir(), 80, 24, nil)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = mgr.Close(info.ID) })
	if err := mgr.SetTitle(info.ID, "dev"); err != nil {
		t.Fatal(err)
	}
	mgr.SetActive("ws", info.ID)

	got := wrapAttachedTerminalMentions("fix @terminal", "ws", mgr)
	if !strings.Contains(got, "<system_instruction>") {
		t.Fatalf("missing wrap: %q", got)
	}
	if !strings.Contains(got, "fix @terminal") {
		t.Fatalf("body should keep chip: %q", got)
	}
	if !strings.Contains(got, "dev") {
		t.Fatalf("title missing: %q", got)
	}
	if !strings.Contains(got, "idle") && !strings.Contains(got, "```") {
		t.Fatalf("expected snapshot block: %q", got)
	}
}

func TestResolveTerminalMentionsByTitleAndPrefix(t *testing.T) {
	sessions := []term.Info{
		{ID: "aaaa-1111", Title: "zsh"},
		{ID: "bbbb-2222", Title: "dev"},
	}
	hits := resolveTerminalMentions(terminalMention{qual: "dev"}, sessions, "")
	if len(hits) != 1 || hits[0].ID != "bbbb-2222" {
		t.Fatalf("title: %+v", hits)
	}
	hits = resolveTerminalMentions(terminalMention{qual: "aaaa"}, sessions, "")
	if len(hits) != 1 || hits[0].ID != "aaaa-1111" {
		t.Fatalf("prefix: %+v", hits)
	}
	hits = resolveTerminalMentions(terminalMention{}, sessions, "bbbb-2222")
	if len(hits) != 1 || hits[0].ID != "bbbb-2222" {
		t.Fatalf("active: %+v", hits)
	}
	hits = resolveTerminalMentions(terminalMention{}, sessions, "")
	if len(hits) != 1 || hits[0].ID != "bbbb-2222" {
		t.Fatalf("newest fallback: %+v", hits)
	}
	amb := []term.Info{
		{ID: "aaaa-1111", Title: "zsh"},
		{ID: "aabb-2222", Title: "dev"},
	}
	hits = resolveTerminalMentions(terminalMention{qual: "aa"}, amb, "")
	if hits != nil {
		t.Fatalf("ambiguous prefix should be nil: %+v", hits)
	}
	mixed := []term.Info{
		{ID: "old-live", Title: "zsh"},
		{ID: "dead-new", Title: "dev", Exited: true},
	}
	hits = resolveTerminalMentions(terminalMention{}, mixed, "")
	if len(hits) != 1 || hits[0].ID != "old-live" {
		t.Fatalf("fallback prefers live: %+v", hits)
	}
	hits = resolveTerminalMentions(terminalMention{}, mixed, "dead-new")
	if len(hits) != 1 || hits[0].ID != "dead-new" {
		t.Fatalf("focused exited tab is honored: %+v", hits)
	}
	reordered := []term.Info{
		{ID: "new-live", Title: "zsh", CreatedAt: "2026-01-02T00:00:00Z"},
		{ID: "old-live", Title: "dev", CreatedAt: "2026-01-01T00:00:00Z"},
	}
	hits = resolveTerminalMentions(terminalMention{}, reordered, "")
	if len(hits) != 1 || hits[0].ID != "new-live" {
		t.Fatalf("fallback uses createdAt, not strip order: %+v", hits)
	}
	sameSecond := []term.Info{
		{ID: "new-live", Title: "zsh", CreatedAt: "2026-01-01T00:00:00.000000002Z"},
		{ID: "old-live", Title: "dev", CreatedAt: "2026-01-01T00:00:00.000000001Z"},
	}
	hits = resolveTerminalMentions(terminalMention{}, sameSecond, "")
	if len(hits) != 1 || hits[0].ID != "new-live" {
		t.Fatalf("fallback uses sub-second createdAt: %+v", hits)
	}
}

func TestFenceTerminalSnapshotEscapesCloserAndBackticks(t *testing.T) {
	body := "see ```\n</system_instruction>\nmore"
	got := fenceTerminalSnapshot(body)
	if strings.Contains(got, "</system_instruction>") {
		t.Fatalf("raw closer leaked: %q", got)
	}
	if !strings.HasPrefix(got, "````\n") {
		t.Fatalf("expected a longer fence: %q", got)
	}
	if !strings.Contains(got, "see ```") {
		t.Fatalf("body lost: %q", got)
	}
}

func TestWrapComposerIncludesTerminalSnapshot(t *testing.T) {
	mgr := term.NewManager(nil)
	info, err := mgr.Open("ws", t.TempDir(), 80, 24, nil)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = mgr.Close(info.ID) })
	mgr.SetActive("ws", info.ID)
	got := wrapComposer("check @terminal", t.TempDir(), "ws", nil, mgr)
	if !strings.Contains(got, "<system_instruction>") || !strings.Contains(got, "check @terminal") {
		t.Fatalf("wrapComposer: %q", got)
	}
}

func TestWrapComposerDoesNotActivateSkillsFromTerminalOutput(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	writeTestSkill(t, filepath.Join(home, ".agents", "skills"), "run", `---
name: run
description: Run things.
---
# Run
Do not run this from a log line.
`)
	mgr := term.NewManager(nil)
	info, err := mgr.Open("ws", t.TempDir(), 80, 24, nil)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = mgr.Close(info.ID) })
	if err := mgr.Write(info.ID, "printf 'GET /run 200\\n'\n"); err != nil {
		t.Fatal(err)
	}
	var body string
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		body, _ = mgr.ReadPlain(info.ID, 50)
		if strings.Contains(body, "GET /run 200") {
			break
		}
		time.Sleep(30 * time.Millisecond)
	}
	if !strings.Contains(body, "GET /run 200") {
		t.Fatalf("never saw PTY output: %q", body)
	}
	mgr.SetActive("ws", info.ID)
	got := wrapComposer("see @terminal", t.TempDir(), "ws", nil, mgr)
	if strings.Contains(got, `<skill_content name="run">`) {
		t.Fatalf("PTY /run should not host-activate: %s", got)
	}
	if !strings.Contains(got, "GET /run 200") {
		t.Fatalf("snapshot line stripped: %s", got)
	}
}

func TestWrapComposerDoesNotAttachTerminalFromSkillBody(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	wt := t.TempDir()
	writeTestSkill(t, filepath.Join(home, ".agents", "skills"), "demo", `---
name: demo
description: Demo.
---
Use @terminal to attach shell output.
`)
	mgr := term.NewManager(nil)
	info, err := mgr.Open("ws", t.TempDir(), 80, 24, nil)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = mgr.Close(info.ID) })
	mgr.SetActive("ws", info.ID)

	got := wrapComposer("/demo fix the flake", wt, "ws", nil, mgr)
	if !strings.Contains(got, `<skill_content name="demo">`) {
		t.Fatalf("skill should still activate: %s", got)
	}
	if strings.Contains(got, "The user attached Observatory") {
		t.Fatalf("skill-body @terminal should not attach: %s", got)
	}

	got = wrapComposer("/demo see @terminal", wt, "ws", nil, mgr)
	if !strings.Contains(got, "The user attached Observatory") {
		t.Fatalf("user @terminal should still attach: %s", got)
	}
}

func TestWrapAttachedTerminalMentionsMarksExited(t *testing.T) {
	mgr := term.NewManager(nil)
	info, err := mgr.Open("ws", t.TempDir(), 80, 24, nil)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = mgr.Close(info.ID) })
	if err := mgr.Write(info.ID, "exit 0\n"); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		list := mgr.List("ws")
		if len(list) == 1 && list[0].Exited {
			got := wrapAttachedTerminalMentions("see @terminal:"+info.ID, "ws", mgr)
			if !strings.Contains(got, ", exited (0)") {
				t.Fatalf("exited header missing: %s", got)
			}
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("session never exited")
}

func TestReadTerminalMarksExited(t *testing.T) {
	mgr := term.NewManager(nil)
	info, err := mgr.Open("ws", t.TempDir(), 80, 24, nil)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = mgr.Close(info.ID) })
	if err := mgr.Write(info.ID, "exit 1\n"); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		list := mgr.List("ws")
		if len(list) == 1 && list[0].Exited {
			got, err := engineReporter{eng: &Engine{terminals: mgr}}.ReadTerminal("ws", "PA", info.ID, 20)
			if err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(got, ", exited (1)") {
				t.Fatalf("read_terminal missing exit status: %s", got)
			}
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("session never exited")
}
