package engine

import (
	"context"
	"fmt"
	"html"
	"strings"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// FetchWorkingCopyUpstream refreshes the remote tip the titlebar compares
// against: the main working copy's tracking branch, a workspace's merge base
// (whatever BaseBranch is — origin/develop, main, …), and an open GitHub PR
// head. Best-effort — never an error, so a hung or offline fetch cannot toast
// on workspace open.
func (e *Engine) FetchWorkingCopyUpstream(workspaceID string) domain.WorkspaceLandState {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok || ws.WorktreePath == "" || !gitpkg.IsRepo(ws.WorktreePath) {
		return e.WorkspaceLandState(workspaceID)
	}
	if ws.IsMain {
		mgr := gitpkg.NewManager(ws.WorktreePath, e.worktreeRoot)
		ctx, cancel := context.WithTimeout(context.Background(), gitpkg.RemoteFetchTimeout)
		defer cancel()
		if up, uok := mgr.Upstream(ctx, ws.WorktreePath); uok {
			_ = mgr.FetchRemoteBranch(ctx, up.Remote, up.Branch)
		}
		return e.WorkspaceLandState(workspaceID)
	}
	e.fetchWorkspaceMergeBase(ws)
	e.fetchOpenPRRemote(ws)
	return e.WorkspaceLandState(workspaceID)
}

func (e *Engine) fetchWorkspaceMergeBase(ws domain.Workspace) {
	proj, ok := e.Store.Project(ws.ProjectID)
	if !ok || proj.RepoPath == "" {
		return
	}
	base := strings.TrimSpace(ws.BaseBranch)
	if base == "" {
		base = strings.TrimSpace(proj.BaseBranch)
	}
	if base == "" {
		return
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), gitpkg.RemoteFetchTimeout)
	defer cancel()
	_ = mgr.FetchCompareRef(ctx, base)
}

func (e *Engine) fetchOpenPRRemote(ws domain.Workspace) {
	pr, ok := e.Store.PRByWorkspace(ws.ID)
	if !ok || pr.Source != domain.SourceGitHub || pr.State == "Merged" || pr.Group == "merged" {
		return
	}
	_, remote, short := githubPRTrackingRef(pr.Branch)
	if remote == "" || short == "" {
		return
	}
	proj, pok := e.Store.Project(ws.ProjectID)
	if !pok || proj.RepoPath == "" {
		return
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), gitpkg.RemoteFetchTimeout)
	defer cancel()
	_ = mgr.FetchRemoteBranch(ctx, remote, short)
}

// UpdateWorkingCopyFromRemote fetches the main working copy's upstream and
// merges or rebases it into HEAD (project/settings UpdateBaseMethod) with
// --autostash so leftover uncommitted edits survive. On conflict it aborts so
// the real checkout is left as it was and the caller gets an error — unlike
// UpdateWorkspaceWithBase, which leaves markers for /update. An update the
// user already started (MERGE_HEAD / REBASE_HEAD) is refused rather than
// aborted. If the merge/rebase succeeds but autostash cannot reapply dirty
// edits, Status is autostash so the UI can point at `git stash pop`. Refused
// on non-main workspaces.
func (e *Engine) UpdateWorkingCopyFromRemote(workspaceID string, method domain.UpdateBaseMethod) (domain.UpdateWorkspaceResult, error) {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if !ws.IsMain {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("update from remote is only for the main working copy")
	}
	if ws.WorktreePath == "" || !gitpkg.IsRepo(ws.WorktreePath) {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("this folder is not a git checkout")
	}
	mgr := gitpkg.NewManager(ws.WorktreePath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	up, ok := mgr.Upstream(ctx, ws.WorktreePath)
	if !ok {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("this branch is not tracking a remote")
	}
	fetchErr := mgr.FetchRemoteBranch(ctx, up.Remote, up.Branch)

	proj, _ := e.Store.Project(ws.ProjectID)
	if method == "" {
		method = domain.EffectiveUpdateBaseMethod(proj, e.settings())
	} else {
		method = domain.NormalizeUpdateBaseMethod(string(method))
	}
	if mgr.UpdateInProgress(ctx, ws.WorktreePath) {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("a merge or rebase is already in progress — finish or abort it before updating")
	}

	behind, _ := mgr.CommitsBehind(ctx, ws.WorktreePath, up.Ref)
	if behind == 0 {
		if fetchErr != nil {
			return domain.UpdateWorkspaceResult{}, fmt.Errorf("fetch %s: %w", up.Ref, fetchErr)
		}
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceNoop, Workspace: ws, Base: up.Ref}, nil
	}

	stashBefore := mgr.StashTip(ctx, ws.WorktreePath)
	err := mgr.BeginUpdateWithBaseAutostash(ctx, ws.WorktreePath, up.Ref, string(method))
	if err == nil {
		status := domain.UpdateWorkspaceUpdated
		event := fmt.Sprintf(
			"Updated with latest <code>%s</code> (%s).",
			html.EscapeString(up.Ref), html.EscapeString(domain.UpdateBaseMethodLabel(method)))
		if mgr.AutostashLeftInStash(ctx, ws.WorktreePath, stashBefore) {
			if rerr := mgr.ResetWorktreeToHead(ctx, ws.WorktreePath); rerr != nil {
				return domain.UpdateWorkspaceResult{}, fmt.Errorf("updated from %s but leftover edits are in git stash and the working tree could not be cleaned: %w", up.Ref, rerr)
			}
			status = domain.UpdateWorkspaceAutostash
			event = fmt.Sprintf(
				"Updated with latest <code>%s</code> (%s), but uncommitted edits could not be reapplied and were left in git stash.",
				html.EscapeString(up.Ref), html.EscapeString(domain.UpdateBaseMethodLabel(method)))
		}
		e.syncWorkspaceDiff(workspaceID, true)
		e.emitWorkspaceEvent(workspaceID, "AR", "You", event)
		ws, _ = e.Store.Workspace(workspaceID)
		return domain.UpdateWorkspaceResult{Status: status, Workspace: ws, Base: up.Ref}, nil
	}
	if mgr.UpdateInProgress(ctx, ws.WorktreePath) {
		if aerr := mgr.AbortInProgressUpdate(ctx, ws.WorktreePath); aerr != nil {
			return domain.UpdateWorkspaceResult{}, fmt.Errorf("update from %s hit conflicts and could not abort: %w", up.Ref, aerr)
		}
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("incoming changes on %s conflict with your branch — your checkout was left unchanged", up.Ref)
	}
	return domain.UpdateWorkspaceResult{}, fmt.Errorf("update from %s: %w", up.Ref, err)
}

// PushWorkingCopy pushes the main working copy's HEAD to its tracking remote.
// Uncommitted files are left alone — a working copy is not one body of work, so
// leftover untracked or modified files must not block pushing already-committed
// commits. Unmerged / behind checkouts are still refused.
func (e *Engine) PushWorkingCopy(workspaceID string) (domain.Workspace, error) {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if !ws.IsMain {
		return domain.Workspace{}, fmt.Errorf("push is only for the main working copy")
	}
	if ws.WorktreePath == "" || !gitpkg.IsRepo(ws.WorktreePath) {
		return domain.Workspace{}, fmt.Errorf("this folder is not a git checkout")
	}
	mgr := gitpkg.NewManager(ws.WorktreePath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	up, ok := mgr.Upstream(ctx, ws.WorktreePath)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("this branch is not tracking a remote")
	}
	if gitpkg.InspectDiscardRisk(ctx, ws.WorktreePath).Unmerged {
		return domain.Workspace{}, fmt.Errorf("resolve merge conflicts first")
	}
	_ = mgr.FetchRemoteBranch(ctx, up.Remote, up.Branch)
	if behind, err := mgr.CommitsBehind(ctx, ws.WorktreePath, up.Ref); err == nil && behind > 0 {
		return domain.Workspace{}, fmt.Errorf("this branch is %d commits behind %s — update first", behind, up.Ref)
	}
	ahead, _ := mgr.CommitsAhead(ctx, ws.WorktreePath, up.Ref)
	if ahead == 0 {
		return domain.Workspace{}, fmt.Errorf("already up to date with %s", up.Ref)
	}
	if err := mgr.PushUpstream(ctx, ws.WorktreePath); err != nil {
		return domain.Workspace{}, err
	}
	e.emitWorkspaceEvent(workspaceID, "AR", "You", fmt.Sprintf(
		"Pushed to <code>%s</code>.", html.EscapeString(up.Remote)))
	e.syncWorkspaceDiff(workspaceID, true)
	ws, ok = e.Store.Workspace(workspaceID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found after push", workspaceID)
	}
	return ws, nil
}
