package engine

import (
	"context"
	"fmt"
	"log"
	"strings"
	"time"

	"github.com/google/uuid"
	"recur/internal/domain"
)

// SaveScheduledPrompt validates and saves the next occurrence. The scheduler
// mutex serializes configuration changes, occurrence claims, and shutdown.
func (e *Engine) SaveScheduledPrompt(p domain.ScheduledPrompt) (domain.ScheduledPrompt, error) {
	e.scheduleMu.Lock()
	defer e.scheduleMu.Unlock()
	p.Name, p.Prompt, p.Cron = strings.TrimSpace(p.Name), strings.TrimSpace(p.Prompt), strings.TrimSpace(p.Cron)
	if p.Mode == domain.ChatModeLoop {
		guard, reason := e.resolveGuard(p.LoopID, p.PolicyID, e.Store.Policy().Autonomy)
		if reason != "" {
			return p, fmt.Errorf("%s", reason)
		}
		p.LoopID, p.PolicyID = guard.Loop.ID, guard.Policy.ID
	}
	if p.Name == "" || p.Prompt == "" {
		return p, fmt.Errorf("name and prompt are required")
	}
	if err := e.validateScheduledPrompt(p); err != nil {
		return p, err
	}
	if p.ID != "" {
		old, err := e.scheduledPrompt(p.ID)
		if err != nil {
			return p, err
		}
		if old.ProjectID != p.ProjectID {
			return p, fmt.Errorf("a schedule cannot change projects")
		}
	} else {
		p.ID = uuid.Must(uuid.NewV7()).String()
	}
	next, err := domain.NextScheduledTime(p, e.now())
	if err != nil {
		return p, err
	}
	if next.IsZero() && !p.OnStartup() {
		return p, fmt.Errorf("choose a future date and time that exists in this time zone")
	}
	p.NextRunAt = scheduledMillis(next)
	if p.Simplify == nil {
		on := e.settings().SimplifyEnabled()
		p.Simplify = &on
	}
	p.LastRun = nil
	return p, e.Store.SaveScheduledPrompt(p)
}

func (e *Engine) validateScheduledPrompt(p domain.ScheduledPrompt) error {
	proj, ok := e.Store.Project(p.ProjectID)
	if !ok {
		return fmt.Errorf("project not found")
	}
	switch p.Mode {
	case domain.ChatModeAsk, domain.ChatModePlan, domain.ChatModeAgent, domain.ChatModeReview:
	case domain.ChatModeLoop:
		if _, reason := e.resolveGuard(p.LoopID, p.PolicyID, e.Store.Policy().Autonomy); reason != "" {
			return fmt.Errorf("%s", reason)
		}
	default:
		return fmt.Errorf("choose Ask, Plan, Agent, Review, or Loop mode")
	}
	ref := domain.ParseModelRef(p.Model)
	if ref.ID == "" || !strings.Contains(p.Model, "/") || !e.Store.Settings().ProviderOffersModels(ref.Provider) {
		return fmt.Errorf("choose an available model")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if !e.gitManager(proj.RepoPath).BaseBranchExists(ctx, p.BaseBranch) {
		return fmt.Errorf("base branch no longer exists")
	}
	return nil
}

func (e *Engine) scheduledPrompt(id string) (domain.ScheduledPrompt, error) {
	all, err := e.Store.ScheduledPrompts()
	if err != nil {
		return domain.ScheduledPrompt{}, err
	}
	for _, p := range all {
		if p.ID == id {
			return p, nil
		}
	}
	return domain.ScheduledPrompt{}, fmt.Errorf("scheduled prompt not found")
}

// PauseScheduledPrompt keeps run history and skips elapsed times when resumed.
func (e *Engine) PauseScheduledPrompt(id string, paused bool) error {
	e.scheduleMu.Lock()
	defer e.scheduleMu.Unlock()
	p, err := e.scheduledPrompt(id)
	if err != nil {
		return err
	}
	p.Paused = paused
	if !paused && !(p.Cadence == "once" && p.NextRunAt == 0) {
		next, err := domain.NextScheduledTime(p, e.now())
		if err != nil {
			return err
		}
		p.NextRunAt = scheduledMillis(next)
	}
	return e.Store.SaveScheduledPrompt(p)
}

// DeleteScheduledPrompt stops future occurrences without stopping a running chat.
func (e *Engine) DeleteScheduledPrompt(id string) error {
	e.scheduleMu.Lock()
	defer e.scheduleMu.Unlock()
	return e.Store.DeleteScheduledPrompt(id)
}

// RunScheduledPromptNow claims a manual occurrence and returns its new workspace.
// A one-time schedule is consumed; recurring schedules keep their next due time.
func (e *Engine) RunScheduledPromptNow(id string) (string, error) {
	e.scheduleMu.Lock()
	defer e.scheduleMu.Unlock()
	if e.isShuttingDown() {
		return "", fmt.Errorf("Recur is closing")
	}
	p, err := e.scheduledPrompt(id)
	if err != nil {
		return "", err
	}
	runs, err := e.Store.ActiveScheduledPromptRuns()
	if err != nil {
		return "", err
	}
	if e.scheduleBusy(p.ID, runs) {
		return "", fmt.Errorf("the previous run is still active or needs your input")
	}
	if err := e.validateScheduledPrompt(p); err != nil {
		return "", err
	}
	now := e.now()
	if p.Cadence == "once" {
		p.NextRunAt = 0
	} else if p.NextRunAt > 0 && p.NextRunAt <= now.UnixMilli() {
		next, err := domain.NextScheduledTime(p, now)
		if err != nil {
			return "", err
		}
		p.NextRunAt = scheduledMillis(next)
	}
	r := domain.ScheduledPromptRun{ID: uuid.Must(uuid.NewV7()).String(), ScheduleID: id, DueAt: now.UnixMilli(), Status: "running"}
	claimed, err := e.Store.ClaimScheduledPrompt(p, r)
	if err != nil {
		return "", err
	}
	if !claimed {
		return "", fmt.Errorf("this occurrence was already claimed")
	}
	return e.launchScheduledPrompt(p, r)
}

func scheduledMillis(t time.Time) int64 {
	if t.IsZero() {
		return 0
	}
	return t.UnixMilli()
}

func (e *Engine) scheduleBusy(id string, runs []domain.ScheduledPromptRun) bool {
	for _, r := range runs {
		if r.ScheduleID != id {
			continue
		}
		if r.Status == "running" {
			return true
		}
		if r.WorkspaceID != "" {
			if e.scheduledChatActive(r.WorkspaceID) {
				return true
			}
		}
	}
	return false
}

// StartScheduledPrompts starts one app-owned timer after the event bus is wired.
// Startup and wake never replay overdue occurrences automatically.
func (e *Engine) StartScheduledPrompts() error {
	e.scheduleMu.Lock()
	defer e.scheduleMu.Unlock()
	if e.scheduleStop != nil || e.isShuttingDown() {
		return nil
	}
	if err := e.tickScheduledPrompts(e.now(), true); err != nil {
		return err
	}
	e.scheduleStop, e.scheduleDone = make(chan struct{}), make(chan struct{})
	go func() {
		defer close(e.scheduleDone)
		ticker := time.NewTicker(time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-e.scheduleStop:
				return
			case <-ticker.C:
				e.scheduleMu.Lock()
				if !e.isShuttingDown() {
					if err := e.tickScheduledPrompts(e.now(), false); err != nil {
						log.Printf("scheduled prompts: %v", err)
					}
				}
				e.scheduleMu.Unlock()
			}
		}
	}()
	go e.reconcileScheduledWorkspaces()
	return nil
}

// stampScheduledWorkspaceOrigins copies schedule IDs from occurrence rows onto
// workspaces created before ScheduleID existed on the record.
func (e *Engine) stampScheduledWorkspaceOrigins() {
	runs, err := e.Store.ScheduledPromptRuns()
	if err != nil {
		log.Printf("scheduled workspace origins: %v", err)
		return
	}
	for _, r := range runs {
		if r.WorkspaceID == "" || r.ScheduleID == "" {
			continue
		}
		e.Store.UpdateWorkspace(r.WorkspaceID, func(w *domain.Workspace) {
			if w.ScheduleID == "" {
				w.ScheduleID = r.ScheduleID
			}
		})
	}
}

// PruneScheduledWorkspaces removes settled occurrence workspaces beyond Keep last N
// per schedule. Empty scheduleID prunes every schedule. Running, waiting, parked,
// dirty, and open-PR workspaces are left in place.
func (e *Engine) PruneScheduledWorkspaces(scheduleID string) {
	keep := e.settings().ScheduledWorkspaceKeep
	if keep <= 0 {
		return
	}
	by := map[string][]string{}
	children := map[string]bool{}
	for _, w := range e.Store.Workspaces() {
		if w.Archived || w.IsMain || w.IsPRChatHost() {
			continue
		}
		if pid := strings.TrimSpace(w.ParentID); pid != "" {
			children[pid] = true
		}
		if w.ScheduleID == "" {
			continue
		}
		if scheduleID != "" && w.ScheduleID != scheduleID {
			continue
		}
		by[w.ScheduleID] = append(by[w.ScheduleID], w.ID)
	}
	for _, ids := range by {
		if len(ids) <= keep {
			continue
		}
		for _, id := range ids[:len(ids)-keep] {
			if children[id] || e.scheduledOccurrenceProtected(id) {
				continue
			}
			if _, err := e.RemoveWorkspace(id); err != nil {
				log.Printf("scheduled workspace prune %s: %v", id, err)
			}
		}
	}
}

func (e *Engine) scheduledOccurrenceProtected(id string) bool {
	ws, ok := e.Store.Workspace(id)
	if !ok {
		return true
	}
	if ws.State == domain.StatePROpen {
		return true
	}
	if _, ok := e.Store.OpenPRByWorkspace(id); ok {
		return true
	}
	if e.scheduledChatActive(id) {
		return true
	}
	return e.WorkspaceDiscardRisk(id) != ""
}

func (e *Engine) reconcileScheduledWorkspaces() {
	e.stampScheduledWorkspaceOrigins()
	e.PruneScheduledWorkspaces("")
}

// tickScheduledPrompts is synchronous; callers hold scheduleMu. Tests supply now.
func (e *Engine) tickScheduledPrompts(now time.Time, startup bool) error {
	all, err := e.Store.DueScheduledPrompts(now, startup)
	if err != nil {
		return err
	}
	runs, err := e.Store.ActiveScheduledPromptRuns()
	if err != nil {
		return err
	}
	for i := range runs {
		r := &runs[i]
		if startup && (r.Status == "running" || r.Status == "waiting") {
			r.Status, r.Detail = "interrupted", "Recur closed during this run. Inspect its workspace before retrying."
			if err := e.Store.UpdateScheduledPromptRun(*r); err != nil {
				return err
			}
		}
		if r.Status == "waiting" && !e.scheduledChatActive(r.WorkspaceID) {
			r.Status, r.Detail = e.scheduledChatOutcome(r.WorkspaceID, "")
			if err := e.Store.UpdateScheduledPromptRun(*r); err != nil {
				return err
			}
			e.clearScheduledPromptPending(r.WorkspaceID)
		}
	}
	e.settleScheduleWait()
	for _, p := range all {
		onStartup := p.OnStartup()
		if p.Paused || (onStartup && !startup) || (!onStartup && (p.NextRunAt == 0 || p.NextRunAt > now.UnixMilli())) {
			continue
		}
		r := domain.ScheduledPromptRun{ID: uuid.Must(uuid.NewV7()).String(), ScheduleID: p.ID, DueAt: p.NextRunAt, Status: "running"}
		if onStartup {
			r.DueAt = now.UnixMilli()
		}
		next, err := domain.NextScheduledTime(p, now)
		if err != nil {
			return err
		}
		p.NextRunAt = scheduledMillis(next)
		// A one-minute grace tolerates timer jitter without replaying a backlog.
		// Later wake delivery is a missed run; a daemon would be needed while closed.
		if !onStartup && (startup || now.Sub(time.UnixMilli(r.DueAt)) >= time.Minute) {
			r.Status, r.Detail = "missed", "Recur was unavailable at the scheduled time. Use Run now to run once."
		} else if e.scheduleBusy(p.ID, runs) {
			r.Status, r.Detail = "skipped", "The previous run is still active or needs your input."
		}
		claimed, err := e.Store.ClaimScheduledPrompt(p, r)
		if err != nil {
			return err
		}
		if claimed && r.Status == "running" {
			if _, err := e.launchScheduledPrompt(p, r); err != nil {
				log.Printf("scheduled prompt %s: %v", p.ID, err)
			}
		}
	}
	return nil
}

func (e *Engine) finishScheduledPrompt(r domain.ScheduledPromptRun, status, detail string) {
	r.Status, r.Detail = status, detail
	if err := e.Store.UpdateScheduledPromptRun(r); err != nil {
		log.Printf("scheduled prompt outcome: %v", err)
	}
}

// promptLaunch is one engine-initiated workspace: allocate a fresh worktree,
// then run the prompt on its first chat tab (or engage the loop). Shared by
// scheduled runs and the create_workspace MCP tool.
type promptLaunch struct {
	ProjectID, Name, BaseBranch, Prompt, Mode, Model, LoopID, PolicyID, ScheduleID string
	Flags                                                                          map[string]string
	Simplify                                                                       *bool
}

// launchScheduledPrompt persists the association before allocating or sending.
// Its worker uses the existing allocation cancellation and chat stop controls.
func (e *Engine) launchScheduledPrompt(p domain.ScheduledPrompt, r domain.ScheduledPromptRun) (string, error) {
	if err := e.validateScheduledPrompt(p); err != nil {
		e.finishScheduledPrompt(r, "failed", err.Error())
		return "", err
	}
	l := promptLaunch{ProjectID: p.ProjectID, Name: p.Name, BaseBranch: p.BaseBranch, Prompt: p.Prompt, Mode: p.Mode,
		Model: p.Model, LoopID: p.LoopID, PolicyID: p.PolicyID, Flags: p.Flags, Simplify: p.Simplify, ScheduleID: p.ID}
	id, err := e.launchPromptWorkspace(l, func(wsID string) error {
		r.WorkspaceID = wsID
		return e.Store.LinkScheduledPromptRun(r.ID, wsID)
	}, func(status, detail string) { e.finishScheduledPrompt(r, status, detail) })
	if err != nil {
		e.finishScheduledPrompt(r, "failed", err.Error())
	}
	return id, err
}

// launchPromptWorkspace creates the record, calls link with its id before any
// side effects, and runs the prompt in the background. onDone receives the
// outcome (finished/failed/stopped/waiting) once the first turn settles.
func (e *Engine) launchPromptWorkspace(l promptLaunch, link func(wsID string) error, onDone func(status, detail string)) (string, error) {
	ws, err := e.persistCreatingWorkspace(l.ProjectID, l.Name, l.BaseBranch)
	if err != nil {
		return "", err
	}
	if link != nil {
		if err := link(ws.ID); err != nil {
			e.rollbackCreating(ws.ID)
			return "", err
		}
	}
	e.Store.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.ScheduledPromptPending = true
		w.ScheduleID = l.ScheduleID
	})
	ctx, cancel := context.WithCancel(context.Background())
	alloc := &creatingAlloc{cancel: cancel, done: make(chan struct{})}
	e.creating.Store(ws.ID, alloc)
	e.scheduleWG.Add(1)
	go func() {
		defer e.scheduleWG.Done()
		err := e.allocateWorkspaceWorktree(ctx, ws)
		cancel()
		e.creating.Delete(ws.ID)
		close(alloc.done)
		if e.isShuttingDown() {
			return
		}
		if err != nil {
			e.failCreating(ws.ID, err)
			onDone("failed", err.Error())
		} else {
			e.ensurePairAgent(ws.ID, pairAgentCode, l.Model)
			e.RememberWorkspaceChatSelection(ws.ID, pairAgentCode, l.Model, l.Flags, l.Mode, l.Simplify)
			var reply string
			if l.Mode == domain.ChatModeLoop {
				// Loop kickoff returns after registering its worker. Serialize that
				// registration with Shutdown's snapshot of workers to cancel.
				e.scheduleMu.Lock()
				if e.isShuttingDown() {
					e.scheduleMu.Unlock()
					return
				}
				e.RememberChatLoopPolicy(ws.ID, pairAgentCode, l.LoopID, l.PolicyID)
				reply = e.SendWorkspaceMessage(ws.ID, pairAgentCode, l.Prompt, l.Model, l.Flags, nil, l.Mode, "")
				e.scheduleMu.Unlock()
			} else {
				reply = e.SendWorkspaceMessage(ws.ID, pairAgentCode, l.Prompt, l.Model, l.Flags, nil, l.Mode, "")
			}
			if e.isShuttingDown() {
				return
			}
			status, detail := e.scheduledChatOutcome(ws.ID, reply)
			onDone(status, detail)
			if status == "waiting" {
				e.scheduleWait.Store(ws.ID, struct{}{})
				if l.ScheduleID != "" {
					e.PruneScheduledWorkspaces(l.ScheduleID)
				}
				return
			}
		}
		e.clearScheduledPromptPending(ws.ID)
		if l.ScheduleID != "" {
			e.PruneScheduledWorkspaces(l.ScheduleID)
		}
	}()
	return ws.ID, nil
}

func (e *Engine) scheduledChatOutcome(workspaceID, reply string) (string, string) {
	if e.scheduledChatActive(workspaceID) {
		return "waiting", "Run is continuing or waiting for input in the workspace."
	}
	if ws, ok := e.Store.Workspace(workspaceID); ok && ws.HasRun() {
		switch ws.State {
		case domain.StateFailed:
			return "failed", ws.LastError
		case domain.StateStopped:
			return "stopped", "Loop stopped."
		default:
			return "finished", "Open the workspace to inspect the loop result."
		}
	}
	if reply == chatStoppedReply {
		return "stopped", reply
	}
	if strings.TrimSpace(reply) == "" {
		reply = "Open the workspace for details."
	}
	if sess, ok := e.Store.SessionForLoopAgent(workspaceID, pairAgentCode); ok {
		messages, err := e.Store.SessionTranscript(sess.ID)
		if err != nil {
			return "failed", err.Error()
		}
		for i := len(messages) - 1; i >= 0; i-- {
			if messages[i].Role == "assistant" {
				if messages[i].StreamStatus == domain.StreamError {
					return "failed", reply
				}
				return "finished", "Open the workspace to inspect the result."
			}
		}
	}
	return "failed", reply
}

// scheduledChatActive includes the gap between a mode switch and its remounted
// turn, as well as parked questions that no longer have a live provider process.
func (e *Engine) scheduledChatActive(workspaceID string) bool {
	if e.runningLoopByID(workspaceID) != nil {
		return true
	}
	if ws, ok := e.Store.Workspace(workspaceID); ok && ws.HasRun() {
		if domain.RunActive(ws.State) || ws.State == domain.StateEscalated || (ws.State == domain.StatePaused && !scheduledPauseSettled(ws.Phase)) {
			return true
		}
	}
	if e.workspaceHasLiveChat(workspaceID) || e.askUsers.workspaceBusy(workspaceID) {
		return true
	}
	e.continueGenMu.Lock()
	defer e.continueGenMu.Unlock()
	for _, id := range e.liveContinues {
		if id.workspaceID == workspaceID {
			return true
		}
	}
	return false
}

func (e *Engine) clearScheduledPromptPending(workspaceID string) {
	if workspaceID == "" {
		return
	}
	e.scheduleWait.Delete(workspaceID)
	e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) { w.ScheduledPromptPending = false })
}

func (e *Engine) settleScheduleWait() {
	e.scheduleWait.Range(func(k, _ any) bool {
		id, _ := k.(string)
		if id == "" || e.scheduledChatActive(id) {
			return true
		}
		e.clearScheduledPromptPending(id)
		return true
	})
}

// scheduledPauseSettled matches walker-finished pauses (idle, artifact, missing
// graph) so a completed Docs loop does not keep the schedule busy.
func scheduledPauseSettled(phase string) bool {
	return loopSettled(domain.StatePaused, phase) || phase == "phase missing" || phase == "loop refs missing"
}
