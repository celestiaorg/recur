package git

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestFetchPRHeadRejectsInvalidNumber(t *testing.T) {
	t.Parallel()
	m := NewManager(t.TempDir(), t.TempDir())
	if _, err := m.FetchPRHead(context.Background(), 0); err == nil {
		t.Fatal("FetchPRHead(0) error = nil, want invalid number")
	}
}

func TestFetchPRHeadForceUpdatesAfterRewind(t *testing.T) {
	clone, _ := initRepoWithRemote(t)
	gitConfigIdentity(t, clone)
	runIn(t, clone, "checkout", "-b", "feat/pr")
	if err := os.WriteFile(filepath.Join(clone, "one.txt"), []byte("one\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, clone, "add", ".")
	runIn(t, clone, "commit", "-m", "one")
	sha1 := strings.TrimSpace(gitOut(t, clone, "rev-parse", "HEAD"))
	runIn(t, clone, "push", "origin", sha1+":refs/pull/7/head")

	m := NewManager(clone, t.TempDir())
	ctx := context.Background()
	ref, err := m.FetchPRHead(ctx, 7)
	if err != nil {
		t.Fatalf("FetchPRHead first: %v", err)
	}
	if ref != "refs/recur/pr/7" {
		t.Fatalf("FetchPRHead first ref = %q, want refs/recur/pr/7", ref)
	}

	runIn(t, clone, "checkout", "--orphan", "tmp-pr")
	runIn(t, clone, "reset")
	if err := os.WriteFile(filepath.Join(clone, "two.txt"), []byte("two\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, clone, "add", ".")
	runIn(t, clone, "commit", "-m", "two")
	sha2 := strings.TrimSpace(gitOut(t, clone, "rev-parse", "HEAD"))
	runIn(t, clone, "push", "--force", "origin", sha2+":refs/pull/7/head")

	ref, err = m.FetchPRHead(ctx, 7)
	if err != nil {
		t.Fatalf("FetchPRHead after force-push: %v", err)
	}
	got := strings.TrimSpace(gitOut(t, clone, "rev-parse", ref))
	if got != sha2 {
		t.Fatalf("FetchPRHead after force-push rev-parse(%s) = %q, want %q", ref, got, sha2)
	}
}

func gitOut(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return string(out)
}
