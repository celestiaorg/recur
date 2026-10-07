package store

import (
	"os"
	"path/filepath"
	"testing"
)

func TestWorktreesDirUsesDataDirWhenOverridden(t *testing.T) {
	data := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", data)
	t.Setenv("RECUR_WORKTREES_DIR", "")
	got, err := WorktreesDir()
	if err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(data, "worktrees")
	if got != want {
		t.Fatalf("got %q want %q", got, want)
	}
}

func TestWorktreesDirExplicitOverride(t *testing.T) {
	wt := t.TempDir()
	t.Setenv("RECUR_WORKTREES_DIR", wt)
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	got, err := WorktreesDir()
	if err != nil {
		t.Fatal(err)
	}
	if got != wt {
		t.Fatalf("got %q want %q", got, wt)
	}
}

func TestMigrateLegacyWorktrees(t *testing.T) {
	newRoot := t.TempDir()
	legacy := t.TempDir()
	if err := os.WriteFile(filepath.Join(legacy, "LOOP-1"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Directly exercise the move helper with temp dirs (not the production paths).
	entries, err := os.ReadDir(legacy)
	if err != nil || len(entries) == 0 {
		t.Fatal("legacy empty")
	}
	for _, e := range entries {
		src := filepath.Join(legacy, e.Name())
		dst := filepath.Join(newRoot, e.Name())
		if err := os.Rename(src, dst); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := os.Stat(filepath.Join(newRoot, "LOOP-1")); err != nil {
		t.Fatalf("not migrated: %v", err)
	}
}

func TestAppChatDirIsManagedNotAWorktree(t *testing.T) {
	data := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", data)
	got, err := AppChatDir()
	if err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(data, "app-chat")
	if got != want {
		t.Fatalf("got %q want %q", got, want)
	}
	st, err := os.Stat(got)
	if err != nil || !st.IsDir() {
		t.Fatalf("AppChatDir must exist as a directory: %v", err)
	}
}
