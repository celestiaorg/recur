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

// SetWorkspaceBaseBranch changes the branch this workspace's diffs, PRs, and
// "update with base" compare against. It does not rebase or merge the worktree —
// that stays the separate Update-with-base action. An open (non-merged) PR is
// retargeted to match; a GitHub PR is retargeted via `gh pr edit --base` first so
// Recur and GitHub stay in sync. The main working copy has no merge base.
func (e *Engine) SetWorkspaceBaseBranch(workspaceID, baseBranch string) (domain.Workspace, error) {
	baseBranch = strings.TrimSpace(baseBranch)
	if baseBranch == "" {
		return domain.Workspace{}, fmt.Errorf("base branch is required")
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if ws.IsMain {
		return domain.Workspace{}, fmt.Errorf("the main working copy has no merge base — it tracks the checkout as-is")
	}
	if ws.BaseBranch == baseBranch {
		return ws, nil
	}
	if ws.Branch != "" && baseBranch == ws.Branch {
		return domain.Workspace{}, fmt.Errorf("target branch cannot be the workspace's own branch %q", ws.Branch)
	}
	proj, ok := e.Store.Project(ws.ProjectID)
	if !ok || proj.RepoPath == "" {
		return domain.Workspace{}, fmt.Errorf("project for workspace %q not found", workspaceID)
	}

	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if !mgr.BaseBranchExists(ctx, baseBranch) {
		return domain.Workspace{}, fmt.Errorf("branch %q does not exist (or has no commits)", baseBranch)
	}

	pr, hasPR := e.Store.PRByWorkspace(workspaceID)
	openPR := hasPR && pr.State != "Merged" && pr.Group != "merged"
	if openPR && pr.Source == domain.SourceGitHub {
		if err := retargetGitHubPR(ctx, proj.RepoPath, pr.Number, baseBranch); err != nil {
			return domain.Workspace{}, fmt.Errorf("retarget GitHub PR: %w", err)
		}
	}

	updated, ok := e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
		w.BaseBranch = baseBranch
	})
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found", workspaceID)
	}

	if openPR {
		e.retargetOpenPR(mgr, updated, pr.ID, baseBranch)
	}

	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	e.Bus.Publish(Event{Name: "workspace.fs_changed", Data: map[string]any{
		"workspaceId": workspaceID,
	}})
	e.emitWorkspaceEvent(workspaceID, "AR", "You",
		fmt.Sprintf("Target branch is now <code>%s</code> — diffs and PRs compare against it.", html.EscapeString(baseBranch)))
	return updated, nil
}

// retargetOpenPR points an open PR at the new base and refreshes its snapshot
// (diff / commits / behind-count) so the PR surface matches the new comparison.
// Snapshot fields are only overwritten when the corresponding git call succeeds,
// so a transient failure cannot blank the PR view. Git ops get their own timeout
// — the caller's ctx may already be spent on validation / `gh pr edit`.
func (e *Engine) retargetOpenPR(mgr *gitpkg.Manager, ws domain.Workspace, prID, base string) {
	var files []domain.FileDiff
	var add, del int
	var commits []domain.Commit
	haveDiff, haveCommits := false, false
	if ws.WorktreePath != "" {
		gitCtx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		if diff, err := mgr.DiffPR(gitCtx, ws.WorktreePath, base); err == nil {
			files, add, del = diff.Files, diff.Add, diff.Del
			haveDiff = true
		}
		if cs, err := mgr.LogCommits(gitCtx, ws.WorktreePath, base); err == nil {
			commits = cs
			haveCommits = true
		}
	}
	if _, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Base = base
		if haveDiff {
			p.Files = files
			p.Diff = domain.DiffStat{Add: add, Del: del}
			p.FileCount = len(files)
		}
		if haveCommits {
			p.Commits = commits
		}
		touchPRUpdated(p)
	}); !ok {
		return
	}
	e.RefreshPRBehind(prID)
	e.publishPRUpdated(prID)
}

// retargetGitHubPR changes an open GitHub PR's base via `gh pr edit --base`.
// GitHub wants the short name (main); the workspace may store origin/main.
func retargetGitHubPR(ctx context.Context, repoPath string, number int, base string) error {
	if !ghCLIAvailable() {
		return fmt.Errorf("gh CLI is not installed or not authenticated")
	}
	_, err := runGH(ctx, repoPath, "pr", "edit", fmt.Sprintf("%d", number), "--base", githubPRBase(ctx, repoPath, base))
	return err
}
