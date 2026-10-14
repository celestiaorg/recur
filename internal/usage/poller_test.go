package usage

import (
	"context"
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestRefreshSingleFlight(t *testing.T) {
	started := make(chan struct{}, 1)
	release := make(chan struct{})
	var n atomic.Int32
	f := &Fetcher{
		WantClaude: true,
		Now:        func() time.Time { return time.UnixMilli(1_000) },
		ClaudeToken: func() (string, error) {
			n.Add(1)
			select {
			case started <- struct{}{}:
			default:
			}
			<-release
			return "", fmt.Errorf("no token")
		},
	}
	p := NewPoller(f, nil)
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		p.Refresh(context.Background())
	}()
	<-started
	go func() {
		defer wg.Done()
		p.Refresh(context.Background())
	}()
	time.Sleep(20 * time.Millisecond)
	close(release)
	wg.Wait()
	if n.Load() != 1 {
		t.Fatalf("collects = %d, want 1", n.Load())
	}
	p.Stop()
}

func TestRefreshCleansUpAfterPanic(t *testing.T) {
	var n atomic.Int32
	f := &Fetcher{
		WantClaude: true,
		Now:        func() time.Time { return time.UnixMilli(1) },
		ClaudeToken: func() (string, error) {
			if n.Add(1) == 1 {
				panic("collect boom")
			}
			return "", fmt.Errorf("no token")
		},
	}
	p := NewPoller(f, nil)
	func() {
		defer func() { _ = recover() }()
		p.Refresh(context.Background())
	}()
	done := make(chan struct{})
	go func() {
		p.Refresh(context.Background())
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("Refresh blocked after a panicking Collect")
	}
	p.Stop()
}

func TestMarkCursorConnectingClearsHint(t *testing.T) {
	t.Parallel()
	p := NewPoller(&Fetcher{}, nil)
	p.markCursorConnecting(true)
	if p.last.Providers[0].Hint == "" || !p.last.Providers[0].Connecting {
		t.Fatalf("on = %+v", p.last.Providers[0])
	}
	p.markCursorConnecting(false)
	if p.last.Providers[0].Connecting {
		t.Fatal("Connecting still set")
	}
	if p.last.Providers[0].Hint != "" {
		t.Fatalf("Hint = %q, want cleared", p.last.Providers[0].Hint)
	}
}

func TestForgetProviderDropsRow(t *testing.T) {
	t.Parallel()
	p := NewPoller(&Fetcher{}, nil)
	p.last = Snapshot{Providers: []ProviderQuota{
		{ID: ProviderClaude, Windows: []Window{{ID: WindowFiveHour}}},
		{ID: ProviderCursor, Windows: []Window{{ID: "billing"}}},
	}}
	p.has = true
	p.forgetProvider(ProviderCursor)
	if len(p.last.Providers) != 1 || p.last.Providers[0].ID != ProviderClaude {
		t.Errorf("forgetProvider(cursor) = %+v, want only claude", p.last.Providers)
	}
}

func TestMarkCursorConnectFailed(t *testing.T) {
	t.Parallel()
	p := NewPoller(&Fetcher{}, nil)
	p.markCursorConnecting(true)
	p.markCursorConnectFailed("Sign-in timed out — try Connect again")
	q := p.last.Providers[0]
	if q.Connecting {
		t.Fatal("still connecting")
	}
	if q.Error == "" || q.Hint == "" {
		t.Fatalf("failed = %+v", q)
	}
}
