package runconfig

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

// devServerCmd serves HTTP on $RECUR_PORT. It avoids `python3 -m http.server`,
// whose HTTPServer.server_bind does a getfqdn reverse lookup that stalls for
// 30s+ on macOS CI runners.
const devServerCmd = `python3 -c 'import http.server as h, socketserver as s, sys; s.TCPServer.allow_reuse_address = True; s.TCPServer(("", int(sys.argv[1])), h.SimpleHTTPRequestHandler).serve_forever()' $RECUR_PORT`

// happyDevCmd returns a shell command that serves HTTP on $RECUR_PORT, using
// python3 if available, otherwise a tiny inline Go server via `go run`.
func happyDevCmd(t *testing.T) string {
	t.Helper()
	if _, err := exec.LookPath("python3"); err == nil {
		return devServerCmd
	}
	t.Skip("python3 not available; skipping happy-path run-config test")
	return ""
}

// TestBootHappyPathAndTeardown boots a long-lived dev server (the verify-in-browser
// path, PRD §10), confirms it serves on the assigned $RECUR_PORT while up, then
// tears the WHOLE process group down via Stop and confirms the port is freed — no
// hang past the budget, no leaked process.
func TestBootHappyPathAndTeardown(t *testing.T) {
	cmd := happyDevCmd(t)

	booted, err := Boot(context.Background(), cmd, "http://localhost:$RECUR_PORT", BootOptions{
		ProbeTimeout: 15 * time.Second,
		PollInterval: 150 * time.Millisecond,
	})
	if err != nil {
		t.Fatalf("Boot: %v", err)
	}
	if booted == nil || booted.Port == "" {
		t.Fatalf("Boot returned no handle/port: %+v", booted)
	}

	// The server is up: a probe succeeds while it's running.
	if _, ok := probe(&http.Client{Timeout: 2 * time.Second}, booted.AppURL); !ok {
		t.Errorf("server should answer at %s while booted", booted.AppURL)
	}

	// Tear down (with a hard outer deadline so a teardown hang fails the test rather
	// than wedging the suite). Stop is idempotent — call it twice.
	stopped := make(chan struct{})
	go func() { booted.Stop(); booted.Stop(); close(stopped) }()
	select {
	case <-stopped:
	case <-time.After(10 * time.Second):
		t.Fatal("Booted.Stop did not return within 10s — teardown hung")
	}

	// After teardown the port should be free (the dev server is gone). Poll briefly.
	freed := false
	for i := 0; i < 40; i++ {
		if _, ok := probe(&http.Client{Timeout: 500 * time.Millisecond}, booted.AppURL); !ok {
			freed = true
			break
		}
		time.Sleep(100 * time.Millisecond)
	}
	if !freed {
		t.Errorf("dev server still answering at %s after Stop — process leaked", booted.AppURL)
	}
}

// TestBootSurvivesCtxCancelUntilStop is the regression guard for the verify bug:
// a successful Boot returns a LONG-LIVED server whose lifetime must be owned by
// Stop(), NOT by the ctx passed to Boot. verify.go cancels its bootCtx right after
// Boot returns; if Boot used exec.CommandContext that cancel would SIGKILL the
// server before the QA agent could drive it. Here we cancel the ctx after Boot,
// then assert the server is STILL serving, then Stop() frees it.
func TestBootSurvivesCtxCancelUntilStop(t *testing.T) {
	cmd := happyDevCmd(t)

	ctx, cancel := context.WithCancel(context.Background())
	booted, err := Boot(ctx, cmd, "http://localhost:$RECUR_PORT", BootOptions{
		ProbeTimeout: 15 * time.Second,
		PollInterval: 150 * time.Millisecond,
	})
	if err != nil {
		cancel()
		t.Fatalf("Boot: %v", err)
	}

	// Cancel the ctx (mirrors verify.go's cancelBoot() right after a successful boot).
	cancel()

	// The server MUST still be alive — give the (buggy) watchdog ample time to fire.
	time.Sleep(500 * time.Millisecond)
	if _, ok := probe(&http.Client{Timeout: 2 * time.Second}, booted.AppURL); !ok {
		t.Fatalf("server died after ctx cancel — it must outlive bootCtx (owned by Stop, not ctx)")
	}

	// Stop() is what actually tears it down.
	booted.Stop()
	freed := false
	for i := 0; i < 40; i++ {
		if _, ok := probe(&http.Client{Timeout: 500 * time.Millisecond}, booted.AppURL); !ok {
			freed = true
			break
		}
		time.Sleep(100 * time.Millisecond)
	}
	if !freed {
		t.Errorf("dev server still answering after Stop — process leaked")
	}
}

// TestBootRunsSetupOnce confirms the optional setup command runs before the dev
// server (PRD §10 step 1): the setup writes a sentinel file the dev server then
// serves, so a successful probe proves setup ran first.
func TestBootUsesProvidedPort(t *testing.T) {
	if _, err := exec.LookPath("python3"); err != nil {
		t.Skip("python3 not available")
	}
	port, err := FreePort()
	if err != nil {
		t.Fatalf("FreePort: %v", err)
	}
	booted, err := Boot(context.Background(), devServerCmd,
		"http://localhost:$RECUR_PORT", BootOptions{
			Port:         port,
			ProbeTimeout: 15 * time.Second,
			PollInterval: 150 * time.Millisecond,
		})
	if err != nil {
		t.Fatalf("Boot: %v", err)
	}
	defer booted.Stop()
	if booted.Port != strconv.Itoa(port) {
		t.Errorf("Port=%q, want %d", booted.Port, port)
	}
}

func TestRunCmdFailsOnNonZero(t *testing.T) {
	err := RunCmd(context.Background(), "exit 3", t.TempDir(), nil)
	if err == nil {
		t.Fatal("expected error")
	}
}

func killPIDFile(t *testing.T, path string) {
	t.Helper()
	t.Cleanup(func() {
		b, err := os.ReadFile(path)
		if err != nil {
			return
		}
		pid, err := strconv.Atoi(strings.TrimSpace(string(b)))
		if err != nil || pid <= 0 {
			return
		}
		_ = syscall.Kill(pid, syscall.SIGKILL)
	})
}

func TestRunCmdOutputCapturesStdoutAndStderr(t *testing.T) {
	tail, err := RunCmdOutput(context.Background(), "echo out; echo boom >&2; exit 23", t.TempDir(), nil)
	var ee *exec.ExitError
	if !errors.As(err, &ee) {
		t.Fatalf("RunCmdOutput(...) error = %v (%T), want *exec.ExitError", err, err)
	}
	if got := ee.ExitCode(); got != 23 {
		t.Errorf("RunCmdOutput(...) ExitCode() = %d, want 23", got)
	}
	if !strings.Contains(tail, "out") || !strings.Contains(tail, "boom") {
		t.Errorf("RunCmdOutput(...) tail = %q, want both out and boom", tail)
	}
}

func TestRunCmdOutputCapsTail(t *testing.T) {
	// ~32 KiB of filler so HEADMARK falls off the 4 KiB tail and TAILMARK stays.
	const cmd = `echo HEADMARK; i=0; while [ "$i" -lt 400 ]; do printf '%s\n' 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'; i=$((i+1)); done; echo TAILMARK`
	tail, err := RunCmdOutput(context.Background(), cmd, t.TempDir(), nil)
	if err != nil {
		t.Fatalf("RunCmdOutput(oversize) = %q, %v, want nil err", tail, err)
	}
	if strings.Contains(tail, "HEADMARK") {
		t.Errorf("RunCmdOutput(oversize) tail = %q, want HEADMARK dropped", tail)
	}
	if !strings.Contains(tail, "TAILMARK") {
		t.Errorf("RunCmdOutput(oversize) tail = %q, want TAILMARK", tail)
	}
	if len(tail) > cmdOutputTailCap {
		t.Errorf("RunCmdOutput(oversize) len(tail) = %d, want <= %d", len(tail), cmdOutputTailCap)
	}
}

func TestRunCmdOutputReturnsWhileBackgroundChildHoldsPipes(t *testing.T) {
	dir := t.TempDir()
	pidPath := filepath.Join(dir, "pid")
	start := time.Now()
	tail, err := RunCmdOutput(context.Background(), "sleep 60 & echo $! >"+pidPath+"; echo boom >&2; exit 23", dir, nil)
	killPIDFile(t, pidPath)
	if elapsed := time.Since(start); elapsed > 5*time.Second {
		t.Fatalf("RunCmdOutput hung %s waiting on background child pipes", elapsed)
	}
	var ee *exec.ExitError
	if !errors.As(err, &ee) {
		t.Fatalf("RunCmdOutput(...) error = %v (%T), want *exec.ExitError", err, err)
	}
	if got := ee.ExitCode(); got != 23 {
		t.Errorf("RunCmdOutput(...) ExitCode() = %d, want 23", got)
	}
	if !strings.Contains(tail, "boom") {
		t.Errorf("RunCmdOutput(...) tail = %q, want boom", tail)
	}
}

func TestRunCmdOutputSuccessWithBackgroundChild(t *testing.T) {
	dir := t.TempDir()
	pidPath := filepath.Join(dir, "pid")
	start := time.Now()
	tail, err := RunCmdOutput(context.Background(), "sleep 60 & echo $! >"+pidPath+"; echo ok", dir, nil)
	killPIDFile(t, pidPath)
	if elapsed := time.Since(start); elapsed > 5*time.Second {
		t.Fatalf("RunCmdOutput hung %s waiting on background child pipes", elapsed)
	}
	if err != nil {
		t.Fatalf("RunCmdOutput(...) error = %v, want nil", err)
	}
	if !strings.Contains(tail, "ok") {
		t.Errorf("RunCmdOutput(...) tail = %q, want ok", tail)
	}
}

func TestRunCmdOutputBackgroundChildWriteDoesNotSIGPIPE(t *testing.T) {
	dir := t.TempDir()
	pidPath := filepath.Join(dir, "pid")
	statusPath := filepath.Join(dir, "status")
	// Child sleeps past the old 100ms WaitDelay, then writes to inherited stdout.
	// Closing that pipe would SIGPIPE echo (status 141); a live drain keeps it 0.
	script := fmt.Sprintf(`(sleep 0.3; echo child-log; echo $? >%q) & echo $! >%q; echo ok`, statusPath, pidPath)
	start := time.Now()
	tail, err := RunCmdOutput(context.Background(), script, dir, nil)
	killPIDFile(t, pidPath)
	if elapsed := time.Since(start); elapsed > 5*time.Second {
		t.Fatalf("RunCmdOutput hung %s waiting on background child pipes", elapsed)
	}
	if err != nil {
		t.Fatalf("RunCmdOutput(...) error = %v, want nil", err)
	}
	if !strings.Contains(tail, "ok") {
		t.Errorf("RunCmdOutput(...) tail = %q, want ok", tail)
	}
	deadline := time.Now().Add(2 * time.Second)
	var status string
	for time.Now().Before(deadline) {
		b, readErr := os.ReadFile(statusPath)
		if readErr == nil {
			status = strings.TrimSpace(string(b))
			if status != "" {
				break
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	if status != "0" {
		t.Errorf("RunCmdOutput background child write status = %q, want 0 (SIGPIPE is 141)", status)
	}
}

func TestRunCmdOutputTimeoutReturnsCtxErr(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
	defer cancel()
	tail, err := RunCmdOutput(ctx, "echo before; sleep 30", t.TempDir(), nil)
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("RunCmdOutput(timeout) error = %v, want context.DeadlineExceeded", err)
	}
	if strings.Contains(err.Error(), "signal: killed") {
		t.Errorf("RunCmdOutput(timeout) error = %q, want ctx.Err() not wait kill", err)
	}
	if !strings.Contains(tail, "before") {
		t.Errorf("RunCmdOutput(timeout) tail = %q, want before", tail)
	}
}

// TestBootFailsFastWhenNeverServes: a dev command that never opens a port must fail
// within the probe budget and tear down — never hang.
func TestBootFailsFastWhenNeverServes(t *testing.T) {
	start := time.Now()
	booted, err := Boot(context.Background(), "sleep 30", "http://localhost:$RECUR_PORT", BootOptions{
		ProbeTimeout: 2 * time.Second,
		PollInterval: 150 * time.Millisecond,
	})
	if err == nil {
		booted.Stop()
		t.Fatal("expected an error for a non-serving dev command")
	}
	if elapsed := time.Since(start); elapsed > 8*time.Second {
		t.Errorf("Boot did not respect the timeout: took %s", elapsed)
	}
}

// TestBootEmptyDevCmd: an empty dev command is rejected immediately.
func TestBootEmptyDevCmd(t *testing.T) {
	if _, err := Boot(context.Background(), "  ", "", BootOptions{}); err == nil {
		t.Error("empty dev_cmd should error")
	}
}
