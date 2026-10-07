package git

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func gitConfigIdentity(t *testing.T, dir string) {
	t.Helper()
	for _, args := range [][]string{
		{"config", "user.name", "test"},
		{"config", "user.email", "test@example.com"},
	} {
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
}

func TestUpstreamPushAndFetch(t *testing.T) {
	clone, bare := initRepoWithRemote(t)
	gitConfigIdentity(t, clone)
	m := NewManager(clone, t.TempDir())
	ctx := context.Background()

	up, ok := m.Upstream(ctx, clone)
	if !ok || up.Remote != "origin" || up.Branch != "main" || up.Ref != "origin/main" {
		t.Fatalf("Upstream = %+v ok=%v, want origin/main", up, ok)
	}

	if err := os.WriteFile(filepath.Join(clone, "local.txt"), []byte("mine\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, clone, "add", "local.txt")
	runIn(t, clone, "commit", "-m", "local commit")

	ahead, err := m.CommitsAhead(ctx, clone, up.Ref)
	if err != nil {
		t.Fatalf("CommitsAhead: %v", err)
	}
	if ahead != 1 {
		t.Fatalf("CommitsAhead = %d, want 1", ahead)
	}

	if err := m.PushUpstream(ctx, clone); err != nil {
		t.Fatalf("PushUpstream: %v", err)
	}
	ahead, err = m.CommitsAhead(ctx, clone, up.Ref)
	if err != nil {
		t.Fatalf("CommitsAhead after push: %v", err)
	}
	if ahead != 0 {
		t.Fatalf("CommitsAhead after push = %d, want 0", ahead)
	}
	if out := showFile(t, bare, "HEAD:local.txt"); out != "mine" {
		t.Fatalf("bare HEAD:local.txt = %q, want mine", out)
	}

	if err := os.WriteFile(filepath.Join(clone, "pushed.txt"), []byte("branch\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, clone, "add", "pushed.txt")
	runIn(t, clone, "commit", "-m", "via PushBranch")
	if err := m.PushBranch(ctx, "origin", "main"); err != nil {
		t.Fatalf("PushBranch(origin, main): %v", err)
	}
	if out := showFile(t, bare, "HEAD:pushed.txt"); out != "branch" {
		t.Fatalf("bare HEAD:pushed.txt after PushBranch = %q, want branch", out)
	}

	// Advance the bare remote via a second clone; local tracking stays stale
	// until FetchUpstream.
	pusher := filepath.Join(t.TempDir(), "pusher")
	runIn(t, t.TempDir(), "clone", bare, pusher)
	gitConfigIdentity(t, pusher)
	if err := os.WriteFile(filepath.Join(pusher, "remote.txt"), []byte("theirs\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, pusher, "add", "remote.txt")
	runIn(t, pusher, "commit", "-m", "remote advance")
	runIn(t, pusher, "push", "origin", "main")

	behind, err := m.CommitsBehind(ctx, clone, "origin/main")
	if err != nil {
		t.Fatalf("CommitsBehind before fetch: %v", err)
	}
	if behind != 0 {
		t.Fatalf("CommitsBehind before fetch = %d, want 0 (stale tracking tip)", behind)
	}
	if _, err := m.FetchUpstream(ctx, clone); err != nil {
		t.Fatalf("FetchUpstream: %v", err)
	}
	behind, err = m.CommitsBehind(ctx, clone, "origin/main")
	if err != nil {
		t.Fatalf("CommitsBehind after fetch: %v", err)
	}
	if behind != 1 {
		t.Fatalf("CommitsBehind after fetch = %d, want 1", behind)
	}
}

func TestFetchCompareRefSeesRemoteAdvanceOnLocalBaseName(t *testing.T) {
	clone, bare := initRepoWithRemote(t)
	gitConfigIdentity(t, clone)
	m := NewManager(clone, t.TempDir())
	ctx := context.Background()

	if got := m.CompareRef(ctx, "main"); got != "origin/main" {
		t.Fatalf("CompareRef(main) = %q, want origin/main", got)
	}

	pusher := filepath.Join(t.TempDir(), "pusher")
	runIn(t, t.TempDir(), "clone", bare, pusher)
	gitConfigIdentity(t, pusher)
	if err := os.WriteFile(filepath.Join(pusher, "base-moved.txt"), []byte("ahead\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, pusher, "add", "base-moved.txt")
	runIn(t, pusher, "commit", "-m", "advance merge base")
	runIn(t, pusher, "push", "origin", "main")

	behindLocal, err := m.CommitsBehind(ctx, clone, "main")
	if err != nil {
		t.Fatalf("CommitsBehind(main): %v", err)
	}
	if behindLocal != 0 {
		t.Fatalf("CommitsBehind(main) = %d, want 0 (local main is stale)", behindLocal)
	}
	behindStale, err := m.CommitsBehind(ctx, clone, "origin/main")
	if err != nil {
		t.Fatalf("CommitsBehind(origin/main) before fetch: %v", err)
	}
	if behindStale != 0 {
		t.Fatalf("CommitsBehind(origin/main) before fetch = %d, want 0", behindStale)
	}

	ref := m.FetchCompareRef(ctx, "main")
	if ref != "origin/main" {
		t.Fatalf("FetchCompareRef(main) = %q, want origin/main", ref)
	}
	behind, err := m.CommitsBehind(ctx, clone, ref)
	if err != nil {
		t.Fatalf("CommitsBehind after FetchCompareRef: %v", err)
	}
	if behind != 1 {
		t.Fatalf("CommitsBehind after FetchCompareRef = %d, want 1", behind)
	}

	ref, n, err := m.BehindBase(ctx, clone, "main", true)
	if err != nil {
		t.Fatalf("BehindBase: %v", err)
	}
	if ref != "origin/main" || n != 1 {
		t.Fatalf("BehindBase(main, fetch) = %s %d, want origin/main 1", ref, n)
	}
}

func TestFetchCompareRefLocalOnlyRepoStaysOnLocalName(t *testing.T) {
	dir := t.TempDir()
	runIn(t, dir, "init", "-b", "main")
	gitConfigIdentity(t, dir)
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, dir, "add", ".")
	runIn(t, dir, "commit", "-m", "initial")
	m := NewManager(dir, t.TempDir())
	ctx := context.Background()
	if got := m.FetchCompareRef(ctx, "main"); got != "main" {
		t.Fatalf("FetchCompareRef(main) on local-only repo = %q, want main", got)
	}
}

func TestFetchCompareRefUsesNamedRemoteBase(t *testing.T) {
	clone, bare := initRepoWithRemote(t)
	gitConfigIdentity(t, clone)
	runIn(t, clone, "checkout", "-b", "develop")
	runIn(t, clone, "push", "-u", "origin", "develop")
	runIn(t, clone, "checkout", "main")

	pusher := filepath.Join(t.TempDir(), "pusher")
	runIn(t, t.TempDir(), "clone", bare, pusher)
	gitConfigIdentity(t, pusher)
	runIn(t, pusher, "checkout", "develop")
	if err := os.WriteFile(filepath.Join(pusher, "develop-ahead.txt"), []byte("yes\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, pusher, "add", "develop-ahead.txt")
	runIn(t, pusher, "commit", "-m", "advance develop")
	runIn(t, pusher, "push", "origin", "develop")

	m := NewManager(clone, t.TempDir())
	ctx := context.Background()
	if got := m.FetchCompareRef(ctx, "origin/develop"); got != "origin/develop" {
		t.Fatalf("FetchCompareRef(origin/develop) = %q, want origin/develop", got)
	}
	behind, err := m.CommitsBehind(ctx, clone, "origin/develop")
	if err != nil {
		t.Fatalf("CommitsBehind(origin/develop): %v", err)
	}
	if behind != 1 {
		t.Fatalf("CommitsBehind(origin/develop) = %d, want 1", behind)
	}
	ref, n, err := m.BehindBase(ctx, clone, "develop", true)
	if err != nil {
		t.Fatalf("BehindBase(develop): %v", err)
	}
	if ref != "origin/develop" || n != 1 {
		t.Fatalf("BehindBase(develop, fetch) = %s %d, want origin/develop 1", ref, n)
	}
}

func TestPushUpstreamPushesHEADWhenLocalBranchNameDiffers(t *testing.T) {
	clone, bare := initRepoWithRemote(t)
	gitConfigIdentity(t, clone)
	runIn(t, clone, "checkout", "-b", "work")
	runIn(t, clone, "branch", "--set-upstream-to=origin/main")
	if err := os.WriteFile(filepath.Join(clone, "from-work.txt"), []byte("yes\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, clone, "add", "from-work.txt")
	runIn(t, clone, "commit", "-m", "on work")

	m := NewManager(clone, t.TempDir())
	if err := m.PushUpstream(context.Background(), clone); err != nil {
		t.Fatalf("PushUpstream: %v", err)
	}
	if out := showFile(t, bare, "HEAD:from-work.txt"); out != "yes" {
		t.Fatalf("bare HEAD:from-work.txt = %q, want yes (HEAD should have been pushed, not local main)", out)
	}
	mainBlob, err := exec.Command("git", "-C", clone, "show", "main:from-work.txt").CombinedOutput()
	if err == nil {
		t.Fatalf("local main should not contain from-work.txt, got %q", mainBlob)
	}
}

func TestUpstreamMissing(t *testing.T) {
	dir := t.TempDir()
	runIn(t, dir, "init", "-b", "main")
	gitConfigIdentity(t, dir)
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, dir, "add", ".")
	runIn(t, dir, "commit", "-m", "initial")
	m := NewManager(dir, t.TempDir())
	if _, ok := m.Upstream(context.Background(), dir); ok {
		t.Fatal("Upstream on a repo with no remote should be missing")
	}
	if err := m.PushUpstream(context.Background(), dir); err == nil {
		t.Fatal("PushUpstream without tracking should error")
	}
}

func runIn(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
		"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
	)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v (in %s): %v\n%s", args, dir, err, out)
	}
}

func showFile(t *testing.T, repo, spec string) string {
	t.Helper()
	out, err := exec.Command("git", "-C", repo, "show", spec).CombinedOutput()
	if err != nil {
		t.Fatalf("git show %s: %v\n%s", spec, err, out)
	}
	return strings.TrimSpace(string(out))
}
