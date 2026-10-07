package term

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"time"
)

// processCwd returns the current working directory of pid, or "" when it cannot
// be read. Used when snapshotting tabs so a restore starts in the directory the
// user `cd`'d to, not the worktree they originally opened the shell in.
func processCwd(pid int) string {
	if pid <= 0 {
		return ""
	}
	if runtime.GOOS == "linux" {
		if p, err := os.Readlink("/proc/" + strconv.Itoa(pid) + "/cwd"); err == nil {
			return p
		}
	}
	return lsofCwd(pid)
}

func lsofCwd(pid int) string {
	ctx, cancel := context.WithTimeout(context.Background(), 400*time.Millisecond)
	defer cancel()
	cmd := exec.CommandContext(ctx, "lsof", "-a", "-p", strconv.Itoa(pid), "-d", "cwd", "-Fn", "-w")
	out, err := cmd.Output()
	if err != nil {
		return ""
	}
	for _, line := range bytes.Split(out, []byte{'\n'}) {
		if len(line) < 2 || line[0] != 'n' {
			continue
		}
		path := string(line[1:])
		if cut := strings.Index(path, " ("); cut >= 0 {
			path = path[:cut]
		}
		return path
	}
	return ""
}
