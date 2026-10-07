package term

import (
	"errors"
	"fmt"
	"syscall"
	"time"

	"golang.org/x/sys/unix"
)

// interruptGrace matches the rest of Recur's subprocess teardown (dev-cmd
// validate, claude, the cursor bridge): SIGINT, wait, SIGTERM, wait, SIGKILL.
const interruptGrace = 2 * time.Second

const interruptPoll = 20 * time.Millisecond

// Interrupt stops the session's foreground process group without killing the
// shell. Writing ^C through the PTY only delivers SIGINT when the tty has ISIG
// set; a dev server in raw mode (vite's interactive keys) swallows the byte.
// Signalling the foreground pgrp directly bypasses termios, then escalates to
// SIGTERM / SIGKILL if that group is still alive. Success means kill(-pgid, 0)
// returns ESRCH — the shell reclaiming the tty is not enough, because a wrapper
// can exit while its children still hold the port. The shell's own pgrp is
// never SIGKILL'd — that's Close's job.
func (m *Manager) Interrupt(id string) error {
	s, err := m.get(id)
	if err != nil {
		return err
	}
	err = s.interruptForeground()
	s.kickIdleWatch()
	return err
}

func (s *Session) interruptForeground() error {
	if s.cmd.Process == nil {
		return nil
	}
	shellPgid := s.cmd.Process.Pid
	fg, ok := s.foregroundPgid()
	if !ok || isShellForeground(fg, shellPgid) {
		return nil
	}
	target := fg
	s.signalGroup(target, syscall.SIGINT)
	if s.waitUntilGroupGone(target, interruptGrace) {
		return nil
	}
	s.signalGroup(target, syscall.SIGTERM)
	if s.waitUntilGroupGone(target, interruptGrace) {
		return nil
	}
	s.signalGroup(target, syscall.SIGKILL)
	if s.waitUntilGroupGone(target, 500*time.Millisecond) {
		return nil
	}
	return fmt.Errorf("terminal: process group %d still running after interrupt", target)
}

func isShellForeground(fg, shellPgid int) bool {
	return fg <= 1 || fg == shellPgid
}

func (s *Session) signalGroup(pgid int, sig syscall.Signal) {
	if pgid <= 1 {
		return
	}
	_ = syscall.Kill(-pgid, sig)
}

func groupGone(pgid int) bool {
	if pgid <= 1 {
		return true
	}
	err := syscall.Kill(-pgid, 0)
	return errors.Is(err, syscall.ESRCH)
}

func (s *Session) waitUntilGroupGone(target int, d time.Duration) bool {
	deadline := time.Now().Add(d)
	for {
		if s.isClosed() || groupGone(target) {
			return true
		}
		if !time.Now().Before(deadline) {
			return false
		}
		time.Sleep(interruptPoll)
	}
}

func (s *Session) isClosed() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.closed
}

func (s *Session) foregroundPgid() (int, bool) {
	s.mu.Lock()
	ptmx := s.ptmx
	closed := s.closed
	s.mu.Unlock()
	if closed || ptmx == nil {
		return 0, false
	}
	pgid, err := unix.IoctlGetInt(int(ptmx.Fd()), unix.TIOCGPGRP)
	if err != nil || pgid <= 1 {
		return 0, false
	}
	return pgid, true
}

const idlePoll = 25 * time.Millisecond
const idlePollMax = time.Second

func (s *Session) kickIdleWatch() {
	if s.idleKick == nil {
		return
	}
	select {
	case s.idleKick <- struct{}{}:
	default:
	}
}

// watchIdle emits term.idle each time a foreground command returns to the
// login shell, so Recur can mark a run stopped without killing the tab.
// While the session is quiet the poll backs off (up to idlePollMax), including
// while a long-running command owns the tty. A Write or Interrupt kicks it
// back to idlePoll so a new command (or a Stop) is noticed quickly.
func (s *Session) watchIdle() {
	for s.cmd == nil || s.cmd.Process == nil {
		if s.isClosed() {
			return
		}
		time.Sleep(idlePoll)
	}
	shellPgid := s.cmd.Process.Pid
	sawCmd := false
	delay := idlePoll
	for {
		if s.isClosed() {
			return
		}
		fg, ok := s.foregroundPgid()
		if ok && !isShellForeground(fg, shellPgid) {
			if !sawCmd {
				s.emit("term.busy", map[string]any{"id": s.id, "workspaceId": s.workspaceID})
			}
			sawCmd = true
		} else if ok && sawCmd {
			s.emit("term.idle", map[string]any{"id": s.id, "workspaceId": s.workspaceID})
			sawCmd = false
		}
		if delay < idlePollMax {
			next := delay * 2
			if next > idlePollMax {
				next = idlePollMax
			}
			delay = next
		}
		timer := time.NewTimer(delay)
		select {
		case <-s.idleKick:
			if !timer.Stop() {
				select {
				case <-timer.C:
				default:
				}
			}
			delay = idlePoll
		case <-timer.C:
		}
	}
}
