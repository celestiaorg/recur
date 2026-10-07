// Package openipc is the unix-socket protocol between the Recur CLI (`recur .`)
// and the running desktop app. The CLI never touches SQLite; it only hands an
// absolute path to the app, which runs OpenPath.
package openipc

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"sync"
	"syscall"
	"time"

	"recur/internal/store"
)

// Request is one line of JSON the CLI writes.
type Request struct {
	Path string `json:"path"` // empty = focus the app only
}

// Response is one line of JSON the app writes back.
type Response struct {
	OK          bool   `json:"ok"`
	Error       string `json:"error,omitempty"`
	Path        string `json:"path,omitempty"`
	ProjectID   string `json:"projectId,omitempty"`
	WorkspaceID string `json:"workspaceId,omitempty"`
	File        string `json:"file,omitempty"`
	Created     bool   `json:"created,omitempty"`
	Loose       bool   `json:"loose,omitempty"`
}

// ErrShuttingDown is Response.Error when this instance is quitting.
// Clients must retry (Claim / Call) rather than treating it as a permanent
// OpenPath failure.
const ErrShuttingDown = "shutting down"

// ErrUnavailable means the peer asked the client to retry (shutting down).
var ErrUnavailable = errors.New("openipc: instance unavailable")

// Retryable reports a decoded response that is not a completed handoff.
func Retryable(res Response) bool {
	return !res.OK && res.Error == ErrShuttingDown
}

// SockPath is the per-data-dir socket ($RECUR_DATA_DIR/open.sock).
func SockPath() (string, error) {
	return store.OpenSockPath()
}

// LockPath is the per-data-dir flock file that serializes which process
// may open the store. Released when the holding *os.File is closed (or
// the process exits).
func LockPath() (string, error) {
	root, err := store.DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(root, "instance.lock"), nil
}

// ErrHeld means another process already owns this data dir.
var ErrHeld = errors.New("openipc: instance already running")

// Instance is the exclusive owner of one data dir: flock + listening socket.
type Instance struct {
	Lock *os.File
	Ln   net.Listener
	Sock string
}

// Close releases the listener and the flock.
func (in *Instance) Close() {
	if in == nil {
		return
	}
	if in.Ln != nil {
		_ = in.Ln.Close()
	}
	if in.Lock != nil {
		_ = in.Lock.Close()
	}
}

// Claim atomically owns this data dir: exclusive flock, then bind open.sock.
// The caller must Close the Instance. ErrHeld means another process won;
// hand off rather than opening the store.
func Claim() (*Instance, error) {
	sock, err := SockPath()
	if err != nil {
		return nil, err
	}
	if socketLive(sock) {
		return nil, ErrHeld
	}
	lock, err := tryLock()
	if err != nil {
		return nil, err
	}
	if socketLive(sock) {
		_ = lock.Close()
		return nil, ErrHeld
	}
	ln, err := listenSock(sock)
	if err != nil {
		_ = lock.Close()
		return nil, err
	}
	return &Instance{Lock: lock, Ln: ln, Sock: sock}, nil
}

func tryLock() (*os.File, error) {
	path, err := LockPath()
	if err != nil {
		return nil, err
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, fmt.Errorf("openipc lock: %w", err)
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		_ = f.Close()
		if errors.Is(err, syscall.EAGAIN) || errors.Is(err, syscall.EWOULDBLOCK) {
			return nil, ErrHeld
		}
		return nil, fmt.Errorf("openipc lock: %w", err)
	}
	return f, nil
}

func listenSock(sock string) (net.Listener, error) {
	_ = os.Remove(sock)
	ln, err := net.Listen("unix", sock)
	if err != nil {
		return nil, fmt.Errorf("openipc listen: %w", err)
	}
	if err := os.Chmod(sock, 0o600); err != nil {
		_ = ln.Close()
		if !socketLive(sock) {
			_ = os.Remove(sock)
		}
		return nil, fmt.Errorf("openipc chmod: %w", err)
	}
	return ln, nil
}

// DialTimeout is how long the CLI waits for a single connect attempt.
const DialTimeout = 400 * time.Millisecond

// CallTimeout is how long a connected Call waits for OpenPath (git budgets
// plus a queued request ahead of this one).
const CallTimeout = 60 * time.Second

// DialError is a failed connect — no live listener. Other Call errors mean
// we reached an app that then failed or timed out; the CLI must not launch
// a second instance in that case.
type DialError struct {
	Err error
}

func (e *DialError) Error() string {
	if e == nil || e.Err == nil {
		return "openipc: dial failed"
	}
	return e.Err.Error()
}

func (e *DialError) Unwrap() error {
	if e == nil {
		return nil
	}
	return e.Err
}

// IsDialError reports a connect failure (no listener / stale socket).
func IsDialError(err error) bool {
	var d *DialError
	return errors.As(err, &d)
}

func socketLive(sock string) bool {
	c, err := net.DialTimeout("unix", sock, 150*time.Millisecond)
	if err != nil {
		return false
	}
	_ = c.Close()
	return true
}

// Serve listens on sock and calls handle for each connection. A live
// listener is left alone (we do not steal it). A stale socket file is
// unlinked first. On stop we only remove the file if nothing else is
// accepting on it.
func Serve(stop <-chan struct{}, sock string, handle func(Request) Response) error {
	if sock == "" {
		return fmt.Errorf("openipc: empty socket path")
	}
	if socketLive(sock) {
		return fmt.Errorf("openipc: already listening on %s", sock)
	}
	ln, err := listenSock(sock)
	if err != nil {
		return err
	}
	return ServeListener(stop, &Instance{Ln: ln, Sock: sock}, handle)
}

// ServeListener accepts on an already-bound Instance (from Claim or Serve).
// On stop it closes the listener, waits for in-flight handlers, then returns.
func ServeListener(stop <-chan struct{}, inst *Instance, handle func(Request) Response) error {
	if inst == nil || inst.Ln == nil {
		return fmt.Errorf("openipc: nil listener")
	}
	ln := inst.Ln
	sock := inst.Sock
	var wg sync.WaitGroup
	go func() {
		<-stop
		_ = ln.Close()
	}()
	for {
		conn, err := ln.Accept()
		if err != nil {
			select {
			case <-stop:
				wg.Wait()
				if sock != "" && !socketLive(sock) {
					_ = os.Remove(sock)
				}
				return nil
			default:
				wg.Wait()
				return err
			}
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			serveConn(conn, handle)
		}()
	}
}

func serveConn(conn net.Conn, handle func(Request) Response) {
	defer conn.Close() //nolint:errcheck
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	dec := json.NewDecoder(conn)
	var req Request
	if err := dec.Decode(&req); err != nil {
		writeResponse(conn, Response{OK: false, Error: "invalid request"})
		return
	}
	// OpenPath can hold git budgets well past 5s; do not time out the handler.
	_ = conn.SetDeadline(time.Time{})
	res := handle(req)
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	writeResponse(conn, res)
}

func writeResponse(w io.Writer, res Response) {
	enc := json.NewEncoder(w)
	_ = enc.Encode(res)
}

// Call sends req to a running app and returns its response.
func Call(sock string, req Request) (Response, error) {
	conn, err := net.DialTimeout("unix", sock, DialTimeout)
	if err != nil {
		return Response{}, &DialError{Err: err}
	}
	defer conn.Close() //nolint:errcheck
	_ = conn.SetDeadline(time.Now().Add(CallTimeout))
	if err := json.NewEncoder(conn).Encode(req); err != nil {
		return Response{}, err
	}
	var res Response
	if err := json.NewDecoder(conn).Decode(&res); err != nil {
		return Response{}, err
	}
	return res, nil
}
