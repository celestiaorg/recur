package power

import (
	"os/exec"
	"runtime"
	"syscall"
	"testing"
	"time"
)

func TestKeeperSetTrueFalse(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("caffeinate is macOS-only")
	}
	if _, err := exec.LookPath("caffeinate"); err != nil {
		t.Skip("caffeinate not on PATH")
	}

	var k Keeper
	k.Set(true)
	if !k.Active() {
		t.Fatal("expected caffeinate running after Set(true)")
	}
	k.mu.Lock()
	pid := k.cmd.Process.Pid
	k.mu.Unlock()
	if err := syscall.Kill(pid, 0); err != nil {
		t.Fatalf("caffeinate pid %d not alive: %v", pid, err)
	}

	// Idempotent start should not replace the process.
	k.Set(true)
	k.mu.Lock()
	same := k.cmd != nil && k.cmd.Process.Pid == pid
	k.mu.Unlock()
	if !k.Active() || !same {
		t.Fatal("second Set(true) should keep the same caffeinate process")
	}

	k.Set(false)
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if !k.Active() {
			if err := syscall.Kill(pid, 0); err != nil {
				return
			}
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatalf("caffeinate pid %d still alive after Set(false)", pid)
}

func TestKeeperStopWhenNeverStarted(t *testing.T) {
	var k Keeper
	k.Stop() // must not panic
	if k.Active() {
		t.Fatal("Stop on zero Keeper should leave Active false")
	}
}

func TestKeeperCloseRefusesRestart(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("caffeinate is macOS-only")
	}
	if _, err := exec.LookPath("caffeinate"); err != nil {
		t.Skip("caffeinate not on PATH")
	}

	var k Keeper
	k.Set(true)
	if !k.Active() {
		t.Fatal("expected caffeinate running after Set(true)")
	}
	k.Close()
	deadline := time.Now().Add(3 * time.Second)
	for k.Active() && time.Now().Before(deadline) {
		time.Sleep(50 * time.Millisecond)
	}
	if k.Active() {
		t.Fatal("expected inactive after Close")
	}
	k.Set(true)
	if k.Active() {
		t.Fatal("Set(true) after Close must not respawn caffeinate")
	}
}
