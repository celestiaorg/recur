package app

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestClampShellState(t *testing.T) {
	got := clampShellState(ShellState{
		View: "detail", WorkspaceID: "WS-1", PRID: "pr-1", ProjectID: "p1",
	})
	if got != (ShellState{View: "detail", WorkspaceID: "WS-1"}) {
		t.Fatalf("detail should keep only workspace: %+v", got)
	}

	got = clampShellState(ShellState{View: "inbox", WorkspaceID: "WS-1"})
	if got != (ShellState{View: "inbox"}) {
		t.Fatalf("inbox should drop ids: %+v", got)
	}

	got = clampShellState(ShellState{View: "schedules", WorkspaceID: "WS-1"})
	if got != (ShellState{View: "schedules"}) {
		t.Fatalf("schedules should drop ids: %+v", got)
	}

	got = clampShellState(ShellState{View: "issues", WorkspaceID: "WS-1"})
	if got != (ShellState{View: "issues"}) {
		t.Fatalf("issues should drop ids: %+v", got)
	}

	got = clampShellState(ShellState{View: "briefing", WorkspaceID: "WS-1"})
	if got != (ShellState{View: "briefing"}) {
		t.Fatalf("briefing should drop ids: %+v", got)
	}

	got = clampShellState(ShellState{View: "nope", WorkspaceID: "WS-1"})
	if got != (ShellState{}) {
		t.Fatalf("unknown view should zero: %+v", got)
	}
}

func TestShellStateRoundTrip(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)

	svc := &ShellService{}
	if got := svc.GetShellState(); got != (ShellState{}) {
		t.Fatalf("empty data dir: %+v", got)
	}

	want := ShellState{View: "detail", WorkspaceID: "WS-9"}
	if err := svc.SetShellState(want); err != nil {
		t.Fatal(err)
	}
	if got := svc.GetShellState(); got != want {
		t.Fatalf("memory mismatch before flush: got %+v want %+v", got, want)
	}
	svc.Flush()

	path := filepath.Join(dir, shellStateFile)
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var file ShellState
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	if file != want {
		t.Fatalf("file mismatch: got %+v want %+v", file, want)
	}

	if got := svc.GetShellState(); got != want {
		t.Fatalf("load mismatch: got %+v want %+v", got, want)
	}

	// Same write is a no-op (file still matches).
	if err := svc.SetShellState(want); err != nil {
		t.Fatal(err)
	}

	// Inbox strips leftover workspace id.
	if err := svc.SetShellState(ShellState{View: "inbox", WorkspaceID: "WS-9"}); err != nil {
		t.Fatal(err)
	}
	svc.Flush()
	if got := svc.GetShellState(); got != (ShellState{View: "inbox"}) {
		t.Fatalf("inbox persist: %+v", got)
	}
}

func TestSetShellStateUnknownPreservesFile(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)
	svc := &ShellService{}
	want := ShellState{View: "detail", WorkspaceID: "WS-9"}
	if err := svc.SetShellState(want); err != nil {
		t.Fatal(err)
	}
	svc.Flush()
	if err := svc.SetShellState(ShellState{View: "nope", WorkspaceID: "WS-9"}); err != nil {
		t.Fatal(err)
	}
	if err := svc.SetShellState(ShellState{View: "welcome"}); err != nil {
		t.Fatal(err)
	}
	if got := svc.GetShellState(); got != want {
		t.Fatalf("unknown/welcome write wiped shell: got %+v want %+v", got, want)
	}
}

func TestSetShellStateDebouncedUntilFlush(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)
	svc := &ShellService{}
	want := ShellState{View: "detail", WorkspaceID: "WS-1"}
	if err := svc.SetShellState(want); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, shellStateFile)); !os.IsNotExist(err) {
		t.Fatalf("disk write happened before Flush: %v", err)
	}
	if got := svc.GetShellState(); got != want {
		t.Fatalf("Get before Flush: %+v", got)
	}
	svc.Flush()
	raw, err := os.ReadFile(filepath.Join(dir, shellStateFile))
	if err != nil {
		t.Fatal(err)
	}
	var file ShellState
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	if file != want {
		t.Fatalf("flushed file: %+v", file)
	}
}

func TestFlushRetriesAfterWriteError(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)
	svc := &ShellService{}
	want := ShellState{View: "settings"}
	if err := svc.SetShellState(want); err != nil {
		t.Fatal(err)
	}
	block := filepath.Join(dir, shellStateFile)
	if err := os.Mkdir(block, 0o755); err != nil {
		t.Fatal(err)
	}
	svc.Flush()
	if svc.failDelay < shellSaveDebounce*2 {
		t.Fatalf("backoff not armed after failed flush: %s", svc.failDelay)
	}
	if _, err := os.Stat(filepath.Join(dir, shellStateFile+".tmp")); !os.IsNotExist(err) {
		t.Fatalf("tmp left behind after failed flush: %v", err)
	}
	if err := os.Remove(block); err != nil {
		t.Fatal(err)
	}
	svc.Flush()
	raw, err := os.ReadFile(block)
	if err != nil {
		t.Fatal(err)
	}
	var file ShellState
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	if file != want {
		t.Fatalf("retried flush: %+v", file)
	}
}

func TestLoadShellStateCorrupt(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)
	if err := os.WriteFile(filepath.Join(dir, shellStateFile), []byte("{not json"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := (&ShellService{}).GetShellState(); got != (ShellState{}) {
		t.Fatalf("corrupt file should zero: %+v", got)
	}
}

func TestExpandedSchedulesRoundTrip(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)

	svc := &ShellService{}
	if got := svc.GetExpandedSchedules(); len(got) != 0 {
		t.Fatalf("empty data dir: %v", got)
	}
	if err := svc.SetExpandedSchedules([]string{"a", "", "a", "b"}); err != nil {
		t.Fatal(err)
	}
	if got := svc.GetExpandedSchedules(); len(got) != 2 || got[0] != "a" || got[1] != "b" {
		t.Fatalf("memory: %v", got)
	}

	raw, err := os.ReadFile(filepath.Join(dir, scheduleViewFile))
	if err != nil {
		t.Fatal(err)
	}
	var file scheduleView
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	if len(file.Expanded) != 2 || file.Expanded[0] != "a" || file.Expanded[1] != "b" {
		t.Fatalf("file: %+v", file)
	}

	if got := (&ShellService{}).GetExpandedSchedules(); len(got) != 2 || got[0] != "a" || got[1] != "b" {
		t.Fatalf("reload: %v", got)
	}
}

func TestExpandedSchedulesCorrupt(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)
	if err := os.WriteFile(filepath.Join(dir, scheduleViewFile), []byte("{not json"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := (&ShellService{}).GetExpandedSchedules(); len(got) != 0 {
		t.Fatalf("corrupt file should empty: %v", got)
	}
}

func TestIssuesViewRoundTrip(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)
	svc := &ShellService{}
	if got := svc.GetIssuesView(); got.Layout != "" || got.Groups != nil {
		t.Fatalf("empty data dir: %+v", got)
	}
	want := IssuesView{Layout: "board", Source: "linear", Owner: "mine", Groups: []string{"todo"}, Sort: "priority", Projects: []string{"p1"}}
	if err := svc.SetIssuesView(want); err != nil {
		t.Fatal(err)
	}
	got := (&ShellService{}).GetIssuesView()
	if got.Layout != "board" || got.Source != "linear" || got.Owner != "mine" || got.Sort != "priority" ||
		len(got.Groups) != 1 || got.Groups[0] != "todo" || len(got.Projects) != 1 || got.Projects[0] != "p1" {
		t.Fatalf("reload: %+v", got)
	}
	if err := os.WriteFile(filepath.Join(dir, issuesViewFile), []byte("{not json"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := svc.GetIssuesView(); got.Layout != "" {
		t.Fatalf("corrupt file should zero: %+v", got)
	}
}
