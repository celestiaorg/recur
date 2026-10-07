package engine

import (
	"context"
	"fmt"
	"html"
	"log"
	"runtime/debug"
	"strings"
	"sync"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/mcp"
)

// CommandKind is a control message sent to a running loop (PRD §6.2).
type CommandKind string

const (
	CmdPause            CommandKind = "pause"
	CmdResume           CommandKind = "resume"
	CmdStop             CommandKind = "stop"
	CmdAnswerEscalation CommandKind = "answer_escalation"
)

// Command is a control message delivered on the loop's command channel. The loop
// goroutine select-s on it at safe checkpoints so control is responsive without
// shared-mutable-state races (PRD §6.2). An escalation is "just a pause whose
// resume payload is the human's answer" (PRD §6.2): CmdAnswerEscalation carries the
// Answer the blocked loop applies to resume from its checkpoint.
type Command struct {
	Kind   CommandKind
	Answer EscalationAnswer
}

// EscalationAnswer is the human's reply to an escalation (PRD §13): a structured
// Choice and/or FreeText. The loop applies it when it resumes from the escalation
// checkpoint. Resolved is the final single-line answer the engine persisted.
// Grant, when > 0, is extra converge rounds from extend_rounds; 0 means use policy.
type EscalationAnswer struct {
	Choice   string
	FreeText string
	Resolved string
	Grant    int
}

// Loop is one running unit of work: a goroutine walking its [[loop]] graph
// (walker.go) under its [[policy]] guard (PRD §6.1, §7). Every run has a resolved
// guard — an empty
// loop/policy id resolves to the defaults in newLoop — so there is no
// pre-bundle state machine; a guard that cannot resolve idles with the reason.
type Loop struct {
	ID              string
	ProjectID       string
	Goal            string
	GoalDisplay     string // short UI label when Goal is prompt-action wire
	Branch          string
	BaseBranch      string // dispatch snapshot; live base is Workspace.BaseBranch in the store
	Worktree        string
	Autonomy        domain.AutonomyLevel
	LoopID          string
	PolicyID        string
	ChatCode        string // conductor tab that owns this run (one chat = one loop)
	mapKey          string // e.loops key; workspace id, or workspace+chat when a second run is live
	exitState       domain.WorkspaceState
	exitPhase       string // last setState for this walk; not the workspace row (siblings overwrite that)
	guard           resolvedGuard
	freshWalk       bool   // ignore sibling-chat checkpoint; start at [[loop]] entry
	startPhase      string // simplify: additional-walk reenter; per-chat checkpoints when two walks need independent resume
	missingGuard    string // resolveGuard refuse — idle honestly on missing refs
	onDemandWalk    bool
	resumePhaseID   string
	pendingOnDemand string            // mu-guarded; queued run_phase at a graph checkpoint
	currentPhase    bundles.LoopPhase // phase the walker is inside right now
	catalog         bundles.Catalog   // snapshot for the run — avoid re-walking bundles
	// walkState / walkIter are this goroutine's last setState/setIteration.
	// simplify: workspace State/Iteration stay shared; persist per-chat when two idle walks need independent Maestro decisions.
	walkState domain.WorkspaceState
	walkIter  int

	eng *Engine

	// git is this loop's OWN worktree manager, bound to its project's repo at
	// dispatch time (D22). The engine holds no shared mutable manager, so concurrent
	// loops never race on it.
	git *gitpkg.Manager

	cmds chan Command

	// pause + escalation coordination: the loop blocks on a single condition at a
	// safe checkpoint without busy-waiting (PRD §6.2). All of paused/stopped/
	// escalated/answer are guarded by mu and woken via cond.Broadcast. An escalation
	// is just a pause whose resume payload is the human's answer (PRD §6.2): the loop
	// blocks in awaitAnswer until AnswerEscalation delivers CmdAnswerEscalation (sets
	// answer + clears escalated) or Stop/ctx-cancel wakes it.
	mu          sync.Mutex
	cond        *sync.Cond
	paused      bool
	stopped     bool
	parkIdle    bool // composer Stop: unwind as idle, not StateStopped
	escalated   bool
	roundCapEsc bool // true only while handleCap is blocked in awaitAnswer
	answer      *EscalationAnswer

	// pendingEsc is an escalation request raised by an agent's escalate MCP tool
	// call (set from the MCP HTTP handler goroutine, NOT the loop goroutine, so it
	// is mu-guarded). The HTTP handler returns immediately; the loop goroutine
	// consumes the pending request at its next safe checkpoint and BLOCKS on it
	// (PRD §13). Rail #4 (cap) escalates directly from the loop goroutine instead.
	pendingEsc *mcp.EscalationRequest

	// findingMu guards the current-review-iteration finding count. The Reviewer
	// agent reports findings via the MCP server (a separate subprocess), which
	// calls engineReporter.ReportFinding → collectFinding concurrently with the
	// loop goroutine. curIter is the iteration findings are stamped with; iterFinds
	// counts findings reported during the active review.
	findingMu sync.Mutex
	curIter   int
	iterFinds int

	// verifyMu guards the verify-evidence collector. The QA agent files evidence
	// via the MCP server (a separate subprocess) → engineReporter.AttachEvidence →
	// collectEvidence concurrently with the loop goroutine, so it is mutex-guarded.
	// verifyEvidence accumulates the steps the QA agent attached during the active
	// verify turn; the loop goroutine reads them after the turn ends.
	verifyMu       sync.Mutex
	verifyEvidence []mcp.Evidence

	// plan is the Maestro's planning-turn output (PRD §7.2), set by runPlanner and
	// read by runCoder to brief the Coder with the full investigated plan rather than
	// just the bare task goal (a "lesser" Coder agent has no other context). It is
	// set BEFORE runCoder is dispatched and only ever touched by the loop's own
	// goroutine, so it needs no lock. Empty when planning was skipped or degraded
	// (best-effort; the loop still proceeds task-only). newLoop seeds it from the
	// persisted DefinitionOfDone so a resume still gives the Coder the original plan.
	plan string

	// planStructured marks that the run's plan carries structured steps
	// — the Coder is then told to advance them
	// via mcp__recur__update_plan_step. Same single-goroutine discipline as
	// plan above; newLoop seeds it from the persisted record for reworks/resumes.
	planStructured bool

	// resume, when non-nil, makes this goroutine re-enter the state machine at a
	// durable checkpoint instead of replaying from planning (D55). It is set from the
	// persisted domain.Workspace at dispatch time and read only by the loop goroutine, so
	// it needs no lock. Nil for a fresh loop.
	resume *resumePoint

	// pendingSteer is the latest within-rails Maestro re-plan from chat (PRD §8.5).
	// Written from the MCP/chat goroutine via queueSteer. Newly spawned specialists
	// read it via applyPendingSteer (into their initial Prompt). The in-flight
	// specialist, if any, receives it on activeInject as a user message on its next
	// tool_result turn (D49) — never by mutating its already-cached Prompt.
	pendingSteer string

	// grantedExtra is pending extra fix iterations granted via chat
	// (extend_iterations), consumed by the converge loop at its next iteration
	// boundary. Guarded by mu like pendingSteer.
	grantedExtra int

	// activeInject is the in-flight specialist's Inject channel (set for the duration
	// of Run). queueSteer non-blocking-sends here so a mid-turn steer reaches the
	// running agent without waiting for the next spawn.
	activeInject chan string

	// doneTurnCode is the in-flight specialist's agent code (CO/WR/…). MCP done
	// routes the summary to this walk when two loops share a workspace.
	doneTurnCode string

	// doneSummary is the implementer's last successful mcp__recur__done wrap-up
	// for this iteration (MCP goroutine writes; loop goroutine takes it at commit).
	// lastDoneSummary keeps every successful done() this walk (all landings /
	// children) for the Maestro wrap-up prompt. take still clears the commit copy.
	// doneNotes is the optional handoff filed alongside it (same discipline).
	doneSummary     string
	lastDoneSummary string
	doneNotes       string

	// boardUnblocked are the parent steps merged since the board last started a
	// wave — the next wave's "after" (fanout.go). Loop goroutine only.
	boardUnblocked []string
	// lastHandoff is this landing's implementer notes (replan); boardHandoff
	// accumulates child notes within one board pass. Loop goroutine only.
	lastHandoff  string
	boardHandoff string

	// pendingConflicts are paths a child-branch merge left conflicted (fan-out);
	// the next coder turn resolves them instead of implementing. Loop goroutine only.
	pendingConflicts []string

	// parentID marks a child workspace working one plan step for its parent.
	// Children never fan out themselves.
	parentID string

	startedAt time.Time
}

func (l *Loop) noteDoneSummary(summary, notes string) {
	if l == nil {
		return
	}
	s := strings.TrimSpace(summary)
	if s == "" {
		return
	}
	l.mu.Lock()
	l.doneSummary = s
	l.lastDoneSummary = accumulateHandoff(l.lastDoneSummary, s)
	l.doneNotes = strings.TrimSpace(notes)
	l.mu.Unlock()
}

// noteDoneNotes keeps the handoff notes from a done call the gate refused
// (findings still open). The Coder may never get to a successful done this
// turn; its account of deviations and unfinished work must not vanish with it.
func (l *Loop) noteDoneNotes(notes string) {
	if l == nil {
		return
	}
	n := strings.TrimSpace(notes)
	if n == "" {
		return
	}
	l.mu.Lock()
	l.doneNotes = n
	l.mu.Unlock()
}

func (l *Loop) takeDoneNotes() string {
	if l == nil {
		return ""
	}
	l.mu.Lock()
	n := l.doneNotes
	l.doneNotes = ""
	l.mu.Unlock()
	return n
}

func (l *Loop) reviewHandoff() string {
	if l == nil {
		return ""
	}
	if s := strings.TrimSpace(l.boardHandoff); s != "" {
		return s
	}
	return l.lastHandoff
}

func (l *Loop) takeDoneSummary() string {
	if l == nil {
		return ""
	}
	l.mu.Lock()
	s := l.doneSummary
	l.doneSummary = ""
	l.mu.Unlock()
	return s
}

// wrapUpDone is every successful done summary this walk kept (all landings /
// children), still present after commit takes the current iteration's copy.
func (l *Loop) wrapUpDone() string {
	if l == nil {
		return ""
	}
	l.mu.Lock()
	s := l.lastDoneSummary
	l.mu.Unlock()
	return s
}

// emitSettledWrapUp has Maestro write the operator wrap-up for a settled walk.
// Children skip it — the parent Maestro covers the whole fan-out. PR-open
// already posted the Completed header; wrap is patched onto that fact.
func (l *Loop) emitSettledWrapUp() {
	if l == nil || l.eng == nil || l.wantsIdle() || l.isStopped() {
		return
	}
	if !loopSettled(l.exitState, l.exitPhase) || l.parentID != "" {
		return
	}
	wrap := l.maestroWrapHTML()
	if l.exitState == domain.StatePROpen {
		if wrap != "" {
			l.eng.attachLoopWrap(l.ID, wrap)
		}
		return
	}
	elapsed := time.Duration(0)
	if !l.startedAt.IsZero() {
		elapsed = time.Since(l.startedAt)
	}
	line := "Completed."
	if elapsed > 0 {
		line = "Completed in " + formatLoopElapsed(elapsed) + "."
	}
	l.emitFeed("MA", "Maestro", line+wrap)
}

// resumePoint is the decoded form of a domain.Checkpoint: which phase this goroutine
// re-enters, at which convergence iteration, and whether the coding turn for that
// iteration already completed (so resume reviews the existing commit rather than
// re-running the Coder over work it already did).
type resumePoint struct {
	State     domain.WorkspaceState
	Iteration int
	SkipCoder bool

	// sessions maps a role code to the claude session it was running when the loop was
	// interrupted. The FIRST turn a role takes after a resume continues that session
	// via --resume, so the agent keeps the memory of what it was doing (D55); later
	// turns in the same run start fresh, exactly as they always did.
	sessions map[string]string

	// consumed records roles whose first post-resume turn has already happened, so the
	// continuation is applied once per role. Only the loop goroutine touches it.
	consumed map[string]bool

	// startConsumed is set after the first converge() in this goroutine reads
	// Iteration/SkipCoder, so a later re-entry (verify on_reject, PR-door send-back)
	// does not restart numbering or re-skip the coder.
	startConsumed bool
}

// resumedRole reports whether this is the FIRST turn role code takes after a resume,
// and if so the prior claude session id to continue (empty when none was captured —
// an older DB, or a role that had not started). One-shot per role: later turns of the
// same run report false and run normally.
func (l *Loop) resumedRole(code string) (session string, first bool) {
	if l.resume == nil {
		return "", false
	}
	if l.resume.consumed[code] {
		return "", false
	}
	if l.resume.consumed == nil {
		l.resume.consumed = map[string]bool{}
	}
	l.resume.consumed[code] = true
	return l.resume.sessions[code], true
}

// queueSteer records a Maestro chat re-plan. Called from the MCP handler goroutine
// (concurrent with the loop). A newer steer replaces an older one. Newly spawned
// agents pick it up via applyPendingSteer; an in-flight agent is notified on
// activeInject (user-message inject on its next tool_result — prompt cache intact).
// grantExtension credits extra fix iterations from a chat extend_iterations grant
// (already clamped by the engine to the autonomy allowance). The converge loop
// consumes the credit at its next iteration boundary.
func (l *Loop) grantExtension(n int) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.grantedExtra += n
}

// takeGrantedExtension drains any pending chat-granted extension credit.
func (l *Loop) takeGrantedExtension() int {
	l.mu.Lock()
	defer l.mu.Unlock()
	n := l.grantedExtra
	l.grantedExtra = 0
	return n
}

func (l *Loop) queueSteer(summary string) {
	summary = strings.TrimSpace(summary)
	if summary == "" {
		return
	}
	l.mu.Lock()
	l.pendingSteer = summary
	ch := l.activeInject
	l.mu.Unlock()
	if ch == nil {
		return
	}
	select {
	case ch <- summary:
	default:
		// Buffer full — pendingSteer still covers the next newly spawned agent.
	}
}

// bindInject arms mid-turn steer delivery for one specialist Run. The returned
// channel is set on AgentSpec.Inject; the cleanup clears activeInject. Must be
// deferred by the caller so a late steer can't write to a closed/finished turn.
func (l *Loop) bindInject(agentCode string) (inject <-chan string, cleanup func()) {
	ch := make(chan string, 1)
	l.mu.Lock()
	l.activeInject = ch
	l.doneTurnCode = agentCode
	l.mu.Unlock()
	return ch, func() {
		l.mu.Lock()
		if l.activeInject == ch {
			l.activeInject = nil
		}
		if l.doneTurnCode == agentCode {
			l.doneTurnCode = ""
		}
		l.mu.Unlock()
	}
}

func (l *Loop) holdsDoneTurn(agentCode string) bool {
	if l == nil {
		return false
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.doneTurnCode == agentCode
}

func (l *Loop) acceptsDoneSummary(agentCode string) bool {
	if l == nil || strings.TrimSpace(agentCode) == "" {
		return false
	}
	if l.holdsDoneTurn(agentCode) {
		return true
	}
	return l.slotRoleSpec("implementer", roles[domain.RoleCoder]).code == agentCode
}

// applyPendingSteer appends the pending Maestro steer onto a NEW agent's initial
// Prompt. In-flight agents must NOT go through this path for mid-turn steers —
// they receive a user-message inject instead (prompt-cache safe). Sticky until a
// newer steer replaces it so later spawns still see the updated plan.
func (l *Loop) applyPendingSteer(prompt string) string {
	l.mu.Lock()
	steer := l.pendingSteer
	l.mu.Unlock()
	if steer == "" {
		return prompt
	}
	// Steer is now in a specialist prompt — the restored answer has been consumed.
	l.eng.Store.MarkEscalationApplied(l.ID)
	return prompt + "\n\n" + formatSteerUserMessage(steer)
}

// formatSteerUserMessage is the canonical steer text used both as a new-agent
// Prompt suffix and as an in-flight stdin user message.
func formatSteerUserMessage(summary string) string {
	return "Maestro steer (follow this; it supersedes earlier plan guidance):\n" + summary
}

// joinPlainInjects concatenates mid-turn Inject payloads into one follow-up
// user message with no Maestro prefix. Cursor can only Send once after
// CancelRun, so every payload that arrived before that Send rides together.
func joinPlainInjects(msgs []string) string {
	var parts []string
	for _, m := range msgs {
		m = strings.TrimSpace(m)
		if m == "" {
			continue
		}
		parts = append(parts, m)
	}
	return strings.Join(parts, "\n\n")
}

// joinSteerInjects concatenates mid-turn Inject payloads into one follow-up
// user message. Cursor can only Send once after CancelRun, so every steer that
// arrived before that Send has to ride together; Claude/Codex flush each as
// its own user message instead.
func joinSteerInjects(msgs []string) string {
	joined := joinPlainInjects(msgs)
	if joined == "" {
		return ""
	}
	return formatSteerUserMessage(joined)
}

// injectPlainSep splits UI display from the model prompt when they differ.
// ASCII RS is not typed in composer text; wrapComposer output never includes it.
const injectPlainSep = "\x1e"

func encodeInjectPlain(prompt, display string) string {
	prompt = strings.TrimSpace(prompt)
	display = strings.TrimSpace(display)
	if display == "" || display == prompt {
		return prompt
	}
	return display + injectPlainSep + prompt
}

func decodeInjectPlain(s string) (prompt, display string) {
	s = strings.TrimSpace(s)
	display, prompt, ok := strings.Cut(s, injectPlainSep)
	if !ok {
		return s, s
	}
	prompt = strings.TrimSpace(prompt)
	display = strings.TrimSpace(display)
	if prompt == "" {
		return display, display
	}
	if display == "" {
		display = prompt
	}
	return prompt, display
}

func joinPlainInjectPrompts(msgs []string) string {
	var parts []string
	for _, m := range msgs {
		p, _ := decodeInjectPlain(m)
		if p == "" {
			continue
		}
		parts = append(parts, p)
	}
	return strings.Join(parts, "\n\n")
}

// followUpInjectText is the Cursor follow-up Send body for a batch of Inject
// payloads. Loop specialists get the Maestro prefix; workspace chat does not.
func followUpInjectText(spec AgentSpec, msgs []string) string {
	if spec.InjectPlain {
		return joinPlainInjectPrompts(msgs)
	}
	return joinSteerInjects(msgs)
}

// injectStdinText is the user-message body Claude/Codex write for one Inject
// payload. Workspace chat (InjectPlain) is the composer text; loop specialists
// keep the Maestro prefix.
func injectStdinText(spec AgentSpec, msg string) string {
	msg = strings.TrimSpace(msg)
	if spec.InjectPlain {
		p, _ := decodeInjectPlain(msg)
		return p
	}
	if msg == "" {
		return ""
	}
	return formatSteerUserMessage(msg)
}

func emitInjectUser(spec AgentSpec, emit func(ParsedEvent), raw string) {
	if emit == nil || !spec.InjectPlain {
		return
	}
	p, d := decodeInjectPlain(raw)
	if p == "" {
		return
	}
	if d == p {
		d = ""
	}
	emit(ParsedEvent{Kind: KindUser, Text: p, DisplayText: d})
}

func ackInject(spec AgentSpec, ok bool) {
	if spec.InjectAck != nil {
		spec.InjectAck <- ok
	}
}

// beginVerify resets the verify-evidence collector before the QA agent runs.
func (l *Loop) beginVerify() {
	l.verifyMu.Lock()
	l.verifyEvidence = nil
	l.verifyMu.Unlock()
	if l.eng != nil {
		l.eng.Store.UpdateWorkspace(l.ID, func(ws *domain.Workspace) { ws.TestResult = nil })
	}
}

// collectEvidence records one evidence step attached during the active verify
// turn. Called from the MCP callback path (a different goroutine than the loop),
// so it is mutex-guarded.
func (l *Loop) collectEvidence(e mcp.Evidence) {
	l.verifyMu.Lock()
	l.verifyEvidence = append(l.verifyEvidence, e)
	l.verifyMu.Unlock()
}

// evidenceThisVerify returns a copy of the evidence the QA agent attached during
// the active verify turn (read by the loop goroutine after the turn ends).
func (l *Loop) evidenceThisVerify() []mcp.Evidence {
	l.verifyMu.Lock()
	defer l.verifyMu.Unlock()
	return append([]mcp.Evidence(nil), l.verifyEvidence...)
}

// beginReviewIteration sets the iteration that incoming findings are stamped with
// and resets the per-iteration counter, before the Reviewer agent runs.
func (l *Loop) beginReviewIteration(iter int) {
	l.findingMu.Lock()
	l.curIter = iter
	l.iterFinds = 0
	l.findingMu.Unlock()
}

// collectFinding records one finding reported during the active review iteration
// and returns the iteration to stamp it with. Called from the MCP callback path
// (a different goroutine than the loop), so it is mutex-guarded.
func (l *Loop) collectFinding() int {
	l.findingMu.Lock()
	defer l.findingMu.Unlock()
	l.iterFinds++
	return l.curIter
}

// findingsThisIteration returns how many findings the Reviewer reported during the
// active review iteration (read by the loop goroutine after the review turn ends).
func (l *Loop) findingsThisIteration() int {
	l.findingMu.Lock()
	defer l.findingMu.Unlock()
	return l.iterFinds
}

// newLoop constructs a loop bound to the engine. The command channel is buffered
// so a control send never blocks the caller even between checkpoints.
func newLoop(eng *Engine, l domain.Workspace) *Loop {
	goal, display := l.Goal, l.GoalDisplay
	if code := strings.TrimSpace(l.LoopChatCode); code != "" {
		if g, d := eng.chatLoopGoal(l.ID, code); g != "" {
			goal, display = g, d
		}
	}
	lp := &Loop{
		ID:          l.ID,
		ProjectID:   l.ProjectID,
		Goal:        goal,
		GoalDisplay: display,
		Branch:      l.Branch,
		BaseBranch:  l.BaseBranch,
		Worktree:    l.WorktreePath,
		Autonomy:    l.Autonomy,
		LoopID:      l.LoopID,
		PolicyID:    l.PolicyID,
		ChatCode:    strings.TrimSpace(l.LoopChatCode),
		walkIter:    l.Iteration,
		eng:         eng,
		cmds:        make(chan Command, 8),
		startedAt:   time.Now(),
		// Seed from the persisted plan (empty for a fresh loop).
		plan:           l.DefinitionOfDone,
		planStructured: l.Plan != nil && len(l.Plan.Steps) > 0,
		parentID:       strings.TrimSpace(l.ParentID),
		// A loop resumed straight into its review must still brief the Reviewer
		// with the handoff the implementer filed before the restart.
		lastHandoff:  strings.TrimSpace(l.Handoff),
		boardHandoff: strings.TrimSpace(l.Handoff),
	}
	lp.cond = sync.NewCond(&lp.mu)

	// Re-enter at the durable checkpoint if this loop was interrupted or is being
	// retried (D55). Decoded here — not at each call site — so every dispatch path
	// (StartWorkspace, queued drain, Resume, Retry, answer-resume) resumes identically.
	lp.resume = resumePointFor(l, eng.agentSessions(l.ID))
	// Every run walks a [[loop]] graph. An empty loop/policy id resolves to the
	// settings default, then the shipped To a PR loop (resolveGuard), and the
	// resolved ids are stamped on the run so the dock and the row agree. A guard
	// that cannot resolve idles with the reason — there is no pre-bundle path.
	if g, reason := eng.guardForWorkspace(l); reason == "" {
		lp.guard = g
		if lp.LoopID == "" {
			lp.LoopID = g.Loop.ID
		}
		if lp.PolicyID == "" {
			lp.PolicyID = g.Policy.ID
		}
	} else {
		lp.missingGuard = reason
	}
	if cat, err := eng.discoverLoopCatalog(); err == nil {
		lp.catalog = cat
	}
	if l.Checkpoint != nil {
		if id := strings.TrimSpace(l.Checkpoint.PhaseID); id != "" {
			lp.startPhase = id
		}
		if rid := strings.TrimSpace(l.Checkpoint.ResumePhaseID); rid != "" {
			lp.resumePhaseID = rid
		}
		if ph, ok := lp.guard.Loop.PhaseByID(strings.TrimSpace(l.Checkpoint.PhaseID)); ok && ph.Dispatch == "on_demand" {
			lp.onDemandWalk = true
		}
	}
	// Restart-after-escalation: this goroutine is not blocked in awaitAnswer, so
	// seed the already-persisted answer for Run/escalate to apply — including
	// paused/queued records waiting on a delayed dispatch (answer not yet Applied)
	// and legacy answered rows Recover parked as stuck (missing applied defaulted true).
	if esc, ok := eng.Store.Escalation(l.ID); ok && shouldSeedEscalationAnswer(l, esc) {
		lp.answer = restoreEscalationAnswer(esc)
	}
	return lp
}

// applyGoalImages attaches the opening user pastes to a specialist turn so the
// model can see them (Claude interleaved blocks / Cursor images[]).
func (l *Loop) applyGoalImages(spec *AgentSpec) {
	if l == nil || spec == nil || l.eng == nil {
		return
	}
	imgs := l.eng.loopGoalImages(l.ID, l.ChatCode)
	if len(imgs) == 0 {
		return
	}
	spec.Images = imgs
	spec.Prompt = ensureAttachmentMarkers(spec.Prompt, imgs)
}

// goalLabel is the short UI string for this loop's goal. Empty GoalDisplay
// (plain-text goals) returns Goal.
func (l *Loop) goalLabel() string {
	if l == nil {
		return ""
	}
	return domain.Workspace{Goal: l.Goal, GoalDisplay: l.GoalDisplay}.GoalLabel()
}

// send delivers a command to the loop (non-blocking; the channel is buffered).
// Pause/Resume also flip the pause condition so a loop blocked at a checkpoint
// wakes. Stop additionally is observed via ctx cancellation by the engine.
func (l *Loop) send(c Command) {
	switch c.Kind {
	case CmdPause:
		l.mu.Lock()
		l.paused = true
		l.mu.Unlock()
	case CmdResume:
		l.mu.Lock()
		l.paused = false
		l.mu.Unlock()
		l.cond.Broadcast()
	case CmdStop:
		l.mu.Lock()
		l.stopped = true
		l.paused = false    // unblock a paused loop so it can observe the stop
		l.escalated = false // unblock a loop blocked on an escalation so Stop wins
		l.mu.Unlock()
		l.cond.Broadcast()
	case CmdAnswerEscalation:
		l.mu.Lock()
		ans := c.Answer
		l.answer = &ans
		l.escalated = false // clears the escalation block; the loop resumes
		l.paused = false
		l.mu.Unlock()
		l.cond.Broadcast()
	}
	select {
	case l.cmds <- c:
	default:
	}
}

// requestIdle marks this walk to park idle when its context is cancelled
// (composer Stop). Closing a tab still uses CmdStop → StateStopped.
func (l *Loop) requestIdle() {
	if l == nil {
		return
	}
	l.mu.Lock()
	l.parkIdle = true
	l.mu.Unlock()
	if l.cond != nil {
		l.cond.Broadcast()
	}
}

func (l *Loop) wantsIdle() bool {
	if l == nil {
		return false
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.parkIdle
}

func (l *Loop) isStopped() bool {
	if l == nil {
		return false
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.stopped
}

// requestEscalation records an escalation request raised by an agent's escalate
// MCP tool call (called from the MCP HTTP handler goroutine). It does NOT block —
// the loop goroutine consumes it at its next checkpoint and blocks there (PRD §13).
// If one is already pending/active it is ignored (first wins).
func (l *Loop) requestEscalation(req mcp.EscalationRequest) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.pendingEsc != nil || l.escalated {
		return
	}
	r := req
	l.pendingEsc = &r
}

// takePendingEscalation returns and clears a pending escalation request (consumed
// by the loop goroutine at a checkpoint), or nil if none is pending.
func (l *Loop) takePendingEscalation() *mcp.EscalationRequest {
	l.mu.Lock()
	defer l.mu.Unlock()
	req := l.pendingEsc
	l.pendingEsc = nil
	return req
}

// hasPendingEscalation reports whether an escalation is currently pending, WITHOUT
// consuming it — used by runPlanner to detect that the planning turn called
// mcp__reloop__escalate (maestroPlanSystemPrompt tells it to do so "instead of
// planning" on genuine ambiguity) so it can skip treating the turn's narration text
// as a plan. The request itself is left for converge's checkpoint to consume and
// handle via takePendingEscalation as usual.
func (l *Loop) hasPendingEscalation() bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.pendingEsc != nil
}

// checkpoint is a safe point between phases: it drains pending commands, services a
// pending escalation (raised by an agent's escalate tool), and, if paused, blocks
// on the condition until resumed or stopped (PRD §6.2 — no busy-wait). It returns
// false if the loop should stop (ctx cancelled, Stop, or a Stop won an escalation).
//
// A ctx-cancel watcher broadcasts the cond so a paused loop wakes promptly when
// Stop cancels the context (Stop also flips stopped + broadcasts via send, but the
// watcher covers engine Shutdown / direct cancellation too).
func (l *Loop) checkpoint(ctx context.Context) bool {
	// Drain queued commands (their effect on paused/stopped was applied in send).
drain:
	for {
		select {
		case <-l.cmds:
		default:
			break drain
		}
	}

	// Service an agent-raised escalation: block here until answered/stopped. This is
	// the escalate-MCP-tool path; rail #4 escalates directly from converge instead.
	if req := l.takePendingEscalation(); req != nil {
		_, keepGoing := l.escalate(ctx, *req, escKindAgent)
		if !keepGoing {
			return false
		}
		l.eng.Store.MarkEscalationApplied(l.ID)
	}

	l.mu.Lock()
	defer l.mu.Unlock()

	if !l.paused {
		return !l.stopped && ctx.Err() == nil
	}

	// Paused: publish the paused state once, then block on the cond until resumed
	// or stopped. Wake the cond on ctx cancel via a one-shot watcher.
	stop := context.AfterFunc(ctx, func() {
		l.mu.Lock()
		l.cond.Broadcast()
		l.mu.Unlock()
	})
	defer stop()

	l.mu.Unlock()
	l.setState(domain.StatePaused, "paused")
	l.mu.Lock()

	for l.paused && !l.stopped && ctx.Err() == nil {
		l.cond.Wait()
	}
	return !l.stopped && ctx.Err() == nil
}

// Run walks the loop's graph to a terminal or idle state (PRD §6.1, §7). One
// goroutine per loop. ctx threads Engine → Loop → Agent; cancelling it (Stop or
// engine shutdown) unwinds the loop and kills its agents.
func (l *Loop) Run(ctx context.Context) {
	// Defers run LIFO: loopWG.Done (so Shutdown's join completes) runs last; the
	// slot release + queued drain + map removal (loopExited) runs after the panic
	// guard; the panic guard runs first so a panic is converted to a failed loop
	// BEFORE the slot is released. One loop's panic must degrade to a failed loop,
	// never crash the whole app (D25).
	defer l.eng.loopWG.Done()
	defer l.eng.loopExited(l)
	defer l.recoverPanic()

	if l.missingGuard != "" {
		l.emitFeed("MA", "Maestro", html.EscapeString(l.missingGuard)+" — idling.")
		l.setState(domain.StatePaused, "loop refs missing")
		return
	}
	if l.applyAnsweredCap(ctx) {
		return
	}
	l.walkLoop(ctx)
}

// converge drives the code↔review iteration loop to convergence (PRD §7.3): the
// Coder edits, the engine commits, a SEPARATE Reviewer (rail #2) reviews the diff
// and files findings via the MCP; if findings > 0 the loop returns to coding (the
// Coder gets the findings to fix) and re-reviews; it repeats until ZERO findings
// (convergence) or a max-iteration cap. The ENGINE — never the Coder — decides
// "converged" (PRD §7.3): convergence is declared here, off the engine-counted
// finding total, not off anything the Coder says.
//
// RAIL #4 (escalate when looping, PRD §9): hitting the cap WITHOUT converging no
// longer silently stops — it RAISES an escalation ("I've run N iterations without
// converging — how should I proceed?") and BLOCKS until the human answers (PRD §13),
// then applies the choice (keep iterating / change approach / ship with caveats /
// stop). If rail #4 is off, it falls back to the M3 behavior (stop honestly).
//
// Returns the terminal next state for the outer state machine: pr_open on
// convergence, stopped if control/answer ends it, or an error.
func (l *Loop) converge(ctx context.Context) (domain.WorkspaceState, error) {
	maxIter := l.guard.Policy.MaxRounds
	if ph, ok := l.walkPhase(); ok {
		maxIter = l.guard.phaseMaxRounds(ph)
	}
	var prevFindings []domain.Finding
	extraCap := 0 // grows when the human answers "keep iterating" at the cap (rail #4)

	// RESUMING (D55): pick up at the checkpointed iteration rather than replaying
	// iteration 1 over work already committed. skipCoder is set when the interruption
	// happened AFTER the coding turn committed (checkpoint was reviewing/verifying), so
	// this iteration goes straight to the review of the real diff. Open findings are
	// NOT bulk-cleared — the Coder owns closing them via update_finding. If we would
	// skip coding while opens remain, force a coding turn to finish the checklist.
	startIter := 1
	skipCoder := false
	resumeEntry := false
	if l.resume != nil && !l.resume.startConsumed {
		startIter = l.resume.Iteration
		skipCoder = l.resume.SkipCoder
		if skipCoder && l.eng.Store.OpenFindingsCount(l.ID) > 0 {
			skipCoder = false
		}
		l.resume.startConsumed = true
		resumeEntry = true
	} else if ws, ok := l.eng.Store.Workspace(l.ID); ok && ws.Iteration > 0 {
		startIter = ws.Iteration + 1
	}
	// start=review is per entry, not per absolute iteration: reenter_phase
	// after a Review & fix walk must still open on the reviewer.
	if ph, ok := l.walkPhase(); ok && ph.Start == "review" && !resumeEntry {
		skipCoder = true
	}
	if startIter > 1 && !skipCoder {
		if seeded := seedFixFindings(startIter, skipCoder, l.eng.Store.OpenFindings(l.ID)); len(seeded) > 0 {
			prevFindings = seeded
		}
	}

	// Each converge() call gets maxIter rounds from startIter, not an absolute
	// 1..maxIter window. Re-entry after a capped run would otherwise have
	// startIter > maxIter and fall through to stop without a turn.
	endIter := convergeRoundWindow(startIter, maxIter)

	for iter := startIter; iter <= endIter+extraCap; iter++ {
		// Pause/Stop/escalate is observed between iterations (a safe checkpoint).
		if !l.checkpoint(ctx) {
			return domain.StateStopped, nil
		}
		// A chat extend_iterations grant (already autonomy-clamped) widens the budget.
		extraCap += l.takeGrantedExtension()

		// ---- the board (first coding pass of this entry) ----
		// A structured plan is worked round by round: fan out ready independent
		// steps into child workspaces, run the rest here in scoped turns,
		// repeat until nothing is pending (board.go). The review then judges
		// the aggregate diff.
		boardRan := false
		if iter == startIter && !skipCoder && len(prevFindings) == 0 {
			ran, stop, err := l.runBoard(ctx, iter)
			if err != nil {
				return domain.StateFailed, err
			}
			if stop != "" {
				return stop, nil
			}
			if ran {
				skipCoder, boardRan = true, true
			}
		}

		// ---- coding turn ----
		skippedCoder := false
		if skipCoder {
			// Only the FIRST iteration of a resumed (or start=review) run skips it;
			// later iterations of the same run are normal code→review cycles.
			skipCoder = false
			skippedCoder = true
			l.setIteration(iter)
			msg := fmt.Sprintf(
				"Iteration %d was already coded before the interruption — re-reviewing the committed diff.", iter)
			if boardRan {
				msg = "Board finished — reviewing the aggregate diff."
			} else if l.resume == nil {
				msg = "Reviewing the current worktree first."
			}
			l.emitFeed("MA", "Maestro", msg)
		} else {
			l.resetBoardHandoff()
			l.setState(domain.StateCoding, fmt.Sprintf("iteration %d", iter))
			l.setIteration(iter)
			if err := l.runCoder(ctx, iter, prevFindings); err != nil {
				return domain.StateFailed, err
			}
			// Replan hook: a non-empty handoff gives the Maestro one bounded
			// turn to revise the plan before the Reviewer judges the diff.
			if l.lastHandoff != "" && l.checkpoint(ctx) {
				l.runReplanner(ctx, l.lastHandoff)
			}
		}

		if !l.checkpoint(ctx) {
			return domain.StateStopped, nil
		}

		// ---- review turn (SEPARATE agent; rail #2) ----
		l.setState(domain.StateReviewing, fmt.Sprintf("reviewing iteration %d", iter))
		if _, err := l.runReviewer(ctx, iter); err != nil {
			return domain.StateFailed, err
		}

		// Policy bar is remaining opens, not this review turn's filings.
		// A rereview that files nothing new must not converge over leftovers.
		barFindings := l.eng.Store.OpenFindings(l.ID)

		// Record this iteration's open-finding count for the convergence chart
		// (PRD §7.3) and emit it so the LoopDetail chart updates live.
		l.recordConvergence(iter, len(barFindings))

		// One review iteration completed — bump the machine-global "iters today"
		// status-bar counter and publish the rollup (PRD §6.4, §16.1, D6).
		l.eng.gov.AddIters(1)
		l.eng.publishResource()

		if l.convergeBarPassed() {
			// start=review skips runCoder, which is otherwise the only commit
			// site. Commit the reviewed worktree before ensure_pr / DiffPR.
			if skippedCoder {
				if err := l.commitWorktree(ctx, "MA", "Maestro", l.reviewedCommitMessage(ctx)); err != nil {
					return domain.StateFailed, err
				}
			}
			// ENGINE declares convergence. The walker runs ensure_pr / verify next.
			l.emitFeed("MA", "Maestro",
				fmt.Sprintf("Converged after %d iteration(s): 0 open findings.", iter))
			return stateAdvance, nil
		}

		l.emitFeed("MA", "Maestro", fmt.Sprintf(
			"Iteration %d: %d finding(s) — sending back to the Coder.", iter, len(barFindings)))
		prevFindings = barFindings

		// At the (current) cap without converging → policy on_max_rounds.
		if iter >= endIter+extraCap {
			switch l.capOnMaxRounds() {
			case "proceed":
				l.emitFeed("MA", "Maestro", "Round cap reached — proceeding.")
				return stateAdvance, nil
			case "fail":
				l.emitFeed("MA", "Maestro", fmt.Sprintf(
					"Stopped at the %d-iteration cap with open findings.", iter))
				l.setState(domain.StateFailed, "round cap")
				return domain.StateFailed, nil
			}
			next, more, err := l.handleCap(ctx, iter)
			if err != nil {
				return domain.StateStopped, err
			}
			if more > 0 {
				extraCap += more // "keep iterating": extend the cap and continue
				continue
			}
			return next, nil
		}
	}

	return domain.StateStopped, nil
}

// seedFixFindings gives a re-entry coder turn the open findings list so it
// takes the fix slot instead of implement. start=review entries skip the coder
// first, so they stay unseeded.
func seedFixFindings(startIter int, skipCoder bool, open []domain.Finding) []domain.Finding {
	if startIter > 1 && !skipCoder && len(open) > 0 {
		return open
	}
	return nil
}

// convergeRoundWindow is the last iteration number of one converge() call:
// maxIter rounds starting at startIter (not an absolute 1..maxIter window).
func convergeRoundWindow(startIter, maxIter int) int {
	if startIter < 1 {
		startIter = 1
	}
	if maxIter < 1 {
		maxIter = 1
	}
	return startIter + maxIter - 1
}

func (l *Loop) capOnMaxRounds() string {
	onMax := l.guard.Policy.OnMaxRounds
	if ph, ok := l.walkPhase(); ok {
		onMax = l.guard.phaseOnMaxRounds(ph)
	}
	if onMax == "" {
		return "escalate"
	}
	return onMax
}

// handleCap asks the human at on_max_rounds=escalate. Returns the next state, or
// a positive `more` to extend the cap ("keep iterating").
func (l *Loop) handleCap(ctx context.Context, iter int) (next domain.WorkspaceState, more int, err error) {
	l.mu.Lock()
	l.roundCapEsc = true
	l.mu.Unlock()
	defer func() {
		l.mu.Lock()
		l.roundCapEsc = false
		l.mu.Unlock()
	}()
	open := l.eng.Store.OpenFindingsCount(l.ID)
	next, more = l.escalateAtCapApply(ctx, iter, open)
	return next, more, nil
}

// escalateAtCapApply raises the rail-#4 escalation and maps the human's answer to a
// loop action (PRD §13). Options: keep iterating / change approach / stop, plus
// "ship with caveats" only when rail #3 would actually allow opening a PR (otherwise
// offering it just stops the loop after the human picks it — a trap).
func (l *Loop) escalateAtCapApply(ctx context.Context, iter, open int) (domain.WorkspaceState, int) {
	return l.escalateAtCapApplyMsg(ctx, iter, open, "", false)
}

// escalateAtCapApplyMsg is escalateAtCapApply with an optional question override
// (the PR-door bar is not a round cap). shipAnyway offers "Ship with caveats"
// even when the policy bar currently blocks — the explicit punch through ensure_pr.
func (l *Loop) escalateAtCapApplyMsg(ctx context.Context, iter, open int, question string, shipAnyway bool) (domain.WorkspaceState, int) {
	opts := []mcp.EscalationOption{
		{Text: "Keep iterating", Tag: "more effort", Desc: "Give me more fix iterations to converge."},
		{Text: "Change approach", Tag: "rethink", Desc: "The current direction isn't working; I'll try a different one."},
	}
	if shipAnyway {
		opts = append(opts, mcp.EscalationOption{
			Text: "Ship with caveats", Tag: "accept risk", Desc: "Open the PR anyway, noting remaining findings.",
		})
	} else if ok, _ := l.eng.CanOpenPR(l.ID); ok {
		opts = append(opts, mcp.EscalationOption{
			Text: "Ship with caveats", Tag: "accept risk", Desc: "Open the PR noting remaining risk (rails currently allow it).",
		})
	}
	opts = append(opts, mcp.EscalationOption{
		Text: "Stop", Tag: "halt", Desc: "Stop the loop; I'll leave the work as-is for you to inspect.",
	})

	if strings.TrimSpace(question) == "" {
		question = fmt.Sprintf("I've run %d iteration(s) without converging — %s remain. How should I proceed?",
			iter, plural(open, "open finding", "open findings"))
	}
	req := mcp.EscalationRequest{
		Question: question,
		Context: fmt.Sprintf("The reviewer keeps finding issues on the current diff (loop %s). "+
			"I won't open a PR with unresolved findings, and I won't spin forever — this decision is yours.", l.ID),
		Options: opts,
	}

	kind := escKindCap
	if shipAnyway {
		kind = escKindEnsurePR
	}
	ans, keepGoing := l.escalate(ctx, req, kind)
	if !keepGoing {
		return domain.StateStopped, 0
	}

	grant := ans.Grant
	if grant < 1 {
		grant = l.policyExtendRounds()
	}
	switch classifyCapAnswer(ans) {
	case capKeepIterating:
		l.emitFeed("MA", "Maestro", "You chose to keep iterating — extending the convergence budget.")
		return domain.StateCoding, grant
	case capChangeApproach:
		l.emitFeed("MA", "Maestro", "You asked me to change approach — continuing with that guidance.")
		return domain.StateCoding, grant
	case capGuidance:
		l.queueSteer(ans.FreeText)
		l.emitFeed("MA", "Maestro", "Continuing with your guidance: "+html.EscapeString(truncate(ans.FreeText, 160)))
		return domain.StateCoding, grant
	case capShip:
		if !shipAnyway {
			if ok, reason := l.eng.CanOpenPR(l.ID); !ok {
				l.setState(domain.StateStopped, "policy bar blocked ship-with-caveats")
				l.eng.Store.MarkEscalationApplied(l.ID)
				l.emitFeed("MA", "Maestro", "Can't ship: "+html.EscapeString(reason)+" — stopping instead.")
				return domain.StateStopped, 0
			}
		}
		l.emitFeed("MA", "Maestro", "Shipping with caveats per your decision.")
		return domain.StatePROpen, 0
	default: // capStop
		l.setState(domain.StateStopped, "stopped per escalation answer")
		l.eng.Store.MarkEscalationApplied(l.ID)
		l.emitFeed("MA", "Maestro", "Stopping per your decision.")
		return domain.StateStopped, 0
	}
}

// acquireTurnSlot takes a governor agent-slot. A pending keep-iterating grant
// stays unapplied until this succeeds — the turn is about to run.
func (l *Loop) acquireTurnSlot(ctx context.Context) (release func(), ok bool) {
	release, ok = l.eng.acquireAgent(ctx.Done())
	if ok {
		l.markCapAnswerApplied()
	}
	return release, ok
}

// markCapAnswerApplied records that a keep-iterating / change-approach / ship
// answer's durable continuation is persisted. Unconsumed guidance on the
// current answer stays pending until applyPendingSteer puts it in a prompt.
// A leftover Maestro steer from an earlier answer does not block this.
func (l *Loop) markCapAnswerApplied() {
	if l.currentAnswerNeedsSteer() {
		return
	}
	l.eng.Store.MarkEscalationApplied(l.ID)
}

func (l *Loop) currentAnswerNeedsSteer() bool {
	esc, ok := l.eng.Store.Escalation(l.ID)
	if !ok || esc.Applied || esc.Status != "answered" {
		return false
	}
	switch persistedEscalationKind(esc) {
	case escKindAgent:
		return true
	case escKindCap, escKindEnsurePR:
		ans := restoreEscalationAnswer(esc)
		return ans != nil && classifyCapAnswer(*ans) == capGuidance
	default:
		return false
	}
}

func (l *Loop) policyExtendRounds() int {
	if l.guard.Policy.ExtendRounds > 0 {
		return l.guard.Policy.ExtendRounds
	}
	return capExtension(l.Autonomy)
}

// capAnswer enumerates the rail-#4 cap escalation choices.
type capAnswer int

const (
	capStop capAnswer = iota
	capKeepIterating
	capChangeApproach
	capShip
	// capGuidance: the human typed free text instead of picking an option. The
	// text is NOT keyword-guessed — it travels verbatim as a
	// steer; the loop continues under the engine's bounded cap extension.
	capGuidance
)

// classifyCapAnswer maps a cap-escalation answer to an action. A STRUCTURED
// choice maps by the exact option text the ENGINE itself offered — never by
// keyword matching. Free text is guidance: it is steered into subsequent turns
// raw, with the engine only enforcing the iteration-cap arithmetic. Empty
// answers stop (the safe default).
func classifyCapAnswer(ans EscalationAnswer) capAnswer {
	switch strings.ToLower(strings.TrimSpace(ans.Choice)) {
	case "keep iterating":
		return capKeepIterating
	case "change approach":
		return capChangeApproach
	case "ship with caveats":
		return capShip
	case "stop", "i'll set the run config":
		return capStop
	}
	if strings.TrimSpace(ans.FreeText) != "" {
		return capGuidance
	}
	return capStop
}

// capExtension is how many extra iterations "keep iterating" grants, autonomy-keyed
// (a small bounded extension so a repeated "keep iterating" still can't spin
// forever without re-asking the human).
func capExtension(a domain.AutonomyLevel) int {
	switch a {
	case domain.AutonomyCautious:
		return 1
	case domain.AutonomyAggressive:
		return 3
	default:
		return 2
	}
}

// runPlanner runs the Maestro's PLANNING turn (PRD §7.2): a REAL `claude -p` turn
// (the production default) in which the Maestro decomposes the task into a
// thorough plan + a definition-of-done BEFORE any code is written. Planning is no longer a
// hardcoded no-op feed line. The plan is surfaced to the activity feed and persisted
// on the loop (DefinitionOfDone). An empty or genuine (non-transient) failure
// degrades to coding with the bare task — loudly. A TRANSIENT provider failure
// (overload, rate limit, 5xx) is returned so the walk can Retry from the planning
// checkpoint (D56) instead of coding with no plan. A genuine ambiguity the Maestro
// must not guess on surfaces as an escalation (mcp__recur__escalate), consumed at the
// next safe checkpoint (the converge loop). It runs as agent code "MA" (distinct from
// the Coder "CO"/Reviewer "RV", so role separation is unaffected).
// adoptExistingPlan seeds the run from a plan that already exists on the
// workspace (a Plan-mode chat filed it, or the Plan panel's Run-as-loop
// handoff). Returns true when a plan was adopted — the planning turn is then
// skipped: the plan the human reviewed IS the brief.
func (l *Loop) adoptExistingPlan() bool {
	ws, ok := l.eng.Store.Workspace(l.ID)
	if !ok || ws.Plan == nil || strings.TrimSpace(ws.DefinitionOfDone) == "" {
		return false
	}
	l.plan = truncate(ws.DefinitionOfDone, planMaxPersistChars)
	l.planStructured = len(ws.Plan.Steps) > 0
	state := "draft"
	if ws.Plan.Approved {
		state = "approved"
	}
	l.emitFeed(maestroAgentCode, "Maestro", fmt.Sprintf(
		"Using the existing %s plan (%s, %d steps) — skipping the planning turn.",
		state, html.EscapeString(ws.Plan.Source), len(ws.Plan.Steps)))
	return true
}

// convergeBarPassed is the engine's policy bar after a review turn: remaining
// opens (not this turn's filings) so a "no new findings" rereview cannot
// converge over leftovers. ensure_pr re-checks the bar at the PR door.
func (l *Loop) convergeBarPassed() bool {
	th := l.guard.Policy.FindingsThreshold
	if ph, ok := l.walkPhase(); ok {
		th = l.guard.phaseThreshold(ph)
	}
	blocked, _ := l.eng.findingsBarBlocks(l.ID, th)
	return !blocked
}

// gatePlanApproval pauses a fresh run after planning for the human's sign-off
// : Cautious and Balanced ask (decision #4);
// Aggressive proceeds. Only STRUCTURED plans gate — a run whose planner
// degraded to prose (or nothing) proceeds as before rather than blocking on an
// artifact that doesn't exist. Pre-approved plans skip the gate. The answer's
// free text is folded into the brief and steered like any plan change.
func (l *Loop) gatePlanApproval(ctx context.Context) (domain.WorkspaceState, bool) {
	ws, ok := l.eng.Store.Workspace(l.ID)
	if !ok {
		return "", false
	}
	if skipPlanApproval(l.guard.Policy, ws) {
		return "", false
	}
	if ws.Plan == nil || (len(ws.Plan.Steps) == 0 && l.guard.Policy.PlanApproval != "always") || ws.Plan.Approved {
		return "", false
	}
	ans, keepGoing := l.escalate(ctx, mcp.EscalationRequest{
		Question: "Review the plan before I start coding.",
		Context:  truncate(ws.DefinitionOfDone, 4000),
		Options: []mcp.EscalationOption{
			{Text: "Approve and proceed", Tag: "recommended", Desc: "Start coding to this plan as written."},
			{Text: "Proceed with my notes", Tag: "notes", Desc: "Add guidance in the text box — I fold it into the plan and start."},
		},
	}, escKindPlan)
	if !keepGoing {
		l.finishWalkAbort(ctx)
		return domain.StateStopped, true
	}
	if t := strings.TrimSpace(ans.FreeText); t != "" {
		l.foldPlanNotes(t)
	}
	_ = l.eng.ApprovePlan(l.ID)
	l.eng.Store.MarkEscalationApplied(l.ID)
	l.eng.markGroup(l.ID, domain.GroupActive)
	return "", false
}

// foldPlanNotes appends the human's plan-approval guidance to the plan. It goes
// into the structured plan's BODY (not just the prose mirror) so every later
// re-mirror — a step update, a board merge, a re-file — keeps it.
func (l *Loop) foldPlanNotes(t string) {
	note := "\n\nHuman guidance at plan approval:\n" + t
	updated, ok := l.eng.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) {
		if lp.Plan != nil {
			p := *lp.Plan
			p.Body = strings.TrimSpace(strings.TrimSpace(p.Body) + note)
			capPlanForPersist(&p)
			lp.Plan = &p
			lp.DefinitionOfDone = truncate(p.Text(), planMaxPersistChars)
			return
		}
		lp.DefinitionOfDone = truncate(lp.DefinitionOfDone+note, planMaxPersistChars)
	})
	if ok {
		l.plan = truncate(updated.DefinitionOfDone, planMaxPersistChars)
	} else {
		l.plan = truncate(l.plan+note, planMaxPersistChars)
	}
	l.emitFeed(maestroAgentCode, "Maestro", "Folding your approval notes into the plan: "+html.EscapeString(truncate(t, 200)))
}

// runPlanner returns an error when the turn could not be attempted (no MCP) or
// when it failed on a transient provider error. A genuine (non-transient)
// failure still degrades to coding with the bare task, loudly.
func (l *Loop) runPlanner(ctx context.Context) error {
	ctx, cancel := l.turnContext(ctx)
	defer cancel()
	rs := l.slotRoleSpec("planner", roleSpec{
		role: domain.RoleMaestro, code: maestroAgentCode, name: "Maestro",
		allowedTools: maestroPlanTools,
	})
	agentID := l.ID + "-" + rs.code
	set := l.eng.settings()
	model, flags := l.roleModel(rs, set.MaestroModel().String(), set.MaestroFlags())
	l.eng.upsertAgent(l.ID, domain.Agent{
		ID: agentID, WorkspaceID: l.ID, Code: rs.code, Name: rs.name,
		Role: "plans & orchestrates", Model: domain.ParseModelRef(model).ID,
		Status: "planning", Color: "var(--amber)", Task: l.goalLabel(), Cur: -1,
	})

	allowed := rs.allowedTools
	var renameNudge string
	if v, ok := l.eng.loopRenameNudge.LoadAndDelete(l.ID + loopMapSep + l.ChatCode); ok {
		if nudge, _ := v.(string); nudge != "" {
			renameNudge = nudge
			allowed = append(append([]string(nil), rs.allowedTools...), mcpRenameSession)
			if nudge == firstWorkspaceRenamePrompt {
				allowed = append(allowed, mcpRenameWorkspace, mcpRenameBranch)
			}
		}
	}

	token, mcpURL, err := l.eng.registerMCP(l.ID, rs.code, l.ChatCode, allowed, model)
	if err != nil {
		l.eng.updateAgentStatus(l.ID, rs.code, "idle")
		return err
	}
	defer l.eng.unregisterMCP(token)

	inject, clearInject := l.bindInject(rs.code)
	defer clearInject()

	// Snapshot the plan mirror so a structured plan filed DURING this turn is
	// recognised whatever role code filed it.
	planBefore := ""
	if ws, ok := l.eng.Store.Workspace(l.ID); ok {
		planBefore = ws.DefinitionOfDone
	}

	sysPrompt := maestroPlanSystemPrompt(l.Goal)
	if overlay := l.bundleSystemPrompt(l.phaseRoleID("planner"), "", l.Goal, ""); overlay != "" {
		sysPrompt = overlay
	}
	planUser, display, err := l.expandPhasePrompt(ctx, "plan", l.Goal)
	if err != nil {
		return err
	}
	planDisplay := l.goalLabel()
	if display != "" {
		planDisplay = display
	}

	spec := AgentSpec{
		WorkspaceID:        l.ID,
		ChatCode:           l.ChatCode,
		AgentID:            agentID,
		Role:               domain.RoleMaestro,
		Model:              model,
		Flags:              flags,
		SystemPrompt:       sysPrompt,
		AppendSystemPrompt: renameNudge,
		AllowedTools:       allowed,
		Prompt:             planUser,
		DisplayText:        planDisplay,
		Worktree:           l.Worktree,
		PermissionMode:     set.PermissionMode(),
		MCPURL:             mcpURL,
		Inject:             inject,
	}
	l.applyGoalImages(&spec)

	release, ok := l.acquireTurnSlot(ctx)
	if !ok {
		return nil // ctx cancelled — the outer checkpoint settles the loop
	}
	defer release()

	// Now that the Maestro has Read/Grep/Glob (maestroPlanTools), a planning turn
	// routinely spans several tool calls with narration text blocks interleaved
	// between them ("Let me look at X first…", "Now checking Y…"). Collecting each
	// block separately (rather than concatenating into one buffer) lets us fall
	// back to joining ALL of them (with a separator, so distinct blocks never run
	// together mid-word) if res.Text ever comes back empty — parseAssistant emits
	// one KindText per text block, and the final assistant message itself can span
	// several blocks, so keeping only the last one would silently drop the rest of
	// the Maestro's concluding, synthesized brief.
	//
	// On Cursor, read-only roles run in plan mode and the model often puts the
	// real brief in createPlan's args (with only a short ack in assistant text).
	// Harvest that when present so DefinitionOfDone is not left as narration.
	var textBlocks []string
	var createPlanText string
	res, err := l.eng.runAgent(ctx, spec, func(ev ParsedEvent) {
		if ev.nested() {
			return
		}
		switch ev.Kind {
		case KindText:
			if t := strings.TrimSpace(ev.Text); t != "" {
				textBlocks = append(textBlocks, t)
			}
		case KindInit:
			l.eng.setAgentSession(l.ID, rs.code, ev.SessionID)
		case KindStep:
			if isCreatePlanTool(ev.Tool) {
				if p := createPlanBody(ev.ToolInputRaw); p != "" {
					createPlanText = p
				}
			}
			l.eng.appendAgentStep(l.ID, rs.code, domain.AgentStep{
				Kind: stepKind(displayToolFor(ev).Name), Text: displayToolFor(ev).Name + " " + ev.ToolInput,
			})
		}
	})
	l.eng.publishResource()
	l.eng.updateAgentStatus(l.ID, rs.code, "idle")

	if err != nil || res.IsError {
		te := newTurnError(rs.name, err, res)
		if te.transient {
			// D56: park at the planning checkpoint so Retry (or a Settings model
			// switch, then Retry) can re-enter. Coding without a plan is the bug.
			l.emitFeed(rs.code, rs.name,
				"The planning turn failed — not starting without a plan. Retry, or switch the planner model in Settings and retry. "+
					html.EscapeString(truncate(te.reason, 200)))
			return te
		}
		// Genuine failure: degrade to coding with the bare task — but say so
		// (a failed planning turn must not be silent).
		l.emitFeed(rs.code, rs.name,
			"The planning turn failed — proceeding with the bare task. "+html.EscapeString(truncate(te.reason, 200)))
		return nil
	}
	if l.hasPendingEscalation() {
		// The Maestro called mcp__reloop__escalate "instead of planning" (its system
		// prompt's instruction for genuine ambiguity) rather than producing a plan.
		// Any text alongside that call is narration about the question raised, not a
		// plan — persisting it as DefinitionOfDone/l.plan would inject that narration
		// into every Coder turn's system prompt as an authoritative brief. Leave the
		// pending escalation for converge's checkpoint to consume/handle as usual and
		// skip the persist/l.plan assignment below entirely.
		return nil
	}
	// STRUCTURED path: if the turn filed a plan via
	// mcp__recur__submit_plan, applySubmittedPlan already persisted the artifact
	// and mirrored it into DefinitionOfDone — adopt that as the brief and skip
	// the prose-parse below (which would overwrite the mirror with raw narration).
	if ws, ok := l.eng.Store.Workspace(l.ID); ok && ws.Plan != nil &&
		(ws.Plan.Source == "loop-planner" || (len(ws.Plan.Steps) > 0 && ws.DefinitionOfDone != planBefore)) {
		l.plan = truncate(ws.DefinitionOfDone, planMaxPersistChars)
		l.planStructured = len(ws.Plan.Steps) > 0
		// The filing already painted a plan card; the full body is in the Plan panel.
		return nil
	}
	// Cursor plan mode may still put the brief in createPlan (despite the prompt
	// asking for mcp__recur__submit_plan). Prefer that over assistant narration
	// ("I've written a plan."), then res.Text, then streamed text blocks.
	fromCreatePlan := strings.TrimSpace(createPlanText) != ""
	text := strings.TrimSpace(createPlanText)
	if text == "" {
		text = strings.TrimSpace(res.Text)
	}
	if text == "" {
		text = strings.Join(textBlocks, "\n\n")
	}
	if text == "" {
		l.emitFeed(rs.code, rs.name,
			"The planning turn produced no plan — proceeding with the bare task.")
		return nil
	}
	// Keep the (bounded) plan in memory for runCoder to brief the Coder with (a
	// "lesser" agent needs the whole investigated plan, not a truncated summary) —
	// read only by this same loop goroutine later, so no lock is needed (see the
	// `plan` field doc). Bound it to planMaxPersistChars for the same reason the
	// persisted copy below is bounded — l.plan is injected verbatim into EVERY
	// Coder turn's system prompt for the life of the fresh run, so a runaway
	// planning response would otherwise pay its full token cost on every
	// convergence iteration. Bounding it here also keeps a fresh-run Coder and a
	// rework Coder (seeded from the truncated DefinitionOfDone in newLoop) in sync
	// on the same plan text.
	text = truncate(text, planMaxPersistChars)
	l.plan = text
	// createPlan → Recur Plan artifact (summary-only, no steps) so the Plan panel
	// / approval gate see it. Plain-text briefs still only mirror DefinitionOfDone.
	if fromCreatePlan {
		l.eng.adoptProsePlan(l.ID, rs.code, text)
		l.planStructured = false
	} else {
		// Persist a (generously-sized, but still bounded) copy as the loop's
		// definition-of-done for the UI/API. This persisted copy is NOT just a display
		// string — it is also the ONLY copy of the
		// plan a REWORK dispatch ever sees (newLoop seeds l.plan from it, since a rework
		// never re-runs runPlanner; l.plan above is in-memory and does not survive past
		// this goroutine). A thorough, Claude-Code-plan-mode-style brief (summary +
		// relevant files + numbered steps + edge cases + definition of done) routinely
		// runs several thousand characters, so the bound is set generously high
		// (planMaxPersistChars) rather than the old 4000, which could silently cut a
		// real plan off mid-document before a rework Coder ever saw the rest of it.
		l.eng.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) { lp.DefinitionOfDone = text })
	}
	// The persisted DefinitionOfDone keeps the full (up to planMaxPersistChars) plan,
	// since a rework dispatch needs the whole document — but the activity FEED is a
	// stream of many entries and every other feed path bounds text at feedInputLimit
	// (8000 bytes); rendering the full plan there would push a single 20KB+ HTML
	// blob over SSE and dwarf every other entry. Bound the feed's copy to the same
	// feedInputLimit other feed entries use; the full plan remains available via the
	// loop's DefinitionOfDone/plan APIs.
	// Prose plan (no submit_plan): a short preview here; the Plan panel has it all.
	l.emitFeed(rs.code, rs.name, "Plan &amp; definition-of-done: "+html.EscapeString(truncate(firstParagraph(text), 300))+
		" <span class=\"muted\">(full plan in the Plan panel)</span>")
	return nil
}

// planMaxPersistChars bounds the persisted copy of the Maestro's planning-turn
// brief (DefinitionOfDone). It intentionally sits well above a typical thorough
// plan-mode document's length — this task is explicitly about the Maestro
// producing MORE detailed plans, so the safety bound must not be the thing that
// truncates them for a rework dispatch, which persists as the only surviving copy
// of the plan (see runPlanner). It still exists as a bound (not unlimited) so a
// pathological/runaway response can't grow the stored loop record unboundedly.
// Note the activity FEED uses the smaller feedInputLimit instead (see runPlanner)
// so a single plan entry doesn't dwarf every other feed entry.
const planMaxPersistChars = 20000

// runCoder spawns the Coder agent in the worktree, streams its steps to the feed,
// then has the ENGINE commit the worktree changes (deterministic, testable — not
// reliant on the agent committing; PRD §11). On iteration > 1 it prompts the Coder
// with the previous iteration's reviewer findings to fix (the convergence loop).
// The Coder must mark each finding resolved/invalid via MCP before done; the
// engine does NOT blanket-resolve. Re-review re-files anything still broken.
func (l *Loop) runCoder(ctx context.Context, iter int, prevFindings []domain.Finding) error {
	ctx, cancel := l.turnContext(ctx)
	defer cancel()

	rs := l.slotRoleSpec("implementer", roles[domain.RoleCoder])
	agentID := l.ID + "-" + rs.code
	set := l.eng.settings()
	model, flags := l.roleModel(rs, set.CoderModel().String(), set.CoderFlags())
	l.eng.upsertAgent(l.ID, domain.Agent{
		ID: agentID, WorkspaceID: l.ID, Code: rs.code, Name: rs.name,
		Role: "writes & edits code", Model: domain.ParseModelRef(model).ID,
		Status: "working", Color: "var(--cyan)", Task: l.goalLabel(), Cur: -1,
	})

	// l.plan is the Maestro's planning-turn brief (PRD §7.2), set by runPlanner before
	// the loop ever reaches runCoder (or seeded from the persisted DefinitionOfDone
	// for a rework dispatch — see newLoop); empty when planning was skipped/degraded.
	// coderSystemPrompt attaches it (via planSection) with identical bytes on
	// every iteration so --resume / thread/resume can keep the cached prefix —
	// the per-iteration user prompts built below (coderFixPrompt/coderReworkPrompt)
	// deliberately do NOT repeat it, so a thorough multi-KB plan is not paid for
	// twice per turn. Findings/rework take precedence via those user prompts, not
	// by rewriting the system-prompt plan block (that used to bust the cache).

	// RESUMING (D55): the Coder's first turn after an interruption CONTINUES the
	// session it was killed in, so it still remembers the plan it was working through
	// and the edits it had already made. resumeSession is passed to the runner in every
	// branch below — continuity is orthogonal to which instruction the turn carries.
	resumeSession, resumed := l.resumedRole(rs.code)

	prompt := l.Goal
	coderDisplay := ""
	sysPrompt := coderSystemPrompt(l.Goal, l.plan, l.planStructured)
	if roleID := l.phaseRoleID("implementer"); roleID != "" {
		sysPrompt = l.bundleSystemPrompt(roleID, sysPrompt, l.Goal, planSection(l.plan))
	}
	switch {
	case len(l.pendingConflicts) > 0:
		// A child-branch merge left conflicts (fan-out): this turn resolves them.
		prompt = coderConflictPrompt(l.Goal, l.pendingConflicts)
		l.pendingConflicts = nil
	case iter > 1 && len(prevFindings) > 0:
		// Findings stay open until the Coder marks them via update_finding.
		prompt = coderFixPrompt(l.Goal, prevFindings)
		coderDisplay = coderDispatchDisplay(prompt)
		if s := l.phaseSlot("fix"); s != "" {
			expanded, display, err := l.expandPhasePrompt(ctx, "fix", prompt)
			if err != nil {
				return err
			}
			prompt = expanded
			if display != "" {
				coderDisplay = display
			}
		}
	case resumed:
		// Nothing more specific to say: without this the turn would restate the bare
		// goal, inviting an agent that has already done half the work to start over.
		prompt = coderResumePrompt(l.Goal, iter, resumeSession != "")
	default:
		if s := l.phaseSlot("implement"); s != "" {
			expanded, display, err := l.expandPhasePrompt(ctx, "implement", prompt)
			if err != nil {
				return err
			}
			prompt = expanded
			if display != "" {
				coderDisplay = display
			}
		}
	}
	if coderDisplay == "" {
		coderDisplay = coderDispatchDisplay(prompt)
	}
	if coderDisplay == "" && prompt == l.Goal {
		coderDisplay = l.goalLabel()
	}

	// Register a per-agent MCP token so the Coder's mcp__recur__* tools are
	// attributed to this loop+agent. Unregister at turn end unblocks permission
	// waits; the token stays so the next iteration reuses the same MCP URL.
	token, mcpURL, err := l.eng.registerMCP(l.ID, rs.code, l.ChatCode, rs.allowedTools, model)
	if err != nil {
		return err
	}
	defer l.eng.unregisterMCP(token)

	inject, clearInject := l.bindInject(rs.code)
	defer clearInject()

	// Acquire an agent slot (machine-global cap) before spawning (PRD §6.4).
	release, ok := l.acquireTurnSlot(ctx)
	if !ok {
		return ctx.Err()
	}
	defer release()

	// Post-exit guard: if the agent stops with open findings (skipped done or
	// ignored the tool error), re-enter with a nudge instead of advancing to review.
	const maxFindingNudges = 3
	for nudge := 0; ; nudge++ {
		spec := AgentSpec{
			WorkspaceID:        l.ID,
			ChatCode:           l.ChatCode,
			AgentID:            agentID,
			Role:               domain.RoleCoder,
			Model:              model,
			Flags:              flags,
			SystemPrompt:       sysPrompt,
			AppendSystemPrompt: l.eng.simplifyCoderAppend(l.ID),
			AllowedTools:       rs.allowedTools,
			Prompt:             l.applyPendingSteer(prompt),
			DisplayText:        coderDisplay,
			Worktree:           l.Worktree,
			PermissionMode:     set.PermissionMode(),
			MCPURL:             mcpURL,
			Inject:             inject,
			ResumeSession:      resumeSession,
		}
		l.applyGoalImages(&spec)

		res, err := l.eng.runAgent(ctx, spec, func(ev ParsedEvent) {
			l.onAgentEvent(rs, ev)
		})
		l.eng.publishResource()

		if err != nil || res.IsError {
			l.eng.updateAgentStatus(l.ID, rs.code, "idle")
			// A failed turn is CLASSIFIED (D56): a transient provider failure.
			return newTurnError(rs.name, err, res)
		}

		open := l.eng.Store.OpenFindings(l.ID)
		if len(open) == 0 {
			break
		}
		// Do NOT fail the loop here. A hard error would skip review and map to
		// StateFailed, bypassing the iteration-cap → rail #4 escalation path
		// that used to catch a Coder who couldn't/wouldn't close findings.
		// Proceed to review with opens still present: the Reviewer re-judges the
		// diff, and rail #3 / the cap still gate any PR.
		if nudge >= maxFindingNudges {
			l.emitFeed(rs.code, rs.name, fmt.Sprintf(
				"%d finding(s) still open after %d nudge(s) — proceeding to review anyway.",
				len(open), maxFindingNudges))
			break
		}
		l.emitFeed(rs.code, rs.name, fmt.Sprintf(
			"%d finding(s) still open — nudging Coder to close them before review.", len(open)))
		prompt = coderOpenFindingsNudgePrompt(l.Goal, open)
		if res.SessionID != "" {
			resumeSession = res.SessionID
		}
	}

	l.eng.updateAgentStatus(l.ID, rs.code, "idle")

	if err := l.commitWorktree(ctx, rs.code, rs.name, ""); err != nil {
		return err
	}
	l.recordHandoff(rs, l.takeDoneNotes())
	return nil
}

// commitWorktree stages and commits via the loop's own manager (D22 / PRD §11).
// Author is the user's git identity (D17). No-op when the tree is already clean.
// Empty msg builds the subject from the Coder's done summary or the goal.
func (l *Loop) commitWorktree(ctx context.Context, code, name, msg string) error {
	if msg == "" {
		msg = loopCommitMessage(l.takeDoneSummary(), l.goalLabel(), statusPaths(ctx, l.Worktree))
	}
	author, email, ierr := gitpkg.Identity(ctx, l.Worktree)
	if ierr != nil {
		return fmt.Errorf("engine commit identity: %w", ierr)
	}
	commit, cerr := l.git.Commit(ctx, l.Worktree, msg, author, email)
	if cerr != nil {
		return fmt.Errorf("engine commit: %w", cerr)
	}
	if commit.Committed {
		l.eng.updateLoopDiff(l.ID, commit.Additions, commit.Deletions)
		l.emitFeed(code, name, fmt.Sprintf(
			"Committed <code>%s</code> (+%d −%d) to <code>%s</code>.",
			html.EscapeString(commit.Hash), commit.Additions, commit.Deletions, html.EscapeString(l.Branch)))
	} else {
		l.emitFeed(code, name, "No file changes to commit this iteration.")
	}
	return nil
}

// reviewedCommitMessage describes a worktree no Coder turn wrote (start=review
// converged first try), so neither a done summary nor the loop goal can: the
// title model writes it from the diff. Empty on failure.
func (l *Loop) reviewedCommitMessage(ctx context.Context) string {
	if err := gitpkg.Stage(ctx, l.Worktree, []string{"."}); err != nil {
		return ""
	}
	diff, err := gitpkg.StagedDiff(ctx, l.Worktree)
	if err != nil {
		return ""
	}
	msg, err := l.eng.GenerateCommitMessage(ctx, l.ID, l.Worktree, diff, gitpkg.RecentCommitSubjects(ctx, l.Worktree, 8))
	if err != nil {
		return ""
	}
	return msg
}

// runReviewer spawns a SEPARATE Reviewer agent (rail #2: never the agent that
// wrote the code — asserted below in code, not relied on from a prompt) that
// reviews the current diff and files findings via the MCP report_finding tool. It
// returns the findings reported during this review iteration (read from the
// engine-counted total, NOT from anything the Reviewer says about convergence).
func (l *Loop) runReviewer(ctx context.Context, iter int) ([]domain.Finding, error) {
	ctx, cancel := l.turnContext(ctx)
	defer cancel()

	rs := l.slotRoleSpec("reviewer", roles[domain.RoleReviewer])
	coder := l.slotRoleSpec("implementer", roles[domain.RoleCoder])

	// Reviewer ≠ implementer: a distinct agent
	// code/id is a structural guarantee, not a prompt instruction.
	if rs.code == coder.code {
		return nil, fmt.Errorf("rail #2 violation: reviewer and coder share agent code %q", rs.code)
	}
	reviewerID := l.ID + "-" + rs.code
	coderID := l.ID + "-" + coder.code
	if reviewerID == coderID {
		return nil, fmt.Errorf("rail #2 violation: reviewer and coder share agent id %q", reviewerID)
	}

	set := l.eng.settings()
	model, flags := l.roleModel(rs, set.ReviewerModel().String(), set.ReviewerFlags())
	l.eng.upsertAgent(l.ID, domain.Agent{
		ID: reviewerID, WorkspaceID: l.ID, Code: rs.code, Name: rs.name,
		Role: "reviews the diff", Model: domain.ParseModelRef(model).ID,
		Status: "working", Color: "var(--indigo)", Task: l.goalLabel(), Cur: -1,
	})

	// Reset the per-iteration finding collector BEFORE the reviewer runs, so every
	// report_finding callback during this turn is counted for this iteration.
	l.beginReviewIteration(iter)

	token, mcpURL, err := l.eng.registerMCP(l.ID, rs.code, l.ChatCode, rs.allowedTools, model)
	if err != nil {
		return nil, err
	}
	defer l.eng.unregisterMCP(token)

	inject, clearInject := l.bindInject(rs.code)
	defer clearInject()

	sysPrompt := reviewerPromptFor(l, iter)
	if roleID := l.phaseRoleID("reviewer"); roleID != "" {
		if overlay := l.bundleSystemPrompt(roleID, "", l.Goal, ""); overlay != "" {
			sysPrompt = overlay
			if plan := strings.TrimSpace(l.plan); plan != "" {
				sysPrompt += "\n\nPlan / definition of done:\n" + plan
			}
			sysPrompt += coderDisputedFindingsSection(l, iter)
		}
	}
	if brief := l.findingsThresholdBriefing(); brief != "" {
		sysPrompt += "\n\n" + brief
	}
	sysPrompt += handoffSection(l.reviewHandoff())
	reviewUser := fmt.Sprintf("Review the current diff for iteration %d of: %s", iter, l.Goal)
	reviewDisplay := fmt.Sprintf("Review the current diff for iteration %d of: %s", iter, l.goalLabel())
	{
		slot := "review"
		if iter > 1 {
			slot = "rereview"
		}
		if s := l.phaseSlot(slot); s == "" && slot == "rereview" {
			slot = "review"
		}
		if s := l.phaseSlot(slot); s != "" {
			expanded, display, err := l.expandPhasePrompt(ctx, slot, reviewUser)
			if err != nil {
				return nil, err
			}
			reviewUser = expanded
			if display != "" {
				reviewDisplay = display
			}
		}
	}

	spec := AgentSpec{
		WorkspaceID:    l.ID,
		ChatCode:       l.ChatCode,
		AgentID:        reviewerID,
		Role:           domain.RoleReviewer,
		Model:          model,
		Flags:          flags,
		SystemPrompt:   sysPrompt,
		AllowedTools:   rs.allowedTools, // explore + Bash verify + report/list_finding (prompt: no edits)
		Prompt:         l.applyPendingSteer(reviewUser),
		DisplayText:    reviewDisplay,
		Worktree:       l.Worktree,
		PermissionMode: set.PermissionMode(),
		MCPURL:         mcpURL,
		Inject:         inject,
	}

	release, ok := l.acquireTurnSlot(ctx)
	if !ok {
		return nil, ctx.Err()
	}
	defer release()

	res, err := l.eng.runAgent(ctx, spec, func(ev ParsedEvent) {
		l.onAgentEvent(rs, ev)
	})
	l.eng.publishResource()
	l.eng.updateAgentStatus(l.ID, rs.code, "idle")

	if err != nil || res.IsError {
		return nil, newTurnError(rs.name, err, res)
	}

	// This-iteration filings (engine-counted via MCP), never the Reviewer's words.
	// The converge bar uses remaining opens, not this slice.
	n := l.findingsThisIteration()
	all := l.eng.Store.Findings(l.ID)
	out := make([]domain.Finding, 0, n)
	for _, f := range all {
		if f.Iteration == iter && f.Status == "open" {
			out = append(out, f)
		}
	}
	return out, nil
}

// baseBranchFor resolves the branch this loop's diffs, PRs, and rails compare
// against. The store is the source of truth so a mid-run retarget is visible
// without mutating Loop.BaseBranch from another goroutine. Fall back to the
// value captured at dispatch, then the project default, then "main".
func baseBranchFor(l *Loop) string {
	if rec, ok := l.eng.Store.Workspace(l.ID); ok && rec.BaseBranch != "" {
		return rec.BaseBranch
	}
	if l.BaseBranch != "" {
		return l.BaseBranch
	}
	if p, ok := l.eng.Store.Project(l.ProjectID); ok && p.BaseBranch != "" {
		return p.BaseBranch
	}
	return "main"
}

// recordConvergence appends one iteration's open-finding count to the loop's
// convergence series and emits loop.updated so the LoopDetail chart updates live
// (PRD §7.3). The bar color follows severity-ish bands: 0 = emerald (converged),
// else amber/coral as the count rises.
func (l *Loop) recordConvergence(iter, findings int) {
	point := domain.ConvPoint{Iteration: iter, Findings: findings, Color: convColor(findings)}
	updated, ok := l.eng.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) {
		// Replace an existing point for this iteration (defensive) or append.
		for i := range lp.Convergence {
			if lp.Convergence[i].Iteration == iter {
				lp.Convergence[i] = point
				return
			}
		}
		lp.Convergence = append(lp.Convergence, point)
	})
	if !ok {
		return
	}
	l.eng.publishLoopUpdated(l.ID, updated.State, updated.Phase, updated.Iteration)
}

// convColor maps a finding count to a convergence-bar color (CSS vars from the
// mockup). 0 findings = converged (emerald); a few = amber; many = coral.
func convColor(findings int) string {
	switch {
	case findings == 0:
		return "var(--emerald)"
	case findings <= 3:
		return "var(--amber)"
	default:
		return "var(--coral)"
	}
}

// setIteration records the loop's current iteration number on the loop record.
// It also advances the resume checkpoint's iteration so the checkpoint written by
// the enclosing setState (which ran before the iteration was bumped) points at the
// iteration actually about to run (D55).
func (l *Loop) setIteration(iter int) {
	l.mu.Lock()
	l.walkIter = iter
	l.mu.Unlock()
	l.eng.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) {
		lp.Iteration = iter
		if lp.Checkpoint != nil {
			lp.Checkpoint.Iteration = iter
		}
	})
}

// onAgentEvent turns a parsed agent stream event into feed + agent.step events.
func (l *Loop) onAgentEvent(rs roleSpec, ev ParsedEvent) {
	if ev.nested() {
		return
	}
	switch ev.Kind {
	case KindStep:
		l.eng.appendAgentStep(l.ID, rs.code, domain.AgentStep{
			Kind: stepKind(displayToolFor(ev).Name), Text: displayToolFor(ev).Name + " " + ev.ToolInput,
		})
		text := html.EscapeString(displayToolFor(ev).Name)
		if ev.ToolInput != "" {
			text += ": <code>" + html.EscapeString(ev.ToolInput) + "</code>"
		}
		l.eng.emitWorkspaceEventItem(l.ID, domain.FeedItem{
			AgentCode: rs.code,
			Name:      rs.name,
			Text:      text,
			ToolUseID: ev.ToolUseID,
			ChatCode:  l.ChatCode,
		})
	case KindToolResult:
		l.eng.attachToolResult(l.ID, ev.ToolUseID, ev.Text, ev.IsError)
	case KindText, KindNotice:
		// KindText is assistant narration; KindNotice is Recur status (e.g. a
		// leftover-run drain). Both are Activity notes, not engine outcomes.
		if t := truncate(ev.Text, feedInputLimit); t != "" {
			l.eng.emitWorkspaceEventItem(l.ID, domain.FeedItem{
				AgentCode: rs.code,
				Name:      rs.name,
				Text:      html.EscapeString(t),
				Kind:      domain.FeedKindNote,
			})
		}
	case KindUser:
		if t := truncate(ev.Text, feedInputLimit); t != "" {
			l.eng.emitWorkspaceEventItem(l.ID, domain.FeedItem{
				AgentCode: rs.code,
				Name:      rs.name,
				Text:      html.EscapeString(t),
				Kind:      domain.FeedKindNote,
			})
		}
	case KindInit:
		// Record the session id on the agent for later inspection/resume.
		l.eng.setAgentSession(l.ID, rs.code, ev.SessionID)
	}
}

// stepKind maps a tool name to the AgentStep.kind tint used by the UI.
func stepKind(tool string) string {
	switch tool {
	case "Read", "Grep":
		return "read"
	case "Edit", "Write", "Delete":
		return "edit"
	case "Bash":
		return "run"
	default:
		return "note"
	}
}

// leaveStateOnShutdown is true when this goroutine is exiting because the app
// is quitting, not because the human pressed Stop. Leave the persisted state
// alone so Recover keeps an unanswered escalation waiting (and parks in-flight
// work as interrupted) instead of recording a fake Stop.
func (l *Loop) leaveStateOnShutdown() bool {
	if l == nil || l.eng == nil {
		return false
	}
	// Same lock Stop holds when accepting a running stop, so a concurrent Stop
	// either sets l.stopped before this snapshot or persistStop after it.
	l.eng.teardownMu.Lock()
	defer l.eng.teardownMu.Unlock()
	if !l.eng.shuttingDown {
		return false
	}
	l.mu.Lock()
	stopped := l.stopped
	paused := l.paused
	escalated := l.escalated
	l.mu.Unlock()
	if stopped {
		return false
	}
	// Pause is accepted in send() immediately, but StatePaused is only written
	// at the next checkpoint. Snapshot it here so Pause-then-quit is not
	// recovered as interrupted and auto-resumed.
	if paused && !escalated {
		l.setState(domain.StatePaused, "paused")
	}
	return true
}

// finishStopped records a Stop/cancel terminal state.
func (l *Loop) finishStopped(ctx context.Context) {
	if l.leaveStateOnShutdown() {
		return
	}
	l.setState(domain.StateStopped, "stopped")
	l.emitFeed("MA", "Maestro", "Loop stopped.")
}

// finishWalkAbort parks idle when composer Stop requested it; otherwise Stop.
func (l *Loop) finishWalkAbort(ctx context.Context) {
	if l.wantsIdle() {
		ws, ok := l.eng.Store.Workspace(l.ID)
		if ok && ws.State == domain.StatePaused && ws.Phase == "idle" {
			return
		}
		l.setState(domain.StatePaused, "idle")
		l.emitFeed("MA", "Maestro", "Walk cancelled.")
		return
	}
	l.finishStopped(ctx)
}

// recoverPanic converts a panic in the loop goroutine into a failed loop instead
// of crashing the whole process (D25). It is the innermost Run defer, so the slot
// release + queued drain (loopExited) and the WaitGroup Done still run afterwards
// and other loops are unaffected. The stack is logged for diagnosis.
func (l *Loop) recoverPanic() {
	r := recover()
	if r == nil {
		return
	}
	log.Printf("recur: loop %s panicked: %v\n%s", l.ID, r, debug.Stack())
	l.setState(domain.StateFailed, "failed")
	l.emitFeed("MA", "Maestro", fmt.Sprintf("Loop failed (internal error): %s", html.EscapeString(fmt.Sprint(r))))
}

// fail records a failed state with the reason. A TRANSIENT provider failure (spend or
// rate limit, overload, dropped connection) is recorded as RETRYABLE (D56): the loop
// keeps its checkpoint, lands in "needs you" with the provider's own message, and the
// UI offers Retry to re-enter where it stopped. Any other failure stays terminal —
// re-running a genuine defect would just fail again.
func (l *Loop) fail(ctx context.Context, err error) {
	// A Stop manifests as ctx cancellation inside the agent — report it as stopped,
	// not failed, so the UI is honest.
	if ctx.Err() != nil {
		l.finishWalkAbort(ctx)
		return
	}
	if reason, transient := transientFailure(err); transient {
		l.eng.failRetryable(l.ID, reason)
		return
	}
	l.eng.recordFailure(l.ID, err.Error(), false)
	l.setState(domain.StateFailed, "failed")
	l.emitFeed("MA", "Maestro", "Loop failed: "+html.EscapeString(err.Error()))
}

// ---- emit helpers (write through the engine so events persist + fan out) ----

func (l *Loop) setState(state domain.WorkspaceState, phase string) {
	l.mu.Lock()
	l.walkState = state
	l.mu.Unlock()
	l.exitState = state
	l.exitPhase = phase
	l.eng.setWorkspaceState(l.ID, state, phase)
}

func (l *Loop) emitFeed(code, name, textHTML string) {
	l.eng.emitWorkspaceEventItem(l.ID, domain.FeedItem{
		AgentCode: code, Name: name, Text: textHTML, ChatCode: l.ChatCode,
	})
}
