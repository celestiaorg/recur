package engine

import (
	"context"
	"fmt"
	"html"
	"strings"
	"sync"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	"recur/internal/mcp"
)

// escalationOutcome is what a blocked-on-escalation loop wakes up with.
type escalationOutcome int

const (
	escAnswered escalationOutcome = iota // the human answered; resume with l.answer
	escStopped                           // Stop or ctx-cancel woke the loop; caller unwinds
)

// escalate raises an escalation and BLOCKS the loop goroutine at a safe checkpoint
// until the human answers (PRD §13, §6.2: "escalation is just a pause whose resume
// payload is the human's answer"). It is called by the engine reporter (the
// escalate MCP tool) and directly by the convergence loop on rail #4 (loop
// detection at the cap). Steps:
//  1. persist a domain.Escalation + set the loop state=escalated, group=needs;
//  2. emit escalation.raised + an inbox-worthy event (so "needs you" lights up);
//  3. block on the loop's cond until CmdAnswerEscalation arrives (or Stop/cancel),
//     reusing the SAME cond/cmds machinery as Pause — no busy-wait, and a Stop or
//     engine Shutdown (ctx cancel) still wakes it.
//
// It returns the resolved answer (the human's single-line decision) and whether the
// loop should keep running (false if Stop/cancel won the race instead of an answer).
func (l *Loop) escalate(ctx context.Context, req mcp.EscalationRequest, kind string) (answer EscalationAnswer, keepGoing bool) {
	// Short-circuit if the loop is already stopping (Stop won the race, or the ctx
	// was cancelled): a loop about to exit must NOT raise a dangling escalation. Doing
	// the raise+emit+flip-to-needs here would briefly light up "needs you" and add a
	// stale inbox row for a loop that immediately stops (a raised-then-stopped flicker
	// + an orphaned actionable row). Bail before any side effect so the loop just
	// finishes stopped (keepGoing=false → the caller returns and Run finishes stopped).
	l.mu.Lock()
	stopping := l.stopped
	var pending EscalationAnswer
	hasPending := l.answer != nil
	if hasPending {
		pending = *l.answer
		l.answer = nil
		l.escalated = false
	}
	l.mu.Unlock()
	if stopping || ctx.Err() != nil {
		return EscalationAnswer{}, false
	}
	// Answer already delivered (AnswerEscalation after restart seeded the new
	// goroutine). Do not re-raise; the caller applies this answer.
	if hasPending {
		return pending, true
	}

	esc := l.eng.raiseEscalation(l.ID, l.ChatCode, req, kind)

	// Mark escalated + move to "needs you" BEFORE blocking, so an observer that
	// reacts to escalation.raised sees a consistent state.
	l.mu.Lock()
	l.escalated = true
	l.answer = nil
	l.mu.Unlock()

	// Emit escalation.raised BEFORE the terminal state transition so an observer that
	// keys off state==escalated always finds the raised event already published
	// (they run sequentially on this goroutine; the ordering removes the race window).
	l.eng.publishEscalationRaised(l.ID, esc)
	l.eng.markGroup(l.ID, domain.GroupNeeds)
	l.setState(domain.StateEscalated, "asking a question")
	l.eng.publishResource()

	switch l.awaitAnswer(ctx) {
	case escAnswered:
		l.mu.Lock()
		ans := EscalationAnswer{}
		if l.answer != nil {
			ans = *l.answer
		}
		l.answer = nil
		l.mu.Unlock()
		return ans, true
	default:
		// Stop / composer Stop — not Shutdown. Leave the row open on quit so
		// Recover can still surface the unanswered question after restart.
		l.mu.Lock()
		explicit := l.stopped || l.parkIdle
		l.mu.Unlock()
		if explicit {
			l.eng.retireWalkEscalation(l.ID, l.ChatCode)
		}
		return EscalationAnswer{}, false
	}
}

// awaitAnswer blocks the loop on its cond until an escalation answer arrives, or
// Stop/ctx-cancel wakes it (PRD §6.2 — no busy-wait). A one-shot ctx watcher
// broadcasts the cond on cancel so an app Shutdown or Stop while blocked-on-
// escalation wakes the loop promptly (mirrors checkpoint's watcher).
func (l *Loop) awaitAnswer(ctx context.Context) escalationOutcome {
	stop := context.AfterFunc(ctx, func() {
		l.mu.Lock()
		l.cond.Broadcast()
		l.mu.Unlock()
	})
	defer stop()

	l.mu.Lock()
	defer l.mu.Unlock()
	for l.escalated && !l.stopped && ctx.Err() == nil {
		l.cond.Wait()
	}
	if l.stopped || ctx.Err() != nil {
		return escStopped
	}
	return escAnswered
}

// ---- engine side: raise + deliver + emit ----

// raiseEscalation builds, persists, and records a domain.Escalation for a loop
// (PRD §13, §14). The ENGINE stamps the id/status/timing — the agent supplies only
// the human-facing content. It also adds an actionable "needs you" inbox row
// (PRD §16.11) and bumps the escalation-precision instrumentation counter (§13).
func (e *Engine) raiseEscalation(workspaceID, agentCode string, req mcp.EscalationRequest, kind string) domain.Escalation {
	opts := make([]domain.EscalationOption, 0, len(req.Options))
	for _, o := range req.Options {
		opts = append(opts, domain.EscalationOption{Text: o.Text, Tag: o.Tag, Desc: o.Desc})
	}
	esc := domain.Escalation{
		ID:          fmt.Sprintf("esc-%s-%d", workspaceID, time.Now().UnixNano()),
		WorkspaceID: workspaceID,
		AgentCode:   strings.TrimSpace(agentCode),
		Question:    req.Question,
		Context:     req.Context,
		Options:     opts,
		Status:      "open",
		Kind:        kind,
		AskedAgo:    "now",
	}
	e.Store.AddEscalation(esc)
	e.escPrecision.raised()

	// Activity feed line (also lands in the durable log).
	e.emitWorkspaceEvent(workspaceID, "MA", "Maestro",
		"Paused to ask: "+html.EscapeString(truncate(req.Question, 160)))

	// Actionable inbox row (PRD §16.11, D5: needs-you is actionable-only).
	short := workspaceID
	if l, ok := e.Store.Workspace(workspaceID); ok && l.Short != "" {
		short = l.Short
	}
	e.Store.AddInboxItem(domain.InboxItem{
		ID:          "ib-esc-" + workspaceID,
		Group:       "needs",
		Unread:      true,
		Kind:        "pause",
		Color:       "var(--coral)",
		Title:       fmt.Sprintf("<b>%s</b> paused — needs your decision", html.EscapeString(workspaceID)),
		Sub:         html.EscapeString(truncate(short, 64)) + " · " + html.EscapeString(truncate(req.Question, 64)),
		Time:        "now",
		Action:      "Answer",
		ActionClass: "urgent",
		Target:      "workspace:" + workspaceID,
		WorkspaceID: workspaceID,
	})
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})

	// Generic "notify" bus event (Phase 3.6): main.go turns this into a native OS
	// notification so a backgrounded window still surfaces "a loop needs you". The
	// engine stays Wails-free — it just publishes the event.
	e.Bus.Publish(Event{Name: "notify", Data: map[string]any{
		"title":  "A loop needs you",
		"body":   truncate(req.Question, 120),
		"target": "workspace:" + workspaceID,
	}})
	return esc
}

// publishEscalationRaised persists + emits escalation.raised (PRD §15.2). The
// payload carries the full escalation so the frontend can render the live question
// immediately (it also re-fetches via GetEscalation, but the event saves a round-trip).
func (e *Engine) publishEscalationRaised(workspaceID string, esc domain.Escalation) {
	payload := map[string]any{"workspaceId": workspaceID, "escalation": esc}
	e.persistEvent(workspaceID, "", "escalation.raised", payload)
	e.Bus.Publish(Event{Name: "escalation.raised", Data: payload})
}

// AnswerEscalation delivers the human's answer to the loop and resumes it (PRD §13,
// §15.1). It persists answer + status=answered, clears the "needs you" inbox row,
// emits escalation.resolved, and DELIVERS Command{CmdAnswerEscalation} to the
// running loop's command channel so it resumes from its blocked checkpoint. If the
// owning walk is not currently running (e.g. interrupted on restart, including a
// chat-owned row with AgentCode set), it re-dispatches that walk via the M2 Resume
// path first, then delivers — but note: a re-dispatched loop restarts from the
// durable record (v1 does not replay a killed claude turn, PRD §6.5), so for the
// common live case the loop is running and resumes mid-flight.
//
// Returns the answered escalation, or an empty one if there was no escalation to
// answer (so the bound service can report nothing-to-do honestly).
func (e *Engine) AnswerEscalation(workspaceID, choice, freeText string) (domain.Escalation, bool) {
	return e.AnswerEscalationFor(workspaceID, "", choice, freeText)
}

// AnswerEscalationFor answers the escalation owned by agentCode. Empty agentCode
// keeps the workspace-only fallback (one open row, or the lex-first of several).
func (e *Engine) AnswerEscalationFor(workspaceID, agentCode, choice, freeText string) (domain.Escalation, bool) {
	return e.answerEscalationTo(workspaceID, nil, agentCode, EscalationAnswer{Choice: choice, FreeText: freeText})
}

// answerEscalationTo persists the workspace escalation and delivers to lp when
// set (extend_rounds on a chat-suffixed walk). A nil lp delivers to agentCode,
// then esc.AgentCode.
func (e *Engine) answerEscalationTo(workspaceID string, lp *Loop, agentCode string, ans EscalationAnswer) (domain.Escalation, bool) {
	resolved := strings.TrimSpace(ans.FreeText)
	if resolved == "" {
		resolved = strings.TrimSpace(ans.Choice)
	}
	ans.Resolved = resolved

	chat := strings.TrimSpace(agentCode)
	if lp != nil {
		if c := strings.TrimSpace(lp.ChatCode); c != "" {
			chat = c
		}
	}
	esc, ok := e.Store.AnswerEscalationFor(workspaceID, chat, ans.Choice, ans.FreeText)
	if !ok {
		// Already answered (idempotent no-op at the root) or no escalation to answer.
		// Either way: do NOT deliver an answer or re-dispatch — a second answer must
		// never resurrect a loop that already settled on the first one.
		return domain.Escalation{}, false
	}
	resolved = esc.Answer

	e.escPrecision.answered()

	stillOpen := e.Store.HasOpenEscalation(workspaceID)
	if !stillOpen {
		e.Store.RemoveInboxByTarget("workspace:" + workspaceID)
		e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
		e.markGroup(workspaceID, domain.GroupActive)
	}

	cmd := Command{Kind: CmdAnswerEscalation, Answer: ans}

	if lp == nil {
		if code := strings.TrimSpace(esc.AgentCode); code != "" {
			lp = e.runningLoopByChat(workspaceID, code)
		} else {
			lp = e.runningLoopByID(workspaceID)
		}
	}
	if lp != nil {
		// Live case: the loop goroutine is blocked in awaitAnswer — deliver the answer
		// so it resumes from the checkpoint. Clear state=escalated now; the loop will
		// set the real next phase on its next setState (otherwise the UI stays stuck
		// on "Needs you" with no Resume until that happens).
		lp.send(cmd)
		if !stillOpen {
			if rec, found := e.Store.Workspace(workspaceID); found && rec.State == domain.StateEscalated {
				e.setWorkspaceState(workspaceID, domain.StateCoding, "resuming")
			}
		}
	} else if !e.resumeAfterAnswer(workspaceID, strings.TrimSpace(esc.AgentCode), cmd) {
		// Not running (interrupted on restart, chat-owned crash recovery, or the
		// seeded demo): resumeAfterAnswer re-dispatches the owning walk and BAILS on
		// a terminal loop or a live sibling. No goroutine started: don't claim
		// "coding". Leave a non-terminal loop paused so Resume can pick it up —
		// unless another chat's question is still open (workspace stays escalated).
		if strings.TrimSpace(esc.AgentCode) == "" || !stillOpen {
			if rec, found := e.Store.Workspace(workspaceID); found && !terminal(rec.State) {
				e.setWorkspaceState(workspaceID, domain.StatePaused, "answered — waiting to resume")
			}
		}
	}

	e.emitWorkspaceEvent(workspaceID, "MA", "Maestro", "Answer received: "+html.EscapeString(truncate(resolved, 160))+" — resuming.")
	e.Bus.Publish(Event{Name: "escalation.resolved", Data: map[string]any{
		"workspaceId": workspaceID, "answer": resolved, "agentCode": esc.AgentCode, "open": stillOpen,
	}})
	e.publishResource()
	return esc, true
}

// retireWalkEscalation closes this chat's open question without delivering an
// answer (Stop / composer Stop). A sibling walk's unanswered row stays open.
// Empty agentCode is the lex-first open row (workspace-scoped / UI fallback).
func (e *Engine) retireWalkEscalation(workspaceID, agentCode string) bool {
	esc, ok := e.Store.AnswerEscalationFor(workspaceID, agentCode, "cancelled", "")
	if !ok {
		return false
	}
	stillOpen := e.Store.HasOpenEscalation(workspaceID)
	if !stillOpen {
		e.Store.RemoveInboxByTarget("workspace:" + workspaceID)
		e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
		e.markGroup(workspaceID, domain.GroupActive)
	}
	e.Bus.Publish(Event{Name: "escalation.resolved", Data: map[string]any{
		"workspaceId": workspaceID, "answer": "cancelled", "agentCode": esc.AgentCode, "open": stillOpen,
	}})
	return true
}

// retireOpenEscalations closes every unanswered question on this workspace.
// Used by not-running Stop: Recover left the rows open with no goroutine.
func (e *Engine) retireOpenEscalations(workspaceID string) {
	for e.retireWalkEscalation(workspaceID, "") {
	}
}

// resumeAfterAnswer best-effort re-dispatches a not-running loop after its
// escalation was answered. A newly dispatched goroutine is not blocked in
// awaitAnswer; newLoop seeds the persisted answer so Run applies it. agentCode
// scopes the lookup to the owning chat (empty = workspace walk). It returns
// true only if a goroutine is actually running for that walk so the caller can
// keep the UI honest — a not-running answer must not claim "coding" when
// nothing started.
//
// Unlike Resume, a dispatch failure here does NOT mark the loop failed (the seeded
// demo loop has no real worktree to re-dispatch and that is fine). Serialized with
// the dispatch machinery via dispatchMu (D26) so it can't double-dispatch against a
// concurrent Resume, and — crucially — it RE-READS the loop record under the lock
// and BAILS if the loop is terminal (pr_open/merged/failed/stopped), so a late or
// duplicate answer can never resurrect a settled loop (mirrors Stop/Resume's
// terminal() TOCTOU guard). A live sibling walk is left alone: sending the answer
// there or calling dispatch() would clobber it.
func (e *Engine) resumeAfterAnswer(workspaceID, agentCode string, cmd Command) (started bool) {
	e.dispatchMu.Lock()
	if lp := e.runningLoopForHand(workspaceID, agentCode); lp != nil {
		e.dispatchMu.Unlock()
		lp.send(cmd)
		return true
	}
	if strings.TrimSpace(agentCode) != "" && e.runningLoopByID(workspaceID) != nil {
		e.dispatchMu.Unlock()
		return false
	}
	rec, ok := e.Store.Workspace(workspaceID)
	if !ok {
		e.dispatchMu.Unlock()
		return false
	}
	// Never resurrect a loop that already settled (the second-answer / late-answer
	// race): a re-read under dispatchMu sees a Stop/finish that made it terminal.
	if terminal(rec.State) {
		e.dispatchMu.Unlock()
		return false
	}
	rec = e.chatScopedWorkspace(rec, agentCode)
	// Only attempt a real re-dispatch if the project + repo are present (an actual
	// interrupted loop). The seeded demo escalation points at a fake repo; skip it.
	if proj, ok := e.Store.Project(rec.ProjectID); !ok || proj.RepoPath == "" {
		e.dispatchMu.Unlock()
		return false
	}
	// Identity lives in rec until dispatch's AddWorkspace. Persist now so a
	// no-slot / dispatch-miss Resume reloads this chat, not the sibling owner.
	if strings.TrimSpace(agentCode) != "" {
		e.Store.UpdateWorkspace(rec.ID, func(l *domain.Workspace) {
			if c := strings.TrimSpace(rec.LoopChatCode); c != "" {
				l.LoopChatCode = c
			}
			if id := strings.TrimSpace(rec.LoopID); id != "" {
				l.LoopID = id
			}
			if id := strings.TrimSpace(rec.PolicyID); id != "" {
				l.PolicyID = id
			}
			l.ReplaceGoal(rec.Goal, rec.GoalDisplay)
			l.Checkpoint = rec.Checkpoint
		})
	}
	if !e.gov.TryAcquireLoop() {
		e.dispatchMu.Unlock()
		return false // No slot; leave it for the drain in an honest resting state.
	}
	err := e.dispatch(rec)
	e.dispatchMu.Unlock()
	if err != nil {
		e.gov.ReleaseLoop()
		return false // best-effort: don't downgrade to failed on a re-dispatch miss
	}
	// The new goroutine is not blocked in awaitAnswer — newLoop seeds l.answer
	// from the persisted record so Run applies it (sending CmdAnswerEscalation
	// here would only leave a stale answer after the first consume).
	return true
}

// applyAnsweredCap applies a persisted answer that arrived after restart, when
// this goroutine is not sitting in awaitAnswer. Returns true if the loop is
// finished (Stop, Ship, or idle with no next). Keep-iterating on a round
// cap grants extra rounds; keep-iterating on the ensure_pr door rewinds to the
// inbound graph phase. A converge cap ship follows ph.Next (live
// runPhaseAttempt). Plan notes are folded here;
// agent MCP answers are steered into the next turn so they cannot poison a
// later escalate().
func (l *Loop) applyAnsweredCap(ctx context.Context) bool {
	l.mu.Lock()
	has := l.answer != nil
	l.mu.Unlock()
	if !has {
		return false
	}
	esc, ok := l.eng.Store.Escalation(l.ID)
	if !ok {
		l.clearAnswer()
		return false
	}
	kind := persistedEscalationKind(esc)
	switch kind {
	case escKindCap, escKindEnsurePR:
		iter := 1
		if l.resume != nil && l.resume.Iteration > 0 {
			iter = l.resume.Iteration
		}
		door := kind == escKindEnsurePR
		next, more := l.escalateAtCapApplyMsg(ctx, iter, l.eng.Store.OpenFindingsCount(l.ID), "", door)
		if more > 0 {
			if door {
				if l.rewindEnsurePR(ctx) {
					return true
				}
				l.markCapAnswerApplied()
				return false
			}
			l.grantExtension(more)
			return false
		}
		if next == domain.StatePROpen {
			if !door {
				if ph, ok := l.walkPhase(); ok && ph.Uses == bundles.VerbConverge {
					if n := strings.TrimSpace(ph.Next); n != "" {
						l.setWalkPhase(n)
						l.markCapAnswerApplied()
						return false
					}
					l.setState(domain.StatePaused, "idle")
					l.markCapAnswerApplied()
					return true
				}
			}
			// "Ship with caveats" after a restart: open the PR here (the live
			// walker would have run ensure_pr), then follow its next edge.
			if _, err := l.openPR(ctx); err != nil && ctx.Err() == nil {
				l.emitFeed("MA", "Maestro", "Could not open the PR: "+html.EscapeString(err.Error()))
			}
			l.eng.markGroup(l.ID, domain.GroupReady)
			l.setState(domain.StatePROpen, "PR open — ready for your review")
			if door && l.continueEnsurePRGraph() {
				l.markCapAnswerApplied()
				return false
			}
			l.markCapAnswerApplied()
		}
		return true
	case escKindPlan:
		ans := l.takeAnswer()
		if t := strings.TrimSpace(ans.FreeText); t != "" {
			l.foldPlanNotes(t)
		}
		_ = l.eng.ApprovePlan(l.ID)
		l.eng.Store.MarkEscalationApplied(l.ID)
		return false
	}
	// Agent MCP: consume so a later escalate() cannot steal this answer.
	// The original pendingEsc call site died with the pre-restart process.
	// Leave Applied false until applyPendingSteer puts the guidance in a prompt.
	ans := l.takeAnswer()
	guidance := strings.TrimSpace(ans.FreeText)
	if guidance == "" {
		guidance = strings.TrimSpace(ans.Resolved)
	}
	if guidance == "" {
		guidance = strings.TrimSpace(ans.Choice)
	}
	if guidance != "" {
		l.queueSteer(guidance)
	} else {
		l.eng.Store.MarkEscalationApplied(l.ID)
	}
	return false
}

func (l *Loop) takeAnswer() EscalationAnswer {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.answer == nil {
		return EscalationAnswer{}
	}
	ans := *l.answer
	l.answer = nil
	return ans
}

func (l *Loop) clearAnswer() {
	l.mu.Lock()
	l.answer = nil
	l.mu.Unlock()
}

const (
	escKindAgent    = "agent"
	escKindCap      = "cap"
	escKindEnsurePR = "ensure_pr"
	escKindPlan     = "plan"
)

// persistedEscalationKind returns the engine-stamped kind, falling back to
// option-text heuristics only for rows raised before Kind existed.
func persistedEscalationKind(esc domain.Escalation) string {
	if k := strings.TrimSpace(esc.Kind); k != "" {
		return k
	}
	if capEscalation(esc) {
		if ensurePRDoorEscalation(esc) {
			return escKindEnsurePR
		}
		return escKindCap
	}
	if planApprovalEscalation(esc) {
		return escKindPlan
	}
	return escKindAgent
}

// shouldSeedEscalationAnswer is true when a new goroutine must apply a persisted
// answer. Legacy JSON defaulted missing applied to true; Recover's stuck-answer
// states (escalated, or paused waiting to resume) still have a pending decision.
func shouldSeedEscalationAnswer(ws domain.Workspace, esc domain.Escalation) bool {
	if esc.Status != "answered" {
		return false
	}
	if !esc.Applied {
		return true
	}
	if ws.State == domain.StateEscalated {
		return true
	}
	return ws.State == domain.StatePaused && ws.Phase == "answered — waiting to resume"
}

// restoreEscalationAnswer rebuilds the in-memory answer shape from the durable
// record. Old rows only stored Answer; those map onto Choice when the text
// matches an offered option, otherwise FreeText, so classifyCapAnswer and plan
// notes still see the original shape.
func restoreEscalationAnswer(esc domain.Escalation) *EscalationAnswer {
	choice := strings.TrimSpace(esc.Choice)
	free := strings.TrimSpace(esc.FreeText)
	resolved := strings.TrimSpace(esc.Answer)
	if choice == "" && free == "" && resolved == "" {
		return nil
	}
	if choice == "" && free == "" && resolved != "" {
		if escalationChoiceMatch(esc, resolved) {
			choice = resolved
		} else {
			free = resolved
		}
	}
	return &EscalationAnswer{Choice: choice, FreeText: free, Resolved: resolved}
}

func escalationChoiceMatch(esc domain.Escalation, resolved string) bool {
	want := strings.ToLower(strings.TrimSpace(resolved))
	for _, o := range esc.Options {
		if strings.ToLower(strings.TrimSpace(o.Text)) == want {
			return true
		}
	}
	return false
}

// capEscalation reports whether this is a rail-#4 cap question (the engine's
// own option text — same strings classifyCapAnswer keys off).
func capEscalation(esc domain.Escalation) bool {
	for _, o := range esc.Options {
		switch strings.ToLower(strings.TrimSpace(o.Text)) {
		case "keep iterating", "ship with caveats":
			return true
		}
	}
	return false
}

// ensurePRDoorEscalation reconstructs the ensure_pr policy-bar raise
// (shipAnyway=true + inbound continuation). That call site writes a unique
// question prefix and "anyway" ship option; round-cap raises do neither.
func ensurePRDoorEscalation(esc domain.Escalation) bool {
	if strings.HasPrefix(esc.Question, "Policy bar blocked the PR:") {
		return true
	}
	for _, o := range esc.Options {
		if strings.EqualFold(strings.TrimSpace(o.Text), "Ship with caveats") &&
			strings.Contains(strings.ToLower(o.Desc), "anyway") {
			return true
		}
	}
	return false
}

// rewindEnsurePR sends a keep-iterating/guidance answer back to the inbound
// graph phase, matching the live ensure_pr path. Returns true if the loop
// should stop (no inbound phase to return to).
func (l *Loop) rewindEnsurePR(ctx context.Context) bool {
	prev := l.inboundGraphPhase(l.checkpointEnsurePRPhaseID())
	if prev == "" {
		l.finishWalkAbort(ctx)
		return true
	}
	l.setWalkPhase(prev)
	return false
}

func (l *Loop) checkpointEnsurePRPhaseID() string {
	id := l.ensurePRPhaseID()
	if ws, ok := l.eng.Store.Workspace(l.ID); ok && ws.Checkpoint != nil {
		if cid := strings.TrimSpace(ws.Checkpoint.PhaseID); cid != "" {
			if ph, exists := l.guard.Loop.PhaseByID(cid); exists && ph.Uses == bundles.VerbEnsurePR {
				return cid
			}
		}
	}
	return id
}

// continueEnsurePRGraph follows the checkpointed ensure_pr phase's next edge
// after a restarted "Ship with caveats" opens the PR, matching the live walker.
func (l *Loop) continueEnsurePRGraph() bool {
	ph, ok := l.guard.Loop.PhaseByID(l.checkpointEnsurePRPhaseID())
	if !ok || strings.TrimSpace(ph.Next) == "" {
		return false
	}
	l.setWalkPhase(ph.Next)
	return true
}

func planApprovalEscalation(esc domain.Escalation) bool {
	for _, o := range esc.Options {
		if strings.ToLower(strings.TrimSpace(o.Text)) == "approve and proceed" {
			return true
		}
	}
	return false
}

// escPrecision is the escalation-precision instrumentation hook (PRD §13, §21.1:
// "Track escalation precision; make it tunable"). v1 keeps simple counters
// (raised / answered) that future telemetry can divide into a precision signal;
// it is concurrency-safe so the loop goroutine (raise) and a bound service call
// (answer) can both touch it.
type escPrecisionCounter struct {
	mu          sync.Mutex
	numRaised   int
	numAnswered int
}

// (raised/answered) bump the counters.
func (c *escPrecisionCounter) raised()   { c.mu.Lock(); c.numRaised++; c.mu.Unlock() }
func (c *escPrecisionCounter) answered() { c.mu.Lock(); c.numAnswered++; c.mu.Unlock() }

// EscalationStats exposes the raised/answered counters (status/telemetry; PRD §13).
func (e *Engine) EscalationStats() (raised, answered int) {
	e.escPrecision.mu.Lock()
	defer e.escPrecision.mu.Unlock()
	return e.escPrecision.numRaised, e.escPrecision.numAnswered
}
