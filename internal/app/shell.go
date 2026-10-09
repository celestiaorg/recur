package app

import (
	"encoding/json"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"recur/internal/store"
)

const (
	shellStateFile      = "shell.json"
	scheduleViewFile    = "schedule-view.json"
	issuesViewFile      = "issues-view.json"
	shellSaveDebounce   = 250 * time.Millisecond
	shellSaveBackoffMax = 30 * time.Second
)

// ShellState is the last top-level UI the user was looking at. Persisted under
// the managed data root (next to window.json) so the open workspace/view
// survives app restarts, webview storage wipes, and dev↔packaged origin changes.
type ShellState struct {
	View        string `json:"view"`
	WorkspaceID string `json:"workspaceId,omitempty"`
	PRID        string `json:"prId,omitempty"`
	ProjectID   string `json:"projectId,omitempty"`
}

// ShellService is the Wails-bound read/write surface for ShellState.
// SetShellState updates the in-memory snapshot immediately and coalesces disk
// writes; Flush (window close) commits any pending snapshot so a quit inside
// the debounce window still restores the last view.
type ShellService struct {
	mu         sync.Mutex
	timer      *time.Timer
	loaded     bool
	last       ShellState
	pending    *ShellState
	failDelay  time.Duration
	failLogged bool

	scheduleLoaded bool
	expanded       []string
}

// GetShellState returns the last persisted shell, or a zero value when none
// exists (first run / corrupt file). The frontend treats an empty View as
// "stay on the default Inbox" unless first-run routing sends Welcome.
func (s *ShellService) GetShellState() ShellState {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.ensureLoaded()
	return s.last
}

// SetShellState records the current shell. Unknown views (including welcome)
// are ignored so a future client cannot wipe a good shell.json. The disk write
// is coalesced; call Flush on shutdown so the last snapshot is durable.
func (s *ShellService) SetShellState(st ShellState) error {
	if !knownShellView(st.View) {
		return nil
	}
	st = clampShellState(st)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.ensureLoaded()
	if s.last == st {
		return nil
	}
	s.last = st
	cp := st
	s.pending = &cp
	s.scheduleLocked()
	return nil
}

// GetExpandedSchedules returns which schedule dashboard rows are expanded.
// Lives next to shell.json so accordion state survives a webview storage wipe.
func (s *ShellService) GetExpandedSchedules() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.ensureScheduleViewLocked()
	return append([]string(nil), s.expanded...)
}

// SetExpandedSchedules records expanded schedule dashboard rows.
func (s *ShellService) SetExpandedSchedules(ids []string) error {
	ids = normalizeExpanded(ids)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.ensureScheduleViewLocked()
	if expandedEqual(s.expanded, ids) {
		return nil
	}
	s.expanded = ids
	return saveScheduleView(scheduleView{Expanded: ids})
}

// Flush writes any coalesced snapshot to disk. Called from the window-close
// hook so ⌘Q does not depend on a frontend pagehide IPC. The lock is held
// across the write so overlapping flushes cannot share shell.json.tmp. A
// failed write keeps pending and retries with exponential backoff; only the
// first failure (and later recovery) is logged.
func (s *ShellService) Flush() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.timer != nil {
		s.timer.Stop()
		s.timer = nil
	}
	if s.pending == nil {
		return
	}
	st := *s.pending
	if err := saveShellState(st); err != nil {
		if !s.failLogged {
			log.Printf("shell: persist: %v", err)
			s.failLogged = true
		}
		if s.failDelay == 0 {
			s.failDelay = shellSaveDebounce * 2
		} else if s.failDelay < shellSaveBackoffMax {
			s.failDelay *= 2
			if s.failDelay > shellSaveBackoffMax {
				s.failDelay = shellSaveBackoffMax
			}
		}
		s.scheduleLocked()
		return
	}
	if s.failLogged {
		log.Printf("shell: persist recovered")
	}
	s.failLogged = false
	s.failDelay = 0
	s.pending = nil
}

func (s *ShellService) ensureLoaded() {
	if s.loaded {
		return
	}
	s.last = loadShellState()
	s.loaded = true
}

func (s *ShellService) scheduleLocked() {
	if s.timer != nil {
		s.timer.Stop()
	}
	d := shellSaveDebounce
	if s.failDelay > 0 {
		d = s.failDelay
	}
	s.timer = time.AfterFunc(d, s.Flush)
}

func shellStatePath() (string, error) {
	root, err := store.DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(root, shellStateFile), nil
}

func loadShellState() ShellState {
	path, err := shellStatePath()
	if err != nil {
		return ShellState{}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return ShellState{}
	}
	var st ShellState
	if err := json.Unmarshal(data, &st); err != nil {
		return ShellState{}
	}
	return clampShellState(st)
}

func saveShellState(st ShellState) error {
	path, err := shellStatePath()
	if err != nil {
		return err
	}
	data, err := json.MarshalIndent(st, "", "  ")
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	if err := os.Rename(tmp, path); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return nil
}

func clampShellState(st ShellState) ShellState {
	switch st.View {
	case "detail", "escalation":
		st.PRID = ""
		st.ProjectID = ""
	case "prdetail":
		st.WorkspaceID = ""
		st.ProjectID = ""
	case "project":
		st.WorkspaceID = ""
		st.PRID = ""
	case "inbox", "briefing", "settings", "pr", "schedules":
		st.WorkspaceID = ""
		st.PRID = ""
		st.ProjectID = ""
	default:
		return ShellState{}
	}
	return st
}

// knownShellView is the persistable set. Must stay in sync with snapshotShell /
// resolveShellRestore in frontend/src/lib/shell-state.ts (welcome is first-run
// only and is intentionally omitted).
func knownShellView(v string) bool {
	switch v {
	case "detail", "escalation", "prdetail", "project", "inbox", "briefing", "settings", "pr", "schedules":
		return true
	}
	return false
}

type scheduleView struct {
	Expanded []string `json:"expanded,omitempty"`
}

func (s *ShellService) ensureScheduleViewLocked() {
	if s.scheduleLoaded {
		return
	}
	s.expanded = loadScheduleView().Expanded
	if s.expanded == nil {
		s.expanded = []string{}
	}
	s.scheduleLoaded = true
}

func normalizeExpanded(ids []string) []string {
	if len(ids) == 0 {
		return []string{}
	}
	seen := make(map[string]bool, len(ids))
	out := make([]string, 0, len(ids))
	for _, id := range ids {
		id = strings.TrimSpace(id)
		if id == "" || seen[id] {
			continue
		}
		seen[id] = true
		out = append(out, id)
	}
	return out
}

func expandedEqual(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func scheduleViewPath() (string, error) {
	root, err := store.DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(root, scheduleViewFile), nil
}

func loadScheduleView() scheduleView {
	path, err := scheduleViewPath()
	if err != nil {
		return scheduleView{}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return scheduleView{}
	}
	var st scheduleView
	if json.Unmarshal(data, &st) != nil {
		return scheduleView{}
	}
	st.Expanded = normalizeExpanded(st.Expanded)
	return st
}

func saveScheduleView(st scheduleView) error {
	return writeDataJSON(scheduleViewFile, st)
}

// IssuesView is the Issues page's layout and filters. The frontend validates
// values on load, so an unknown or stale entry falls back to its default.
type IssuesView struct {
	Layout   string   `json:"layout,omitempty"`
	Source   string   `json:"source,omitempty"`
	Owner    string   `json:"owner,omitempty"`
	Groups   []string `json:"groups,omitempty"`
	Sort     string   `json:"sort,omitempty"`
	Projects []string `json:"projects,omitempty"`
}

// GetIssuesView returns the saved Issues view, or a zero value when none
// exists or the file is corrupt.
func (s *ShellService) GetIssuesView() IssuesView {
	var v IssuesView
	root, err := store.DataDir()
	if err != nil {
		return v
	}
	data, err := os.ReadFile(filepath.Join(root, issuesViewFile))
	if err != nil || json.Unmarshal(data, &v) != nil {
		return IssuesView{}
	}
	return v
}

// SetIssuesView records the Issues view next to shell.json.
func (s *ShellService) SetIssuesView(v IssuesView) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return writeDataJSON(issuesViewFile, v)
}

// writeDataJSON atomically writes v as indented JSON to name under DataDir.
func writeDataJSON(name string, v any) error {
	root, err := store.DataDir()
	if err != nil {
		return err
	}
	path := filepath.Join(root, name)
	data, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	if err := os.Rename(tmp, path); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return nil
}
