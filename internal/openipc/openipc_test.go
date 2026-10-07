package openipc

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestCallRoundTrip(t *testing.T) {
	shortDataDir(t)
	sock, err := SockPath()
	if err != nil {
		t.Fatal(err)
	}
	stop := make(chan struct{})
	defer close(stop)
	go func() {
		_ = Serve(stop, sock, func(req Request) Response {
			return Response{OK: true, Path: req.Path, WorkspaceID: "w"}
		})
	}()
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(sock); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("Serve did not create the socket")
		}
		time.Sleep(10 * time.Millisecond)
	}

	res, err := Call(sock, Request{Path: "/tmp/x"})
	if err != nil {
		t.Fatalf("Call: %v", err)
	}
	if !res.OK || res.Path != "/tmp/x" || res.WorkspaceID != "w" {
		t.Fatalf("Call response = %+v, want ok path=/tmp/x workspace=w", res)
	}
}

func TestServeDoesNotStealLiveSocket(t *testing.T) {
	// Darwin unix-socket paths cap at ~104 bytes; t.TempDir()+test name is too long.
	dir := t.TempDir()
	sock := filepath.Join(dir, "s")
	stop1 := make(chan struct{})
	defer close(stop1)
	serveErr := make(chan error, 1)
	go func() {
		serveErr <- Serve(stop1, sock, func(Request) Response {
			return Response{OK: true, WorkspaceID: "first"}
		})
	}()
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(sock); err == nil {
			break
		}
		select {
		case err := <-serveErr:
			t.Fatalf("first Serve returned before listen: %v", err)
		default:
		}
		if time.Now().After(deadline) {
			t.Fatal("first Serve did not create the socket")
		}
		time.Sleep(10 * time.Millisecond)
	}

	err := Serve(make(chan struct{}), sock, func(Request) Response {
		return Response{OK: true, WorkspaceID: "second"}
	})
	if err == nil {
		t.Fatal("Serve(second) error = nil, want already listening")
	}

	res, err := Call(sock, Request{})
	if err != nil {
		t.Fatalf("Call after second Serve: %v", err)
	}
	if res.WorkspaceID != "first" {
		t.Fatalf("Call WorkspaceID = %q, want first", res.WorkspaceID)
	}
}

func TestCallDialError(t *testing.T) {
	err := &DialError{Err: os.ErrNotExist}
	if !IsDialError(err) {
		t.Fatal("IsDialError(DialError) = false, want true")
	}
	if IsDialError(os.ErrNotExist) {
		t.Fatal("IsDialError(plain) = true, want false")
	}
}

func TestRetryable(t *testing.T) {
	if !Retryable(Response{OK: false, Error: ErrShuttingDown}) {
		t.Fatal("Retryable(shutting down) = false, want true")
	}
	if Retryable(Response{OK: true}) {
		t.Fatal("Retryable(OK) = true, want false")
	}
	if Retryable(Response{OK: false, Error: "not a git repo"}) {
		t.Fatal("Retryable(permanent) = true, want false")
	}
}

func TestClaimSecondGetsErrHeld(t *testing.T) {
	shortDataDir(t)
	a, err := Claim()
	if err != nil {
		t.Fatalf("Claim() first error = %v, want nil", err)
	}
	t.Cleanup(a.Close)

	b, err := Claim()
	if !errors.Is(err, ErrHeld) {
		t.Fatalf("Claim() second error = %v, want ErrHeld", err)
	}
	if b != nil {
		t.Fatal("Claim() second instance != nil, want nil")
	}

	a.Close()
	c, err := Claim()
	if err != nil {
		t.Fatalf("Claim() after release error = %v, want nil", err)
	}
	c.Close()
}

func TestClaimConcurrent(t *testing.T) {
	shortDataDir(t)
	type result struct {
		inst *Instance
		err  error
	}
	ch := make(chan result, 2)
	for range 2 {
		go func() {
			inst, err := Claim()
			ch <- result{inst, err}
		}()
	}
	got := []result{<-ch, <-ch}
	winners := 0
	for _, r := range got {
		switch {
		case r.err == nil:
			winners++
			if r.inst != nil {
				r.inst.Close()
			}
		case !errors.Is(r.err, ErrHeld):
			t.Fatalf("Claim() concurrent error = %v, want nil or ErrHeld", r.err)
		}
	}
	if winners != 1 {
		t.Fatalf("Claim() concurrent winners = %d, want 1", winners)
	}
}

func TestClaimServeListenerRoundTrip(t *testing.T) {
	shortDataDir(t)
	inst, err := Claim()
	if err != nil {
		t.Fatalf("Claim() error = %v, want nil", err)
	}
	t.Cleanup(inst.Close)
	stop := make(chan struct{})
	t.Cleanup(func() { close(stop) })
	go func() {
		_ = ServeListener(stop, inst, func(req Request) Response {
			return Response{OK: true, Path: req.Path, WorkspaceID: "w"}
		})
	}()

	res, err := Call(inst.Sock, Request{Path: "/tmp/x"})
	if err != nil {
		t.Fatalf("Call after Claim: %v", err)
	}
	if !res.OK || res.Path != "/tmp/x" || res.WorkspaceID != "w" {
		t.Fatalf("Call response = %+v, want ok path=/tmp/x workspace=w", res)
	}
}

func TestServeListenerStopWaitsForHandler(t *testing.T) {
	shortDataDir(t)
	inst, err := Claim()
	if err != nil {
		t.Fatalf("Claim() error = %v, want nil", err)
	}
	t.Cleanup(inst.Close)

	started := make(chan struct{})
	release := make(chan struct{})
	stop := make(chan struct{})
	done := make(chan error, 1)
	go func() {
		done <- ServeListener(stop, inst, func(Request) Response {
			close(started)
			<-release
			return Response{OK: true}
		})
	}()
	go func() { _, _ = Call(inst.Sock, Request{}) }()
	select {
	case <-started:
	case <-time.After(2 * time.Second):
		t.Fatal("handler did not start")
	}
	close(stop)
	select {
	case err := <-done:
		t.Fatalf("ServeListener returned before handler finished: %v", err)
	case <-time.After(80 * time.Millisecond):
	}
	close(release)
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("ServeListener stop error = %v, want nil", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("ServeListener did not return after handler finished")
	}
}

func shortDataDir(t *testing.T) {
	t.Helper()
	dir, err := os.MkdirTemp("", "oi")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	t.Setenv("RECUR_DATA_DIR", dir)
}
