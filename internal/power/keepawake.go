// Package power manages machine power assertions (macOS caffeinate).
package power

import (
	"log"
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"sync"
	"syscall"
	"time"
)

// keepAwakeKillGrace matches other Recur subprocess teardown (bridge / claude):
// SIGTERM, brief wait, then SIGKILL the process group.
const keepAwakeKillGrace = 2 * time.Second

// Keeper holds a single macOS caffeinate assertion while Set(true). On non-darwin
// platforms every method is a no-op. Safe for concurrent Set/Stop/Close.
//
// caffeinate is started with `-w <recur-pid>` so the assertion self-releases if
// Recur crashes or is force-quit (any path that skips Close). Close marks the
// Keeper terminal so a late Set(true) during Shutdown cannot respawn it.
type Keeper struct {
	mu     sync.Mutex
	cmd    *exec.Cmd
	done   chan struct{} // closed when Wait returns for cmd; nil when idle
	closed bool
}

// Set starts or stops the assertion to match want. Idempotent. After Close,
// Set(true) is a no-op and Set(false) still drops any leftover child.
func (k *Keeper) Set(want bool) {
	if runtime.GOOS != "darwin" {
		return
	}
	k.mu.Lock()
	if k.closed {
		want = false
	}
	if want {
		k.startLocked()
		k.mu.Unlock()
		return
	}
	proc, done := k.takeLocked()
	k.mu.Unlock()
	k.kill(proc, done)
}

// Stop drops any held assertion. Equivalent to Set(false). Does not Close.
func (k *Keeper) Stop() { k.Set(false) }

// Close permanently disables the Keeper and drops any held assertion. Subsequent
// Set(true) calls are ignored — used by Engine.Shutdown so a late agent release
// cannot respawn caffeinate as the process exits.
func (k *Keeper) Close() {
	k.mu.Lock()
	k.closed = true
	if runtime.GOOS != "darwin" {
		k.mu.Unlock()
		return
	}
	proc, done := k.takeLocked()
	k.mu.Unlock()
	k.kill(proc, done)
}

// Active reports whether a caffeinate child is currently tracked. Tests only.
func (k *Keeper) Active() bool {
	k.mu.Lock()
	defer k.mu.Unlock()
	return k.aliveLocked()
}

func (k *Keeper) startLocked() {
	if k.closed {
		return
	}
	if k.aliveLocked() {
		return
	}
	k.cmd = nil
	k.done = nil
	// -w tethers the assertion to this process: if Recur dies without Close,
	// caffeinate exits and idle-sleep can resume.
	cmd := exec.Command("caffeinate", "-i", "-w", strconv.Itoa(os.Getpid()))
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		log.Printf("recur: warning: start caffeinate: %v", err)
		return
	}
	done := make(chan struct{})
	k.cmd = cmd
	k.done = done
	go func() {
		_ = cmd.Wait()
		close(done)
		k.mu.Lock()
		if k.cmd == cmd {
			k.cmd = nil
			k.done = nil
		}
		k.mu.Unlock()
	}()
}

// aliveLocked reports whether the tracked child is still running, using the
// Wait-done channel rather than kill(pid, 0) so a recycled pid cannot look live.
func (k *Keeper) aliveLocked() bool {
	if k.cmd == nil || k.done == nil {
		k.cmd = nil
		k.done = nil
		return false
	}
	select {
	case <-k.done:
		k.cmd = nil
		k.done = nil
		return false
	default:
		return true
	}
}

// takeLocked clears the tracked child and returns it for teardown outside k.mu.
func (k *Keeper) takeLocked() (proc *os.Process, done <-chan struct{}) {
	if !k.aliveLocked() {
		return nil, nil
	}
	proc = k.cmd.Process
	done = k.done
	k.cmd = nil
	k.done = nil
	return proc, done
}

// kill signals the process group without holding k.mu. Waits for Wait to finish
// asynchronously so agent acquire/release and settings saves are not stalled.
// SIGKILL is only sent while done is still open, so a reaped (and possibly
// recycled) pid is never signalled.
func (k *Keeper) kill(proc *os.Process, done <-chan struct{}) {
	if proc == nil {
		return
	}
	if done != nil {
		select {
		case <-done:
			return
		default:
		}
	}
	pgid := proc.Pid // Setpgid → pgid == pid
	_ = syscall.Kill(-pgid, syscall.SIGTERM)
	if done == nil {
		return
	}
	go func() {
		select {
		case <-done:
			return
		case <-time.After(keepAwakeKillGrace):
		}
		select {
		case <-done:
			return
		default:
			_ = syscall.Kill(-pgid, syscall.SIGKILL)
			<-done
		}
	}()
}
