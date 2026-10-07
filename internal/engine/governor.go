package engine

import "sync"

// Governor enforces the machine-global resource limits (PRD §6.4): a cap on
// concurrently-running loops and a cap on concurrently-running agent subprocesses.
//
// Loops acquire a loop-slot before they start running; a loop that can't get one
// stays queued. Agent runners acquire an agent-slot before spawning `claude -p`
// and release it on exit. Both are machine-global because compute is physical
// (PRD §6.4). Caps come from store Settings at construction and can be updated
// live via SetCaps.
type Governor struct {
	mu        sync.Mutex
	agentWait *sync.Cond

	maxLoops  int
	maxAgents int
	loops     int
	agents    int
	waiting   int // AcquireAgent callers blocked on the cap
	iters     int // machine-global review iterations completed (status-bar "iters today")
}

// NewGovernor builds a Governor with the given caps. Non-positive caps are
// treated as unlimited so a misconfigured 0 never wedges the engine.
func NewGovernor(maxLoops, maxAgents int) *Governor {
	g := &Governor{maxLoops: maxLoops, maxAgents: maxAgents}
	g.agentWait = sync.NewCond(&g.mu)
	return g
}

func atCap(n, max int) bool { return max > 0 && n >= max }

// SetCaps updates the live caps. Raising the agent cap wakes waiters; raising
// the loop cap does not start queued loops — the caller should drainQueued.
// Lowering does not preempt in-flight work; new acquires wait until the count
// drops under the new max.
func (g *Governor) SetCaps(maxLoops, maxAgents int) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.maxLoops = maxLoops
	g.maxAgents = maxAgents
	g.agentWait.Broadcast()
}

// TryAcquireLoop takes a loop-slot without blocking. Returns false if all loop
// slots are in use (the caller should mark the loop queued).
func (g *Governor) TryAcquireLoop() bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	if atCap(g.loops, g.maxLoops) {
		return false
	}
	g.loops++
	return true
}

// FreeLoops is how many more loops may start right now. Unlimited caps report
// a large number.
func (g *Governor) FreeLoops() int {
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.maxLoops <= 0 {
		return 1 << 20
	}
	if n := g.maxLoops - g.loops; n > 0 {
		return n
	}
	return 0
}

// ReleaseLoop returns a loop-slot. Safe to call once per successful acquire.
func (g *Governor) ReleaseLoop() {
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.loops > 0 {
		g.loops--
	}
}

// AcquireAgent blocks until an agent-slot is free or done is closed. Returns a
// release func (call once) and ok=false if done closed before a slot opened.
func (g *Governor) AcquireAgent(done <-chan struct{}) (release func(), ok bool) {
	g.mu.Lock()
	defer g.mu.Unlock()
	for atCap(g.agents, g.maxAgents) {
		if doneClosed(done) {
			return func() {}, false
		}
		stop := make(chan struct{})
		go func() {
			select {
			case <-done:
				g.mu.Lock()
				g.agentWait.Broadcast()
				g.mu.Unlock()
			case <-stop:
			}
		}()
		g.waiting++
		g.agentWait.Wait()
		g.waiting--
		close(stop)
		if doneClosed(done) {
			return func() {}, false
		}
	}
	g.agents++
	return func() { g.releaseAgent() }, true
}

func doneClosed(done <-chan struct{}) bool {
	select {
	case <-done:
		return true
	default:
		return false
	}
}

func (g *Governor) releaseAgent() {
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.agents > 0 {
		g.agents--
	}
	g.agentWait.Signal()
}

// AddIters accumulates completed review iterations into the machine-global counter
// surfaced as "iters today" in the status bar (PRD §6.4, §16.1). Called once per
// completed review iteration across all loops; concurrency-safe (the loop goroutines
// touch it concurrently).
func (g *Governor) AddIters(n int) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.iters += n
}

// Iters returns the machine-global completed-iteration count (status-bar rollup).
func (g *Governor) Iters() int {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.iters
}

// LoopsRunning / AgentsRunning report the current in-use slot counts for the
// status-bar (resource.updated).
func (g *Governor) LoopsRunning() int {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.loops
}

func (g *Governor) AgentsRunning() int {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.agents
}

func (g *Governor) AgentsWaiting() int {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.waiting
}
