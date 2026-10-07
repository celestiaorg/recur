package engine

import (
	"encoding/base64"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestBeginCreateWorkspaceReturnsBeforeWorktree(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)
	marker := filepath.Join(dataDir, "setup.slow")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "sleep 0.4 && echo ok > '" + marker + "'",
	})

	ws, err := eng.BeginCreateWorkspace(proj.ID, "eager", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	if ws.WorktreePath != "" {
		t.Fatal("BeginCreateWorkspace must return before the worktree exists")
	}
	if ws.Phase != phaseCreating {
		t.Fatalf("phase=%q, want creating", ws.Phase)
	}
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("setup must not have finished before BeginCreateWorkspace returned")
	}

	waitFor(t, 5*time.Second, "worktree allocate", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.WorktreePath != "" && got.Phase == "editing"
	})
	got, _ := st.Workspace(ws.ID)
	if _, err := os.Stat(marker); err != nil {
		t.Fatalf("setup did not run: %v", err)
	}
	if got.Port <= 0 {
		t.Fatalf("expected port allocated, got %d", got.Port)
	}
}

func TestAllocatePublishesWorktreeBeforeSetup(t *testing.T) {
	eng, st, sink, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)
	start := filepath.Join(dataDir, "setup.start")
	done := filepath.Join(dataDir, "setup.done")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "while [ ! -f '" + start + "' ]; do sleep 0.05; done; echo ok > '" + done + "'",
	})
	t.Cleanup(func() { _ = os.WriteFile(start, []byte("x"), 0o644) })

	ws, err := eng.BeginCreateWorkspace(proj.ID, "eager-paint", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}

	waitFor(t, 5*time.Second, "worktree path published before setup", func() bool {
		got, ok := st.Workspace(ws.ID)
		if !ok || got.WorktreePath == "" || got.Phase != phaseCreating {
			return false
		}
		for _, ev := range sink.byName("workspace.updated") {
			d, _ := ev.Data.(map[string]any)
			if d["workspaceId"] != ws.ID {
				continue
			}
			if p, _ := d["worktreePath"].(string); p != "" {
				return true
			}
		}
		return false
	})
	if _, err := os.Stat(done); err == nil {
		t.Fatal("setup finished before worktreePath was published")
	}

	if err := os.WriteFile(start, []byte("go"), 0o644); err != nil {
		t.Fatal(err)
	}
	waitFor(t, 5*time.Second, "setup complete", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.Phase == "editing"
	})
	if _, err := os.Stat(done); err != nil {
		t.Fatalf("setup did not run: %v", err)
	}
}

func TestFailedSetupKeepsWorktreeAfterEagerPublish(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)
	start := filepath.Join(dataDir, "setup.start")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "while [ ! -f '" + start + "' ]; do sleep 0.05; done; exit 1",
	})
	t.Cleanup(func() { _ = os.WriteFile(start, []byte("x"), 0o644) })

	ws, err := eng.BeginCreateWorkspace(proj.ID, "keep-on-fail", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}

	waitFor(t, 5*time.Second, "worktree path before setup", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.WorktreePath != "" && got.Phase == phaseCreating
	})
	got, _ := st.Workspace(ws.ID)
	userFile := filepath.Join(got.WorktreePath, "user-edit.txt")
	if err := os.WriteFile(userFile, []byte("keep me"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(start, []byte("go"), 0o644); err != nil {
		t.Fatal(err)
	}

	waitFor(t, 5*time.Second, "create failure", func() bool {
		cur, ok := st.Workspace(ws.ID)
		return ok && cur.Phase == "failed"
	})
	got, ok := st.Workspace(ws.ID)
	if !ok {
		t.Fatal("workspace missing after failed setup")
	}
	if got.WorktreePath == "" {
		t.Fatal("failed setup cleared WorktreePath; user edits would be unreachable")
	}
	if got.LastError == "" {
		t.Fatal("failed setup left LastError empty")
	}
	body, err := os.ReadFile(userFile)
	if err != nil {
		t.Fatalf("user file wiped on setup failure: %v", err)
	}
	if string(body) != "keep me" {
		t.Fatalf("user file = %q, want kept", body)
	}
}

func TestDeleteWorkspaceCancelsInFlightCreate(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)
	marker := filepath.Join(dataDir, "setup.should-not")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "sleep 2 && echo no > '" + marker + "'",
	})

	ws, err := eng.BeginCreateWorkspace(proj.ID, "discard-me", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	if err := eng.DeleteWorkspace(ws.ID); err != nil {
		t.Fatalf("DeleteWorkspace during create: %v", err)
	}
	if _, ok := st.Workspace(ws.ID); ok {
		t.Fatal("deleted workspace still in store")
	}
	time.Sleep(50 * time.Millisecond)
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("setup continued after delete")
	}
}

func TestDeleteDuringCreateDoesNotTeardownMainRepo(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	torn := filepath.Join(repo, "teardown.marker")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd:    "sleep 2",
		TeardownCmd: "echo torn > teardown.marker",
	})
	ws, err := eng.BeginCreateWorkspace(proj.ID, "cancel-me", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	if err := eng.DeleteWorkspace(ws.ID); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	eng.Shutdown()
	if _, err := os.Stat(torn); err == nil {
		t.Fatal("teardown ran in the project's real checkout")
	}
}

func TestRemoveCancelsInFlightCreate(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)
	marker := filepath.Join(dataDir, "setup.after-remove")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "sleep 2 && echo no > '" + marker + "'",
	})
	ws, err := eng.BeginCreateWorkspace(proj.ID, "remove-me", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	removed, err := eng.RemoveWorkspace(ws.ID)
	if err != nil {
		t.Fatalf("RemoveWorkspace: %v", err)
	}
	if !removed.Archived {
		t.Fatal("expected Archived=true")
	}
	time.Sleep(80 * time.Millisecond)
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("setup continued after remove")
	}
	got, ok := st.Workspace(ws.ID)
	if !ok {
		t.Fatal("removed workspace missing")
	}
	if got.Phase == "editing" && got.WorktreePath != "" {
		t.Fatal("allocate finished after remove and restored the worktree")
	}
}

func TestRetryWorkspaceSetupAfterFailedCreate(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)
	marker := filepath.Join(dataDir, "setup.retry")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "exit 1",
	})

	ws, err := eng.BeginCreateWorkspace(proj.ID, "retry-me", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "create failure", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.Phase == "failed" && got.WorktreePath != ""
	})
	failed, ok := st.Workspace(ws.ID)
	if !ok {
		t.Fatal("workspace missing after failed setup")
	}
	kept := failed.WorktreePath

	proj.SetupCmd = "echo ok > '" + marker + "'"
	proj.WriteProjectFile = true
	if _, err := st.UpdateProjectRunConfig(proj); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	if err := eng.RetryWorkspaceSetup(ws.ID); err != nil {
		t.Fatalf("RetryWorkspaceSetup: %v", err)
	}
	waitFor(t, 5*time.Second, "retry setup complete", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.Phase == "editing"
	})
	got, _ := st.Workspace(ws.ID)
	if got.LastError != "" {
		t.Fatalf("LastError=%q, want empty", got.LastError)
	}
	if got.WorktreePath != kept {
		t.Fatalf("WorktreePath=%q, want kept %q", got.WorktreePath, kept)
	}
	if _, err := os.Stat(marker); err != nil {
		t.Fatalf("retry setup did not run: %v", err)
	}
}

func TestRetryWorkspaceSetupRejectsMissingWorktree(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	rec, err := st.AddWorkspace(domain.Workspace{
		ID:          "ws-no-tree",
		ProjectID:   "p1",
		Type:        domain.WorkspaceSolo,
		Phase:       "failed",
		StatusLabel: "Couldn't create",
		LastError:   "setup command failed: exit status 23",
	})
	if err != nil {
		t.Fatal(err)
	}
	err = eng.RetryWorkspaceSetup(rec.ID)
	if err == nil {
		t.Fatal("RetryWorkspaceSetup should refuse a workspace with no worktree")
	}
	if !strings.Contains(err.Error(), "the worktree was removed") {
		t.Fatalf("RetryWorkspaceSetup() error = %q, want worktree-removed", err)
	}
	got, ok := st.Workspace(rec.ID)
	if !ok {
		t.Fatal("workspace missing")
	}
	if got.Phase != "failed" || got.LastError != "setup command failed: exit status 23" {
		t.Fatalf("row mutated: Phase=%q LastError=%q", got.Phase, got.LastError)
	}
}

func TestRetryWorkspaceSetupRejectsInFlight(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "p1", Name: "p1"}); err != nil {
		t.Fatal(err)
	}
	tree := t.TempDir()
	rec, err := st.AddWorkspace(domain.Workspace{
		ID:           "ws-inflight",
		ProjectID:    "p1",
		Type:         domain.WorkspaceSolo,
		Phase:        "failed",
		StatusLabel:  "Couldn't create",
		LastError:    "setup command failed: exit status 1",
		WorktreePath: tree,
	})
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan struct{})
	close(done)
	eng.creating.Store(rec.ID, &creatingAlloc{cancel: func() {}, done: done})
	t.Cleanup(func() { eng.creating.Delete(rec.ID) })

	err = eng.RetryWorkspaceSetup(rec.ID)
	if err == nil {
		t.Fatal("RetryWorkspaceSetup should refuse while setup is in flight")
	}
	if !strings.Contains(err.Error(), "already running") {
		t.Fatalf("RetryWorkspaceSetup() error = %q, want already-running", err)
	}
	got, ok := st.Workspace(rec.ID)
	if !ok {
		t.Fatal("workspace missing")
	}
	if got.Phase != "failed" || got.LastError != "setup command failed: exit status 1" {
		t.Fatalf("row mutated: Phase=%q LastError=%q", got.Phase, got.LastError)
	}
}

func TestRetryWorkspaceSetupRejectsCopyFilesFailure(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	tree := t.TempDir()
	rec, err := st.AddWorkspace(domain.Workspace{
		ID:           "ws-copy",
		ProjectID:    "p1",
		Type:         domain.WorkspaceSolo,
		Phase:        "failed",
		StatusLabel:  "Couldn't create",
		LastError:    "copy files: glob matched a directory",
		WorktreePath: tree,
	})
	if err != nil {
		t.Fatal(err)
	}
	err = eng.RetryWorkspaceSetup(rec.ID)
	if err == nil {
		t.Fatal("RetryWorkspaceSetup should refuse a copy-files failure")
	}
	if !strings.Contains(err.Error(), "did not fail setup") {
		t.Fatalf("RetryWorkspaceSetup() error = %q, want did-not-fail-setup", err)
	}
	got, ok := st.Workspace(rec.ID)
	if !ok {
		t.Fatal("workspace missing")
	}
	if got.Phase != "failed" || got.LastError != "copy files: glob matched a directory" {
		t.Fatalf("row mutated: Phase=%q LastError=%q", got.Phase, got.LastError)
	}
}

func TestRetryWorkspaceSetupFinishesPendingFork(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)
	marker := filepath.Join(dataDir, "setup.retry-fork")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
	})
	src, err := eng.CreateWorkspace(proj.ID, "src", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if src.WorktreePath == "" {
		t.Fatal("source worktree missing")
	}
	if err := os.WriteFile(filepath.Join(src.WorktreePath, "dirty.txt"), []byte("uncommitted\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	proj.SetupCmd = "exit 1"
	proj.WriteProjectFile = true
	if _, err := st.UpdateProjectRunConfig(proj); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	dest, err := eng.BeginCreateWorkspace(proj.ID, "fork-dest", src.Branch)
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "create failure", func() bool {
		got, ok := st.Workspace(dest.ID)
		return ok && got.Phase == "failed" && got.WorktreePath != ""
	})
	failed, ok := st.Workspace(dest.ID)
	if !ok {
		t.Fatal("dest missing after failed setup")
	}
	if failed.BaseBranch != src.Branch {
		t.Fatalf("dest.BaseBranch = %q, want source branch %q before overlay", failed.BaseBranch, src.Branch)
	}
	eng.forkOverlay.Store(dest.ID, src)

	proj.SetupCmd = "echo ok > '" + marker + "'"
	if _, err := st.UpdateProjectRunConfig(proj); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	if err := eng.RetryWorkspaceSetup(dest.ID); err != nil {
		t.Fatalf("RetryWorkspaceSetup: %v", err)
	}
	waitFor(t, 5*time.Second, "retry setup complete", func() bool {
		got, ok := st.Workspace(dest.ID)
		return ok && got.Phase == "editing"
	})
	got, _ := st.Workspace(dest.ID)
	if got.BaseBranch != src.BaseBranch {
		t.Errorf("RetryWorkspaceSetup dest.BaseBranch = %q, want source merge-base %q", got.BaseBranch, src.BaseBranch)
	}
	if b, err := os.ReadFile(filepath.Join(got.WorktreePath, "dirty.txt")); err != nil || string(b) != "uncommitted\n" {
		t.Fatalf("RetryWorkspaceSetup dest missing uncommitted overlay: %s (%v)", b, err)
	}
	if _, err := os.Stat(marker); err != nil {
		t.Fatalf("retry setup did not run: %v", err)
	}
}

func TestStartRunOnFailedCreateRefuses(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "exit 1",
	})
	ws, err := eng.BeginCreateWorkspace(proj.ID, "will-fail", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "create failure", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.Phase == "failed"
	})
	if _, err := eng.StartRunOnWorkspace(ws.ID, "do the thing"); err == nil {
		t.Fatal("StartRunOnWorkspace should refuse a failed create")
	}
	if err := eng.DeleteWorkspace(ws.ID); err != nil {
		t.Fatalf("DeleteWorkspace of failed create: %v", err)
	}
}

func TestFailCreatingKeepsWorktreePathAndFailsRun(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	rec, err := st.AddWorkspace(domain.Workspace{
		ID:           "ws-fail",
		ProjectID:    "p1",
		RunStarted:   true,
		State:        domain.StateQueued,
		WorktreePath: "/tmp/recur-kept-worktree",
		Phase:        "creating",
	})
	if err != nil {
		t.Fatal(err)
	}
	eng.failCreating(rec.ID, errors.New("dispatch failed"))
	got, ok := st.Workspace(rec.ID)
	if !ok {
		t.Fatal("missing")
	}
	if got.WorktreePath != "/tmp/recur-kept-worktree" {
		t.Fatalf("WorktreePath=%q, want kept", got.WorktreePath)
	}
	if got.State != domain.StateFailed {
		t.Fatalf("State=%q, want failed", got.State)
	}
	if err := eng.checkLoopSettled(rec.ID, got); err != nil {
		t.Fatalf("failed create should be deletable: %v", err)
	}
}

func TestStartRunOnWorkspaceDoesNotStampOverFailedCreate(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	rec, err := st.AddWorkspace(domain.Workspace{
		ID:          "ws-race",
		ProjectID:   "p1",
		Type:        domain.WorkspaceSolo,
		Phase:       "failed",
		LastError:   "setup failed",
		StatusLabel: "Couldn't create",
	})
	if err != nil {
		t.Fatal(err)
	}
	// Inflight map still set — the window after failCreating's store write and
	// before creating.Delete. The stamp must not overwrite the failure.
	done := make(chan struct{})
	close(done)
	eng.creating.Store(rec.ID, &creatingAlloc{cancel: func() {}, done: done})
	t.Cleanup(func() { eng.creating.Delete(rec.ID) })

	if _, err := eng.StartRunOnWorkspace(rec.ID, "do the thing"); err == nil {
		t.Fatal("StartRunOnWorkspace should refuse a create that already failed")
	}
	got, ok := st.Workspace(rec.ID)
	if !ok {
		t.Fatal("missing")
	}
	if got.RunStarted || got.State == domain.StateQueued {
		t.Fatalf("stamped a run over a failed create: RunStarted=%v State=%q", got.RunStarted, got.State)
	}
	if got.Phase != "failed" || got.State == domain.StateQueued {
		t.Fatalf("phase=%q state=%q, want failed create left intact", got.Phase, got.State)
	}
}

func TestAllocateCopiesConfiguredFilesBeforeSetup(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)
	if err := os.MkdirAll(filepath.Join(repo, "apps", "web"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, ".env"), []byte("SECRET=1\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "apps", "web", ".env.local"), []byte("X=2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	marker := filepath.Join(dataDir, "setup.saw.env")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		CopyFiles: ".env*\n",
		SetupCmd:  "test -f .env && test -f apps/web/.env.local && echo ok > '" + marker + "'",
	})

	ws, err := eng.CreateWorkspace(proj.ID, "copy-files", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.WorktreePath == "" {
		t.Fatal("expected worktree path")
	}
	got, err := os.ReadFile(filepath.Join(ws.WorktreePath, ".env"))
	if err != nil {
		t.Fatalf("copied .env: %v", err)
	}
	if string(got) != "SECRET=1\n" {
		t.Errorf(".env = %q, want SECRET=1", got)
	}
	if _, err := os.Stat(filepath.Join(ws.WorktreePath, "apps", "web", ".env.local")); err != nil {
		t.Fatalf("copied nested .env.local: %v", err)
	}
	if _, err := os.Stat(marker); err != nil {
		t.Fatalf("setup did not see copied files: %v", err)
	}
}

func TestRemoveWorkspaceRefusesInFlightChat(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	rec, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-chat", ProjectID: "p1", Type: domain.WorkspaceSolo, Short: "hand",
	})
	if err != nil {
		t.Fatal(err)
	}
	eng.registerChatTurn(rec.ID, "PA", func() {})
	_, err = eng.RemoveWorkspace(rec.ID)
	if err == nil || !strings.Contains(err.Error(), "running") {
		t.Fatalf("RemoveWorkspace with live chat: err=%v", err)
	}
	got, ok := st.Workspace(rec.ID)
	if !ok || got.Archived {
		t.Fatal("in-flight chat should not archive the workspace")
	}
}

func TestRegisterChatTurnRefusesDeletingWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	addSoloWorkspace(t, st, "ws-del")
	eng.deleting.Store("ws-del", struct{}{})
	cancelled := false
	if turn := eng.registerChatTurn("ws-del", "PA", func() { cancelled = true }); turn != nil {
		t.Fatal("registerChatTurn should refuse a workspace being removed")
	}
	if cancelled {
		t.Fatal("pre-swap refuse should not invoke cancel")
	}
	if eng.workspaceHasLiveChat("ws-del") {
		t.Fatal("refused register must not leave a live chat")
	}
	eng.deleting.Delete("ws-del")
	if eng.registerChatTurn("ws-del", "PA", func() {}) == nil {
		t.Fatal("registerChatTurn should succeed after the tombstone is cleared")
	}
}

func TestRegisterChatTurnRefusesArchivedWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-arch", ProjectID: "p1", Type: domain.WorkspaceSolo, Short: "hand", Archived: true,
	}); err != nil {
		t.Fatal(err)
	}
	if eng.registerChatTurn("ws-arch", "PA", func() {}) != nil {
		t.Fatal("registerChatTurn should refuse an archived workspace")
	}
}

func TestRegisterChatTurnRefusesAfterRemoveWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	addSoloWorkspace(t, st, "ws-gone")
	if _, err := eng.RemoveWorkspace("ws-gone"); err != nil {
		t.Fatalf("RemoveWorkspace: %v", err)
	}
	if turn := eng.registerChatTurn("ws-gone", "PA", func() {}); turn != nil {
		t.Fatal("registerChatTurn after RemoveWorkspace must return nil")
	}
	if eng.workspaceHasLiveChat("ws-gone") {
		t.Fatal("removed workspace must not keep a live chat")
	}
}

func TestBeginStartWorkspaceAttachmentErrorRollsBack(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	projID := addProject(t, st, initRepo(t))
	before := len(st.Workspaces())
	raw := make([]byte, maxChatAttachmentBytes+1)
	_, err := eng.BeginStartWorkspaceWith(projID, "look", "", "", "", "", "", nil, []domain.ChatAttachment{{
		ID: "huge", Name: "huge.png", Mime: "image/png", Data: base64.StdEncoding.EncodeToString(raw),
	}})
	if err == nil {
		t.Fatal("BeginStartWorkspaceWith(too-large attachment) error = nil, want error")
	}
	if got := len(st.Workspaces()); got != before {
		t.Fatalf("Workspaces after failed attachments = %d, want %d (rolled back)", got, before)
	}
	if !eng.gov.TryAcquireLoop() {
		t.Fatal("governor slot leaked after attachment rollback")
	}
	eng.gov.ReleaseLoop()
}

func TestBeginStartWorkspaceWithSlotIsCreating(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	projID := addProject(t, st, initRepo(t))
	ws, acquired, err := eng.queueStartWorkspace(projID, "ship it", "", "", domain.AutonomyBalanced, "", "", nil)
	if err != nil {
		t.Fatal(err)
	}
	if !acquired {
		t.Fatal("queueStartWorkspace: expected a loop slot")
	}
	t.Cleanup(eng.gov.ReleaseLoop)
	if ws.Phase != phaseCreating {
		t.Fatalf("queueStartWorkspace() phase = %q, want %q (slot acquired; worktree not ready yet)", ws.Phase, phaseCreating)
	}
	if ws.StatusLabel != statusCreating {
		t.Fatalf("queueStartWorkspace() statusLabel = %q, want %q", ws.StatusLabel, statusCreating)
	}
}

func TestBeginStartWorkspaceWithoutSlotWaits(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	projID := addProject(t, st, initRepo(t))
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	ws, err := eng.BeginStartWorkspace(projID, "ship it", "", domain.AutonomyBalanced, nil)
	if err != nil {
		t.Fatal(err)
	}
	if ws.Phase != "waiting for a slot" {
		t.Fatalf("phase=%q, want waiting for a slot", ws.Phase)
	}
}
