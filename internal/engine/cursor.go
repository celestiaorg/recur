package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"recur/internal/cursor"
	"recur/internal/domain"
)

// CursorRunner is the Cursor AgentRunner: it drives Cursor's agent through the
// local cursor-sdk-bridge (internal/cursor) instead of spawning a CLI.
//
// It is a peer of ClaudeRunner and satisfies the same AgentRunner contract, so
// the engine, state machine and persistence are untouched by which provider ran
// a turn. The mapping is:
//
//	AgentSpec.Worktree  → local.cwd (the run's project root)
//	AgentSpec.Model     → model.id  (a Cursor model id, e.g. composer-2.5)
//	AgentSpec.MCPURL    → mcpServers["recur"], passed INLINE
//	AgentSpec.Playwright→ mcpServers["playwright"] (stdio, via npx)
//	tool_call events    → KindStep / KindToolResult, with mcp tools relabelled
//	                      to Claude Code's mcp__server__tool spelling
//
// Two AgentSpec fields cannot be honoured on Cursor. They are limits of
// Cursor's backend, not of this client, and are reported rather than silently
// dropped:
//
//   - SystemPrompt: Cursor exposes no custom system prompt (the CLI's hidden
//     --system-prompt is gated to Anysphere/OpenAI accounts and the backend
//     rejects it). The role prompt is PREPENDED to the user prompt instead.
//   - AllowedTools: there is no per-run tool allowlist. Read-only roles are
//     mapped onto Cursor's "plan" mode, which is the closest equivalent; other
//     roles run with the full tool set inside the loop's isolated worktree.
//
// Inject IS honoured. Cursor's run.steer appends a user message to the
// turn in flight (SteerRun). complete_delivered keeps the current Send.
// revert_to_followup, a missing RPC, or any other failure maps to CancelRun
// plus a follow-up Send of the concatenated steers on the SAME agentId —
// still inside this Run, so the engine does not see a failed specialist turn.
type CursorRunner struct {
	// Bridge is the already-started bridge for this runner's worktree. The
	// engine pools one bridge per worktree; agents on that worktree are
	// multiplexed over it by agentId.
	Bridge *cursor.Bridge

	// APIKey authenticates runs. Also passed to the bridge process as
	// CURSOR_API_KEY, but sent per-call so a settings change takes effect
	// without a bridge restart.
	APIKey string

	// StrictConfig suppresses the user's ambient Cursor config (~/.cursor and
	// the repo's own .cursor/rules + AGENTS.md). Off by default: a repo's rules
	// are legitimate project context, exactly as ClaudeRunner lets CLAUDE.md
	// load.
	StrictConfig bool

	// skipObserve is set when the bridge does not advertise run.observe.
	// Recovery then goes straight to cancel+Send. Zero value (tests) still
	// tries ObserveRun.
	skipObserve bool

	// steerUnavailable is set when SteerRun is unimplemented so later Injects
	// skip the RPC and go straight to CancelRun+Send.
	steerUnavailable atomic.Bool

	// inUse is outstanding pool handouts of this shared runner. ensureRunner
	// increments it under the pool mutex; releaseRunner decrements after the turn.
	inUse atomic.Int32
}

// NewCursorRunner returns a runner bound to an already-started bridge.
func NewCursorRunner(b *cursor.Bridge, apiKey string) *CursorRunner {
	return &CursorRunner{Bridge: b, APIKey: apiKey}
}

// NeedsPermissionPromptTool reports that Cursor has no --permission-prompt-tool
// equivalent (docs/cursor-provider.md). The tool must stay off Cursor MCP
// bindings — advertising it is pure spurious-call surface.
func (*CursorRunner) NeedsPermissionPromptTool() bool { return false }

// Run executes one agent turn and streams parsed events to emit.
func (r *CursorRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if r.Bridge == nil || r.Bridge.Client() == nil {
		return Result{}, fmt.Errorf("cursor runner: bridge not started")
	}
	cl := r.Bridge.Client()

	opts := r.agentOptions(spec)

	// Reuse the prior agent when the engine asked to continue a session;
	// otherwise start a fresh one. ResumeSession carries the Cursor agentId for
	// this provider (it carries a Claude session id under ClaudeRunner).
	var (
		agentID string
		err     error
		resumed bool
	)
	if spec.ResumeSession != "" {
		agentID, err = cl.ResumeAgent(ctx, spec.ResumeSession, opts)
		if err != nil {
			// A resumed agent can legitimately be gone (bridge restarted,
			// state pruned). Fall back to a fresh agent rather than failing the
			// turn — the engine will persist the new id. Standing notes must
			// ride in the user message again (the new agent has no history).
			agentID, err = cl.CreateAgent(ctx, opts)
		} else {
			resumed = true
		}
	} else {
		agentID, err = cl.CreateAgent(ctx, opts)
	}
	if err != nil {
		return Result{}, fmt.Errorf("cursor create agent: %w", r.enrichErr(err))
	}

	// The bridge holds per-agent state until told otherwise. Closing is
	// best-effort: leaking a little bridge memory must never fail a good turn.
	// CancelRun (below) must run BEFORE CloseAgent so the provider actually
	// stops — aborting our stream alone leaves Cursor (and its sub-agents)
	// working in the background.
	defer func() {
		cctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), closeAgentTimeout)
		_ = cl.CloseAgent(cctx, agentID)
		cancel()
	}()

	// system/init equivalent: Cursor has no init event carrying the model and
	// MCP status, so synthesise one from what we just configured. The activity
	// feed and the engine's MCP health check both expect it.
	if emit != nil {
		emit(ParsedEvent{
			Kind:       KindInit,
			SessionID:  agentID,
			Model:      spec.Model,
			MCPServers: mcpStatusFor(spec),
		})
	}

	st := &cursorTurn{spec: spec, emit: emit, agentID: agentID, gotRunID: make(chan string, 1), deferResult: true}
	// Aborting the HTTP stream only disconnects Recur. Cursor's CancelRun is
	// what stops the provider-side agent (including Task sub-agents tied to
	// that run). Fire as soon as ctx is cancelled AND we know a run id —
	// waiting until Send returns leaves the agent running through teardown.
	//
	// Steer cancels MUST NOT share this Once: a later user Stop still has to
	// CancelRun the follow-up Send's new run id.
	var stopCancelOnce sync.Once
	cancelRuns := func(parent context.Context, ids []string) {
		if len(ids) == 0 {
			return
		}
		if parent == nil {
			parent = context.Background()
		}
		cctx, cancel := context.WithTimeout(parent, cancelRunTimeout)
		defer cancel()
		for _, id := range ids {
			_ = cl.CancelRun(cctx, id)
		}
	}
	doStopCancel := func() {
		stopCancelOnce.Do(func() {
			if len(st.allRunIDs()) == 0 {
				_ = st.waitRunID(2 * time.Second)
			}
			cancelRuns(context.Background(), st.allRunIDs())
		})
	}
	stopWatch := make(chan struct{})
	watchDone := make(chan struct{})
	go func() {
		defer close(watchDone)
		select {
		case <-ctx.Done():
			doStopCancel()
		case <-stopWatch:
		}
	}()
	defer func() {
		close(stopWatch)
		<-watchDone
	}()

	var (
		sendMu     sync.Mutex
		sendGen    uint64
		sendCancel context.CancelFunc
	)
	setSendCancel := func(c context.CancelFunc) {
		sendMu.Lock()
		sendGen++
		sendCancel = c
		sendMu.Unlock()
	}
	currentSendGen := func() uint64 {
		sendMu.Lock()
		defer sendMu.Unlock()
		return sendGen
	}
	// interruptSend cancels only the Send that was in flight when gen was
	// captured. CancelRun can make Send return before interruptSend runs; the
	// main loop then installs the follow-up's cancel. A stale interrupt must
	// not kill that follow-up (TestCursorRunnerSteerCancelsThenSends).
	interruptSend := func(gen uint64) {
		sendMu.Lock()
		c := sendCancel
		g := sendGen
		sendMu.Unlock()
		if c != nil && g == gen {
			c()
		}
	}

	var steers steerBuf
	defer func() {
		replyInject(spec, len(steers.take()), false)
	}()
	steerCtx, stopSteer := context.WithCancel(ctx)
	steerDone := make(chan struct{})
	go func() {
		defer close(steerDone)
		if spec.Inject == nil {
			return
		}
		for {
			select {
			case <-steerCtx.Done():
				return
			case msg, ok := <-spec.Inject:
				if !ok {
					return
				}
				msg = strings.TrimSpace(msg)
				if msg == "" {
					continue
				}
				steers.add(msg)
				drainInject(spec.Inject, &steers)
				for {
					drainInject(spec.Inject, &steers)
					reserved := steers.reserve()
					if len(reserved) == 0 {
						break
					}
					if testAfterSteerReserve != nil {
						testAfterSteerReserve()
					}
					joined := followUpInjectText(spec, reserved)
					if joined != "" && r.trySteer(steerCtx, st, joined) {
						// Flush+KindUser before commit so waitReserved keeps the
						// main loop off finish() until the live-steer events land.
						fireFollowUpUsers(st, spec, reserved, joined, true)
						steers.commit()
						replyInject(spec, len(reserved), true)
						continue
					}
					steers.release()
					batch := steers.batchSeq()
					// Suppress a CANCELLED result frame so session.turn_end does
					// not fire; a FINISHED result still wins (late steer).
					st.setSuppressResult(true)
					gen := currentSendGen()
					if !steers.consumed(batch) && len(st.allRunIDs()) == 0 {
						_ = st.waitRunIDCtx(steerCtx, 2*time.Second)
					}
					// take() acks this batch when the main loop folds it into a
					// follow-up prompt. CancelRun and interruptSend would then
					// kill that Send even though the steer is already in the prompt.
					if steers.consumed(batch) {
						continue
					}
					// Same RPC as SDK run.cancel — not the Stop Once, so a later
					// ctx cancel can still stop a follow-up run. Bound by steerCtx
					// so Stop does not wait the full CancelRun timeout.
					cancelRuns(steerCtx, st.allRunIDs())
					drainInject(spec.Inject, &steers)
					interruptSend(gen)
					break
				}
			}
		}
	}()
	defer func() {
		stopSteer()
		<-steerDone
	}()

	prompt := r.wirePrompt(spec, standingInUserPrompt(spec, resumed))
	images := cursorImages(spec)
	followUp := false
	for {
		sendCtx, cancelSend := context.WithCancel(ctx)
		setSendCancel(cancelSend)
		// Steers that landed after the previous take() — during
		// prepareSteerFollowUp — belong on this Send, not a post-completion
		// discard. take() acks the batch so the listener skips CancelRun and
		// interruptSend of this new run. A still-later Inject is not acked
		// and still cancels.
		if followUp {
			if testBeforeFollowUpTake != nil {
				testBeforeFollowUpTake()
			}
			pending := steers.take()
			if extra := followUpInjectText(spec, pending); extra != "" {
				replyInject(spec, len(pending), true)
				fireFollowUpUsers(st, spec, pending, extra, false)
				prompt = prompt + "\n\n" + extra
			} else {
				replyInject(spec, len(pending), false)
			}
		}

		sendErr := r.sendOnce(sendCtx, cl, agentID, prompt, images, st)

		if ctx.Err() != nil {
			cancelSend()
			doStopCancel()
			st.finish()
			return st.result(), ctx.Err()
		}

		if st.haveResult {
			waitCtx, cancelWait := context.WithTimeout(ctx, 2*time.Second)
			steers.waitReserved(waitCtx)
			cancelWait()
			replyInject(spec, steers.abandon(), false)
		} else {
			steers.waitReserved(ctx)
		}
		pending := steers.take()
		if joined := followUpInjectText(spec, pending); joined != "" && !st.haveResult {
			replyInject(spec, len(pending), true)
			cancelSend()
			st.prepareSteerFollowUp()
			fireFollowUpUsers(st, spec, pending, joined, false)
			prompt = joined
			images = nil
			followUp = true
			continue
		}
		replyInject(spec, len(pending), false)
		// Late Inject after FINISHED: interruptSend cancels the stream tail,
		// but the turn already completed. Do not report that as a failed run.
		lateSteerCancel := st.haveResult && sendCtx.Err() != nil
		cancelSend()
		st.finish()

		if sendErr != nil && !lateSteerCancel {
			var cerr *cursor.Error
			if errors.As(sendErr, &cerr) && cerr.Unauthenticated() {
				return st.result(), fmt.Errorf("cursor auth failed (reconnect Cursor in Settings): %w", r.enrichErr(sendErr))
			}
			return st.result(), fmt.Errorf("cursor run: %w", r.enrichErr(sendErr))
		}
		if !st.haveResult {
			tail := ""
			if r.Bridge != nil {
				tail = r.Bridge.StderrTail()
			}
			if tail != "" {
				log.Printf("recur: cursor run ended without a result\n%s", tail)
			}
			return st.result(), errors.New(cursorNoResultMessage(tail, r.bridgeLogHint()))
		}
		return st.result(), nil
	}
}

// sendOnce streams one Send. A leftover activeRunId (dropped Send stream)
// is drained or cancelled via recoverActiveRun, then this prompt is retried.
func (r *CursorRunner) sendOnce(sendCtx context.Context, cl *cursor.Client, agentID, prompt string, images []cursor.Image, st *cursorTurn) error {
	sendErr := cl.Send(sendCtx, agentID, prompt, images, st.onEvent)
	if sendErr != nil && cursor.ActiveRunConflict(sendErr) && sendCtx.Err() == nil {
		sendErr = r.recoverActiveRun(sendCtx, cl, agentID, prompt, images, st)
	}
	return sendErr
}

func (r *CursorRunner) trySteer(ctx context.Context, st *cursorTurn, text string) bool {
	if r == nil || st == nil || text == "" || r.steerUnavailable.Load() {
		return false
	}
	if r.Bridge == nil || r.Bridge.Client() == nil {
		return false
	}
	id := st.parentRun()
	if id == "" {
		id = st.waitRunIDCtx(ctx, 2*time.Second)
	}
	if id == "" {
		return false
	}
	outcome, err := r.Bridge.Client().SteerRun(ctx, id, text)
	if err != nil {
		var cerr *cursor.Error
		if errors.As(err, &cerr) && cerr.Unimplemented() {
			r.steerUnavailable.Store(true)
		}
		return false
	}
	return outcome == cursor.SteerCompleteDelivered
}

// steerBuf collects Inject payloads until the follow-up Send. Cursor concatenates
// them: one CancelRun, one Send, every waiting steer in the user message.
//
// seq/acked let the main loop take() a batch into a follow-up prompt while the
// listener is mid-wait: without that ack, the listener's CancelRun hits the new
// run and the turn ends without a result.
//
// reserved holds a batch exclusively owned by an in-flight SteerRun so take()
// cannot fold or false-ack the same text.
type steerBuf struct {
	mu       sync.Mutex
	msgs     []string
	seq      uint64
	acked    uint64
	reserved []string
	resSeq   uint64
	idle     chan struct{} // closed when reserved clears; nil when idle
}

func (s *steerBuf) add(msg string) {
	s.mu.Lock()
	s.seq++
	s.msgs = append(s.msgs, msg)
	s.mu.Unlock()
}

func (s *steerBuf) take() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := s.msgs
	s.msgs = nil
	if len(s.reserved) == 0 {
		s.acked = s.seq
	}
	return out
}

func (s *steerBuf) snapshot() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]string(nil), s.msgs...)
}

func (s *steerBuf) takePrefix(n int) []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if n <= 0 {
		return nil
	}
	if n > len(s.msgs) {
		n = len(s.msgs)
	}
	out := append([]string(nil), s.msgs[:n]...)
	s.msgs = s.msgs[n:]
	if len(s.msgs) == 0 {
		s.acked = s.seq
	}
	return out
}

func (s *steerBuf) batchSeq() uint64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.seq
}

func (s *steerBuf) consumed(batch uint64) bool {
	if s == nil || batch == 0 {
		return false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.acked >= batch
}

func (s *steerBuf) reserve() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if len(s.reserved) > 0 || len(s.msgs) == 0 {
		return nil
	}
	s.resSeq = s.seq
	out := s.msgs
	s.msgs = nil
	s.reserved = out
	s.idle = make(chan struct{})
	return append([]string(nil), out...)
}

func (s *steerBuf) commit() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.acked = s.resSeq
	s.reserved = nil
	s.finishIdleLocked()
}

func (s *steerBuf) release() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.msgs = append(append([]string(nil), s.reserved...), s.msgs...)
	s.reserved = nil
	s.finishIdleLocked()
}

func (s *steerBuf) finishIdleLocked() {
	if s.idle != nil {
		close(s.idle)
		s.idle = nil
	}
}

func (s *steerBuf) waitReserved(ctx context.Context) {
	if s == nil {
		return
	}
	for {
		s.mu.Lock()
		if len(s.reserved) == 0 {
			s.mu.Unlock()
			return
		}
		ch := s.idle
		s.mu.Unlock()
		if ch == nil {
			return
		}
		if ctx == nil {
			<-ch
			continue
		}
		select {
		case <-ch:
		case <-ctx.Done():
			return
		}
	}
}

func (s *steerBuf) abandon() int {
	if s == nil {
		return 0
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	n := len(s.msgs) + len(s.reserved)
	s.msgs = nil
	s.reserved = nil
	s.acked = s.seq
	s.finishIdleLocked()
	return n
}

func replyInject(spec AgentSpec, n int, ok bool) {
	if spec.InjectAck == nil || n <= 0 {
		return
	}
	for i := 0; i < n; i++ {
		spec.InjectAck <- ok
	}
}

// testBeforeFollowUpTake, if set, runs after the follow-up Send is armed and
// before steers.take(), so tests can inject a steer that captures the new
// generation (TestCursorRunnerHandoffSteerDoesNotCancelFollowUp).
var testBeforeFollowUpTake func()

// testAfterSteerReserve, if set, runs once the listener owns a steer batch for
// SteerRun, so a handoff test can wait for the reserve instead of sleeping.
var testAfterSteerReserve func()

func drainInject(ch <-chan string, steers *steerBuf) {
	if ch == nil || steers == nil {
		return
	}
	for {
		select {
		case msg, ok := <-ch:
			if !ok {
				return
			}
			msg = strings.TrimSpace(msg)
			if msg != "" {
				steers.add(msg)
			}
		default:
			return
		}
	}
}

// fireFollowUpUsers records KindUser for a Cursor follow-up Send or a live
// delivered steer. Workspace chat (InjectPlain) keeps one live bubble per
// follow-up; Maestro D49 keeps a single prefixed row matching the concatenated
// Send body. live is true when SteerRun delivered into the current run.
func fireFollowUpUsers(st *cursorTurn, spec AgentSpec, msgs []string, joined string, live bool) {
	if st == nil || joined == "" {
		return
	}
	st.emitMu.Lock()
	defer st.emitMu.Unlock()
	if live {
		st.flushThinking()
		st.flushText()
	}
	if spec.InjectPlain {
		for _, m := range msgs {
			p, d := decodeInjectPlain(m)
			if p == "" {
				continue
			}
			if d == p {
				d = ""
			}
			st.emitLocked(ParsedEvent{Kind: KindUser, Text: p, DisplayText: d, LiveSteer: live})
		}
		return
	}
	st.emitLocked(ParsedEvent{Kind: KindUser, Text: joined, LiveSteer: live})
}

// bridgeTailLooksLikeShellSpawnCrash reports the Cursor bridge crash signature
// where Node fails to spawn the shell binary (usually a missing working_directory).
// Both "spawn /bin/" and "ENOENT" must appear on the same stderr line so an older
// non-fatal `spawn npx ENOENT` (e.g. Playwright MCP) cannot misattribute a later
// result-less turn.
func bridgeTailLooksLikeShellSpawnCrash(tail string) bool {
	for _, ln := range strings.Split(tail, "\n") {
		if strings.Contains(ln, "spawn /bin/") && strings.Contains(ln, "ENOENT") {
			return true
		}
	}
	return false
}

// enrichErr points opaque bridge/Connect failures at the on-disk log. The
// Connect client already writes the fault body into that log (the Node bridge
// does not); here we also mirror transient codes to the app log and append the
// log path so the UI's "[internal] internal error" is actionable.
func (r *CursorRunner) enrichErr(err error) error {
	if err == nil || r == nil || r.Bridge == nil {
		return err
	}
	var cerr *cursor.Error
	if errors.As(err, &cerr) && cerr.Transient() {
		if tail := r.Bridge.StderrTail(); tail != "" {
			log.Printf("recur: cursor bridge %v\n%s", err, tail)
		} else if cerr.Raw != "" {
			log.Printf("recur: cursor bridge %v\n%s", err, cerr.Raw)
		}
	}
	if p := r.Bridge.LogPath(); p != "" {
		return fmt.Errorf("%w (bridge log: %s)", err, p)
	}
	return err
}

func (r *CursorRunner) bridgeLogHint() string {
	if r == nil || r.Bridge == nil {
		return ""
	}
	if p := r.Bridge.LogPath(); p != "" {
		return " (bridge log: " + p + ")"
	}
	return ""
}

// cursorNoResultMessage is the Run error when Send returned with no result
// frame. Chat gets a one-line reason plus the log path; the stderr ring is
// written to Recur's log at the call site.
func cursorNoResultMessage(tail, logHint string) string {
	msg := "cursor run ended without a result"
	switch {
	case bridgeTailLooksLikeShellSpawnCrash(tail):
		msg += ": bridge crashed on Shell spawn ENOENT (often a missing working_directory such as ~/.cursor/projects/…/terminals); retry the turn"
	case strings.Contains(tail, "SIGTERM"):
		msg += ": the Cursor bridge stopped mid-turn; retry"
	}
	return msg + logHint
}

// unreportedResultText is what a tool call shows when Cursor never reported one.
// Deliberately states only what is known — the provider gives no reason, and the
// most common cause (a missing file) is a guess we should not print as fact.
const unreportedResultText = "No result reported. Cursor sends no terminal frame " +
	"for a tool call that fails, so the reason is not available — the agent's own " +
	"reply usually explains it."

// steerCancelledCallText is the synthetic result when a Maestro steer CancelRun
// interrupts in-flight tools. Those calls did not fail; Recur cancelled them.
const steerCancelledCallText = "Interrupted by a Maestro steer — this call was cancelled, not failed."

// closeAgentTimeout bounds the best-effort CloseAgent on the way out.
const closeAgentTimeout = 10 * time.Second

// cancelRunTimeout bounds the best-effort CancelRun after the user (or a
// deadline) interrupts a turn.
const cancelRunTimeout = 10 * time.Second

// observeIdleTimeout is how long ObserveRun may sit with no progress frames
// (Offset set) before recovery falls back to cancel + Send. Bridge keepalives
// are empty envelopes with no offset, so a stuck run.wait() does not reset
// this. A live tool call often emits only "running" and then nothing until it
// completes (see cursor/events.go), so this bound must exceed realistic tool
// wall time; a hung wait() trips any finite deadline. Tests shorten this.
var observeIdleTimeout = 10 * time.Minute

// recoverActiveRun drains or clears a leftover run, then Sends this turn's
// prompt. Observe frames are not folded into st — that would persist the
// previous run as this turn's answer and duplicate content if Send retried.
func (r *CursorRunner) recoverActiveRun(ctx context.Context, cl *cursor.Client, agentID, prompt string, images []cursor.Image, st *cursorTurn) error {
	cctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), cancelRunTimeout)
	run, lerr := cl.FirstNonTerminalRun(cctx, agentID)
	cancel()
	if lerr != nil {
		log.Printf("recur: cursor list runs on %s after active-run conflict: %v", agentID, lerr)
	} else if run.RunID != "" && !r.skipObserve && cursor.RunCreatedOnThisBridge(run, r.bridgeStarted()) {
		log.Printf("recur: cursor draining active run %s on %s before Send", run.RunID, agentID)
		st.noteRunID(run.RunID)
		st.fire(ParsedEvent{Kind: KindNotice, Text: "Waiting for previous run " + run.RunID + " to finish."})
		obsErr := r.observeRunToIdle(ctx, cl, st, run.RunID)
		if obsErr == nil {
			st.clearSteerTarget()
			sendErr := cl.Send(ctx, agentID, prompt, images, st.onEvent)
			// Observe EOF is not proof the store cleared activeRunId.
			if sendErr == nil || !cursor.ActiveRunConflict(sendErr) || ctx.Err() != nil {
				return sendErr
			}
			log.Printf("recur: cursor send after observe still conflicted on %s; cancelling", agentID)
		} else if ctx.Err() != nil {
			return obsErr
		} else {
			log.Printf("recur: cursor observe run %s on %s failed (%v); cancelling and retrying Send", run.RunID, agentID, obsErr)
		}
		st.fire(ParsedEvent{Kind: KindNotice, Text: "Previous run did not finish; cancelling and retrying."})
	} else if run.RunID != "" {
		if r.skipObserve {
			log.Printf("recur: cursor skipping observe of run %s on %s (bridge lacks run.observe)", run.RunID, agentID)
		} else {
			log.Printf("recur: cursor skipping observe of run %s on %s (not created on this bridge)", run.RunID, agentID)
		}
	}

	cctx, cancel = context.WithTimeout(context.WithoutCancel(ctx), cancelRunTimeout)
	n, cerr := cl.CancelNonTerminalRuns(cctx, agentID)
	cancel()
	if cerr != nil {
		log.Printf("recur: cursor clear stale runs on %s: %v", agentID, cerr)
	} else if n > 0 {
		log.Printf("recur: cursor cleared %d stale run(s) on %s after active-run conflict", n, agentID)
	}
	st.clearSteerTarget()
	return cl.Send(ctx, agentID, prompt, images, st.onEvent)
}

// observeRunToIdle waits for an existing run to finish without emitting into
// the current turn. The idle deadline resets only on frames with a non-empty
// Offset — keepalives do not count — so durable replay cannot disarm it.
func (r *CursorRunner) observeRunToIdle(ctx context.Context, cl *cursor.Client, st *cursorTurn, runID string) error {
	obsCtx, stop := context.WithCancel(ctx)
	defer stop()
	var mu sync.Mutex
	lastProgress := time.Now()
	go func() {
		timer := time.NewTimer(observeIdleTimeout)
		defer timer.Stop()
		for {
			select {
			case <-obsCtx.Done():
				return
			case <-timer.C:
				mu.Lock()
				wait := observeIdleTimeout - time.Since(lastProgress)
				mu.Unlock()
				if wait <= 0 {
					stop()
					return
				}
				timer.Reset(wait)
			}
		}
	}()
	err := cl.ObserveRun(obsCtx, runID, "", func(ev cursor.RunStreamEvent) error {
		if ev.Offset != "" {
			mu.Lock()
			lastProgress = time.Now()
			mu.Unlock()
		}
		if ev.Result != nil {
			st.noteRunID(ev.Result.RunID)
			if ev.Result.Result != nil {
				st.noteRunID(ev.Result.Result.RunID)
			}
		}
		if ev.SDKMessage != nil {
			st.noteRunID(ev.SDKMessage.RunID)
		}
		return nil
	})
	if err != nil && ctx.Err() == nil && obsCtx.Err() != nil {
		return fmt.Errorf("observe idle deadline exceeded: %w", err)
	}
	return err
}

func (r *CursorRunner) bridgeStarted() time.Time {
	if r == nil || r.Bridge == nil {
		return time.Time{}
	}
	return r.Bridge.StartedAt()
}

// agentOptions maps an AgentSpec onto sdk.v1 AgentOptions.
func (r *CursorRunner) agentOptions(spec AgentSpec) cursor.AgentOptions {
	local := &cursor.LocalOptions{
		// Always send enabled:false — see LocalOptions.Sandbox comment.
		Sandbox: cursor.SandboxDisabled,
	}
	if spec.Worktree != "" {
		local.Cwd = []string{spec.Worktree}
	}
	if r.StrictConfig {
		// NOTE: an empty slice would mean "SDK defaults", not "nothing" — the
		// bridge maps length 0 to undefined. Naming the narrowest layer is the
		// only way to actually suppress project/user config.
		local.SettingSources = cursor.SettingSourcesMinimal
	}
	// Native Cursor skills are stubbed in the SDK minify patch (getAgentSkills →
	// []), so settingSources can still load AGENTS.md / .cursor/rules.

	servers := map[string]cursor.MCPServer{}
	if spec.MCPURL != "" {
		servers["recur"] = cursor.NewHTTPMCPServer(spec.MCPURL)
	}
	if spec.Playwright {
		servers["playwright"] = cursor.NewStdioMCPServer("npx", "-y", "@playwright/mcp@latest")
	}
	for _, u := range spec.UserMCP {
		name := strings.TrimSpace(u.Name)
		if name == "" {
			continue
		}
		servers[name] = cursorUserMCP(u)
	}

	opts := cursor.AgentOptions{
		APIKey: r.APIKey,
		Name:   spec.AgentID,
		Local:  local,
	}
	if spec.Model != "" {
		opts.Model = &cursor.ModelSelection{
			ID:     spec.Model,
			Params: cursorParamsFromFlags(spec.Flags),
		}
	}
	if len(servers) > 0 {
		opts.MCPServers = servers
	}
	if isReadOnlyRole(spec) {
		// The closest thing Cursor offers to a per-role tool allowlist.
		opts.Mode = "plan"
	}
	return opts
}

// prompt composes the turn's user message. Tests and callers that do not know
// whether ResumeAgent succeeded treat ResumeSession as "will resume".
func (r *CursorRunner) prompt(spec AgentSpec) string {
	resumed := spec.ResumeSession != "" && !spec.RefreshStandingInstructions
	return r.wirePrompt(spec, standingInUserPrompt(spec, resumed))
}

// wirePrompt composes the Cursor user message. Because Cursor accepts no custom
// system prompt, the role prompt has to ride along in-band on a fresh (or
// remounted) session; it is fenced so the model treats it as standing
// instructions. A successful ResumeAgent already has those notes in history, so
// includeStanding is false on ordinary follow-ups.
//
// When images are attached, markers stay in the body (position) and a
// system_instruction maps each ⟦attachment:id⟧ to its ordinal in
// SDKUserMessage.images — Cursor has no per-image id on the wire.
func (r *CursorRunner) wirePrompt(spec AgentSpec, includeStanding bool) string {
	body := wrapAttachedImagesForCursor(spec.Prompt, spec.Images)
	body = joinAppendPrompt(body, spec.TurnAppendPrompt)
	if !includeStanding {
		return body
	}
	role := strings.TrimSpace(spec.SystemPrompt)
	for _, add := range []string{strings.TrimSpace(spec.AppendSystemPrompt), cursorSubagentOverlay(spec)} {
		if add == "" {
			continue
		}
		if role != "" {
			role += "\n\n"
		}
		role += add
	}
	if role == "" {
		return body
	}
	var b strings.Builder
	b.WriteString("<role-instructions>\n")
	b.WriteString(role)
	b.WriteString("\n</role-instructions>\n\n")
	b.WriteString(body)
	return b.String()
}

// cursorSubagentOverlay is the Cursor-only standing instruction that blocks
// native subagents (Cursor has no per-run tool allowlist). Parent Agent chats
// are pointed at create_workspace; nested workspaces are told to execute here.
const cursorSubagentPrompt = "Cursor's built-in subagents are disabled by policy in Recur. Never spawn a Cursor " +
	"subagent or background agent. For anything beyond a very simple task, call mcp__recur__create_workspace: " +
	"that is the Recur subagent; you are re-engaged with its report when it finishes. Redirect a running child, or resume a finished one, with mcp__recur__message_child."

const cursorChildSubagentPrompt = "Cursor's built-in subagents are disabled by policy in Recur. Never spawn a Cursor " +
	"subagent or background agent. Execute the assigned task here."

func cursorSubagentOverlay(spec AgentSpec) string {
	if spec.ChatChild {
		return cursorChildSubagentPrompt
	}
	for _, t := range spec.AllowedTools {
		if t == mcpCreateWorkspace {
			return cursorSubagentPrompt
		}
	}
	return ""
}

// cursorImages maps AgentSpec images onto Cursor SDKImage payloads, ordered to
// match attachment markers in the prompt text.
func cursorImages(spec AgentSpec) []cursor.Image {
	ordered := imagesInMarkerOrder(spec.Prompt, spec.Images)
	if len(ordered) == 0 {
		return nil
	}
	out := make([]cursor.Image, len(ordered))
	for i, img := range ordered {
		out[i] = cursor.Image{Data: img.Data, MimeType: img.Mime}
	}
	return out
}

// cursorParamsFromFlags maps Settings flags onto sdk.v1 ModelSelection.params.
// Empty / nil flags yield nil (SDK then uses each parameter's first value).
// Keys are sorted so wire payloads are stable across runs.
func cursorParamsFromFlags(flags map[string]string) []cursor.ModelParameterValue {
	if len(flags) == 0 {
		return nil
	}
	ids := make([]string, 0, len(flags))
	for id := range flags {
		if id != "" {
			ids = append(ids, id)
		}
	}
	sort.Strings(ids)
	out := make([]cursor.ModelParameterValue, 0, len(ids))
	for _, id := range ids {
		out = append(out, cursor.ModelParameterValue{ID: id, Value: flags[id]})
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// isReadOnlyRole reports whether a role's allow-list contains no file-edit
// tools, in which case Cursor's plan mode is a reasonable stand-in for tool
// scoping. Bare Bash (lint/test) alone does NOT count as write-capable — the
// Reviewer allowlists Bash without Edit/Write and must stay in plan mode.
func isReadOnlyRole(spec AgentSpec) bool {
	if len(spec.AllowedTools) == 0 {
		return false
	}
	for _, t := range spec.AllowedTools {
		switch t {
		case "Edit", "Write", "NotebookEdit":
			return false
		}
	}
	return true
}

// mcpStatusFor reports the MCP servers wired for this turn. The bridge does not
// surface per-server connection status the way Claude Code's init line does, so
// these are reported as configured rather than verified; a genuinely broken MCP
// shows up as a failing tool call instead.
func mcpStatusFor(spec AgentSpec) []MCPServerStatus {
	var out []MCPServerStatus
	if spec.MCPURL != "" {
		out = append(out, MCPServerStatus{Name: "recur", Status: "connected"})
	}
	if spec.Playwright {
		out = append(out, MCPServerStatus{Name: "playwright", Status: "connected"})
	}
	for _, u := range spec.UserMCP {
		if name := strings.TrimSpace(u.Name); name != "" {
			out = append(out, MCPServerStatus{Name: name, Status: "connected"})
		}
	}
	return out
}

func cursorUserMCP(u MCPAttach) cursor.MCPServer {
	if u.URL != "" {
		s := cursor.NewHTTPMCPServer(u.URL)
		if len(u.Headers) > 0 && s.HTTP != nil {
			s.HTTP.Headers = u.Headers
		}
		return s
	}
	s := cursor.NewStdioMCPServer(u.Command, u.Args...)
	if len(u.Env) > 0 && s.Stdio != nil {
		s.Stdio.Env = u.Env
	}
	return s
}

// cursorTurn accumulates one turn's events into a Result.
type cursorTurn struct {
	spec    AgentSpec
	emit    func(ParsedEvent)
	agentID string

	haveResult bool
	res        cursor.RunResult
	// statusError is the reason from the last status:ERROR frame. The bridge's
	// proto RunResult carries no error, so the terminal frame arrives as a bare
	// ERROR and this is what fills Result.Stderr.
	statusError     string
	lastText        string
	priorDurationMS int
	usage           *cursor.TokenUsage
	usagePerCall    bool // last frame was a single-call footprint (turn-ended)
	// usageCumulative is the last run-cumulative usage frame. Distinct from
	// usage, which turn-ended may overwrite with a per-call footprint.
	usageCumulative *cursor.TokenUsage

	// compactBaseline is the cumulative usage snapshot at summary-started.
	// Later usage frames still include pre-compact billing; the meter subtracts
	// this so fill is the post-summary window, not the discarded context.
	compactBaseline *cursor.TokenUsage
	// compactPreFill is the scaled pre-compact fill shown on the compact card.
	compactPreFill int64
	compactSummary string
	compactEmitted bool

	// toolWaves counts tool-call batches separated by model output (thinking /
	// reply). Cursor's turn-ended usage sums every model call in the loop;
	// waves+1 ≈ model calls so the context meter can scale cumulative billing
	// back to a last-call fill. Waves close on reasoning/reply boundaries, not
	// when openCalls drains — sibling tools often stream as running→completed
	// pairs, which would over-count waves and under-report fill.
	// Reset at summary so post-compact scaling only counts later model calls.
	toolWaves int
	waveOpen  bool

	// thinking accumulates the current reasoning block until Cursor terminates it.
	thinking strings.Builder

	// text accumulates the current run of assistant chunks so the transcript
	// stores one coherent block instead of one per frame.
	text strings.Builder

	// steppedCalls remembers which call ids already produced a KindStep, so a
	// tool whose progress is re-reported yields ONE tool_use block.
	steppedCalls map[string]bool

	// createPlanArgs keeps the richest args seen per createPlan call id across
	// streaming running frames, so the terminal emit can use the fullest body
	// even if a later frame is somehow shorter.
	createPlanArgs map[string]json.RawMessage

	// openCalls tracks calls that emitted a step but never a terminal frame, in
	// call order. Cursor simply STOPS sending frames for a tool call that fails —
	// a read of a missing file produces `running` and nothing else, and the run
	// still finishes successfully — so without closing these the transcript keeps
	// a call with no result and its card falls back to dumping the raw input.
	openCalls []string
	openTool  map[string]string // call id -> provider tool name, for the message

	// runIDs are every Cursor run id seen on this turn (parent + Task sub-agents).
	// Guarded by mu so the cancel watcher can read them while onEvent writes.
	mu             sync.Mutex
	emitMu         sync.Mutex // onEvent vs live-steer flush+KindUser
	runIDs         map[string]struct{}
	parentRunID    string
	gotRunID       chan string // buffered; receives the first run id (cancel wait)
	suppressResult bool        // drop CANCELLED KindResult during a D49 steer
	// deferResult holds KindResult until finish() so a live KindUser from an
	// in-flight SteerRun is published before session.turn_end.
	deferResult bool
	resultHeld  bool

	// seen is a TEST hook: when set, it points at the slice the emit closure
	// appends to, so a test can inspect events emitted after it stopped feeding.
	seen *[]ParsedEvent
}

// noteRunID records a provider run id for CancelRun. First id also wakes waiters.
func (t *cursorTurn) noteRunID(id string) {
	if t == nil || id == "" {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.runIDs == nil {
		t.runIDs = map[string]struct{}{}
	}
	becameParent := t.parentRunID == ""
	if becameParent {
		t.parentRunID = id
	}
	t.runIDs[id] = struct{}{}
	if becameParent && t.gotRunID != nil {
		select {
		case t.gotRunID <- id:
		default:
		}
	}
}

func (t *cursorTurn) allRunIDs() []string {
	if t == nil {
		return nil
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	if len(t.runIDs) == 0 {
		return nil
	}
	out := make([]string, 0, len(t.runIDs))
	for id := range t.runIDs {
		out = append(out, id)
	}
	sort.Strings(out)
	return out
}

func (t *cursorTurn) parentRun() string {
	if t == nil {
		return ""
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.parentRunID
}

// clearSteerTarget drops the current SteerRun id so the next Send's first
// noteRunID becomes the live target. runIDs stay so CancelRun still sees
// leftover recovery ids.
func (t *cursorTurn) clearSteerTarget() {
	if t == nil {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	t.parentRunID = ""
	if t.gotRunID != nil {
		select {
		case <-t.gotRunID:
		default:
		}
	}
}

// waitRunID blocks until a run id is known or d elapses.
func (t *cursorTurn) waitRunID(d time.Duration) string {
	return t.waitRunIDCtx(context.Background(), d)
}

func (t *cursorTurn) waitRunIDCtx(ctx context.Context, d time.Duration) string {
	if t == nil {
		return ""
	}
	if id := t.parentRun(); id != "" {
		return id
	}
	// Snapshot under mu: prepareSteerFollowUp drains this channel under the
	// same lock. Reading the field unguarded raced when an earlier revision
	// replaced the channel (TestCursorWaitRunIDCtxAndPrepareSteerFollowUpNoRace).
	t.mu.Lock()
	ch := t.gotRunID
	t.mu.Unlock()
	if ch == nil {
		return ""
	}
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case id := <-ch:
		return id
	case <-ctx.Done():
		return ""
	case <-timer.C:
		return t.parentRun()
	}
}

func (t *cursorTurn) setSuppressResult(v bool) {
	if t == nil {
		return
	}
	t.mu.Lock()
	t.suppressResult = v
	t.mu.Unlock()
}

func (t *cursorTurn) suppressingResult() bool {
	if t == nil {
		return false
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.suppressResult
}

func (t *cursorTurn) prepareSteerFollowUp() {
	if t == nil {
		return
	}
	t.finishSteerCancel()
	// interruptSend can return Send before the CANCELLED frame is read, so
	// bank mid-stream duration here. Idempotent if onEvent already banked.
	t.bankSuppressedDuration()
	t.haveResult = false
	t.res = cursor.RunResult{}
	// Do not clear suppressResult. A steer can land between steers.take() and
	// here; clearing would let that steer's CancelRun emit error_cancelled and
	// drop the follow-up. FINISHED/ERROR still emit while the flag is set.
	// The channel is write-once at construction: drain a leftover first-id so
	// the follow-up's noteRunID can wake waiters without replacing the field
	// (that raced with waitRunIDCtx).
	t.toolWaves = 0
	t.waveOpen = false
	t.compactBaseline = nil
	t.compactPreFill = 0
	t.compactSummary = ""
	t.steppedCalls = nil
	t.createPlanArgs = nil
	t.openCalls = nil
	t.openTool = nil
	t.usage = nil
	t.usageCumulative = nil
	t.usagePerCall = false
	t.mu.Lock()
	t.runIDs = nil
	t.parentRunID = ""
	if t.gotRunID != nil {
		select {
		case <-t.gotRunID:
		default:
		}
	}
	t.mu.Unlock()
}

func (t *cursorTurn) bankSuppressedDuration() {
	if t == nil {
		return
	}
	// Bank wall time across a cancelled steer; drop usage so the follow-up
	// result does not reuse this run's token snapshot.
	t.priorDurationMS += t.res.DurationMS.Int()
	t.usage = nil
	t.usageCumulative = nil
	t.usagePerCall = false
	t.res.Usage = nil
	t.res.DurationMS = 0
}

// emitted returns every event fired so far (test hook; nil outside tests).
func (t *cursorTurn) emitted() []ParsedEvent {
	if t.seen == nil {
		return nil
	}
	return *t.seen
}

// emitStepOnce emits the tool call's KindStep the first time it is seen. Cursor
// re-reports progress for some tools (edit sends `running` twice), and every
// KindStep mints a new tool_use block in the transcript.
//
// createPlan is special: Cursor streams the markdown across running frames
// (often {"plan":""}, then a partial body, then the full body on completed).
// KindStep freezes args on first emit and there is no update path, so emitting
// on the first non-empty running frame permanently captures a stub. Defer every
// createPlan running frame and emit only on the terminal status, when args are
// final — that is what the card and runPlanner's harvest both read.
func (t *cursorTurn) emitStepOnce(m *cursor.SDKMessage) {
	if t.steppedCalls == nil {
		t.steppedCalls = map[string]bool{}
	}
	args := m.Args
	if m.Name == "createPlan" {
		if t.createPlanArgs == nil {
			t.createPlanArgs = map[string]json.RawMessage{}
		}
		key := m.CallID
		if len(createPlanBody(m.Args)) >= len(createPlanBody(t.createPlanArgs[key])) {
			t.createPlanArgs[key] = m.Args
		}
		// Wait for the terminal frame so we do not freeze a partial body.
		if m.Status == cursor.ToolRunning {
			return
		}
		if richest := t.createPlanArgs[key]; len(richest) > 0 {
			args = richest
		}
	}
	// A blank call id cannot be correlated with its result, so it is never
	// deduped — better a duplicate row than a swallowed call.
	if m.CallID != "" {
		if t.steppedCalls[m.CallID] {
			return
		}
		t.steppedCalls[m.CallID] = true
		// First tool after a model boundary starts a wave; further tools share it
		// until thinking/reply closes the wave (see endToolWave).
		if !t.waveOpen {
			t.toolWaves++
			t.waveOpen = true
		}
		t.openCalls = append(t.openCalls, m.CallID)
		if t.openTool == nil {
			t.openTool = map[string]string{}
		}
		t.openTool[m.CallID] = m.Name
	}
	t.emitLocked(ParsedEvent{
		Kind:         KindStep,
		Tool:         m.Name,
		ToolInputRaw: args,
		// The feed's one-line summary is a display concern, so it is derived
		// from the COMMON shape, not Cursor's field names.
		ToolInput: summarizeDisplayInput(domain.ProviderCursor, m.Name, args),
		ToolUseID: m.CallID,
	})
}

// finish flushes anything still buffered at the end of a turn.
func (t *cursorTurn) finish() {
	if t == nil {
		return
	}
	t.emitMu.Lock()
	defer t.emitMu.Unlock()
	t.flushThinking()
	t.closeOpenCalls()
	t.flushText()
	if t.resultHeld {
		t.resultHeld = false
		t.emitResultLocked()
	}
}

func (t *cursorTurn) emitResultLocked() {
	// Mirror Claude's stream-json path: emit KindResult so the session
	// mirror can stamp duration_ms and the live UI can show the turn footer
	// via session.turn_end. Returning the Result from Run alone is not enough
	// — emit consumers never see it.
	res := t.result()
	t.emitLocked(ParsedEvent{Kind: KindResult, Text: res.Text, IsError: res.IsError, Result: &res})
}

// finishSteerCancel flushes the cancelled run without labeling in-flight tools
// as unexplained failures.
func (t *cursorTurn) finishSteerCancel() {
	if t == nil {
		return
	}
	t.emitMu.Lock()
	defer t.emitMu.Unlock()
	t.flushThinking()
	t.closeOpenCallsResult(steerCancelledCallText, false)
	t.flushText()
}

// resolveCall marks a call as having reported a terminal frame.
func (t *cursorTurn) resolveCall(callID string) {
	if callID == "" {
		return
	}
	for i, id := range t.openCalls {
		if id == callID {
			t.openCalls = append(t.openCalls[:i], t.openCalls[i+1:]...)
			return
		}
	}
}

// endToolWave marks the current tool batch finished. Called at model output
// boundaries (thinking / reply / synthetic close), not when openCalls drains.
func (t *cursorTurn) endToolWave() {
	t.waveOpen = false
}

// closeOpenCalls gives every call that never reported a terminal frame a
// synthetic error result.
//
// Cursor stops sending frames for a tool call that fails — a read of a missing
// file emits `running` and nothing more, and the failure is mentioned only in
// the model's own prose, never machine-readably. Leaving the call unresolved
// makes the transcript store a tool_use with no tool_result, which renders as a
// bare card dumping the input JSON and looks identical to a tool that returned
// nothing.
//
// WHEN this runs matters as much as that it runs. It is called at the next
// reasoning or reply frame, not just at the end of the run: the model cannot
// think or speak again until its outstanding tool results are back, so by then
// an unreported call is definitively finished. Waiting for the run to end left
// a failed read showing "Reading go.mod…" for the rest of the session, with
// later thoughts and tool calls rendered above it — while the very next thought
// was already "go.mod does not exist."
//
// A late real result is not a problem: it overwrites the synthetic one by call
// id on both the live path and the transcript.
//
// The message says only what is actually known: no result arrived.
func (t *cursorTurn) closeOpenCalls() {
	t.closeOpenCallsResult(unreportedResultText, true)
}

func (t *cursorTurn) closeOpenCallsResult(text string, isError bool) {
	// Always end the tool wave: callers are at a reasoning/reply boundary, even
	// when every call already reported a real result (openCalls empty).
	t.endToolWave()
	if len(t.openCalls) == 0 {
		return
	}
	open := t.openCalls
	t.openCalls = nil
	for _, id := range open {
		t.emitLocked(ParsedEvent{
			Kind:      KindToolResult,
			Tool:      t.openTool[id],
			ToolUseID: id,
			Text:      text,
			IsError:   isError,
		})
	}
}

// flushText emits the accumulated assistant chunks as ONE text block, which is
// what the transcript stores. No-op when nothing is pending.
func (t *cursorTurn) flushText() {
	if t.text.Len() == 0 {
		return
	}
	t.endToolWave()
	text := t.text.String()
	t.text.Reset()
	// The final Result carries the whole reply, so accumulate across runs rather
	// than overwriting — otherwise Result.Text is only the last fragment.
	t.lastText += text
	t.emitLocked(ParsedEvent{Kind: KindText, Text: text})
}

// flushThinking emits the accumulated reasoning as one thinking block. No-op when
// nothing is pending, so a duplicate terminator cannot produce an empty block.
func (t *cursorTurn) flushThinking() {
	if t.thinking.Len() == 0 {
		return
	}
	t.endToolWave()
	text := t.thinking.String()
	t.thinking.Reset()
	t.emitLocked(ParsedEvent{Kind: KindThinking, Text: text})
}

func (t *cursorTurn) onEvent(ev cursor.RunStreamEvent) error {
	t.emitMu.Lock()
	defer t.emitMu.Unlock()
	// Collect run ids from every frame so CancelRun can stop the provider-side
	// agent (and any Task sub-agent runs that report their own id).
	if ev.Result != nil {
		t.noteRunID(ev.Result.RunID)
		if ev.Result.Result != nil {
			t.noteRunID(ev.Result.Result.RunID)
		}
	}
	if ev.SDKMessage != nil {
		t.noteRunID(ev.SDKMessage.RunID)
	}

	if len(ev.InteractionUpdate) > 0 {
		t.onInteraction(ev.InteractionUpdate)
		if ev.SDKMessage == nil && (ev.Result == nil || ev.Result.Result == nil) {
			return nil
		}
	}

	if ev.Result != nil && ev.Result.Result != nil {
		t.flushThinking()
		t.res = *ev.Result.Result
		if t.res.IsError() && t.res.Error == nil && t.statusError != "" {
			t.res.Error = &cursor.RunError{Message: t.statusError}
		}
		t.haveResult = true
		// A steer CancelRun ends the stream as CANCELLED. Emitting KindResult
		// would publish session.turn_end and fail the specialist turn; skip it
		// and follow up with Send. FINISHED still wins (late Inject). ERROR is
		// not suppressed — a racing overload must surface, not start another run.
		if t.suppressingResult() && t.res.Status == cursor.RunCancelled {
			t.closeOpenCallsResult(steerCancelledCallText, false)
			t.flushText()
			t.bankSuppressedDuration()
			t.haveResult = false
			return nil
		}
		t.closeOpenCalls()
		t.flushText()
		if t.deferResult {
			t.resultHeld = true
			return nil
		}
		t.emitResultLocked()
		return nil
	}
	m := ev.SDKMessage
	if m == nil {
		return nil
	}

	switch m.Type {
	case cursor.MsgStatus:
		if m.Status == cursor.StatusError && m.Message != "" {
			t.statusError = expandCursorRunError(m.Message)
		}

	case cursor.MsgAssistant:
		// Cursor streams a reply as a RUN of assistant frames, each carrying the
		// next CHUNK — not a complete message the way Claude Code does. Storing a
		// text block per frame split the reply mid-sentence and mid-markdown: a
		// fenced code block opened in one block and closed in the next, so the
		// reloaded transcript rendered ```bash\ngo and run .\n``` as two broken
		// fragments. Accumulate, and write ONE block per contiguous run of text.
		chunk := m.Content.TextContent()
		if chunk == "" {
			return nil
		}
		// The model cannot speak again until its outstanding tool results are
		// back, so any call still open is finished (see closeOpenCalls).
		t.closeOpenCalls()
		// Reasoning always precedes the reply; close it so ordering is preserved.
		t.flushThinking()
		t.text.WriteString(chunk)
		// Each chunk still streams live — that is what makes the reply appear
		// progressively. The mirror suppresses the duplicate publish when the
		// accumulated KindText lands (its gotDelta guard).
		t.emitLocked(ParsedEvent{Kind: KindTextDelta, Text: chunk})

	case cursor.MsgToolCall:
		t.flushThinking()
		t.flushText()
		// RAW provider data flows through: Cursor's own tool name, its verbatim
		// args, and its verbatim result envelope. The session store is the record
		// of what Cursor actually did; rendering these in the common
		// cross-provider vocabulary happens at the display boundary (toolview.go).
		//
		// Cursor's tool_call lifecycle is NOT one running + one terminal frame:
		//
		//   read  → running, completed
		//   edit  → running, running, completed      (progress is re-reported)
		//
		// so the frames are folded into exactly one KindStep and one
		// KindToolResult per call id. Emitting per frame produced a second,
		// resultless tool_use block for every edit, and since the id map kept only
		// the last one, the result attached to the second and the first rendered
		// as a bare input card.
		if m.Status == cursor.ToolRunning {
			t.emitStepOnce(m)
			return nil
		}
		// ANY non-running status is terminal. Matching only "completed"/"failed"
		// meant a status Cursor never documented (and that this client has not
		// seen) silently dropped the result, leaving the call with no output at
		// all — indistinguishable from a tool that returned nothing.
		t.emitStepOnce(m) // a result without a preceding step still gets its call
		t.resolveCall(m.CallID)
		t.emitLocked(ParsedEvent{
			Kind:      KindToolResult,
			Tool:      m.Name,
			ToolUseID: m.CallID,
			// Verbatim {"status":…,"value":{…}}; unpacked for display only.
			Text: string(m.Result),
			// Any status other than "completed" is a failure — the SDK documents
			// "error", but the runner does not match on it so a status added
			// later still counts.
			IsError:   m.Status != cursor.ToolCompleted || m.ToolResultIsError(),
			Truncated: m.Truncated.Result,
		})

	case cursor.MsgUsage:
		// Usage arrives mid-stream as well as on the result; keep the latest so
		// a run that dies before its result still reports what it spent, and
		// push KindUsage so the composer context meter can update live.
		if m.Usage != nil {
			t.emitCursorUsage(m.Usage, false)
		}

	case cursor.MsgThinking:
		// Cursor streams reasoning as a run of text frames terminated by an empty
		// one carrying the duration. Accumulate, then emit ONE KindThinking so the
		// transcript gets a thinking block like Claude's — otherwise reasoning is
		// lost entirely and the Activity panel shows a gap before each tool call.
		if m.Text != "" {
			// Same boundary as an assistant reply: the model cannot reason again
			// until its outstanding tool results are back.
			t.closeOpenCalls()
			// Reasoning interrupts a reply, so close any open text run first.
			t.flushText()
			t.thinking.WriteString(m.Text)
			// KindThinkingDelta, not KindTextDelta: reasoning streams to the
			// thinking pane. Emitting it as a text delta would splice the model's
			// private reasoning into the assistant's visible reply.
			//
			// Gated like Claude's --include-partial-messages. runAgent sets
			// IncludePartials for cockpit/chat turns; SkipThinking one-shots leave
			// it off. The stored KindThinking block below is written either way.
			if t.spec.IncludePartials {
				t.emitLocked(ParsedEvent{Kind: KindThinkingDelta, Text: m.Text})
			}
			return nil
		}
		t.flushThinking()
	}
	return nil
}

func (t *cursorTurn) fire(ev ParsedEvent) {
	if t == nil {
		return
	}
	t.emitMu.Lock()
	defer t.emitMu.Unlock()
	t.emitLocked(ev)
}

func (t *cursorTurn) emitLocked(ev ParsedEvent) {
	if t.emit == nil {
		return
	}
	// Tagged here rather than at each call site, so no event can escape without
	// the provider a display layer needs to interpret its raw fields.
	ev.Provider = domain.ProviderCursor
	t.emit(ev)
}

// result renders the accumulated turn as an engine Result.
func (t *cursorTurn) result() Result {
	// Meter prefers the latest mid-stream usage frame. Cursor SDK / result
	// usage is often cumulative across every model call in the tool loop —
	// scale it back to a last-call fill (see scaleCursorMeterUsage). A
	// turn-ended frame that already fits the window is the footprint as-is.
	meterUsage := t.usage
	perCall := t.usagePerCall
	if meterUsage == nil {
		meterUsage = t.res.Usage
		perCall = false
	}

	text := t.res.Result
	if text == "" {
		text = t.lastText
	}

	out := Result{
		SessionID:  t.agentID, // persisted so the next turn can ResumeAgent
		Subtype:    cursorSubtype(&t.res, t.haveResult),
		IsError:    !t.haveResult || t.res.IsError(),
		DurationMS: t.res.DurationMS.Int() + t.priorDurationMS,
		Text:       text,
	}
	if meterUsage != nil {
		meter := t.meterResult(meterUsage, perCall)
		out.InputTokens = meter.InputTokens
		out.OutputTokens = meter.OutputTokens
		out.CacheCreationTokens = meter.CacheCreationTokens
		out.CacheReadTokens = meter.CacheReadTokens
	}
	if t.res.Error != nil {
		out.Stderr = t.res.Error.Message
	} else if out.IsError && t.statusError != "" {
		out.Stderr = t.statusError
	}
	return out
}

// modelRounds is the estimated number of model calls in this Cursor run:
// one per tool wave that requested tools, plus one for the final reply.
func (t *cursorTurn) modelRounds() int {
	if t == nil {
		return 1
	}
	n := t.toolWaves + 1
	if n < 1 {
		return 1
	}
	return n
}

// emitCursorUsage publishes a live meter frame. perCall is true when the numbers
// are a single conversation-turn footprint (turn-ended), not run-cumulative
// billing. Cumulative frames subtract the pre-summary snapshot when compact ran.
func (t *cursorTurn) emitCursorUsage(raw *cursor.TokenUsage, perCall bool) {
	if t == nil || raw == nil {
		return
	}
	t.usage = raw
	t.usagePerCall = perCall
	if !perCall {
		snap := *raw
		t.usageCumulative = &snap
	}
	partial := t.meterResult(raw, perCall)
	if contextUsedTokens(&partial) > 0 {
		t.emitLocked(ParsedEvent{Kind: KindUsage, Result: &partial})
	}
}

func (t *cursorTurn) meterResult(raw *cursor.TokenUsage, perCall bool) Result {
	if raw == nil {
		return Result{}
	}
	meter := raw
	if !perCall && t.compactBaseline != nil {
		sub := cursor.SubUsage(*raw, *t.compactBaseline)
		meter = &sub
	}
	out := Result{}
	applyCursorUsage(&out, meter)
	t.fitCursorMeter(&out, perCall)
	return out
}

func (t *cursorTurn) fitCursorMeter(out *Result, perCall bool) {
	window := ResolveCursorContextWindow(t.spec.Model, t.spec.Flags)
	if perCall {
		used := contextUsedTokens(out)
		if window > 0 && used > window {
			scaleCursorMeterUsage(out, window, 1)
		}
		return
	}
	scaleCursorMeterUsage(out, window, t.modelRounds())
}

func (t *cursorTurn) onInteraction(raw json.RawMessage) {
	typ, body := cursor.ParseInteractionType(raw)
	switch typ {
	case cursor.DeltaSummaryStarted:
		t.beginCompact()
	case cursor.DeltaSummaryCompleted:
		t.finishCompact()
	case cursor.DeltaSummary:
		var p struct {
			Summary string `json:"summary"`
		}
		if json.Unmarshal(body, &p) == nil && strings.TrimSpace(p.Summary) != "" {
			t.compactSummary = p.Summary
		}
	case cursor.DeltaTurnEnded:
		var p struct {
			Usage *cursor.TokenUsage `json:"usage"`
		}
		if json.Unmarshal(body, &p) != nil || p.Usage == nil {
			return
		}
		window := ResolveCursorContextWindow(t.spec.Model, t.spec.Flags)
		used := int64(p.Usage.PromptTokens())
		// A turn-ended usage that still fits the window is the per-call footprint
		// the meter wants. Larger totals are run-cumulative billing.
		t.emitCursorUsage(p.Usage, window > 0 && used > 0 && used <= window)
	}
}

func (t *cursorTurn) beginCompact() {
	// Flush first so the compact marker persists after the reply that streamed
	// before auto-compaction, not before it.
	t.flushThinking()
	t.flushText()
	t.snapshotCompactBaseline()
	t.compactEmitted = false
	t.compactSummary = ""
	// Scale with current waves before they reset for the post-summary loop.
	t.compactPreFill = t.compactCardTokens(t.compactBaseline)
	t.toolWaves = 0
	t.waveOpen = false
	t.emitLocked(ParsedEvent{
		Kind:           KindCompact,
		CompactStatus:  domain.CompactStarted,
		CompactTrigger: "auto",
		Tokens:         t.compactPreFill,
	})
}

func (t *cursorTurn) finishCompact() {
	if t.compactEmitted {
		return
	}
	if t.compactBaseline == nil {
		t.snapshotCompactBaseline()
		t.compactPreFill = t.compactCardTokens(t.compactBaseline)
	}
	t.compactEmitted = true
	t.emitLocked(ParsedEvent{
		Kind:           KindCompact,
		CompactStatus:  domain.CompactDone,
		CompactTrigger: "auto",
		Text:           t.compactSummary,
		Tokens:         t.compactPreFill,
	})
	// Pre-compact turn-ended footprints describe discarded context. Re-meter
	// from cumulative−baseline so the composer ring (and the turn stamp) do
	// not keep the old fill. At the compact boundary that delta is 0 until
	// the next usage frame; KindUsage is omitted when used==0 and the live
	// UI resets on session.compact done.
	if t.compactBaseline == nil {
		return
	}
	t.usagePerCall = false
	if t.usageCumulative != nil {
		t.emitCursorUsage(t.usageCumulative, false)
	}
}

// snapshotCompactBaseline records the last run-cumulative usage, never a
// turn-ended per-call footprint (those would under-subtract post-compact).
func (t *cursorTurn) snapshotCompactBaseline() {
	src := t.usageCumulative
	if src == nil && t.usage != nil && !t.usagePerCall {
		src = t.usage
	}
	if src == nil {
		return
	}
	snap := *src
	t.compactBaseline = &snap
}

func (t *cursorTurn) compactCardTokens(u *cursor.TokenUsage) int64 {
	if u == nil {
		return 0
	}
	out := Result{}
	applyCursorUsage(&out, u)
	scaleCursorMeterUsage(&out, ResolveCursorContextWindow(t.spec.Model, t.spec.Flags), t.modelRounds())
	return contextUsedTokens(&out)
}

// applyCursorUsage copies TokenUsage onto a Result for the context meter.
// Cache writes count toward occupied context like fresh input.
// When only TotalTokens is present, it is stored as InputTokens as a last-resort
// approximation — that total includes output (and reasoning), so the prompt-side
// meter overstates fill until a detailed breakdown arrives.
func applyCursorUsage(out *Result, usage *cursor.TokenUsage) {
	if out == nil || usage == nil {
		return
	}
	out.InputTokens = int64(usage.InputTokens.Int())
	out.OutputTokens = int64(usage.OutputTokens.Int())
	out.CacheReadTokens = int64(usage.CacheReadTokens.Int())
	out.CacheCreationTokens = int64(usage.CacheWriteTokens.Int())
	if out.InputTokens+out.CacheReadTokens+out.CacheCreationTokens == 0 {
		out.InputTokens = int64(usage.TotalTokens.Int())
	}
}

// scaleCursorMeterUsage rewrites prompt-side token fields when Cursor's
// usage is cumulative billing across the tool loop. OutputTokens
// are left alone.
//
// A single-call total that still fits the window is already a footprint.
// Multiple model calls use an arithmetic-series last-call estimate
// (last ≈ used * 2 / (rounds+1)) and clamp to the window, whether or not
// the cumulative total itself fits. Equal-split under the window under-reports
// the last call (context grows across the loop) and jumped ~1.6x at the
// window boundary versus the over-window branch.
func scaleCursorMeterUsage(out *Result, window int64, rounds int) {
	if out == nil {
		return
	}
	used := contextUsedTokens(out)
	if used <= 0 {
		return
	}
	if window <= 0 {
		window = 200_000
	}
	if rounds < 1 {
		rounds = 1
	}
	target := used
	if rounds > 1 {
		// last = 2/(n+1) * sum for s+2s+…+ns
		target = used * 2 / int64(rounds+1)
	}
	if target > window {
		target = window
	}
	if target < 1 {
		target = 1
	}
	if target == used {
		return
	}
	// Scale prompt-side fields together so the popover split stays meaningful.
	num := target
	den := used
	out.InputTokens = out.InputTokens * num / den
	out.CacheReadTokens = out.CacheReadTokens * num / den
	out.CacheCreationTokens = out.CacheCreationTokens * num / den
	// Fix rounding drift so contextUsedTokens matches target.
	if got := contextUsedTokens(out); got != target && got > 0 {
		out.InputTokens += target - got
		if out.InputTokens < 0 {
			out.InputTokens = 0
		}
	}
}

// cursorRunErrorHints explain the Connect codes Cursor's backend returns with
// no detail at all — the SDK then reports literally "[resource_exhausted] Error",
// which reads like a Recur bug rather than the provider rate limit it is.
var cursorRunErrorHints = map[string]string{
	"resource_exhausted": "Cursor's model provider is rate-limited or at capacity. Wait a moment and retry.",
	"unavailable":        "Cursor could not reach the model provider. Usually temporary; retry.",
	"deadline_exceeded":  "Cursor's model provider timed out. Usually temporary; retry.",
	"internal":           "Cursor-side fault, not a defect in the work. Retry.",
}

// expandCursorRunError appends the hint for a bare "[code] …" status message.
func expandCursorRunError(msg string) string {
	if !strings.HasPrefix(msg, "[") {
		return msg
	}
	end := strings.IndexByte(msg, ']')
	if end < 0 {
		return msg
	}
	if hint, ok := cursorRunErrorHints[strings.ToLower(msg[1:end])]; ok {
		return msg + ". " + hint
	}
	return msg
}

// cursorSubtype maps a Cursor run status onto the subtype vocabulary the engine
// already branches on (success / error_during_execution / …).
func cursorSubtype(r *cursor.RunResult, haveResult bool) string {
	if !haveResult {
		return "error_during_execution"
	}
	switch r.Status {
	case cursor.RunFinished:
		if r.Error != nil {
			return "error_during_execution"
		}
		return "success"
	case cursor.RunCancelled:
		return "error_cancelled"
	case cursor.RunError_:
		return "error_during_execution"
	default:
		return "success"
	}
}
