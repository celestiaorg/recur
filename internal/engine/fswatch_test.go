package engine

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

// TestWatchWorkspaceFiresOnDiskChange covers the active-worktree watcher: after
// arming, an on-disk change made OUTSIDE any agent/tool path (a plain file write,
// like an external editor) publishes workspace.fs_changed via the recursive
// FSEvents stream; arming alone publishes nothing.
func TestWatchWorkspaceFiresOnDiskChange(t *testing.T) {
	oldLat, oldPoll := fsEventLatency, fsPollBackstop
	fsEventLatency = 20 * time.Millisecond
	fsPollBackstop = 150 * time.Millisecond
	t.Cleanup(func() { fsEventLatency, fsPollBackstop = oldLat, oldPoll })

	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "watched", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	eng.WatchWorkspace(ws.ID)
	t.Cleanup(func() { eng.WatchWorkspace("") })

	// Let the watcher settle (stream armed, backstop baselined) before changing
	// the disk — arming alone must not fire.
	time.Sleep(3 * fsPollBackstop)
	if sink.has("workspace.fs_changed") {
		t.Fatal("no change yet — arming/baselining must not fire an event")
	}

	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "external.txt"), []byte("hi"), 0o644); err != nil {
		t.Fatalf("write: %v", err)
	}
	waitFor(t, 3*time.Second, "fs_changed after an external write", func() bool {
		return sink.has("workspace.fs_changed")
	})
}

// TestWatchWorkspaceFiresOnNonGitWorkingCopy pins live refresh for plain
// project folders (OpenMainWorkspace on a non-git RepoPath). The fingerprint
// backstop used to treat `git status` failure as "worktree gone" and exit the
// whole watch loop — so after the first poll, agent/editor writes never
// published workspace.fs_changed and the Observatory buffer stayed stale.
func TestWatchWorkspaceFiresOnNonGitWorkingCopy(t *testing.T) {
	oldLat, oldPoll := fsEventLatency, fsPollBackstop
	fsEventLatency = 20 * time.Millisecond
	fsPollBackstop = 150 * time.Millisecond
	t.Cleanup(func() { fsEventLatency, fsPollBackstop = oldLat, oldPoll })

	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "readme.txt"), []byte("v1\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, dir)
	ws, err := eng.OpenMainWorkspace(projID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}
	if ws.WorktreePath != dir {
		t.Fatalf("WorktreePath = %q, want plain dir %q", ws.WorktreePath, dir)
	}

	eng.WatchWorkspace(ws.ID)
	t.Cleanup(func() { eng.WatchWorkspace("") })

	// Survive several fingerprint polls — this is where the old watcher died
	// (git status failure was treated as "worktree gone" and exited the loop).
	time.Sleep(3 * fsPollBackstop)
	if sink.has("workspace.fs_changed") {
		t.Fatal("no change yet — arming/baselining must not fire an event")
	}

	// Modify an EXISTING file (the open-buffer live-refresh case). Creates alone
	// are not enough — an unresolved /var watch path used to deliver creates
	// but drop modifies.
	if err := os.WriteFile(filepath.Join(dir, "readme.txt"), []byte("v2 agent\n"), 0o644); err != nil {
		t.Fatalf("write: %v", err)
	}
	waitFor(t, 3*time.Second, "fs_changed after modify in non-git working copy", func() bool {
		return sink.has("workspace.fs_changed")
	})
}

// skipUnlessFSEvents probes a raw stream and skips when this environment
// cannot deliver file-granularity events (CI sandbox — see
// TestSyncWorkspaceBranchOnCheckout). Existing watcher tests keep a short
// fingerprint backstop so they still pass there; the tests below isolate the
// stream and must not hard-fail without it.
func skipUnlessFSEvents(t *testing.T) {
	t.Helper()
	dir, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	events, stop, err := startFSWatch(dir)
	if err != nil {
		t.Skipf("startFSWatch: %v", err)
	}
	defer stop()
	if err := os.WriteFile(filepath.Join(dir, "probe.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	select {
	case <-events:
	case <-time.After(2 * time.Second):
		t.Skip("fs watch not delivering in this environment")
	}
}

// TestWatchWorkspaceFSEventsBeatsBackstop proves the fast path is the FSEvents
// stream, not the git-status fingerprint. Existing tests shrink the backstop to
// 150ms, so they still pass if Start() fails and only the poll is alive.
func TestWatchWorkspaceFSEventsBeatsBackstop(t *testing.T) {
	skipUnlessFSEvents(t)
	oldLat, oldPoll := fsEventLatency, fsPollBackstop
	fsEventLatency = 20 * time.Millisecond
	fsPollBackstop = 30 * time.Second
	t.Cleanup(func() { fsEventLatency, fsPollBackstop = oldLat, oldPoll })

	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "watched", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	eng.WatchWorkspace(ws.ID)
	t.Cleanup(func() { eng.WatchWorkspace("") })
	time.Sleep(fsEventLatency + 100*time.Millisecond)
	before := len(sink.byName("workspace.fs_changed"))

	start := time.Now()
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "external.txt"), []byte("hi"), 0o644); err != nil {
		t.Fatalf("write: %v", err)
	}
	waitFor(t, 2*time.Second, "fs_changed via FSEvents (backstop is 30s)", func() bool {
		return len(sink.byName("workspace.fs_changed")) > before
	})
	t.Logf("fs_changed after %s", time.Since(start).Round(time.Millisecond))
}

// TestWatchWorkspaceIdleCreateIsNearInstant pins that an isolated create after
// idle is delivered by the stream (not the 30s backstop) at production latency.
// Timing is logged rather than asserted against the 400ms window: under -race
// or a loaded box the end-to-end path is not a tight SLA.
func TestWatchWorkspaceIdleCreateIsNearInstant(t *testing.T) {
	skipUnlessFSEvents(t)
	oldPoll := fsPollBackstop
	fsPollBackstop = 30 * time.Second
	t.Cleanup(func() { fsPollBackstop = oldPoll })
	if fsEventLatency != 400*time.Millisecond {
		t.Fatalf("fsEventLatency = %s, want production 400ms", fsEventLatency)
	}

	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "watched", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	eng.WatchWorkspace(ws.ID)
	t.Cleanup(func() { eng.WatchWorkspace("") })
	// Wait out the NoDefer coalescing window so residual .git writes from
	// CreateWorkspace don't delay the test file's delivery.
	time.Sleep(fsEventLatency + 100*time.Millisecond)
	before := len(sink.byName("workspace.fs_changed"))

	start := time.Now()
	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "external.txt"), []byte("hi"), 0o644); err != nil {
		t.Fatalf("write: %v", err)
	}
	waitFor(t, 2*time.Second, "fs_changed via FSEvents (backstop is 30s)", func() bool {
		return len(sink.byName("workspace.fs_changed")) > before
	})
	t.Logf("idle create fs_changed after %s", time.Since(start).Round(time.Millisecond))
}

// TestFSEventsStreamCreateLatency times a raw stream with Recur's production
// flags. Skips when the kernel does not deliver; logs duration rather than
// pinning it under the 400ms window (that bound flakes under load / -race).
func TestFSEventsStreamCreateLatency(t *testing.T) {
	skipUnlessFSEvents(t)
	dir, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	events, stop, err := startFSWatch(dir)
	if err != nil {
		t.Skipf("startFSWatch: %v", err)
	}
	t.Cleanup(stop)

	start := time.Now()
	if err := os.WriteFile(filepath.Join(dir, "new.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	select {
	case batch := <-events:
		t.Logf("create delivered after %s (%d events)", time.Since(start).Round(time.Millisecond), len(batch))
	case <-time.After(3 * time.Second):
		t.Skip("fs watch not delivering in this environment")
	}
}

// TestFSWatchSeesFileInNewSubdir pins recursion on per-directory backends
// (inotify): a directory created after arming must be watched too, and files
// written into it before its watch is added must still be reported.
func TestFSWatchSeesFileInNewSubdir(t *testing.T) {
	skipUnlessFSEvents(t)
	dir, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	events, stop, err := startFSWatch(dir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(stop)

	sub := filepath.Join(dir, "a", "b")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(sub, "deep.txt")
	if err := os.WriteFile(want, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	deadline := time.After(3 * time.Second)
	for {
		select {
		case batch := <-events:
			for _, ev := range batch {
				if ev.Path == want {
					return
				}
			}
		case <-deadline:
			t.Fatalf("no event for %s", want)
		}
	}
}

// TestFSWatchFollowsRenamedDir pins that a directory moved inside the root
// keeps reporting under its new path, including its own direct children.
func TestFSWatchFollowsRenamedDir(t *testing.T) {
	skipUnlessFSEvents(t)
	dir, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(dir, "src", "old", "deep"), 0o755); err != nil {
		t.Fatal(err)
	}
	events, stop, err := startFSWatch(dir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(stop)

	newDir := filepath.Join(dir, "src", "new")
	if err := os.Rename(filepath.Join(dir, "src", "old"), newDir); err != nil {
		t.Fatal(err)
	}
	time.Sleep(200 * time.Millisecond)
	want := map[string]bool{
		filepath.Join(newDir, "a.txt"):         false,
		filepath.Join(newDir, "deep", "b.txt"): false,
	}
	for p := range want {
		if err := os.WriteFile(p, []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	deadline := time.After(3 * time.Second)
	for missing := len(want); missing > 0; {
		select {
		case batch := <-events:
			for _, ev := range batch {
				if seen, ok := want[ev.Path]; ok && !seen {
					want[ev.Path] = true
					missing--
				}
			}
		case <-deadline:
			t.Fatalf("missing events: %v", want)
		}
	}
}

// TestWorktreeFingerprintSeesUpstreamAdvance: `git fetch` that advances
// origin/main must change the fingerprint even when HEAD and the dirty set
// stay the same — that's how the titlebar learns a tracking branch moved.
func TestWorktreeFingerprintSeesUpstreamAdvance(t *testing.T) {
	origin := initRepo(t)
	dir := t.TempDir()
	gitIn(t, dir, "clone", origin, ".")
	before, ok := worktreeFingerprint(t.Context(), dir, "")
	if !ok {
		t.Fatal("fingerprint failed")
	}

	gitIn(t, origin, "commit", "--allow-empty", "-m", "ahead")
	gitIn(t, dir, "fetch")

	after, ok := worktreeFingerprint(t.Context(), dir, "")
	if !ok {
		t.Fatal("fingerprint after fetch failed")
	}
	if before == after {
		t.Fatal("fingerprint unchanged after git fetch advanced @{upstream}")
	}
}

// TestWorktreeFingerprintSeesBaseAdvance: a feature worktree with no upstream
// must still notice origin/main moving — that's the "Update with main" button.
// @{upstream} is unset, HEAD is unchanged; only the hashed base tip changes.
func TestWorktreeFingerprintSeesBaseAdvance(t *testing.T) {
	origin := initRepo(t)
	dir := t.TempDir()
	gitIn(t, dir, "clone", origin, ".")
	gitIn(t, dir, "checkout", "-b", "feature")
	before, ok := worktreeFingerprint(t.Context(), dir, "main")
	if !ok {
		t.Fatal("fingerprint failed")
	}

	gitIn(t, origin, "commit", "--allow-empty", "-m", "ahead")
	gitIn(t, dir, "fetch")

	after, ok := worktreeFingerprint(t.Context(), dir, "main")
	if !ok {
		t.Fatal("fingerprint after fetch failed")
	}
	if before == after {
		t.Fatal("fingerprint unchanged after origin/main advanced (base tip)")
	}
}

// TestWorktreeFingerprintSeesSlashyBaseAdvance: CompareRef maps release/v1 to
// origin/release/v1. A Contains("/") skip would hash only the local branch and
// miss git fetch advancing the remote tracking ref.
func TestWorktreeFingerprintSeesSlashyBaseAdvance(t *testing.T) {
	origin := initRepo(t)
	gitIn(t, origin, "checkout", "-b", "release/v1")
	dir := t.TempDir()
	gitIn(t, dir, "clone", origin, ".")
	gitIn(t, dir, "checkout", "-b", "feature")
	before, ok := worktreeFingerprint(t.Context(), dir, "release/v1")
	if !ok {
		t.Fatal("fingerprint failed")
	}

	gitIn(t, origin, "commit", "--allow-empty", "-m", "ahead")
	gitIn(t, dir, "fetch")

	after, ok := worktreeFingerprint(t.Context(), dir, "release/v1")
	if !ok {
		t.Fatal("fingerprint after fetch failed")
	}
	if before == after {
		t.Fatal("fingerprint unchanged after origin/release/v1 advanced")
	}
}

// TestWorktreeFingerprintSeesNonOriginBaseAdvance: CompareRef uses the first
// configured remote when origin is absent. Hashing origin/<short> would miss
// git fetch on upstream/main.
func TestWorktreeFingerprintSeesNonOriginBaseAdvance(t *testing.T) {
	origin := initRepo(t)
	dir := t.TempDir()
	gitIn(t, dir, "clone", origin, ".")
	gitIn(t, dir, "remote", "rename", "origin", "upstream")
	gitIn(t, dir, "checkout", "-b", "feature")
	before, ok := worktreeFingerprint(t.Context(), dir, "main")
	if !ok {
		t.Fatal("fingerprint failed")
	}

	gitIn(t, origin, "commit", "--allow-empty", "-m", "ahead")
	gitIn(t, dir, "fetch")

	after, ok := worktreeFingerprint(t.Context(), dir, "main")
	if !ok {
		t.Fatal("fingerprint after fetch failed")
	}
	if before == after {
		t.Fatal("fingerprint unchanged after upstream/main advanced")
	}
}

// TestWorktreeFingerprintSeesFirstCommit: an unborn HEAD (git init, no commits)
// still baselines. The first commit is .git-only and must flip the fingerprint
// so the 7s backstop publishes — status succeeds while rev-parse HEAD exits 1.
func TestWorktreeFingerprintSeesFirstCommit(t *testing.T) {
	dir := t.TempDir()
	gitIn(t, dir, "init", "-b", "main")
	before, ok := worktreeFingerprint(t.Context(), dir, "")
	if !ok {
		t.Fatal("unborn HEAD must still fingerprint, not soft-fail")
	}

	gitIn(t, dir, "commit", "--allow-empty", "-m", "init")
	after, ok := worktreeFingerprint(t.Context(), dir, "")
	if !ok {
		t.Fatal("fingerprint after first commit failed")
	}
	if before == after {
		t.Fatal("fingerprint unchanged after first commit on unborn HEAD")
	}
}

// TestWorktreeFingerprintOKWithoutUpstream: unpublished workspace branches have
// no @{upstream}. Fingerprinting must still succeed (empty optional refs), not
// soft-fail — otherwise the 7s backstop would never fire for those worktrees.
func TestWorktreeFingerprintOKWithoutUpstream(t *testing.T) {
	dir := initRepo(t)
	gitIn(t, dir, "checkout", "-b", "feature")
	if _, ok := worktreeFingerprint(t.Context(), dir, "main"); !ok {
		t.Fatal("no-upstream worktree must still fingerprint, not soft-fail")
	}
}

// TestWorktreeFingerprintSoftFailsOutsideGit: a plain directory must NOT return
// a "nogit" sentinel fingerprint (that used to flap git worktrees through two
// full rescans on a transient `git status` failure). Soft-fail (ok=false) lets
// the watch loop keep its previous baseline; non-git watches skip this helper.
func TestWorktreeFingerprintSoftFailsOutsideGit(t *testing.T) {
	dir := t.TempDir()
	if _, ok := worktreeFingerprint(t.Context(), dir, ""); ok {
		t.Fatal("non-git directory must soft-fail (ok=false), not return a sentinel")
	}
}

// TestFsIsHeadChange pins the fast-path matcher: only the worktree's own
// .git/HEAD (the file a checkout rewrites) is surfaced past the .git filter, so a
// branch switch repaints the crumb/sidebar without waiting for the backstop —
// while .git/index (which `git status` itself writes) and ordinary files aren't.
func TestFsIsHeadChange(t *testing.T) {
	root := "/repo"
	cases := []struct {
		path string
		want bool
	}{
		{"/repo/.git/HEAD", true},
		{"/repo/.git/index", false},
		{"/repo/.git/refs/heads/main", false},
		{"/repo/.git/HEAD.lock", false},
		{"/repo/HEAD", false},
		{"/repo/src/main.go", false},
		{"/repo/sub/.git/HEAD", false}, // a nested repo's HEAD is not ours
	}
	for _, c := range cases {
		if got := fsIsHeadChange(root, c.path); got != c.want {
			t.Errorf("fsIsHeadChange(%q, %q) = %v, want %v", root, c.path, got, c.want)
		}
	}
}

// TestSyncWorkspaceBranchOnCheckout covers the sync the fast .git/HEAD path drives:
// after a checkout in the main working copy, syncWorkspaceBranch updates the cached
// Workspace.Branch and emits workspace.updated carrying the new branch (what paints
// the titlebar crumb and sidebar). The watcher→sync wiring itself relies on
// file-granularity FSEvents, which the CI sandbox doesn't provide, so this drives
// the sync directly rather than through the stream.
func TestSyncWorkspaceBranchOnCheckout(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	repo := initRepo(t)
	projID := addProject(t, st, repo)
	ws, err := eng.OpenMainWorkspace(projID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}
	if ws.Branch != "main" {
		t.Fatalf("main workspace branch = %q, want main", ws.Branch)
	}

	gitIn(t, repo, "checkout", "-b", "feature")

	if got := eng.syncWorkspaceBranch(ws.ID); got != "feature" {
		t.Fatalf("syncWorkspaceBranch = %q, want feature", got)
	}
	if w, _ := st.Workspace(ws.ID); w.Branch != "feature" {
		t.Fatalf("cached branch = %q, want feature", w.Branch)
	}
	var carried bool
	for _, ev := range sink.byName("workspace.updated") {
		if d, ok := ev.Data.(map[string]any); ok && d["branch"] == "feature" {
			carried = true
		}
	}
	if !carried {
		t.Error("no workspace.updated carried branch=feature")
	}

	// Idempotent: a no-op checkout doesn't churn the record or re-emit.
	before := len(sink.byName("workspace.updated"))
	if got := eng.syncWorkspaceBranch(ws.ID); got != "feature" {
		t.Fatalf("second syncWorkspaceBranch = %q, want feature", got)
	}
	if after := len(sink.byName("workspace.updated")); after != before {
		t.Errorf("unchanged branch re-emitted workspace.updated: %d → %d", before, after)
	}
}

// gitIn runs a git command in dir with a stable test identity.
func gitIn(t *testing.T, dir string, args ...string) {
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
