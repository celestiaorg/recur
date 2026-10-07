package git

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"strings"
	"syscall"
	"time"
)

// gitCancelGrace is how long a cancelled git process has after SIGTERM before
// os/exec SIGKILLs the group leader. Matches other Recur subprocess teardown.
const gitCancelGrace = 2 * time.Second

// Cmd is a git subprocess configured by Command. Output, CombinedOutput, Run,
// and Wait apply waitErr so a successful git exit whose inherited pipes are
// still held open (ssh ControlMaster) is not reported as failure. Other
// *exec.Cmd fields (Dir, Env, Stdout) remain available via embedding.
type Cmd struct {
	*exec.Cmd
}

// Command returns a git subprocess that will not take optional index locks
// (so a cancelled `git status` / `git diff` cannot leave .git/index.lock).
//
// On context cancel it SIGTERMs the process group (Setpgid) so hook
// grandchildren — `git commit` → pre-commit → `git add` — can unlink required
// locks. os/exec then SIGKILLs the group leader after WaitDelay if it has not
// exited. WaitDelay also bounds the wait for inherited stdout/stderr after a
// clean exit (ssh ControlMaster); Output/CombinedOutput/Run/Wait map that timeout
// to success.
func Command(ctx context.Context, args ...string) *Cmd {
	return commandWithEnv(ctx, nil, args...)
}

func commandWithEnv(ctx context.Context, extraEnv []string, args ...string) *Cmd {
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Env = gitEnviron(extraEnv)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error {
		err := syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM)
		if err == syscall.ESRCH {
			return os.ErrProcessDone
		}
		return err
	}
	cmd.WaitDelay = gitCancelGrace
	return &Cmd{Cmd: cmd}
}

func gitEnviron(extra []string) []string {
	env := os.Environ()
	out := make([]string, 0, len(env)+1+len(extra))
	for _, e := range env {
		if strings.HasPrefix(e, "GIT_OPTIONAL_LOCKS=") {
			continue
		}
		out = append(out, e)
	}
	out = append(out, "GIT_OPTIONAL_LOCKS=0")
	for _, e := range extra {
		if strings.HasPrefix(e, "GIT_OPTIONAL_LOCKS=") {
			continue
		}
		out = append(out, e)
	}
	return out
}

// waitErr maps os/exec's post-exit pipe timeout to success when git itself
// exited 0. WaitDelay bounds both SIGKILL-after-cancel and waiting for
// inherited stdout/stderr to close. ssh ControlMaster/ControlPersist leaves a
// background ssh holding git's stderr, so a successful push/fetch would
// otherwise return ErrWaitDelay after gitCancelGrace.
func waitErr(cmd *exec.Cmd, err error) error {
	if err == nil {
		return nil
	}
	if errors.Is(err, exec.ErrWaitDelay) && cmd != nil && cmd.ProcessState != nil && cmd.ProcessState.Success() {
		return nil
	}
	return err
}

// Output returns git's stdout. A successful exit whose inherited pipes are
// still held open (ssh ControlMaster) is reported as success, not ErrWaitDelay.
func (c *Cmd) Output() ([]byte, error) {
	out, err := c.Cmd.Output()
	return out, waitErr(c.Cmd, err)
}

// CombinedOutput returns git's stdout and stderr combined. A successful exit
// whose inherited pipes are still held open is reported as success.
func (c *Cmd) CombinedOutput() ([]byte, error) {
	out, err := c.Cmd.CombinedOutput()
	return out, waitErr(c.Cmd, err)
}

// Run waits for git to exit. A successful exit whose inherited pipes are
// still held open is reported as success.
func (c *Cmd) Run() error {
	return waitErr(c.Cmd, c.Cmd.Run())
}

// Wait waits for git to exit after Start. A successful exit whose inherited
// pipes are still held open is reported as success.
func (c *Cmd) Wait() error {
	return waitErr(c.Cmd, c.Cmd.Wait())
}
