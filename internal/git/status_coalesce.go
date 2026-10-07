package git

import (
	"context"
	"sync"
	"time"
)

// Observatory switch fires ListTree + GitStatus + ListChangedFiles + WorkingStatus
// + land-state/discard-risk in parallel, and each used to shell its own
// merge-base / diff / porcelain. Concurrent git on one worktree serializes on
// index.lock, so the "parallel" refresh was often slower than one pass — and
// it billed every workspace equally, even a quiet one.
//
// In-flight coalescing shares one computation among waiters. Results are not
// cached after the flight ends: the next live-refresh tick must see disk.

const statusWorkTimeout = 15 * time.Second

type statusFlight struct {
	done chan struct{}
	val  map[string]string
}

type statusFlights struct {
	mu    sync.Mutex
	calls map[string]*statusFlight
}

func (g *statusFlights) do(ctx context.Context, key string, work func(context.Context) map[string]string) map[string]string {
	g.mu.Lock()
	if g.calls == nil {
		g.calls = map[string]*statusFlight{}
	}
	if f, ok := g.calls[key]; ok {
		g.mu.Unlock()
		select {
		case <-ctx.Done():
			return nil
		case <-f.done:
			return f.val
		}
	}
	f := &statusFlight{done: make(chan struct{})}
	g.calls[key] = f
	g.mu.Unlock()

	runCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), statusWorkTimeout)
	val := work(runCtx)
	cancel()
	if statusComputeHook != nil {
		statusComputeHook("flight-hold")
	}

	g.mu.Lock()
	f.val = val
	delete(g.calls, key)
	close(f.done)
	g.mu.Unlock()
	return val
}

var (
	changeStatusFlights statusFlights
	statusMapFlights    statusFlights
)

// statusComputeHook, when set (tests), runs inside the real git work — not on
// waiters — so coalescing can be counted.
var statusComputeHook func(kind string)
