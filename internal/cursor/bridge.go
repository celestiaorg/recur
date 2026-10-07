// Package cursor is a Go client for the Cursor SDK bridge — the local
// Connect/JSON server that Cursor ships inside its Python SDK wheel
// (`cursor_sdk/_vendor/bridge`). The bridge is a bundled Node runtime running
// Cursor's TypeScript SDK; it exposes the SAME `sdk.v1` surface to any host
// language, which is how Recur drives Cursor agents without embedding JS.
//
// Why the bridge and not `cursor-agent` (the CLI):
//   - MCP servers are passed INLINE per agent, so Recur never has to write a
//     `.cursor/mcp.json` into the loop's worktree (which would dirty the tree
//     the diff/PR flow reads, and could clobber a user's own file).
//   - `settingSources: []` suppresses ambient project/user config — the
//     equivalent of Claude Code's --strict-mcp-config, which the CLI lacks.
//   - Typed run events instead of the CLI's stream-json, whose tool_call payload
//     is an undocumented per-tool union (readToolCall/globToolCall/…).
//
// What the bridge does NOT give us (true of the CLI too — these are server-side
// facts about Cursor's backend, not client limitations):
//   - No custom system prompt. `--system-prompt` exists on the CLI but is gated
//     to Anysphere/OpenAI accounts; api2.cursor.sh rejects it with
//     `[invalid_argument] unknown option '--system-prompt'`. Role prompts must
//     therefore be prepended to the user prompt (see engine/cursor.go).
//   - No mid-run user-message append. One run per agent at a time. Recur maps
//     D49 Inject to CancelRun (capability `run.cancel`) plus a follow-up Send
//     on the same agentId.
//   - No dollar cost for local runs — GetUsage is cloud-only. Token counts do
//     arrive on the run result, so the governor is fed via a price table.
package cursor

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"
)

// readyPrefix is what the bridge prints (on STDERR, not stdout) once it is
// listening. The remainder of the line is the discovery JSON.
const readyPrefix = "cursor-sdk-bridge ready "

// bridgeStartTimeout bounds how long we wait for the ready line. The bundled
// Node runtime is ~117MB and cold-starts slowly on first run (page cache miss),
// so this is deliberately generous.
const bridgeStartTimeout = 60 * time.Second

// bridgeKillGrace matches engine.claudeKillGrace: SIGTERM, wait, then SIGKILL
// the whole process group.
const bridgeKillGrace = 2 * time.Second

// Discovery is the JSON payload on the bridge's ready line. Recur asserts
// schemaVersion/transport/protocol rather than trusting them, so a future bridge
// that switches to (say) a unix socket fails loudly at startup instead of
// misbehaving at the first RPC.
type Discovery struct {
	SchemaVersion int    `json:"schemaVersion"`
	ServerVersion string `json:"serverVersion"`
	PID           int    `json:"pid"`
	Transport     string `json:"transport"` // "tcp"
	Protocol      string `json:"protocol"`  // "connect"
	Host          string `json:"host"`
	Port          int    `json:"port"`
	URL           string `json:"url"`
	AuthToken     string `json:"authToken"`
	AuthTokenFile string `json:"authTokenFile"`
	WorkspaceRef  string `json:"workspaceRef"`
	StateRoot     string `json:"stateRoot"`
}

// Bridge is a running cursor-sdk-bridge process plus the Connect client bound to
// it. The engine pools one Bridge per worktree (see engine/cursorProvider):
// agents for the same worktree are multiplexed by agentId; different worktrees
// get different bridges so shell cwd / workspaceRef stay correct.
//
// Concurrency: Start/Close are guarded so double-Close is safe; the embedded
// Client is safe for concurrent use (net/http.Client is).
type Bridge struct {
	// Bin is the path to the `cursor-sdk-bridge` launcher script. Required —
	// resolution from settings/env/disk lives in Locate.
	Bin string

	// APIKey, when set, is exported as CURSOR_API_KEY to the bridge process so
	// runs authenticate without the user having done `cursor-agent login`.
	APIKey string

	// Workspace is the absolute worktree path this bridge is bound to. Passed as
	// --workspace (discovery workspaceRef / defaultCwd) and as the child
	// process's working directory. Required for agent shells to start in the
	// project rather than the GUI app's cwd ("/"); per-agent local.cwd still
	// overrides for CreateAgent.
	Workspace string

	// LogDir, when set, tees bridge stderr AND Recur-observed Connect faults
	// to LogDir/cursor-bridge.log, and holds cursor-bridge.pid for stale-process
	// reaping across crashes. The Node bridge stays silent on opaque
	// "[internal] internal error" responses — those are written here by the
	// Go client. Empty means ring-buffer only (still available via StderrTail).
	// Per-worktree bridges use distinct LogDirs so pidfiles do not clobber.
	LogDir string

	// AttributionConfigPath, when set, is exported as RECUR_CURSOR_ATTRIBUTION_PATH
	// so the install-time SDK patch can read commit/PR attribution toggles.
	AttributionConfigPath string

	cmd    *exec.Cmd
	disc   Discovery
	client *Client

	mu        sync.Mutex
	closed    bool
	reaped    chan struct{}
	recent    *lineRing
	logFile   *os.File
	logPath   string
	startedAt time.Time
}

// Start spawns the bridge and blocks until it reports ready (or ctx/timeout
// fires). On success the Bridge owns a live child process; the caller MUST
// Close it.
func (b *Bridge) Start(ctx context.Context) error {
	if b.Bin == "" {
		return fmt.Errorf("cursor bridge: no bridge binary configured")
	}
	if _, err := os.Stat(b.Bin); err != nil {
		return fmt.Errorf("cursor bridge: %w", err)
	}

	args := []string{"--port", "0"}
	if b.Workspace != "" {
		if fi, err := os.Stat(b.Workspace); err != nil || !fi.IsDir() {
			return fmt.Errorf("cursor bridge: workspace %q is unavailable", b.Workspace)
		}
		args = append(args, "--workspace", b.Workspace)
	}

	//nolint:gosec // argv is app-owned; Bin is validated above.
	cmd := exec.Command(b.Bin, args...)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	// Bind the Node process itself to the worktree. GUI apps launch with cwd
	// "/", and the SDK's shell tool falls back to process.cwd() when the model
	// omits working_directory — without this, pwd is disk root even when
	// local.cwd / user_info report the correct workspace.
	if b.Workspace != "" {
		cmd.Dir = b.Workspace
		// Best-effort: missing ~/.cursor/projects/<slug>/terminals makes Cursor's
		// Shell tool crash the bridge with a misleading spawn /bin/zsh ENOENT.
		if err := EnsureCursorProjectDirs(b.Workspace); err != nil {
			log.Printf("recur: ensure cursor project dirs for %s: %v", b.Workspace, err)
		}
	}
	cmd.Env = dropEnvKey(os.Environ(), "LINEAR_API_KEY")
	cmd.Env = append(cmd.Env, "CURSOR_SDK_CLIENT_LANGUAGE=go")
	if b.APIKey != "" {
		cmd.Env = append(cmd.Env, "CURSOR_API_KEY="+b.APIKey)
	}
	if b.AttributionConfigPath != "" {
		cmd.Env = append(cmd.Env, EnvAttributionPath+"="+b.AttributionConfigPath)
	}
	// Dock-launched macOS apps often inherit a thin env with no SHELL. Cursor's
	// local shell tool then resolves zsh via PATH; pin an absolute shell that
	// exists so ZshState/BashState get a real binary.
	cmd.Env = ensureBridgeShellEnv(cmd.Env)

	// The ready line goes to STDERR (stdout stays clean for the host), so that
	// is what we scan — and tee to the log / ring buffer for diagnostics.
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return fmt.Errorf("cursor bridge stderr pipe: %w", err)
	}
	cmd.Stdout = io.Discard

	b.recent = newLineRing(bridgeRecentLines)
	if f := openBridgeLog(b.LogDir); f != nil {
		b.logFile = f
		b.logPath = filepath.Join(b.LogDir, bridgeLogFile)
	}
	b.noteStderr(fmt.Sprintf("recur: starting cursor-sdk-bridge bin=%s workspace=%s", b.Bin, b.Workspace))

	// Setpgid keeps a bridge alive after Recur crashes / Force Quits. Reap any
	// leftover from this same launcher (and our pidfile) before we spawn, so
	// orphans do not accumulate across runs. Live bridges from other data dirs'
	// dogfood instances are untouched; orphans from any Recur data dir are reaped.
	reapStaleBridges(b.Bin, b.LogDir, 0, b.noteStderr)

	if err := cmd.Start(); err != nil {
		b.closeLog()
		return fmt.Errorf("start cursor bridge: %w", err)
	}
	b.cmd = cmd

	// Single reaper owning Wait, close-broadcasting the exit — the same pattern
	// as engine.ClaudeRunner, so a cancel path can never race a second wait4.
	b.reaped = make(chan struct{})
	go func() {
		_ = cmd.Wait()
		close(b.reaped)
		// Process exited (crash or kill): drop the pidfile so a later Start
		// does not try to signal a reused PID.
		clearBridgePidfile(b.LogDir)
	}()

	disc, err := b.awaitReady(ctx, stderr)
	if err != nil {
		b.kill()
		b.closeLog()
		return err
	}
	b.disc = disc
	if b.logPath != "" {
		log.Printf("recur: cursor bridge ready on %s (stderr → %s)", disc.URL, b.logPath)
	}

	token := disc.AuthToken
	if token == "" && disc.AuthTokenFile != "" {
		raw, rerr := os.ReadFile(disc.AuthTokenFile)
		if rerr != nil {
			b.kill()
			b.closeLog()
			return fmt.Errorf("cursor bridge: read auth token: %w", rerr)
		}
		token = strings.TrimSpace(string(raw))
	}
	if token == "" {
		b.kill()
		b.closeLog()
		return fmt.Errorf("cursor bridge: discovery carried no auth token")
	}

	writeBridgePidfile(b.LogDir, cmd.Process.Pid)

	cl := newClient(disc.URL, token)
	cl.log = b.noteStderr
	b.client = cl
	b.startedAt = time.Now()
	return nil
}

// awaitReady scans stderr for the ready line. Every line — before and after
// ready — is recorded (redacted) to the log file / ring buffer so bridge
// crashes and Connect `[internal]` faults leave a trail. After ready, scanning
// continues in the background so a chatty bridge can never fill the pipe and
// wedge itself.
func (b *Bridge) awaitReady(ctx context.Context, stderr io.Reader) (Discovery, error) {
	type result struct {
		disc Discovery
		err  error
	}
	done := make(chan result, 1)
	sc := bufio.NewScanner(stderr)
	sc.Buffer(make([]byte, 0, 64*1024), 1024*1024)

	go func() {
		reported := false
		for sc.Scan() {
			line := sc.Text()
			b.noteStderr(line)
			if reported {
				continue
			}
			idx := strings.Index(line, readyPrefix)
			if idx < 0 {
				continue
			}
			var d Discovery
			if err := json.Unmarshal([]byte(line[idx+len(readyPrefix):]), &d); err != nil {
				done <- result{err: fmt.Errorf("cursor bridge: bad discovery json: %w", err)}
				reported = true
				continue
			}
			done <- result{disc: d}
			reported = true
		}
		if !reported {
			done <- result{err: fmt.Errorf("cursor bridge: exited before reporting ready")}
		}
		if err := sc.Err(); err != nil {
			b.noteStderr("recur: bridge stderr scan ended: " + err.Error())
		} else {
			b.noteStderr("recur: bridge stderr closed (process exiting)")
		}
	}()

	select {
	case r := <-done:
		if r.err != nil {
			return Discovery{}, r.err
		}
		if err := validateDiscovery(r.disc); err != nil {
			return Discovery{}, err
		}
		return r.disc, nil
	case <-ctx.Done():
		return Discovery{}, ctx.Err()
	case <-time.After(bridgeStartTimeout):
		return Discovery{}, fmt.Errorf("cursor bridge: not ready after %s", bridgeStartTimeout)
	}
}

// ensureBridgeShellEnv pins SHELL to an absolute binary that exists when the
// inherited value is missing or unusable (common for Dock-launched macOS apps).
func ensureBridgeShellEnv(env []string) []string {
	shell := envValue(env, "SHELL")
	if shellOK(shell) {
		return env
	}
	for _, candidate := range []string{"/bin/zsh", "/bin/bash", "/bin/sh"} {
		if shellOK(candidate) {
			return setEnvValue(env, "SHELL", candidate)
		}
	}
	return env
}

func envValue(env []string, key string) string {
	prefix := key + "="
	for i := len(env) - 1; i >= 0; i-- {
		if strings.HasPrefix(env[i], prefix) {
			return env[i][len(prefix):]
		}
	}
	return ""
}

func setEnvValue(env []string, key, value string) []string {
	return append(dropEnvKey(env, key), key+"="+value)
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

func shellOK(path string) bool {
	if path == "" || !filepath.IsAbs(path) {
		return false
	}
	fi, err := os.Stat(path)
	return err == nil && !fi.IsDir() && fi.Mode()&0o111 != 0
}

// validateDiscovery fails closed on any shape we were not built against. The
// bridge advertises schemaVersion/protocol precisely so hosts can do this.
func validateDiscovery(d Discovery) error {
	if d.SchemaVersion != 1 {
		return fmt.Errorf("cursor bridge: unsupported discovery schemaVersion %d (want 1)", d.SchemaVersion)
	}
	if d.Transport != "tcp" || d.Protocol != "connect" {
		return fmt.Errorf("cursor bridge: unsupported transport %q/%q (want tcp/connect)", d.Transport, d.Protocol)
	}
	if d.URL == "" {
		return fmt.Errorf("cursor bridge: discovery carried no url")
	}
	return nil
}

// Client returns the Connect client bound to this bridge (nil before Start).
func (b *Bridge) Client() *Client { return b.client }

// StartedAt is when Start returned ready. Zero if the client was bound in tests
// without Start — callers then treat run timestamps as unknown.
func (b *Bridge) StartedAt() time.Time {
	if b == nil {
		return time.Time{}
	}
	return b.startedAt
}

// SetStartedAtForTest sets StartedAt without spawning a bridge.
func SetStartedAtForTest(b *Bridge, t time.Time) {
	if b == nil {
		return
	}
	b.startedAt = t
}

// BindClientForTest attaches a Connect client to b without Start. Used by
// engine tests that drive CursorRunner against an httptest stand-in.
func BindClientForTest(b *Bridge, baseURL, token string) {
	if b == nil {
		return
	}
	b.client = newClient(baseURL, token)
}

// EncodeConnectFrame builds a 5-byte-prefixed Connect frame for tests.
func EncodeConnectFrame(endStream bool, payload []byte) []byte {
	flag := byte(0)
	if endStream {
		flag = endStreamFlag
	}
	return encodeFrame(flag, payload)
}

// Discovery returns the ready-line payload (zero value before Start).
func (b *Bridge) Discovery() Discovery { return b.disc }

// Alive reports whether the bridge process is still running. A nil or
// never-started Bridge is not alive. Used by the engine to self-heal: if the
// Node process dies (OOM, kill, crash) the cached Bridge must not be reused,
// or every subsequent CreateAgent fails with connection refused on a dead port.
func (b *Bridge) Alive() bool {
	if b == nil {
		return false
	}
	b.mu.Lock()
	closed, reaped := b.closed, b.reaped
	b.mu.Unlock()
	if closed || reaped == nil {
		return false
	}
	select {
	case <-reaped:
		return false
	default:
		return true
	}
}

// Close asks the bridge to shut down gracefully, then terminates the process
// group. Safe to call more than once.
func (b *Bridge) Close() error {
	b.mu.Lock()
	if b.closed {
		b.mu.Unlock()
		return nil
	}
	b.closed = true
	b.mu.Unlock()

	// Best-effort graceful shutdown: lets the bridge finish flushing local
	// agent state before we signal it.
	if b.client != nil {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		_ = b.client.Shutdown(ctx)
		cancel()
	}
	b.kill()
	clearBridgePidfile(b.LogDir)
	b.closeLog()
	return nil
}

func (b *Bridge) closeLog() {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.logFile != nil {
		_ = b.logFile.Close()
		b.logFile = nil
	}
}

// kill terminates the bridge's whole process group (SIGTERM → grace → SIGKILL)
// and blocks until the reaper observes the exit. Mirrors engine.killGroup.
func (b *Bridge) kill() {
	if b.cmd == nil || b.cmd.Process == nil {
		return
	}
	pgid := b.cmd.Process.Pid // Setpgid → pgid == pid
	_ = syscall.Kill(-pgid, syscall.SIGTERM)
	select {
	case <-b.reaped:
	case <-time.After(bridgeKillGrace):
		_ = syscall.Kill(-pgid, syscall.SIGKILL)
		<-b.reaped
	}
}
