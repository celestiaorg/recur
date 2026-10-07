package main

import (
	"errors"
	"fmt"
	"log"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"

	"recur/internal/app"
	"recur/internal/domain"
	"recur/internal/openipc"
)

// peelOpenArgs pulls `--open PATH` / `--open=PATH` out of argv so Wails does
// not see them as unknown flags. Remaining args are written back to os.Args.
func peelOpenArgs(args []string) (opens []string, rest []string) {
	if len(args) == 0 {
		return nil, nil
	}
	rest = make([]string, 0, len(args))
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "--open":
			if i+1 < len(args) {
				opens = append(opens, args[i+1])
				i++
			}
		case strings.HasPrefix(a, "--open="):
			opens = append(opens, strings.TrimPrefix(a, "--open="))
		default:
			rest = append(rest, a)
		}
	}
	return opens, rest
}

// openHub replays the last OpenPath onto the UI if it happened before the
// frontend subscribed to app.open (cold start via `recur .` / --open).
type openHub struct {
	mu        sync.Mutex
	focusing  sync.WaitGroup
	dead      chan struct{} // closed by dropUI
	uiReady   bool
	appLive   bool
	revoked   bool
	delivered bool
	last      *domain.OpenPathResult
	emit      func(domain.OpenPathResult)
	// focus posts native Show. applied() must run from the main-queue
	// block (or immediately in tests). Hook return is only "posted".
	focus func(applied func())
}

func (h *openHub) note(res domain.OpenPathResult) {
	h.mu.Lock()
	defer h.mu.Unlock()
	cp := res
	h.last = &cp
	if h.uiReady {
		h.delivered = true
	} else {
		h.delivered = false
	}
}

func (h *openHub) uiReadyNow() {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.uiReady = true
	if h.last != nil && !h.delivered && h.emit != nil {
		h.emit(*h.last)
		h.delivered = true
	}
}

func (h *openHub) setHooks(emit func(domain.OpenPathResult), focus func(applied func())) {
	h.mu.Lock()
	h.ensureLocked()
	h.emit = emit
	h.focus = focus
	h.mu.Unlock()
}

func (h *openHub) ensureLocked() {
	if h.dead == nil {
		h.dead = make(chan struct{})
	}
}

func (h *openHub) markAppLive() {
	h.mu.Lock()
	h.appLive = true
	h.mu.Unlock()
}

// dropUI makes later bringForward calls return false. Call from revokeOpenUI
// before Wails destroys the window so a draining handler cannot Show().
func (h *openHub) dropUI() {
	h.mu.Lock()
	h.ensureLocked()
	h.appLive = false
	h.focus = nil
	if !h.revoked {
		h.revoked = true
		close(h.dead)
	}
	h.mu.Unlock()
}

// waitFocus blocks until every admitted focus hook has returned (the
// InvokeAsync post, not the Show callback). Call after revokeOpenUI from
// WindowClosing.
func (h *openHub) waitFocus() {
	h.focusing.Wait()
}

// revokeOpenUI drops focus hooks then stops IPC admission. A later handoff
// gets ErrShuttingDown instead of OK from a no-op bringForward. Safe to
// call more than once (close-last-window and Quit both hit it).
func revokeOpenUI(hub *openHub, gate *ipcGate) {
	if hub != nil {
		hub.dropUI()
	}
	if gate != nil {
		gate.Stop()
	}
}

// bringForward focuses the window. It returns false after dropUI so IPC
// can send ErrShuttingDown instead of OK. True means the focus hook's
// applied() ran and we are not revoked — posting InvokeAsync is not
// enough: macOS applicationShouldTerminate runs OnShutdown on main and
// returns NSTerminateNow without draining dispatch_async. Waiting for
// Show's InvokeSync would pin the IPC handler on main; waitFocus only
// waits until the hook returns (the post), not until applied.
func (h *openHub) bringForward() bool {
	h.mu.Lock()
	h.ensureLocked()
	if h.revoked {
		h.mu.Unlock()
		return false
	}
	fn := h.focus
	dead := h.dead
	if fn == nil || !h.appLive {
		h.mu.Unlock()
		return true
	}
	h.focusing.Add(1)
	h.mu.Unlock()
	applied := make(chan struct{})
	signal := sync.OnceFunc(func() { close(applied) })
	go func() {
		defer h.focusing.Done()
		h.mu.Lock()
		if h.revoked {
			h.mu.Unlock()
			return
		}
		h.mu.Unlock()
		fn(signal)
	}()
	select {
	case <-dead:
		return false
	default:
	}
	select {
	case <-applied:
		h.mu.Lock()
		revoked := h.revoked
		h.mu.Unlock()
		return !revoked
	case <-dead:
		return false
	}
}

// handoffToRunningInstance asks a live Recur to take this launch instead of
// starting a second process. OS notification clicks, Finder re-opens, and
// `open Recur.app --args --open PATH` all enter main(); the CLI already
// hands off via openipc, but the .app itself did not, so a click spawned a
// second window (and a second Recover() against the same store). Empty
// paths mean focus only. Returns true when a running instance accepted the
// request (caller should exit without opening the store).
func handoffToRunningInstance(paths []string) bool {
	sock, err := openipc.SockPath()
	if err != nil {
		return false
	}
	if len(paths) == 0 {
		paths = []string{""}
	}
	for _, p := range paths {
		res, err := openipc.Call(sock, openipc.Request{Path: p})
		if err != nil {
			// DialError: no listener. EOF/timeout: owner may be dying.
			// Either way this path was not delivered — retry Claim rather
			// than exiting (a prior path succeeding is not a full handoff).
			if !openipc.IsDialError(err) {
				log.Printf("handoff: %v", err)
			}
			return false
		}
		if openipc.Retryable(res) {
			log.Printf("handoff: %s", res.Error)
			return false
		}
		if !res.OK && res.Error != "" {
			log.Printf("handoff: %s", res.Error)
		}
	}
	return true
}

// claimRetryFor waits for a dying owner to drain in-flight OpenPath
// (bounded by CallTimeout) and engine.Shutdown (5s) before the flock is
// free. Handoff fails while that owner is quitting, so we retry Claim
// until turnover rather than a Shutdown-only budget.
// simplify: server OpenPath has no deadline; a hung git past CallTimeout
// still loses. Bound ServeListener if that shows up.
const claimRetryFor = openipc.CallTimeout + 8*time.Second

// claimOrHandoff owns this data dir (flock + bound socket) before the store
// opens. If another process already holds it, we hand off; if that owner
// disappears mid-wait, we retry Claim instead of exiting empty-handed.
func claimOrHandoff(paths []string) (inst *openipc.Instance, handed bool, err error) {
	deadline := time.Now().Add(claimRetryFor)
	for {
		inst, err = openipc.Claim()
		if err == nil {
			return inst, false, nil
		}
		if !errors.Is(err, openipc.ErrHeld) {
			return nil, false, err
		}
		if handoffToRunningInstance(paths) {
			return nil, true, nil
		}
		if !time.Now().Before(deadline) {
			return nil, false, fmt.Errorf("another Recur is running this data dir")
		}
		time.Sleep(50 * time.Millisecond)
	}
}

// ipcGate parks openipc requests until Set installs the real handler, so
// Claim can Accept before store.New / Recover without dropping a handoff.
// Stop and admission share one mutex: after Stop returns, Handle will not
// invoke the real handler.
type ipcGate struct {
	mu      sync.Mutex
	cond    *sync.Cond
	stopped bool
	fn      func(openipc.Request) openipc.Response
}

func newIPCGate() *ipcGate {
	g := &ipcGate{}
	g.cond = sync.NewCond(&g.mu)
	return g
}

func (g *ipcGate) Set(fn func(openipc.Request) openipc.Response) {
	g.mu.Lock()
	g.fn = fn
	g.cond.Broadcast()
	g.mu.Unlock()
}

func (g *ipcGate) Stop() {
	g.mu.Lock()
	g.stopped = true
	g.cond.Broadcast()
	g.mu.Unlock()
}

func (g *ipcGate) Handle(req openipc.Request) openipc.Response {
	g.mu.Lock()
	for !g.stopped && g.fn == nil {
		g.cond.Wait()
	}
	if g.stopped {
		g.mu.Unlock()
		return openipc.Response{OK: false, Error: openipc.ErrShuttingDown}
	}
	fn := g.fn
	g.mu.Unlock()
	return fn(req)
}

func startOpenIPC(inst *openipc.Instance, gate *ipcGate, hub *openHub) (stop func()) {
	if inst == nil {
		return func() {}
	}
	done := make(chan struct{})
	finished := make(chan struct{})
	go func() {
		defer close(finished)
		err := openipc.ServeListener(done, inst, gate.Handle)
		if err != nil {
			log.Printf("open ipc: %v", err)
		}
	}()
	return func() {
		revokeOpenUI(hub, gate)
		close(done)
		<-finished
	}
}

func attachOpenIPC(gate *ipcGate, svc *app.Services, hub *openHub) {
	gate.Set(func(req openipc.Request) openipc.Response {
		if strings.TrimSpace(req.Path) == "" {
			if !hub.bringForward() {
				return openipc.Response{OK: false, Error: openipc.ErrShuttingDown}
			}
			return openipc.Response{OK: true}
		}
		res, err := svc.Workspaces.OpenPath(req.Path)
		if err != nil {
			return openipc.Response{OK: false, Error: err.Error()}
		}
		hub.note(res)
		if !hub.bringForward() {
			return openipc.Response{OK: false, Error: openipc.ErrShuttingDown}
		}
		return openipc.Response{
			OK:          true,
			Path:        res.Path,
			ProjectID:   res.ProjectID,
			WorkspaceID: res.WorkspaceID,
			File:        res.File,
			Created:     res.Created,
			Loose:       res.Loose,
		}
	})
}

func applyColdOpens(svc *app.Services, hub *openHub, paths []string) {
	for _, p := range paths {
		p = strings.TrimSpace(p)
		if p == "" {
			continue
		}
		res, err := svc.Workspaces.OpenPath(p)
		if err != nil {
			log.Printf("open %s: %v", p, err)
			continue
		}
		hub.note(res)
	}
}

func wireOpenHub(wapp *application.App, win *application.WebviewWindow, hub *openHub) {
	hub.setHooks(
		func(res domain.OpenPathResult) {
			wapp.Event.Emit("app.open", res)
		},
		func(applied func()) {
			if win == nil {
				applied()
				return
			}
			// Post, don't InvokeSync: OnShutdown already owns main.
			// applied() runs in the block — queueing is not a commit.
			application.InvokeAsync(func() {
				win.UnMinimise()
				win.Show()
				win.Focus()
				applied()
			})
		},
	)
	wapp.Event.OnApplicationEvent(events.Common.ApplicationStarted, func(*application.ApplicationEvent) {
		hub.markAppLive()
	})
	// Notification clicks and Dock re-opens often arrive as HandleReopen
	// rather than a new process. Un-minimise and focus the live window.
	wapp.Event.OnApplicationEvent(events.Mac.ApplicationShouldHandleReopen, func(*application.ApplicationEvent) {
		// Off main: bringForward waits for the Show callback.
		go hub.bringForward()
	})
	wapp.Event.On("app.ready", func(*application.CustomEvent) {
		hub.uiReadyNow()
	})
}

func init() {
	// Strip --open before Wails / flag packages see argv.
	opens, rest := peelOpenArgs(os.Args[1:])
	os.Args = append([]string{os.Args[0]}, rest...)
	coldOpenPaths = opens
}

var coldOpenPaths []string
