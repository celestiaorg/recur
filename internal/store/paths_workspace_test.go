package store

import (
	"os"
	"path/filepath"
	"testing"
)

func TestWorkspaceDataDirs(t *testing.T) {
	root := t.TempDir()
	t.Setenv(dataDirEnv, root)

	ws, err := WorkspaceDir(root, "W1")
	if err != nil {
		t.Fatal(err)
	}
	if ws != filepath.Join(root, "workspaces", "W1") {
		t.Fatalf("WorkspaceDir=%q", ws)
	}
	art, err := ArtifactsDir(root, "W1")
	if err != nil {
		t.Fatal(err)
	}
	if art != filepath.Join(root, "workspaces", "W1", "artifacts") {
		t.Fatalf("ArtifactsDir=%q", art)
	}
	att, err := AttachmentsDir(root, "W1")
	if err != nil {
		t.Fatal(err)
	}
	if att != filepath.Join(root, "workspaces", "W1", "attachments") {
		t.Fatalf("AttachmentsDir=%q", att)
	}
	shot, err := ScreenshotsDir(root, "W1")
	if err != nil {
		t.Fatal(err)
	}
	if shot != filepath.Join(root, "workspaces", "W1", "screenshots") {
		t.Fatalf("ScreenshotsDir=%q", shot)
	}
	for _, d := range []string{art, att, shot} {
		if st, err := os.Stat(d); err != nil || !st.IsDir() {
			t.Fatalf("expected dir %s: %v", d, err)
		}
	}
}

func TestRecurSkillsDir(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv(dataDirEnv, "")
	dir, err := RecurSkillsDir()
	if err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(home, ".recur", "recur-skills")
	if dir != want {
		t.Fatalf("got %q want %q", dir, want)
	}
}

func TestRecurSkillsDirIsolatesDataWithoutChangingHome(t *testing.T) {
	home, data := t.TempDir(), t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv(dataDirEnv, data)
	dir, err := RecurSkillsDir()
	if err != nil {
		t.Fatal(err)
	}
	if dir != filepath.Join(data, "recur-skills") {
		t.Fatalf("skills = %q, want the isolated data profile", dir)
	}
	if os.Getenv("HOME") != home {
		t.Fatal("data isolation changed desktop preferences")
	}
	if _, err := os.Stat(filepath.Join(home, ".recur", "recur-skills")); !os.IsNotExist(err) {
		t.Fatal("isolated profile touched the normal skills directory")
	}
}
