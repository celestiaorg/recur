// Package runconfig boots project dev servers and runs one-shot setup/teardown
// commands: allocate a free port, start the process in its own group, probe the
// app URL, and tear the group down. Boot keeps the server for verify-in-browser
// (PRD §10); RunCmd is for setup_cmd / teardown_cmd.
package runconfig

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

// Default budgets for a Boot probe. Exposed as BootOptions fields for tests.
const (
	defaultProbeTimeout = 15 * time.Second
	defaultPollInterval = 300 * time.Millisecond
	defaultKillGrace    = 2 * time.Second
)

// BootOptions configures a long-lived dev-server boot for verify-in-browser
// (PRD §10). The zero value uses sane defaults.
// Setup belongs on workspace create (engine), not here.
type BootOptions struct {
	WorkDir      string        // dir to run dev_cmd in (the loop's worktree)
	Port         int           // persisted workspace port; 0 = allocate a fresh one
	Env          []string      // full environment for the dev_cmd (nil = os.Environ + port)
	ProbeTimeout time.Duration // total time to wait for the URL to respond
	PollInterval time.Duration // gap between URL probes
}

func (o BootOptions) probeTimeout() time.Duration {
	if o.ProbeTimeout > 0 {
		return o.ProbeTimeout
	}
	return defaultProbeTimeout
}

func (o BootOptions) pollInterval() time.Duration {
	if o.PollInterval > 0 {
		return o.PollInterval
	}
	return defaultPollInterval
}

// Booted is a running dev server: the resolved $RECUR_PORT, the probed app URL,
// and a Stop that tears the WHOLE process group down (idempotent). The caller MUST
// call Stop (defer it) — the verify phase keeps the server up while the QA agent
// drives it, then stops it. Stop never double-Waits and never leaks (it reuses the
// proven close-broadcast reap pattern).
type Booted struct {
	Port   string
	AppURL string
	stop   func()
}

// Stop tears the dev server's process group down. Idempotent and safe to defer.
func (b *Booted) Stop() {
	if b != nil && b.stop != nil {
		b.stop()
	}
}

// Boot boots the dev server on $RECUR_PORT (opts.Port when set, else a freshly
// allocated port) and polls the app URL until it responds — returning a Booted
// handle whose Stop tears the whole process group down. Boot KEEPS the server
// running so the QA agent can drive it (PRD §10); the caller owns teardown via
// Booted.Stop. Process-group + single-reaper + SIGTERM/grace/SIGKILL teardown
// means it never hangs past the probe timeout and never leaks a process.
//
// Project setup_cmd runs on worktree create (engine), not here.
//
// On any failure (no dev_cmd, port alloc failed, server exited or never served) it
// tears down anything it started and returns an error; the returned *Booted is
// nil on error.
func Boot(ctx context.Context, devCmd, appURL string, opts BootOptions) (*Booted, error) {
	if strings.TrimSpace(devCmd) == "" {
		return nil, fmt.Errorf("no dev_cmd configured")
	}

	port := opts.Port
	if port <= 0 {
		var err error
		port, err = FreePort()
		if err != nil {
			return nil, fmt.Errorf("allocate port: %w", err)
		}
	}
	portStr := strconv.Itoa(port)

	probeURL := substitutePort(appURL, portStr)
	if strings.TrimSpace(probeURL) == "" {
		probeURL = "http://localhost:" + portStr
	}

	// Boot the dev server in its own process group so we can kill children.
	// Use plain exec.Command (NOT CommandContext): a successful Boot returns the
	// RUNNING server to the caller, whose ctx (e.g. verify's bootCtx) is cancelled
	// right after. CommandContext would arm a watchdog that SIGKILLs only the leader
	// on that cancel — killing the server before the QA agent can drive it AND
	// orphaning its children. The server's lifetime is owned solely by Stop()/
	// killGroup (SIGTERM→grace→SIGKILL on the whole group); ctx cancellation during
	// the probe is still handled by the <-ctx.Done() case below. (claude.go uses
	// plain exec.Command for the agent subprocess for the same reason.)
	cmd := exec.Command("/bin/sh", "-c", devCmd)
	cmd.Dir = opts.WorkDir
	if opts.Env != nil {
		cmd.Env = opts.Env
	} else {
		cmd.Env = append(os.Environ(), "RECUR_PORT="+portStr, "PORT="+portStr)
	}
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("start dev server: %w", err)
	}

	// Single reaper owns cmd.Wait() (the proven close-broadcast pattern): both the
	// probe select and Stop observe the same close without racing a second wait4.
	reaped := make(chan struct{})
	var waitErr error
	go func() { waitErr = cmd.Wait(); close(reaped) }()

	// stop is idempotent: sync.Once guards the single group teardown so deferring it
	// AND calling it on a probe-failure path can't double-signal/double-wait.
	var stopOnce sync.Once
	stop := func() { stopOnce.Do(func() { killGroup(cmd, reaped) }) }

	// 3) Poll the app URL until it responds (or budget/crash/cancel). On any failure
	// tear the server down before returning so we never leak.
	deadline := time.Now().Add(opts.probeTimeout())
	client := &http.Client{Timeout: 2 * time.Second}
	ticker := time.NewTicker(opts.pollInterval())
	defer ticker.Stop()

	for {
		if _, ok := probe(client, probeURL); ok {
			return &Booted{Port: portStr, AppURL: probeURL, stop: stop}, nil
		}
		select {
		case <-ctx.Done():
			stop()
			return nil, ctx.Err()
		case <-reaped:
			stop()
			detail := "dev server exited before serving"
			if waitErr != nil {
				detail += ": " + waitErr.Error()
			}
			return nil, fmt.Errorf("%s", detail)
		case <-ticker.C:
			if time.Now().After(deadline) {
				stop()
				return nil, fmt.Errorf("no HTTP response at %s within %s", probeURL, opts.probeTimeout())
			}
		}
	}
}

const (
	// cmdOutputTailCap bounds the combined stdout/stderr tail from RunCmdOutput.
	cmdOutputTailCap = 4 * 1024
	// cmdOutputWaitDelay is how long to wait for the shell's last bytes to land
	// in the tail after it exits. Descendants that still hold the pipe keep a
	// drain goroutine instead of having the pipe closed (which would SIGPIPE them).
	cmdOutputWaitDelay = 100 * time.Millisecond
)

// tailWriter keeps the last max bytes written to it.
type tailWriter struct {
	mu  sync.Mutex
	buf []byte
	max int
}

func (w *tailWriter) Write(p []byte) (int, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	n := len(p)
	if w.max <= 0 {
		return n, nil
	}
	if n >= w.max {
		w.buf = append(w.buf[:0], p[n-w.max:]...)
		return n, nil
	}
	if extra := len(w.buf) + n - w.max; extra > 0 {
		w.buf = w.buf[extra:]
	}
	w.buf = append(w.buf, p...)
	return n, nil
}

func (w *tailWriter) String() string {
	w.mu.Lock()
	defer w.mu.Unlock()
	return strings.TrimSpace(string(w.buf))
}

// RunCmd runs a one-shot shell command to completion in its own process group,
// killing the whole group on ctx cancel. A non-zero exit is an error. Used for
// project setup_cmd (worktree create) and teardown_cmd (archive/delete).
func RunCmd(ctx context.Context, shellCmd, workDir string, env []string) error {
	_, err := RunCmdOutput(ctx, shellCmd, workDir, env)
	return err
}

// RunCmdOutput is RunCmd plus the last ~4 KiB of combined stdout/stderr.
// On ctx cancel it killGroups then returns (tail, ctx.Err()), not the wait error.
func RunCmdOutput(ctx context.Context, shellCmd, workDir string, env []string) (string, error) {
	cmd := exec.CommandContext(ctx, "/bin/sh", "-c", shellCmd)
	cmd.Dir = workDir
	if env != nil {
		cmd.Env = env
	}
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}

	// Own *os.File pipes so Wait returns when the shell exits (not when
	// descendants close stdout) and so we can keep draining instead of
	// closing the write end — os/exec's WaitDelay would SIGPIPE survivors.
	pr, pw, err := os.Pipe()
	if err != nil {
		return "", err
	}
	cmd.Stdout = pw
	cmd.Stderr = pw

	tw := &tailWriter{max: cmdOutputTailCap}
	if err := cmd.Start(); err != nil {
		pw.Close()
		pr.Close()
		return "", err
	}
	pw.Close() // parent copy; children still hold the write end

	copied := make(chan struct{})
	go func() {
		defer close(copied)
		defer pr.Close()
		_, _ = io.Copy(tw, pr)
	}()

	reaped := make(chan struct{})
	var waitErr error
	go func() { waitErr = cmd.Wait(); close(reaped) }()
	select {
	case <-reaped:
		if err := ctx.Err(); err != nil {
			return tw.String(), err
		}
		select {
		case <-copied:
		case <-time.After(cmdOutputWaitDelay):
			// simplify: drain goroutine lives until descendants close stdout
			// (same lifetime as writing to /dev/null). Do not close the pipe.
		}
		return tw.String(), waitErr
	case <-ctx.Done():
		killGroup(cmd, reaped)
		return tw.String(), ctx.Err()
	}
}

// FreePort asks the OS for an unused TCP port by binding :0 and releasing it.
// The classic :0 TOCTOU window (the port could be taken between release here and
// the dev server binding it) is acceptable for local workspace alloc: a
// lost race just surfaces as a normal "exited before serving" / probe failure the
// user can re-run, and a real fix (strictPort or pass-the-listener) would couple
// us to each dev server's flags.
func FreePort() (int, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 0, err
	}
	defer l.Close()
	return l.Addr().(*net.TCPAddr).Port, nil
}

// substitutePort replaces $RECUR_PORT / ${RECUR_PORT} / $PORT in s.
func substitutePort(s, port string) string {
	r := strings.NewReplacer(
		"${RECUR_PORT}", port,
		"$RECUR_PORT", port,
		"${PORT}", port,
		"$PORT", port,
	)
	return r.Replace(s)
}

// probe makes one GET and reports the status code. Any HTTP response (even 4xx/5xx)
// counts as "the server is up"; only transport errors mean not-yet-ready.
func probe(client *http.Client, url string) (int, bool) {
	resp, err := client.Get(url)
	if err != nil {
		return 0, false
	}
	defer resp.Body.Close()
	return resp.StatusCode, true
}

// killGroup terminates the command's entire process group (SIGTERM, then SIGKILL
// after a short grace) so no child of the dev server is left running. It does NOT
// call Wait() itself — the caller's reaper goroutine owns cmd.Wait(); killGroup
// only signals the group and observes the reaped close-broadcast, so only one
// wait4 ever runs on the PID (no "no child processes" race). Because reaped is
// closed (not sent on), receiving from it returns immediately and repeatably even
// if the probe select already observed the exit, so killGroup never deadlocks.
func killGroup(cmd *exec.Cmd, reaped <-chan struct{}) {
	if cmd.Process == nil {
		return
	}
	pgid := cmd.Process.Pid
	// Negative pid signals the whole group (we set Setpgid so pgid == pid).
	_ = syscall.Kill(-pgid, syscall.SIGTERM)

	select {
	case <-reaped:
		// Group leader already reaped by the reaper goroutine (or exiting now).
	case <-time.After(defaultKillGrace):
		_ = syscall.Kill(-pgid, syscall.SIGKILL)
		// Block until the reaper observes the exit so the function does not
		// return while the process is still being torn down. After SIGKILL the
		// reaper's cmd.Wait() returns and closes reaped, so this unblocks.
		<-reaped
	}
}
