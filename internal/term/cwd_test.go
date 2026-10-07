package term

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

func TestProcessCwdReadsChildDirectory(t *testing.T) {
	dir, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command("sleep", "30")
	cmd.Dir = dir
	if err := cmd.Start(); err != nil {
		t.Fatalf("start sleep: %v", err)
	}
	defer func() { _ = cmd.Process.Kill(); _ = cmd.Wait() }()

	// Each lsof attempt is capped at 400ms; under a loaded test run several
	// in a row can miss it.
	deadline := time.Now().Add(15 * time.Second)
	var got string
	for time.Now().Before(deadline) {
		got = processCwd(cmd.Process.Pid)
		if sameFile(got, dir) {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("processCwd = %q, want %q", got, dir)
}

func sameFile(a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	ra, err := os.Stat(a)
	if err != nil {
		return false
	}
	rb, err := os.Stat(b)
	if err != nil {
		return false
	}
	return os.SameFile(ra, rb)
}
