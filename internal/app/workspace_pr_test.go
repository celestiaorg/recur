package app

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

// TestWorkspaceOpenPRAndMerge covers the hand-driven workspace PR flow on the
// unified model: create a workspace (type=solo), edit + commit, open a Local PR
// (owned by WorkspaceID), merge it via the shared PR path, and confirm the change
// lands on the base branch on disk — all with zero agents.
func TestWorkspaceOpenPRAndMerge(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "add a feature", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)

	// Opening a PR with nothing committed is refused.
	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err == nil {
		t.Error("OpenWorkspacePR on an empty workspace should error")
	}

	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err == nil || !strings.Contains(err.Error(), "uncommitted") {
		t.Errorf("OpenWorkspacePR(dirty) err = %v, want uncommitted error", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	prID, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false)
	if err != nil {
		t.Fatalf("OpenWorkspacePR: %v", err)
	}
	pr, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if pr.WorkspaceID != ws.ID || pr.Source != domain.SourceLocal || pr.FileCount == 0 {
		t.Errorf("PR shape wrong: workspaceId=%q source=%q files=%d", pr.WorkspaceID, pr.Source, pr.FileCount)
	}
	if got := prCommentDescription(pr); !strings.Contains(got, "add a feature") || !strings.Contains(got, "## Changes") {
		t.Errorf("OpenWorkspacePR description = %q, want githubPRMarkdown fallback with the workspace goal", got)
	}
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); land.PRID != prID || land.PRNumber != pr.Number {
		t.Errorf("WorkspaceLandState() PRID=%q PRNumber=%d, want %q #%d", land.PRID, land.PRNumber, prID, pr.Number)
	}
	// Reopen is idempotent.
	if again, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err != nil || again != prID {
		t.Errorf("reopen PR = %q (err %v), want same id %q", again, err, prID)
	}

	merged, err := svc.PRs.MergePR(prID, "")
	if err != nil {
		t.Fatalf("MergePR: %v", err)
	}
	if merged.State != "Merged" || merged.Group != "merged" {
		t.Fatalf("PR not merged: state=%q group=%q", merged.State, merged.Group)
	}
	out, gerr := exec.Command("git", "-C", repo, "show", ws.BaseBranch+":feature.txt").CombinedOutput()
	if gerr != nil || strings.TrimSpace(string(out)) != "the feature" {
		t.Fatalf("feature.txt not on base after merge: %v (%s)", gerr, out)
	}
}

func TestWorkspaceOpenPRWithAgentBody(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "scratch name", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	prID, err := eng.OpenWorkspacePRWith(ws.ID, "Agent-written title", "This covers the whole workspace diff, not just this chat.", false)
	if err != nil {
		t.Fatalf("OpenWorkspacePRWith: %v", err)
	}
	pr, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if pr.Title != "Agent-written title" {
		t.Errorf("OpenWorkspacePRWith title = %q, want %q", pr.Title, "Agent-written title")
	}
	const wantBody = "This covers the whole workspace diff, not just this chat."
	if got := prCommentDescription(pr); got != wantBody {
		t.Errorf("OpenWorkspacePRWith description = %q, want %q", got, wantBody)
	}

	if err := svc.Code.WriteFile(projectID, ws.ID, "more.txt", "more\n"); err != nil {
		t.Fatalf("WriteFile more: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: more"); err != nil || !res.Committed {
		t.Fatalf("Commit more: res=%+v err=%v", res, err)
	}
	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err != nil {
		t.Fatalf("OpenWorkspacePR(existing): %v", err)
	}
	again, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR after sync: %v", err)
	}
	if again.Title != "Agent-written title" {
		t.Errorf("push without a body title = %q, want original %q", again.Title, "Agent-written title")
	}
	if got := prCommentDescription(again); got != wantBody {
		t.Errorf("push without a body description = %q, want original %q", got, wantBody)
	}
	if again.FileCount < 2 {
		t.Errorf("syncExistingWorkspacePR FileCount = %d, want >= 2 after extra commit", again.FileCount)
	}

	const retryBody = "A later /open-pr on a Local PR must cover the whole workspace."
	if _, err := eng.OpenWorkspacePRWith(ws.ID, "A different title", retryBody, false); err != nil {
		t.Fatalf("OpenWorkspacePRWith(existing local): %v", err)
	}
	rewritten, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR after local rewrite: %v", err)
	}
	if rewritten.Title != "A different title" {
		t.Errorf("Local open_pr title = %q, want the new agent title", rewritten.Title)
	}
	if got := prCommentDescription(rewritten); got != retryBody {
		t.Errorf("Local open_pr description = %q, want %q", got, retryBody)
	}
}

func TestWorkspaceOpenPRFallbackSkipsPromptActionWire(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "scratch name", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	wire := "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR."
	label := "Review & fix to a PR"
	if _, ok := eng.Store.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.Goal = wire
		w.GoalDisplay = label
	}); !ok {
		t.Fatal("UpdateWorkspace")
	}
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	prID, err := eng.OpenWorkspacePR(ws.ID, false)
	if err != nil {
		t.Fatalf("OpenWorkspacePR: %v", err)
	}
	pr, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if pr.Title != label {
		t.Errorf("OpenWorkspacePR title = %q, want GoalDisplay %q", pr.Title, label)
	}
	got := prCommentDescription(pr)
	if strings.Contains(got, "The human already wrote") || strings.Contains(got, "Do not reopen scope") {
		t.Errorf("OpenWorkspacePR description dumped the prompt-action wire: %q", got)
	}
	if !strings.Contains(got, "## Changes") {
		t.Errorf("OpenWorkspacePR description = %q, want the factual changes fallback", got)
	}
}

// TestWorkspaceMergeLocal covers the no-PR land for a hand-driven workspace:
// commit in the worktree, MergeWorkspace, confirm the change is on the local
// base and no PR was created.
func TestWorkspaceMergeLocal(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "add a feature", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)

	if _, err := svc.Workspaces.MergeWorkspace(ws.ID, ""); err == nil {
		t.Error("MergeWorkspace on an empty workspace should error")
	}
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); land.AheadBase != 0 {
		t.Errorf("WorkspaceLandState().AheadBase = %d, want 0 when HEAD is the merge-base", land.AheadBase)
	}

	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); !land.Dirty {
		t.Errorf("WorkspaceLandState().Dirty = %v, want true after uncommitted write", land.Dirty)
	}
	if _, err := svc.Workspaces.MergeWorkspace(ws.ID, ""); err == nil {
		t.Error("MergeWorkspace with uncommitted changes should error")
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); land.AheadBase < 1 {
		t.Errorf("WorkspaceLandState().AheadBase = %d, want >= 1 after commit", land.AheadBase)
	}

	merged, err := svc.Workspaces.MergeWorkspace(ws.ID, "")
	if err != nil {
		t.Fatalf("MergeWorkspace: %v", err)
	}
	if merged.State != domain.StateMerged {
		t.Fatalf("MergeWorkspace state = %q, want %q", merged.State, domain.StateMerged)
	}
	if _, ok := eng.Store.PRByWorkspace(ws.ID); ok {
		t.Error("MergeWorkspace must not create a PR")
	}
	out, gerr := exec.Command("git", "-C", repo, "show", ws.BaseBranch+":feature.txt").CombinedOutput()
	if gerr != nil || strings.TrimSpace(string(out)) != "the feature" {
		t.Fatalf("feature.txt not on base after MergeWorkspace: %v (%s)", gerr, out)
	}

	again, err := svc.Workspaces.MergeWorkspace(ws.ID, "")
	if err != nil {
		t.Fatalf("MergeWorkspace idempotent: %v", err)
	}
	if again.State != domain.StateMerged {
		t.Errorf("MergeWorkspace idempotent state = %q, want %q", again.State, domain.StateMerged)
	}
}

func TestWorkspaceMergeRefusedHasRun(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "looped", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if _, ok := eng.Store.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.RunStarted = true
		w.State = domain.StateCoding
	}); !ok {
		t.Fatal("UpdateWorkspace RunStarted")
	}
	if _, err := svc.Workspaces.MergeWorkspace(ws.ID, ""); err == nil {
		t.Error("MergeWorkspace while a loop is running should error")
	}
	if _, err := svc.Workspaces.UpdateWorkspaceWithBase(ws.ID, ""); err == nil {
		t.Error("UpdateWorkspaceWithBase while a loop is running should error")
	}
	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err == nil {
		t.Error("OpenWorkspacePR while a loop is running should error")
	}

	if _, ok := eng.Store.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.State = domain.StatePaused
		w.Phase = "idle"
	}); !ok {
		t.Fatal("UpdateWorkspace idle")
	}
	if err := svc.Code.WriteFile(projectID, ws.ID, "idle.txt", "done\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "idle: add file"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err != nil {
		t.Errorf("OpenWorkspacePR after idle converge: %v", err)
	}
}

func TestWorkspaceUpdateWithBase(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "catch up", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.BehindBase != 0 || land.AheadBase < 1 || land.Dirty || land.Unmerged {
		t.Fatalf("WorkspaceLandState() = %+v, want behind=0 ahead>=1 dirty=false unmerged=false", land)
	}

	runGit := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	if err := os.WriteFile(filepath.Join(repo, "extra.txt"), []byte("base moved\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit("add", "extra.txt")
	runGit("commit", "-m", "advance main")

	if _, err := svc.Workspaces.MergeWorkspace(ws.ID, ""); err == nil {
		t.Error("MergeWorkspace while behind base should error")
	}
	land = svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.BehindBase < 1 {
		t.Fatalf("WorkspaceLandState().BehindBase = %d, want >= 1", land.BehindBase)
	}

	res, err := svc.Workspaces.UpdateWorkspaceWithBase(ws.ID, "")
	if err != nil {
		t.Fatalf("UpdateWorkspaceWithBase: %v", err)
	}
	if res.Status != domain.UpdateWorkspaceUpdated {
		t.Fatalf("UpdateWorkspaceWithBase status = %q, want %q", res.Status, domain.UpdateWorkspaceUpdated)
	}
	out, gerr := exec.Command("git", "-C", ws.WorktreePath, "show", "HEAD:extra.txt").CombinedOutput()
	if gerr != nil || strings.TrimSpace(string(out)) != "base moved" {
		t.Fatalf("extra.txt not in worktree after update: %v (%s)", gerr, out)
	}
	land = svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.BehindBase != 0 {
		t.Errorf("WorkspaceLandState().BehindBase = %d after update, want 0", land.BehindBase)
	}

	noop, err := svc.Workspaces.UpdateWorkspaceWithBase(ws.ID, "")
	if err != nil {
		t.Fatalf("UpdateWorkspaceWithBase noop: %v", err)
	}
	if noop.Status != domain.UpdateWorkspaceNoop {
		t.Errorf("UpdateWorkspaceWithBase noop status = %q, want %q", noop.Status, domain.UpdateWorkspaceNoop)
	}
}

func TestWorkspaceUpdateWithBaseConflict(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "conflict", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "README.md", "workspace edit\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: edit readme"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	runGit := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("main edit\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit("add", "README.md")
	runGit("commit", "-m", "advance main with conflict")

	res, err := svc.Workspaces.UpdateWorkspaceWithBase(ws.ID, "")
	if err != nil {
		t.Fatalf("UpdateWorkspaceWithBase: %v", err)
	}
	if res.Status != domain.UpdateWorkspaceConflict {
		t.Fatalf("UpdateWorkspaceWithBase() status = %q, want %q", res.Status, domain.UpdateWorkspaceConflict)
	}
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); !land.Unmerged {
		t.Errorf("WorkspaceLandState().Unmerged = %v, want true after conflict", land.Unmerged)
	}
}

// TestWorkspaceRemove covers the checkLoopSettled fix: a non-loop workspace has no
// autopilot state, so RemoveWorkspace must succeed, tear down the worktree, and
// keep the store record (Archived) — not error with "is running".
func TestWorkspaceRemove(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	t.Cleanup(eng.Shutdown)
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "scratch", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	path := ws.WorktreePath
	removed, err := svc.Workspaces.RemoveWorkspace(ws.ID)
	if err != nil {
		t.Fatalf("RemoveWorkspace should succeed for a non-loop workspace: %v", err)
	}
	if !removed.Archived {
		t.Error("expected Archived=true after remove")
	}
	if removed.WorktreePath != "" {
		t.Errorf("expected WorktreePath cleared, got %q", removed.WorktreePath)
	}
	got, err := svc.Workspaces.GetWorkspace(ws.ID)
	if err != nil {
		t.Fatalf("workspace record should remain after remove: %v", err)
	}
	if !got.Archived {
		t.Error("GetWorkspace should return Archived=true")
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		if _, err := os.Stat(path); os.IsNotExist(err) {
			break
		}
		if time.Now().After(deadline) {
			t.Errorf("worktree should be removed after remove, still at %s", path)
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestMergeWorkspacePushesRemoteBase(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	if _, err := eng.Store.AddProject(domain.Project{ID: "premote", Name: "premote", RepoPath: clone, BaseBranch: "origin/main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	ws, err := svc.Workspaces.CreateWorkspace("premote", "land on origin", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); land.Remote != "origin" {
		t.Errorf("WorkspaceLandState().Remote = %q, want origin", land.Remote)
	}

	if err := svc.Code.WriteFile("premote", ws.ID, "feature.txt", "from workspace\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), "premote", ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	merged, err := svc.Workspaces.MergeWorkspace(ws.ID, "squash")
	if err != nil {
		t.Fatalf("MergeWorkspace: %v", err)
	}
	if merged.State != domain.StateMerged {
		t.Fatalf("MergeWorkspace state = %q, want %q", merged.State, domain.StateMerged)
	}
	out, gerr := exec.Command("git", "-C", bare, "show", "HEAD:feature.txt").CombinedOutput()
	if gerr != nil || strings.TrimSpace(string(out)) != "from workspace" {
		t.Fatalf("feature.txt not on origin after MergeWorkspace: %v (%s)", gerr, out)
	}
}

func TestMergeWorkspaceRefusedWhenGitHubRequiresPR(t *testing.T) {
	restore := engine.ReplaceGitHubBranchRequiresPR(func(context.Context, string, string) bool { return true })
	t.Cleanup(restore)

	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	if _, err := eng.Store.AddProject(domain.Project{
		ID: "pghremote", Name: "pghremote", RepoPath: clone, BaseBranch: "origin/main", PROnGitHub: true,
	}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	ws, err := svc.Workspaces.CreateWorkspace("pghremote", "land on origin", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); !land.RequirePR {
		t.Errorf("WorkspaceLandState().RequirePR = false, want true")
	}
	if err := svc.Code.WriteFile("pghremote", ws.ID, "feature.txt", "from workspace\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), "pghremote", ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	if _, err := svc.Workspaces.MergeWorkspace(ws.ID, "squash"); err == nil {
		t.Fatal("MergeWorkspace error = nil, want refuse when GitHub requires a pull request")
	} else if !strings.Contains(err.Error(), "requires a GitHub pull request") {
		t.Errorf("MergeWorkspace error = %v, want requires a GitHub pull request", err)
	}
	if _, gerr := exec.Command("git", "-C", bare, "show", "HEAD:feature.txt").CombinedOutput(); gerr == nil {
		t.Error("MergeWorkspace landed on origin; GitHub-required-PR remotes must Open PR instead")
	}
	if got, ok := eng.Store.Workspace(ws.ID); ok && got.State == domain.StateMerged {
		t.Errorf("workspace state = %q, want unmerged after refused land", got.State)
	}
}

func TestMergeWorkspaceGitHubRemoteWhenPRNotRequired(t *testing.T) {
	restore := engine.ReplaceGitHubBranchRequiresPR(func(context.Context, string, string) bool { return false })
	t.Cleanup(restore)

	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	if _, err := eng.Store.AddProject(domain.Project{
		ID: "pghopen", Name: "pghopen", RepoPath: clone, BaseBranch: "origin/main", PROnGitHub: true,
	}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	ws, err := svc.Workspaces.CreateWorkspace("pghopen", "land on origin", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); land.RequirePR {
		t.Errorf("WorkspaceLandState().RequirePR = true, want false when GitHub allows a direct land")
	}
	if err := svc.Code.WriteFile("pghopen", ws.ID, "feature.txt", "from workspace\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), "pghopen", ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	merged, err := svc.Workspaces.MergeWorkspace(ws.ID, "squash")
	if err != nil {
		t.Fatalf("MergeWorkspace: %v", err)
	}
	if merged.State != domain.StateMerged {
		t.Fatalf("MergeWorkspace state = %q, want %q", merged.State, domain.StateMerged)
	}
	out, gerr := exec.Command("git", "-C", bare, "show", "HEAD:feature.txt").CombinedOutput()
	if gerr != nil || strings.TrimSpace(string(out)) != "from workspace" {
		t.Fatalf("feature.txt not on origin after MergeWorkspace: %v (%s)", gerr, out)
	}
}

func TestMergeWorkspaceMarksMergedWhenPushFails(t *testing.T) {
	clone, _ := initGitRepoWithRemote(t)
	runGit(t, clone, "remote", "set-url", "origin", filepath.Join(t.TempDir(), "missing.git"))
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	if _, err := eng.Store.AddProject(domain.Project{ID: "ppushfail", Name: "ppushfail", RepoPath: clone, BaseBranch: "origin/main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	ws, err := svc.Workspaces.CreateWorkspace("ppushfail", "land locally", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile("ppushfail", ws.ID, "feature.txt", "from workspace\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), "ppushfail", ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	merged, err := svc.Workspaces.MergeWorkspace(ws.ID, "squash")
	if err == nil {
		t.Fatal("MergeWorkspace error = nil, want push failure")
	}
	if merged.State != domain.StateMerged {
		t.Fatalf("MergeWorkspace state = %q, want %q after a failed push", merged.State, domain.StateMerged)
	}
	again, err := svc.Workspaces.MergeWorkspace(ws.ID, "squash")
	if err != nil {
		t.Fatalf("retry MergeWorkspace after failed push: %v", err)
	}
	if again.State != domain.StateMerged {
		t.Errorf("retry state = %q, want %q", again.State, domain.StateMerged)
	}
}

func TestMergeWorkspacePushesLocalBaseWhenOriginExists(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	if _, err := eng.Store.AddProject(domain.Project{ID: "plocalbase", Name: "plocalbase", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	ws, err := svc.Workspaces.CreateWorkspace("plocalbase", "land and push", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if land := svc.Workspaces.WorkspaceLandState(ws.ID); land.Remote != "origin" {
		t.Errorf("WorkspaceLandState().Remote = %q, want origin", land.Remote)
	}

	if err := svc.Code.WriteFile("plocalbase", ws.ID, "feature.txt", "from workspace\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), "plocalbase", ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	merged, err := svc.Workspaces.MergeWorkspace(ws.ID, "squash")
	if err != nil {
		t.Fatalf("MergeWorkspace: %v", err)
	}
	if merged.State != domain.StateMerged {
		t.Fatalf("MergeWorkspace state = %q, want %q", merged.State, domain.StateMerged)
	}
	out, gerr := exec.Command("git", "-C", bare, "show", "HEAD:feature.txt").CombinedOutput()
	if gerr != nil || strings.TrimSpace(string(out)) != "from workspace" {
		t.Fatalf("feature.txt not on origin after MergeWorkspace: %v (%s)", gerr, out)
	}
}

func TestBehindMergeBaseFetchesRemoteTip(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	if _, err := eng.Store.AddProject(domain.Project{
		ID: "pbehind", Name: "pbehind", RepoPath: clone, BaseBranch: "main",
	}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	ws, err := svc.Workspaces.CreateWorkspace("pbehind", "feature", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile("pbehind", ws.ID, "feature.txt", "mine\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), "pbehind", ws.ID, "solo: feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: %v", err)
	}
	if _, err := eng.Store.AddPR(domain.PR{
		ID: "gh-111", WorkspaceID: ws.ID, Group: "you", Number: 111,
		Source: domain.SourceGitHub, Branch: ws.Branch, Base: "main",
		Title: "feat", State: "In review",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	other := filepath.Join(t.TempDir(), "other")
	runGit(t, t.TempDir(), "clone", bare, other)
	runGit(t, other, "config", "user.name", "test")
	runGit(t, other, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(other, "base-moved.txt"), []byte("ahead\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, other, "add", "base-moved.txt")
	runGit(t, other, "commit", "-m", "advance merge base")
	runGit(t, other, "push", "origin", "main")

	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.BehindBase != 0 {
		t.Fatalf("WorkspaceLandState().BehindBase = %d before fetch, want 0 (stale local main)", land.BehindBase)
	}
	eng.RefreshPRBehind("gh-111")
	pr, _ := eng.Store.PR("gh-111")
	if pr.BehindBase < 1 {
		t.Fatalf("RefreshPRBehind BehindBase = %d, want >= 1 after fetching origin/main", pr.BehindBase)
	}
	land = svc.Workspaces.FetchWorkingCopyUpstream(ws.ID)
	if land.BehindBase < 1 {
		t.Fatalf("FetchWorkingCopyUpstream().BehindBase = %d, want >= 1", land.BehindBase)
	}
}

func TestGetPRFetchesRemoteBaseBeforeCommitLog(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "feature", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "mine\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: %v", err)
	}

	staleOut, err := exec.Command("git", "-C", clone, "rev-parse", "origin/main").Output()
	if err != nil {
		t.Fatalf("rev-parse origin/main: %v", err)
	}
	stale := strings.TrimSpace(string(staleOut))

	other := filepath.Join(t.TempDir(), "other")
	runGit(t, t.TempDir(), "clone", bare, other)
	runGit(t, other, "config", "user.name", "test")
	runGit(t, other, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(other, "base-moved.txt"), []byte("ahead\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, other, "add", "base-moved.txt")
	runGit(t, other, "commit", "-m", "advance merge base")
	runGit(t, other, "push", "origin", "main")

	runGit(t, ws.WorktreePath, "fetch", "origin", "main")
	runGit(t, ws.WorktreePath, "merge", "origin/main", "-m", "sync with base")
	runGit(t, clone, "update-ref", "refs/remotes/origin/main", stale)

	if _, err := eng.Store.AddPR(domain.PR{
		ID: "pr-stale-base", WorkspaceID: ws.ID, Group: "you",
		Source: domain.SourceLocal, Branch: ws.Branch, Base: "origin/main",
		Title: "feat", State: "In review",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	pr, err := svc.PRs.GetPR("pr-stale-base")
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	for _, c := range pr.Commits {
		if strings.Contains(c.Message, "advance merge base") {
			t.Errorf("GetPR() commits include %q, want remote base fetched before git log", c.Message)
		}
	}
	found := false
	for _, c := range pr.Commits {
		if strings.Contains(c.Message, "solo: feature") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("GetPR() commits = %+v, want solo: feature after fetching origin/main", pr.Commits)
	}
}

func TestBehindRemoteDevelopBaseWithoutPR(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	runGit(t, clone, "checkout", "-b", "develop")
	runGit(t, clone, "push", "-u", "origin", "develop")
	runGit(t, clone, "checkout", "main")

	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	if _, err := eng.Store.AddProject(domain.Project{
		ID: "pdev", Name: "pdev", RepoPath: clone, BaseBranch: "origin/main",
	}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	ws, err := svc.Workspaces.CreateWorkspace("pdev", "off develop", "origin/develop", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if ws.BaseBranch != "origin/develop" {
		t.Fatalf("workspace BaseBranch = %q, want origin/develop", ws.BaseBranch)
	}
	if _, ok := eng.Store.PRByWorkspace(ws.ID); ok {
		t.Fatal("workspace should have no PR")
	}

	other := filepath.Join(t.TempDir(), "other")
	runGit(t, t.TempDir(), "clone", bare, other)
	runGit(t, other, "config", "user.name", "test")
	runGit(t, other, "config", "user.email", "test@example.com")
	runGit(t, other, "checkout", "develop")
	if err := os.WriteFile(filepath.Join(other, "develop-ahead.txt"), []byte("from develop\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, other, "add", "develop-ahead.txt")
	runGit(t, other, "commit", "-m", "advance develop")
	runGit(t, other, "push", "origin", "develop")
	if err := os.WriteFile(filepath.Join(other, "main-only.txt"), []byte("from main\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, other, "checkout", "main")
	runGit(t, other, "add", "main-only.txt")
	runGit(t, other, "commit", "-m", "advance main — must not be the compare tip")
	runGit(t, other, "push", "origin", "main")

	land := svc.Workspaces.FetchWorkingCopyUpstream(ws.ID)
	if land.BehindBase < 1 {
		t.Fatalf("FetchWorkingCopyUpstream().BehindBase = %d, want >= 1 vs origin/develop", land.BehindBase)
	}

	res, err := svc.Workspaces.UpdateWorkspaceWithBase(ws.ID, "")
	if err != nil {
		t.Fatalf("UpdateWorkspaceWithBase: %v", err)
	}
	if res.Status != domain.UpdateWorkspaceUpdated {
		t.Fatalf("UpdateWorkspaceWithBase status = %q, want %q", res.Status, domain.UpdateWorkspaceUpdated)
	}
	got, gerr := os.ReadFile(filepath.Join(ws.WorktreePath, "develop-ahead.txt"))
	if gerr != nil || strings.TrimSpace(string(got)) != "from develop" {
		t.Errorf("develop-ahead.txt after update = %q (err %v), want from develop", got, gerr)
	}
	if _, err := os.Stat(filepath.Join(ws.WorktreePath, "main-only.txt")); err == nil {
		t.Error("update used origin/main instead of the workspace base origin/develop")
	}
}

func TestMergeWorkspaceGitHubPRDoesNotLandLocally(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "gh land", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "gh only\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	if _, err := eng.Store.AddPR(domain.PR{
		ID: "gh-99", WorkspaceID: ws.ID, Number: 99, Source: domain.SourceGitHub,
		Title: "gh", State: "In review", Group: "you",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	if _, err := svc.Workspaces.MergeWorkspace(ws.ID, "squash"); err == nil {
		t.Error("MergeWorkspace() error = nil, want refuse when a GitHub PR is open")
	}
	if _, gerr := exec.Command("git", "-C", repo, "show", "HEAD:feature.txt").CombinedOutput(); gerr == nil {
		t.Error("MergeWorkspace on a GitHub PR landed locally; merge belongs on the PR page")
	}
}

func TestWorkspaceGitHubPRAheadAndBehindRemote(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "add a feature", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	runGit(t, ws.WorktreePath, "push", "-u", "origin", "HEAD:refs/heads/"+ws.Branch)

	if _, err := eng.Store.AddPR(domain.PR{
		ID: "gh-9", WorkspaceID: ws.ID, Group: "you", Number: 9,
		Source: domain.SourceGitHub, Branch: ws.Branch, Base: "main",
		Title: "feat", State: "In review",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	if err := svc.Code.WriteFile(projectID, ws.ID, "local.txt", "local\n"); err != nil {
		t.Fatalf("WriteFile local: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: local"); err != nil || !res.Committed {
		t.Fatalf("Commit local: res=%+v err=%v", res, err)
	}
	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.AheadRemote < 1 {
		t.Errorf("WorkspaceLandState().AheadRemote = %d, want >= 1 after unpushed commit vs origin/%s", land.AheadRemote, ws.Branch)
	}

	// Reset local extra commit so we can make GitHub ahead without diverging.
	runGit(t, ws.WorktreePath, "reset", "--hard", "origin/"+ws.Branch)

	other := filepath.Join(t.TempDir(), "other")
	runGit(t, t.TempDir(), "clone", bare, other)
	runGit(t, other, "config", "user.name", "test")
	runGit(t, other, "config", "user.email", "test@example.com")
	runGit(t, other, "checkout", ws.Branch)
	if err := os.WriteFile(filepath.Join(other, "from-github.txt"), []byte("remote\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, other, "add", "from-github.txt")
	runGit(t, other, "commit", "-m", "from github")
	runGit(t, other, "push", "origin", "HEAD")

	land = svc.Workspaces.FetchWorkingCopyUpstream(ws.ID)
	if land.BehindRemote < 1 {
		t.Errorf("FetchWorkingCopyUpstream().BehindRemote = %d, want >= 1 after GitHub advanced", land.BehindRemote)
	}

	res, err := svc.Workspaces.PullWorkspacePR(ws.ID)
	if err != nil {
		t.Fatalf("PullWorkspacePR: %v", err)
	}
	if res.Status != domain.UpdateWorkspaceUpdated {
		t.Errorf("PullWorkspacePR status = %q, want %q", res.Status, domain.UpdateWorkspaceUpdated)
	}
	land = svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.BehindRemote != 0 {
		t.Errorf("WorkspaceLandState().BehindRemote = %d after pull, want 0", land.BehindRemote)
	}
	got, err := os.ReadFile(filepath.Join(ws.WorktreePath, "from-github.txt"))
	if err != nil || strings.TrimSpace(string(got)) != "remote" {
		t.Errorf("from-github.txt after pull = %q (err %v), want remote", got, err)
	}
}

func TestFillPromptGitEnvOmitsLocalPRID(t *testing.T) {
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{ID: "p", Name: "p", RepoPath: "/tmp/x", BaseBranch: "main"})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.Store.AddWorkspace(domain.Workspace{ID: "W-local", ProjectID: p.ID, Goal: "g", BaseBranch: "main"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if _, err := eng.Store.AddPR(domain.PR{
		ID: "7", WorkspaceID: ws.ID, Number: 7, Source: domain.SourceLocal,
		State: "In review", Group: "you",
	}); err != nil {
		t.Fatalf("AddPR local: %v", err)
	}
	rc := &bundles.PromptRunContext{}
	svc.Bundle.fillPromptGitEnv(rc, ws.ID)
	if rc.PRID != "" {
		t.Errorf("fillPromptGitEnv(local) PRID=%q, want empty so /open-pr can retry", rc.PRID)
	}
	if rc.PRNumber != 7 {
		t.Errorf("fillPromptGitEnv(local) PRNumber=%d, want 7", rc.PRNumber)
	}
	if rc.PRSource != string(domain.SourceLocal) {
		t.Errorf("fillPromptGitEnv(local) PRSource=%q, want %q", rc.PRSource, domain.SourceLocal)
	}

	ghWS, err := eng.Store.AddWorkspace(domain.Workspace{ID: "W-gh", ProjectID: p.ID, Goal: "g", BaseBranch: "main"})
	if err != nil {
		t.Fatalf("AddWorkspace gh: %v", err)
	}
	if _, err := eng.Store.AddPR(domain.PR{
		ID: "gh-9", WorkspaceID: ghWS.ID, Number: 9, Source: domain.SourceGitHub,
		State: "In review", Group: "you",
	}); err != nil {
		t.Fatalf("AddPR gh: %v", err)
	}
	ghRC := &bundles.PromptRunContext{}
	svc.Bundle.fillPromptGitEnv(ghRC, ghWS.ID)
	if ghRC.PRID != "gh-9" || ghRC.PRNumber != 9 || ghRC.PRSource != string(domain.SourceGitHub) {
		t.Errorf("fillPromptGitEnv(gh) PRID=%q PRNumber=%d PRSource=%q, want gh-9 #9 gh", ghRC.PRID, ghRC.PRNumber, ghRC.PRSource)
	}
}

func TestFillPromptGitEnvPRChatHost(t *testing.T) {
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{ID: "p", Name: "p", RepoPath: "/tmp/clone", BaseBranch: "main"})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-p-4", ProjectID: p.ID, Number: 4, Source: domain.SourceGitHub,
		Title: "Imported", URL: "https://github.com/acme/r/pull/4",
		Base: "main", Branch: "feat", State: "Open", Group: "open",
	}
	if _, err := eng.Store.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.Store.AddWorkspace(domain.Workspace{
		ID: "W-host", ProjectID: p.ID, Goal: "Imported", HostPRID: pr.ID,
		WorktreePath: p.RepoPath, BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	rc := &bundles.PromptRunContext{Surface: "workspace"}
	svc.Bundle.fillPromptGitEnv(rc, host.ID)
	if !rc.PRHost {
		t.Error("fillPromptGitEnv(host) PRHost=false, want true")
	}
	if rc.Surface != "github_pr" {
		t.Errorf("fillPromptGitEnv(host) Surface=%q, want github_pr", rc.Surface)
	}
	if rc.PRID != pr.ID || rc.PRNumber != 4 || rc.PRURL != pr.URL {
		t.Errorf("fillPromptGitEnv(host) PRID=%q Number=%d URL=%q, want %q 4 %q",
			rc.PRID, rc.PRNumber, rc.PRURL, pr.ID, pr.URL)
	}
}

func TestListPRFindingsUsesChatHost(t *testing.T) {
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{ID: "p", Name: "p", RepoPath: "/tmp/clone", BaseBranch: "main"})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-p-5", ProjectID: p.ID, Number: 5, Source: domain.SourceGitHub,
		State: "Open", Group: "open",
	}
	if _, err := eng.Store.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.Store.AddWorkspace(domain.Workspace{
		ID: "W-host-f", ProjectID: p.ID, Goal: "PR", HostPRID: pr.ID,
		WorktreePath: p.RepoPath,
	})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	eng.Store.AddFinding(domain.Finding{
		ID: "f1", WorkspaceID: host.ID, File: "a.go", Line: 1,
		Severity: domain.SevHigh, Message: "bug", Status: "open",
	})
	got := svc.PRs.ListPRFindings(pr.ID)
	if len(got) != 1 || got[0].ID != "f1" {
		t.Errorf("ListPRFindings(%q) = %+v, want host finding f1", pr.ID, got)
	}
}

func TestWorkspaceOpenPRRetriesGitHubAfterLocalFallback(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{
		ID: "pgh", Name: "pgh", RepoPath: repo, BaseBranch: "main", PROnGitHub: true,
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := svc.Workspaces.CreateWorkspace(p.ID, "add a feature", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(p.ID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), p.ID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	prID, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false)
	if err != nil {
		t.Fatalf("OpenWorkspacePR: %v", err)
	}
	pr, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if pr.Source != domain.SourceLocal {
		t.Fatalf("OpenWorkspacePR source = %q, want local fallback when gh create fails", pr.Source)
	}
	again, err := eng.OpenWorkspacePRWith(ws.ID, "Retry title", "Retry body", false)
	if err != nil {
		t.Fatalf("OpenWorkspacePRWith(retry) err=%v, want a retry of GitHub create (still Local if gh is unavailable)", err)
	}
	if again != prID {
		t.Errorf("OpenWorkspacePRWith(retry) id = %q, want same Local id %q", again, prID)
	}
	still, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR after retry: %v", err)
	}
	if still.Source != domain.SourceLocal {
		t.Errorf("retry source = %q, want local while gh is unavailable", still.Source)
	}
}

func TestWorkspaceOpenPRDraftPersistsOnLocalFallback(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{
		ID: "pgh-draft", Name: "pgh-draft", RepoPath: repo, BaseBranch: "main", PROnGitHub: true,
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := svc.Workspaces.CreateWorkspace(p.ID, "add a feature", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(p.ID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), p.ID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}

	prID, err := eng.OpenWorkspacePRWith(ws.ID, "Draft title", "Still cooking.", true)
	if err != nil {
		t.Fatalf("OpenWorkspacePRWith(draft) err=%v", err)
	}
	pr, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if pr.Source != domain.SourceLocal {
		t.Fatalf("OpenWorkspacePRWith(draft) source = %q, want local fallback when gh create fails", pr.Source)
	}
	if !pr.Draft {
		t.Error("OpenWorkspacePRWith(draft) Draft = false, want true so a later GitHub retry stays a draft")
	}
	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if !land.PRDraft {
		t.Error("WorkspaceLandState() PRDraft = false, want true")
	}
}

func TestWorkspaceForcePushDivergedGitHubPR(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "add a feature", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	runGit(t, ws.WorktreePath, "push", "-u", "origin", "HEAD:refs/heads/"+ws.Branch)

	if _, err := eng.Store.AddPR(domain.PR{
		ID: "gh-11", WorkspaceID: ws.ID, Group: "you", Number: 11,
		Source: domain.SourceGitHub, Branch: ws.Branch, Base: "main",
		Title: "feat", State: "In review",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	if err := svc.Code.WriteFile(projectID, ws.ID, "rebased.txt", "after rebase\n"); err != nil {
		t.Fatalf("WriteFile local: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: rebased"); err != nil || !res.Committed {
		t.Fatalf("Commit local: res=%+v err=%v", res, err)
	}

	other := filepath.Join(t.TempDir(), "other")
	runGit(t, t.TempDir(), "clone", bare, other)
	runGit(t, other, "config", "user.name", "test")
	runGit(t, other, "config", "user.email", "test@example.com")
	runGit(t, other, "checkout", ws.Branch)
	if err := os.WriteFile(filepath.Join(other, "stale.txt"), []byte("pre-rebase\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, other, "add", "stale.txt")
	runGit(t, other, "commit", "-m", "stale github tip")
	runGit(t, other, "push", "origin", "HEAD")

	land := svc.Workspaces.FetchWorkingCopyUpstream(ws.ID)
	if land.AheadRemote < 1 || land.BehindRemote < 1 {
		t.Fatalf("FetchWorkingCopyUpstream() ahead=%d behind=%d, want both >= 1 (diverged)", land.AheadRemote, land.BehindRemote)
	}

	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err == nil {
		t.Fatal("OpenWorkspacePR(false) on a diverged GitHub PR should refuse a regular push")
	}
	if out, err := exec.Command("git", "-C", bare, "cat-file", "-e", ws.Branch+":stale.txt").CombinedOutput(); err != nil {
		t.Fatalf("OpenWorkspacePR(false) dropped remote-only stale.txt: %s (err %v)", out, err)
	}

	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, true); err != nil {
		t.Fatalf("OpenWorkspacePR(true) err=%v", err)
	}
	land = svc.Workspaces.FetchWorkingCopyUpstream(ws.ID)
	if land.AheadRemote != 0 || land.BehindRemote != 0 {
		t.Errorf("after force-push, WorkspaceLandState ahead=%d behind=%d, want 0 0", land.AheadRemote, land.BehindRemote)
	}
	got, err := exec.Command("git", "-C", bare, "show", ws.Branch+":rebased.txt").CombinedOutput()
	if err != nil || strings.TrimSpace(string(got)) != "after rebase" {
		t.Errorf("bare %s:rebased.txt = %q (err %v), want after rebase", ws.Branch, got, err)
	}
	if out, err := exec.Command("git", "-C", bare, "cat-file", "-e", ws.Branch+":stale.txt").CombinedOutput(); err == nil {
		t.Errorf("bare still has stale.txt after force-push: %s", out)
	}
}

func TestWorkspaceUpdateWithBasePushesGitHubPR(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "catch up", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	runGit(t, ws.WorktreePath, "push", "-u", "origin", "HEAD:refs/heads/"+ws.Branch)
	if _, err := eng.Store.AddPR(domain.PR{
		ID: "gh-14", WorkspaceID: ws.ID, Group: "you", Number: 14,
		Source: domain.SourceGitHub, Branch: ws.Branch, Base: "main",
		Title: "feat", State: "In review",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	if err := os.WriteFile(filepath.Join(clone, "from-main.txt"), []byte("base moved\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, clone, "add", "from-main.txt")
	runGit(t, clone, "commit", "-m", "advance main")

	res, err := svc.Workspaces.UpdateWorkspaceWithBase(ws.ID, "rebase")
	if err != nil {
		t.Fatalf("UpdateWorkspaceWithBase(rebase) err=%v", err)
	}
	if res.Status != domain.UpdateWorkspaceUpdated {
		t.Fatalf("UpdateWorkspaceWithBase status = %q, want %q", res.Status, domain.UpdateWorkspaceUpdated)
	}
	if res.PushedPR != 14 {
		t.Errorf("UpdateWorkspaceWithBase PushedPR = %d, want 14 (auto-push after rebase)", res.PushedPR)
	}
	if !res.ForcePushed {
		t.Error("UpdateWorkspaceWithBase ForcePushed = false, want true after rebase onto main")
	}
	got, err := exec.Command("git", "-C", bare, "show", ws.Branch+":from-main.txt").CombinedOutput()
	if err != nil || strings.TrimSpace(string(got)) != "base moved" {
		t.Errorf("bare %s:from-main.txt = %q (err %v), want base moved", ws.Branch, got, err)
	}
	land := svc.Workspaces.FetchWorkingCopyUpstream(ws.ID)
	if land.AheadRemote != 0 || land.BehindRemote != 0 {
		t.Errorf("after update+force-push, ahead=%d behind=%d, want 0 0", land.AheadRemote, land.BehindRemote)
	}
}

func TestWorkspaceUpdateWithBaseMergeDoesNotForceGitHubPR(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "catch up", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	runGit(t, ws.WorktreePath, "push", "-u", "origin", "HEAD:refs/heads/"+ws.Branch)
	if _, err := eng.Store.AddPR(domain.PR{
		ID: "gh-15", WorkspaceID: ws.ID, Group: "you", Number: 15,
		Source: domain.SourceGitHub, Branch: ws.Branch, Base: "main",
		Title: "feat", State: "In review",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	other := filepath.Join(t.TempDir(), "other")
	runGit(t, t.TempDir(), "clone", bare, other)
	runGit(t, other, "config", "user.name", "test")
	runGit(t, other, "config", "user.email", "test@example.com")
	runGit(t, other, "checkout", ws.Branch)
	if err := os.WriteFile(filepath.Join(other, "stale.txt"), []byte("reviewer commit\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, other, "add", "stale.txt")
	runGit(t, other, "commit", "-m", "reviewer suggestion")
	runGit(t, other, "push", "origin", "HEAD")

	if err := os.WriteFile(filepath.Join(clone, "from-main.txt"), []byte("base moved\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, clone, "add", "from-main.txt")
	runGit(t, clone, "commit", "-m", "advance main")

	res, err := svc.Workspaces.UpdateWorkspaceWithBase(ws.ID, "merge")
	if err != nil {
		t.Fatalf("UpdateWorkspaceWithBase(merge) err=%v", err)
	}
	if res.Status != domain.UpdateWorkspaceUpdated {
		t.Fatalf("UpdateWorkspaceWithBase status = %q, want %q", res.Status, domain.UpdateWorkspaceUpdated)
	}
	if res.PushedPR != 0 {
		t.Errorf("UpdateWorkspaceWithBase(merge) PushedPR = %d, want 0 (regular push must not overwrite a remote-only commit)", res.PushedPR)
	}
	if res.ForcePushed {
		t.Error("UpdateWorkspaceWithBase(merge) ForcePushed = true, want false")
	}
	if out, err := exec.Command("git", "-C", bare, "cat-file", "-e", ws.Branch+":stale.txt").CombinedOutput(); err != nil {
		t.Errorf("merge-method update dropped remote-only stale.txt: %s (err %v)", out, err)
	}
	if _, err := os.Stat(filepath.Join(ws.WorktreePath, "from-main.txt")); err != nil {
		t.Errorf("local update with main should still apply: from-main.txt: %v", err)
	}
}

// TestWorkspacePushCrossRepoPRPushesToFork covers a fork PR (head in
// auricom/recur): Push updates lands on the fork, never creates the branch on
// origin, and ahead/behind compares against pull/N/head, not origin/<branch>.
func TestWorkspacePushCrossRepoPRPushesToFork(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	root := t.TempDir()
	origin := filepath.Join(root, "celestiaorg", "recur.git")
	fork := filepath.Join(root, "auricom", "recur.git")
	runGit(t, root, "clone", "--bare", bare, origin)
	runGit(t, root, "clone", "--bare", bare, fork)
	runGit(t, clone, "remote", "set-url", "origin", origin)

	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "fork PR", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "fork: feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	const head = "recur/tab-close-context-menu"
	runGit(t, ws.WorktreePath, "push", fork, "HEAD:refs/heads/"+head)
	runGit(t, ws.WorktreePath, "push", origin, "HEAD:refs/pull/9/head")

	if _, err := eng.Store.AddPR(domain.PR{
		ID: "gh-9", WorkspaceID: ws.ID, Group: "you", Number: 9,
		Source: domain.SourceGitHub, Branch: head, Base: "main",
		Title: "feat", State: "In review", IsCrossRepository: true,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if err := svc.Code.WriteFile(projectID, ws.ID, "local.txt", "local\n"); err != nil {
		t.Fatalf("WriteFile local: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "fork: local"); err != nil || !res.Committed {
		t.Fatalf("Commit local: res=%+v err=%v", res, err)
	}

	land := svc.Workspaces.FetchWorkingCopyUpstream(ws.ID)
	if land.AheadRemote != 1 || land.BehindRemote != 0 {
		t.Fatalf("FetchWorkingCopyUpstream() ahead=%d behind=%d, want 1 0 vs pull/9/head", land.AheadRemote, land.BehindRemote)
	}

	ghDir := t.TempDir()
	gh := filepath.Join(ghDir, "gh")
	t.Setenv("PATH", ghDir+string(os.PathListSeparator)+os.Getenv("PATH"))
	if err := os.WriteFile(gh, []byte("#!/bin/sh\nexit 1\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err == nil || !strings.Contains(err.Error(), "fork") {
		t.Errorf("OpenWorkspacePR(unresolved fork) err = %v, want fork resolution error", err)
	}

	script := `#!/bin/sh
if [ "$1 $2 $3" = "pr view 9" ]; then
  echo '{"headRepository":{"name":"recur"},"headRepositoryOwner":{"login":"auricom"}}'
  exit 0
fi
exit 1
`
	if err := os.WriteFile(gh, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err != nil {
		t.Fatalf("OpenWorkspacePR(fork PR): %v", err)
	}
	if got, err := exec.Command("git", "-C", fork, "show", head+":local.txt").CombinedOutput(); err != nil || strings.TrimSpace(string(got)) != "local" {
		t.Errorf("fork %s:local.txt = %q (err %v), want local", head, got, err)
	}
	if out, err := exec.Command("git", "-C", origin, "rev-parse", "--verify", "refs/heads/"+head).CombinedOutput(); err == nil {
		t.Errorf("origin gained branch %s for a fork PR: %s", head, out)
	}
	land = svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.AheadRemote != 0 || land.BehindRemote != 0 {
		t.Errorf("WorkspaceLandState() after push ahead=%d behind=%d, want 0 0", land.AheadRemote, land.BehindRemote)
	}
}

func prCommentDescription(pr domain.PR) string {
	for _, c := range pr.Comments {
		if c.Kind == "description" {
			return c.BodyMD
		}
	}
	return ""
}
