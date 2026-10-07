package lsp

import (
	"io"
	"sync"
	"testing"
	"time"
)

func TestSendEventPreservesOrder(t *testing.T) {
	var got []string
	var mu sync.Mutex
	s := &Session{
		ID:     "s1",
		events: make(chan lspEvent, 64),
		quit:   make(chan struct{}),
		emit: func(name string, data any) {
			time.Sleep(2 * time.Millisecond)
			mu.Lock()
			got = append(got, data.(map[string]any)["message"].(string))
			mu.Unlock()
		},
	}
	go s.emitLoop()
	s.sendEvent("lsp.message", map[string]any{"message": "a"})
	s.sendEvent("lsp.message", map[string]any{"message": "b"})
	s.sendEvent("lsp.message", map[string]any{"message": "c"})
	s.stopEvents()
	deadline := time.Now().Add(time.Second)
	for time.Now().Before(deadline) {
		mu.Lock()
		n := len(got)
		mu.Unlock()
		if n == 3 {
			break
		}
		time.Sleep(5 * time.Millisecond)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(got) != 3 || got[0] != "a" || got[1] != "b" || got[2] != "c" {
		t.Fatalf("order=%v", got)
	}
}

type blockWriter struct {
	started chan struct{}
	release chan struct{}
	once    sync.Once
}

func (w *blockWriter) Write(p []byte) (int, error) {
	w.once.Do(func() { close(w.started) })
	<-w.release
	return len(p), nil
}

func TestCloseDoesNotWaitOnWrite(t *testing.T) {
	w := &blockWriter{started: make(chan struct{}), release: make(chan struct{})}
	s := &Session{
		stdin:  nopCloser{w},
		events: make(chan lspEvent, 8),
		quit:   make(chan struct{}),
		emit:   func(string, any) {},
		cmd:    nil,
	}
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		_ = s.Write(`{"jsonrpc":"2.0"}`)
	}()
	select {
	case <-w.started:
	case <-time.After(time.Second):
		t.Fatal("write did not start")
	}
	done := make(chan struct{})
	go func() {
		s.Close()
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(time.Second):
		close(w.release)
		t.Fatal("Close blocked on Write")
	}
	close(w.release)
	wg.Wait()
}

type nopCloser struct{ io.Writer }

func (nopCloser) Close() error { return nil }

func TestWriteRejectsAfterClose(t *testing.T) {
	s := &Session{
		stdin:  nopCloser{io.Discard},
		events: make(chan lspEvent, 8),
		quit:   make(chan struct{}),
		emit:   func(string, any) {},
	}
	s.Close()
	if err := s.Write("{}"); err == nil {
		t.Fatal("expected closed error")
	}
}

func TestCloseAfterStopEventsDoesNotPanic(t *testing.T) {
	s := &Session{
		ID:     "s1",
		stdin:  nopCloser{io.Discard},
		events: make(chan lspEvent, 8),
		quit:   make(chan struct{}),
		emit:   func(string, any) {},
	}
	go s.emitLoop()
	s.sendEvent("lsp.message", map[string]any{"message": "m"})
	s.emitExit()
	s.stopEvents()
	s.Close()
}

func TestConcurrentSendStopAndCloseDoesNotPanic(t *testing.T) {
	for i := 0; i < 100; i++ {
		s := &Session{
			ID:     "s",
			stdin:  nopCloser{io.Discard},
			events: make(chan lspEvent, 8),
			quit:   make(chan struct{}),
			emit:   func(string, any) {},
		}
		go s.emitLoop()
		var wg sync.WaitGroup
		wg.Add(3)
		go func() {
			defer wg.Done()
			for j := 0; j < 20; j++ {
				s.sendEvent("lsp.message", map[string]any{"message": "m"})
			}
		}()
		go func() {
			defer wg.Done()
			s.emitExit()
		}()
		go func() {
			defer wg.Done()
			s.Close()
			s.stopEvents()
		}()
		wg.Wait()
	}
}

func TestExitFollowsStdoutMessages(t *testing.T) {
	var got []string
	var mu sync.Mutex
	s := &Session{
		ID:     "s1",
		events: make(chan lspEvent, 64),
		quit:   make(chan struct{}),
		emit: func(name string, data any) {
			mu.Lock()
			defer mu.Unlock()
			if name == "lsp.exit" {
				got = append(got, "exit")
				return
			}
			got = append(got, data.(map[string]any)["message"].(string))
		},
	}
	go s.emitLoop()
	s.sendEvent("lsp.message", map[string]any{"message": "a"})
	s.sendEvent("lsp.message", map[string]any{"message": "b"})
	s.emitExit()
	s.stopEvents()
	deadline := time.Now().Add(time.Second)
	for time.Now().Before(deadline) {
		mu.Lock()
		n := len(got)
		mu.Unlock()
		if n == 3 {
			break
		}
		time.Sleep(5 * time.Millisecond)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(got) != 3 || got[0] != "a" || got[1] != "b" || got[2] != "exit" {
		t.Fatalf("order=%v", got)
	}
}
