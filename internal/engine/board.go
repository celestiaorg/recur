package engine

import (
	"context"
	"fmt"
	"strings"

	"recur/internal/domain"
)

// The board. A structured plan is a board of steps. The first coding pass of a
// converge entry works it continuously: every ready step runs in a child
// workspace (fanout.go — grouped by file overlap so sharers ride together), a
// child merges back the moment it settles, whatever that landing unblocked
// starts at once while slower siblings keep running, and the Maestro may
// replan off each handoff as it arrives. The parent never codes except to
// resolve a merge conflict or, as a fallback, to finish steps whose child
// failed (or when no loop slot is free for a child). The review then judges
// the aggregate diff. No worker waits on its slowest sibling, and the board
// re-evaluates after every handoff, so a plan whose steps all wait on one
// bootstrap step still fans out once that step lands.

// boardMaxRounds bounds the events (handoff, conflict, fallback) one board
// pass handles — a safety net, not a budget a healthy plan reaches.
var boardMaxRounds = 64

const boardCapPhase = "board event cap"

// runBoard drives the board for the entry's first coding pass. It returns
// ran=true when it performed the coding for this iteration (the caller then
// skips its own Coder turn and reviews the aggregate diff), stop when the
// parent was stopped, or err on a failed turn.
func (l *Loop) runBoard(ctx context.Context, iter int) (ran bool, stop domain.WorkspaceState, err error) {
	if !l.fanOutEligible() {
		return false, "", nil
	}
	// Success leaves nothing in flight. Error/stop/pause must not leak a sibling
	// whose dispatch outlives this parent walk.
	// simplify: retries>0 re-enter and merge these as failed; to let a sibling
	// finish across a failed attempt, skip this when runLoopPhase still has tries.
	defer l.stopInFlightChildren()
	l.startBoardHandoff()
	exclude := map[string]bool{} // child ids whose child failed once: the parent finishes them
	for round := 0; round < boardMaxRounds; round++ {
		res := l.fanOutSteps(ctx, exclude)
		if res.stopped {
			return ran, domain.StateStopped, nil
		}
		if res.merged > 0 {
			ran = true
		}
		// Children handed off: record it on the parent (the aggregate Reviewer and
		// the conductor read it) and let the Maestro revise the rest of the board
		// while the other children keep running. Then the board resumes; a
		// revision fans out like any other ready work. Conflicts in the same
		// result are resolved before that resume so MERGE_HEAD is gone first.
		if len(res.handoffs) > 0 {
			l.recordHandoff(roleSpec{code: "MA", name: "Maestro"}, strings.Join(res.handoffs, "\n"))
			if l.checkpoint(ctx) {
				l.runReplanner(ctx, l.lastHandoff)
			}
		}
		if len(res.conflicts) > 0 {
			l.pendingConflicts = res.conflicts
			l.setState(domain.StateCoding, fmt.Sprintf("resolving merge conflicts (iteration %d)", iter))
			l.setIteration(iter)
			if err := l.runCoder(ctx, iter, nil); err != nil {
				return ran, domain.StateFailed, err
			}
			if l.git != nil && l.git.MergeHead(ctx, l.Worktree) {
				return ran, domain.StateFailed, fmt.Errorf("merge still in progress after the resolve turn")
			}
			l.markConflictsResolved()
			ran = true
			continue
		}
		if len(res.handoffs) > 0 {
			continue
		}
		ws, ok := l.eng.Store.Workspace(l.ID)
		if !ok || ws.Plan == nil {
			return ran, "", nil
		}
		if !hasPendingSteps(*ws.Plan) {
			if ran {
				l.emitFeed("MA", "Maestro", "Every step landed — reviewing the aggregate diff.")
			}
			return ran, "", nil
		}
		// Pending steps but nothing more can fan out: no slot, a child already
		// failed on them, or their dependencies are unknown/cyclic. One plain
		// Coder turn here over the whole plan finishes what is left.
		why := "steps blocked by unknown dependencies"
		switch {
		case res.noSlot:
			why = "no free loop slot for a child workspace"
		case len(exclude) > 0:
			why = "a child workspace failed on them"
		}
		l.emitFeed("MA", "Maestro", "Finishing the remaining steps here — "+why+".")
		l.setState(domain.StateCoding, fmt.Sprintf("iteration %d", iter))
		l.setIteration(iter)
		if err := l.runCoder(ctx, iter, nil); err != nil {
			return ran, domain.StateFailed, err
		}
		ran = true
		// The parent's own handoff may revise the board too; a revision goes
		// back through fan-out, otherwise this turn concludes the board.
		if l.lastHandoff != "" && l.checkpoint(ctx) && l.runReplanner(ctx, l.lastHandoff) {
			continue
		}
		return true, "", nil
	}
	l.setState(domain.StatePaused, boardCapPhase)
	l.emitFeed("MA", "Maestro", "Board paused — too many events with work still open.")
	return ran, domain.StatePaused, nil
}

// startBoardHandoff resets notes for a new coding round. A resumed board keeps
// or rebuilds already-landed children's notes so a later empty sibling cannot
// drop them.
func (l *Loop) startBoardHandoff() {
	if l.boardPassInProgress() {
		if strings.TrimSpace(l.boardHandoff) == "" {
			l.boardHandoff = l.landedBoardHandoff()
		}
		return
	}
	l.resetBoardHandoff()
}

func (l *Loop) boardPassInProgress() bool {
	if l.eng == nil {
		return false
	}
	ws, ok := l.eng.Store.Workspace(l.ID)
	if !ok || ws.Plan == nil {
		return false
	}
	ids, _ := inFlightChildren(*ws.Plan)
	if len(ids) > 0 {
		return true
	}
	for _, s := range ws.Plan.Steps {
		if s.Status == domain.StepDone || s.Status == domain.StepSkipped {
			return true
		}
	}
	return false
}

func (l *Loop) landedBoardHandoff() string {
	if l.eng == nil {
		return ""
	}
	var parts []string
	for _, c := range l.eng.Store.Workspaces() {
		if c.ParentID != l.ID || c.FanOut == nil {
			continue
		}
		if c.FanOut.Outcome != "merged" && !strings.HasPrefix(c.FanOut.Outcome, "conflict") {
			continue
		}
		note := strings.TrimSpace(c.FanOut.Note)
		if note == "" {
			continue
		}
		if ids := strings.Join(c.ParentStepIDs, ", "); ids != "" {
			parts = append(parts, ids+": "+note)
		} else {
			parts = append(parts, note)
		}
	}
	return strings.Join(parts, "\n")
}

func hasPendingSteps(p domain.Plan) bool {
	for _, s := range p.Steps {
		if s.Status == domain.StepPending || s.Status == domain.StepDoing {
			return true
		}
	}
	return false
}
