package usage

import (
	"context"
	"errors"
	"sync"
	"time"
)

const (
	pollInterval   = 3 * time.Minute
	freshWindow    = 60 * time.Second
	collectTimeout = 25 * time.Second
	// claudeCollectBudget caps CLI + HTTP when Codex/Cursor also run, so a
	// hung `claude auth status` cannot spend the whole Collect timeout.
	claudeCollectBudget = 12 * time.Second
)

// Publisher is the engine bus (or a test fake).
type Publisher func(name string, data any)

type refreshCall struct {
	done chan struct{}
	snap Snapshot
}

// Poller caches the last snapshot and refreshes on a conservative interval.
type Poller struct {
	fetch *Fetcher
	pub   Publisher

	mu         sync.Mutex
	last       Snapshot
	has        bool
	stop       chan struct{}
	stopped    bool
	looping    bool
	refreshing *refreshCall
}

func NewPoller(fetch *Fetcher, pub Publisher) *Poller {
	return &Poller{fetch: fetch, pub: pub, last: EmptySnapshot(), stop: make(chan struct{})}
}

// Start prepares the poller. The first Snapshot/Refresh starts the ticker so
// launch does not touch provider keychains until the UI asks.
func (p *Poller) Start() {}

func (p *Poller) Stop() {
	p.mu.Lock()
	if p.stopped {
		p.mu.Unlock()
		return
	}
	p.stopped = true
	close(p.stop)
	p.mu.Unlock()
	p.fetch.CancelConnect()
}

func (p *Poller) kickLoop() {
	p.mu.Lock()
	if p.looping || p.stopped {
		p.mu.Unlock()
		return
	}
	p.looping = true
	p.mu.Unlock()
	go p.loop()
}

func (p *Poller) loop() {
	tick := time.NewTicker(pollInterval)
	defer tick.Stop()
	for {
		select {
		case <-p.stop:
			return
		case <-tick.C:
			p.Refresh(context.Background())
		}
	}
}

// Snapshot returns the last result, refreshing if it is older than freshWindow.
// force bypasses the cache but still joins an in-flight collect.
func (p *Poller) Snapshot(force bool) Snapshot {
	p.kickLoop()
	p.mu.Lock()
	last := p.last
	has := p.has
	p.mu.Unlock()
	if !force && has && p.fetch.now().Sub(time.UnixMilli(last.FetchedAt)) < freshWindow {
		return copySnapshot(last)
	}
	return p.Refresh(context.Background())
}

func (p *Poller) Refresh(ctx context.Context) Snapshot {
	p.kickLoop()
	p.mu.Lock()
	if p.refreshing != nil {
		call := p.refreshing
		p.mu.Unlock()
		<-call.done
		return copySnapshot(call.snap)
	}
	call := &refreshCall{done: make(chan struct{})}
	p.refreshing = call
	p.mu.Unlock()
	defer func() {
		p.mu.Lock()
		if p.refreshing == call {
			p.refreshing = nil
			close(call.done)
		}
		p.mu.Unlock()
	}()

	cctx, cancel := context.WithTimeout(ctx, collectTimeout)
	next := p.fetch.Collect(cctx)
	cancel()

	p.mu.Lock()
	merged := mergeStale(p.last, next)
	p.last = merged
	p.has = true
	call.snap = merged
	p.mu.Unlock()
	if p.pub != nil {
		p.pub("provider.usage", copySnapshot(merged))
	}
	return copySnapshot(merged)
}

func mergeStale(prev, next Snapshot) Snapshot {
	byID := map[string]ProviderQuota{}
	for _, p := range prev.Providers {
		byID[p.ID] = p
	}
	out := next
	out.Stale = false
	for i, q := range out.Providers {
		old, hasOld := byID[q.ID]
		switch {
		case q.Error == "rate_limited":
			if hasOld && len(old.Windows) > 0 {
				old.Error = ""
				out.Providers[i] = old
				out.Stale = true
				continue
			}
			out.Providers[i].Error = "Usage endpoint is rate limited"
			out.Providers[i].Hint = "Showing nothing until the next successful refresh."
		case isReachabilityError(q.Error) && hasOld && len(old.Windows) > 0 && len(q.Windows) == 0:
			old.Error = ""
			out.Providers[i] = old
			out.Stale = true
		}
	}
	// Keep last-known quota rows when a provider was not fetched this
	// pass (turned off in Settings). Re-enabling can show them immediately.
	// Windowless stubs (no session, Connect tokens) are not carried.
	have := map[string]bool{}
	for _, q := range out.Providers {
		have[q.ID] = true
	}
	for _, old := range prev.Providers {
		if have[old.ID] || len(old.Windows) == 0 {
			continue
		}
		out.Providers = append(out.Providers, old)
	}
	// FetchedAt stays on the attempt (next) so a 429 cannot freeze the
	// freshness gate and turn every UI read into another Collect.
	return out
}

func copySnapshot(s Snapshot) Snapshot {
	out := s
	if s.Providers != nil {
		out.Providers = append([]ProviderQuota(nil), s.Providers...)
		for i, p := range out.Providers {
			if p.Windows != nil {
				out.Providers[i].Windows = append([]Window(nil), p.Windows...)
			}
			if p.Models != nil {
				out.Providers[i].Models = append([]ModelRow(nil), p.Models...)
			}
		}
	}
	return out
}

func (p *Poller) ConnectCursor() CursorUsageConnect {
	p.kickLoop()
	conn := p.fetch.StartConnect(func(err error) {
		if err != nil {
			if errors.Is(err, context.Canceled) {
				return
			}
			msg := "Sign-in timed out — try Connect again"
			if !errors.Is(err, context.DeadlineExceeded) {
				msg = "Cursor sign-in failed — try Connect again"
			}
			p.markCursorConnectFailed(msg)
			return
		}
		p.Refresh(context.Background())
	})
	if conn.Pending {
		p.markCursorConnecting(true)
	}
	return conn
}

func (p *Poller) CancelConnect() {
	p.fetch.CancelConnect()
	p.markCursorConnecting(false)
	go p.Refresh(context.Background())
}

func (p *Poller) markCursorConnecting(on bool) {
	p.mu.Lock()
	s := copySnapshot(p.last)
	found := false
	for i := range s.Providers {
		if s.Providers[i].ID != ProviderCursor {
			continue
		}
		s.Providers[i].Connecting = on
		if on {
			s.Providers[i].Hint = "Waiting for browser sign-in…"
			s.Providers[i].Error = ""
		} else {
			s.Providers[i].Hint = ""
		}
		found = true
	}
	if !found && on {
		s.Providers = append(s.Providers, ProviderQuota{
			ID: ProviderCursor, Name: "Cursor", Connecting: true,
			Hint: "Waiting for browser sign-in…",
		})
	}
	p.last = s
	p.has = true
	p.mu.Unlock()
	if p.pub != nil {
		p.pub("provider.usage", copySnapshot(s))
	}
}

func (p *Poller) markCursorConnectFailed(msg string) {
	p.mu.Lock()
	s := copySnapshot(p.last)
	found := false
	for i := range s.Providers {
		if s.Providers[i].ID != ProviderCursor {
			continue
		}
		s.Providers[i].Connecting = false
		s.Providers[i].Error = "Cursor sign-in did not finish"
		s.Providers[i].Hint = msg
		found = true
	}
	if !found {
		s.Providers = append(s.Providers, ProviderQuota{
			ID: ProviderCursor, Name: "Cursor",
			Error: "Cursor sign-in did not finish", Hint: msg,
		})
	}
	p.last = s
	p.has = true
	p.mu.Unlock()
	if p.pub != nil {
		p.pub("provider.usage", copySnapshot(s))
	}
}

func (p *Poller) DisconnectCursor() error {
	p.fetch.CancelConnect()
	dir := p.fetch.dataDir()
	if dir == "" {
		return nil
	}
	if tok, err := readCursorAuthFile(cursorAuthPath(dir)); err == nil && tok.APIKey != "" && tok.Access != "" {
		_ = revokeCursorUserAPIKey(context.Background(), p.fetch, tok.Access, tok.APIKey, tok.APIKeyID)
	}
	err := removeCursorAuthFile(cursorAuthPath(dir))
	p.forgetProvider(ProviderCursor)
	go p.Refresh(context.Background())
	return err
}

func (p *Poller) forgetProvider(id string) {
	p.mu.Lock()
	s := copySnapshot(p.last)
	kept := s.Providers[:0]
	for _, q := range s.Providers {
		if q.ID != id {
			kept = append(kept, q)
		}
	}
	s.Providers = kept
	p.last = s
	p.mu.Unlock()
}
