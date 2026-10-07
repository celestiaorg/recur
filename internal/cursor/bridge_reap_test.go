package cursor

import (
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestBridgeCommandMatches(t *testing.T) {
	bin := "/Users/josh/Library/Application Support/Recur/cursor-bridge/bin/cursor-sdk-bridge"
	variants := []string{bin}
	cases := []struct {
		cmd  string
		want bool
	}{
		{"sh " + bin + " --port 0", true},
		{bin + " --port 0", true},
		{"/Users/josh/Library/Application Support/Recur/cursor-bridge/bin/node /Users/josh/Library/Application Support/Recur/cursor-bridge/bin/../dist/bin/cursor-sdk-bridge.js --port 0", false},
		{"sh /Users/josh/Library/Application Support/Recur-LOOP-1/cursor-bridge/bin/cursor-sdk-bridge --port 0", false},
		{"", false},
	}
	for _, tc := range cases {
		if got := bridgeCommandMatches(tc.cmd, variants); got != tc.want {
			t.Errorf("bridgeCommandMatches(%q) = %v, want %v", tc.cmd, got, tc.want)
		}
	}
}

func TestParsePSLineKeepsSpacedPath(t *testing.T) {
	line := "1759     1  1759 sh /Users/josh/Library/Application Support/Recur/cursor-bridge/bin/cursor-sdk-bridge --port 0"
	pid, ppid, pgid, cmd, ok := parsePSLine(line)
	if !ok {
		t.Fatal("parse failed")
	}
	if pid != 1759 || ppid != 1 || pgid != 1759 {
		t.Fatalf("pid/ppid/pgid = %d/%d/%d", pid, ppid, pgid)
	}
	if !strings.Contains(cmd, "Application Support/Recur/cursor-bridge") {
		t.Fatalf("command lost spaced path: %q", cmd)
	}
}

func TestBridgeIsOrphan(t *testing.T) {
	if !bridgeIsOrphan(42, 1) {
		t.Fatal("ppid 1 should be orphan")
	}
	if bridgeIsOrphan(42, os.Getpid()) {
		t.Fatal("our direct child must not be treated as orphan")
	}
	if bridgeIsOrphan(1, 1) {
		t.Fatal("pid 1 must never be reaped")
	}
}

func TestStartReapsOrphanBridgeSameBin(t *testing.T) {
	dir := t.TempDir()
	bin := writeFakeBridge(t, dir)
	orphanPID := startOrphanBridge(t, bin)

	logDir := filepath.Join(dir, "logs")
	b := &Bridge{Bin: bin, LogDir: logDir}
	if err := b.Start(t.Context()); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer func() { _ = b.Close() }()

	waitFor(t, 5*time.Second, func() bool {
		return syscall.Kill(orphanPID, 0) != nil
	})

	raw, err := os.ReadFile(filepath.Join(logDir, bridgeLogFile))
	if err != nil {
		t.Fatalf("read log: %v", err)
	}
	if !strings.Contains(string(raw), "reaping stale cursor-sdk-bridge") {
		t.Fatalf("expected reap note in log, got:\n%s", raw)
	}
	if _, err := os.Stat(filepath.Join(logDir, bridgePidFile)); err != nil {
		t.Fatalf("pidfile missing after Start: %v", err)
	}
}

func TestProbeStartDoesNotKillLiveSibling(t *testing.T) {
	// Settings probes start a throwaway bridge (same Bin, no LogDir) while the
	// provider bridge may be mid-run. Reaping must not touch the live sibling.
	dir := t.TempDir()
	bin := writeFakeBridge(t, dir)

	provider := &Bridge{Bin: bin, LogDir: filepath.Join(dir, "logs")}
	if err := provider.Start(t.Context()); err != nil {
		t.Fatalf("provider Start: %v", err)
	}
	defer func() { _ = provider.Close() }()
	providerPID := provider.cmd.Process.Pid

	probe := &Bridge{Bin: bin} // LogDir empty — throwaway probe
	if err := probe.Start(t.Context()); err != nil {
		t.Fatalf("probe Start: %v", err)
	}
	defer func() { _ = probe.Close() }()

	if err := syscall.Kill(providerPID, 0); err != nil {
		t.Fatalf("probe Start reaped the live provider bridge: %v", err)
	}
	if !provider.Alive() {
		t.Fatal("provider bridge reports not alive after probe Start")
	}
}

func TestStartDoesNotReapLiveNonOrphanSameBin(t *testing.T) {
	// A still-parented bridge (ppid = this test) must survive a Start with LogDir,
	// even though it shares Bin — only orphans are in scope.
	dir := t.TempDir()
	bin := writeFakeBridge(t, dir)

	live := exec.Command(bin) //nolint:gosec
	live.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := live.Start(); err != nil {
		t.Fatalf("start live: %v", err)
	}
	livePID := live.Process.Pid
	t.Cleanup(func() {
		_ = syscall.Kill(-livePID, syscall.SIGKILL)
		_, _ = live.Process.Wait()
	})
	time.Sleep(100 * time.Millisecond)

	b := &Bridge{Bin: bin, LogDir: filepath.Join(dir, "logs")}
	if err := b.Start(t.Context()); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer func() { _ = b.Close() }()

	if err := syscall.Kill(livePID, 0); err != nil {
		t.Fatalf("non-orphan same-Bin bridge was reaped: %v", err)
	}
}

func TestStartDoesNotReapOtherBin(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "a"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(dir, "b"), 0o755); err != nil {
		t.Fatal(err)
	}
	binA := writeFakeBridge(t, filepath.Join(dir, "a"))
	binB := writeFakeBridge(t, filepath.Join(dir, "b"))

	otherPID := startOrphanBridge(t, binB)

	b := &Bridge{Bin: binA, LogDir: filepath.Join(dir, "logs")}
	if err := b.Start(t.Context()); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer func() { _ = b.Close() }()

	if err := syscall.Kill(otherPID, 0); err != nil {
		t.Fatalf("bridge for a different Bin was reaped: %v", err)
	}
}

func TestStartReapsOrphanBridgeFromOtherDataDir(t *testing.T) {
	dir := t.TempDir()
	bin := writeFakeBridge(t, filepath.Join(dir, "a"))
	otherBin := writeFakeBridge(t, filepath.Join(dir, "Recur-gone", BridgeDirName, "bin"))
	orphanPID := startOrphanBridge(t, otherBin)

	b := &Bridge{Bin: bin, LogDir: filepath.Join(dir, "logs")}
	if err := b.Start(t.Context()); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer func() { _ = b.Close() }()

	waitFor(t, 5*time.Second, func() bool {
		return syscall.Kill(orphanPID, 0) != nil
	})
}

func TestCloseClearsPidfile(t *testing.T) {
	dir := t.TempDir()
	bin := writeFakeBridge(t, dir)
	logDir := filepath.Join(dir, "logs")
	b := &Bridge{Bin: bin, LogDir: logDir}
	if err := b.Start(t.Context()); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if _, err := os.Stat(filepath.Join(logDir, bridgePidFile)); err != nil {
		t.Fatalf("pidfile missing: %v", err)
	}
	if err := b.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if _, err := os.Stat(filepath.Join(logDir, bridgePidFile)); !os.IsNotExist(err) {
		t.Fatalf("pidfile should be gone after Close, err=%v", err)
	}
}

// startOrphanBridge launches bin from a short-lived helper that exits, leaving
// the bridge reparented to PID 1 (the Force-Quit leftover shape).
func startOrphanBridge(t *testing.T, bin string) int {
	t.Helper()
	helper := filepath.Join(t.TempDir(), "spawn_orphan.py")
	pidFile := filepath.Join(t.TempDir(), "orphan.pid")
	// DEVNULL + os._exit: the child must not inherit the helper's stdout/stderr
	// pipes (CombinedOutput would wait on them forever), and a normal Python
	// exit waits on non-daemon Popen children.
	script := `
import os, subprocess, sys
bin, pid_path = sys.argv[1], sys.argv[2]
p = subprocess.Popen(
    [bin],
    start_new_session=True,
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)
with open(pid_path, "w", encoding="utf-8") as f:
    f.write(str(p.pid))
os._exit(0)
`
	if err := os.WriteFile(helper, []byte(script), 0o644); err != nil {
		t.Fatalf("write helper: %v", err)
	}
	cmd := exec.Command("python3", helper, bin, pidFile) //nolint:gosec
	cmd.Stdout = nil
	cmd.Stderr = nil
	if err := cmd.Run(); err != nil {
		t.Fatalf("spawn orphan: %v", err)
	}
	raw, err := os.ReadFile(pidFile)
	if err != nil {
		t.Fatalf("read orphan pid: %v", err)
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(raw)))
	if err != nil || pid <= 1 {
		t.Fatalf("bad orphan pid %q", raw)
	}
	// Wait until the helper has exited and the child is reparented (ppid 1).
	waitFor(t, 3*time.Second, func() bool {
		return processPPID(pid) == 1
	})
	t.Cleanup(func() {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		_ = syscall.Kill(pid, syscall.SIGKILL)
	})
	return pid
}
