package engine

import (
	"fmt"
	"html"
	"log"

	"recur/internal/domain"
)

// Resume-from-checkpoint (D55, PRD §6.5).
//
// A loop's durable record carries a Checkpoint — the last in-flight phase plus the
// convergence iteration it belonged to (written by Engine.setWorkspaceState). When a loop
// is re-dispatched after an app quit/crash or a transient LLM failure, that
// checkpoint decides where the state machine re-enters, instead of replaying from
// planning with a cold agent.
//
// Two things are restored:
//
//   - POSITION — the phase + iteration, so the loop does not re-plan a task it
//     already planned or re-run iteration 1 over five iterations of committed work.
//   - MEMORY — each role's prior `claude` session id, so the FIRST turn that role
//     runs after the resume continues its old session via --resume and still knows
//     what it was doing. Subsequent turns start fresh, exactly as before.
//
// What is deliberately NOT resumed: the killed `claude -p` turn itself. A process
// that died mid-edit cannot be reattached, so the phase it was in is re-run — with
// its session resumed, so the agent picks up rather than restarts. The engine
// re-counts findings and re-declares convergence, so correctness never depends on
// trusting pre-restart state.

// resumePointFor decodes a persisted checkpoint into the position a re-dispatched
// goroutine starts from, or nil for a loop that should start fresh (no checkpoint,
// or a checkpoint in a phase that is not re-enterable).
//
// The in-iteration phases collapse onto converge: a loop interrupted while REVIEWING
// or VERIFYING already has the Coder's commit in the worktree, so it re-enters
// converge with SkipCoder — the reviewer looks at the real committed diff rather than
// the Coder redoing work it finished. A loop interrupted while CODING re-runs the
// coding turn (its edits may be half-applied; the resumed session sees its own work).
func resumePointFor(rec domain.Workspace, sessions map[string]string) *resumePoint {
	cp := rec.Checkpoint
	if cp == nil {
		return nil
	}
	iter := cp.Iteration
	if iter < 1 {
		iter = 1
	}
	switch cp.State {
	case domain.StatePlanning:
		return &resumePoint{State: domain.StatePlanning, Iteration: iter, sessions: sessions}
	case domain.StateCoding:
		return &resumePoint{State: domain.StateCoding, Iteration: iter, sessions: sessions}
	case domain.StateReviewing, domain.StateVerifying:
		return &resumePoint{
			State: domain.StateCoding, Iteration: iter, SkipCoder: true, sessions: sessions,
		}
	default:
		return nil
	}
}

// agentSessions returns each role code's most recent claude session id for a loop, so
// a resumed turn can continue that session (D55). Roles that never ran are absent.
func (e *Engine) agentSessions(workspaceID string) map[string]string {
	agents := e.Store.Agents(workspaceID)
	if len(agents) == 0 {
		return nil
	}
	m := make(map[string]string, len(agents))
	for _, a := range agents {
		if a.SessionID != "" {
			m[a.Code] = a.SessionID
		}
	}
	if len(m) == 0 {
		return nil
	}
	return m
}

// clearCheckpoint drops a loop's resume position. Called when the loop reaches a
// terminal state: there is nothing left to resume, and a stale checkpoint would let
// a later Resume re-enter a finished loop mid-machine.
func (e *Engine) clearCheckpoint(id string) {
	e.Store.UpdateWorkspace(id, func(l *domain.Workspace) {
		l.Checkpoint = nil
	})
}

// recordFailure persists the reason a loop failed and whether that reason is
// retryable, so the UI can show what actually went wrong (a spend-limit message, not
// "error_during_execution") and decide whether to offer Retry (D56).
func (e *Engine) recordFailure(id, reason string, retryable bool) {
	e.Store.UpdateWorkspace(id, func(l *domain.Workspace) {
		l.LastError = reason
		l.Retryable = retryable
	})
}

// failRetryable parks a loop that hit a TRANSIENT provider failure (D56). The loop is
// marked failed — it genuinely stopped, and pretending otherwise would be dishonest —
// but keeps its checkpoint, records the provider's message, and moves into "needs you"
// with an inbox row + notification, because this is a failure only the human can clear
// (raise the limit, wait out the outage) and then Retry.
func (e *Engine) failRetryable(id, reason string) {
	e.setWorkspaceState(id, domain.StateFailed, "stopped: "+reason)
	e.recordFailure(id, reason, true)
	e.markGroup(id, domain.GroupNeeds)
	e.emitWorkspaceEvent(id, "MA", "Maestro",
		"Loop stopped on a transient error — retry when it clears: "+html.EscapeString(reason))

	rec, ok := e.Store.Workspace(id)
	if !ok {
		return
	}
	e.Store.AddInboxItem(domain.InboxItem{
		ID:          "ib-retry-" + id,
		Group:       "needs",
		Unread:      true,
		Kind:        "pause",
		Color:       "var(--coral)",
		Title:       fmt.Sprintf("<b>%s</b> stopped — retry when it clears", html.EscapeString(id)),
		Sub:         html.EscapeString(truncate(rec.Short, 64)) + " · " + html.EscapeString(truncate(reason, 64)),
		Time:        "now",
		Action:      "Retry",
		ActionClass: "urgent",
		Target:      "workspace:" + id,
		WorkspaceID: id,
	})
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	e.Bus.Publish(Event{Name: "notify", Data: map[string]any{
		"title": "A loop stopped — retry available",
		"body":  truncate(reason, 120),
	}})
}

// Retry re-runs a loop that failed on a TRANSIENT LLM/API error (spend or rate limit,
// overload, dropped connection) from its last checkpoint (D56). It clears the failure
// marks and hands off to Resume, which owns the slot accounting and re-dispatch; the
// checkpoint is left intact so the loop re-enters where it stopped.
//
// A loop that failed for any other reason is NOT retryable and this is a no-op: a
// genuine engine/commit/rail failure would just fail again, and silently re-running it
// would be dishonest about what happened.
// Restart re-runs a deliberately-stopped (or terminally-failed) loop over its
// existing worktree — a hand BOTH the human (strip button) and the Maestro
// (mcp__recur__resume_loop) have. Unlike Retry it does not
// require a retryable failure: an explicit restart is an intentional decision,
// so the engine honors it for stopped and failed loops alike. A stopped loop's
// checkpoint was cleared (D55), so the run starts fresh from the worktree's
// current state, steered by the persisted plan (DefinitionOfDone). Anything
// else is refused with the reason named (chat + feed stay honest).
func (e *Engine) Restart(id string) string {
	if e.runningLoopByID(id) != nil {
		return "Refused: the loop is already running."
	}
	rec, ok := e.Store.Workspace(id)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	if !rec.HasRun() {
		return "Refused: this workspace has no loop to restart — engage Loop mode in the chat first."
	}
	switch rec.State {
	case domain.StateStopped, domain.StateFailed:
		// Restartable.
	case domain.StatePROpen:
		return "Refused: the loop settled with an open PR — Send Back on the PR (talks to Maestro), or reenter_phase."
	case domain.StateMerged:
		return "Refused: this loop's PR is merged — start new work instead."
	default:
		return "Refused: the loop is " + string(rec.State) + " — Resume covers that state."
	}

	return e.dispatchIdleWalk(id)
}

// dispatchIdleWalk re-dispatches a settled loop (paused / pr_open / stopped /
// failed) from its current checkpoint. Used by reenter_phase / run_phase, which
// are legal at idle; Restart itself stays stopped/failed-only.
func (e *Engine) idleWalkRefuse(id string) string {
	if e.runningLoopByID(id) != nil {
		return "Refused: the loop is already running."
	}
	rec, ok := e.Store.Workspace(id)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	if !rec.HasRun() {
		return "Refused: this workspace has no loop to restart — engage Loop mode in the chat first."
	}
	switch rec.State {
	case domain.StateStopped, domain.StateFailed, domain.StatePaused, domain.StatePROpen:
		return ""
	case domain.StateMerged:
		return "Refused: this loop's PR is merged — start new work instead."
	default:
		return "Refused: the loop is " + string(rec.State) + " — wait until it settles."
	}
}

func (e *Engine) dispatchIdleWalk(id string) string {
	e.dispatchMu.Lock()
	defer e.dispatchMu.Unlock()
	return e.dispatchIdleWalkLocked(id)
}

func (e *Engine) dispatchIdleWalkLocked(id string) string {
	if reason := e.idleWalkRefuse(id); reason != "" {
		return reason
	}

	e.Store.UpdateWorkspace(id, func(l *domain.Workspace) {
		l.LastError = ""
		l.Retryable = false
	})
	// Same move as Retry: park in the resting state Resume knows how to
	// re-dispatch rather than teaching Resume another entry condition.
	e.setWorkspaceState(id, domain.StatePaused, "restarting")
	e.Store.RemoveInboxByTarget("workspace:" + id)
	e.emitWorkspaceEvent(id, "MA", "Maestro", "Restarting the loop over the current worktree.")
	e.resumeNotRunningLocked(id, "")
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	return "Loop restarting — it continues from the current worktree, steered by the recorded plan."
}

func (e *Engine) Retry(id string) {
	rec, ok := e.Store.Workspace(id)
	if !ok {
		return
	}
	if rec.State == domain.StateFailed && !rec.Retryable {
		return
	}
	e.Store.UpdateWorkspace(id, func(l *domain.Workspace) {
		l.LastError = ""
		l.Retryable = false
	})
	// Move out of `failed` into the resting state Resume already knows how to
	// re-dispatch, rather than teaching Resume a second entry condition. This must
	// happen BEFORE Resume: it re-reads the record under dispatchMu and would refuse a
	// loop that is still marked failed with the retryable flag now cleared. The
	// checkpoint survives (setWorkspaceState only clears it on stopped/landed), so the
	// re-dispatch re-enters where the loop stopped.
	e.setWorkspaceState(id, domain.StatePaused, "retrying from the last checkpoint")
	e.Store.RemoveInboxByTarget("workspace:" + id)
	e.emitWorkspaceEvent(id, "MA", "Maestro", "Retrying from the last checkpoint.")
	e.Resume(id)
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
}

// AutoResume re-dispatches every loop that Recover parked as interrupted, so closing
// and reopening the app continues the work instead of waiting for the user to press
// Resume on each loop (PRD §6.5). It also continues coding/Maestro chats Recover
// queued as interrupted. Call once at startup, immediately after Recover.
//
// It routes through the normal Resume path, so the governor's MaxLoops cap still
// applies: loops beyond the cap are left queued and the existing drain starts them as
// slots free. Loops that are paused for a REASON the human owns — an unanswered
// escalation (left as escalated by Recover), or a loop the user paused by hand before
// quitting — are not touched; only the "interrupted" phase Recover itself writes is
// auto-resumed, so auto-resume can never override a human's pause.
func (e *Engine) AutoResume() {
	// Subagent watchers re-arm only after interrupted chats are launched, so a
	// healed-but-not-yet-resumed child cannot be reported as finished.
	defer e.rearmChatChildren()
	e.SyncChatInbox()
	// Loop-free chats resume immediately so they are not blocked behind per-loop
	// git work (worktree recreate, up to 60s each). Chats on a workspace that
	// this pass will Resume wait until after that Resume returns: Recover parks
	// the loop at paused/interrupted (RunActive is false), and an agent-mode
	// send in that window would edit the worktree concurrently with the Coder.
	hold := e.workspacesPendingLoopAutoResume()
	e.launchInterruptedChatsExcept(hold)
	for _, l := range e.Store.Workspaces() {
		if l.Archived {
			continue
		}
		switch {
		case l.State == domain.StatePaused && l.Phase == phaseInterrupted:
			if l.Checkpoint == nil {
				continue // nothing durable to resume from; leave it for a manual Resume
			}
			log.Printf("recur: auto-resuming loop %s from %s (iteration %d)",
				l.ID, l.Checkpoint.State, l.Checkpoint.Iteration)
			e.emitWorkspaceEvent(l.ID, "MA", "Maestro",
				"Reloop restarted — resuming from the last checkpoint.")
		case l.State == domain.StateQueued:
			// A loop that was still waiting for a slot when the app exited. Nothing ran, so
			// there is no checkpoint and it starts from planning — but without this it would
			// sit queued forever, because the queued drain only fires when another loop
			// exits and no loop is running after a restart.
			log.Printf("recur: starting queued loop %s after restart", l.ID)
		default:
			continue
		}
		e.Resume(l.ID)
		e.launchInterruptedChatsFor(l.ID)
	}
	e.launchInterruptedChats(e.interruptedChats)
	e.interruptedChats = nil
}

// workspacesPendingLoopAutoResume is the set AutoResume will call Resume on.
func (e *Engine) workspacesPendingLoopAutoResume() map[string]struct{} {
	out := map[string]struct{}{}
	if e == nil || e.Store == nil {
		return out
	}
	for _, l := range e.Store.Workspaces() {
		if l.Archived {
			continue
		}
		switch {
		case l.State == domain.StatePaused && l.Phase == phaseInterrupted && l.Checkpoint != nil:
			out[l.ID] = struct{}{}
		case l.State == domain.StateQueued:
			out[l.ID] = struct{}{}
		}
	}
	return out
}
