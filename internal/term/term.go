// Package term runs interactive PTY sessions for the Observatory's terminal dock
// and streams their output to the frontend.
//
// The design exists to dodge the reason DOM-hosted terminals are usually slow: it
// is almost never the emulator widget, it is the transport. Three rules carry the
// weight here.
//
//  1. COALESCE. A PTY hands back whatever the kernel has, which under `cat bigfile`
//     is thousands of small reads a second. Emitting one event per read would bury
//     the webview bridge in per-message overhead. Output is instead buffered and
//     flushed on a ~8ms tick (or early once a chunk is worth sending), so the
//     bridge sees ~120 large messages a second at worst.
//
//  2. FLOW CONTROL. The frontend acks bytes once xterm.js has actually parsed them;
//     a session with too many unacked bytes in flight stops reading its PTY, which
//     back-pressures the shell. Without this a fast writer outruns the renderer and
//     the queue grows without bound. Blocking the producer is what a real terminal
//     does, and it is what keeps memory flat.
//
//  3. DETACH ≠ KILL. Closing the drawer or switching workspaces only detaches the
//     client. The shell keeps running, output keeps landing in a bounded replay
//     ring (never in the unbounded pending buffer), and re-attaching repaints from
//     that ring. Sessions die when the user closes them or the app exits; the
//     service layer snapshots tab list, cwd and replay so the next launch can
//     restore them.
package term

import (
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/creack/pty"
	"github.com/google/uuid"
)

const (
	// readChunk is the PTY read buffer. Large enough that a burst costs few syscalls.
	readChunk = 64 << 10
	// flushInterval bounds added latency for a small write (a keystroke echo) at one
	// tick. Below ~8ms the bridge chatter stops paying for itself; above ~16ms typing
	// starts to feel spongy.
	flushInterval = 8 * time.Millisecond
	// maxPending flushes early rather than waiting out the tick, so a burst moves in
	// steady chunks instead of one oversized message.
	maxPending = 64 << 10
	// highWater is the ceiling on unacked bytes (queued + in flight) before a session
	// stops reading its PTY.
	highWater = 256 << 10
	// replayCap is the per-session replay ring size. Large enough that a restart
	// restore still has a useful scrollback, not just the last prompt — roughly
	// a long-ish session of coloured shell output.
	replayCap = 1 << 20
)

// restoreTrailer is appended after a persisted replay when seeding a new PTY so
// a snapshot taken inside an alt-screen program (vim, htop) does not leave the
// restored tab stuck in that screen under a fresh prompt.
var restoreTrailer = []byte("\x1b[?1049l\x1b[0m\x1b[?25h\r\n")

// Emitter publishes an event to the frontend. Injected so this package stays free
// of both Wails and the engine.
type Emitter func(name string, data any)

// Session is one live PTY and the shell running inside it.
type Session struct {
	id           string
	workspaceID  string
	cwd          string
	cwdFromOSC   bool      // OSC 7 reported cwd; skip lsof while this holds
	cwdCheckedAt time.Time // last processCwd refresh
	shell        string
	createdAt    time.Time
	title        string // OSC / dock tab label; empty → basename(shell)

	ptmx *os.File
	cmd  *exec.Cmd
	emit Emitter

	// emitMu serialises "take the pending chunk, then publish it" so two concurrent
	// flushes cannot deliver VT bytes out of order. Held across the emit, and
	// deliberately separate from mu so an in-flight publish never blocks an ack.
	emitMu sync.Mutex

	mu       sync.Mutex
	cond     *sync.Cond // signalled when outstanding drops, or on detach/close
	attached bool
	// outstanding is bytes read from the PTY that the client has not yet acked,
	// counted from the moment they are INGESTED rather than when they are flushed.
	// Counting at flush time leaves the pending buffer outside the budget, so a
	// fast writer can queue an entire flush interval's worth of output — megabytes
	// under `yes` — before the reader ever notices it is over budget.
	outstanding int
	pending     []byte
	replay      *ring
	closed      bool
	exited      bool
	exitedAt    time.Time
	exitCode    int

	wake chan struct{} // buffered(1): nudge the flusher when pending is worth sending
	done chan struct{} // closed once the read loop has exited

	idleKick chan struct{} // buffered(1): reset watchIdle backoff after a Write/Interrupt

	// queries answers OSC 10/11/12 + CSI 6n probes on this side of the PTY so
	// tools like termenv don't race the webview bridge (see queries.go).
	queries *queryFilter
	osc7    *osc7Parser

	onDirty func() // set by Manager; called after replay/cwd/title changes
}

// Info is the frontend-facing view of a session.
type Info struct {
	ID          string `json:"id"`
	WorkspaceID string `json:"workspaceId"`
	Cwd         string `json:"cwd"`
	Shell       string `json:"shell"`
	// Title seeds a fresh tab label. Live titles come from xterm.js's OSC handler on
	// the frontend, which already parses the stream — no need to scan it twice.
	Title string `json:"title"`
	// CreatedAt is RFC3339Nano so same-second opens (restore, rapid ⌘T) still sort.
	CreatedAt string `json:"createdAt"`
	// Exited is true after the shell has died but before the user dismisses the tab.
	// Replay stays readable so @terminal / read_terminal can still attach the last screen.
	Exited bool `json:"exited,omitempty"`
	// ExitCode is the shell's status when Exited is true, including 0 (clean exit).
	ExitCode *int `json:"exitCode,omitempty"`
	// Busy is true when a foreground process other than the login shell owns the tty.
	Busy bool `json:"busy,omitempty"`
	// Active is true for the dock's focused tab. Set on List/Restore so the
	// frontend can reselect the same tab after a restart.
	Active bool `json:"active,omitempty"`
}

func (s *Session) info() Info {
	s.mu.Lock()
	title := s.title
	exited := s.exited
	exitCode := s.exitCode
	cwd := s.cwd
	s.mu.Unlock()
	if title == "" {
		title = filepath.Base(s.shell)
	}
	out := Info{
		ID:          s.id,
		WorkspaceID: s.workspaceID,
		Cwd:         cwd,
		Shell:       s.shell,
		Title:       title,
		CreatedAt:   s.createdAt.UTC().Format(time.RFC3339Nano),
		Exited:      exited,
	}
	if exited {
		c := exitCode
		out.ExitCode = &c
	} else {
		out.Busy = s.isBusy()
	}
	return out
}

func (s *Session) isBusy() bool {
	if s.cmd == nil || s.cmd.Process == nil {
		return false
	}
	fg, ok := s.foregroundPgid()
	return ok && !isShellForeground(fg, s.cmd.Process.Pid)
}

// OpenOptions seeds a session opened to restore a previous tab.
type OpenOptions struct {
	Title  string
	Replay []byte
}

// SessionSnapshot is one tab's durable state: enough to spawn a new shell in the
// same directory and repaint the last screen. Process identity is not kept —
// the PTY dies with the app.
type SessionSnapshot struct {
	ID     string
	Title  string
	Cwd    string
	Replay []byte
}

// Manager owns every live session. One per app.
type Manager struct {
	mu       sync.Mutex
	sessions map[string]*Session
	active   map[string]string   // workspaceID → focused session id
	order    map[string][]string // workspaceID → session ids in tab-strip order
	emit     Emitter
	onDirty  func(workspaceID string)
}

func NewManager(emit Emitter) *Manager {
	if emit == nil {
		emit = func(string, any) {}
	}
	return &Manager{
		sessions: map[string]*Session{},
		active:   map[string]string{},
		order:    map[string][]string{},
		emit:     emit,
	}
}

// SetOnDirty registers a hook fired when a workspace's sessions change in a way
// that should be persisted (open/close/title/cwd/output). The hook must not
// block on Manager locks.
func (m *Manager) SetOnDirty(fn func(workspaceID string)) {
	m.mu.Lock()
	m.onDirty = fn
	m.mu.Unlock()
}

func (m *Manager) dirty(workspaceID string) {
	m.mu.Lock()
	fn := m.onDirty
	m.mu.Unlock()
	if fn != nil {
		fn(workspaceID)
	}
}

// Open starts a shell in cwd and returns its session. cols/rows seed the PTY size
// so the shell's first prompt is already correctly wrapped — starting at the 80x24
// default and resizing after the fact makes the initial paint visibly reflow.
// extraEnv is merged on top of the PTY baseline (workspace RECUR_* + project env).
func (m *Manager) Open(workspaceID, cwd string, cols, rows int, extraEnv []string) (Info, error) {
	return m.OpenWith(workspaceID, cwd, cols, rows, extraEnv, OpenOptions{})
}

// OpenWith is Open plus restore seeds (prior title and replay buffer).
func (m *Manager) OpenWith(workspaceID, cwd string, cols, rows int, extraEnv []string, opts OpenOptions) (Info, error) {
	if cwd == "" {
		return Info{}, fmt.Errorf("terminal: no working directory for workspace %q", workspaceID)
	}
	if fi, err := os.Stat(cwd); err != nil || !fi.IsDir() {
		return Info{}, fmt.Errorf("terminal: working directory %q is unavailable", cwd)
	}
	cols, rows = clampSize(cols, rows)

	shell := loginShell()
	// A login shell (-l) is what Terminal.app and iTerm give you, so the user's
	// PATH, version managers and shell functions are all present. Without it the
	// terminal inherits the GUI app's stunted launchd environment and half the
	// user's tooling appears to be missing.
	cmd := exec.Command(shell, "-l") //nolint:gosec // shell path comes from $SHELL / a fixed fallback
	cmd.Dir = cwd
	cmd.Env = mergeEnv(ptyEnv(), extraEnv)

	ptmx, err := pty.StartWithSize(cmd, &pty.Winsize{Cols: uint16(cols), Rows: uint16(rows)})
	if err != nil {
		return Info{}, fmt.Errorf("terminal: start %s: %w", shell, err)
	}

	title := filepath.Base(shell)
	if strings.TrimSpace(opts.Title) != "" {
		title = strings.TrimSpace(opts.Title)
	}
	s := &Session{
		id:          uuid.Must(uuid.NewV7()).String(),
		workspaceID: workspaceID,
		cwd:         cwd,
		shell:       shell,
		createdAt:   time.Now(),
		title:       title,
		ptmx:        ptmx,
		cmd:         cmd,
		emit:        m.emit,
		replay:      newRing(replayCap),
		wake:        make(chan struct{}, 1),
		done:        make(chan struct{}),
		idleKick:    make(chan struct{}, 1),
		queries:     newQueryFilter(),
		osc7:        &osc7Parser{},
		onDirty:     func() { m.dirty(workspaceID) },
	}
	if len(opts.Replay) > 0 {
		s.replay.write(opts.Replay)
		s.replay.write(restoreTrailer)
	}
	s.cond = sync.NewCond(&s.mu)

	m.mu.Lock()
	m.sessions[s.id] = s
	m.appendOrderLocked(workspaceID, s.id)
	m.mu.Unlock()

	go s.flushLoop()
	go s.watchIdle()
	go func() {
		s.readLoop()
		m.reap(s)
	}()

	m.dirty(workspaceID)
	return s.info(), nil
}

func (m *Manager) appendOrderLocked(workspaceID, id string) {
	for _, existing := range m.order[workspaceID] {
		if existing == id {
			return
		}
	}
	m.order[workspaceID] = append(m.order[workspaceID], id)
}

func (m *Manager) removeOrderLocked(workspaceID, id string) {
	ids := m.order[workspaceID]
	if len(ids) == 0 {
		return
	}
	out := make([]string, 0, len(ids))
	for _, x := range ids {
		if x != id {
			out = append(out, x)
		}
	}
	if len(out) == 0 {
		delete(m.order, workspaceID)
		return
	}
	m.order[workspaceID] = out
}

func (s *Session) listedInfo(activeID string) Info {
	info := s.info()
	if s.id == activeID && !info.Exited {
		info.Active = true
	}
	return info
}

// List returns the workspace's sessions in tab-strip order. New tabs append
// until Reorder moves them. Sessions missing from the saved order (shouldn't
// happen outside tests) follow by createdAt.
func (m *Manager) List(workspaceID string) []Info {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.listLocked(workspaceID)
}

func (m *Manager) listLocked(workspaceID string) []Info {
	activeID := m.active[workspaceID]
	byID := make(map[string]*Session)
	for _, s := range m.sessions {
		if s.workspaceID == workspaceID {
			byID[s.id] = s
		}
	}
	out := make([]Info, 0, len(byID))
	seen := make(map[string]bool, len(byID))
	for _, id := range m.order[workspaceID] {
		s, ok := byID[id]
		if !ok {
			continue
		}
		seen[id] = true
		out = append(out, s.listedInfo(activeID))
	}
	var extra []*Session
	for id, s := range byID {
		if !seen[id] {
			extra = append(extra, s)
		}
	}
	sort.Slice(extra, func(i, j int) bool {
		if !extra[i].createdAt.Equal(extra[j].createdAt) {
			return extra[i].createdAt.Before(extra[j].createdAt)
		}
		return extra[i].id < extra[j].id
	})
	for _, s := range extra {
		out = append(out, s.listedInfo(activeID))
	}
	return out
}

// Reorder persists a new tab-strip order. ids must be a permutation of the
// workspace's current session ids (including exited tombstones still listed).
func (m *Manager) Reorder(workspaceID string, ids []string) error {
	m.mu.Lock()
	live := m.listLocked(workspaceID)
	if err := sessionIDsPermutation(live, ids); err != nil {
		m.mu.Unlock()
		return err
	}
	same := true
	for i, info := range live {
		if info.ID != ids[i] {
			same = false
			break
		}
	}
	if same {
		m.mu.Unlock()
		return nil
	}
	if m.order == nil {
		m.order = map[string][]string{}
	}
	m.order[workspaceID] = append([]string(nil), ids...)
	m.mu.Unlock()
	m.dirty(workspaceID)
	return nil
}

func sessionIDsPermutation(live []Info, ids []string) error {
	if len(ids) != len(live) {
		return fmt.Errorf("reorder terminals: got %d ids, have %d sessions", len(ids), len(live))
	}
	have := make(map[string]bool, len(live))
	for _, info := range live {
		have[info.ID] = true
	}
	seen := make(map[string]bool, len(ids))
	for _, id := range ids {
		if !have[id] {
			return fmt.Errorf("reorder terminals: unknown session %q", id)
		}
		if seen[id] {
			return fmt.Errorf("reorder terminals: duplicate session %q", id)
		}
		seen[id] = true
	}
	return nil
}

// Snapshots returns live (non-exited) tabs in List order, with cwd refreshed
// from the shell process when possible.
func (m *Manager) Snapshots(workspaceID string) []SessionSnapshot {
	infos := m.List(workspaceID)
	out := make([]SessionSnapshot, 0, len(infos))
	for _, info := range infos {
		if info.Exited {
			continue
		}
		s, err := m.get(info.ID)
		if err != nil {
			continue
		}
		s.refreshCwd()
		s.mu.Lock()
		title := s.title
		cwd := s.cwd
		replay := s.replay.snapshot()
		s.mu.Unlock()
		if title == "" {
			title = filepath.Base(s.shell)
		}
		out = append(out, SessionSnapshot{ID: s.id, Title: title, Cwd: cwd, Replay: replay})
	}
	return out
}

func (m *Manager) get(id string) (*Session, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	s, ok := m.sessions[id]
	if !ok {
		return nil, fmt.Errorf("terminal: session %q not found", id)
	}
	return s, nil
}

// Attach marks a client as live and returns the replay buffer to repaint from.
func (m *Manager) Attach(id string) (string, error) {
	s, err := m.get(id)
	if err != nil {
		return "", err
	}
	s.mu.Lock()
	s.attached = true
	// A new client has an empty screen and owes nothing for what the last one saw.
	s.outstanding = 0
	s.pending = nil
	snap := s.replay.snapshot()
	s.mu.Unlock()
	s.cond.Broadcast()
	return base64.StdEncoding.EncodeToString(snap), nil
}

// Detach stops streaming without touching the shell. Output continues into the
// replay ring, and the read loop is released from flow control — a detached
// session must never hold its PTY blocked waiting for acks that aren't coming.
func (m *Manager) Detach(id string) error {
	s, err := m.get(id)
	if err != nil {
		return err
	}
	s.mu.Lock()
	s.attached = false
	s.outstanding = 0
	s.pending = nil
	s.mu.Unlock()
	s.cond.Broadcast()
	return nil
}

// Ack reports that the frontend has parsed n more bytes, releasing flow control.
func (m *Manager) Ack(id string, n int) error {
	s, err := m.get(id)
	if err != nil {
		return err
	}
	s.mu.Lock()
	s.outstanding -= n
	if s.outstanding < 0 {
		s.outstanding = 0
	}
	s.mu.Unlock()
	s.cond.Broadcast()
	return nil
}

// Write forwards keystrokes to the shell.
func (m *Manager) Write(id, data string) error {
	s, err := m.get(id)
	if err != nil {
		return err
	}
	if _, err := s.ptmx.Write([]byte(data)); err != nil {
		return fmt.Errorf("terminal: write: %w", err)
	}
	s.kickIdleWatch()
	return nil
}

// Resize updates the PTY window size, which is what makes SIGWINCH-aware programs
// (vim, less, htop) redraw at the new geometry.
func (m *Manager) Resize(id string, cols, rows int) error {
	s, err := m.get(id)
	if err != nil {
		return err
	}
	cols, rows = clampSize(cols, rows)
	if err := pty.Setsize(s.ptmx, &pty.Winsize{Cols: uint16(cols), Rows: uint16(rows)}); err != nil {
		return fmt.Errorf("terminal: resize: %w", err)
	}
	return nil
}

// SetTitle records the live OSC / dock tab label so @terminal chips and MCP
// list_terminals can name a session the way the human sees it.
func (m *Manager) SetTitle(id, title string) error {
	s, err := m.get(id)
	if err != nil {
		return err
	}
	title = strings.TrimSpace(title)
	if title == "" {
		return nil
	}
	s.mu.Lock()
	s.title = title
	s.mu.Unlock()
	m.dirty(s.workspaceID)
	return nil
}

// SetActive records which session the dock is showing for workspaceID. Bare
// @terminal and read_terminal with no id resolve to this tab. id == "" clears.
func (m *Manager) SetActive(workspaceID, id string) {
	if workspaceID == "" {
		return
	}
	m.mu.Lock()
	if id == "" {
		delete(m.active, workspaceID)
	} else {
		m.active[workspaceID] = id
	}
	m.mu.Unlock()
	m.dirty(workspaceID)
}

// ActiveID is the focused session for workspaceID, or "".
func (m *Manager) ActiveID(workspaceID string) string {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.active[workspaceID]
}

// ReadPlain returns ANSI-stripped recent output for a session (last maxLines).
func (m *Manager) ReadPlain(id string, maxLines int) (string, error) {
	s, err := m.get(id)
	if err != nil {
		return "", err
	}
	s.mu.Lock()
	snap := s.replay.snapshot()
	s.mu.Unlock()
	return TailPlain(snap, maxLines), nil
}

// ClearReplay drops the replay buffer. Called when the user clears the terminal so
// re-attaching doesn't bring back the output they just wiped.
func (m *Manager) ClearReplay(id string) error {
	s, err := m.get(id)
	if err != nil {
		return err
	}
	s.mu.Lock()
	s.replay.reset()
	s.mu.Unlock()
	m.dirty(s.workspaceID)
	return nil
}

// Close terminates a session's process tree.
func (m *Manager) Close(id string) error {
	s, err := m.get(id)
	if err != nil {
		return err
	}
	go killAfterGrace(s.terminate())
	m.drop(s)
	m.dirty(s.workspaceID)
	return nil
}

// CloseAll tears down every session. Called on app shutdown so quitting Recur
// doesn't strand orphaned shells (and whatever they were running) behind it.
func (m *Manager) CloseAll() {
	m.mu.Lock()
	all := make([]*Session, 0, len(m.sessions))
	for _, s := range m.sessions {
		all = append(all, s)
	}
	m.sessions = map[string]*Session{}
	m.active = map[string]string{}
	m.order = map[string][]string{}
	m.mu.Unlock()
	var groups []int
	for _, s := range all {
		groups = append(groups, s.terminate()...)
	}
	// Synchronous: the app exits right after shutdown, before a goroutine's grace
	// period would elapse.
	killAfterGrace(groups)
}

func (m *Manager) drop(s *Session) {
	m.mu.Lock()
	delete(m.sessions, s.id)
	if m.active[s.workspaceID] == s.id {
		delete(m.active, s.workspaceID)
	}
	m.removeOrderLocked(s.workspaceID, s.id)
	m.mu.Unlock()
}

// reap runs once a session's PTY has closed: it flushes whatever the shell wrote on
// its way out and reports the exit status. The session stays registered as an
// exited tombstone so the last screen can still be attached until Close.
func (m *Manager) reap(s *Session) {
	s.flush() // the read loop is done, so this is the last chunk by construction

	var code int
	if err := s.cmd.Wait(); err != nil {
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			code = ee.ExitCode()
		} else {
			code = -1
		}
	}

	s.mu.Lock()
	s.exited = true
	s.exitedAt = time.Now()
	s.exitCode = code
	s.closed = true
	s.mu.Unlock()
	s.cond.Broadcast()

	// The shell is already gone; release the PTY master now. terminate() no-ops
	// once closed is set, so this is the only close on a self-exited session.
	_ = s.ptmx.Close()

	// Prune before term.exit so listeners that List() on that event see the cap.
	m.pruneTombstones(s.workspaceID)
	s.emit("term.exit", map[string]any{"id": s.id, "workspaceId": s.workspaceID, "exitCode": code})
	m.dirty(s.workspaceID)
}

const maxExitedTombstones = 4

// pruneTombstones drops the oldest-exited sessions in a workspace past the cap
// so a day of `exit`/crashes cannot hold unbounded rings and fds. Eviction is
// by exit time, not open time: a long-lived first tab that just died is the
// freshest tombstone and stays.
func (m *Manager) pruneTombstones(workspaceID string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var dead []*Session
	for _, s := range m.sessions {
		if s.workspaceID != workspaceID {
			continue
		}
		s.mu.Lock()
		exited := s.exited
		s.mu.Unlock()
		if exited {
			dead = append(dead, s)
		}
	}
	if len(dead) <= maxExitedTombstones {
		return
	}
	sort.Slice(dead, func(i, j int) bool { return dead[i].exitedAt.Before(dead[j].exitedAt) })
	for _, s := range dead[:len(dead)-maxExitedTombstones] {
		delete(m.sessions, s.id)
		if m.active[s.workspaceID] == s.id {
			delete(m.active, s.workspaceID)
		}
		m.removeOrderLocked(s.workspaceID, s.id)
	}
}

// ---- session internals ----

func (s *Session) readLoop() {
	defer close(s.done)
	buf := make([]byte, readChunk)
	for {
		n, err := s.ptmx.Read(buf)
		if n > 0 {
			s.ingest(buf[:n])
		}
		if err != nil {
			// EIO on a closed PTY is the normal "shell exited" path on macOS.
			return
		}
		s.awaitCredit()
	}
}

// ingest records output for replay and, when a client is attached, queues it for
// the next flush. A detached session buffers nothing beyond the bounded ring.
//
// Colour/CPR probes are answered synchronously on the PTY and stripped before
// anything reaches xterm — see queryFilter.
func (s *Session) ingest(p []byte) {
	forward, replies := s.queries.process(p)
	if len(replies) > 0 {
		// Best-effort: a failed write means the PTY is already gone, and the
		// read loop is about to exit anyway.
		_, _ = s.ptmx.Write(replies)
	}
	if len(forward) == 0 {
		return
	}

	var cwd string
	cwdOK := false
	if s.osc7 != nil {
		cwd, cwdOK = s.osc7.feed(forward)
		if cwdOK && !dirExists(cwd) {
			cwdOK = false
		}
	}

	s.mu.Lock()
	if cwdOK {
		s.cwd = cwd
		s.cwdFromOSC = true
	}
	s.replay.write(forward)
	dirty := s.onDirty
	if !s.attached {
		s.mu.Unlock()
		if dirty != nil {
			dirty()
		}
		return
	}
	s.pending = append(s.pending, forward...)
	s.outstanding += len(forward)
	big := len(s.pending) >= maxPending
	s.mu.Unlock()

	if dirty != nil {
		dirty()
	}
	if big {
		select {
		case s.wake <- struct{}{}:
		default: // a flush is already pending; nothing to add
		}
	}
}

const cwdRefreshMin = 2 * time.Second

func (s *Session) refreshCwd() {
	s.mu.Lock()
	fromOSC := s.cwdFromOSC
	cwd := s.cwd
	checked := s.cwdCheckedAt
	s.mu.Unlock()
	if fromOSC && dirExists(cwd) {
		return
	}
	if !checked.IsZero() && time.Since(checked) < cwdRefreshMin {
		return
	}

	var pid int
	if s.cmd != nil && s.cmd.Process != nil {
		pid = s.cmd.Process.Pid
	}
	dir := processCwd(pid)
	ok := dirExists(dir)
	s.mu.Lock()
	s.cwdCheckedAt = time.Now()
	if ok {
		s.cwd = dir
	}
	s.mu.Unlock()
}

func dirExists(p string) bool {
	if p == "" {
		return false
	}
	fi, err := os.Stat(p)
	return err == nil && fi.IsDir()
}

// awaitCredit blocks the read loop while the frontend is too far behind, which
// back-pressures the PTY and, in turn, the shell.
func (s *Session) awaitCredit() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for s.attached && !s.closed && s.outstanding > highWater {
		s.cond.Wait()
	}
}

func (s *Session) flushLoop() {
	t := time.NewTicker(flushInterval)
	defer t.Stop()
	for {
		select {
		case <-t.C:
			s.flush()
		case <-s.wake:
			s.flush()
		case <-s.done:
			// The final flush is reap's job, so it is ordered before term.exit.
			return
		}
	}
}

// flush publishes one coalesced chunk. It does not touch the flow-control budget —
// these bytes were charged at ingest and are only forgiven when the frontend acks.
func (s *Session) flush() {
	s.emitMu.Lock()
	defer s.emitMu.Unlock()

	s.mu.Lock()
	if !s.attached || len(s.pending) == 0 {
		s.mu.Unlock()
		return
	}
	chunk := s.pending
	s.pending = nil
	s.mu.Unlock()

	s.emit("term.data", map[string]any{
		"id":   s.id,
		"data": base64.StdEncoding.EncodeToString(chunk),
	})
}

// terminate SIGHUPs the shell's process group, then closes the PTY, and returns
// the groups the caller must SIGKILL via killAfterGrace. SIGHUP first is what a
// closing terminal window sends, so shells and their children get their normal
// cleanup; SIGKILL is the backstop for anything that ignores it.
//
// Signalling the shell's group alone is not enough: the foreground job has its
// own pgrp, and tools like `wails3 dev` spawn children in yet another. go-task
// and wails3 survive the hangup, so the foreground job's tree gets the same
// treatment. Background jobs are left to the shell's own hangup handling, so
// `nohup cmd &` and disowned jobs outlive the tab as in any terminal.
func (s *Session) terminate() []int {
	fg, _ := s.foregroundPgid()
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return nil
	}
	s.closed = true
	s.mu.Unlock()
	s.cond.Broadcast() // release a read loop parked in awaitCredit

	var groups []int
	if s.cmd.Process != nil {
		groups = treeGroups(s.cmd.Process.Pid, fg)
		for _, g := range groups {
			_ = syscall.Kill(-g, syscall.SIGHUP)
		}
	}
	// Closing the PTY unblocks the read loop, which drives reap → term.exit.
	_ = s.ptmx.Close()
	return groups
}

func killAfterGrace(groups []int) {
	if len(groups) == 0 {
		return
	}
	time.Sleep(250 * time.Millisecond)
	for _, g := range groups {
		_ = syscall.Kill(-g, syscall.SIGKILL)
	}
}

// ---- helpers ----

// treeGroups returns the shell's group (root), the foreground group fg, and the
// groups of every descendant of fg's members. Never includes Recur's group. If ps
// fails it falls back to the root and fg groups.
func treeGroups(root, fg int) []int {
	groups := []int{root}
	recur := syscall.Getpgrp()
	if isShellForeground(fg, root) || fg == recur {
		return groups
	}
	groups = append(groups, fg)
	out, err := exec.Command("ps", "-axo", "pid=,ppid=,pgid=").Output()
	if err != nil {
		return groups
	}
	children := map[int][]int{}
	pgidOf := map[int]int{}
	var queue []int
	for _, line := range strings.Split(string(out), "\n") {
		var pid, ppid, pgid int
		if n, _ := fmt.Sscan(line, &pid, &ppid, &pgid); n == 3 {
			children[ppid] = append(children[ppid], pid)
			pgidOf[pid] = pgid
			if pgid == fg {
				queue = append(queue, pid)
			}
		}
	}
	seen := map[int]bool{root: true, fg: true, recur: true}
	for ; len(queue) > 0; queue = queue[1:] {
		pid := queue[0]
		if g := pgidOf[pid]; g > 1 && !seen[g] {
			seen[g] = true
			groups = append(groups, g)
		}
		queue = append(queue, children[pid]...)
	}
	return groups
}

func clampSize(cols, rows int) (int, int) {
	if cols < 2 {
		cols = 80
	}
	if rows < 1 {
		rows = 24
	}
	// Winsize fields are uint16; a bogus value from a mid-layout measurement must
	// not wrap around into a tiny window.
	if cols > 1000 {
		cols = 1000
	}
	if rows > 1000 {
		rows = 1000
	}
	return cols, rows
}

func loginShell() string {
	if sh := os.Getenv("SHELL"); sh != "" {
		return sh
	}
	return "/bin/zsh"
}

// ptyEnv is the app environment plus the terminal capability advertisement. TERM
// must be set explicitly: a GUI app launched from Finder has no TERM at all, and
// programs fall back to dumb-terminal output when they can't find one.
func ptyEnv() []string {
	env := os.Environ()
	// COLORFGBG: light-on-dark fallback for libraries that honour it when an OSC
	// probe fails. We also answer OSC 10/11 ourselves (queries.go); this is belt
	// and suspenders for tools that skip the probe entirely.
	drop := map[string]bool{
		"TERM": true, "COLORTERM": true, "TERM_PROGRAM": true,
		"COLORFGBG": true, "LINES": true, "COLUMNS": true,
	}
	out := make([]string, 0, len(env)+4)
	for _, kv := range env {
		if i := strings.IndexByte(kv, '='); i > 0 && drop[kv[:i]] {
			continue
		}
		out = append(out, kv)
	}
	return append(out,
		"TERM=xterm-256color",
		"COLORTERM=truecolor",
		"TERM_PROGRAM=Recur",
		"COLORFGBG=15;0",
	)
}

// mergeEnv overlays extra KEY=VALUE pairs onto base, replacing matching keys.
func mergeEnv(base, extra []string) []string {
	if len(extra) == 0 {
		return base
	}
	drop := map[string]bool{}
	for _, kv := range extra {
		if i := strings.IndexByte(kv, '='); i > 0 {
			drop[kv[:i]] = true
		}
	}
	out := make([]string, 0, len(base)+len(extra))
	for _, kv := range base {
		if i := strings.IndexByte(kv, '='); i > 0 && drop[kv[:i]] {
			continue
		}
		out = append(out, kv)
	}
	return append(out, extra...)
}
