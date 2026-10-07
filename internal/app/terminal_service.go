package app

import (
	"errors"
	"fmt"
	"log"
	"os"
	"sync"
	"time"

	"recur/internal/code"
	"recur/internal/term"
	"recur/internal/wsenv"
)

// TerminalService backs the Observatory's terminal dock: real shells running in
// the workspace's worktree, one PTY per tab.
//
// The service is a thin shell over term.Manager — it resolves a workspace to a
// directory and otherwise forwards. The interesting behaviour (output coalescing,
// ack-based flow control, replay on re-attach) lives in the term package, which is
// deliberately free of any Wails or engine dependency.
//
// The frontend contract, per tab:
//
//	OpenTerminal      -> session id
//	RestoreWorkspaceTerminals -> prior tabs after an app restart (or empty)
//	ReorderTerminals  -> persist a new tab-strip order
//	AttachTerminal    -> base64 replay to repaint from; output starts streaming
//	  ...term.data events...   AckTerminal(n) after each is parsed
//	DetachTerminal      -> streaming stops, shell keeps running
//	InterruptTerminal   -> foreground group dies; shell stays
//	CloseTerminal       -> shell dies
//	term.busy / term.idle  -> a command took / left the foreground (shell stays)
//
// Acking is not optional. A session stops reading its PTY once too many unacked
// bytes are outstanding, so a client that streams without acking will wedge its
// own shell after the first few hundred KB.
type TerminalService struct {
	base
	mgr *term.Manager

	persistMu     sync.Mutex
	persistDirty  map[string]bool
	persistTimer  *time.Timer
	persistFirst  time.Time
	persistClosed bool
	restoreMu     sync.Mutex
	chrome        map[string]terminalDockChrome
}

// OpenTerminal starts a shell for the workspace and returns the new session.
//
// The shell runs in the workspace's git worktree, so a terminal opened next to a
// running loop is already sitting on that loop's branch and changes — which is the
// whole point of putting one here. Workspaces with an empty WorktreePath
// (archived, or not yet created) fall back to the project checkout. A recorded
// path whose directory is gone is an error — a shell in the checkout would
// target the wrong branch.
func (s *TerminalService) OpenTerminal(workspaceID string, cols, rows int) (term.Info, error) {
	cwd, err := s.terminalCwd(workspaceID)
	if err != nil {
		return term.Info{}, err
	}
	return s.mgr.Open(workspaceID, cwd, cols, rows, s.terminalEnv(workspaceID))
}

// terminalEnv returns workspace RECUR_* vars plus the project's Env map for
// injection into Observatory shells.
func (s *TerminalService) terminalEnv(workspaceID string) []string {
	ws, ok := s.st().Workspace(workspaceID)
	if !ok {
		return nil
	}
	proj, _ := s.st().Project(ws.ProjectID)
	if ws.WorktreePath == "" {
		ws.WorktreePath, _ = s.terminalCwd(workspaceID)
	}
	return wsenv.FromWorkspace(ws, proj).Environ(os.Environ())
}

// ListTerminals returns the workspace's live sessions in tab order. The dock calls
// this on mount to re-adopt shells that outlived a navigation.
func (s *TerminalService) ListTerminals(workspaceID string) []term.Info {
	return s.mgr.List(workspaceID)
}

// ReorderTerminals persists a new dock tab-strip order. ids must be a
// permutation of the workspace's current session ids.
func (s *TerminalService) ReorderTerminals(workspaceID string, ids []string) error {
	return s.mgr.Reorder(workspaceID, ids)
}

// RestoreWorkspaceTerminals respawns the workspace's last live tabs after an
// app restart: same titles and working directories, with the persisted replay
// buffer painted before the new shell's prompt. No-op when sessions are already
// running (drawer remount) or nothing was saved. Does not spawn a default tab —
// the dock does that when restore returns empty.
func (s *TerminalService) RestoreWorkspaceTerminals(workspaceID string, cols, rows int) ([]term.Info, error) {
	s.restoreMu.Lock()
	defer s.restoreMu.Unlock()
	if live := s.mgr.List(workspaceID); len(live) > 0 {
		return live, nil
	}
	snap := readTerminalSnapshot(workspaceID)
	if snap == nil || len(snap.Tabs) == 0 {
		return []term.Info{}, nil
	}
	// Resolve the workspace folder only for tabs whose own cwd is gone.
	// A recorded-but-missing worktree must not abort the whole restore —
	// tabs that `cd`'d elsewhere (or pointed at a sibling path) are still usable.
	var fallback string
	var fallbackErr error
	fallbackOnce := false
	workspaceDir := func() (string, error) {
		if !fallbackOnce {
			fallbackOnce = true
			fallback, fallbackErr = s.terminalCwd(workspaceID)
		}
		return fallback, fallbackErr
	}
	env := s.terminalEnv(workspaceID)
	out := make([]term.Info, 0, len(snap.Tabs))
	var activeID string
	for i, tab := range snap.Tabs {
		cwd := tab.Cwd
		if !dirExists(cwd) {
			next, err := workspaceDir()
			if err != nil {
				log.Printf("terminal: restore %s tab %q: %v", workspaceID, tab.Title, err)
				continue
			}
			cwd = next
		}
		info, err := s.mgr.OpenWith(workspaceID, cwd, cols, rows, env, term.OpenOptions{
			Title:  tab.Title,
			Replay: tab.Replay,
		})
		if err != nil {
			log.Printf("terminal: restore %s tab %q: %v", workspaceID, tab.Title, err)
			continue
		}
		if i == snap.Active {
			activeID = info.ID
		}
		out = append(out, info)
	}
	if len(out) == 0 {
		return []term.Info{}, nil
	}
	if activeID == "" {
		activeID = out[0].ID
	}
	s.mgr.SetActive(workspaceID, activeID)
	for i := range out {
		out[i].Active = out[i].ID == activeID
	}
	return out, nil
}

// SetTerminalTitle records the OSC / dock tab label for a session so chat
// @terminal chips and MCP list_terminals show the name the human sees.
func (s *TerminalService) SetTerminalTitle(id, title string) error {
	return s.mgr.SetTitle(id, title)
}

// SetActiveTerminal records which dock tab is focused for workspaceID. Bare
// @terminal and read_terminal with no id resolve to this session.
func (s *TerminalService) SetActiveTerminal(workspaceID, id string) {
	s.mgr.SetActive(workspaceID, id)
}

// AttachTerminal starts streaming and returns the replay buffer (base64 raw VT) so
// the client can repaint the screen it missed.
func (s *TerminalService) AttachTerminal(id string) (string, error) {
	return s.mgr.Attach(id)
}

// DetachTerminal stops streaming without killing the shell.
func (s *TerminalService) DetachTerminal(id string) error {
	return s.mgr.Detach(id)
}

// AckTerminal reports that n more bytes have been parsed by the renderer, which is
// what releases flow control.
func (s *TerminalService) AckTerminal(id string, n int) error {
	return s.mgr.Ack(id, n)
}

// WriteTerminal forwards keystrokes to the shell.
func (s *TerminalService) WriteTerminal(id, data string) error {
	return s.mgr.Write(id, data)
}

// ResizeTerminal updates the PTY window size so full-screen programs redraw.
func (s *TerminalService) ResizeTerminal(id string, cols, rows int) error {
	return s.mgr.Resize(id, cols, rows)
}

// ClearTerminal drops the replay buffer after the user clears the screen, so
// re-attaching doesn't bring back the output they just wiped.
func (s *TerminalService) ClearTerminal(id string) error {
	return s.mgr.ClearReplay(id)
}

// InterruptTerminal stops the session's foreground process (the running
// dev server) without killing the shell, so the tab stays and can be reused.
func (s *TerminalService) InterruptTerminal(id string) error {
	return s.mgr.Interrupt(id)
}

// CloseTerminal terminates the session's process tree.
func (s *TerminalService) CloseTerminal(id string) error {
	return s.mgr.Close(id)
}

// ServiceShutdown is Wails' app-teardown hook (it is excluded from the generated
// bindings, so this is not reachable from the frontend). Every session's process
// group is signalled here for the same reason the engine joins its loop goroutines
// on quit (D24): a terminal can be sitting on a long-running dev server, and
// quitting Recur must not leave it orphaned and holding its port.
func (s *TerminalService) ServiceShutdown() error {
	s.persistMu.Lock()
	s.persistClosed = true
	if s.persistTimer != nil {
		s.persistTimer.Stop()
		s.persistTimer = nil
	}
	s.persistMu.Unlock()
	s.flushPersist()
	s.mgr.CloseAll()
	return nil
}

func dirExists(p string) bool {
	if p == "" {
		return false
	}
	fi, err := os.Stat(p)
	return err == nil && fi.IsDir()
}

// terminalCwd resolves the directory a workspace's shells should start in, using
// the same worktree-then-repo precedence as the code browser so the terminal and
// the editor are always looking at the same tree.
func (s *TerminalService) terminalCwd(workspaceID string) (string, error) {
	ws, ok := s.st().Workspace(workspaceID)
	if !ok {
		return "", fmt.Errorf("workspace %q not found", workspaceID)
	}
	var repo string
	if p, ok := s.st().Project(ws.ProjectID); ok {
		repo = p.RepoPath
	}
	root, err := code.ResolveRoot(repo, ws.WorktreePath)
	if err != nil {
		if errors.Is(err, code.ErrWorktreeMissing) {
			return "", fmt.Errorf("this workspace's folder no longer exists: %w", err)
		}
		return "", fmt.Errorf("terminal: no directory for workspace %q: %w", workspaceID, err)
	}
	return root, nil
}
