package engine

import (
	"context"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"time"

	"recur/internal/copyfiles"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/runconfig"
	"recur/internal/store"
)

// defaultWorkspaceTitle is the Goal/Short for a newly created hand-driven
// workspace before the agent chat has a first user message to name it from.
const defaultWorkspaceTitle = "New Workspace"

const (
	phaseCreating  = "creating"
	statusCreating = "Creating"
	allocWait      = 30 * time.Second
	worktreeAddTO  = 60 * time.Second
)

// creatingAlloc is one in-flight git worktree + setup for BeginCreateWorkspace.
// cancel stops git/setup; done closes when the goroutine has fully exited.
type creatingAlloc struct {
	cancel context.CancelFunc
	done   chan struct{}
}

// CreateWorkspace creates a workspace without engaging the autopilot: a
// worktree + branch the human (and its chat tabs, in any mode) drives. The
// autopilot can be engaged later from the chat dock's Loop mode
// (StartRunOnWorkspace). Returns the stored record after the worktree (and
// setup command) have finished — tests and internal callers that immediately
// read/write the tree use this path.
// An empty name becomes "New Workspace"; the coding-agent chat renames it from
// the first user message (see maybeAutoNameWorkspace).
func (e *Engine) CreateWorkspace(projectID, name, baseBranch string) (domain.Workspace, error) {
	rec, err := e.persistCreatingWorkspace(projectID, name, baseBranch)
	if err != nil {
		return domain.Workspace{}, err
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if err := e.allocateWorkspaceWorktree(ctx, rec); err != nil {
		e.rollbackCreating(rec.ID)
		return domain.Workspace{}, err
	}
	e.maybeDispatchAfterAllocate(rec.ID)
	saved, ok := e.Store.Workspace(rec.ID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q missing after create", rec.ID)
	}
	return saved, nil
}

// BeginCreateWorkspace persists the workspace immediately and allocates the
// git worktree + setup command in the background. The returned record has an
// empty WorktreePath and phase "creating". workspace.updated carries the path
// as soon as `git worktree add` finishes (Observatory can load) and again
// when setup completes (phase "editing") or fails (lastError). The UI
// navigates without blocking the rest of the app on git or setup_cmd.
func (e *Engine) BeginCreateWorkspace(projectID, name, baseBranch string) (domain.Workspace, error) {
	rec, err := e.persistCreatingWorkspace(projectID, name, baseBranch)
	if err != nil {
		return domain.Workspace{}, err
	}
	e.startAllocate(rec)
	return rec, nil
}

// BeginStartWorkspace is the UI path for mode=loop: persist the loop record
// (and take a slot if one is free) then dispatch in the background so the
// New-workspace modal can close immediately. Engine.StartWorkspace stays
// synchronous for tests. simplify is the New Workspace chip (nil → Settings /
// inherit).
func (e *Engine) BeginStartWorkspace(projectID, task, baseBranch string, autonomy domain.AutonomyLevel, simplify *bool) (domain.Workspace, error) {
	return e.BeginStartWorkspaceWith(projectID, task, "", baseBranch, autonomy, "", "", simplify, nil)
}

// BeginStartWorkspaceWith is BeginStartWorkspace plus the bundle loop and policy.
// goalDisplay is the short UI label when task is expanded prompt-action wire;
// empty means task is already human-facing. attachments are opening image pastes
// so the planner (and other specialists) can see them.
func (e *Engine) BeginStartWorkspaceWith(projectID, task, goalDisplay, baseBranch string, autonomy domain.AutonomyLevel, loopID, policyID string, simplify *bool, attachments []domain.ChatAttachment) (domain.Workspace, error) {
	if strings.TrimSpace(loopID) != "" || strings.TrimSpace(policyID) != "" {
		if _, reason := e.resolveGuard(loopID, policyID, autonomy); reason != "" {
			return domain.Workspace{}, fmt.Errorf("%s", reason)
		}
	}
	rec, acquired, err := e.queueStartWorkspace(projectID, task, goalDisplay, baseBranch, autonomy, loopID, policyID, simplify)
	if err != nil {
		return domain.Workspace{}, err
	}
	var imgs []UserImage
	if len(attachments) > 0 {
		var attErr error
		_, imgs, attErr = prepareChatAttachments(rec.ID, task, attachments)
		if attErr != nil {
			e.abortQueuedStart(rec, acquired)
			return domain.Workspace{}, fmt.Errorf("I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP.")
		}
	}
	e.stampLoopGoalImages(rec.ID, maestroAgentCode, imgs)
	if !acquired {
		return rec, nil
	}
	go func() {
		if err := e.dispatch(rec); err != nil {
			e.gov.ReleaseLoop()
			if !benignDispatchErr(err) {
				e.failCreating(rec.ID, err)
			}
		}
	}()
	return rec, nil
}

func (e *Engine) queueStartWorkspace(projectID, task, goalDisplay, baseBranch string, autonomy domain.AutonomyLevel, loopID, policyID string, simplify *bool) (domain.Workspace, bool, error) {
	proj, ok := e.Store.Project(projectID)
	if !ok {
		return domain.Workspace{}, false, fmt.Errorf("project %q not found", projectID)
	}
	if !gitpkg.IsRepo(proj.RepoPath) {
		return domain.Workspace{}, false, fmt.Errorf("project %q is not a git repository — workspaces need a git checkout", projectID)
	}
	if baseBranch == "" {
		baseBranch = proj.BaseBranch
	}
	if baseBranch == "" {
		baseBranch = "main"
	}
	if autonomy == "" {
		autonomy = e.Store.Policy().Autonomy
	}
	switch autonomy {
	case domain.AutonomyCautious, domain.AutonomyBalanced, domain.AutonomyAggressive:
	default:
		autonomy = domain.AutonomyBalanced
	}
	if _, reason := e.resolveGuard(loopID, policyID, autonomy); reason != "" {
		return domain.Workspace{}, false, fmt.Errorf("%s", reason)
	}

	mgr := e.gitManager(proj.RepoPath)
	e.idMu.Lock()
	id := e.mintWorkspaceID(projectID, mgr)
	rawTask := strings.TrimSpace(task)
	task = stripAttachmentMarkers(rawTask)
	if task == "" && rawTask != "" {
		task = "image attachment"
	}
	display := stripAttachmentMarkers(strings.TrimSpace(goalDisplay))
	if display == strings.TrimSpace(task) {
		display = ""
	}
	shortSrc := task
	if display != "" {
		shortSrc = display
	}
	rec := domain.Workspace{
		ID:           id,
		ProjectID:    projectID,
		RunStarted:   true,
		Group:        domain.GroupActive,
		Goal:         task,
		GoalDisplay:  display,
		Short:        truncate(shortSrc, 48),
		State:        domain.StateQueued,
		StatusLabel:  "Queued",
		Phase:        "waiting for a slot",
		Iteration:    0,
		Progress:     0,
		Autonomy:     autonomy,
		LoopID:       strings.TrimSpace(loopID),
		PolicyID:     strings.TrimSpace(policyID),
		LoopChatCode: maestroAgentCode,
		Branch:       e.workspaceBranch(gitpkg.WorkspaceSlug(id)),
		BaseBranch:   baseBranch,
		AgentCodes:   []string{"MA"},
		Last:         "queued",
		ElapsedLabel: "just now",
	}

	acquired := e.gov.TryAcquireLoop()
	if acquired {
		// Slot is ours; git/setup is the blocker. Leaving phase as
		// "waiting for a slot" makes the chat bubbles lie until dispatch
		// finishes worktree add.
		rec.Phase = phaseCreating
		rec.StatusLabel = statusCreating
		rec.Last = "creating"
	}
	if _, err := e.Store.AddWorkspace(rec); err != nil {
		e.idMu.Unlock()
		if acquired {
			e.gov.ReleaseLoop()
		}
		return domain.Workspace{}, false, err
	}
	e.idMu.Unlock()
	e.stampMaestroSimplifyIfUnset(rec.ID)
	if simplify != nil {
		e.rememberChatSimplify(rec.ID, maestroAgentCode, *simplify)
	}
	if !acquired {
		e.publishLoopUpdated(rec.ID, rec.State, rec.Phase, rec.Iteration)
		e.publishResource()
		return rec, false, nil
	}
	return rec, true, nil
}

func (e *Engine) abortQueuedStart(rec domain.Workspace, acquired bool) {
	e.rollbackCreating(rec.ID)
	if acquired {
		e.gov.ReleaseLoop()
	}
	if rec.ID != "" {
		if root, err := store.DataDir(); err == nil {
			_ = os.RemoveAll(filepath.Join(root, "workspaces", rec.ID))
		}
	}
}

func (e *Engine) persistCreatingWorkspace(projectID, name, baseBranch string) (domain.Workspace, error) {
	proj, ok := e.Store.Project(projectID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("project %q not found", projectID)
	}
	if proj.RepoPath == "" {
		return domain.Workspace{}, fmt.Errorf("project %q has no repo path", projectID)
	}
	if !gitpkg.IsRepo(proj.RepoPath) {
		return domain.Workspace{}, fmt.Errorf("project %q is not a git repository — workspaces need a git checkout", projectID)
	}
	base := baseBranch
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		base = "main"
	}

	name = strings.TrimSpace(name)
	if name == "" {
		name = defaultWorkspaceTitle
	}

	mgr := e.gitManager(proj.RepoPath)
	e.idMu.Lock()
	id := e.mintWorkspaceID(projectID, mgr)
	rec := domain.Workspace{
		ID:        id,
		ProjectID: projectID,
		// Type marks creation origin for OLDER builds reading this database; the
		// current code branches on HasRun, never on Type.
		Type:         domain.WorkspaceSolo,
		Group:        domain.GroupActive,
		Goal:         name,
		Short:        truncate(name, 48),
		StatusLabel:  statusCreating,
		Phase:        phaseCreating,
		Branch:       e.workspaceBranch(gitpkg.WorkspaceSlug(id)),
		BaseBranch:   base,
		Last:         "creating",
		ElapsedLabel: "just now",
	}
	saved, err := e.Store.AddWorkspace(rec)
	e.idMu.Unlock()
	if err != nil {
		return domain.Workspace{}, err
	}
	e.publishLoopUpdated(saved.ID, saved.State, saved.Phase, saved.Iteration)
	return saved, nil
}

func (e *Engine) startAllocate(rec domain.Workspace) {
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	e.creating.Store(rec.ID, &creatingAlloc{cancel: cancel, done: done})
	go func() {
		defer func() {
			cancel()
			e.creating.Delete(rec.ID)
			close(done)
		}()
		err := e.allocateWorkspaceWorktree(ctx, rec)
		if err != nil {
			if ctx.Err() != nil {
				e.cleanupPartialWorktree(rec)
				return
			}
			e.failCreating(rec.ID, err)
			return
		}
		// Drop the inflight mark before maybeDispatch so a StartRun that races
		// this tail self-dispatches. If the entry is still present after
		// maybeDispatch has already seen !HasRun, the run stays queued forever.
		e.creating.Delete(rec.ID)
		e.maybeDispatchAfterAllocate(rec.ID)
	}()
}

// RetryWorkspaceSetup re-runs setup_cmd on a hand workspace whose create
// failed after the worktree landed. The tree is kept; an edited project
// setup_cmd is picked up. Loop-start failures that deleted the tree must
// be removed and created again.
func (e *Engine) RetryWorkspaceSetup(id string) error {
	ws, ok := e.Store.Workspace(id)
	if !ok {
		return fmt.Errorf("workspace %q not found", id)
	}
	if ws.IsMain || ws.IsPRChatHost() {
		return fmt.Errorf("retry setup only applies to a hand workspace")
	}
	if ws.HasRun() {
		return fmt.Errorf("retry setup only applies to a workspace that never started a loop")
	}
	if ws.Phase != "failed" && ws.StatusLabel != "Couldn't create" {
		return fmt.Errorf("this workspace did not fail setup")
	}
	// copyProjectFiles failures also keep the tree; retry does not re-copy.
	if !strings.HasPrefix(ws.LastError, "setup command failed") {
		return fmt.Errorf("this workspace did not fail setup")
	}
	if ws.WorktreePath == "" {
		return fmt.Errorf("the worktree was removed — remove this workspace and create it again")
	}
	if fi, err := os.Stat(ws.WorktreePath); err != nil || !fi.IsDir() {
		return fmt.Errorf("the worktree was removed — remove this workspace and create it again")
	}
	proj, ok := e.Store.Project(ws.ProjectID)
	if !ok {
		return fmt.Errorf("project %q not found", ws.ProjectID)
	}

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	if _, loaded := e.creating.LoadOrStore(id, &creatingAlloc{cancel: cancel, done: done}); loaded {
		cancel()
		return fmt.Errorf("workspace setup is already running")
	}
	// Install first so Remove/Delete's cancelCreating can wait. Recheck
	// deleting after: a reservation can land between the workspace read
	// and LoadOrStore. Close done on every abort so waitAlloc cannot hang.
	release := func() {
		cancel()
		e.creating.Delete(id)
		close(done)
	}
	if _, busy := e.deleting.Load(id); busy {
		release()
		return fmt.Errorf("workspace %q is being removed", id)
	}

	reset, ok := e.Store.UpdateWorkspace(id, func(w *domain.Workspace) {
		w.Phase = phaseCreating
		w.StatusLabel = statusCreating
		w.Last = "creating"
		w.LastError = ""
	})
	if !ok {
		release()
		return fmt.Errorf("workspace %q not found", id)
	}
	e.publishLoopUpdated(reset.ID, reset.State, reset.Phase, reset.Iteration)

	go func() {
		defer func() {
			cancel()
			e.creating.Delete(id)
			close(done)
		}()
		if ctx.Err() != nil || !e.creatingStillWanted(id) {
			return
		}
		setupCtx, cancelSetup := context.WithTimeout(ctx, projectHookTimeout)
		setupErr := e.runProjectHookWithContext(setupCtx, proj.SetupCmd, reset, proj, "setup")
		cancelSetup()
		if setupErr != nil {
			if ctx.Err() != nil {
				return
			}
			e.failCreating(id, fmt.Errorf("setup command failed: %w", setupErr))
			return
		}
		if ctx.Err() != nil || !e.creatingStillWanted(id) {
			return
		}
		mergeBase := ""
		if v, ok := e.forkOverlay.LoadAndDelete(id); ok {
			src := v.(domain.Workspace)
			if src.WorktreePath != "" && reset.WorktreePath != "" {
				if err := copyWorktreeDirty(src.WorktreePath, reset.WorktreePath); err != nil {
					if ctx.Err() != nil {
						return
					}
					e.failCreating(id, fmt.Errorf("copy source changes: %w", err))
					return
				}
			}
			mergeBase = strings.TrimSpace(src.BaseBranch)
		}
		if ctx.Err() != nil || !e.creatingStillWanted(id) {
			return
		}
		wrote := false
		ready, ok := e.Store.UpdateWorkspace(id, func(w *domain.Workspace) {
			if w.Archived {
				return
			}
			wrote = true
			w.StatusLabel = "Idle"
			w.Phase = "editing"
			w.Last = "created"
			w.LastError = ""
			if mergeBase != "" {
				w.BaseBranch = mergeBase
			}
		})
		if !ok || !wrote {
			return
		}
		e.publishLoopUpdated(ready.ID, ready.State, ready.Phase, ready.Iteration)
		e.creating.Delete(id)
		e.maybeDispatchAfterAllocate(id)
	}()
	return nil
}

func (e *Engine) cancelCreating(id string) *creatingAlloc {
	v, ok := e.creating.Load(id)
	if !ok {
		return nil
	}
	alloc := v.(*creatingAlloc)
	alloc.cancel()
	return alloc
}

func waitAlloc(alloc *creatingAlloc) {
	if alloc == nil {
		return
	}
	select {
	case <-alloc.done:
	case <-time.After(allocWait):
	}
}

func (e *Engine) allocateWorkspaceWorktree(ctx context.Context, rec domain.Workspace) error {
	// Keep the spec visible to CreateWorkspaceFromPR until this allocate
	// returns so a second click reuses the in-flight workspace.
	defer e.prHeadCheckout.Delete(rec.ID)
	proj, ok := e.Store.Project(rec.ProjectID)
	if !ok {
		return fmt.Errorf("project %q not found", rec.ProjectID)
	}
	if !e.creatingStillWanted(rec.ID) {
		return context.Canceled
	}

	base := rec.BaseBranch
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		base = "main"
	}

	mgr := e.gitManager(proj.RepoPath)
	addBase := base
	var headSpec prHeadCheckoutSpec
	if v, ok := e.prHeadCheckout.Load(rec.ID); ok {
		if spec, ok := v.(prHeadCheckoutSpec); ok && spec.Number > 0 {
			headSpec = spec
		}
	}
	if headSpec.Number > 0 {
		if ref, err := mgr.FetchPRHead(ctx, headSpec.Number); err != nil {
			return fmt.Errorf("fetch pull request head: %w", err)
		} else if ref != "" {
			addBase = ref
		}
	}
	addCtx, cancelAdd := context.WithTimeout(ctx, worktreeAddTO)
	var wt gitpkg.Worktree
	var created bool
	var err error
	useNamedHead := headSpec.Number > 0 && headSpec.Branch != "" && !headSpec.CrossRepo
	if useNamedHead {
		wt, created, err = mgr.AddAtRef(addCtx, rec.ID, gitpkg.WorkspaceSlug(rec.ID), headSpec.Branch, addBase)
	} else {
		wt, created, err = mgr.Add(addCtx, rec.ID, gitpkg.WorkspaceSlug(rec.ID), addBase)
	}
	cancelAdd()
	if err != nil {
		return fmt.Errorf("allocate worktree: %w", err)
	}
	if useNamedHead && wt.Path != "" && normalizePRBranch(wt.Branch) == normalizePRBranch(headSpec.Branch) {
		mgr.TrySetUpstream(ctx, wt.Path, headSpec.Branch)
	}

	port := rec.Port
	if created && port <= 0 {
		port, err = runconfig.FreePort()
		if err != nil {
			_ = mgr.Remove(context.Background(), wt.Path)
			return fmt.Errorf("allocate port: %w", err)
		}
	}

	if ctx.Err() != nil {
		_ = mgr.Remove(context.Background(), wt.Path)
		return ctx.Err()
	}
	if !e.creatingStillWanted(rec.ID) {
		_ = mgr.Remove(context.Background(), wt.Path)
		return context.Canceled
	}

	wrote := false
	updated, ok := e.Store.UpdateWorkspace(rec.ID, func(w *domain.Workspace) {
		// Remove may have archived the row while we were between the
		// creatingStillWanted check and this write — never resurrect path/port.
		if w.Archived {
			return
		}
		wrote = true
		w.WorktreePath = wt.Path
		w.Branch = wt.Branch
		w.BaseBranch = base
		w.Port = port
		w.HostPRID = ""
	})
	if !ok || !wrote {
		_ = mgr.Remove(context.Background(), wt.Path)
		if !ok {
			return fmt.Errorf("workspace %q gone during allocate", rec.ID)
		}
		return context.Canceled
	}
	// Paint the Observatory as soon as the worktree exists. setup_cmd can take
	// a second or more (a project that copies a large data dir) and must not
	// hold the creating overlay — chat still waits because phase stays creating.
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	e.linkWorkspaceToMatchingPRs(updated)
	if headSpec.PRID != "" {
		e.Store.UpdatePR(headSpec.PRID, func(p *domain.PR) {
			p.WorkspaceID = updated.ID
			if strings.TrimSpace(p.ProjectID) == "" {
				p.ProjectID = updated.ProjectID
			}
		})
		e.publishPRUpdated(headSpec.PRID)
		e.archivePRChatHosts(headSpec.PRID, updated.ID)
	}

	if created {
		if err := e.copyProjectFiles(ctx, proj, updated.WorktreePath); err != nil {
			return fmt.Errorf("copy files: %w", err)
		}
		setupCtx, cancelSetup := context.WithTimeout(ctx, projectHookTimeout)
		setupErr := e.runProjectHookWithContext(setupCtx, proj.SetupCmd, updated, proj, "setup")
		cancelSetup()
		if setupErr != nil {
			e.runTeardownHook(updated, proj)
			// Keep the worktree: worktreePath was already published, so the
			// editor/terminal may have been used. failCreating surfaces the
			// error; Discard/Delete still remove the tree. (CreateWorkspace
			// rolls the record back and removes disk via rollbackCreating.)
			return fmt.Errorf("setup command failed: %w", setupErr)
		}
		if ctx.Err() != nil {
			e.runTeardownHook(updated, proj)
			_ = mgr.Remove(context.Background(), wt.Path)
			return ctx.Err()
		}
	}

	mergeBase := ""
	if v, ok := e.forkOverlay.LoadAndDelete(rec.ID); ok {
		src := v.(domain.Workspace)
		if src.WorktreePath != "" && updated.WorktreePath != "" {
			if err := copyWorktreeDirty(src.WorktreePath, updated.WorktreePath); err != nil {
				return fmt.Errorf("copy source changes: %w", err)
			}
		}
		mergeBase = strings.TrimSpace(src.BaseBranch)
	}

	ready, ok := e.Store.UpdateWorkspace(rec.ID, func(w *domain.Workspace) {
		w.StatusLabel = "Idle"
		w.Phase = "editing"
		w.Last = "created"
		w.LastError = ""
		if mergeBase != "" {
			w.BaseBranch = mergeBase
		}
	})
	if !ok {
		_ = mgr.Remove(context.Background(), wt.Path)
		return fmt.Errorf("workspace %q gone after allocate", rec.ID)
	}
	e.publishLoopUpdated(ready.ID, ready.State, ready.Phase, ready.Iteration)
	return nil
}

func (e *Engine) maybeDispatchAfterAllocate(id string) {
	rec, ok := e.Store.Workspace(id)
	if !ok || !rec.HasRun() || rec.WorktreePath == "" {
		return
	}
	if _, running := e.loops.Load(id); running {
		return
	}
	if !e.gov.TryAcquireLoop() {
		e.publishResource()
		return
	}
	if err := e.dispatch(rec); err != nil {
		e.gov.ReleaseLoop()
		if !benignDispatchErr(err) {
			e.failCreating(id, err)
		}
	}
}

func (e *Engine) failCreating(id string, err error) {
	e.prHeadCheckout.Delete(id)
	msg := err.Error()
	updated, ok := e.Store.UpdateWorkspace(id, func(w *domain.Workspace) {
		w.StatusLabel = "Couldn't create"
		w.Phase = "failed"
		w.Last = "create failed"
		w.LastError = msg
		// Keep WorktreePath: dispatch may have allocated on disk before failing,
		// and Delete/Remove need the path to remove it. Callers that actually
		// tore the worktree down already cleared the field.
		if w.HasRun() {
			w.State = domain.StateFailed
		}
	})
	if !ok {
		return
	}
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
}

func (e *Engine) creatingStillWanted(id string) bool {
	if _, busy := e.deleting.Load(id); busy {
		return false
	}
	ws, ok := e.Store.Workspace(id)
	return ok && !ws.Archived
}

func (e *Engine) rollbackCreating(id string) {
	e.forkOverlay.Delete(id)
	e.prHeadCheckout.Delete(id)
	ws, ok := e.Store.Workspace(id)
	if !ok {
		return
	}
	proj, pok := e.Store.Project(ws.ProjectID)
	if pok {
		e.runTeardownHook(ws, proj)
	}
	e.cleanupPartialWorktree(ws)
	if pok && ws.Branch != "" {
		base := ws.BaseBranch
		if base == "" {
			base = proj.BaseBranch
		}
		mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
		_ = mgr.DeleteBranchIfAtBase(context.Background(), ws.Branch, base)
	}
	_ = e.Store.DeleteWorkspace(id)
}

func (e *Engine) cleanupPartialWorktree(ws domain.Workspace) {
	path := ws.WorktreePath
	if path == "" && e.worktreeRoot != "" && ws.ID != "" {
		path = filepath.Join(e.worktreeRoot, gitpkg.WorktreeRelPath(ws.ID))
	}
	if path == "" {
		return
	}
	ws.WorktreePath = path
	e.removeLoopWorktree(ws)
}

// copyProjectFiles overlays glob-matched files from the project checkout onto
// a freshly created worktree, before setup_cmd. No-op when CopyFiles is empty.
// Existing dest files are left in place. Bounded by projectHookTimeout and ctx.
func (e *Engine) copyProjectFiles(ctx context.Context, proj domain.Project, dest string) error {
	if ctx == nil {
		ctx = context.Background()
	}
	ctx, cancel := context.WithTimeout(ctx, projectHookTimeout)
	defer cancel()
	n, err := copyfiles.Copy(ctx, proj.RepoPath, dest, proj.CopyFiles)
	if err != nil {
		return err
	}
	if n > 0 {
		log.Printf("recur: copied %d file(s) into %s", n, dest)
	}
	return nil
}
