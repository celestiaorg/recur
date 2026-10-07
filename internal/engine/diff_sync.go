package engine

import (
	"context"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// diffSyncDelay coalesces bursty FSEvents (agent edits) into one numstat refresh
// so the sidebar +N/−M stays live without shelling git on every file write.
const diffSyncDelay = 600 * time.Millisecond

// diffRefreshParallel is how many workspace numstat shells run at once during a
// full refresh. Bounded so a large project list cannot fork dozens of gits.
const diffRefreshParallel = 4

// scheduleWorkspaceDiffSync debounces a live DiffStatVsBase refresh + publish for
// the watched workspace (fswatch path).
func (e *Engine) scheduleWorkspaceDiffSync(workspaceID string) {
	if workspaceID == "" {
		return
	}
	e.diffSyncMu.Lock()
	defer e.diffSyncMu.Unlock()
	if e.diffSyncTimers == nil {
		e.diffSyncTimers = map[string]*time.Timer{}
	}
	if t, ok := e.diffSyncTimers[workspaceID]; ok {
		t.Stop()
	}
	id := workspaceID
	var timer *time.Timer
	timer = time.AfterFunc(diffSyncDelay, func() {
		e.syncWorkspaceDiff(id, true)
		e.diffSyncMu.Lock()
		// Only clear our own slot — a newer schedule may have replaced the timer
		// while this sync was running.
		if e.diffSyncTimers[id] == timer {
			delete(e.diffSyncTimers, id)
		}
		e.diffSyncMu.Unlock()
	})
	e.diffSyncTimers[id] = timer
}

// ScheduleRefreshAllWorkspaceDiffs kicks off a background refresh of every
// workspace DiffStat and returns immediately. Concurrent callers coalesce via
// single-flight (+ one trailing rerun if another request arrived mid-flight) so
// ListWorkspaces never blocks on git and never fans out duplicate process storms.
func (e *Engine) ScheduleRefreshAllWorkspaceDiffs() {
	e.diffRefreshAllMu.Lock()
	defer e.diffRefreshAllMu.Unlock()
	if e.diffRefreshAllBusy {
		e.diffRefreshAllPending = true
		return
	}
	e.diffRefreshAllBusy = true
	go e.refreshAllWorkspaceDiffsLoop()
}

func (e *Engine) refreshAllWorkspaceDiffsLoop() {
	for {
		e.RefreshAllWorkspaceDiffs()
		e.diffRefreshAllMu.Lock()
		if !e.diffRefreshAllPending {
			e.diffRefreshAllBusy = false
			e.diffRefreshAllMu.Unlock()
			return
		}
		e.diffRefreshAllPending = false
		e.diffRefreshAllMu.Unlock()
	}
}

// RefreshAllWorkspaceDiffs recomputes Diff for every workspace that has a real
// worktree, in parallel (bounded), and publishes workspace.updated when a stat
// changes so an async ListWorkspaces refresh paints the sidebar. Seeded demo
// rows without on-disk worktrees are left unchanged.
func (e *Engine) RefreshAllWorkspaceDiffs() {
	workspaces := e.Store.Workspaces()
	if len(workspaces) == 0 {
		return
	}
	sem := make(chan struct{}, diffRefreshParallel)
	var wg sync.WaitGroup
	for _, ws := range workspaces {
		if ws.IsPRChatHost() {
			continue
		}
		id := ws.ID
		wg.Add(1)
		sem <- struct{}{}
		go func() {
			defer wg.Done()
			defer func() { <-sem }()
			e.syncWorkspaceDiff(id, true)
		}()
	}
	wg.Wait()
}

// syncWorkspaceDiff sets Workspace.Diff from `git diff --numstat` vs the
// workspace base (or HEAD for main). When publish is true and the stat changed,
// emits workspace.updated so the sidebar paints live. Returns true when git
// produced a usable stat (even if unchanged), or when a newer sync superseded
// this one (caller should not fall back to cumulative accounting).
func (e *Engine) syncWorkspaceDiff(workspaceID string, publish bool) bool {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok || ws.IsPRChatHost() || ws.WorktreePath == "" || !gitpkg.IsRepo(ws.WorktreePath) {
		return false
	}
	base := strings.TrimSpace(ws.BaseBranch)
	if base == "" && !ws.IsMain {
		if p, pok := e.Store.Project(ws.ProjectID); pok {
			base = strings.TrimSpace(p.BaseBranch)
		}
	}

	gen := e.beginDiffSync(workspaceID)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	add, del, err := gitpkg.DiffStatVsBase(ctx, ws.WorktreePath, base)
	if err != nil {
		return false
	}
	behind := 0
	if !ws.IsMain && base != "" {
		mgr := gitpkg.NewManager(ws.WorktreePath, "")
		if n, berr := mgr.CommitsBehind(ctx, ws.WorktreePath, base); berr == nil {
			behind = n
		}
	}

	// Serialize apply per workspace so a slow older git cannot overwrite a
	// fresher write that already landed (generation checked under the same lock).
	apply := e.diffApplyLock(workspaceID)
	apply.Lock()
	defer apply.Unlock()
	if e.diffSyncStale(workspaceID, gen) {
		return true
	}
	cur, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return false
	}
	if cur.Diff.Add == add && cur.Diff.Del == del && cur.BehindBase == behind {
		return true
	}
	updated, ok := e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
		w.Diff = domain.DiffStat{Add: add, Del: del}
		w.BehindBase = behind
	})
	if !ok {
		return true
	}
	if publish {
		e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	}
	return true
}

func (e *Engine) beginDiffSync(workspaceID string) uint64 {
	e.diffSyncMu.Lock()
	defer e.diffSyncMu.Unlock()
	if e.diffSyncGen == nil {
		e.diffSyncGen = map[string]uint64{}
	}
	e.diffSyncGen[workspaceID]++
	return e.diffSyncGen[workspaceID]
}

func (e *Engine) diffSyncStale(workspaceID string, gen uint64) bool {
	e.diffSyncMu.Lock()
	defer e.diffSyncMu.Unlock()
	return e.diffSyncGen[workspaceID] != gen
}

func (e *Engine) diffApplyLock(workspaceID string) *sync.Mutex {
	e.diffSyncMu.Lock()
	defer e.diffSyncMu.Unlock()
	if e.diffApplyMu == nil {
		e.diffApplyMu = map[string]*sync.Mutex{}
	}
	m, ok := e.diffApplyMu[workspaceID]
	if !ok {
		m = &sync.Mutex{}
		e.diffApplyMu[workspaceID] = m
	}
	return m
}
