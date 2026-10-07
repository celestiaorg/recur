package engine

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"recur/internal/codex"
	"recur/internal/cursor"
	"recur/internal/domain"
	"recur/internal/store"
)

// Provider selection lives here. The engine holds ONE AgentRunner at a time; it
// is swapped when the user changes the provider in Settings, and the Cursor
// bridge is started lazily on first use (like the MCP server) so a Claude-only
// install never pays the bridge's ~117MB Node cold start.

// bridgeStartTimeout bounds the lazy bridge start.
const bridgeStartTimeout = 90 * time.Second

// maxCursorBridges caps how many idle per-worktree Node bridges stay warm.
// A new worktree evicts the least-recently-used idle entry; in-use bridges
// are kept even if that briefly exceeds the cap (same as CloseCursorBridges
// deferring while a turn is running). Same-worktree agents share one bridge.
const maxCursorBridges = 8

// bridgeEntry is one warm cursor-sdk-bridge bound to a single worktree.
type bridgeEntry struct {
	bridge   *cursor.Bridge
	runner   *CursorRunner
	worktree string
	lastUsed time.Time
}

// cursorProvider owns a pool of Cursor bridges, one per worktree. A single
// shared bridge with cwd "/" (the macOS GUI default) leaves agent shells at
// disk root even when local.cwd is set. Each bridge is started with
// --workspace + cmd.Dir per worktree, multiplexing agents for that worktree
// by agentId.
type cursorProvider struct {
	mu sync.Mutex

	// entries keyed by cleaned absolute worktree path.
	entries map[string]*bridgeEntry

	// key/path record what the live bridges were started with, so a settings
	// change (API key or bridge binary) tears the whole pool down.
	key  string
	path string

	// closeWhenIdle defers Close until in-flight agent turns finish, so turning
	// Cursor off in Settings does not kill a mid-stream run.
	closeWhenIdle bool
}

// ensureRunner returns a CursorRunner backed by a live bridge for worktree,
// starting or restarting that worktree's bridge when needed. The runner is
// reserved (inUse) before the mutex drops; the caller must releaseRunner.
func (p *cursorProvider) ensureRunner(ctx context.Context, set domain.Settings, worktree string) (*CursorRunner, error) {
	p.mu.Lock()
	defer p.mu.Unlock()

	if !set.ProviderEnabled(domain.ProviderCursor) {
		return nil, fmt.Errorf("Cursor is turned off in Settings → Providers")
	}
	if !set.CursorConfigured() {
		return nil, fmt.Errorf("a role is set to a Cursor model but Cursor is not connected (Settings → Providers → Connect)")
	}

	wt, err := normalizeCursorWorktree(worktree)
	if err != nil {
		return nil, err
	}

	dataDir, _ := store.DataDir()
	bin, err := cursor.Locate(dataDir)
	if err != nil {
		return nil, err
	}
	attrPath := cursor.AttributionConfigPath(dataDir)

	if p.entries == nil {
		p.entries = make(map[string]*bridgeEntry)
	}

	// API key / bridge binary change: the old processes were started with the
	// previous credential or binary — drop every entry.
	if (p.key != "" || p.path != "") && (p.key != set.CursorAPIKey || p.path != bin) {
		p.closeAllLocked()
	}

	if e := p.entries[wt]; e != nil && e.bridge != nil && e.bridge.Alive() {
		e.runner.StrictConfig = set.CursorStrictConfig
		e.lastUsed = time.Now()
		p.key, p.path = set.CursorAPIKey, bin
		e.runner.inUse.Add(1)
		return e.runner, nil
	}
	// Dead or missing entry for this worktree: close just that slot.
	if e := p.entries[wt]; e != nil {
		p.closeEntryLocked(wt)
	}

	p.evictLocked(maxCursorBridges - 1)

	// Cold start only: patch Recur's own dataDir bridge (Install also patches
	// on download). Locate only returns that tree. An unrecognized SDK cannot
	// stub getAgentSkills — fail closed and ask for a reinstall.
	root := cursor.OwnedBridgeRoot(dataDir, bin)
	if root == "" {
		return nil, fmt.Errorf("cursor bridge is not installed in the Recur data dir — install it from Settings")
	}
	if perr := cursor.EnsureAttributionPatch(root); perr != nil {
		return nil, fmt.Errorf("cursor bridge patch failed; reinstall from Settings: %w", perr)
	}
	if attrPath != "" {
		if werr := cursor.WriteAttributionConfig(attrPath, set.CursorAttributeCommits, set.CursorAttributePRs); werr != nil {
			return nil, fmt.Errorf("cursor attribution config: %w", werr)
		}
	}

	logDir := ""
	if dir, lerr := store.LogsDir(dataDir); lerr == nil {
		// Per-worktree log+pidfile so concurrent bridges do not clobber one
		// cursor-bridge.pid (reaping and crash recovery stay scoped).
		logDir = filepath.Join(dir, "cursor-bridges", worktreeLogKey(wt))
	}
	b := &cursor.Bridge{
		Bin:                   bin,
		APIKey:                set.CursorAPIKey,
		Workspace:             wt,
		LogDir:                logDir,
		AttributionConfigPath: attrPath,
	}
	sctx, cancel := context.WithTimeout(ctx, bridgeStartTimeout)
	defer cancel()
	if err := b.Start(sctx); err != nil {
		return nil, fmt.Errorf("start cursor bridge: %w", err)
	}

	// Fail fast and by name if the bridge is too old for what CursorRunner does,
	// rather than surfacing an opaque RPC error mid-loop.
	v, err := b.Client().GetVersion(sctx)
	if err != nil {
		_ = b.Close()
		return nil, fmt.Errorf("cursor bridge handshake: %w", err)
	}
	for _, want := range []string{"agent.create", "agent.send"} {
		if !v.Has(want) {
			_ = b.Close()
			return nil, fmt.Errorf("cursor bridge %s lacks capability %q — update it from Settings", v.BridgeVersion, want)
		}
	}

	r := NewCursorRunner(b, set.CursorAPIKey)
	r.StrictConfig = set.CursorStrictConfig
	r.skipObserve = !v.Has("run.observe")
	p.entries[wt] = &bridgeEntry{
		bridge:   b,
		runner:   r,
		worktree: wt,
		lastUsed: time.Now(),
	}
	p.key, p.path = set.CursorAPIKey, bin
	r.inUse.Add(1)
	return r, nil
}

// normalizeCursorWorktree requires an existing absolute directory — the loop's
// worktree. Empty paths used to silently start a bridge at "/", which left
// agent shells at disk root.
func normalizeCursorWorktree(worktree string) (string, error) {
	worktree = strings.TrimSpace(worktree)
	if worktree == "" {
		return "", fmt.Errorf("cursor bridge: worktree path is required (refusing to start at /)")
	}
	abs, err := filepath.Abs(worktree)
	if err != nil {
		return "", fmt.Errorf("cursor bridge: worktree %q: %w", worktree, err)
	}
	abs = filepath.Clean(abs)
	if abs == "/" {
		return "", fmt.Errorf("cursor bridge: worktree path is required (refusing to start at /)")
	}
	fi, err := os.Stat(abs)
	if err != nil || !fi.IsDir() {
		return "", fmt.Errorf("cursor bridge: worktree %q is unavailable", abs)
	}
	return abs, nil
}

// worktreeLogKey is a short stable directory name for per-bridge logs/pidfiles.
func worktreeLogKey(worktree string) string {
	sum := sha256.Sum256([]byte(worktree))
	return hex.EncodeToString(sum[:8])
}

func (e *bridgeEntry) busy() bool {
	return e != nil && e.runner != nil && e.runner.inUse.Load() > 0
}

// releaseRunner drops one ensureRunner handout. When the last user of this
// runner goes idle, overflow above maxCursorBridges is trimmed.
func (p *cursorProvider) releaseRunner(r *CursorRunner) {
	if r == nil {
		return
	}
	for {
		n := r.inUse.Load()
		if n <= 0 {
			return
		}
		if r.inUse.CompareAndSwap(n, n-1) {
			if n > 1 {
				return
			}
			break
		}
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	p.evictLocked(maxCursorBridges)
}

func cursorRunnerOf(r AgentRunner) *CursorRunner {
	switch v := r.(type) {
	case *CursorRunner:
		return v
	case mirroringRunner:
		return cursorRunnerOf(v.inner)
	default:
		return nil
	}
}

// evictLocked closes least-recently-used idle entries until len(entries) <= max.
// In-use bridges are skipped so a long turn is not SIGTERM'd by another
// worktree's cold start. Caller holds mu. max < 0 means no eviction.
func (p *cursorProvider) evictLocked(max int) {
	if max < 0 || len(p.entries) <= max {
		return
	}
	for len(p.entries) > max {
		victim := p.lruIdleLocked()
		if victim == "" {
			return
		}
		p.closeEntryLocked(victim)
	}
}

func (p *cursorProvider) lruIdleLocked() string {
	var oldest string
	var oldestAt time.Time
	first := true
	for wt, e := range p.entries {
		if e.busy() {
			continue
		}
		if first || e.lastUsed.Before(oldestAt) {
			oldest, oldestAt, first = wt, e.lastUsed, false
		}
	}
	return oldest
}

// invalidate drops the cached bridge for worktree (or the whole pool when
// worktree is empty) so the next ensureRunner starts a fresh one. Used when an
// RPC fails with a transport error that means the process is gone but Wait has
// not reaped it yet (Alive still true for a moment).
func (p *cursorProvider) invalidate(worktree string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if strings.TrimSpace(worktree) == "" {
		p.closeAllLocked()
		return
	}
	wt, err := normalizeCursorWorktree(worktree)
	if err != nil {
		// Path may already be gone after archive/delete — still drop any key
		// that matches the cleaned absolute form when we can.
		wt = filepath.Clean(worktree)
		if abs, aerr := filepath.Abs(wt); aerr == nil {
			wt = abs
		}
	}
	p.closeEntryLocked(wt)
}

func (p *cursorProvider) closeEntryLocked(worktree string) {
	e := p.entries[worktree]
	if e == nil {
		return
	}
	if e.bridge != nil {
		_ = e.bridge.Close()
	}
	delete(p.entries, worktree)
}

func (p *cursorProvider) closeAllLocked() {
	for wt := range p.entries {
		p.closeEntryLocked(wt)
	}
	p.key, p.path = "", ""
}

// Close stops every bridge process.
func (p *cursorProvider) Close() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.closeWhenIdle = false
	p.closeAllLocked()
}

func (p *cursorProvider) requestCloseWhenIdle() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.closeWhenIdle = true
}

func (p *cursorProvider) cancelCloseWhenIdle() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.closeWhenIdle = false
}

func (p *cursorProvider) closeIfIdleRequested(idle bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.closeWhenIdle || !idle {
		return
	}
	p.closeWhenIdle = false
	p.closeAllLocked()
}

// CloseCursorBridges tears down every warm Cursor bridge. Called when the user
// turns Cursor off in Settings so the Node processes do not linger until exit.
// In-flight agent turns keep their bridge until they finish.
func (e *Engine) CloseCursorBridges() {
	if e == nil {
		return
	}
	if e.gov != nil && e.gov.AgentsRunning() > 0 {
		e.cursor.requestCloseWhenIdle()
		// Last agent may have released between the check and the flag.
		if e.gov.AgentsRunning() == 0 {
			e.cursor.closeIfIdleRequested(true)
		}
		return
	}
	e.cursor.Close()
}

// CancelDeferredCursorClose drops a pending close-when-idle so turning Cursor
// back on before in-flight turns finish does not tear the pool down afterward.
func (e *Engine) CancelDeferredCursorClose() {
	if e == nil {
		return
	}
	e.cursor.cancelCloseWhenIdle()
}

// needsPermissionPromptTool reports whether provider requires
// mcp__recur__permission_request on MCP bindings. Each provider type owns the
// answer (NeedsPermissionPromptTool); this only maps slug → that declaration
// so registerMCP can ask before a live runner exists (Cursor ensureRunner
// starts a bridge and must not run just to mint a token).
func needsPermissionPromptTool(provider string) bool {
	switch provider {
	case domain.ProviderCursor:
		return (*CursorRunner)(nil).NeedsPermissionPromptTool()
	case domain.ProviderCodex:
		return (*CodexRunner)(nil).NeedsPermissionPromptTool()
	case domain.ProviderClaudeCode, "":
		return (*ClaudeRunner)(nil).NeedsPermissionPromptTool()
	default:
		return (*HarnessRunner)(nil).NeedsPermissionPromptTool()
	}
}

// runnerFor returns the AgentRunner for a provider slug.
//
// A test-injected runner (SetRunner) always wins — the ScriptedRunner must not
// be displaced by a settings blob, or the whole suite would start trying to
// reach a real provider.
//
// If Cursor is selected but cannot start (missing bridge, missing or rejected
// key), this returns an error rather than silently falling back to Claude Code:
// quietly running a different model than the user chose — and billing them for
// it — is worse than a visible failure.
//
// worktree is required when provider is Cursor (the bridge is bound to it).
func (e *Engine) runnerFor(ctx context.Context, provider, worktree string) (AgentRunner, error) {
	r, err := e.resolveRunner(ctx, provider, worktree)
	if err != nil {
		return nil, err
	}
	if r == nil {
		return nil, fmt.Errorf("engine: no agent runner for provider %q", provider)
	}
	// ONE wrap point for every provider. A branch that returned an unwrapped
	// runner would bypass the session mirror completely — no transcript, no
	// tool-call blocks, no session.delta events, so the UI shows neither
	// streaming nor tool calls while the run itself appears to succeed. That is
	// exactly what happened to the first Cursor build, so the wrap lives here
	// rather than in each branch where it can be forgotten. withMirroring is
	// idempotent, so already-wrapped runners pass through untouched.
	return withMirroring(e, r), nil
}

// resolveRunner picks the raw runner for a provider. Callers must go through
// runnerFor, which applies the mirror.
func (e *Engine) resolveRunner(ctx context.Context, provider, worktree string) (AgentRunner, error) {
	if e.runnerOverrideSet {
		if e.runnerOverride == nil {
			return nil, fmt.Errorf("engine: agent runner was explicitly set to nil; no turn can run")
		}
		return e.runnerOverride, nil
	}
	switch provider {
	case domain.ProviderCursor:
		// Returning ensureRunner's (*CursorRunner, error) directly would box a
		// nil pointer into a NON-nil AgentRunner on the error path, so callers
		// checking `runner != nil` would sail on and nil-deref. Convert explicitly.
		r, err := e.cursor.ensureRunner(ctx, e.settings(), worktree)
		if err != nil {
			return nil, err
		}
		return r, nil
	case domain.ProviderCodex:
		r, err := e.codexRunner(ctx)
		if err != nil {
			return nil, err
		}
		return r, nil
	case domain.ProviderClaudeCode, "":
		if !e.settings().ProviderEnabled(domain.ProviderClaudeCode) {
			return nil, fmt.Errorf("Claude Code is turned off in Settings → Providers")
		}
		return e.runner, nil
	default:
		p := e.settings().CompatibleByID(provider)
		if p == nil {
			return nil, fmt.Errorf("unknown agent provider %q — pick a model from Settings → Models", provider)
		}
		if !e.settings().ProviderEnabled(provider) {
			return nil, fmt.Errorf("%s is turned off in Settings → Providers", p.Name)
		}
		if !p.Configured() {
			return nil, fmt.Errorf("%s is not configured (Settings → Providers)", p.Name)
		}
		return e.newHarnessRunner(*p), nil
	}
}

// codexRunner builds a CodexRunner from current settings. App-server is spawned
// per Run (MCP URL is per-agent), so there is no warm pool here.
func (e *Engine) codexRunner(_ context.Context) (*CodexRunner, error) {
	set := e.settings()
	if !set.ProviderEnabled(domain.ProviderCodex) {
		return nil, fmt.Errorf("Codex is turned off in Settings → Providers")
	}
	if !set.CodexConfigured() {
		return nil, fmt.Errorf("a role is set to a Codex model but Codex is not configured (Settings → Providers → Codex)")
	}
	bin, err := codex.Locate(set.CodexBinPath)
	if err != nil {
		return nil, err
	}
	apiKey := set.CodexAPIKey
	if set.CodexChatGPTAuth {
		apiKey = ""
	}
	return &CodexRunner{
		Bin:           bin,
		APIKey:        apiKey,
		ChatGPTAuth:   set.CodexChatGPTAuth,
		NetworkAccess: set.CodexNetworkAccess,
		AskPermission: func(ctx context.Context, req domain.PermissionRequest) (domain.PermissionDecision, error) {
			if req.Surface == "" {
				req.Surface = e.permissionSurfaceFor(req.WorkspaceID, req.AgentCode)
			}
			return e.perms.request(ctx, req)
		},
	}, nil
}

// runAgent is the single choke point through which every agent turn is
// dispatched, so provider routing happens in exactly one place.
//
// The provider comes from the spec's qualified model ref (`provider/id`), which
// is why roles can run on different providers within one loop: an orchestrator
// on Claude Code and a coder on Composer is just two different model picks. The
// bare model id is handed to the runner — providers know nothing about refs.
//
// Cursor self-heal: if that worktree's bridge dies before the turn has begun
// streaming (CreateAgent connection refused), invalidate that pool entry,
// start a fresh bridge, and retry the turn once. Mid-stream Send failures
// also mean the bridge is gone — those still invalidate so the next
// turn/Retry is not stuck on a dead port — but they are NOT auto-retried
// here: events may already have been mirrored, and re-running the whole turn
// would duplicate the transcript.
func (e *Engine) runAgent(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	applySkillGuidance(&spec)
	applyCommentHygiene(&spec)
	// Every real agent turn gets Settings → MCP. SkipThinking one-shots
	// (title, commit message) are not agents.
	if !spec.SkipThinking {
		e.attachWorkspaceUserMCP(&spec, spec.WorkspaceID, linearModeFor(spec))
	}
	ref := domain.ParseModelRef(spec.Model)
	r, err := e.runnerFor(ctx, ref.Provider, spec.Worktree)
	if err != nil {
		return Result{}, err
	}
	// Runners get the bare id; the mirror gets the provider explicitly, since the
	// prefix is gone by the time the mirroring wrapper sees the spec.
	spec.Model = ref.ID
	spec.Provider = ref.Provider
	// Cockpit is a chat transcript now — same thinking/text deltas as pair chat.
	// SkipThinking one-shots (title) stay off so they don't pay for stream_event.
	if !spec.SkipThinking {
		spec.IncludePartials = true
	}
	run := func(r AgentRunner) (Result, error) {
		if cr := cursorRunnerOf(r); cr != nil {
			defer e.cursor.releaseRunner(cr)
		}
		return r.Run(ctx, spec, emit)
	}
	res, err := run(r)
	if err != nil && ref.Provider == domain.ProviderCursor && !e.settings().ProviderEnabled(domain.ProviderCursor) &&
		!errors.Is(err, context.Canceled) && !errors.Is(err, context.DeadlineExceeded) {
		e.cursor.invalidate(spec.Worktree)
		return res, fmt.Errorf("Cursor is turned off in Settings → Providers: %w", err)
	}
	if err == nil || ref.Provider != domain.ProviderCursor || !cursor.BridgeUnreachable(err) {
		return res, err
	}
	e.cursor.invalidate(spec.Worktree)
	if !cursorDiedBeforeTurn(err) {
		return res, err
	}
	// Bridge died on CreateAgent — nothing streamed yet, safe to retry once.
	orig := err
	r, err = e.runnerFor(ctx, ref.Provider, spec.Worktree)
	if err != nil {
		return res, fmt.Errorf("%w (bridge restart failed: %v)", orig, err)
	}
	return run(r)
}

// cursorDiedBeforeTurn reports whether a Cursor failure happened before any
// run-stream events could have been emitted (CreateAgent/ResumeAgent), as
// opposed to a mid-Send transport drop.
func cursorDiedBeforeTurn(err error) bool {
	if err == nil {
		return false
	}
	// CursorRunner wraps pre-stream failures as "cursor create agent: …".
	// Send-path failures use "cursor run:" / "cursor auth failed:".
	return strings.Contains(err.Error(), "cursor create agent:")
}
