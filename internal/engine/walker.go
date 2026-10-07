package engine

import (
	"context"
	"errors"
	"fmt"
	"html"
	"strings"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
)

var (
	errVerifyRejected = errors.New("verify rejected")
	errPhaseFailed    = errors.New("phase failed")
)

const stateAdvance domain.WorkspaceState = "advance"

func (l *Loop) walkLoop(ctx context.Context) {
	start := l.guard.Loop.Entry
	if id := strings.TrimSpace(l.startPhase); id != "" {
		start = id
	} else if !l.freshWalk {
		if ws, ok := l.eng.Store.Workspace(l.ID); ok && ws.Checkpoint != nil {
			if id := strings.TrimSpace(ws.Checkpoint.PhaseID); id != "" {
				if _, exists := l.guard.Loop.PhaseByID(id); !exists {
					l.emitFeed("MA", "Maestro", "Checkpoint phase "+html.EscapeString(id)+" is gone — idling.")
					l.setState(domain.StatePaused, "phase missing")
					return
				}
				start = id
			}
			if rid := strings.TrimSpace(ws.Checkpoint.ResumePhaseID); rid != "" && l.resumePhaseID == "" {
				l.resumePhaseID = rid
			}
			if ph, ok := l.guard.Loop.PhaseByID(start); ok && ph.Dispatch == "on_demand" {
				l.onDemandWalk = true
			}
		}
	}

	steps := 0
	id := start
	for {
		if !l.checkpoint(ctx) {
			l.finishWalkAbort(ctx)
			return
		}
		if pending := l.takePendingOnDemand(); pending != "" {
			l.resumePhaseID = id
			l.onDemandWalk = true
			l.eng.Store.UpdateWorkspace(l.ID, func(ws *domain.Workspace) {
				if ws.Checkpoint == nil {
					ws.Checkpoint = &domain.Checkpoint{}
				}
				ws.Checkpoint.PhaseID = pending
				ws.Checkpoint.ResumePhaseID = id
			})
			id = pending
			continue
		}
		ph, ok := l.guard.Loop.PhaseByID(id)
		if !ok {
			l.emitFeed("MA", "Maestro", "Phase "+html.EscapeString(id)+" is gone — idling.")
			l.setState(domain.StatePaused, "phase missing")
			return
		}
		if ph.Dispatch == "on_demand" && !l.onDemandWalk {
			l.emitFeed("MA", "Maestro", "Skipping on-demand phase "+html.EscapeString(ph.ID)+".")
			id = ""
			if l.resumePhaseID != "" {
				id = l.resumePhaseID
				l.resumePhaseID = ""
			}
			if id == "" {
				l.setState(domain.StatePaused, "idle")
				return
			}
			continue
		}
		l.setWalkPhase(ph.ID)
		l.noteThresholdOverride(ph)
		next, done := l.runLoopPhase(ctx, ph)
		if done {
			l.finishStoppedIfInFlight()
			return
		}
		steps++
		if steps >= bundles.WalkStepCap {
			l.emitFeed("MA", "Maestro", "Walk-step cap reached — escalating.")
			st, more := l.escalateAtCapApply(ctx, steps, l.eng.Store.OpenFindingsCount(l.ID))
			if more == 0 && st != domain.StatePROpen {
				l.finishWalkAbort(ctx)
				return
			}
			steps = 0
			if st == domain.StatePROpen {
				if pr := l.ensurePRPhaseID(); pr != "" {
					next = pr
					l.setWalkPhase(pr)
				}
			}
			l.markCapAnswerApplied()
		}
		if next == "" {
			if l.onDemandWalk && l.resumePhaseID != "" {
				id = l.resumePhaseID
				l.resumePhaseID = ""
				l.onDemandWalk = false
				continue
			}
			l.setState(domain.StatePaused, "idle")
			return
		}
		id = next
	}
}

func (l *Loop) runLoopPhase(ctx context.Context, ph bundles.LoopPhase) (next string, done bool) {
	attempts := 1 + l.phaseRetries(ph)
	var lastErr error
	for i := 0; i < attempts; i++ {
		// Parent ctx only — policy timeout wraps each specialist turn, not
		// plan-gate / cap / Pause waits.
		next, done, err := l.runPhaseAttempt(ctx, ph)
		if ctx.Err() != nil {
			l.finishWalkAbort(ctx)
			return next, true
		}
		if err == nil {
			return next, done
		}
		lastErr = err
		if i+1 < attempts {
			l.emitFeed("MA", "Maestro", fmt.Sprintf(
				"Phase %s failed (%s) — retrying.", html.EscapeString(ph.ID), html.EscapeString(err.Error())))
		}
	}
	if lastErr != nil {
		return l.takeOnFail(ctx, ph, lastErr)
	}
	return next, done
}

func (l *Loop) runPhaseAttempt(ctx context.Context, ph bundles.LoopPhase) (next string, done bool, err error) {
	l.mu.Lock()
	l.currentPhase = ph
	l.mu.Unlock()
	switch ph.Uses {
	case bundles.VerbPlan:
		l.setState(domain.StatePlanning, "decomposing the task")
		if l.adoptExistingPlan() {
			// already have a plan
		} else {
			if perr := l.runPlanner(ctx); perr != nil {
				return "", false, perr
			}
		}
		if n, stop := l.gatePlanApproval(ctx); stop {
			return string(n), true, nil
		}
		return ph.Next, false, nil

	case bundles.VerbConverge:
		st, cerr := l.converge(ctx)
		if cerr != nil {
			return "", false, cerr
		}
		if st == domain.StateStopped {
			l.finishStoppedIfInFlight()
			return string(st), true, nil
		}
		if st == domain.StateFailed {
			return string(st), true, nil
		}
		if st == domain.StatePaused {
			return string(st), true, nil
		}
		if st == domain.StatePROpen {
			if n := strings.TrimSpace(ph.Next); n != "" {
				l.setWalkPhase(n)
			}
			l.markCapAnswerApplied()
		}
		return ph.Next, false, nil

	case bundles.VerbImplement:
		l.setState(domain.StateCoding, "implementing")
		if cerr := l.runCoder(ctx, 1, nil); cerr != nil {
			return "", false, cerr
		}
		return ph.Next, false, nil

	case bundles.VerbReview:
		l.setState(domain.StateReviewing, "reviewing")
		if _, rerr := l.runReviewer(ctx, 1); rerr != nil {
			return "", false, rerr
		}
		return ph.Next, false, nil

	case bundles.VerbEnsurePR:
		th := l.guard.phaseThreshold(ph)
		if blocked, reason := l.eng.findingsBarBlocks(l.ID, th); blocked {
			l.emitFeed("MA", "Maestro", "Policy bar blocked the PR: "+html.EscapeString(reason))
			iter := 0
			if ws, ok := l.eng.Store.Workspace(l.ID); ok {
				iter = ws.Iteration
			}
			open := l.eng.Store.OpenFindingsCount(l.ID)
			question := fmt.Sprintf("Policy bar blocked the PR: %s. How should I proceed?", reason)
			st, more := l.escalateAtCapApplyMsg(ctx, iter, open, question, true)
			if more > 0 {
				if l.rewindEnsurePR(ctx) {
					return "", true, nil
				}
				l.markCapAnswerApplied()
				return l.inboundGraphPhase(ph.ID), false, nil
			}
			if st != domain.StatePROpen {
				l.finishWalkAbort(ctx)
				return "", true, nil
			}
			// "Ship with caveats" — fall through and open the PR.
		}
		if _, oerr := l.openPR(ctx); oerr != nil && ctx.Err() == nil {
			l.emitFeed("MA", "Maestro", "Could not open the PR: "+html.EscapeString(oerr.Error()))
		}
		l.eng.markGroup(l.ID, domain.GroupReady)
		l.setState(domain.StatePROpen, "PR open — ready for your review")
		if n := strings.TrimSpace(ph.Next); n != "" {
			l.setWalkPhase(n)
		}
		l.markCapAnswerApplied()
		return ph.Next, ph.Next == "", nil

	case bundles.VerbVerify:
		l.setState(domain.StateVerifying, "verifying")
		passed, verr := l.runVerify(ctx)
		if verr != nil {
			return "", false, verr
		}
		if passed {
			return ph.Next, false, nil
		}
		if ph.OnReject != "" {
			return ph.OnReject, false, nil
		}
		l.fail(ctx, errVerifyRejected)
		return "", true, nil

	case bundles.VerbArtifact:
		l.setState(domain.StatePaused, "artifact")
		return ph.Next, ph.Next == "", nil

	default:
		l.emitFeed("MA", "Maestro", "Unknown verb "+html.EscapeString(ph.Uses))
		return "", true, nil
	}
}

func (l *Loop) phaseRetries(ph bundles.LoopPhase) int {
	if ph.Retries != nil && *ph.Retries >= 0 {
		return *ph.Retries
	}
	if l.guard.Policy.Retries > 0 {
		return l.guard.Policy.Retries
	}
	return 0
}

func (l *Loop) phaseContext(ctx context.Context, ph bundles.LoopPhase) (context.Context, context.CancelFunc) {
	raw := strings.TrimSpace(ph.Timeout)
	if raw == "" {
		raw = strings.TrimSpace(l.guard.Policy.Timeout)
	}
	if raw == "" {
		return ctx, func() {}
	}
	d, err := time.ParseDuration(raw)
	if err != nil || d <= 0 {
		return ctx, func() {}
	}
	return context.WithTimeout(ctx, d)
}

// turnContext applies the policy/phase timeout to one specialist turn.
func (l *Loop) turnContext(ctx context.Context) (context.Context, context.CancelFunc) {
	if l == nil {
		return ctx, func() {}
	}
	l.mu.Lock()
	ph := l.currentPhase
	l.mu.Unlock()
	return l.phaseContext(ctx, ph)
}

func (l *Loop) ensurePRPhaseID() string {
	for _, p := range l.guard.Loop.Phases {
		if p.Dispatch == "on_demand" {
			continue
		}
		if p.Uses == bundles.VerbEnsurePR {
			return p.ID
		}
	}
	return ""
}

func (l *Loop) inboundGraphPhase(id string) string {
	for _, p := range l.guard.Loop.Phases {
		if p.Dispatch == "on_demand" {
			continue
		}
		if p.Next == id {
			return p.ID
		}
	}
	return ""
}

func (l *Loop) takePendingOnDemand() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	id := l.pendingOnDemand
	l.pendingOnDemand = ""
	return id
}

func (l *Loop) queueOnDemand(id string) {
	l.mu.Lock()
	l.pendingOnDemand = strings.TrimSpace(id)
	l.mu.Unlock()
}

func (l *Loop) takeOnFail(ctx context.Context, ph bundles.LoopPhase, err error) (string, bool) {
	action := ph.OnFail
	if action == "" {
		if ph.Dispatch == "on_demand" {
			action = "skip"
		} else {
			action = l.guard.Policy.OnFail
		}
	}
	if action == "" {
		action = "fail"
	}
	if err == nil {
		err = errPhaseFailed
	}
	switch action {
	case "skip":
		if ph.Dispatch == "on_demand" {
			return "", false
		}
		return ph.Next, false
	case "escalate":
		iter := 0
		if ws, ok := l.eng.Store.Workspace(l.ID); ok {
			iter = ws.Iteration
		}
		_, more := l.escalateAtCapApply(ctx, iter, l.eng.Store.OpenFindingsCount(l.ID))
		if more == 0 {
			l.finishWalkAbort(ctx)
			return "", true
		}
		l.markCapAnswerApplied()
		return ph.ID, false
	default:
		l.fail(ctx, err)
		return "", true
	}
}

func (l *Loop) finishStoppedIfInFlight() {
	// Walk-local outcome, not the workspace row: a sibling walk can overwrite
	// that to in-flight between ensure_pr/artifact and this call.
	if loopSettled(l.exitState, l.exitPhase) {
		return
	}
	ws, ok := l.eng.Store.Workspace(l.ID)
	if !ok {
		return
	}
	switch ws.State {
	case domain.StateStopped, domain.StateFailed, domain.StateMerged, domain.StatePROpen:
		return
	case domain.StatePaused:
		if ws.Phase == "idle" || ws.Phase == "artifact" || ws.Phase == "phase missing" || ws.Phase == "loop refs missing" || ws.Phase == boardCapPhase {
			return
		}
	}
	l.finishStopped(context.Background())
}

func (l *Loop) noteThresholdOverride(ph bundles.LoopPhase) {
	if ph.Findings == nil {
		return
	}
	pol := l.guard.Policy.FindingsThreshold
	if err := bundles.SameOrStricter(*ph.Findings, pol); err == nil {
		return
	}
	l.emitFeed("MA", "Maestro", fmt.Sprintf(
		"Phase %s findings_threshold is looser than policy %s — using the policy bar.",
		html.EscapeString(ph.ID), html.EscapeString(l.guard.Policy.ID)))
}

func (l *Loop) setWalkPhase(id string) {
	l.eng.Store.UpdateWorkspace(l.ID, func(ws *domain.Workspace) {
		if ws.Checkpoint == nil {
			ws.Checkpoint = &domain.Checkpoint{}
		}
		ws.Checkpoint.PhaseID = id
	})
}
