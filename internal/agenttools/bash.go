package agenttools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"syscall"
	"time"
	"unicode/utf8"
)

var schemaBash = json.RawMessage(`{"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"number"}},"required":["command"]}`)

const maxBashCapture = (maxBashLines + 2) * (maxLineChars + 8)

type bashTool struct{}

func (bashTool) Name() string { return "Bash" }
func (bashTool) Description() string {
	return "Run a shell command in the worktree. Destructive commands require permission when Bypass Permissions is off."
}
func (bashTool) Schema() json.RawMessage { return schemaBash }
func (bashTool) Execute(c Call) (string, error) {
	var in struct {
		Command string  `json:"command"`
		Timeout float64 `json:"timeout"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("invalid Bash args: %w", err)
	}
	if strings.TrimSpace(in.Command) == "" {
		return "", fmt.Errorf("command is required")
	}
	if !c.BypassPerms && c.Permitter != nil {
		allow, reason, err := c.Permitter.RequestPermission(c.ctx(), c.WorkspaceID, c.AgentCode, "Bash", c.Args)
		if err != nil {
			return "", err
		}
		if !allow {
			if reason == "" {
				reason = "denied"
			}
			return "", fmt.Errorf("permission denied: %s", reason)
		}
	}
	wait := defaultBashWait
	if in.Timeout > 0 {
		wait = time.Duration(in.Timeout * float64(time.Second))
		if wait > maxBashWait {
			wait = maxBashWait
		}
	}
	// WithTimeout keeps an earlier parent deadline (turn ctx) when it is tighter.
	ctx, cancel := context.WithTimeout(c.ctx(), wait)
	defer cancel()
	cmd := exec.CommandContext(ctx, "bash", "-lc", in.Command)
	cmd.Dir = c.Worktree
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error {
		if cmd.Process == nil {
			return nil
		}
		return syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
	}
	cmd.WaitDelay = 2 * time.Second
	var stdout, stderr capBuffer
	stdout.max, stderr.max = maxBashCapture, maxBashCapture
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	out := truncateCmdOutput(stdout.String(), stderr.String(), err)
	if err != nil && ctx.Err() != nil {
		return out, ctx.Err()
	}
	return out, nil
}

// capBuffer drains the pipe while bounding retained bytes so a runaway command
// cannot fill RAM. Overflow is discarded; clipLines still applies the line cap.
type capBuffer struct {
	buf     bytes.Buffer
	max     int
	clipped bool
}

func (c *capBuffer) Write(p []byte) (int, error) {
	n := len(p)
	if c.clipped {
		return n, nil
	}
	room := c.max - c.buf.Len()
	if room <= 0 {
		c.clipped = true
		return n, nil
	}
	if len(p) > room {
		c.buf.Write(p[:room])
		c.clipped = true
		return n, nil
	}
	c.buf.Write(p)
	return n, nil
}

func (c *capBuffer) String() string {
	s := clipLines(c.buf.String())
	if c.clipped && !strings.Contains(s, "<response clipped>") {
		if s != "" {
			s += "\n"
		}
		s += "<response clipped>"
	}
	return s
}

func truncateCmdOutput(stdout, stderr string, runErr error) string {
	var b strings.Builder
	if stdout != "" {
		b.WriteString(stdout)
	}
	if stderr != "" {
		if b.Len() > 0 {
			b.WriteString("\n")
		}
		b.WriteString("stderr:\n")
		b.WriteString(stderr)
	}
	if runErr != nil {
		if b.Len() > 0 {
			b.WriteString("\n")
		}
		fmt.Fprintf(&b, "exit: %v", runErr)
	}
	if b.Len() == 0 {
		return "(no output)"
	}
	return b.String()
}

func clipLines(s string) string {
	sc := strings.Split(s, "\n")
	var lines []string
	for _, line := range sc {
		if utf8.RuneCountInString(line) > maxLineChars {
			runes := []rune(line)
			line = string(runes[:maxLineChars]) + "…"
		}
		lines = append(lines, line)
		if len(lines) >= maxBashLines {
			lines = append(lines, "<response clipped>")
			break
		}
	}
	return strings.Join(lines, "\n")
}
