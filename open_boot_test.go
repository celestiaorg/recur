package main

import (
	"net"
	"os"
	"reflect"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"recur/internal/openipc"
)

func TestPeelOpenArgs(t *testing.T) {
	opens, rest := peelOpenArgs([]string{"--open", "/tmp/a", "--other", "--open=/tmp/b", "x"})
	if !reflect.DeepEqual(opens, []string{"/tmp/a", "/tmp/b"}) {
		t.Fatalf("peelOpenArgs opens = %v, want [/tmp/a /tmp/b]", opens)
	}
	if !reflect.DeepEqual(rest, []string{"--other", "x"}) {
		t.Fatalf("peelOpenArgs rest = %v, want [--other x]", rest)
	}
	opens, rest = peelOpenArgs(nil)
	if len(opens) != 0 || len(rest) != 0 {
		t.Fatalf("peelOpenArgs(nil) = %v %v, want empty", opens, rest)
	}
	opens, rest = peelOpenArgs([]string{"--open"})
	if len(opens) != 0 {
		t.Fatalf("peelOpenArgs([--open]) opens = %v, want empty", opens)
	}
	if len(rest) != 0 {
		t.Fatalf("peelOpenArgs([--open]) rest = %v, want empty (flag dropped)", rest)
	}
}

func TestHandoffToRunningInstanceNoListener(t *testing.T) {
	shortDataDir(t)
	if handoffToRunningInstance(nil) {
		t.Fatal("handoffToRunningInstance(nil) = true, want false (no listener)")
	}
	if handoffToRunningInstance([]string{"/tmp/x"}) {
		t.Fatal("handoffToRunningInstance([/tmp/x]) = true, want false (no listener)")
	}
}

func TestHandoffToRunningInstanceFocusesLiveApp(t *testing.T) {
	shortDataDir(t)
	sock, err := openipc.SockPath()
	if err != nil {
		t.Fatal(err)
	}
	stop := make(chan struct{})
	defer close(stop)
	got := make(chan openipc.Request, 2)
	serveErr := make(chan error, 1)
	go func() {
		serveErr <- openipc.Serve(stop, sock, func(req openipc.Request) openipc.Response {
			got <- req
			return openipc.Response{OK: true, Path: req.Path}
		})
	}()
	waitForSock(t, sock, serveErr)

	if !handoffToRunningInstance(nil) {
		t.Fatal("handoffToRunningInstance(nil) = false, want true (live listener)")
	}
	select {
	case req := <-got:
		if req.Path != "" {
			t.Fatalf("handoffToRunningInstance(nil) Path = %q, want empty (focus only)", req.Path)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("handoffToRunningInstance(nil) did not reach the socket")
	}
}

func TestHandoffToRunningInstanceForwardsOpenPaths(t *testing.T) {
	shortDataDir(t)
	sock, err := openipc.SockPath()
	if err != nil {
		t.Fatal(err)
	}
	stop := make(chan struct{})
	defer close(stop)
	got := make(chan openipc.Request, 2)
	serveErr := make(chan error, 1)
	go func() {
		serveErr <- openipc.Serve(stop, sock, func(req openipc.Request) openipc.Response {
			got <- req
			return openipc.Response{OK: true, Path: req.Path}
		})
	}()
	waitForSock(t, sock, serveErr)

	paths := []string{"/tmp/a", "/tmp/b"}
	if !handoffToRunningInstance(paths) {
		t.Fatal("handoffToRunningInstance([/tmp/a /tmp/b]) = false, want true")
	}
	for i, want := range paths {
		select {
		case req := <-got:
			if req.Path != want {
				t.Fatalf("handoffToRunningInstance path[%d] = %q, want %q", i, req.Path, want)
			}
		case <-time.After(2 * time.Second):
			t.Fatalf("handoffToRunningInstance did not forward path[%d] %q", i, want)
		}
	}
}

func TestHandoffToRunningInstanceConnectedError(t *testing.T) {
	// A live listener that drops the connection is not a completed handoff.
	// claimOrHandoff retries Claim; the flock still prevents Recover beside it.
	shortDataDir(t)
	sock, err := openipc.SockPath()
	if err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			_ = c.Close()
		}
	}()

	if handoffToRunningInstance(nil) {
		t.Fatal("handoffToRunningInstance(nil) = true after connected drop, want false (retry Claim)")
	}
}

func TestClaimOrHandoffWhenHeld(t *testing.T) {
	shortDataDir(t)
	inst, err := openipc.Claim()
	if err != nil {
		t.Fatalf("Claim() error = %v, want nil", err)
	}
	t.Cleanup(inst.Close)
	stop := make(chan struct{})
	t.Cleanup(func() { close(stop) })
	go func() {
		_ = openipc.ServeListener(stop, inst, func(req openipc.Request) openipc.Response {
			return openipc.Response{OK: true, Path: req.Path}
		})
	}()

	_, handed, err := claimOrHandoff(nil)
	if err != nil {
		t.Fatalf("claimOrHandoff(nil) error = %v, want nil", err)
	}
	if !handed {
		t.Fatal("claimOrHandoff(nil) handed = false, want true")
	}
}

func TestClaimOrHandoffReclaimsAfterRelease(t *testing.T) {
	shortDataDir(t)
	held, err := openipc.Claim()
	if err != nil {
		t.Fatalf("Claim() error = %v, want nil", err)
	}
	go func() {
		time.Sleep(80 * time.Millisecond)
		held.Close()
	}()

	inst, handed, err := claimOrHandoff(nil)
	if err != nil {
		t.Fatalf("claimOrHandoff(nil) error = %v, want nil", err)
	}
	if handed {
		t.Fatal("claimOrHandoff(nil) handed = true, want false (reclaimed)")
	}
	if inst == nil {
		t.Fatal("claimOrHandoff(nil) inst = nil, want claimed instance")
	}
	inst.Close()
}

func TestIPCGateStopAuthoritative(t *testing.T) {
	g := newIPCGate()
	called := 0
	g.Set(func(openipc.Request) openipc.Response {
		called++
		return openipc.Response{OK: true}
	})
	g.Stop()
	for i := 0; i < 50; i++ {
		res := g.Handle(openipc.Request{})
		if res.OK || res.Error != openipc.ErrShuttingDown {
			t.Fatalf("Handle after Stop = %+v, want error %q", res, openipc.ErrShuttingDown)
		}
	}
	if called != 0 {
		t.Fatalf("handler called %d times after Stop, want 0", called)
	}
}

func TestIPCGateStopPreventsAdmission(t *testing.T) {
	g := newIPCGate()
	var calls atomic.Int64
	g.Set(func(openipc.Request) openipc.Response {
		calls.Add(1)
		return openipc.Response{OK: true}
	})
	var wg sync.WaitGroup
	for i := 0; i < 64; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			g.Handle(openipc.Request{})
		}()
	}
	g.Stop()
	wg.Wait()
	afterStop := calls.Load()
	for i := 0; i < 64; i++ {
		res := g.Handle(openipc.Request{})
		if res.OK || res.Error != openipc.ErrShuttingDown {
			t.Fatalf("Handle after Stop = %+v, want error %q", res, openipc.ErrShuttingDown)
		}
	}
	if got := calls.Load(); got != afterStop {
		t.Fatalf("Handle after Stop admitted %d more calls, want 0 (afterStop=%d)", got-afterStop, afterStop)
	}
}

func TestIPCGateStopUnblocksWaiter(t *testing.T) {
	g := newIPCGate()
	done := make(chan openipc.Response, 1)
	go func() {
		done <- g.Handle(openipc.Request{})
	}()
	g.Stop()
	select {
	case res := <-done:
		if res.OK || res.Error != openipc.ErrShuttingDown {
			t.Fatalf("Handle after Stop = %+v, want error %q", res, openipc.ErrShuttingDown)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Handle still blocked after Stop, want shutting-down response")
	}
}

func TestOpenHubDropUISkipsFocus(t *testing.T) {
	h := &openHub{}
	called := 0
	h.setHooks(nil, func(applied func()) { called++; applied() })
	h.markAppLive()
	if !h.bringForward() {
		t.Fatal("bringForward while live = false, want true")
	}
	h.waitFocus()
	if called != 1 {
		t.Fatalf("bringForward while live called focus %d times, want 1", called)
	}
	h.dropUI()
	if h.bringForward() {
		t.Fatal("bringForward after dropUI = true, want false")
	}
	h.waitFocus()
	if called != 1 {
		t.Fatalf("bringForward after dropUI called focus %d times, want 1", called)
	}
}

func TestOpenHubBringForwardWaitsForApplied(t *testing.T) {
	// Posting InvokeAsync is not a commit: terminate can skip the queue.
	h := &openHub{}
	var apply func()
	posted := make(chan struct{})
	h.setHooks(nil, func(applied func()) {
		apply = applied
		close(posted)
	})
	h.markAppLive()
	got := make(chan bool, 1)
	go func() { got <- h.bringForward() }()
	select {
	case <-posted:
	case <-time.After(2 * time.Second):
		t.Fatal("bringForward did not post focus")
	}
	select {
	case ok := <-got:
		t.Fatalf("bringForward() = %v after post, want wait until applied", ok)
	case <-time.After(20 * time.Millisecond):
	}
	apply()
	select {
	case ok := <-got:
		if !ok {
			t.Fatal("bringForward() = false after applied, want true")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("bringForward did not return after applied")
	}
	h.waitFocus()
}

func TestOpenHubBringForwardFalseIfRevokedBeforeApplied(t *testing.T) {
	// Quit's terminate delegate can dropUI before the queued Show runs.
	h := &openHub{}
	posted := make(chan struct{})
	h.setHooks(nil, func(func()) { close(posted) })
	h.markAppLive()
	got := make(chan bool, 1)
	go func() { got <- h.bringForward() }()
	select {
	case <-posted:
	case <-time.After(2 * time.Second):
		t.Fatal("bringForward did not post focus")
	}
	h.dropUI()
	select {
	case ok := <-got:
		if ok {
			t.Fatal("bringForward() = true after dropUI before applied, want false")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("bringForward did not return after dropUI")
	}
	h.waitFocus()
}

func TestOpenHubWaitFocusDrainsInFlight(t *testing.T) {
	h := &openHub{}
	started := make(chan struct{})
	release := make(chan struct{})
	h.setHooks(nil, func(applied func()) {
		close(started)
		<-release
		applied()
	})
	h.markAppLive()
	go h.bringForward()
	select {
	case <-started:
	case <-time.After(2 * time.Second):
		t.Fatal("bringForward did not start focus")
	}
	h.dropUI()
	done := make(chan struct{})
	go func() {
		h.waitFocus()
		close(done)
	}()
	select {
	case <-done:
		t.Fatal("waitFocus returned while focus still running, want block until Done")
	case <-time.After(50 * time.Millisecond):
	}
	close(release)
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("waitFocus did not return after focus finished")
	}
}

func TestRevokeOpenUIRejectsHandoffDuringWaitFocus(t *testing.T) {
	g := newIPCGate()
	h := &openHub{}
	started := make(chan struct{})
	release := make(chan struct{})
	h.setHooks(nil, func(applied func()) {
		close(started)
		<-release
		applied()
	})
	h.markAppLive()
	g.Set(func(openipc.Request) openipc.Response {
		h.bringForward()
		return openipc.Response{OK: true}
	})
	go func() { _ = g.Handle(openipc.Request{}) }()
	select {
	case <-started:
	case <-time.After(2 * time.Second):
		t.Fatal("admitted Handle did not start focus")
	}
	revokeOpenUI(h, g)
	res := g.Handle(openipc.Request{})
	if res.OK || res.Error != openipc.ErrShuttingDown {
		t.Fatalf("Handle after revokeOpenUI = %+v, want error %q", res, openipc.ErrShuttingDown)
	}
	close(release)
	h.waitFocus()
}

func TestAttachOpenIPCEmptyPathAfterDropUI(t *testing.T) {
	// Gate still admits (Stop not called); UI is already gone. Must not OK.
	g := newIPCGate()
	h := &openHub{}
	attachOpenIPC(g, nil, h)
	h.setHooks(nil, func(func()) { t.Error("focus called after dropUI") })
	h.markAppLive()
	h.dropUI()
	res := g.Handle(openipc.Request{})
	if res.OK || res.Error != openipc.ErrShuttingDown {
		t.Fatalf("Handle after dropUI = %+v, want error %q", res, openipc.ErrShuttingDown)
	}
}

func TestCopiedHandleReturnsShuttingDownAfterRevoke(t *testing.T) {
	g := newIPCGate()
	h := &openHub{}
	entered := make(chan struct{})
	release := make(chan struct{})
	h.setHooks(nil, func(func()) { t.Error("focus called after dropUI") })
	h.markAppLive()
	g.Set(func(openipc.Request) openipc.Response {
		close(entered)
		<-release
		if !h.bringForward() {
			return openipc.Response{OK: false, Error: openipc.ErrShuttingDown}
		}
		return openipc.Response{OK: true}
	})
	done := make(chan openipc.Response, 1)
	go func() { done <- g.Handle(openipc.Request{}) }()
	select {
	case <-entered:
	case <-time.After(2 * time.Second):
		t.Fatal("Handle did not start")
	}
	revokeOpenUI(h, g)
	close(release)
	select {
	case res := <-done:
		if res.OK || res.Error != openipc.ErrShuttingDown {
			t.Fatalf("Handle overlapping revokeOpenUI = %+v, want error %q", res, openipc.ErrShuttingDown)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Handle did not return after revokeOpenUI")
	}
}

func TestOpenHubWaitFocusIsAuthoritative(t *testing.T) {
	h := &openHub{}
	var n atomic.Int64
	h.setHooks(nil, func(applied func()) {
		time.Sleep(5 * time.Millisecond)
		n.Add(1)
		applied()
	})
	h.markAppLive()
	var wg sync.WaitGroup
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			h.bringForward()
		}()
	}
	time.Sleep(2 * time.Millisecond)
	h.dropUI()
	h.waitFocus()
	after := n.Load()
	for i := 0; i < 32; i++ {
		h.bringForward()
	}
	wg.Wait()
	if got := n.Load(); got != after {
		t.Fatalf("bringForward after waitFocus called focus %d more times, want 0 (after=%d)", got-after, after)
	}
}

func TestClaimRetryCoversOpenPathDrain(t *testing.T) {
	min := openipc.CallTimeout + 8*time.Second
	if claimRetryFor < min {
		t.Fatalf("claimRetryFor = %v, want >= %v so reclaim waits for OpenPath drain plus Shutdown", claimRetryFor, min)
	}
}

func TestHandoffShutdownResponseIsNotHandoff(t *testing.T) {
	shortDataDir(t)
	inst, err := openipc.Claim()
	if err != nil {
		t.Fatalf("Claim() error = %v, want nil", err)
	}
	t.Cleanup(inst.Close)
	stop := make(chan struct{})
	t.Cleanup(func() { close(stop) })
	go func() {
		_ = openipc.ServeListener(stop, inst, func(openipc.Request) openipc.Response {
			return openipc.Response{OK: false, Error: openipc.ErrShuttingDown}
		})
	}()
	if handoffToRunningInstance(nil) {
		t.Fatal("handoffToRunningInstance(nil) = true on shutting down, want false")
	}
}

func TestHandoffPartialShutdownIsNotHandoff(t *testing.T) {
	shortDataDir(t)
	inst, err := openipc.Claim()
	if err != nil {
		t.Fatalf("Claim() error = %v, want nil", err)
	}
	t.Cleanup(inst.Close)
	stop := make(chan struct{})
	t.Cleanup(func() { close(stop) })
	go func() {
		_ = openipc.ServeListener(stop, inst, func(req openipc.Request) openipc.Response {
			if req.Path == "/tmp/b" {
				return openipc.Response{OK: false, Error: openipc.ErrShuttingDown}
			}
			return openipc.Response{OK: true, Path: req.Path}
		})
	}()
	if handoffToRunningInstance([]string{"/tmp/a", "/tmp/b"}) {
		t.Fatal("handoffToRunningInstance([/tmp/a /tmp/b]) = true after second path shutting down, want false")
	}
}

func TestClaimOrHandoffRetriesOnShutdown(t *testing.T) {
	shortDataDir(t)
	held, err := openipc.Claim()
	if err != nil {
		t.Fatalf("Claim() error = %v, want nil", err)
	}
	stop := make(chan struct{})
	go func() {
		_ = openipc.ServeListener(stop, held, func(openipc.Request) openipc.Response {
			return openipc.Response{OK: false, Error: openipc.ErrShuttingDown}
		})
	}()
	go func() {
		time.Sleep(80 * time.Millisecond)
		close(stop)
		held.Close()
	}()

	inst, handed, err := claimOrHandoff(nil)
	if err != nil {
		t.Fatalf("claimOrHandoff(nil) error = %v, want nil", err)
	}
	if handed {
		t.Fatal("claimOrHandoff(nil) handed = true after shutdown response, want reclaim")
	}
	if inst == nil {
		t.Fatal("claimOrHandoff(nil) inst = nil, want claimed instance")
	}
	inst.Close()
}

// shortDataDir points Recur at a short-path temp root. Darwin unix-socket
// paths cap at ~104 bytes, and t.TempDir() includes the test name.
func shortDataDir(t *testing.T) {
	t.Helper()
	dir, err := os.MkdirTemp("", "rh")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	t.Setenv("RECUR_DATA_DIR", dir)
}

func waitForSock(t *testing.T, sock string, serveErr <-chan error) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(sock); err == nil {
			return
		}
		select {
		case err := <-serveErr:
			t.Fatalf("Serve returned before listen: %v", err)
		default:
		}
		if time.Now().After(deadline) {
			t.Fatalf("socket was not created (%s, %d bytes)", sock, len(sock))
		}
		time.Sleep(10 * time.Millisecond)
	}
}
