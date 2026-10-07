package cursor

import (
	"os"
	"path/filepath"
	"testing"
)

func TestCursorProjectSlug(t *testing.T) {
	got := CursorProjectSlug("/Users/josh/.recur/worktrees/velvety-croissant")
	want := "Users-josh-recur-worktrees-velvety-croissant"
	if got != want {
		t.Fatalf("slug=%q, want %q", got, want)
	}
	if CursorProjectSlug("  ") != "" {
		t.Fatal("empty workspace should yield empty slug")
	}
}

func TestEnsureCursorProjectDirsCreatesTerminals(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	// UserHomeDir reads HOME on Unix.
	ws := "/Users/josh/.recur/worktrees/velvety-croissant"
	if err := EnsureCursorProjectDirs(ws); err != nil {
		t.Fatal(err)
	}
	term := filepath.Join(home, ".cursor", "projects", CursorProjectSlug(ws), "terminals")
	fi, err := os.Stat(term)
	if err != nil || !fi.IsDir() {
		t.Fatalf("terminals dir missing: %v", err)
	}
}

func TestRemoveCursorProjectDirsDeletesSlugTree(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	ws := "/Users/josh/.recur/worktrees/velvety-croissant"
	if err := EnsureCursorProjectDirs(ws); err != nil {
		t.Fatal(err)
	}
	// Simulate sdk-agent-store sitting alongside terminals.
	store := filepath.Join(home, ".cursor", "projects", CursorProjectSlug(ws), "sdk-agent-store")
	if err := os.MkdirAll(store, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := RemoveCursorProjectDirs(ws); err != nil {
		t.Fatal(err)
	}
	root := filepath.Join(home, ".cursor", "projects", CursorProjectSlug(ws))
	if _, err := os.Stat(root); !os.IsNotExist(err) {
		t.Fatalf("project slug dir should be gone, stat err=%v", err)
	}
	// Idempotent.
	if err := RemoveCursorProjectDirs(ws); err != nil {
		t.Fatalf("second remove: %v", err)
	}
}
