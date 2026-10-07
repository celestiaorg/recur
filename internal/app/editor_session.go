package app

import (
	"compress/gzip"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sync"

	"recur/internal/store"
)

// EditorSessionService persists per-workspace editor UI state — open tabs,
// selection, scratch docs, unsaved ("stashed") buffers, per-file caret /
// scroll / Code|Preview|Diff, TextMate language-mode overrides, whether
// the Observatory main column is Editor or Review, and left/right pane
// kind plus visibility — as one gzipped JSON snapshot under the workspace
// data dir:
//
//	<DataDir>/workspaces/<id>/editor-session.json.gz
//
// The frontend used to keep this in localStorage, which meant origin-storage
// quota gymnastics and sessions that silently didn't survive a webview storage
// wipe (or a dev↔packaged origin change). Here the state rides the same
// lifecycle as artifacts/attachments: workspace deletion sweeps the directory,
// and the write is a plain atomic temp+rename with no quota ceiling to trim to.
//
// The wire protocol is patch-based so typing doesn't ship the whole session
// over the bridge every debounce: UpdateEditorSession carries only what
// changed (optional meta, upserted buffers/scratch docs keyed by path, and
// explicit drops) and the merge happens here. Because absence in a patch means
// "unchanged", deletions MUST be explicit — the frontend sends a drop when a
// save lands or a tab closes.
type EditorSessionService struct {
	base

	mu     sync.Mutex
	states map[string]*editorSessionState // dropped on Clear and workspace.deleted
}

// editorSessionState serializes reads/merges/writes for one workspace and
// caches the decoded session so a patch doesn't re-read and re-parse the file
// it wrote 400ms ago.
type editorSessionState struct {
	mu     sync.Mutex
	loaded bool
	sess   *EditorSession // nil = no session on disk
}

// EditorScratchDoc is a `scratch://…` virtual document (command output, notes).
type EditorScratchDoc struct {
	Title   string `json:"title"`
	Content string `json:"content"`
	Lang    string `json:"lang"`
	// When set (e.g. image/*), the observatory renders as binary instead of text.
	Mime string `json:"mime,omitempty"`
}

// EditorBuffer is one unsaved disk buffer: the user's draft plus a hash of the
// on-disk text it diverged from. The frontend computes and compares the hash;
// this service just stores it. Keeping a hash instead of the full saved copy
// halves the payload and still lets restore detect "disk moved underneath the
// stash" (agents rewrite worktree files while tabs sit closed).
type EditorBuffer struct {
	Draft    string `json:"draft"`
	BaseHash string `json:"baseHash"`
}

// EditorFileView is per-file caret, scroll, and Code|Preview|Diff. Coordinates
// are 1-based (line/col/topLine); pixels are a fallback when topLine is absent.
type EditorFileView struct {
	ViewMode   string `json:"viewMode,omitempty"`
	Line       int    `json:"line"`
	Col        int    `json:"col"`
	AnchorLine int    `json:"anchorLine,omitempty"`
	AnchorCol  int    `json:"anchorCol,omitempty"`
	TopLine    int    `json:"topLine,omitempty"`
	ScrollTop  int    `json:"scrollTop"`
	ScrollLeft int    `json:"scrollLeft,omitempty"`
	// FilePreview scroller. Independent of CodeMirror ScrollTop.
	PreviewScrollTop  int     `json:"previewScrollTop,omitempty"`
	PreviewScrollLeft int     `json:"previewScrollLeft,omitempty"`
	PreviewZoom       float64 `json:"previewZoom,omitempty"`
}

// EditorSessionMeta is the cheap, often-rewritten part of a session: tab strip,
// selection, split pane, scratch tab sequence, per-file view state,
// TextMate language-mode maps, the Observatory Editor vs Review column, and
// left/right pane kind plus visibility. A patch replaces it wholesale.
type EditorSessionMeta struct {
	OpenFiles    []string                  `json:"openFiles"`
	SelectedPath string                    `json:"selectedPath"`
	SplitPath    string                    `json:"splitPath,omitempty"`
	ScratchSeq   int                       `json:"scratchSeq,omitempty"`
	FileViews    map[string]EditorFileView `json:"fileViews,omitempty"`
	// Syntax maps are optional on a meta patch: nil (JSON omitted) means
	// unchanged, so a tab-strip write before restore cannot wipe stored picks.
	// A non-nil empty map clears.
	SyntaxByPath     *map[string]string `json:"syntaxByPath,omitempty"`
	SyntaxByFileType *map[string]string `json:"syntaxByFileType,omitempty"`
	// "review" keeps the Review column on cold remount. Omitted / anything
	// else is the Editor column.
	MainSurface string `json:"mainSurface,omitempty"`
	// Observatory pane chrome. Empty kind is the side default (maestro / files).
	// Open flags are pointers so JSON omitempty does not treat a hidden pane
	// (`false`) as "unset" — nil means default-open, matching old sessions.
	LeftKind  string `json:"leftKind,omitempty"`
	RightKind string `json:"rightKind,omitempty"`
	LeftOpen  *bool  `json:"leftOpen,omitempty"`
	RightOpen *bool  `json:"rightOpen,omitempty"`
}

// EditorSession is the full per-workspace snapshot, as stored on disk and as
// returned to the frontend on restore.
type EditorSession struct {
	OpenFiles        []string                    `json:"openFiles"`
	SelectedPath     string                      `json:"selectedPath"`
	SplitPath        string                      `json:"splitPath,omitempty"`
	ScratchSeq       int                         `json:"scratchSeq,omitempty"`
	ScratchDocs      map[string]EditorScratchDoc `json:"scratchDocs,omitempty"`
	Buffers          map[string]EditorBuffer     `json:"buffers,omitempty"`
	FileViews        map[string]EditorFileView   `json:"fileViews,omitempty"`
	SyntaxByPath     map[string]string           `json:"syntaxByPath,omitempty"`
	SyntaxByFileType map[string]string           `json:"syntaxByFileType,omitempty"`
	MainSurface      string                      `json:"mainSurface,omitempty"`
	LeftKind         string                      `json:"leftKind,omitempty"`
	RightKind        string                      `json:"rightKind,omitempty"`
	LeftOpen         *bool                       `json:"leftOpen,omitempty"`
	RightOpen        *bool                       `json:"rightOpen,omitempty"`
}

// EditorSessionPatch is one incremental update. Meta replaces wholesale when
// present; Buffers/ScratchDocs upsert by path; Drop* delete by path. Fields
// left nil mean "unchanged".
type EditorSessionPatch struct {
	Meta            *EditorSessionMeta          `json:"meta,omitempty"`
	Buffers         map[string]EditorBuffer     `json:"buffers,omitempty"`
	DropBuffers     []string                    `json:"dropBuffers,omitempty"`
	ScratchDocs     map[string]EditorScratchDoc `json:"scratchDocs,omitempty"`
	DropScratchDocs []string                    `json:"dropScratchDocs,omitempty"`
}

const editorSessionFile = "editor-session.json.gz"

// maxEditorBufferBytes bounds a single stored draft/scratch body. The editor
// caps editable files at 16MB (maxEditBytes); anything past that here is a
// malfunctioning caller, not a user draft worth keeping.
const maxEditorBufferBytes = 32 << 20

// GetEditorSession returns the stored session for a workspace, or nil when
// none exists. Called once when the Observatory opens a workspace root.
func (s *EditorSessionService) GetEditorSession(workspaceID string) (*EditorSession, error) {
	if workspaceID == "" {
		return nil, nil
	}
	st := s.state(workspaceID)
	st.mu.Lock()
	defer st.mu.Unlock()
	if err := s.ensureLoaded(st, workspaceID); err != nil {
		return nil, err
	}
	// Copy out: Wails marshals the return value after the lock is released, and
	// a concurrent patch must not mutate the maps mid-marshal.
	return st.sess.clone(), nil
}

// UpdateEditorSession merges one patch into the stored session and writes it
// through. A session that ends up empty (no tabs, no buffers, no scratch, no
// language-mode maps, editor column) is deleted — an explicitly closed-out
// editor must not resurrect on restart. Language overrides and the Review
// column outlive the tab strip: closing the last tab keeps the file if
// syntax maps remain or Review is showing.
func (s *EditorSessionService) UpdateEditorSession(workspaceID string, patch EditorSessionPatch) error {
	if workspaceID == "" {
		return fmt.Errorf("workspace id is required")
	}
	// Refuse writes for unknown workspaces so a late patch can't recreate the
	// data dir of a workspace that was just deleted (DeleteWorkspace sweeps
	// workspaces/<id> wholesale).
	if _, ok := s.st().Workspace(workspaceID); !ok {
		return fmt.Errorf("workspace %q not found", workspaceID)
	}
	st := s.state(workspaceID)
	st.mu.Lock()
	defer st.mu.Unlock()
	if err := s.ensureLoaded(st, workspaceID); err != nil {
		return err
	}
	st.sess = mergeEditorSession(st.sess, patch)
	if st.sess == nil {
		return s.removeSessionFile(workspaceID)
	}
	return s.writeSessionFile(workspaceID, st.sess)
}

// ClearEditorSession deletes the stored session (last tab closed / discarded)
// and drops the in-memory cache entry so draft text is not retained.
func (s *EditorSessionService) ClearEditorSession(workspaceID string) error {
	if workspaceID == "" {
		return nil
	}
	st := s.state(workspaceID)
	st.mu.Lock()
	st.loaded = true
	st.sess = nil
	err := s.removeSessionFile(workspaceID)
	st.mu.Unlock()
	s.forget(workspaceID)
	return err
}

// state returns the per-workspace lock+cache entry, creating it on first use.
func (s *EditorSessionService) state(workspaceID string) *editorSessionState {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.states == nil {
		s.states = map[string]*editorSessionState{}
	}
	st, ok := s.states[workspaceID]
	if !ok {
		st = &editorSessionState{}
		s.states[workspaceID] = st
	}
	return st
}

// forget drops the in-memory cache entry. Called from ClearEditorSession and
// on workspace.deleted so decoded draft text does not outlive the workspace.
func (s *EditorSessionService) forget(workspaceID string) {
	if workspaceID == "" {
		return
	}
	s.mu.Lock()
	delete(s.states, workspaceID)
	s.mu.Unlock()
}

// ensureLoaded reads the on-disk session into the cache once. A missing or
// unreadable file is an absent session, not an error — the worst honest
// outcome is "nothing to restore".
func (s *EditorSessionService) ensureLoaded(st *editorSessionState, workspaceID string) error {
	if st.loaded {
		return nil
	}
	root, err := store.DataDir()
	if err != nil {
		return err
	}
	st.sess = readEditorSessionFile(editorSessionPath(root, workspaceID))
	st.loaded = true
	return nil
}

func editorSessionPath(root, workspaceID string) string {
	return filepath.Join(root, "workspaces", workspaceID, editorSessionFile)
}

// readEditorSessionFile decodes a gzipped session snapshot; nil on any failure
// (absent, truncated, corrupt) — tolerant reads, never a boot-blocking error.
func readEditorSessionFile(path string) *EditorSession {
	f, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer f.Close()
	zr, err := gzip.NewReader(f)
	if err != nil {
		return nil
	}
	defer zr.Close()
	var sess EditorSession
	if err := json.NewDecoder(zr).Decode(&sess); err != nil {
		return nil
	}
	return &sess
}

// writeSessionFile persists the session atomically: gzip into a temp file in
// the same directory, then rename over the live one. BestSpeed keeps the CPU
// cost trivial at the 400ms flush cadence; source text still shrinks ~4–5×.
func (s *EditorSessionService) writeSessionFile(workspaceID string, sess *EditorSession) error {
	root, err := store.DataDir()
	if err != nil {
		return err
	}
	dir, err := store.WorkspaceDir(root, workspaceID)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".editor-session-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name()) // no-op after a successful rename
	zw, err := gzip.NewWriterLevel(tmp, gzip.BestSpeed)
	if err != nil {
		tmp.Close()
		return err
	}
	encErr := json.NewEncoder(zw).Encode(sess)
	if err := zw.Close(); encErr == nil {
		encErr = err
	}
	if err := tmp.Close(); encErr == nil {
		encErr = err
	}
	if encErr != nil {
		return encErr
	}
	return os.Rename(tmp.Name(), filepath.Join(dir, editorSessionFile))
}

func (s *EditorSessionService) removeSessionFile(workspaceID string) error {
	root, err := store.DataDir()
	if err != nil {
		return err
	}
	if err := os.Remove(editorSessionPath(root, workspaceID)); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	return nil
}

// mergeEditorSession applies one patch. Returns nil when the merged session is
// empty, signalling the caller to delete the file instead of writing it.
func mergeEditorSession(sess *EditorSession, patch EditorSessionPatch) *EditorSession {
	if sess == nil {
		sess = &EditorSession{}
	}
	if patch.Meta != nil {
		sess.OpenFiles = append([]string(nil), patch.Meta.OpenFiles...)
		sess.SelectedPath = patch.Meta.SelectedPath
		sess.SplitPath = patch.Meta.SplitPath
		sess.ScratchSeq = patch.Meta.ScratchSeq
		sess.FileViews = cloneFileViews(patch.Meta.FileViews)
		if patch.Meta.SyntaxByPath != nil {
			sess.SyntaxByPath = cloneStringMap(*patch.Meta.SyntaxByPath)
		}
		if patch.Meta.SyntaxByFileType != nil {
			sess.SyntaxByFileType = cloneStringMap(*patch.Meta.SyntaxByFileType)
		}
		if patch.Meta.MainSurface == "review" {
			sess.MainSurface = "review"
		} else {
			sess.MainSurface = ""
		}
		sess.LeftKind = sanitizeEditorPanelKey(patch.Meta.LeftKind)
		sess.RightKind = sanitizeEditorPanelKey(patch.Meta.RightKind)
		sess.LeftOpen = cloneBoolPtr(patch.Meta.LeftOpen)
		sess.RightOpen = cloneBoolPtr(patch.Meta.RightOpen)
	}
	for path, buf := range patch.Buffers {
		if path == "" || len(buf.Draft) > maxEditorBufferBytes {
			continue
		}
		if sess.Buffers == nil {
			sess.Buffers = map[string]EditorBuffer{}
		}
		sess.Buffers[path] = buf
	}
	for _, path := range patch.DropBuffers {
		delete(sess.Buffers, path)
	}
	for path, doc := range patch.ScratchDocs {
		if path == "" || len(doc.Content) > maxEditorBufferBytes {
			continue
		}
		if sess.ScratchDocs == nil {
			sess.ScratchDocs = map[string]EditorScratchDoc{}
		}
		sess.ScratchDocs[path] = doc
	}
	for _, path := range patch.DropScratchDocs {
		delete(sess.ScratchDocs, path)
	}
	if editorSessionEmpty(sess) {
		return nil
	}
	return sess
}

func editorSessionEmpty(sess *EditorSession) bool {
	return len(sess.OpenFiles) == 0 && len(sess.Buffers) == 0 && len(sess.ScratchDocs) == 0 &&
		len(sess.SyntaxByPath) == 0 && len(sess.SyntaxByFileType) == 0 &&
		sess.MainSurface != "review" && !editorSessionHasPaneChrome(sess)
}

// editorSessionHasPaneChrome is true when left/right kind or visibility is
// not the Observatory cold default (maestro + files, both open).
func editorSessionHasPaneChrome(sess *EditorSession) bool {
	if sess == nil {
		return false
	}
	if sess.LeftKind != "" && sess.LeftKind != "maestro" {
		return true
	}
	if sess.RightKind != "" && sess.RightKind != "files" {
		return true
	}
	if sess.LeftOpen != nil && !*sess.LeftOpen {
		return true
	}
	if sess.RightOpen != nil && !*sess.RightOpen {
		return true
	}
	return false
}

// clone deep-copies the session (map values are plain structs, so copying the
// maps and the slice is enough).
func (sess *EditorSession) clone() *EditorSession {
	if sess == nil {
		return nil
	}
	out := &EditorSession{
		OpenFiles:        append([]string(nil), sess.OpenFiles...),
		SelectedPath:     sess.SelectedPath,
		SplitPath:        sess.SplitPath,
		ScratchSeq:       sess.ScratchSeq,
		FileViews:        cloneFileViews(sess.FileViews),
		SyntaxByPath:     cloneStringMap(sess.SyntaxByPath),
		SyntaxByFileType: cloneStringMap(sess.SyntaxByFileType),
		MainSurface:      sess.MainSurface,
		LeftKind:         sess.LeftKind,
		RightKind:        sess.RightKind,
		LeftOpen:         cloneBoolPtr(sess.LeftOpen),
		RightOpen:        cloneBoolPtr(sess.RightOpen),
	}
	if len(sess.ScratchDocs) > 0 {
		out.ScratchDocs = make(map[string]EditorScratchDoc, len(sess.ScratchDocs))
		for k, v := range sess.ScratchDocs {
			out.ScratchDocs[k] = v
		}
	}
	if len(sess.Buffers) > 0 {
		out.Buffers = make(map[string]EditorBuffer, len(sess.Buffers))
		for k, v := range sess.Buffers {
			out.Buffers[k] = v
		}
	}
	return out
}

func cloneFileViews(in map[string]EditorFileView) map[string]EditorFileView {
	if len(in) == 0 {
		return nil
	}
	out := make(map[string]EditorFileView, len(in))
	for k, v := range in {
		out[k] = v
	}
	return out
}

func cloneStringMap(in map[string]string) map[string]string {
	if len(in) == 0 {
		return nil
	}
	out := make(map[string]string, len(in))
	for k, v := range in {
		out[k] = v
	}
	return out
}

func cloneBoolPtr(p *bool) *bool {
	if p == nil {
		return nil
	}
	v := *p
	return &v
}

// editorPanelKeys matches frontend CODE_PANEL_KEYS. Unknown values are dropped
// so a bad patch cannot pin a session that would otherwise delete.
var editorPanelKeys = map[string]struct{}{
	"maestro": {}, "plan": {}, "findings": {}, "files": {},
	"scm": {}, "outline": {}, "search": {}, "refs": {}, "artifacts": {},
}

func sanitizeEditorPanelKey(s string) string {
	if _, ok := editorPanelKeys[s]; ok {
		return s
	}
	return ""
}
