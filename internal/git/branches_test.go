package git

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// initRepoWithRemote creates a bare upstream, a clone with origin, and returns
// (clonePath, barePath). The clone has one commit on main tracked from origin.
func initRepoWithRemote(t *testing.T) (clone, bare string) {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	bare = filepath.Join(t.TempDir(), "upstream.git")
	run := func(dir string, args ...string) {
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
	seed := t.TempDir()
	run(seed, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(seed, "README.md"), []byte("hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(seed, "add", ".")
	run(seed, "commit", "-m", "initial")
	run(seed, "clone", "--bare", seed, bare)

	clone = filepath.Join(t.TempDir(), "work")
	run(t.TempDir(), "clone", bare, clone)
	return clone, bare
}

func TestListRemoteBranchesAndParse(t *testing.T) {
	clone, _ := initRepoWithRemote(t)
	m := NewManager(clone, t.TempDir())
	ctx := context.Background()

	remotes, err := m.Remotes(ctx)
	if err != nil {
		t.Fatalf("Remotes: %v", err)
	}
	if len(remotes) != 1 || remotes[0] != "origin" {
		t.Fatalf("Remotes = %v, want [origin]", remotes)
	}

	remoteBranches, err := m.ListRemoteBranches(ctx)
	if err != nil {
		t.Fatalf("ListRemoteBranches: %v", err)
	}
	seen := map[string]bool{}
	for _, b := range remoteBranches {
		seen[b] = true
		if strings.HasSuffix(b, "/HEAD") {
			t.Errorf("ListRemoteBranches included HEAD ref %q", b)
		}
	}
	if !seen["origin/main"] {
		t.Fatalf("expected origin/main, got %v", remoteBranches)
	}

	remote, branch, ok := m.ParseRemoteTracking(ctx, "origin/main")
	if !ok || remote != "origin" || branch != "main" {
		t.Fatalf("ParseRemoteTracking(origin/main) = %q %q %v", remote, branch, ok)
	}
	if got := m.ShortBranchName(ctx, "origin/main"); got != "main" {
		t.Fatalf("ShortBranchName(origin/main) = %q, want main", got)
	}
	if got := m.ShortBranchName(ctx, "main"); got != "main" {
		t.Fatalf("ShortBranchName(main) = %q, want main", got)
	}
	if _, _, ok := m.ParseRemoteTracking(ctx, "main"); ok {
		t.Fatal("ParseRemoteTracking(main) should be false")
	}
}

func TestBaseBranchExists(t *testing.T) {
	clone, _ := initRepoWithRemote(t)
	m := NewManager(clone, t.TempDir())
	ctx := context.Background()

	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = clone
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("tag", "v1.0")
	shaOut, err := exec.Command("git", "-C", clone, "rev-parse", "HEAD").Output()
	if err != nil {
		t.Fatalf("rev-parse HEAD: %v", err)
	}
	sha := strings.TrimSpace(string(shaOut))

	cases := []struct {
		name string
		ref  string
		want bool
	}{
		{"local main", "main", true},
		{"remote tracking", "origin/main", true},
		{"HEAD", "HEAD", false},
		{"origin HEAD", "origin/HEAD", false},
		{"tag", "v1.0", false},
		{"sha", sha, false},
		{"missing", "does-not-exist", false},
		{"empty", "", false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := m.BaseBranchExists(ctx, tc.ref); got != tc.want {
				t.Fatalf("BaseBranchExists(%q) = %v, want %v", tc.ref, got, tc.want)
			}
		})
	}
}

func TestAddFetchesRemoteBase(t *testing.T) {
	clone, bare := initRepoWithRemote(t)
	// Advance the bare upstream past what the clone has fetched.
	pusher := filepath.Join(t.TempDir(), "pusher")
	cmd := exec.Command("git", "clone", bare, pusher)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("clone pusher: %v\n%s", err, out)
	}
	if err := os.WriteFile(filepath.Join(pusher, "newer.txt"), []byte("from upstream\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn := func(dir string, args ...string) {
		t.Helper()
		c := exec.Command("git", args...)
		c.Dir = dir
		c.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := c.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	runIn(pusher, "add", ".")
	runIn(pusher, "commit", "-m", "upstream advance")
	runIn(pusher, "push", "origin", "main")

	m := NewManager(clone, t.TempDir())
	ctx := context.Background()
	// Stale origin/main should not yet have newer.txt.
	if _, err := m.run(ctx, "cat-file", "-e", "origin/main:newer.txt"); err == nil {
		t.Fatal("origin/main unexpectedly already has newer.txt before Add")
	}

	wt, created, err := m.Add(ctx, "ws-fetch", "ws-fetch", "origin/main")
	if err != nil {
		t.Fatalf("Add off origin/main: %v", err)
	}
	if !created {
		t.Fatal("expected new worktree")
	}
	body, err := os.ReadFile(filepath.Join(wt.Path, "newer.txt"))
	if err != nil {
		t.Fatalf("worktree missing newer.txt after fetch-on-Add: %v", err)
	}
	if string(body) != "from upstream\n" {
		t.Fatalf("newer.txt = %q", body)
	}
}

func TestParseRemoteTrackingRefNoGit(t *testing.T) {
	remotes := []string{"origin", "upstream"}
	remote, branch, ok := ParseRemoteTrackingRef(remotes, "upstream/main")
	if !ok || remote != "upstream" || branch != "main" {
		t.Fatalf("got %q %q %v", remote, branch, ok)
	}
	// Longest prefix wins over a shorter remote name that is a prefix.
	remotes = []string{"origin", "origin-fork"}
	remote, branch, ok = ParseRemoteTrackingRef(remotes, "origin-fork/feature")
	if !ok || remote != "origin-fork" || branch != "feature" {
		t.Fatalf("longest prefix: got %q %q %v", remote, branch, ok)
	}
	if _, _, ok := ParseRemoteTrackingRef(remotes, "main"); ok {
		t.Fatal("local ref must not parse as remote")
	}
}

func TestMergeCreatesLocalFromSelectedRemote(t *testing.T) {
	// Two remotes both have main; no local main. Merge with base upstream/main
	// must create local main from upstream (not DWIM to origin).
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	run := func(dir string, args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	originBare := filepath.Join(t.TempDir(), "origin.git")
	upstreamBare := filepath.Join(t.TempDir(), "upstream.git")
	seed := t.TempDir()
	run(seed, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(seed, "README.md"), []byte("origin tip\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(seed, "add", ".")
	run(seed, "commit", "-m", "origin initial")
	run(seed, "clone", "--bare", seed, originBare)

	upSeed := t.TempDir()
	run(upSeed, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(upSeed, "README.md"), []byte("upstream tip\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(upSeed, "add", ".")
	run(upSeed, "commit", "-m", "upstream initial")
	run(upSeed, "clone", "--bare", upSeed, upstreamBare)

	clone := filepath.Join(t.TempDir(), "work")
	run(t.TempDir(), "clone", originBare, clone)
	run(clone, "remote", "add", "upstream", upstreamBare)
	run(clone, "fetch", "upstream")
	// Detach and delete local main so checkout must create it.
	run(clone, "checkout", "--detach")
	run(clone, "branch", "-D", "main")

	m := NewManager(clone, t.TempDir())
	ctx := context.Background()
	wt, _, err := m.Add(ctx, "ws-up", "ws-up", "upstream/main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	write(t, wt.Path, "feat.txt", "from loop\n")
	if _, err := m.Commit(ctx, wt.Path, "feat", "Coder", "coder@recur.local"); err != nil {
		t.Fatalf("Commit: %v", err)
	}

	if _, err := m.Merge(ctx, wt.Branch, "upstream/main", "merge up", "merge"); err != nil {
		t.Fatalf("Merge onto upstream/main: %v", err)
	}
	body, err := os.ReadFile(filepath.Join(clone, "README.md"))
	if err != nil {
		t.Fatal(err)
	}
	if string(body) != "upstream tip\n" {
		t.Fatalf("main should come from upstream, README=%q", body)
	}
}

func TestSuggestBaseBranchPrefersOriginMain(t *testing.T) {
	clone, _ := initRepoWithRemote(t)
	m := NewManager(clone, t.TempDir())
	ctx := context.Background()
	if got := m.SuggestBaseBranch(ctx); got != "origin/main" {
		t.Fatalf("SuggestBaseBranch = %q, want origin/main", got)
	}
}

func TestSuggestBaseBranchFallsBackToLocalMain(t *testing.T) {
	repo := initRepo(t)
	m := NewManager(repo, t.TempDir())
	ctx := context.Background()
	if got := m.SuggestBaseBranch(ctx); got != "main" {
		t.Fatalf("SuggestBaseBranch (no remote) = %q, want main", got)
	}
}

func TestAddEmptyRepoRemoteBaseFails(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	dir := t.TempDir()
	cmd := exec.Command("git", "init", "-b", "main")
	cmd.Dir = dir
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git init: %v\n%s", err, out)
	}
	// Fake a remote name so ParseRemoteTracking succeeds without a real URL.
	addRemote := exec.Command("git", "remote", "add", "origin", "https://example.invalid/repo.git")
	addRemote.Dir = dir
	if out, err := addRemote.CombinedOutput(); err != nil {
		t.Fatalf("remote add: %v\n%s", err, out)
	}

	m := NewManager(dir, t.TempDir())
	ctx := context.Background()
	_, _, err := m.Add(ctx, "ws-empty", "ws-empty", "origin/main")
	if err == nil {
		t.Fatal("Add with remote base on empty repo should fail")
	}
}
