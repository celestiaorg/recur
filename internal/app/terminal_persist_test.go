package app

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

func TestTerminalPersistRoundTripAcrossShutdown(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	repo := t.TempDir()

	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: projectID}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	info, err := svc.Terminal.OpenTerminal("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("OpenTerminal: %v", err)
	}
	if err := svc.Terminal.SetTerminalTitle(info.ID, "dev"); err != nil {
		t.Fatalf("SetTerminalTitle: %v", err)
	}
	svc.Terminal.SetActiveTerminal("ws-1", info.ID)
	if err := svc.Terminal.WriteTerminal(info.ID, "echo persist-marker\n"); err != nil {
		t.Fatalf("WriteTerminal: %v", err)
	}
	waitForReplay(t, svc, info.ID, "persist-marker")

	if err := svc.Terminal.ServiceShutdown(); err != nil {
		t.Fatalf("ServiceShutdown: %v", err)
	}

	svc2 := NewServices(eng)
	t.Cleanup(func() { _ = svc2.Terminal.ServiceShutdown() })
	restored, err := svc2.Terminal.RestoreWorkspaceTerminals("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("RestoreWorkspaceTerminals: %v", err)
	}
	if len(restored) != 1 {
		t.Fatalf("restored %d tabs, want 1", len(restored))
	}
	if restored[0].Title != "dev" {
		t.Errorf("title = %q, want dev", restored[0].Title)
	}
	if !restored[0].Active {
		t.Error("restored tab should be marked active")
	}
	if !sameDir(t, restored[0].Cwd, repo) {
		t.Errorf("cwd = %q, want repo %q", restored[0].Cwd, repo)
	}
	replay, err := svc2.Terminal.AttachTerminal(restored[0].ID)
	if err != nil {
		t.Fatalf("AttachTerminal: %v", err)
	}
	if !strings.Contains(decodeB64(t, replay), "persist-marker") {
		t.Errorf("restored replay missing history: %q", decodeB64(t, replay))
	}
}

func TestRestoreDoesNotDuplicateLiveSessions(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	repo := t.TempDir()
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	t.Cleanup(func() { _ = svc.Terminal.ServiceShutdown() })
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: projectID}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	info, err := svc.Terminal.OpenTerminal("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("OpenTerminal: %v", err)
	}

	got, err := svc.Terminal.RestoreWorkspaceTerminals("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("Restore: %v", err)
	}
	if len(got) != 1 || got[0].ID != info.ID {
		t.Fatalf("Restore while live = %+v, want the existing session", got)
	}
	if n := len(svc.Terminal.ListTerminals("ws-1")); n != 1 {
		t.Fatalf("List after Restore = %d, want 1", n)
	}
}

func TestClosingLastTabDropsPersistedSnapshot(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	repo := t.TempDir()
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	t.Cleanup(func() { _ = svc.Terminal.ServiceShutdown() })
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: projectID}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	info, err := svc.Terminal.OpenTerminal("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("OpenTerminal: %v", err)
	}
	svc.Terminal.flushPersist()
	if readTerminalSnapshot("ws-1") == nil {
		t.Fatal("expected a snapshot after open")
	}
	if err := svc.Terminal.CloseTerminal(info.ID); err != nil {
		t.Fatalf("CloseTerminal: %v", err)
	}
	svc.Terminal.flushPersist()
	if readTerminalSnapshot("ws-1") != nil {
		t.Fatal("snapshot should be removed after the last tab closes")
	}

	svc2 := NewServices(eng)
	t.Cleanup(func() { _ = svc2.Terminal.ServiceShutdown() })
	got, err := svc2.Terminal.RestoreWorkspaceTerminals("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("Restore: %v", err)
	}
	if len(got) != 0 {
		t.Fatalf("Restore after close = %d tabs, want 0", len(got))
	}
}

func TestRestoreFallsBackWhenSavedCwdIsGone(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	repo := t.TempDir()
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	t.Cleanup(func() { _ = svc.Terminal.ServiceShutdown() })
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: projectID}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if err := writeTerminalSnapshot("ws-1", persistedTerminals{
		Tabs: []persistedTab{{
			Title:  "zsh",
			Cwd:    filepath.Join(dataDir, "deleted-dir"),
			Replay: []byte("old-output\r\n"),
		}},
	}); err != nil {
		t.Fatalf("writeTerminalSnapshot: %v", err)
	}

	got, err := svc.Terminal.RestoreWorkspaceTerminals("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("Restore: %v", err)
	}
	if len(got) != 1 {
		t.Fatalf("restored %d, want 1", len(got))
	}
	if !sameDir(t, got[0].Cwd, repo) {
		t.Errorf("cwd = %q, want fallback repo %q", got[0].Cwd, repo)
	}
}

func TestRestoreKeepsLiveCwdWhenWorktreeIsGone(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	repo := t.TempDir()
	alive := t.TempDir()
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	t.Cleanup(func() { _ = svc.Terminal.ServiceShutdown() })
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{
		ID: "ws-1", ProjectID: projectID,
		WorktreePath: filepath.Join(t.TempDir(), "gone"),
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if err := writeTerminalSnapshot("ws-1", persistedTerminals{
		Tabs: []persistedTab{{
			Title:  "elsewhere",
			Cwd:    alive,
			Replay: []byte("kept\r\n"),
		}},
	}); err != nil {
		t.Fatalf("writeTerminalSnapshot: %v", err)
	}

	got, err := svc.Terminal.RestoreWorkspaceTerminals("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("RestoreWorkspaceTerminals() error = %v, want nil", err)
	}
	if len(got) != 1 {
		t.Fatalf("RestoreWorkspaceTerminals() restored %d tabs, want 1", len(got))
	}
	if !sameDir(t, got[0].Cwd, alive) {
		t.Errorf("RestoreWorkspaceTerminals() cwd = %q, want live tab cwd %q", got[0].Cwd, alive)
	}
}

func TestTerminalDockChromeRoundTripAcrossShutdown(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	repo := t.TempDir()

	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: projectID}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if _, err := svc.Terminal.OpenTerminal("ws-1", 80, 24); err != nil {
		t.Fatalf("OpenTerminal: %v", err)
	}
	if err := svc.Terminal.SetTerminalDockChrome("ws-1", TerminalDockChrome{
		Open: true, Maximized: true, EditorHidden: true, Beside: true, BesidePct: 40,
	}); err != nil {
		t.Fatalf("SetTerminalDockChrome: %v", err)
	}
	if err := svc.Terminal.ServiceShutdown(); err != nil {
		t.Fatalf("ServiceShutdown: %v", err)
	}

	svc2 := NewServices(eng)
	t.Cleanup(func() { _ = svc2.Terminal.ServiceShutdown() })
	got := svc2.Terminal.ListTerminalDockChrome()
	if len(got) != 1 {
		t.Fatalf("ListTerminalDockChrome() = %d, want 1", len(got))
	}
	if got[0].WorkspaceID != "ws-1" || !got[0].Open || !got[0].Maximized || !got[0].EditorHidden || !got[0].Beside || got[0].BesidePct != 40 {
		t.Errorf("ListTerminalDockChrome() = %+v, want ws-1 open+maximized+editorHidden+beside@40", got[0])
	}
}

func TestSetTerminalDockChromeClosedDropsMaximize(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	t.Cleanup(func() { _ = svc.Terminal.ServiceShutdown() })
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: "p"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if err := svc.Terminal.SetTerminalDockChrome("ws-1", TerminalDockChrome{
		Open: false, Maximized: true, EditorHidden: true,
	}); err != nil {
		t.Fatalf("SetTerminalDockChrome: %v", err)
	}
	got := svc.Terminal.ListTerminalDockChrome()
	if len(got) != 0 {
		t.Fatalf("ListTerminalDockChrome() after closed = %+v, want empty", got)
	}
}

func TestSetTerminalDockChromeClosedKeepsBeside(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	t.Cleanup(func() { _ = svc.Terminal.ServiceShutdown() })
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: "p"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if err := svc.Terminal.SetTerminalDockChrome("ws-1", TerminalDockChrome{
		Open: false, Beside: true, BesidePct: 40,
	}); err != nil {
		t.Fatalf("SetTerminalDockChrome: %v", err)
	}
	got := svc.Terminal.ListTerminalDockChrome()
	if len(got) != 1 || got[0].Open || !got[0].Beside || got[0].BesidePct != 40 {
		t.Fatalf("ListTerminalDockChrome() = %+v, want closed+beside@40", got)
	}
}

func TestBesidePctSurvivesClosedBelowRestart(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: "p"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if err := svc.Terminal.SetTerminalDockChrome("ws-1", TerminalDockChrome{
		Open: true, Beside: true, BesidePct: 40,
	}); err != nil {
		t.Fatalf("SetTerminalDockChrome(beside): %v", err)
	}
	if err := svc.Terminal.SetTerminalDockChrome("ws-1", TerminalDockChrome{
		Open: false, BesidePct: 40,
	}); err != nil {
		t.Fatalf("SetTerminalDockChrome(closed): %v", err)
	}
	got := svc.Terminal.ListTerminalDockChrome()
	if len(got) != 1 || got[0].Open || got[0].Beside || got[0].BesidePct != 40 {
		t.Fatalf("ListTerminalDockChrome() = %+v, want closed below @40", got)
	}
	svc.Terminal.flushPersist()
	snap := readTerminalSnapshot("ws-1")
	if snap == nil || snap.BesidePct != 40 || snap.Beside || snap.Open {
		t.Fatalf("readTerminalSnapshot() = %+v, want closed below @40", snap)
	}
	if err := svc.Terminal.ServiceShutdown(); err != nil {
		t.Fatalf("ServiceShutdown: %v", err)
	}

	svc2 := NewServices(eng)
	t.Cleanup(func() { _ = svc2.Terminal.ServiceShutdown() })
	got = svc2.Terminal.ListTerminalDockChrome()
	if len(got) != 1 {
		t.Fatalf("ListTerminalDockChrome() after restart = %d rows, want 1", len(got))
	}
	if got[0].WorkspaceID != "ws-1" || got[0].Open || got[0].Beside || got[0].BesidePct != 40 {
		t.Errorf("ListTerminalDockChrome() after restart = %+v, want ws-1 closed below @40", got[0])
	}
}

func TestRestoreEmptyWhenNothingSaved(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: "p"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	got, err := svc.Terminal.RestoreWorkspaceTerminals("ws-1", 80, 24)
	if err != nil {
		t.Fatalf("Restore: %v", err)
	}
	if len(got) != 0 {
		t.Fatalf("got %d, want empty", len(got))
	}
}

func TestShutdownDoesNotEraseSnapshotAfterCloseAll(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	repo := t.TempDir()
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: projectID}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if _, err := svc.Terminal.OpenTerminal("ws-1", 80, 24); err != nil {
		t.Fatalf("OpenTerminal: %v", err)
	}
	if err := svc.Terminal.ServiceShutdown(); err != nil {
		t.Fatalf("ServiceShutdown: %v", err)
	}
	time.Sleep(1500 * time.Millisecond)
	snap := readTerminalSnapshot("ws-1")
	if snap == nil || len(snap.Tabs) != 1 {
		t.Fatalf("readTerminalSnapshot after shutdown+wait = %+v, want 1 tab", snap)
	}
}

func TestPersistDelayCapsTrailingDebounce(t *testing.T) {
	now := time.Date(2026, 8, 17, 12, 0, 0, 0, time.UTC)
	debounce := time.Second
	maxAge := 5 * time.Second
	if got := persistDelay(now, time.Time{}, debounce, maxAge); got != debounce {
		t.Errorf("persistDelay(first=zero) = %v, want %v", got, debounce)
	}
	if got := persistDelay(now, now.Add(-4*time.Second), debounce, maxAge); got != time.Second {
		t.Errorf("persistDelay(age=4s) = %v, want 1s", got)
	}
	if got := persistDelay(now, now.Add(-4500*time.Millisecond), debounce, maxAge); got != 500*time.Millisecond {
		t.Errorf("persistDelay(age=4.5s) = %v, want 500ms", got)
	}
	if got := persistDelay(now, now.Add(-5*time.Second), debounce, maxAge); got != 0 {
		t.Errorf("persistDelay(age=5s) = %v, want 0", got)
	}
	if got := persistDelay(now, now.Add(-6*time.Second), debounce, maxAge); got != 0 {
		t.Errorf("persistDelay(age=6s) = %v, want 0", got)
	}
}

func TestListTerminalDockChromeCachesDiskRead(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	t.Cleanup(func() { _ = svc.Terminal.ServiceShutdown() })
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: "p"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if err := writeTerminalSnapshot("ws-1", persistedTerminals{
		Open: true, Maximized: true,
		Tabs: []persistedTab{{Title: "zsh", Cwd: t.TempDir(), Replay: make([]byte, 1024)}},
	}); err != nil {
		t.Fatalf("writeTerminalSnapshot: %v", err)
	}
	got := svc.Terminal.ListTerminalDockChrome()
	if len(got) != 1 || !got[0].Open || !got[0].Maximized {
		t.Fatalf("ListTerminalDockChrome() = %+v, want open+maximized", got)
	}
	if err := removeTerminalSnapshot("ws-1"); err != nil {
		t.Fatalf("removeTerminalSnapshot: %v", err)
	}
	got = svc.Terminal.ListTerminalDockChrome()
	if len(got) != 1 || !got[0].Open || !got[0].Maximized {
		t.Fatalf("ListTerminalDockChrome() after file removed = %+v, want cached chrome", got)
	}
}

func TestTerminalSnapshotFileMode(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	if err := writeTerminalSnapshot("ws-1", persistedTerminals{
		Tabs: []persistedTab{{Title: "zsh", Cwd: t.TempDir()}},
	}); err != nil {
		t.Fatalf("writeTerminalSnapshot: %v", err)
	}
	fi, err := os.Stat(terminalSnapshotPath(dataDir, "ws-1"))
	if err != nil {
		t.Fatalf("Stat: %v", err)
	}
	if perm := fi.Mode().Perm(); perm != terminalPersistMode {
		t.Errorf("terminals.json.gz mode = %o, want %o", perm, terminalPersistMode)
	}
}

func TestRestoreSerializesOverlappingCalls(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	repo := t.TempDir()
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	t.Cleanup(func() { _ = svc.Terminal.ServiceShutdown() })
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: projectID}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if err := writeTerminalSnapshot("ws-1", persistedTerminals{
		Tabs: []persistedTab{{Title: "zsh", Cwd: repo, Replay: []byte("hi\r\n")}},
	}); err != nil {
		t.Fatalf("writeTerminalSnapshot: %v", err)
	}

	var wg sync.WaitGroup
	errCh := make(chan error, 8)
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, err := svc.Terminal.RestoreWorkspaceTerminals("ws-1", 80, 24)
			if err != nil {
				errCh <- err
			}
		}()
	}
	wg.Wait()
	close(errCh)
	for err := range errCh {
		t.Errorf("RestoreWorkspaceTerminals: %v", err)
	}
	if n := len(svc.Terminal.ListTerminals("ws-1")); n != 1 {
		t.Fatalf("ListTerminals after overlapping Restore = %d, want 1", n)
	}
}
