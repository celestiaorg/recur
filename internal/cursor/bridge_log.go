package cursor

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// bridgeLogFile is the stderr capture under LogDir. Per-worktree bridges use
// distinct LogDirs; rotated when it grows past bridgeLogMaxBytes.
const bridgeLogFile = "cursor-bridge.log"

// bridgeLogMaxBytes caps the log before we rotate to cursor-bridge.log.old.
const bridgeLogMaxBytes = 2 << 20 // 2 MiB

// bridgeRecentLines is how many stderr lines StderrTail keeps for attaching to
// agent errors — enough context for "[internal] internal error", not enough to
// bloat Result.Stderr.
const bridgeRecentLines = 40

// lineRing is a fixed-size ring of the most recent stderr lines.
type lineRing struct {
	mu   sync.Mutex
	lines []string
	next  int
	full  bool
}

func newLineRing(n int) *lineRing {
	if n < 1 {
		n = 1
	}
	return &lineRing{lines: make([]string, n)}
}

func (r *lineRing) add(line string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.lines[r.next] = line
	r.next = (r.next + 1) % len(r.lines)
	if r.next == 0 {
		r.full = true
	}
}

func (r *lineRing) tail() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	if len(r.lines) == 0 {
		return ""
	}
	var out []string
	if r.full {
		out = append(out, r.lines[r.next:]...)
		out = append(out, r.lines[:r.next]...)
	} else {
		out = append(out, r.lines[:r.next]...)
	}
	// Drop empty slots from a partially-filled ring.
	cleaned := out[:0]
	for _, ln := range out {
		if ln != "" {
			cleaned = append(cleaned, ln)
		}
	}
	return strings.Join(cleaned, "\n")
}

// openBridgeLog creates/appends LogDir/cursor-bridge.log, rotating if oversized.
// Returns nil when LogDir is empty or the file cannot be opened (logging must
// never fail a bridge start).
func openBridgeLog(logDir string) *os.File {
	if logDir == "" {
		return nil
	}
	if err := os.MkdirAll(logDir, 0o755); err != nil {
		return nil
	}
	path := filepath.Join(logDir, bridgeLogFile)
	if fi, err := os.Stat(path); err == nil && fi.Size() >= bridgeLogMaxBytes {
		_ = os.Rename(path, path+".old")
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return nil
	}
	return f
}

// redactBridgeLine strips secrets from a stderr line before it hits disk or the
// ring buffer. The discovery ready-line carries authToken in plaintext JSON.
func redactBridgeLine(line string) string {
	idx := strings.Index(line, readyPrefix)
	if idx < 0 {
		return line
	}
	raw := line[idx+len(readyPrefix):]
	var d map[string]any
	if err := json.Unmarshal([]byte(raw), &d); err != nil {
		return line[:idx+len(readyPrefix)] + "[redacted-discovery]"
	}
	if _, ok := d["authToken"]; ok {
		d["authToken"] = "[redacted]"
	}
	out, err := json.Marshal(d)
	if err != nil {
		return line[:idx+len(readyPrefix)] + "[redacted-discovery]"
	}
	return line[:idx+len(readyPrefix)] + string(out)
}

// noteStderr records one bridge stderr line (redacted) to the ring buffer and
// the on-disk log. Safe for concurrent use with StderrTail/Close.
func (b *Bridge) noteStderr(line string) {
	safe := redactBridgeLine(line)
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.recent == nil {
		b.recent = newLineRing(bridgeRecentLines)
	}
	b.recent.add(safe)
	if b.logFile != nil {
		_, _ = fmt.Fprintf(b.logFile, "%s %s\n", time.Now().UTC().Format(time.RFC3339), safe)
	}
}

// StderrTail returns the most recent bridge stderr lines (redacted), for
// attaching to agent failures so "[internal] internal error" is not a dead end.
func (b *Bridge) StderrTail() string {
	if b == nil {
		return ""
	}
	b.mu.Lock()
	r := b.recent
	path := b.logPath
	b.mu.Unlock()
	tail := ""
	if r != nil {
		tail = r.tail()
	}
	if path == "" || tail == "" {
		return tail
	}
	return tail + "\n(bridge log: " + path + ")"
}

// LogPath returns the on-disk stderr capture path, or "" when logging is off.
func (b *Bridge) LogPath() string {
	if b == nil {
		return ""
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.logPath
}
