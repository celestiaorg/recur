package code

import (
	"context"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"
)

func TestFileIndexUpsertRemove(t *testing.T) {
	idx := NewFileIndex(t.TempDir())
	idx.Upsert("validators/decode-hyperlane-message.sh")
	idx.Upsert("validators/hyperlane_validator2.sh")
	idx.Upsert("README.md")

	snap := idx.Snapshot()
	if len(snap.Paths) != 3 {
		t.Fatalf("Snapshot().Paths len = %d, want 3", len(snap.Paths))
	}
	if snap.Bytes <= 0 {
		t.Fatalf("Snapshot().Bytes = %d, want > 0", snap.Bytes)
	}

	idx.Remove("README.md")
	snap = idx.Snapshot()
	if len(snap.Paths) != 2 {
		t.Fatalf("after Remove: Paths len = %d, want 2", len(snap.Paths))
	}
	seen := map[string]bool{}
	for _, p := range snap.Paths {
		seen[p] = true
	}
	if !seen["validators/hyperlane_validator2.sh"] || !seen["validators/decode-hyperlane-message.sh"] {
		t.Fatalf("paths = %v, want both hyperlane validators", snap.Paths)
	}
}

func TestFileIndexApplyFSChange(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "validators")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "hyperlane_validator2.sh")
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	idx := NewFileIndex(root)
	idx.ApplyFSChange(root, "validators/hyperlane_validator2.sh")
	snap := idx.Snapshot()
	if len(snap.Paths) != 1 || snap.Paths[0] != "validators/hyperlane_validator2.sh" {
		t.Fatalf("after create: %v", snap.Paths)
	}

	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	idx.ApplyFSChange(root, "validators/hyperlane_validator2.sh")
	if got := idx.Snapshot().Paths; len(got) != 0 {
		t.Fatalf("after delete: %v", got)
	}
}

func TestIndexHubRebuildWalkFallbackIncludesValidators(t *testing.T) {
	root := t.TempDir()
	late := filepath.Join(root, "validators")
	if err := os.MkdirAll(late, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(late, "decode-hyperlane-message.sh"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(late, "hyperlane_validator2.sh"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "hypothesis.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}

	done := make(chan struct{}, 1)
	hub := NewIndexHub(t.TempDir(), func(_ string, snap Snapshot) {
		if snap.Ready && !snap.Building {
			select {
			case done <- struct{}{}:
			default:
			}
		}
	})
	hub.SetRgPath("/nonexistent/rg-binary")
	hub.Refresh(context.Background(), "ws1", root)

	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("timeout waiting for index rebuild")
	}
	paths := hub.Ensure(context.Background(), "ws1", root).Paths
	seen := map[string]bool{}
	for _, p := range paths {
		seen[p] = true
	}
	for _, want := range []string{
		"validators/hyperlane_validator2.sh",
		"validators/decode-hyperlane-message.sh",
		"hypothesis.txt",
	} {
		if !seen[want] {
			t.Fatalf("paths = %v, missing %q", paths, want)
		}
	}
}

func TestIndexHubApplyChangesMarksDirtyInsteadOfRebuild(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	hub := NewIndexHub(t.TempDir(), nil)
	hub.SetRgPath("/nonexistent/rg-binary")

	done := make(chan struct{}, 1)
	hub.publish = func(_ string, snap Snapshot) {
		if snap.Ready && !snap.Building {
			select {
			case done <- struct{}{}:
			default:
			}
		}
	}
	hub.Refresh(context.Background(), "ws1", root)
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("timeout on initial rebuild")
	}

	hub.ApplyChanges(context.Background(), "ws1", root, nil, true)
	snap := hub.indexes["ws1"].Snapshot()
	if !snap.Dirty {
		t.Fatal("rescan should mark dirty, not clear it")
	}
	if snap.Building {
		t.Fatal("rescan must not start a rebuild")
	}

	// Cold workspace: FS noise must not create an index.
	hub.ApplyChanges(context.Background(), "ws-cold", root, []string{"a.txt"}, false)
	if hub.Len() != 1 {
		t.Fatalf("Len = %d, want 1 (cold untouched)", hub.Len())
	}
}

func TestIndexHubRefreshIfDirtySkipsCleanAndCold(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	hub := NewIndexHub(t.TempDir(), nil)
	hub.SetRgPath("/nonexistent/rg-binary")

	done := make(chan struct{}, 1)
	hub.publish = func(_ string, snap Snapshot) {
		if snap.Ready && !snap.Building {
			select {
			case done <- struct{}{}:
			default:
			}
		}
	}
	hub.Refresh(context.Background(), "ws1", root)
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("timeout on initial rebuild")
	}

	builds := 0
	hub.publish = func(_ string, snap Snapshot) {
		if snap.Ready && !snap.Building {
			builds++
		}
	}
	hub.RefreshIfDirty(context.Background(), "ws1", root)
	if builds != 0 {
		t.Fatalf("clean RefreshIfDirty started %d rebuilds", builds)
	}
	cold := hub.RefreshIfDirty(context.Background(), "ws-cold", root)
	if cold.Ready || hub.Len() != 1 {
		t.Fatalf("cold RefreshIfDirty should no-op, snap=%+v len=%d", cold, hub.Len())
	}

	hub.MarkDirty("ws1")
	hub.RefreshIfDirty(context.Background(), "ws1", root)
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if !hub.indexes["ws1"].Snapshot().Dirty && hub.indexes["ws1"].Snapshot().Ready {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("dirty RefreshIfDirty did not clear dirty")
}

func TestIndexHubByteBudgetEvictsLRUNotPinned(t *testing.T) {
	hub := NewIndexHub(t.TempDir(), nil)
	hub.SetRgPath("/nonexistent/rg-binary")

	mk := func(id string) string {
		root := t.TempDir()
		// Enough path bytes to matter under a tiny budget.
		for i := 0; i < 40; i++ {
			name := filepath.Join(root, "file-with-a-fairly-long-name-"+string(rune('a'+i%26))+".txt")
			_ = os.WriteFile(name, []byte("x"), 0o644)
		}
		done := make(chan struct{}, 1)
		hub.publish = func(ws string, snap Snapshot) {
			if ws == id && snap.Ready {
				select {
				case done <- struct{}{}:
				default:
				}
			}
		}
		hub.Refresh(context.Background(), id, root)
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatalf("timeout building %s", id)
		}
		return root
	}

	mk("ws-old")
	time.Sleep(5 * time.Millisecond)
	mk("ws-pin")
	time.Sleep(5 * time.Millisecond)
	hub.SetPinned("ws-pin")

	// Tiny budget: force eviction of the colder unpinned index.
	hub.SetMaxBytes(1)
	if hub.Len() != 1 {
		t.Fatalf("after budget=1: Len = %d, want 1", hub.Len())
	}
	if _, ok := hub.indexes["ws-pin"]; !ok {
		t.Fatal("pinned workspace was evicted")
	}
	if _, ok := hub.indexes["ws-old"]; ok {
		t.Fatal("LRU unpinned workspace should be evicted")
	}
}

func TestFileIndexRebuildCoalesce(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	idx := NewFileIndex(root)
	var resolves atomic.Int32
	started := make(chan struct{}, 1)
	release := make(chan struct{})
	resolve := func(context.Context) string {
		resolves.Add(1)
		select {
		case started <- struct{}{}:
		default:
		}
		<-release
		return "/nonexistent/rg-binary"
	}
	idx.Rebuild(context.Background(), resolve, nil, false, false, nil, excludeSet{})
	select {
	case <-started:
	case <-time.After(2 * time.Second):
		t.Fatal("first rebuild did not start")
	}
	// Non-force while building must not start a second resolveRg.
	idx.Rebuild(context.Background(), resolve, nil, false, false, nil, excludeSet{})
	if got := resolves.Load(); got != 1 {
		close(release)
		t.Fatalf("resolveRg calls = %d, want 1 (coalesced)", got)
	}
	hub := NewIndexHub(t.TempDir(), nil)
	hub.SetRgPath("/nonexistent/rg-binary")
	hub.indexes["ws1"] = idx
	snap := hub.Ensure(context.Background(), "ws1", root)
	if !snap.Building {
		close(release)
		t.Fatal("Ensure should return in-flight snapshot without restarting")
	}
	if got := resolves.Load(); got != 1 {
		close(release)
		t.Fatalf("Ensure restarted build: resolveRg calls = %d", got)
	}
	close(release)
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if idx.Snapshot().Ready {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("coalesced rebuild never finished")
}

func TestFileIndexFinishRebuildKeepsDirtyOnRace(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	idx := NewFileIndex(root)
	var resolves atomic.Int32
	started := make(chan struct{}, 1)
	release := make(chan struct{})
	resolve := func(context.Context) string {
		resolves.Add(1)
		select {
		case started <- struct{}{}:
		default:
		}
		<-release
		return "/nonexistent/rg-binary"
	}
	idx.Rebuild(context.Background(), resolve, nil, false, true, nil, excludeSet{})
	select {
	case <-started:
	case <-time.After(2 * time.Second):
		t.Fatal("rebuild did not start")
	}
	// Mid-walk FS signal — must not be erased when the walk finishes.
	idx.MarkDirty()
	if err := os.WriteFile(filepath.Join(root, "b.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	idx.ApplyFSChange(root, "b.txt")
	close(release)

	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		snap := idx.Snapshot()
		if snap.Ready && !snap.Building {
			if !snap.Dirty {
				t.Fatal("finishRebuild cleared dirty after mid-walk MarkDirty/ApplyFSChange")
			}
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("rebuild never finished")
}

func TestFileIndexRebuildSurvivesCanceledParentContext(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	idx := NewFileIndex(root)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{}, 1)
	idx.Rebuild(ctx, func(context.Context) string { return "/nonexistent/rg-binary" }, func(snap Snapshot) {
		if snap.Ready && !snap.Building {
			select {
			case done <- struct{}{}:
			default:
			}
		}
	}, false, true, nil, excludeSet{})
	cancel() // simulate Wails RPC return
	select {
	case <-done:
		snap := idx.Snapshot()
		if len(snap.Paths) != 1 || snap.Paths[0] != "a.txt" {
			t.Fatalf("paths = %v", snap.Paths)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("rebuild died with parent context cancel")
	}
}

func TestIndexHubApplyChangesSplice(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	hub := NewIndexHub(t.TempDir(), nil)
	hub.SetRgPath("/nonexistent/rg-binary")

	done := make(chan struct{}, 1)
	hub.publish = func(_ string, snap Snapshot) {
		if snap.Ready {
			select {
			case done <- struct{}{}:
			default:
			}
		}
	}
	hub.Refresh(context.Background(), "ws1", root)
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("timeout on initial rebuild")
	}

	if err := os.WriteFile(filepath.Join(root, "b.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	hub.ApplyChanges(context.Background(), "ws1", root, []string{"b.txt"}, false)
	snap := hub.Ensure(context.Background(), "ws1", root)
	seen := map[string]bool{}
	for _, p := range snap.Paths {
		seen[p] = true
	}
	if !seen["a.txt"] || !seen["b.txt"] {
		t.Fatalf("after splice: %v", snap.Paths)
	}
}

func TestIndexHubSetExcludeFiltersListing(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "dist"), 0o755); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"src.go", "dist/bundle.js", ".DS_Store"} {
		dir := filepath.Join(root, filepath.Dir(name))
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(root, name), []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	hub := NewIndexHub(t.TempDir(), nil)
	hub.SetRgPath("/nonexistent/rg-binary")
	hub.SetExclude([]string{"dist/**", "**/.DS_Store"})

	done := make(chan struct{}, 1)
	hub.publish = func(_ string, snap Snapshot) {
		if snap.Ready && !snap.Building {
			select {
			case done <- struct{}{}:
			default:
			}
		}
	}
	hub.Refresh(context.Background(), "ws1", root)
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("timeout waiting for rebuild")
	}
	snap := hub.Ensure(context.Background(), "ws1", root)
	for _, p := range snap.Paths {
		if p == "dist/bundle.js" || p == ".DS_Store" {
			t.Fatalf("excluded path in index: %v", snap.Paths)
		}
	}
	if len(snap.Paths) != 1 || snap.Paths[0] != "src.go" {
		t.Fatalf("paths=%v want [src.go]", snap.Paths)
	}
}
