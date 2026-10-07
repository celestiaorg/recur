package app

import (
	"compress/gzip"
	"encoding/json"
	"errors"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"time"

	"recur/internal/store"
)

const (
	terminalPersistFile     = "terminals.json.gz"
	terminalPersistDebounce = time.Second
	terminalPersistMaxAge   = 5 * time.Second
	terminalPersistMode     = 0o600
)

// persistedTerminals is the per-workspace snapshot written under
// <DataDir>/workspaces/<id>/terminals.json.gz. Same lifecycle as
// editor-session.json.gz: workspace deletion sweeps the directory.
// Open/Maximized/EditorHidden/Beside are the Observatory dock layout (not
// webview localStorage) so they survive a storage wipe and a
// dev↔packaged origin change the way editor-session does.
type persistedTerminals struct {
	Open         bool           `json:"open,omitempty"`
	Maximized    bool           `json:"maximized,omitempty"`
	EditorHidden bool           `json:"editorHidden,omitempty"`
	Beside       bool           `json:"beside,omitempty"`
	BesidePct    int            `json:"besidePct,omitempty"`
	Active       int            `json:"active"`
	Tabs         []persistedTab `json:"tabs"`
}

// TerminalDockChrome is the Observatory dock layout for one workspace.
// ListTerminalDockChrome returns these at boot; SetTerminalDockChrome
// writes them onto terminals.json.gz next to the tab snapshot.
type TerminalDockChrome struct {
	WorkspaceID  string `json:"workspaceId,omitempty"`
	Open         bool   `json:"open"`
	Maximized    bool   `json:"maximized"`
	EditorHidden bool   `json:"editorHidden"`
	Beside       bool   `json:"beside"`
	BesidePct    int    `json:"besidePct,omitempty"`
}

type terminalDockChrome struct {
	Open, Maximized, EditorHidden, Beside bool
	BesidePct                             int
}

func (c terminalDockChrome) idle() bool {
	return !c.Open && !c.Maximized && !c.EditorHidden && !c.Beside && c.BesidePct == 0
}

func clampBesidePct(n int) int {
	if n <= 0 {
		return 0
	}
	if n < 20 {
		return 20
	}
	if n > 80 {
		return 80
	}
	return n
}

func clampChrome(c terminalDockChrome) terminalDockChrome {
	if !c.Open {
		c.Maximized = false
		c.EditorHidden = false
	}
	c.BesidePct = clampBesidePct(c.BesidePct)
	return c
}

func snapshotIdle(snap persistedTerminals) bool {
	return len(snap.Tabs) == 0 && terminalDockChrome{
		Open: snap.Open, Maximized: snap.Maximized, EditorHidden: snap.EditorHidden,
		Beside: snap.Beside, BesidePct: snap.BesidePct,
	}.idle()
}

type persistedTab struct {
	Title  string `json:"title"`
	Cwd    string `json:"cwd"`
	Replay []byte `json:"replay,omitempty"`
}

func persistDelay(now, firstDirty time.Time, debounce, maxAge time.Duration) time.Duration {
	delay := debounce
	if firstDirty.IsZero() {
		return delay
	}
	remain := maxAge - now.Sub(firstDirty)
	if remain < delay {
		if remain < 0 {
			return 0
		}
		return remain
	}
	return delay
}

func (s *TerminalService) markDirty(workspaceID string) {
	if s == nil || workspaceID == "" {
		return
	}
	s.persistMu.Lock()
	defer s.persistMu.Unlock()
	if s.persistClosed {
		return
	}
	now := time.Now()
	if s.persistFirst.IsZero() {
		s.persistFirst = now
	}
	if s.persistDirty == nil {
		s.persistDirty = map[string]bool{}
	}
	s.persistDirty[workspaceID] = true
	delay := persistDelay(now, s.persistFirst, terminalPersistDebounce, terminalPersistMaxAge)
	if s.persistTimer == nil {
		s.persistTimer = time.AfterFunc(delay, s.flushPersist)
	} else {
		s.persistTimer.Reset(delay)
	}
}

func (s *TerminalService) forget(workspaceID string) {
	if workspaceID == "" {
		return
	}
	s.persistMu.Lock()
	delete(s.persistDirty, workspaceID)
	delete(s.chrome, workspaceID)
	s.persistMu.Unlock()
}

// flushPersist writes every dirty workspace snapshot. Called from the debounce
// timer and from ServiceShutdown so a quit inside the window still restores.
func (s *TerminalService) flushPersist() {
	s.persistMu.Lock()
	if s.persistTimer != nil {
		s.persistTimer.Stop()
		s.persistTimer = nil
	}
	s.persistFirst = time.Time{}
	ids := make([]string, 0, len(s.persistDirty))
	for id, dirty := range s.persistDirty {
		if dirty {
			ids = append(ids, id)
		}
	}
	s.persistDirty = map[string]bool{}
	s.persistMu.Unlock()

	for _, id := range ids {
		if err := s.persistWorkspace(id); err != nil {
			log.Printf("terminal: persist %s: %v", id, err)
			s.markDirty(id)
		}
	}
}

func (s *TerminalService) persistWorkspace(workspaceID string) error {
	if _, ok := s.st().Workspace(workspaceID); !ok {
		return removeTerminalSnapshot(workspaceID)
	}
	chrome := clampChrome(s.chromeOf(workspaceID))
	snaps := s.mgr.Snapshots(workspaceID)
	s.persistMu.Lock()
	closed := s.persistClosed
	s.persistMu.Unlock()
	if len(snaps) == 0 {
		if closed {
			// CloseAll has already emptied the roster; do not erase the
			// snapshot flushPersist just wrote.
			return nil
		}
		if chrome.idle() {
			return removeTerminalSnapshot(workspaceID)
		}
	}
	activeID := s.mgr.ActiveID(workspaceID)
	active := 0
	tabs := make([]persistedTab, len(snaps))
	for i, snap := range snaps {
		if snap.ID == activeID {
			active = i
		}
		tabs[i] = persistedTab{Title: snap.Title, Cwd: snap.Cwd, Replay: snap.Replay}
	}
	return writeTerminalSnapshot(workspaceID, persistedTerminals{
		Open:         chrome.Open,
		Maximized:    chrome.Maximized,
		EditorHidden: chrome.EditorHidden,
		Beside:       chrome.Beside,
		BesidePct:    chrome.BesidePct,
		Active:       active,
		Tabs:         tabs,
	})
}

func (s *TerminalService) chromeOf(workspaceID string) terminalDockChrome {
	s.persistMu.Lock()
	c, ok := s.chrome[workspaceID]
	s.persistMu.Unlock()
	if ok {
		return c
	}
	c = terminalDockChrome{}
	if snap := readTerminalChrome(workspaceID); snap != nil {
		c = terminalDockChrome{
			Open: snap.Open, Maximized: snap.Maximized, EditorHidden: snap.EditorHidden,
			Beside: snap.Beside, BesidePct: snap.BesidePct,
		}
	}
	s.persistMu.Lock()
	if s.chrome == nil {
		s.chrome = map[string]terminalDockChrome{}
	}
	if cached, exists := s.chrome[workspaceID]; exists {
		s.persistMu.Unlock()
		return cached
	}
	s.chrome[workspaceID] = c
	s.persistMu.Unlock()
	return c
}

// SetTerminalDockChrome records the Observatory dock layout for a workspace.
// Disk write is the same debounce as tab snapshots (flushed on quit).
func (s *TerminalService) SetTerminalDockChrome(workspaceID string, chrome TerminalDockChrome) error {
	if workspaceID == "" {
		return nil
	}
	c := clampChrome(terminalDockChrome{
		Open: chrome.Open, Maximized: chrome.Maximized, EditorHidden: chrome.EditorHidden,
		Beside: chrome.Beside, BesidePct: chrome.BesidePct,
	})
	s.persistMu.Lock()
	if s.chrome == nil {
		s.chrome = map[string]terminalDockChrome{}
	}
	s.chrome[workspaceID] = c
	s.persistMu.Unlock()
	s.markDirty(workspaceID)
	return nil
}

// ListTerminalDockChrome returns dock layouts that are not the closed default,
// so the frontend can restore open/maximized/editor-hidden before first paint.
func (s *TerminalService) ListTerminalDockChrome() []TerminalDockChrome {
	out := []TerminalDockChrome{}
	for _, ws := range s.st().Workspaces() {
		if ws.Archived || ws.IsPRChatHost() {
			continue
		}
		c := clampChrome(s.chromeOf(ws.ID))
		if c.idle() {
			continue
		}
		out = append(out, TerminalDockChrome{
			WorkspaceID:  ws.ID,
			Open:         c.Open,
			Maximized:    c.Maximized,
			EditorHidden: c.EditorHidden,
			Beside:       c.Beside,
			BesidePct:    c.BesidePct,
		})
	}
	return out
}

func terminalSnapshotPath(root, workspaceID string) string {
	return filepath.Join(root, "workspaces", workspaceID, terminalPersistFile)
}

func decodeTerminalSnapshot(workspaceID string, dst any) bool {
	if workspaceID == "" {
		return false
	}
	root, err := store.DataDir()
	if err != nil {
		return false
	}
	f, err := os.Open(terminalSnapshotPath(root, workspaceID))
	if err != nil {
		return false
	}
	defer f.Close()
	zr, err := gzip.NewReader(f)
	if err != nil {
		return false
	}
	defer zr.Close()
	return json.NewDecoder(zr).Decode(dst) == nil
}

func readTerminalSnapshot(workspaceID string) *persistedTerminals {
	var snap persistedTerminals
	if !decodeTerminalSnapshot(workspaceID, &snap) {
		return nil
	}
	if snapshotIdle(snap) {
		return nil
	}
	return &snap
}

// persistedChrome is the dock-layout prefix of terminals.json.gz. Decoding
// into this type skips Tabs/Replay so ListTerminalDockChrome does not
// allocate scrollback just to read chrome flags.
type persistedChrome struct {
	Open         bool `json:"open,omitempty"`
	Maximized    bool `json:"maximized,omitempty"`
	EditorHidden bool `json:"editorHidden,omitempty"`
	Beside       bool `json:"beside,omitempty"`
	BesidePct    int  `json:"besidePct,omitempty"`
}

func readTerminalChrome(workspaceID string) *persistedChrome {
	var snap persistedChrome
	if !decodeTerminalSnapshot(workspaceID, &snap) {
		return nil
	}
	return &snap
}

func writeTerminalSnapshot(workspaceID string, snap persistedTerminals) error {
	root, err := store.DataDir()
	if err != nil {
		return err
	}
	dir, err := store.WorkspaceDir(root, workspaceID)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".terminals-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	zw, err := gzip.NewWriterLevel(tmp, gzip.BestSpeed)
	if err != nil {
		tmp.Close()
		return err
	}
	encErr := json.NewEncoder(zw).Encode(snap)
	if err := zw.Close(); encErr == nil {
		encErr = err
	}
	if err := tmp.Close(); encErr == nil {
		encErr = err
	}
	if encErr != nil {
		return encErr
	}
	path := filepath.Join(dir, terminalPersistFile)
	if err := os.Rename(tmp.Name(), path); err != nil {
		return err
	}
	return os.Chmod(path, terminalPersistMode)
}

func removeTerminalSnapshot(workspaceID string) error {
	root, err := store.DataDir()
	if err != nil {
		return err
	}
	if err := os.Remove(terminalSnapshotPath(root, workspaceID)); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	return nil
}
