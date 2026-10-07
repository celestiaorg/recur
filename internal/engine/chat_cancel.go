package engine

import (
	"context"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// chatTurn is one in-flight chat cancel handle. Stored as a pointer in chats so
// CompareAndDelete can match the exact turn (CancelFunc itself is not comparable).
//
// The optional armed deadline kills a wedged CLI turn. ParkChatTurn cancels the
// turn after ask_user parks a question or switch_mode remounts, so the tab goes
// fully idle (no agent process / MCP wait) without showing "Stopped by user".
type chatTurn struct {
	cancel context.CancelFunc

	mu       sync.Mutex
	armed    time.Duration
	timer    *time.Timer
	parked   bool
	stopped  bool // CancelChat (user Stop); park must not override this
	timedOut bool
	// ended is closed in unregisterChatTurn so a follow-up can wait for this
	// turn to drop status ownership before starting a replacement run.
	ended     chan struct{}
	endedOnce sync.Once
	// owned is true for a live Send that will unregister. CancelChat must not
	// close ended until that owner drops. Tests that register a handle without
	// an owning RPC call dropOwner so Stop can close ended itself.
	owned bool
	// inject is the live Cursor (or other Inject-honouring) follow-up channel.
	// Unbuffered: tryInject returns true only after the runner has received
	// the payload. injectOff is set when the turn starts unwinding so a late
	// SendWorkspaceMessage cannot send on a channel nobody will read.
	// injectDone is closed by stopChatInject so a blocking tryInject unblocks
	// instead of starting a replacement turn.
	inject     chan string
	injectAck  chan bool
	injectOff  bool
	injectDone chan struct{}
	injectOnce sync.Once
}

// chatTurnKey uniquely identifies an in-flight chat turn (one per workspace tab).
func chatTurnKey(workspaceID, code string) string {
	return workspaceID + "\x00" + code
}

// chatTabSlot FIFO-orders SendWorkspaceMessage on one tab and tracks Stop so a
// follow-up that lost inject cannot start a replacement run.
type chatTabSlot struct {
	mu      sync.Mutex
	cond    *sync.Cond
	next    uint64
	cur     uint64
	stopGen atomic.Uint64
}

func (e *Engine) chatTabSlot(workspaceID, code string) *chatTabSlot {
	key := chatTurnKey(workspaceID, code)
	if v, ok := e.chatTabs.Load(key); ok {
		return v.(*chatTabSlot)
	}
	s := &chatTabSlot{}
	s.cond = sync.NewCond(&s.mu)
	actual, _ := e.chatTabs.LoadOrStore(key, s)
	return actual.(*chatTabSlot)
}

func (s *chatTabSlot) begin() (seq, gen uint64) {
	s.mu.Lock()
	s.next++
	seq, gen = s.next, s.stopGen.Load()
	s.mu.Unlock()
	return seq, gen
}

func (s *chatTabSlot) claim(seq uint64) {
	s.mu.Lock()
	for s.cur+1 != seq {
		s.cond.Wait()
	}
}

func (s *chatTabSlot) finish() {
	s.cur++
	s.cond.Broadcast()
	s.mu.Unlock()
}

func (s *chatTabSlot) pause() { s.mu.Unlock() }

func (s *chatTabSlot) resume() { s.mu.Lock() }

func chatLifeKey(workspaceID, code string) string {
	code = strings.TrimSpace(code)
	if code != maestroAgentCode {
		code = normalizePairChatCode(code)
	}
	return chatTurnKey(workspaceID, code)
}

// continueID is the chat a live switch_mode remount gen belongs to.
type continueID struct {
	workspaceID string
	code        string
}

// lifeLock is a reference-counted per-chat mutex. refs is guarded by
// Engine.chatLifeMu (not this mutex) so retain and retire cannot split a key
// across two slots.
type lifeLock struct {
	mu   sync.Mutex
	refs int
}

// lockChatLife serializes reopen, send prologue, create/close, dock Remember,
// and closed-chat delete for one tab. ok is false when the workspace is being
// removed, hoisted, or already archived; unlock is still safe to defer. The
// returned unlock is idempotent so a send can release after the turn is
// registered without racing the function's defer.
func (e *Engine) lockChatLife(workspaceID, code string) (unlock func(), ok bool) {
	nop := func() {}
	if e == nil {
		return nop, false
	}
	key := chatLifeKey(workspaceID, code)
	e.chatLifeMu.Lock()
	if e.chatLifeReserved(workspaceID) {
		e.chatLifeMu.Unlock()
		return nop, false
	}
	if e.chatLife == nil {
		e.chatLife = map[string]*lifeLock{}
	}
	slot := e.chatLife[key]
	if slot == nil {
		slot = &lifeLock{}
		e.chatLife[key] = slot
	}
	slot.refs++
	e.chatLifeMu.Unlock()
	slot.mu.Lock()
	var once sync.Once
	return func() {
		once.Do(func() {
			slot.mu.Unlock()
			e.chatLifeMu.Lock()
			slot.refs--
			if slot.refs == 0 && e.chatLife[key] == slot {
				delete(e.chatLife, key)
			}
			e.chatLifeMu.Unlock()
			e.maybeArchiveLinkedPRChatHost(workspaceID)
		})
	}, true
}

func (e *Engine) allocContinueGen(workspaceID, code string) uint64 {
	if e == nil {
		return 0
	}
	workspaceID = strings.TrimSpace(workspaceID)
	e.continueGenMu.Lock()
	defer e.continueGenMu.Unlock()
	if e.liveContinues == nil {
		e.liveContinues = map[uint64]continueID{}
	}
	e.nextContinueGen++
	gen := e.nextContinueGen
	e.liveContinues[gen] = continueID{workspaceID: workspaceID, code: chatLifeKey(workspaceID, code)}
	return gen
}

func (e *Engine) continueGenLive(gen uint64) bool {
	if e == nil || gen == 0 {
		return false
	}
	e.continueGenMu.Lock()
	defer e.continueGenMu.Unlock()
	_, ok := e.liveContinues[gen]
	return ok
}

func (e *Engine) dropContinueGen(gen uint64) {
	if e == nil || gen == 0 {
		return
	}
	e.continueGenMu.Lock()
	id, ok := e.liveContinues[gen]
	delete(e.liveContinues, gen)
	e.continueGenMu.Unlock()
	if !ok {
		return
	}
	// sendWorkspaceTurn holds lockChatLife across this call. Hoist waits for
	// those refs, so retrying here deadlocks; unlock retries.
	if e.chatLifeBusy(id.workspaceID) {
		return
	}
	e.maybeArchiveLinkedPRChatHost(id.workspaceID)
}

func (e *Engine) invalidateContinueGens(workspaceID, code string) {
	workspaceID = strings.TrimSpace(workspaceID)
	if e == nil || workspaceID == "" {
		return
	}
	match := ""
	if code != "" {
		match = chatLifeKey(workspaceID, code)
	}
	e.continueGenMu.Lock()
	for gen, id := range e.liveContinues {
		if id.workspaceID != workspaceID {
			continue
		}
		if match != "" && id.code != match {
			continue
		}
		delete(e.liveContinues, gen)
	}
	e.continueGenMu.Unlock()
}

func (e *Engine) dropChatLifeForWorkspace(workspaceID string) {
	workspaceID = strings.TrimSpace(workspaceID)
	if e == nil || workspaceID == "" {
		return
	}
	prefix := workspaceID + "\x00"
	e.chatLifeMu.Lock()
	for k, slot := range e.chatLife {
		if strings.HasPrefix(k, prefix) && slot.refs == 0 {
			delete(e.chatLife, k)
		}
	}
	e.chatLifeMu.Unlock()
	e.invalidateContinueGens(workspaceID, "")
}

// chatLifeBusy is true while any tab in workspaceID holds lockChatLife.
func (e *Engine) chatLifeBusy(workspaceID string) bool {
	workspaceID = strings.TrimSpace(workspaceID)
	if e == nil || workspaceID == "" {
		return false
	}
	prefix := workspaceID + "\x00"
	e.chatLifeMu.Lock()
	defer e.chatLifeMu.Unlock()
	for k, slot := range e.chatLife {
		if strings.HasPrefix(k, prefix) && slot != nil && slot.refs > 0 {
			return true
		}
	}
	return false
}

// waitChatLifeIdle blocks until no tab in workspaceID holds lockChatLife, so
// teardown can invalidate remount gens without racing a prologue that already
// passed its liveness check.
func (e *Engine) waitChatLifeIdle(workspaceID string) {
	for {
		if !e.chatLifeBusy(workspaceID) {
			return
		}
		time.Sleep(time.Millisecond)
	}
}

// registerChatTurn publishes cancel so CancelChat can interrupt this turn.
// If a prior turn is somehow still registered for the same tab, it is cancelled
// first — a tab never runs two agents at once. Returns the handle to pass to
// unregisterChatTurn, or nil when the workspace is archived / being removed.
func (e *Engine) registerChatTurn(workspaceID, code string, cancel context.CancelFunc) *chatTurn {
	if e == nil || cancel == nil {
		return nil
	}
	if e.isShuttingDown() || e.workspaceRemoveBlocksChat(workspaceID) {
		return nil
	}
	turn := &chatTurn{cancel: cancel, ended: make(chan struct{}), owned: true}
	key := chatTurnKey(workspaceID, code)
	if prev, loaded := e.chats.Swap(key, turn); loaded {
		if p, ok := prev.(*chatTurn); ok && p != nil {
			p.stopChatInject()
			if p.cancel != nil {
				p.cancel()
			}
		}
	}
	if e.isShuttingDown() || e.workspaceRemoveBlocksChat(workspaceID) {
		e.chats.CompareAndDelete(key, turn)
		cancel()
		return nil
	}
	return turn
}

// registerChatTurnIfIdle registers a turn only when the tab is idle. Used by
// switch_mode remounts and app-restart AutoResume so they cannot displace a
// user send that landed during the send prologue. Returns nil when a turn is
// already live or the workspace is archived / being removed.
func (e *Engine) registerChatTurnIfIdle(workspaceID, code string, cancel context.CancelFunc) *chatTurn {
	if e == nil || cancel == nil {
		return nil
	}
	if e.isShuttingDown() || e.workspaceRemoveBlocksChat(workspaceID) {
		return nil
	}
	turn := &chatTurn{cancel: cancel, ended: make(chan struct{}), owned: true}
	key := chatTurnKey(workspaceID, code)
	if _, loaded := e.chats.LoadOrStore(key, turn); loaded {
		return nil
	}
	if e.isShuttingDown() || e.workspaceRemoveBlocksChat(workspaceID) {
		e.chats.CompareAndDelete(key, turn)
		cancel()
		return nil
	}
	return turn
}

// unregisterChatTurn drops the cancel handle when the turn ends. Only the
// matching turn is removed so a newer turn's registration is preserved.
func (e *Engine) unregisterChatTurn(workspaceID, code string, turn *chatTurn) {
	if e == nil || turn == nil {
		return
	}
	turn.stopChatInject()
	turn.disarm()
	turn.closeEnded()
	e.chats.CompareAndDelete(chatTurnKey(workspaceID, code), turn)
	e.childWaves.Delete(turn)
	// sendWorkspaceTurn holds lockChatLife across continuation setup. Hoist
	// waits for those refs, so retrying here deadlocks; unlock retries.
	if e.chatLifeBusy(workspaceID) {
		return
	}
	e.maybeArchiveLinkedPRChatHost(workspaceID)
}

// armChatInject opens the mid-turn follow-up channel for this turn. Unbuffered
// so tryInject is true only after the runner has received the payload.
func (t *chatTurn) armChatInject() chan string {
	if t == nil {
		return nil
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.injectOff {
		return nil
	}
	if t.inject == nil {
		t.inject = make(chan string)
		t.injectAck = make(chan bool, 8)
		t.injectDone = make(chan struct{})
	}
	return t.inject
}

func (t *chatTurn) stopChatInject() {
	if t == nil {
		return
	}
	t.mu.Lock()
	t.injectOff = true
	done := t.injectDone
	t.mu.Unlock()
	t.injectOnce.Do(func() {
		if done != nil {
			close(done)
		}
	})
}

func (t *chatTurn) closeEnded() {
	if t == nil || t.ended == nil {
		return
	}
	t.endedOnce.Do(func() { close(t.ended) })
}

func (t *chatTurn) tryInject(msg string) bool {
	msg = strings.TrimSpace(msg)
	if t == nil || msg == "" {
		return false
	}
	t.mu.Lock()
	ch, ack, off, done := t.inject, t.injectAck, t.injectOff, t.injectDone
	refuse := off || t.stopped || t.parked || t.timedOut
	t.mu.Unlock()
	if refuse || ch == nil {
		return false
	}
	if done == nil {
		select {
		case ch <- msg:
		default:
			return false
		}
	} else {
		select {
		case ch <- msg:
		case <-done:
			return false
		}
	}
	if ack == nil {
		return true
	}
	select {
	case ok := <-ack:
		return ok
	case <-done:
		select {
		case ok := <-ack:
			return ok
		default:
			return false
		}
	}
}

// tryInjectChat delivers a follow-up onto a live chat turn's Inject channel.
// False means there is no live Inject (idle or the turn is unwinding) — the
// caller should start a new Run.
func (e *Engine) tryInjectChat(workspaceID, code, msg string) bool {
	if e == nil {
		return false
	}
	return e.liveChatTurn(workspaceID, code).tryInject(msg)
}

// workspaceHasLiveChat reports an in-flight agent turn on any tab of this workspace.
func (e *Engine) workspaceHasLiveChat(id string) bool {
	if e == nil || id == "" {
		return false
	}
	prefix := id + "\x00"
	found := false
	e.chats.Range(func(k, _ any) bool {
		key, ok := k.(string)
		if !ok || !strings.HasPrefix(key, prefix) {
			return true
		}
		found = true
		return false
	})
	return found
}

// workspaceRemoveBlocksChat is true while Remove/Delete is tearing the workspace
// down or the store row is already archived/gone — a new chat must not start.
func (e *Engine) workspaceRemoveBlocksChat(id string) bool {
	if e == nil {
		return true
	}
	if id == "" {
		return false // app-scoped Ask Recur
	}
	if _, busy := e.deleting.Load(id); busy {
		return true
	}
	if e.Store == nil {
		return false
	}
	ws, ok := e.Store.Workspace(id)
	return !ok || ws.Archived
}

// chatLifeReserved is true while Remove/Delete holds the deleting tombstone,
// archivePRChatHosts is remapping this workspace, or the row is already
// archived. Missing ids are not reserved so tests and post-delete lookups can
// still serialize; new sends load the workspace first.
func (e *Engine) chatLifeReserved(id string) bool {
	if e == nil {
		return true
	}
	if id == "" {
		return false
	}
	if _, busy := e.deleting.Load(id); busy {
		return true
	}
	if _, busy := e.hoisting.Load(id); busy {
		return true
	}
	if e.Store == nil {
		return false
	}
	ws, ok := e.Store.Workspace(id)
	return ok && ws.Archived
}

// reserveDeleting marks id as tearing down. Acquire is atomic with
// holdPRChatHoist (both take hoistMu and check the other tombstone). ok is
// false when a hoist already covers this id, or when another Remove/Delete
// owns the deleting reservation — waiting for a hoist then tearing down
// would cascade chats that just landed here.
func (e *Engine) reserveDeleting(id string) bool {
	id = strings.TrimSpace(id)
	if e == nil || id == "" {
		return false
	}
	e.hoistMu.Lock()
	defer e.hoistMu.Unlock()
	if _, busy := e.hoisting.Load(id); busy {
		return false
	}
	_, loaded := e.deleting.LoadOrStore(id, struct{}{})
	return !loaded
}

// workspaceTeardownCommitted is true after Remove/Delete has archived or
// dropped the store row. The transient deleting tombstone is not enough —
// teardown may still refuse and leave the workspace in place.
func (e *Engine) workspaceTeardownCommitted(id string) bool {
	if e == nil || id == "" || e.Store == nil {
		return false
	}
	ws, ok := e.Store.Workspace(id)
	return !ok || ws.Archived
}

// CancelChat interrupts an in-flight chat turn for workspaceID+code (PA*, MA).
// Returns true if a turn was running and cancel was invoked, or if a zombie
// live status (persisted working/directing, no goroutine) was healed — that
// is the restart case where Stop previously did nothing and follow-ups queued.
func (e *Engine) CancelChat(workspaceID, code string) bool {
	if e == nil {
		return false
	}
	slot := e.chatTabSlot(workspaceID, code)
	slot.mu.Lock()
	turn := e.liveChatTurn(workspaceID, code)
	if turn != nil && turn.cancel != nil {
		slot.stopGen.Add(1)
		turn.markStopped()
		owned := turn.isOwned()
		if !owned {
			// No owning RPC will unregister — close ended so a waiter is not
			// stuck, then drop the handle after cancel.
			turn.closeEnded()
		}
		slot.mu.Unlock()
		if p, ok := e.takeModeContinue(workspaceID, code); ok {
			e.dropContinueGen(p.epoch)
			e.uncountModeRemount(workspaceID, code)
		}
		turn.cancel()
		if !owned {
			e.chats.CompareAndDelete(chatTurnKey(workspaceID, code), turn)
		}
		_ = e.idleWalkByChat(workspaceID, code)
		return true
	}
	// Ticket taken, turn not registered yet (attachment save / prompt wrap).
	if slot.next > slot.cur {
		slot.stopGen.Add(1)
		slot.mu.Unlock()
		_ = e.healZombieChat(workspaceID, code)
		_ = e.idleWalkByChat(workspaceID, code)
		return true
	}
	slot.mu.Unlock()
	if e.idleWalkByChat(workspaceID, code) {
		return true
	}
	return e.healZombieChat(workspaceID, code)
}

// ParkChatTurn stops the live chat turn for workspaceID+code (test / unconditional).
// Prefer parkChatTurnIf when the asking turn pointer is known.
func (e *Engine) ParkChatTurn(workspaceID, code string) bool {
	return e.parkChatTurnIf(workspaceID, code, e.liveChatTurn(workspaceID, code))
}

// parkChatTurnIf stops expect only when it is still the live turn for the tab.
// A nil expect (asking turn already gone) or a different live turn is a no-op so
// answering a parked ask_user cannot cancel a newer unrelated send.
func (e *Engine) parkChatTurnIf(workspaceID, code string, expect *chatTurn) bool {
	if e == nil || expect == nil || expect.cancel == nil {
		return false
	}
	turn := e.liveChatTurn(workspaceID, code)
	if turn == nil || turn != expect {
		return false
	}
	if turn.isStopped() {
		return false
	}
	turn.markParked()
	turn.cancel()
	return true
}

// chatTurnWasParked reports whether the live turn was stop-for-ask_user.
// Checked from finalizeChatReply while the turn handle is still registered.
func (e *Engine) chatTurnWasParked(workspaceID, code string) bool {
	if e == nil {
		return false
	}
	turn := e.liveChatTurn(workspaceID, code)
	if turn == nil {
		return false
	}
	return turn.isParked()
}

// chatTurnWasTimedOut reports whether the live turn's wedged deadline fired.
func (e *Engine) chatTurnWasTimedOut(workspaceID, code string) bool {
	if e == nil {
		return false
	}
	turn := e.liveChatTurn(workspaceID, code)
	if turn == nil {
		return false
	}
	return turn.isTimedOut()
}

func (t *chatTurn) markParked() {
	if t == nil {
		return
	}
	t.mu.Lock()
	t.parked = true
	t.mu.Unlock()
	t.stopChatInject()
}

func (t *chatTurn) markStopped() {
	if t == nil {
		return
	}
	t.mu.Lock()
	t.stopped = true
	t.mu.Unlock()
	t.stopChatInject()
}

func (t *chatTurn) dropOwner() {
	if t == nil {
		return
	}
	t.mu.Lock()
	t.owned = false
	t.mu.Unlock()
}

func (t *chatTurn) isOwned() bool {
	if t == nil {
		return false
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.owned
}

func (t *chatTurn) isStopped() bool {
	if t == nil {
		return false
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.stopped
}

func (t *chatTurn) isParked() bool {
	if t == nil {
		return false
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.parked
}

func (t *chatTurn) isTimedOut() bool {
	if t == nil {
		return false
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.timedOut
}

// arm starts (or restarts) the wedged-turn deadline.
func (t *chatTurn) arm(d time.Duration) {
	if t == nil || d <= 0 {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	t.armed = d
	t.rearmLocked()
}

// disarm stops any pending deadline (turn is ending).
func (t *chatTurn) disarm() {
	if t == nil {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.timer != nil {
		t.timer.Stop()
		t.timer = nil
	}
	t.armed = 0
}

func (t *chatTurn) rearmLocked() {
	if t.armed <= 0 || t.cancel == nil {
		return
	}
	if t.timer != nil {
		t.timer.Stop()
	}
	t.timer = time.AfterFunc(t.armed, func() {
		t.mu.Lock()
		t.timedOut = true
		cancel := t.cancel
		t.mu.Unlock()
		t.stopChatInject()
		if cancel != nil {
			cancel()
		}
	})
}

func (e *Engine) liveChatTurn(workspaceID, agentCode string) *chatTurn {
	workspaceID = strings.TrimSpace(workspaceID)
	agentCode = strings.TrimSpace(agentCode)
	if agentCode == "" {
		return nil
	}
	// Ask Recur turns are app-scoped: registered under an empty workspace ID
	// with an APP/APx tab code. Any other empty-workspace lookup is invalid.
	if workspaceID == "" && !isAppChatCode(agentCode) {
		return nil
	}
	if isPairChatCode(agentCode) {
		agentCode = normalizePairChatCode(agentCode)
	}
	v, ok := e.chats.Load(chatTurnKey(workspaceID, agentCode))
	if !ok {
		return nil
	}
	t, _ := v.(*chatTurn)
	return t
}

// chatTurnEnded is the prior turn's unregister signal. A follow-up that could
// not inject must wait so registerChatTurn does not Swap-cancel a healthy live
// turn. Nil means the tab is idle.
func (e *Engine) chatTurnEnded(workspaceID, code string) <-chan struct{} {
	t := e.liveChatTurn(workspaceID, code)
	if t == nil || t.ended == nil {
		return nil
	}
	return t.ended
}
