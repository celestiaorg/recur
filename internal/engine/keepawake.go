package engine

// acquireAgent takes a governor agent-slot and syncs the keep-awake assertion
// so macOS idle-sleep is suppressed while any agent subprocess is running (when
// Settings.PreventSleepWhileAgentsRun is on).
func (e *Engine) acquireAgent(done <-chan struct{}) (release func(), ok bool) {
	var slotRelease func()
	slotRelease, ok = e.gov.AcquireAgent(done)
	if !ok {
		return func() {}, false
	}
	e.syncKeepAwake()
	return func() {
		slotRelease()
		e.cursor.closeIfIdleRequested(e.gov.AgentsRunning() == 0)
		e.syncKeepAwake()
	}, true
}

// SyncKeepAwake re-evaluates the keep-awake assertion from current settings and
// agent-slot occupancy. Called after settings saves so toggling the preference
// applies immediately to in-flight agents.
func (e *Engine) SyncKeepAwake() { e.syncKeepAwake() }

func (e *Engine) syncKeepAwake() {
	// Serialize read+Set so a stale Set(false) from a just-finished release cannot
	// clobber a newer acquire's Set(true) (or the reverse). Slot mutations happen
	// before this call; under the lock we re-read the live count.
	e.keepAwakeMu.Lock()
	defer e.keepAwakeMu.Unlock()
	want := e.Store.Settings().PreventSleepWhileAgentsRun && e.gov.AgentsRunning() > 0
	e.keepAwake.Set(want)
}
