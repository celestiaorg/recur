package git

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestChangeStatusCoalescesConcurrentCalls(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	root := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root}, args...)...)
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	if err := os.WriteFile(filepath.Join(root, "base.go"), []byte("package b\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "base")
	run("checkout", "-b", "recur/feat")
	if err := os.WriteFile(filepath.Join(root, "feat.go"), []byte("package f\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "feat")

	var vsBase atomic.Int32
	statusComputeHook = func(kind string) {
		if kind == "vs-base" {
			vsBase.Add(1)
			time.Sleep(80 * time.Millisecond)
		}
	}
	t.Cleanup(func() { statusComputeHook = nil })

	const n = 8
	results := make([]map[string]string, n)
	var wg sync.WaitGroup
	for i := range n {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			results[i] = ChangeStatus(context.Background(), root, "main")
		}(i)
	}
	wg.Wait()

	if got := vsBase.Load(); got != 1 {
		t.Fatalf("ChangeStatus concurrent git work ran %d times, want 1", got)
	}
	for i, st := range results {
		if st["feat.go"] != "a" {
			t.Errorf("ChangeStatus(root, main) goroutine %d = %#v, want feat.go=a", i, st)
		}
	}

	// After the flight ends, a later call must recompute (no stale cache).
	_ = ChangeStatus(context.Background(), root, "main")
	if got := vsBase.Load(); got != 2 {
		t.Fatalf("ChangeStatus after flight = %d git runs, want 2", got)
	}
}

func TestStatusMapCoalescesConcurrentCalls(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	root := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root}, args...)...)
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\nfunc F() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	var porcelain atomic.Int32
	statusComputeHook = func(kind string) {
		if kind == "porcelain" {
			porcelain.Add(1)
			time.Sleep(80 * time.Millisecond)
		}
	}
	t.Cleanup(func() { statusComputeHook = nil })

	const n = 8
	results := make([]map[string]string, n)
	var wg sync.WaitGroup
	for i := range n {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			results[i] = StatusMap(context.Background(), root)
		}(i)
	}
	wg.Wait()

	if got := porcelain.Load(); got != 1 {
		t.Fatalf("StatusMap concurrent git work ran %d times, want 1", got)
	}
	for i, st := range results {
		if st["a.go"] != "m" {
			t.Errorf("StatusMap(root) goroutine %d = %#v, want a.go=m", i, st)
		}
	}
}

func TestDiscardFileDoesNotJoinInFlightStatusMap(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	root := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root}, args...)...)
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")

	enteredHold := make(chan struct{})
	release := make(chan struct{})
	statusComputeHook = func(kind string) {
		if kind == "flight-hold" {
			close(enteredHold)
			<-release
		}
	}
	t.Cleanup(func() {
		statusComputeHook = nil
		select {
		case <-release:
		default:
			close(release)
		}
	})

	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		_ = StatusMap(context.Background(), root)
	}()

	select {
	case <-enteredHold:
	case <-time.After(5 * time.Second):
		t.Fatal("StatusMap never reached flight-hold")
	}

	fresh := filepath.Join(root, "fresh.go")
	if err := os.WriteFile(fresh, []byte("package f\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	done := make(chan error, 1)
	go func() {
		done <- DiscardFile(context.Background(), root, "fresh.go")
	}()

	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("DiscardFile: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("DiscardFile blocked on coalesced StatusMap")
	}

	if _, err := os.Stat(fresh); !os.IsNotExist(err) {
		t.Fatalf("expected DiscardFile to remove fresh.go, stat=%v", err)
	}

	close(release)
	wg.Wait()
}
