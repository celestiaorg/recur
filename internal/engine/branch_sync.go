package engine

import (
	"context"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// syncWorkspaceBranch reads the live worktree branch and, when it differs from
// the cached Workspace.Branch, updates the store, the in-memory Loop (if any),
// and any open Local PR. GitHub PR.Branch is left alone — it is the remote head
// frozen at create time so later pushes keep updating the open GH PR after a
// local rename. Returns the branch name callers should use for git ops (live when
// known, else the cached value).
func (e *Engine) syncWorkspaceBranch(workspaceID string) string {
	e.wsUpstreamCache.Delete(workspaceID)
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return ""
	}
	if ws.WorktreePath == "" {
		return ws.Branch
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	live, err := gitpkg.CurrentBranch(ctx, ws.WorktreePath)
	if err != nil || live == "" {
		return ws.Branch
	}
	if live == ws.Branch {
		return live
	}

	updated, ok := e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
		w.Branch = live
	})
	if !ok {
		return live
	}

	if v, loaded := e.loops.Load(workspaceID); loaded {
		if rl, ok := v.(*runningLoop); ok && rl.loop != nil {
			rl.loop.Branch = live
		}
	}

	if pr, ok := e.Store.PRByWorkspace(workspaceID); ok &&
		pr.State != "Merged" && pr.Source == domain.SourceLocal && pr.Branch != live {
		if _, uok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) { p.Branch = live }); uok {
			e.publishPRUpdated(pr.ID)
		}
	}

	// Paint crumbs/titlebar live — publishLoopUpdated now carries branch.
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	e.linkWorkspaceToMatchingPRs(updated)
	return live
}
