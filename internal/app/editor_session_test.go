package app

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"recur/internal/code"
	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

// newEditorSessionService seeds an in-memory store with one workspace and
// points the data dir at a temp root.
func newEditorSessionService(t *testing.T) (*EditorSessionService, string, string) {
	t.Helper()
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	eng := engine.New(store.NewInMemory())
	ws, err := eng.Store.AddWorkspace(domain.Workspace{ID: "WS-1", ProjectID: "p"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	return &EditorSessionService{base: base{eng: eng}}, ws.ID, dataDir
}

func metaPatch(openFiles []string, selected string) EditorSessionPatch {
	return EditorSessionPatch{Meta: &EditorSessionMeta{OpenFiles: openFiles, SelectedPath: selected}}
}

func ptrStringMap(m map[string]string) *map[string]string {
	return &m
}

func TestEditorSessionRoundTrip(t *testing.T) {
	svc, wsID, dataDir := newEditorSessionService(t)

	if sess, err := svc.GetEditorSession(wsID); err != nil || sess != nil {
		t.Fatalf("GetEditorSession(empty) = %v, %v; want nil, nil", sess, err)
	}

	patch := metaPatch([]string{"src/main.go", "src/other.go"}, "src/main.go")
	patch.Buffers = map[string]EditorBuffer{
		"src/main.go": {Draft: "package main // edited\n", BaseHash: "abc123"},
	}
	patch.ScratchDocs = map[string]EditorScratchDoc{
		"scratch://1-notes.txt": {Title: "Notes", Content: "todo\n", Lang: "txt"},
	}
	patch.Meta.FileViews = map[string]EditorFileView{
		"src/main.go": {ViewMode: "preview", Line: 12, Col: 4, TopLine: 10, ScrollTop: 80, PreviewScrollTop: 240},
	}
	if err := svc.UpdateEditorSession(wsID, patch); err != nil {
		t.Fatalf("UpdateEditorSession: %v", err)
	}

	sess, err := svc.GetEditorSession(wsID)
	if err != nil || sess == nil {
		t.Fatalf("GetEditorSession = %v, %v", sess, err)
	}
	if got := sess.Buffers["src/main.go"]; got.Draft != "package main // edited\n" || got.BaseHash != "abc123" {
		t.Fatalf("buffer = %+v", got)
	}
	if got := sess.ScratchDocs["scratch://1-notes.txt"]; got.Content != "todo\n" {
		t.Fatalf("scratch = %+v", got)
	}
	if len(sess.OpenFiles) != 2 || sess.SelectedPath != "src/main.go" {
		t.Fatalf("meta = %+v", sess)
	}
	if got := sess.FileViews["src/main.go"]; got.ViewMode != "preview" || got.Line != 12 || got.Col != 4 || got.TopLine != 10 || got.PreviewScrollTop != 240 {
		t.Fatalf("fileViews = %+v", sess.FileViews)
	}

	// Survives a fresh service (i.e. the file on disk, not just the cache).
	fresh := &EditorSessionService{base: svc.base}
	sess2, err := fresh.GetEditorSession(wsID)
	if err != nil || sess2 == nil || sess2.Buffers["src/main.go"].Draft != "package main // edited\n" {
		t.Fatalf("fresh GetEditorSession = %+v, %v", sess2, err)
	}
	if got := sess2.FileViews["src/main.go"]; got.ViewMode != "preview" || got.Line != 12 {
		t.Fatalf("fresh fileViews = %+v", sess2.FileViews)
	}
	if _, statErr := os.Stat(filepath.Join(dataDir, "workspaces", wsID, editorSessionFile)); statErr != nil {
		t.Fatalf("session file: %v", statErr)
	}
}

func TestEditorSessionPatchMerge(t *testing.T) {
	svc, wsID, _ := newEditorSessionService(t)

	first := metaPatch([]string{"a.go", "b.go"}, "a.go")
	first.Buffers = map[string]EditorBuffer{
		"a.go": {Draft: "A1", BaseHash: "ha"},
		"b.go": {Draft: "B1", BaseHash: "hb"},
	}
	if err := svc.UpdateEditorSession(wsID, first); err != nil {
		t.Fatalf("first patch: %v", err)
	}

	// Buffer-only upsert leaves meta and the other buffer alone.
	if err := svc.UpdateEditorSession(wsID, EditorSessionPatch{
		Buffers: map[string]EditorBuffer{"a.go": {Draft: "A2", BaseHash: "ha"}},
	}); err != nil {
		t.Fatalf("upsert patch: %v", err)
	}
	sess, _ := svc.GetEditorSession(wsID)
	if sess.Buffers["a.go"].Draft != "A2" || sess.Buffers["b.go"].Draft != "B1" {
		t.Fatalf("after upsert: %+v", sess.Buffers)
	}
	if sess.SelectedPath != "a.go" {
		t.Fatalf("meta clobbered: %+v", sess)
	}

	// Meta replace ships a new fileViews map wholesale.
	if err := svc.UpdateEditorSession(wsID, EditorSessionPatch{
		Meta: &EditorSessionMeta{
			OpenFiles:    []string{"a.go", "b.go"},
			SelectedPath: "b.go",
			FileViews:    map[string]EditorFileView{"b.go": {ViewMode: "diff", Line: 9, Col: 1}},
		},
	}); err != nil {
		t.Fatalf("fileViews patch: %v", err)
	}
	sess, _ = svc.GetEditorSession(wsID)
	if sess.SelectedPath != "b.go" {
		t.Fatalf("selected after fileViews patch: %+v", sess)
	}
	if got := sess.FileViews["b.go"]; got.ViewMode != "diff" || got.Line != 9 {
		t.Fatalf("fileViews = %+v", sess.FileViews)
	}
	if _, ok := sess.FileViews["a.go"]; ok {
		t.Fatalf("stale fileView kept: %+v", sess.FileViews)
	}

	// Explicit drop removes exactly one buffer.
	if err := svc.UpdateEditorSession(wsID, EditorSessionPatch{DropBuffers: []string{"a.go"}}); err != nil {
		t.Fatalf("drop patch: %v", err)
	}
	sess, _ = svc.GetEditorSession(wsID)
	if _, ok := sess.Buffers["a.go"]; ok {
		t.Fatalf("a.go not dropped: %+v", sess.Buffers)
	}
	if sess.Buffers["b.go"].Draft != "B1" {
		t.Fatalf("b.go lost: %+v", sess.Buffers)
	}
}

func TestEditorSessionSyntaxMapsRoundTrip(t *testing.T) {
	svc, wsID, _ := newEditorSessionService(t)

	patch := metaPatch([]string{"src/main.go"}, "src/main.go")
	patch.Meta.SyntaxByPath = ptrStringMap(map[string]string{"src/main.go": ""})
	patch.Meta.SyntaxByFileType = ptrStringMap(map[string]string{"*.conf": "source.yaml"})
	if err := svc.UpdateEditorSession(wsID, patch); err != nil {
		t.Fatalf("UpdateEditorSession: %v", err)
	}

	sess, err := svc.GetEditorSession(wsID)
	if err != nil || sess == nil {
		t.Fatalf("GetEditorSession = %v, %v", sess, err)
	}
	if got, ok := sess.SyntaxByPath["src/main.go"]; !ok || got != "" {
		t.Fatalf("syntaxByPath = %+v", sess.SyntaxByPath)
	}
	if got := sess.SyntaxByFileType["*.conf"]; got != "source.yaml" {
		t.Fatalf("syntaxByFileType = %+v", sess.SyntaxByFileType)
	}

	// Buffer-only upsert leaves the maps alone (meta not sent).
	if err := svc.UpdateEditorSession(wsID, EditorSessionPatch{
		Buffers: map[string]EditorBuffer{"src/main.go": {Draft: "x", BaseHash: "h"}},
	}); err != nil {
		t.Fatalf("buffer patch: %v", err)
	}
	sess, _ = svc.GetEditorSession(wsID)
	if got := sess.SyntaxByFileType["*.conf"]; got != "source.yaml" {
		t.Fatalf("syntax maps clobbered: %+v", sess.SyntaxByFileType)
	}

	// Meta without syntax fields (tab-strip write before restore) leaves maps.
	if err := svc.UpdateEditorSession(wsID, metaPatch([]string{"src/other.go"}, "src/other.go")); err != nil {
		t.Fatalf("meta without syntax: %v", err)
	}
	sess, _ = svc.GetEditorSession(wsID)
	if got := sess.SyntaxByFileType["*.conf"]; got != "source.yaml" {
		t.Fatalf("syntax maps wiped by tab-only meta: %+v", sess.SyntaxByFileType)
	}
	if got, ok := sess.SyntaxByPath["src/main.go"]; !ok || got != "" {
		t.Fatalf("syntaxByPath wiped: %+v", sess.SyntaxByPath)
	}

	// Fresh service reads the gzip file, not just the cache.
	fresh := &EditorSessionService{base: svc.base}
	sess2, err := fresh.GetEditorSession(wsID)
	if err != nil || sess2 == nil || sess2.SyntaxByFileType["*.conf"] != "source.yaml" {
		t.Fatalf("fresh GetEditorSession = %+v, %v", sess2, err)
	}
}

func TestEditorSessionSyntaxOnlyIsKept(t *testing.T) {
	svc, wsID, dataDir := newEditorSessionService(t)

	seed := metaPatch([]string{"a.go"}, "a.go")
	seed.Meta.SyntaxByPath = ptrStringMap(map[string]string{"a.go": "source.yaml"})
	seed.Buffers = map[string]EditorBuffer{"a.go": {Draft: "X", BaseHash: "h"}}
	if err := svc.UpdateEditorSession(wsID, seed); err != nil {
		t.Fatalf("seed: %v", err)
	}

	// Closing the last tab keeps the session when language maps remain.
	emptyTabs := metaPatch(nil, "")
	emptyTabs.Meta.SyntaxByPath = ptrStringMap(map[string]string{"a.go": "source.yaml"})
	emptyTabs.DropBuffers = []string{"a.go"}
	if err := svc.UpdateEditorSession(wsID, emptyTabs); err != nil {
		t.Fatalf("syntax-only patch: %v", err)
	}
	sess, _ := svc.GetEditorSession(wsID)
	if sess == nil {
		t.Fatal("syntax-only session should be kept")
	}
	if len(sess.OpenFiles) != 0 || len(sess.Buffers) != 0 {
		t.Fatalf("want empty tabs/buffers, got %+v", sess)
	}
	if got := sess.SyntaxByPath["a.go"]; got != "source.yaml" {
		t.Fatalf("syntaxByPath = %+v", sess.SyntaxByPath)
	}
	if _, err := os.Stat(filepath.Join(dataDir, "workspaces", wsID, editorSessionFile)); err != nil {
		t.Fatalf("session file should remain: %v", err)
	}

	// Clearing the maps too deletes the file.
	gone := metaPatch(nil, "")
	gone.Meta.SyntaxByPath = ptrStringMap(nil)
	gone.Meta.SyntaxByFileType = ptrStringMap(nil)
	if err := svc.UpdateEditorSession(wsID, gone); err != nil {
		t.Fatalf("empty patch: %v", err)
	}
	if sess, _ := svc.GetEditorSession(wsID); sess != nil {
		t.Fatalf("session should be gone, got %+v", sess)
	}
}

func TestEditorSessionReviewOnlyIsKept(t *testing.T) {
	svc, wsID, dataDir := newEditorSessionService(t)

	review := metaPatch(nil, "")
	review.Meta.MainSurface = "review"
	if err := svc.UpdateEditorSession(wsID, review); err != nil {
		t.Fatalf("review-only patch: %v", err)
	}
	sess, err := svc.GetEditorSession(wsID)
	if err != nil || sess == nil || sess.MainSurface != "review" {
		t.Fatalf("GetEditorSession = %+v, %v", sess, err)
	}
	if _, err := os.Stat(filepath.Join(dataDir, "workspaces", wsID, editorSessionFile)); err != nil {
		t.Fatalf("session file should remain: %v", err)
	}

	fresh := &EditorSessionService{base: svc.base}
	sess2, err := fresh.GetEditorSession(wsID)
	if err != nil || sess2 == nil || sess2.MainSurface != "review" {
		t.Fatalf("fresh GetEditorSession = %+v, %v", sess2, err)
	}

	editor := metaPatch(nil, "")
	if err := svc.UpdateEditorSession(wsID, editor); err != nil {
		t.Fatalf("editor patch: %v", err)
	}
	if sess, _ := svc.GetEditorSession(wsID); sess != nil {
		t.Fatalf("editor-only empty session should be gone, got %+v", sess)
	}
}

func TestEditorSessionPaneChromeOnlyIsKept(t *testing.T) {
	svc, wsID, dataDir := newEditorSessionService(t)

	pane := metaPatch(nil, "")
	pane.Meta.LeftKind = "findings"
	pane.Meta.RightKind = "scm"
	pane.Meta.LeftOpen = boolPtr(false)
	pane.Meta.RightOpen = boolPtr(false)
	if err := svc.UpdateEditorSession(wsID, pane); err != nil {
		t.Fatalf("pane-chrome patch: %v", err)
	}
	sess, err := svc.GetEditorSession(wsID)
	if err != nil || sess == nil {
		t.Fatalf("GetEditorSession = %+v, %v", sess, err)
	}
	if sess.LeftKind != "findings" || sess.RightKind != "scm" {
		t.Fatalf("kinds = %q / %q", sess.LeftKind, sess.RightKind)
	}
	if sess.LeftOpen == nil || *sess.LeftOpen || sess.RightOpen == nil || *sess.RightOpen {
		t.Fatalf("open flags = left %+v right %+v", sess.LeftOpen, sess.RightOpen)
	}
	if _, err := os.Stat(filepath.Join(dataDir, "workspaces", wsID, editorSessionFile)); err != nil {
		t.Fatalf("session file should remain: %v", err)
	}

	fresh := &EditorSessionService{base: svc.base}
	sess2, err := fresh.GetEditorSession(wsID)
	if err != nil || sess2 == nil || sess2.LeftKind != "findings" || sess2.RightKind != "scm" {
		t.Fatalf("fresh GetEditorSession = %+v, %v", sess2, err)
	}

	// Default kinds + both open is empty chrome — delete, like old sessions.
	gone := metaPatch(nil, "")
	gone.Meta.LeftKind = "maestro"
	gone.Meta.RightKind = "files"
	gone.Meta.LeftOpen = boolPtr(true)
	gone.Meta.RightOpen = boolPtr(true)
	if err := svc.UpdateEditorSession(wsID, gone); err != nil {
		t.Fatalf("default pane patch: %v", err)
	}
	if sess, _ := svc.GetEditorSession(wsID); sess != nil {
		t.Fatalf("default pane-only session should be gone, got %+v", sess)
	}

	// Unknown kind + default-open must not pin an otherwise empty session.
	bad := metaPatch(nil, "")
	bad.Meta.LeftKind = "nope"
	if err := svc.UpdateEditorSession(wsID, bad); err != nil {
		t.Fatalf("invalid kind patch: %v", err)
	}
	if sess, _ := svc.GetEditorSession(wsID); sess != nil {
		t.Fatalf("invalid kind should not keep an empty session, got %+v", sess)
	}
}

func boolPtr(v bool) *bool { return &v }

func TestEditorSessionEmptyMergeDeletes(t *testing.T) {
	svc, wsID, dataDir := newEditorSessionService(t)

	patch := metaPatch([]string{"a.go"}, "a.go")
	patch.Buffers = map[string]EditorBuffer{"a.go": {Draft: "X", BaseHash: "h"}}
	if err := svc.UpdateEditorSession(wsID, patch); err != nil {
		t.Fatalf("seed: %v", err)
	}

	// Closing the last tab: empty meta + explicit drop empties the session,
	// which must delete the file — a discarded draft must not resurrect.
	empty := metaPatch(nil, "")
	empty.DropBuffers = []string{"a.go"}
	if err := svc.UpdateEditorSession(wsID, empty); err != nil {
		t.Fatalf("empty patch: %v", err)
	}
	if sess, _ := svc.GetEditorSession(wsID); sess != nil {
		t.Fatalf("session should be gone, got %+v", sess)
	}
	if _, err := os.Stat(filepath.Join(dataDir, "workspaces", wsID, editorSessionFile)); !os.IsNotExist(err) {
		t.Fatalf("session file should be deleted, stat err = %v", err)
	}
}

func TestEditorSessionClear(t *testing.T) {
	svc, wsID, dataDir := newEditorSessionService(t)
	if err := svc.UpdateEditorSession(wsID, metaPatch([]string{"a.go"}, "a.go")); err != nil {
		t.Fatalf("seed: %v", err)
	}
	if err := svc.ClearEditorSession(wsID); err != nil {
		t.Fatalf("ClearEditorSession: %v", err)
	}
	if sess, _ := svc.GetEditorSession(wsID); sess != nil {
		t.Fatalf("session should be gone, got %+v", sess)
	}
	if _, err := os.Stat(filepath.Join(dataDir, "workspaces", wsID, editorSessionFile)); !os.IsNotExist(err) {
		t.Fatalf("session file should be deleted, stat err = %v", err)
	}
	// Clearing an already-clear session is a no-op success.
	if err := svc.ClearEditorSession(wsID); err != nil {
		t.Fatalf("ClearEditorSession(again): %v", err)
	}
}

func TestEditorSessionUnknownWorkspaceRefused(t *testing.T) {
	svc, _, dataDir := newEditorSessionService(t)
	if err := svc.UpdateEditorSession("nope", metaPatch([]string{"a.go"}, "a.go")); err == nil {
		t.Fatal("UpdateEditorSession(unknown) should error")
	}
	// The refusal must not have created a data dir for the phantom workspace.
	if _, err := os.Stat(filepath.Join(dataDir, "workspaces", "nope")); !os.IsNotExist(err) {
		t.Fatalf("phantom workspace dir created, stat err = %v", err)
	}
	// Get on an unknown workspace is just an absent session.
	if sess, err := svc.GetEditorSession("nope"); err != nil || sess != nil {
		t.Fatalf("GetEditorSession(unknown) = %v, %v", sess, err)
	}
}

func TestEditorSessionCorruptFileIsAbsent(t *testing.T) {
	svc, wsID, dataDir := newEditorSessionService(t)
	dir := filepath.Join(dataDir, "workspaces", wsID)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, editorSessionFile), []byte("not gzip"), 0o644); err != nil {
		t.Fatal(err)
	}
	if sess, err := svc.GetEditorSession(wsID); err != nil || sess != nil {
		t.Fatalf("corrupt session should read as absent, got %v, %v", sess, err)
	}
	// And a write recovers it.
	if err := svc.UpdateEditorSession(wsID, metaPatch([]string{"a.go"}, "a.go")); err != nil {
		t.Fatalf("recover write: %v", err)
	}
	if sess, _ := svc.GetEditorSession(wsID); sess == nil || sess.SelectedPath != "a.go" {
		t.Fatalf("recovered session = %+v", sess)
	}
}

func TestEditorSessionConcurrentPatches(t *testing.T) {
	svc, wsID, _ := newEditorSessionService(t)
	if err := svc.UpdateEditorSession(wsID, metaPatch([]string{"a.go"}, "a.go")); err != nil {
		t.Fatalf("seed: %v", err)
	}
	var wg sync.WaitGroup
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			patch := EditorSessionPatch{Buffers: map[string]EditorBuffer{
				string(rune('a'+i)) + ".txt": {Draft: "body", BaseHash: "h"},
			}}
			if err := svc.UpdateEditorSession(wsID, patch); err != nil {
				t.Errorf("concurrent patch %d: %v", i, err)
			}
		}(i)
	}
	wg.Wait()
	sess, err := svc.GetEditorSession(wsID)
	if err != nil || sess == nil {
		t.Fatalf("GetEditorSession = %v, %v", sess, err)
	}
	if len(sess.Buffers) != 8 {
		t.Fatalf("want 8 buffers after concurrent patches, got %d", len(sess.Buffers))
	}
}

func TestEditorSessionDoesNotAllowUndroppedAbsPath(t *testing.T) {
	t.Cleanup(code.ResetExternalPaths)
	svc, wsID, _ := newEditorSessionService(t)

	dir := t.TempDir()
	abs := filepath.Join(dir, "export-111.csv")
	if err := os.WriteFile(abs, []byte("a,b\n1,2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := svc.UpdateEditorSession(wsID, metaPatch([]string{abs, "src/main.go"}, abs)); err != nil {
		t.Fatalf("UpdateEditorSession: %v", err)
	}

	fresh := &EditorSessionService{base: svc.base}
	sess, err := fresh.GetEditorSession(wsID)
	if err != nil || sess == nil {
		t.Fatalf("GetEditorSession = %v, %v", sess, err)
	}
	if _, _, err := code.PrepareExternal(abs); err == nil || !strings.Contains(err.Error(), "path not allowed") {
		t.Fatalf("PrepareExternal after session restore = %v, want path not allowed", err)
	}
}

func TestEditorSessionClearDropsCache(t *testing.T) {
	svc, wsID, _ := newEditorSessionService(t)
	if err := svc.UpdateEditorSession(wsID, metaPatch([]string{"a.go"}, "a.go")); err != nil {
		t.Fatalf("seed: %v", err)
	}
	if err := svc.ClearEditorSession(wsID); err != nil {
		t.Fatalf("ClearEditorSession: %v", err)
	}
	svc.mu.Lock()
	_, ok := svc.states[wsID]
	svc.mu.Unlock()
	if ok {
		t.Fatal("states still holds workspace after ClearEditorSession")
	}
}

func TestEditorSessionDroppedOnWorkspaceDeleted(t *testing.T) {
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	eng := engine.New(store.NewInMemory())
	svcs := NewServices(eng)
	ws, err := eng.Store.AddWorkspace(domain.Workspace{ID: "WS-del", ProjectID: "p"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if err := svcs.EditorSession.UpdateEditorSession(ws.ID, metaPatch([]string{"a.go"}, "a.go")); err != nil {
		t.Fatalf("Update: %v", err)
	}
	svcs.EditorSession.mu.Lock()
	_, ok := svcs.EditorSession.states[ws.ID]
	svcs.EditorSession.mu.Unlock()
	if !ok {
		t.Fatal("expected cache entry after Update")
	}
	eng.Bus.Publish(engine.Event{Name: "workspace.deleted", Data: map[string]any{"workspaceId": ws.ID}})
	svcs.EditorSession.mu.Lock()
	_, ok = svcs.EditorSession.states[ws.ID]
	svcs.EditorSession.mu.Unlock()
	if ok {
		t.Fatal("states still holds workspace after workspace.deleted")
	}
}
