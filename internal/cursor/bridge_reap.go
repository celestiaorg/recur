package cursor

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"
)

// bridgePidFile records the process-group leader of the bridge Recur last
// started for this LogDir. Used to reap a leftover from a crash / Force Quit
// before the next Start — Setpgid keeps the bridge alive after the parent dies,
// so clean Shutdown alone is not enough.
const bridgePidFile = "cursor-bridge.pid"

// reapStaleBridges kills leftover cursor-sdk-bridge process groups from a prior
// Recur process that crashed or was Force-Quit. The orphan scan also covers
// Recur-installed bridges from other data dirs: a killed dogfood instance whose
// data dir was torn down never starts again to reap its own.
//
// Safety rules (Settings probes start throwaway bridges with the same Bin while
// the provider bridge may be mid-run):
//
//   - No LogDir → no reap. Throwaway probes pass LogDir "" and must not scan.
//   - Only orphan leaders are killed (PPID 1, or a dead parent). A live sibling
//     whose PPID is this Recur process — or another still-running Recur — is
//     left alone. exceptPID is reserved for the process we just spawned.
//
// Best-effort: failures to list or signal are ignored — Start must still proceed.
func reapStaleBridges(bin, logDir string, exceptPID int, note func(string)) {
	if logDir == "" {
		// Throwaway Settings probes share Bin with the long-lived provider
		// bridge; scanning would kill an in-flight agent turn.
		return
	}
	variants := bridgePathVariants(bin)
	if len(variants) == 0 {
		return
	}
	if note == nil {
		note = func(string) {}
	}

	seen := map[int]bool{}
	kill := func(pgid int, why string) {
		if pgid <= 1 || pgid == exceptPID || seen[pgid] {
			return
		}
		seen[pgid] = true
		note(fmt.Sprintf("recur: reaping stale cursor-sdk-bridge pgid=%d (%s)", pgid, why))
		killProcessGroup(pgid)
	}

	if pgid, ok := readBridgePidfile(logDir); ok {
		ours := bridgeProcessStillOurs(pgid, variants)
		orphan := ours && bridgeIsOrphan(pgid, processPPID(pgid))
		if orphan {
			kill(pgid, "pidfile")
		}
		// Drop a stale claim. If the pidfile still names a live non-orphan
		// (should not happen under ensureRunner, which Closes first), leave it
		// — writeBridgePidfile after ready refreshes the owner.
		if !ours || orphan {
			clearBridgePidfile(logDir)
		}
	}

	owned := string(filepath.Separator) + filepath.Join(BridgeDirName, launcherRelPath)
	for _, pgid := range listOrphanBridgeLeaders(append(variants, owned)) {
		kill(pgid, "orphan scan")
	}
}

// bridgePathVariants lists cleaned absolute forms of bin, including symlink
// targets. macOS temp paths often appear as both /var/… and /private/var/… in
// ps vs EvalSymlinks; matching any variant keeps reaping reliable.
func bridgePathVariants(bin string) []string {
	if bin == "" {
		return nil
	}
	seen := map[string]bool{}
	var out []string
	add := func(p string) {
		p = filepath.Clean(p)
		if p == "" || p == "." || seen[p] {
			return
		}
		seen[p] = true
		out = append(out, p)
	}
	add(bin)
	if abs, err := filepath.Abs(bin); err == nil {
		add(abs)
		if resolved, err := filepath.EvalSymlinks(abs); err == nil {
			add(resolved)
		}
	}
	return out
}

func bridgePidPath(logDir string) string {
	return filepath.Join(logDir, bridgePidFile)
}

func writeBridgePidfile(logDir string, pid int) {
	if logDir == "" || pid <= 1 {
		return
	}
	_ = os.MkdirAll(logDir, 0o755)
	_ = os.WriteFile(bridgePidPath(logDir), []byte(strconv.Itoa(pid)+"\n"), 0o600)
}

func clearBridgePidfile(logDir string) {
	if logDir == "" {
		return
	}
	_ = os.Remove(bridgePidPath(logDir))
}

func readBridgePidfile(logDir string) (int, bool) {
	raw, err := os.ReadFile(bridgePidPath(logDir))
	if err != nil {
		return 0, false
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(raw)))
	if err != nil || pid <= 1 {
		return 0, false
	}
	return pid, true
}

// bridgeProcessStillOurs reports whether pid is alive and its command line still
// references our launcher — guards against PID reuse after reboot.
func bridgeProcessStillOurs(pid int, variants []string) bool {
	if err := syscall.Kill(pid, 0); err != nil {
		return false
	}
	cmd, ok := processCommand(pid)
	if !ok {
		// Process exists but we cannot read its command (permissions): treat as
		// ours if the pidfile said so — worse case we signal a stranger with the
		// same pid, which is vanishingly unlikely within one boot.
		return true
	}
	return bridgeCommandMatches(cmd, variants)
}

// bridgeCommandMatches reports whether a ps command line is a launcher for one
// of our Bin path variants. The Node worker runs `…/cursor-sdk-bridge.js` and
// must not match.
func bridgeCommandMatches(command string, variants []string) bool {
	if command == "" || len(variants) == 0 {
		return false
	}
	for _, bin := range variants {
		if !strings.Contains(command, bin) {
			continue
		}
		// Defend against a future argv shape that embeds bin as a prefix of the JS.
		if strings.Contains(command, bin+".js") {
			continue
		}
		return true
	}
	return false
}

// bridgeIsOrphan reports whether a bridge leader was abandoned by a dead parent
// (typical Force-Quit / crash: PPID becomes 1). Live children of this process
// or of another still-running Recur must not be reaped. Unknown PPID (< 0)
// fails closed — do not kill.
func bridgeIsOrphan(pid, ppid int) bool {
	if pid <= 1 || ppid < 0 {
		return false
	}
	if ppid <= 1 {
		return true
	}
	if ppid == os.Getpid() {
		return false
	}
	if err := syscall.Kill(ppid, 0); err != nil {
		return true
	}
	return false
}

// listOrphanBridgeLeaders returns process-group leaders for our launcher whose
// parent is gone. Leaders only (pid == pgid): signalling -pgid reaps the Node
// child too.
func listOrphanBridgeLeaders(variants []string) []int {
	// -ww: unlimited width on darwin/linux so deep install paths are not
	// column-truncated in the command field.
	out, err := exec.Command("ps", "-wwaxo", "pid=,ppid=,pgid=,command=").Output()
	if err != nil {
		return nil
	}
	var leaders []int
	for _, line := range bytes.Split(out, []byte{'\n'}) {
		line = bytes.TrimSpace(line)
		if len(line) == 0 {
			continue
		}
		pid, ppid, pgid, command, ok := parsePSLine(string(line))
		if !ok || pid != pgid {
			continue
		}
		if !bridgeCommandMatches(command, variants) {
			continue
		}
		if !bridgeIsOrphan(pid, ppid) {
			continue
		}
		leaders = append(leaders, pgid)
	}
	return leaders
}

func parsePSLine(line string) (pid, ppid, pgid int, command string, ok bool) {
	fields := strings.Fields(line)
	if len(fields) < 4 {
		return 0, 0, 0, "", false
	}
	var err error
	pid, err = strconv.Atoi(fields[0])
	if err != nil {
		return 0, 0, 0, "", false
	}
	ppid, err = strconv.Atoi(fields[1])
	if err != nil {
		return 0, 0, 0, "", false
	}
	pgid, err = strconv.Atoi(fields[2])
	if err != nil {
		return 0, 0, 0, "", false
	}
	// command may contain spaces (…/Application Support/…); rebuild from the
	// original line after the three leading numeric columns.
	rest := strings.TrimSpace(line)
	for _, col := range fields[:3] {
		rest = strings.TrimSpace(strings.TrimPrefix(rest, col))
	}
	command = rest
	if command == "" {
		return 0, 0, 0, "", false
	}
	return pid, ppid, pgid, command, true
}

func processCommand(pid int) (string, bool) {
	out, err := exec.Command("ps", "-wwp", strconv.Itoa(pid), "-o", "command=").Output()
	if err != nil {
		return "", false
	}
	cmd := strings.TrimSpace(string(out))
	if cmd == "" {
		return "", false
	}
	return cmd, true
}

func processPPID(pid int) int {
	out, err := exec.Command("ps", "-wwp", strconv.Itoa(pid), "-o", "ppid=").Output()
	if err != nil {
		return -1
	}
	ppid, err := strconv.Atoi(strings.TrimSpace(string(out)))
	if err != nil {
		return -1
	}
	return ppid
}

// killProcessGroup sends SIGTERM to a process group, then SIGKILL after grace.
func killProcessGroup(pgid int) {
	if pgid <= 1 {
		return
	}
	_ = syscall.Kill(-pgid, syscall.SIGTERM)
	deadline := time.Now().Add(bridgeKillGrace)
	for time.Now().Before(deadline) {
		if err := syscall.Kill(pgid, 0); err != nil {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	_ = syscall.Kill(-pgid, syscall.SIGKILL)
}
