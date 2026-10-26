package codex

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"
)

const (
	serverKillGrace   = 2 * time.Second
	serverStartBudget = 45 * time.Second
)

// Server is a running `codex app-server` process plus its JSON-RPC Conn.
type Server struct {
	Bin      string
	Home     string // CODEX_HOME (Recur-owned)
	Worktree string // cmd.Dir
	APIKey   string // optional CODEX_API_KEY
	// ConfigOverrides are `-c key=value` entries (MCP, approval, sandbox).
	ConfigOverrides []string
	// SeedAuthFrom is an optional path to auth.json to copy into Home before
	// start (ChatGPT login reuse). Never the worktree.
	SeedAuthFrom string

	cmd    *exec.Cmd
	conn   *Conn
	stderr *lineRing
	reaped chan struct{}

	mu     sync.Mutex
	closed bool
}

// Start spawns app-server, completes initialize/initialized, and returns a
// ready Conn. Caller must Close.
func (s *Server) Start(ctx context.Context) error {
	if s.Bin == "" {
		return fmt.Errorf("codex server: no binary")
	}
	if s.Home == "" {
		return fmt.Errorf("codex server: CODEX_HOME required")
	}
	if err := os.MkdirAll(s.Home, 0o755); err != nil {
		return fmt.Errorf("codex server: mkdir CODEX_HOME: %w", err)
	}
	if s.SeedAuthFrom != "" {
		if err := seedAuth(s.SeedAuthFrom, filepath.Join(s.Home, "auth.json")); err != nil {
			return err
		}
	}
	// Isolated CODEX_HOME has no project trust table — without this, sandbox
	// writes in a temp/loop worktree fail with "workspace tool failed to start".
	if s.Worktree != "" {
		if err := ensureTrustedProject(s.Home, s.Worktree); err != nil {
			return err
		}
	}

	args := []string{"app-server"}
	args = append(args, ConfigArgs(s.ConfigOverrides)...)

	//nolint:gosec // Bin is Locate-validated; args are app-owned.
	cmd := exec.Command(s.Bin, args...)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if s.Worktree != "" {
		cmd.Dir = s.Worktree
	}
	cmd.Env = serverLaunchEnv(os.Environ(), s.Home, s.APIKey)

	stdin, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return err
	}

	s.stderr = newLineRing(80)
	go drainStderr(stderr, s.stderr)

	if err := cmd.Start(); err != nil {
		return fmt.Errorf("start codex app-server: %w", err)
	}
	s.cmd = cmd
	s.reaped = make(chan struct{})
	go func() {
		_ = cmd.Wait()
		close(s.reaped)
	}()

	s.conn = newConn(stdout, stdin)
	s.conn.OnServerRequest(AutoApproveServerRequest)

	sctx, cancel := context.WithTimeout(ctx, serverStartBudget)
	defer cancel()
	if err := s.handshake(sctx); err != nil {
		s.Close()
		return err
	}
	return nil
}

func (s *Server) handshake(ctx context.Context) error {
	var initRes map[string]any
	err := s.conn.Call(ctx, "initialize", map[string]any{
		"clientInfo": map[string]string{
			"name":    "recur",
			"title":   "Recur",
			"version": "0.1.0",
		},
	}, &initRes)
	if err != nil {
		return fmt.Errorf("codex initialize: %w", err)
	}
	if err := s.conn.Notify("initialized", map[string]any{}); err != nil {
		return fmt.Errorf("codex initialized: %w", err)
	}
	return nil
}

// Conn returns the live JSON-RPC connection (nil before Start).
func (s *Server) Conn() *Conn { return s.conn }

// Reaped is closed when the app-server process has been waited. Callers can
// select on it to detect mid-turn crashes without waiting for a hung RPC.
func (s *Server) Reaped() <-chan struct{} {
	if s == nil {
		return nil
	}
	return s.reaped
}

// Alive reports whether the process is still running.
func (s *Server) Alive() bool {
	if s == nil || s.cmd == nil || s.cmd.Process == nil {
		return false
	}
	select {
	case <-s.reaped:
		return false
	default:
		return true
	}
}

// StderrTail returns recent stderr lines for error enrichment.
func (s *Server) StderrTail() string {
	if s.stderr == nil {
		return ""
	}
	return s.stderr.String()
}

// Close kills the process group and waits for exit.
func (s *Server) Close() error {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return nil
	}
	s.closed = true
	s.mu.Unlock()
	if s.cmd != nil {
		killGroup(s.cmd, s.reaped)
	}
	return nil
}

// ensureTrustedProject appends a projects.<cwd> trust entry to CODEX_HOME's
// config.toml when missing. Does not touch the worktree or ~/.codex.
func ensureTrustedProject(home, worktree string) error {
	abs, err := filepath.Abs(worktree)
	if err != nil {
		return err
	}
	cfgPath := filepath.Join(home, "config.toml")
	marker := fmt.Sprintf("[projects.%q]", abs)
	if raw, err := os.ReadFile(cfgPath); err == nil && strings.Contains(string(raw), marker) {
		return nil
	}
	f, err := os.OpenFile(cfgPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return err
	}
	defer f.Close()
	_, err = fmt.Fprintf(f, "\n%s\ntrust_level = \"trusted\"\n", marker)
	return err
}

func seedAuth(from, to string) error {
	if _, err := os.Stat(filepath.Join(filepath.Dir(to), browserLoginMarker)); err == nil {
		return nil
	}
	raw, err := os.ReadFile(from)
	if err != nil {
		if os.IsNotExist(err) {
			// Browser sign-in can create Recur-owned credentials without a
			// machine-wide CLI login. Importing that login is optional then.
			if dst, derr := os.Stat(to); derr == nil && dst.Size() > 0 {
				return nil
			}
			return fmt.Errorf("codex ChatGPT auth: sign in with ChatGPT in Settings, run `codex login`, or set an API key (%s not found)", from)
		}
		return err
	}
	if err := os.MkdirAll(filepath.Dir(to), 0o755); err != nil {
		return err
	}
	// Don't overwrite a newer Recur-owned auth with an empty source.
	if len(raw) == 0 {
		return fmt.Errorf("codex ChatGPT auth: %s is empty", from)
	}
	// Preserve tokens Codex refreshed inside Recur's CODEX_HOME. Re-seed only
	// when Recur has no auth yet, or the machine login file is strictly newer.
	if dst, err := os.Stat(to); err == nil {
		src, serr := os.Stat(from)
		if serr == nil && !src.ModTime().After(dst.ModTime()) {
			return nil
		}
	}
	return os.WriteFile(to, raw, 0o600)
}

// browserLoginMarker in CODEX_HOME means auth.json came from a browser sign-in
// in Recur, so the machine `codex login` must not replace it.
const browserLoginMarker = "recur-browser-login"

// MarkBrowserLogin records that home's auth.json is a Recur browser sign-in.
func MarkBrowserLogin(home string) error {
	return os.WriteFile(filepath.Join(home, browserLoginMarker), nil, 0o600)
}

// DefaultSeedAuthPath is the user's machine login file.
func DefaultSeedAuthPath() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".codex", "auth.json")
}

func killGroup(cmd *exec.Cmd, reaped <-chan struct{}) {
	if cmd.Process == nil {
		return
	}
	pgid := cmd.Process.Pid
	_ = syscall.Kill(-pgid, syscall.SIGTERM)
	select {
	case <-reaped:
	case <-time.After(serverKillGrace):
		_ = syscall.Kill(-pgid, syscall.SIGKILL)
		<-reaped
	}
}

func drainStderr(r io.Reader, ring *lineRing) {
	buf := make([]byte, 4096)
	var partial string
	for {
		n, err := r.Read(buf)
		if n > 0 {
			partial += string(buf[:n])
			for {
				i := strings.IndexByte(partial, '\n')
				if i < 0 {
					break
				}
				ring.Add(partial[:i])
				partial = partial[i+1:]
			}
		}
		if err != nil {
			if partial != "" {
				ring.Add(partial)
			}
			return
		}
	}
}

// AutoApproveServerRequest accepts MCP elicitations and command approvals so
// automation is not wedged. Pair with approval_policy=never + MCP
// default_tools_approval_mode=approve; this is the failsafe. CodexRunner
// replaces it with a human prompt when Settings.BypassPermissions is off.
func AutoApproveServerRequest(method string, _ json.RawMessage) (any, error) {
	switch method {
	case "mcpServer/elicitation/request":
		return map[string]any{"action": "accept", "content": map[string]any{}}, nil
	case "item/commandExecution/requestApproval", "commandExecution/requestApproval":
		return map[string]any{"decision": "acceptForSession"}, nil
	case "execCommandApproval":
		return map[string]any{"decision": "approved"}, nil
	case "applyPatchApproval", "fileChange/requestApproval", "item/fileChange/requestApproval":
		return map[string]any{"decision": "acceptForSession"}, nil
	case "item/permissions/requestApproval":
		return map[string]any{"decision": "accept"}, nil
	default:
		// Don't hang the turn on an unknown request — accept empty.
		return map[string]any{}, nil
	}
}

// lineRing is a tiny stderr ring buffer.
type lineRing struct {
	mu    sync.Mutex
	lines []string
	max   int
}

func newLineRing(max int) *lineRing { return &lineRing{max: max} }

func (r *lineRing) Add(line string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.lines = append(r.lines, line)
	if len(r.lines) > r.max {
		r.lines = r.lines[len(r.lines)-r.max:]
	}
}

func (r *lineRing) String() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return strings.Join(r.lines, "\n")
}

func serverLaunchEnv(base []string, home, apiKey string) []string {
	env := dropEnvKey(base, "LINEAR_API_KEY")
	env = dropEnvKey(env, "CODEX_API_KEY")
	env = dropEnvKey(env, "OPENAI_API_KEY")
	env = append(env, "CODEX_HOME="+home)
	if apiKey != "" {
		env = append(env, "CODEX_API_KEY="+apiKey)
	}
	return env
}

func dropEnvKey(env []string, key string) []string {
	prefix := key + "="
	out := make([]string, 0, len(env))
	for _, e := range env {
		if strings.HasPrefix(e, prefix) {
			continue
		}
		out = append(out, e)
	}
	return out
}
