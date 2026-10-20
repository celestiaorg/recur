// Package engine is the concurrency spine (PRD §6): one goroutine per loop, the
// event Bus (fan-in from loops → fan-out to UI + persistence), the resource
// Governor, the pluggable AgentRunner, the WorktreeManager, and the Store. M2
// replaces the M0 fake emitter with real per-loop goroutines that run a Coder via
// the runner, commit the worktree, and stream real events.
package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"html"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"recur/internal/agenttools"
	"recur/internal/cursor"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/mcp"
	"recur/internal/power"
	"recur/internal/runconfig"
	"recur/internal/store"
	"recur/internal/term"
)

// errAlreadyDispatched is returned by dispatch when the loop already has a live map
// entry — i.e. a concurrent dispatch beat us to it (D26 belt-and-suspenders guard).
// Callers treat it as benign: release the just-acquired slot and leave the loop
// alone (the other dispatch owns it), rather than marking it failed.
var errAlreadyDispatched = errors.New("loop already dispatched")

// errLoopOwnerGone is returned when CloseWorkspaceChat cleared the conductor
// before dispatch registered the goroutine. Callers treat it as benign: release
// the slot and do not mark the workspace failed.
var errLoopOwnerGone = errors.New("loop owner closed")

func benignDispatchErr(err error) bool {
	return errors.Is(err, errAlreadyDispatched) || errors.Is(err, errLoopOwnerGone)
}

// Engine owns everything (PRD §6.1): the store, the event bus, the resource
// governor, the agent runner, the worktree root, and the running loop goroutines.
type Engine struct {
	// scheduleMu guards claims and worker launches; Shutdown closes the timer before joining workers.
	scheduleMu   sync.Mutex
	scheduleStop chan struct{}
	scheduleDone chan struct{}
	scheduleWG   sync.WaitGroup
	// scheduleWait tracks launched workspaces whose first turn settled into
	// "waiting" so tick can clear ScheduledPromptPending without a run row.
	scheduleWait sync.Map

	Store *store.Store
	Bus   *Bus

	gov *Governor

	// runner is the DEFAULT runner for the Claude Code provider (ClaudeRunner in
	// production). runnerOverride, when set by SetRunner, wins over both it and
	// provider selection — that is how tests inject a ScriptedRunner without a
	// settings blob being able to redirect turns at a real provider.
	runner         AgentRunner
	runnerOverride AgentRunner
	// runnerOverrideSet records that SetRunner was CALLED, separately from what it
	// was called with. A test that injects nil (because it never dispatches) must
	// fail closed rather than silently fall through to the production runner and
	// make a live API call.
	runnerOverrideSet bool

	// nowFn, when set, is the clock Briefing uses (tests inject a frozen time).
	nowFn func() time.Time

	// cursor owns the shared cursor-sdk-bridge process and the CursorRunner
	// bound to it, started lazily on the first turn when the Cursor provider is
	// selected (see provider.go).
	cursor cursorProvider

	// todos is the native-harness TodoWrite store (session-scoped, in-process).
	todos agenttools.TodoStore

	// mcpSrv is the Recur MCP HTTP server (PRD §8.3, D7). Agents connect to it
	// over localhost with per-agent tokens; each tool call becomes a typed engine
	// callback (engineReporter). It is started lazily on the first StartWorkspace (or
	// explicitly via StartMCP) so tests that never start a loop don't bind a port.
	mcpSrv *mcp.Server
	// mcpMu guards the lazy start. A failed start is NOT cached: binding a
	// localhost port can fail transiently, and caching the error would leave
	// every later turn — including an explicit Retry — permanently broken until
	// the app restarts.
	mcpMu      sync.Mutex
	mcpStarted bool

	// worktreeRoot is the managed worktrees/ parent; per-loop managers are built
	// per project repo at StartWorkspace time. There is NO shared mutable *git.Manager on
	// the engine (D22): each loop owns its own manager (Loop.git), so concurrent
	// StartWorkspaces and a running loop's commit never race on engine state.
	worktreeRoot string

	// gitWarning is set at construction when git is missing or older than
	// gitpkg.MinVersion. Surfaced on Resource and as a boot notify.
	gitWarning      string
	gitWarningTitle string

	loops sync.Map // workspaceID -> *runningLoop

	// loopRenameNudge holds the rename prompt computed at loop engage, keyed by
	// workspaceID+loopMapSep+chatCode. runPlanner LoadAndDeletes it once.
	loopRenameNudge sync.Map

	// chats tracks in-flight chat turns (solo/pair PA* + Maestro MA) so the UI
	// can interrupt them. Keyed by workspaceID+"\x00"+code → *chatTurn.
	chats sync.Map
	// childWaves maps a live parent chat turn (*chatTurn) to the subagent
	// wave it is creating, so siblings from one turn share a card.
	childWaves sync.Map
	// chatChildWatchers holds child ids with a live watchChatChild, so a
	// reopened child and a restart rearm cannot both report it.
	chatChildWatchers sync.Map
	// fanOutPoll is how often fan-out waits and child watchers re-check
	// children. Fixed after New; tests shorten it before spawning children.
	fanOutPoll time.Duration
	// mergeChildLocks serializes merge_child per parent workspace so two
	// tabs cannot git-merge into the same worktree at once.
	mergeChildLocks sync.Map
	// resumingChats holds chatTurnKeys of interrupted chats between AutoResume
	// launching them and their resumed turn finishing: Recover healed their
	// agents to idle, so without this a subagent looks settled and reports
	// "finished, no work" before its resumed turn even registers.
	resumingChats sync.Map
	// chatTabs FIFO-orders SendWorkspaceMessage per tab (inject vs register)
	// and holds a stop generation so a steer RPC cannot restart after Stop.
	chatTabs sync.Map

	// chatLife serializes reopen / send prologue / closed-chat delete per tab
	// so those RPCs cannot interleave (open a tab whose transcript was just
	// deleted, or delete underneath a turn that is about to register).
	// chatLifeMu guards the map and each slot's refs so retain/retire is atomic.
	chatLifeMu sync.Mutex
	chatLife   map[string]*lifeLock

	// liveContinues holds unique gens for in-flight switch_mode remounts.
	// Allocated at noteModeContinue and dropped when the remount runs, is
	// cancelled, or the chat/workspace is torn down — no permanent per-ID
	// tombstones.
	continueGenMu   sync.Mutex
	nextContinueGen uint64
	liveContinues   map[uint64]continueID

	// lastChatSurfaces remembers the looking-at surface for Retry. Transcripts
	// store the peeled prompt (no ⟦recur-surface⟧ marker), and WorkspaceView
	// skips attachChatSurface on retry; without this, a PR-dock retry on a
	// linked worktree falls through to ChatSurfaceWorkspace.
	lastChatSurfaces sync.Map

	// earlyTitles records titles ApplyProvisionalTitles stamped from a queued
	// send: workspace ID -> earlyWorkspaceTitle, workspace ID+"\x00"+code ->
	// chat tab title. The first real send consumes them (dropEarlyTitles).
	earlyTitles sync.Map

	// interruptedChats is filled by Recover with PA*/MA tabs that were persisted
	// live (working/directing) with no goroutine — an app quit/crash killed the
	// turn. AutoResume continues those chats after the UI bus is wired, matching
	// loop auto-resume. Specialists (CO/RV/…) are idled but not listed here; the
	// loop they belong to is what AutoResume re-dispatches.
	interruptedChats []interruptedChat

	// prConvBusy / prConvLast throttle GitHub conversation snapshots so GetPR
	// (polled for live checks) cannot spawn a parallel gh snapshot every tick.
	prConvBusy sync.Map // prID → struct{}
	prConvLast sync.Map // prID → time.Time

	// prDiffBusy / prDiffLast throttle gh-pr-diff hydration the same way —
	// GetPR is polled every 15s and a 20s `gh pr diff` must not run on the RPC.
	prDiffBusy sync.Map // prID → struct{}
	prDiffLast sync.Map // prID → time.Time

	// dispatchMu serializes the "promote a loop to running" critical section
	// (D26): selecting a target loop record, confirming it is not already running,
	// acquiring its loop-slot, and registering it in the loops map. It is SEPARATE
	// from any store lock. Without it, two concurrent drains/Resumes can both pick
	// the same queued loop, both pass the non-atomic "already running?" check, and
	// both dispatch it — leaking a cancel, racing two `git worktree add`, and
	// releasing the slot twice. A dispatched goroutine never blocks on this mutex,
	// so there is no deadlock.
	dispatchMu sync.Mutex

	// sessionEnsureMu serializes ensureOwnedAgentSession's lookup-then-create so
	// concurrent Remember selection + group (and send) cannot both observe a miss
	// and insert duplicate durable rows for the same workspace+code.
	sessionEnsureMu sync.Mutex

	// promoteHook is a TEST-ONLY seam (nil in production). When set, it is called
	// inside the dispatchMu-protected promotion critical section — after the target
	// loop has been selected and confirmed not-running but before it is dispatched —
	// so the concurrency regression test can deterministically pile several drains
	// into the exact double-dispatch window. Because the fix holds dispatchMu across
	// this section, a blocking hook serializes (one drain at a time) and proves the
	// guard; without the fix, concurrent drains all reach it at once.
	promoteHook func()

	// startWalkHook is TEST-ONLY (nil in production). Called after
	// startChatLoopWalk releases lockChatLife and before Resume, so a test can
	// close the tab in that window.
	startWalkHook func()

	// childQueueHook is TEST-ONLY (nil in production). Called after a fan-out
	// child is inserted as queued and before ParentID/Plan are written, so a
	// test can drainQueued in that window. startStepChild holds dispatchMu
	// across insert, init, and dispatch so a concurrent drain blocks until the
	// row is complete (or already running).
	childQueueHook func()

	// loopWG tracks every live loop goroutine so Shutdown can join them with a
	// bounded deadline (D24).
	loopWG sync.WaitGroup

	// escPrecision instruments escalations raised vs answered (PRD §13, §21.1).
	escPrecision escPrecisionCounter

	// perms brokers agent permission requests to the human when
	// Settings.BypassPermissions is off (--permission-prompt-tool).
	perms *permissionBroker

	// askUsers brokers interactive ask_user prompts (block turn + inbox; no
	// StateEscalated).
	askUsers *askUserBroker

	// modeContinues is a mid-turn switch_mode remount: park the current turn
	// and start a continuation with the new chatModeShape. Keyed like chats.
	// modeRemountDepth bounds flip-flop remount chains on the same tab.
	modeContinueMu   sync.Mutex
	modeContinues    map[string]pendingModeContinue
	modeRemountDepth map[string]int

	// itersBase is the status-bar "iters today" baseline captured at construction
	// (the seeded/persisted value). publishResource shows itersBase + the governor's
	// real completed-iteration count, so a fresh boot keeps the seeded demo number and
	// real loop work adds to it live (PRD §6.4, §16.1).
	itersBase int

	// idMu serializes cozy-kitchen id allocation so two concurrent creates
	// cannot claim the same project.adj-dish pair before either is persisted.
	idMu sync.Mutex

	// streamSeq is the last published live-stream seq per workspace+agent+kind
	// (text / thinking). sessionBroadcast seeds from it so consecutive turns keep
	// a process-wide monotonic sequence — the frontend drops seq <= last-seen, and
	// a per-turn reset made turn N+1's snapshots look stale (token estimates still
	// applied because they carry no seq).
	streamSeqMu sync.Mutex
	streamSeq   map[string]int

	// droppedComments remembers recently deleted GH line notes so a concurrent
	// conversation refresh cannot resurrect them from a stale snapshot.
	droppedComments   map[string]time.Time
	droppedCommentsMu sync.Mutex

	// threadResolved remembers a just-toggled review thread so a concurrent
	// conversation refresh cannot overwrite it with a stale GH snapshot.
	threadResolved   map[string]threadResolveHold
	threadResolvedMu sync.Mutex

	// commentReactions remembers a just-toggled emoji so a concurrent
	// conversation refresh cannot overwrite it with a stale GH snapshot.
	commentReactions   map[string]commentReactionHold
	commentReactionsMu sync.Mutex

	// Active-worktree watcher (fswatch.go): at most one workspace is watched via
	// FSEvents at a time — the one on screen.
	watchMu     sync.Mutex
	watchCancel context.CancelFunc
	watchedID   string // workspace currently watched; empty when none

	// Debounced sidebar DiffStat refresh (diff_sync.go) keyed by workspace id.
	diffSyncMu     sync.Mutex
	diffSyncTimers map[string]*time.Timer
	diffSyncGen    map[string]uint64      // bumps per sync start; stale writers skip store
	diffApplyMu    map[string]*sync.Mutex // serializes store write/publish per workspace
	// Single-flight for ScheduleRefreshAllWorkspaceDiffs (ListWorkspaces hot path).
	diffRefreshAllMu      sync.Mutex
	diffRefreshAllBusy    bool
	diffRefreshAllPending bool

	// chatFocus is the chat tab the human is currently viewing (workspace + code).
	// Used so a turn finishing on the focused tab does not raise an unseen badge.
	// Cleared when the Observatory stops watching a workspace (user navigated away).
	// "Really visible" also requires appFocused && !uiOccludesChat (D-attention):
	// OS window focus + no modal overlay (New workspace, palettes, permission dialog).
	chatFocusMu sync.Mutex
	// chatUnseenMu orders Unseen mutation + attention.updated so MarkWorkspaceChatSeen
	// and MarkAllChatsSeen cannot publish a clear and then lose to a stale
	// flagChatUnseen / updateAgentStatus true.
	chatUnseenMu   sync.Mutex
	chatFocus      struct{ workspaceID, code string }
	appFocused     bool // OS key window; default true (set in New)
	uiOccludesChat bool // frontend modal/dialog covering the chat surface

	// creating tracks in-flight worktree allocation for BeginCreateWorkspace
	// (and the loop CreateWorkspace path). Delete/Shutdown cancel these so a
	// discarded workspace does not keep `git worktree add` / setup running.
	creating sync.Map // workspaceID -> *creatingAlloc

	// forkOverlay is the source workspace for a dest that still needs its
	// uncommitted tree copied before leaving phase "creating".
	forkOverlay sync.Map // destID -> domain.Workspace

	// prHeadCheckout is the GitHub PR whose tip a creating workspace should
	// check out instead of the project base (CreateWorkspaceFromPR). Stored
	// before startAllocate so the goroutine cannot miss it.
	prHeadCheckout sync.Map // workspaceID -> prHeadCheckoutSpec

	// wsUpstreamCache is @{upstream}'s branch name per workspace (empty string
	// means none). Filled by lookupWorkspaceUpstreamBranch, overwritten by each
	// PR sweep's indexWorkspacesByPRHead; dropped when the live checkout branch
	// changes.
	wsUpstreamCache sync.Map // workspaceID -> string

	// syncGitHubPRsMu / syncGitHubPRsWait collapse concurrent SyncGitHubPRs
	// into one sweep (boot, PR-list open, background ticker). syncGitHubPRsAt is last finish
	// time so a short TTL can skip a repeat `gh pr list` storm.
	syncGitHubPRsMu   sync.Mutex
	syncGitHubPRsWait chan struct{}
	syncGitHubPRsAt   time.Time

	// deleting holds workspace ids whose store record is gone but whose
	// worktree / data-dir teardown is still running. newWorkspaceID treats
	// these as taken so a recycled kitchen id cannot land on a path that is
	// still being removed.
	deleting sync.Map // workspaceID -> struct{}

	// hoisting is a short-lived tombstone while archivePRChatHosts remaps and
	// moves chats. chatLifeReserved refuses new lockChatLife so a send cannot
	// land mid-hoist on the leftover host or the destination worktree.
	// hoistMu makes holdPRChatHoist exclusive with itself and with
	// reserveDeleting: overlapping hoists wait, and a Remove/Delete of any
	// involved id refuses (and is refused by) the other so chats cannot move
	// onto a dying dest or be torn down after landing.
	hoistMu  sync.Mutex
	hoisting sync.Map // workspaceID -> struct{}
	// chatHoistAlias maps leftover-host chatTurnKey → dest chatTurnKey after a
	// PR-host hoist so a send that still names the archived host follows.
	chatHoistAlias sync.Map

	// teardownWG tracks background DeleteWorkspace / RemoveWorkspace disk-removal
	// goroutines so Shutdown can join them (same bounded deadline as loopWG).
	// teardownMu serializes Add against Shutdown's Wait (WaitGroup forbids
	// Add-from-zero concurrent with Wait) and shuttingDown stops new spawns once
	// quit starts.
	teardownMu   sync.Mutex
	teardownWG   sync.WaitGroup
	shuttingDown bool

	// openMainMu serializes OpenMainWorkspace's find-or-create so two concurrent
	// opens (a sidebar double-click fires two RPCs on separate Wails goroutines)
	// can't both pass the reuse scan and add a DUPLICATE main record — the store
	// has no (ProjectID, IsMain) uniqueness constraint, and a duplicate is
	// unrecoverable from the UI (filtered out of every list; no Discard/Delete for
	// main workspaces).
	openMainMu sync.Mutex

	// keepAwake holds a macOS caffeinate assertion while agents are running and
	// Settings.PreventSleepWhileAgentsRun is on (see keepawake.go).
	keepAwake   power.Keeper
	keepAwakeMu sync.Mutex // serializes syncKeepAwake read+Set

	// terminals is the Observatory PTY manager (nil in tests that never open a
	// dock). Chat @terminal wrap and MCP list/read_terminals read from it.
	terminals *term.Manager
}

// runningLoop bundles a Loop with its context + cancel so commands and shutdown
// can reach it.
type runningLoop struct {
	loop   *Loop
	cancel context.CancelFunc
}

// New creates the Engine over a store with the PRODUCTION ClaudeRunner and a
// Governor sized from the store's Settings. main wires the Wails + persistence
// sinks onto the Bus.
func New(st *store.Store) *Engine {
	set := st.Settings()
	wtRoot, _ := store.WorktreesDir()
	e := &Engine{
		Store:            st,
		Bus:              NewBus(),
		gov:              NewGovernor(domain.EffectiveMaxLoops(set.MaxLoops), domain.EffectiveMaxAgents(set.MaxAgents)),
		worktreeRoot:     wtRoot,
		fanOutPoll:       defaultFanOutPoll,
		itersBase:        st.Resource().ItersToday, // seeded/persisted baseline (PRD §6.4)
		appFocused:       true,                     // assume key until WindowLostFocus; zero-value would suppress all "seen"
		modeContinues:    map[string]pendingModeContinue{},
		modeRemountDepth: map[string]int{},
		chatLife:         map[string]*lifeLock{},
		liveContinues:    map[uint64]continueID{},
	}
	// Every turn funnels through e.runAgent, which resolves the provider from the
	// spec's model ref and lands here for Claude Code. Wrapping the runner is the
	// whole S1 mirror wiring — specialist and maestro turns all
	// get a durable, lossless transcript with no per-call-site changes.
	e.runner = withMirroring(e, NewClaudeRunner())
	// The MCP server is constructed here (backed by the engine's typed reporter)
	// but not started until a loop actually runs (ensureMCP), so a test that never
	// starts a loop binds no port.
	e.mcpSrv = mcp.New(engineReporter{eng: e})
	e.perms = newPermissionBroker()
	e.perms.notify = e.publishPermissionRequested
	e.perms.resolved = e.publishPermissionResolved
	e.askUsers = newAskUserBroker()
	e.askUsers.notify = e.publishAskUserRequested
	e.askUsers.resolved = e.publishAskUserResolved
	e.gitWarningTitle, e.gitWarning = gitpkg.UnsupportedAlert(gitpkg.Probe())
	return e
}

// GitWarning is the boot-time alert when git is missing or older than
// gitpkg.MinVersion. Empty when the install is fine.
func (e *Engine) GitWarning() string { return e.gitWarning }

// CheckGit publishes a notify (OS notification + resource.updated) when git is
// missing or too old. Probe already ran in New; this is the user-facing flag.
func (e *Engine) CheckGit() {
	if e.gitWarning == "" {
		return
	}
	log.Printf("git: %s", e.gitWarning)
	title := e.gitWarningTitle
	if title == "" {
		title = "Git is too old"
	}
	e.Bus.Publish(Event{Name: "notify", Data: map[string]any{
		"title": title,
		"body":  e.gitWarning,
	}})
	e.publishResource()
}

// CheckCursorBridge publishes a notify when the installed Cursor bridge is
// behind the cursor-sdk pin Recur would install. Settings already labels this
// "update available", but nobody opens Settings to find out why Cursor turns
// started failing — the stale bridge has to announce itself at boot.
func (e *Engine) CheckCursorBridge() {
	dataDir, _ := store.DataDir()
	bin, err := cursor.Locate(dataDir)
	if err != nil {
		return
	}
	have, want := cursor.SDKVersion(bin), cursor.PinnedSDKVersion()
	if have == "" || have == want {
		return
	}
	log.Printf("cursor: bridge sdk %s installed, %s pinned", have, want)
	e.Bus.Publish(Event{Name: "notify", Data: map[string]any{
		"title": "Cursor bridge update available",
		"body":  fmt.Sprintf("Installed cursor-sdk %s, Recur expects %s. Update it in Settings > Providers > Cursor.", have, want),
	}})
}

// publishPermissionRequested surfaces a live permission request. A loop turn
// also raises an inbox row: nobody is necessarily watching the autopilot, and a
// request nobody sees is a turn that stalls until it times out.
func (e *Engine) publishPermissionRequested(req domain.PermissionRequest) {
	e.Bus.Publish(Event{Name: "permission.requested", Data: req})
	if req.Surface != permissionSurfaceLoop {
		return
	}
	e.Store.AddInboxItem(domain.InboxItem{
		ID:          req.ID,
		Group:       string(domain.GroupNeeds),
		Kind:        "pause",
		Title:       "Permission needed: <b>" + permissionToolLabel(req.Tool) + "</b>",
		Sub:         req.Summary,
		Action:      "Answer",
		ActionClass: "urgent",
		Unread:      true,
		Target:      "workspace:" + req.WorkspaceID,
		WorkspaceID: req.WorkspaceID,
	})
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	e.Bus.Publish(Event{Name: "notify", Data: map[string]any{
		"title":  "Permission needed",
		"body":   req.Summary,
		"target": "workspace:" + req.WorkspaceID,
	}})
	e.publishResource()
}

// publishPermissionResolved tells the UI a request left the queue, whether the
// human answered it or it expired with the turn.
func (e *Engine) publishPermissionResolved(id string, d domain.PermissionDecision) {
	e.Store.RemoveInboxItem(id)
	e.Bus.Publish(Event{Name: "permission.resolved", Data: map[string]any{
		"id":     id,
		"allow":  d.Allow,
		"reason": d.Reason,
	}})
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	e.publishResource()
}

// SetRunner overrides the agent runner for ALL turns, ignoring provider routing.
// Production leaves this unset (the model ref picks ClaudeRunner or CursorRunner);
// tests inject a ScriptedRunner. Must be called before StartWorkspace.
//
// Wrapped in withMirroring like every other runner so an injected runner's turns
// still land in the session mirror.
func (e *Engine) SetRunner(r AgentRunner) {
	e.runnerOverride = withMirroring(e, r)
	e.runnerOverrideSet = true
}

// SetWorktreeRoot overrides the worktrees parent dir (tests point it at a temp dir).
func (e *Engine) SetWorktreeRoot(root string) { e.worktreeRoot = root }

// SetTerminals attaches the Observatory PTY manager so chat @terminal wrap and
// MCP list/read_terminals can snapshot live shells. Nil is allowed (tests).
func (e *Engine) SetTerminals(m *term.Manager) { e.terminals = m }

// Governor exposes the resource governor (status-bar counters, caps).
func (e *Engine) Governor() *Governor { return e.gov }

// ApplyGovernorCaps updates the live loop/agent caps and drains queued loops
// into any newly freed slots. In-flight work is not preempted.
func (e *Engine) ApplyGovernorCaps(maxLoops, maxAgents int) {
	if e.gov == nil {
		return
	}
	e.gov.SetCaps(domain.EffectiveMaxLoops(maxLoops), domain.EffectiveMaxAgents(maxAgents))
	for {
		n := e.gov.LoopsRunning()
		e.drainQueued()
		if e.gov.LoopsRunning() <= n {
			break
		}
	}
	e.publishResource()
}

// MCP exposes the Recur MCP server (for the runner to register per-agent URLs,
// and for tests). It is constructed in New but not started until the first loop.
func (e *Engine) MCP() *mcp.Server { return e.mcpSrv }

// ensureMCP starts the MCP HTTP server on first use (idempotent,
// concurrency-safe). Called before an agent that needs MCP tools is spawned.
// A previous failure is retried rather than remembered.
func (e *Engine) ensureMCP() error {
	e.mcpMu.Lock()
	defer e.mcpMu.Unlock()
	if e.mcpStarted {
		return nil
	}
	if err := e.mcpSrv.Start(); err != nil {
		return err
	}
	e.mcpStarted = true
	return nil
}

// errMCPUnavailable marks a turn that could not start because the MCP server is
// down. It is classified TRANSIENT (agenterr.go) so the loop lands in "needs you"
// with a Retry that can actually succeed — ensureMCP retries the bind.
var errMCPUnavailable = errors.New("the Recur MCP server could not start, so agents have " +
	"no way to report findings, completion, or escalations")

// registerMCP starts the MCP server (if needed) and returns the MCP token +
// URL the runner puts in --mcp-config. Sequential turns for the same
// workspace+agent reuse the URL (prompt-cache friendly) after Unregister
// deactivates it; a concurrent overlapping turn mints its own token so
// allowlists cannot clobber each other.
// allowedTools is the turn's runner allowlist; Recur MCP names are extracted so
// tools/list only advertises what this agent may call (Cursor has no equivalent
// of --allowedTools for MCP). model selects the provider; whether
// permission_request is included is that provider's NeedsPermissionPromptTool
// answer (Claude Code yes, Cursor no) — not a slug check here.
//
// A failure here FAILS THE TURN — callers must not run the agent without it.
// Recur's whole structured-signal design routes through these tools (D7/D27), so
// an agent without them does not degrade, it lies: a Reviewer that cannot call
// report_finding reports nothing, the engine counts zero findings, and
// `converge` reads zero as success and opens a PR on code nobody reviewed. A
// wedged loop that says why is strictly better than a green one that is wrong.
func (e *Engine) registerMCP(workspaceID, agentCode, chatCode string, allowedTools []string, model string) (token, url string, err error) {
	provider := domain.ParseModelRef(model).Provider
	if e.settings().IsCompatibleSlug(provider) {
		// Native harness calls host tools in-process; still return a turn token
		// so unregisterMCP can clear "remember for this turn" approvals.
		return compatTurnToken(workspaceID, agentCode), "", nil
	}
	if err := e.ensureMCP(); err != nil {
		return "", "", fmt.Errorf("%w: %v", errMCPUnavailable, err)
	}
	names := mcp.NamesFromAllowedTools(allowedTools)
	if needsPermissionPromptTool(provider) {
		names = mcp.AppendPermissionRequest(names)
	}
	// Always pass a non-nil slice so an allowlist with zero Recur tools scopes
	// to nothing (empty tools/list) instead of falling back to unrestricted.
	token, url = e.mcpSrv.RegisterOwned(workspaceID, agentCode, chatCode, names)
	return token, url, nil
}

// unregisterMCP marks a turn ended so permission waits unblock and the MCP
// URL 404s until the next Register. Parked ask_user prompts intentionally
// survive — they outlive the turn and resume on answer.
func (e *Engine) unregisterMCP(token string) {
	if ws, code, ok := parseCompatTurnToken(token); ok {
		e.perms.endTurn(ws, code)
		return
	}
	if token == "" {
		return
	}
	workspaceID, agentCode, ok := e.mcpSrv.Unregister(token)
	if ok {
		e.perms.endTurn(workspaceID, agentCode)
	}
}

const compatTurnPrefix = "compat\x00"

func compatTurnToken(workspaceID, agentCode string) string {
	return compatTurnPrefix + workspaceID + "\x00" + agentCode
}

func parseCompatTurnToken(token string) (workspaceID, agentCode string, ok bool) {
	rest, ok := strings.CutPrefix(token, compatTurnPrefix)
	if !ok {
		return "", "", false
	}
	workspaceID, agentCode, ok = strings.Cut(rest, "\x00")
	return workspaceID, agentCode, ok
}

// settings reads the live settings snapshot.
func (e *Engine) settings() domain.Settings { return e.Store.Settings() }

// branchNamePrefix is the git path segment prepended to new workspace branches.
// Empty means no prefix. The stored Settings string is the source of truth —
// GitHub is never queried here (the Settings chip fills the string once).
func (e *Engine) branchNamePrefix() string {
	return gitpkg.NormalizeBranchPrefix(e.settings().EffectiveBranchNamePrefix())
}

func (e *Engine) dropMCPAgent(workspaceID, agentCode string) {
	if e == nil || e.mcpSrv == nil {
		return
	}
	e.mcpSrv.DropAgent(workspaceID, agentCode)
}

func (e *Engine) workspaceBranch(slug string) string {
	return gitpkg.PrefixedBranch(e.branchNamePrefix(), slug)
}

func (e *Engine) gitManager(repoPath string) *gitpkg.Manager {
	return gitpkg.NewManager(repoPath, e.worktreeRoot).WithBranchPrefix(e.branchNamePrefix())
}

// skillEnabledForWorkspace applies Settings.DisabledSkills plus the workspace
// project's EnabledSkills / DisabledSkills overrides.
func (e *Engine) skillEnabledForWorkspace(workspaceID, name string) bool {
	global := e.settings().DisabledSkills
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.EffectiveSkillEnabled(name, global, nil, nil)
	}
	p, ok := e.Store.Project(ws.ProjectID)
	if !ok {
		return domain.EffectiveSkillEnabled(name, global, nil, nil)
	}
	return domain.EffectiveSkillEnabled(name, global, p.DisabledSkills, p.EnabledSkills)
}

// StartWorkspace creates a loop record, allocates a worktree+branch off baseBranch
// (or the project's BaseBranch when baseBranch is empty), registers and starts
// the loop goroutine, and returns the new loop id (PRD §15.1, §11). If no
// loop-slot is free (Governor cap), the loop is persisted as queued and not
// started; a queued loop is auto-dispatched later when a slot frees (loopExited
// drain) or on Resume (D23).
//
// autonomy stamps this loop's latitude (PRD §9). Empty falls back to the global
// policy default; once started, a loop keeps its own value when the dial moves.
func (e *Engine) StartWorkspace(projectID, task, baseBranch string, autonomy domain.AutonomyLevel) (string, error) {
	rec, acquired, err := e.queueStartWorkspace(projectID, task, "", baseBranch, autonomy, "", "", nil)
	if err != nil {
		return "", err
	}
	if !acquired {
		return rec.ID, nil
	}
	// dispatch allocates the worktree, registers the goroutine, and starts it. If it
	// fails it releases the slot and surfaces the error.
	if err := e.dispatch(rec); err != nil {
		e.gov.ReleaseLoop()
		return "", err
	}
	return rec.ID, nil
}

// StartWorkspaceWith is StartWorkspace plus an explicit loop + policy stamp
// (empty falls back the same way as StartWorkspace). Tests that need a graph
// (verify, docs-loop) use this instead of forcing every StartWorkspace onto
// the walker.
func (e *Engine) StartWorkspaceWith(projectID, task, baseBranch string, autonomy domain.AutonomyLevel, loopID, policyID string) (string, error) {
	rec, acquired, err := e.queueStartWorkspace(projectID, task, "", baseBranch, autonomy, loopID, policyID, nil)
	if err != nil {
		return "", err
	}
	if !acquired {
		return rec.ID, nil
	}
	if err := e.dispatch(rec); err != nil {
		e.gov.ReleaseLoop()
		return "", err
	}
	return rec.ID, nil
}

// CreateWorkspace / BeginCreateWorkspace live in workspace_create.go.

// OpenMainWorkspace returns the project's singleton "main working copy"
// workspace, creating it on first open. Unlike CreateWorkspace it allocates NO
// git worktree and NO branch: WorktreePath is the project's real checkout, so
// the Observatory edits/commits land directly in the user's repo. It never runs
// the setup hook (the checkout is already the user's) and carries no base branch
// (the Changed view is uncommitted-vs-HEAD). Loop mode and PRs are refused on it
// (StartRunOnWorkspace / OpenWorkspacePR guards); Archive/Delete never remove the
// checkout. Lookup is by (ProjectID, IsMain) so repeated opens reuse one record.
func (e *Engine) OpenMainWorkspace(projectID string) (domain.Workspace, error) {
	proj, ok := e.Store.Project(projectID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("project %q not found", projectID)
	}
	if strings.TrimSpace(proj.RepoPath) == "" {
		return domain.Workspace{}, fmt.Errorf("project %q has no repo path", projectID)
	}

	root, err := filepath.Abs(gitpkg.ExpandHome(proj.RepoPath))
	if err != nil {
		return domain.Workspace{}, fmt.Errorf("resolve repo path: %w", err)
	}
	if st, err := os.Stat(root); err != nil || !st.IsDir() {
		return domain.Workspace{}, fmt.Errorf("repository path %q is not a directory", proj.RepoPath)
	}

	// Serialize find-or-create so two concurrent opens can't both pass the reuse
	// scan and add a duplicate main record (see openMainMu). Opens are rare and
	// user-initiated, so holding it across the branch probe below is fine.
	e.openMainMu.Lock()
	defer e.openMainMu.Unlock()

	// Live branch of the checkout when this path is a git root. Non-git folders
	// clear any stale Branch. A probe *failure* on a real repo (timeout, missing
	// git) keeps the stored Branch — distinguishable from a successful empty
	// result (detached HEAD / unborn).
	isGit := gitpkg.IsRepo(root)
	var branch string
	branchOK := false
	if isGit {
		branchCtx, cancelBranch := context.WithTimeout(context.Background(), 10*time.Second)
		var branchErr error
		branch, branchErr = gitpkg.CurrentBranch(branchCtx, root)
		cancelBranch()
		branchOK = branchErr == nil
	}

	// Reuse the existing main workspace for this project, but REPAIR it against the
	// project's CURRENT checkout first: RepoPath is editable after creation (project
	// settings replace the whole record), so a moved/re-pointed project must not
	// keep the main workspace browsing/committing to the OLD path. Refresh the cached
	// branch too, so the sidebar/crumb aren't stale until the backstop poll.
	for _, w := range e.Store.Workspaces() {
		if !w.IsMain || w.ProjectID != projectID {
			continue
		}
		wantBranch := w.Branch
		if !isGit {
			wantBranch = ""
		} else if branchOK {
			wantBranch = branch
		}
		if w.WorktreePath == root && w.Branch == wantBranch {
			return w, nil
		}
		updated, uok := e.Store.UpdateWorkspace(w.ID, func(m *domain.Workspace) {
			m.WorktreePath = root
			if !isGit {
				m.Branch = ""
			} else if branchOK {
				m.Branch = branch
			}
		})
		if !uok {
			return w, nil
		}
		e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
		return updated, nil
	}

	// A free port lets Run dev work against the checkout like any workspace.
	port, err := runconfig.FreePort()
	if err != nil {
		return domain.Workspace{}, fmt.Errorf("allocate port: %w", err)
	}

	mainBranch := ""
	if isGit && branchOK {
		mainBranch = branch
	}
	rec := domain.Workspace{
		ID:        e.newWorkspaceID(projectID, nil),
		ProjectID: projectID,
		// Type marks creation origin for OLDER builds; current code branches on IsMain.
		Type:         domain.WorkspaceMain,
		IsMain:       true,
		Group:        domain.GroupActive,
		Goal:         "Working copy",
		Short:        "Working copy",
		StatusLabel:  "Idle",
		Phase:        "editing",
		Branch:       mainBranch,
		BaseBranch:   "", // uncommitted-vs-HEAD baseline
		WorktreePath: root,
		Port:         port,
		Last:         "opened",
		ElapsedLabel: "just now",
	}
	saved, err := e.Store.AddWorkspace(rec)
	if err != nil {
		return domain.Workspace{}, err
	}
	e.publishLoopUpdated(saved.ID, saved.State, saved.Phase, saved.Iteration)
	return saved, nil
}

// StartRunOnWorkspace engages the autopilot on an EXISTING workspace — the chat
// dock's Loop mode. goal becomes the run's task;
// the run reuses the workspace's worktree/branch (dispatch's idempotent Add).
// Autonomy comes from the global policy (per-run override rides the same field
// later). Refused when a run was already engaged: Resume/Restart/Retry own
// re-runs, and two state machines must never share a worktree. queued reports
// whether the run is waiting for a loop slot rather than started.
func (e *Engine) StartRunOnWorkspace(workspaceID, goal string) (queued bool, err error) {
	loopID, policyID := "", ""
	if rec, ok := e.Store.Workspace(workspaceID); ok {
		loopID, policyID = rec.LoopID, rec.PolicyID
	}
	return e.StartRunOnWorkspaceWith(workspaceID, goal, loopID, policyID)
}

func (e *Engine) StartRunOnWorkspaceWith(workspaceID, goal, loopID, policyID string) (queued bool, err error) {
	goal = strings.TrimSpace(goal)
	if goal == "" {
		return false, fmt.Errorf("the loop needs a goal — describe the task")
	}
	rec, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return false, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if rec.IsMain {
		return false, fmt.Errorf("the main working copy can't run the autopilot — create a workspace instead")
	}
	if rec.IsPRChatHost() {
		return false, fmt.Errorf("create a workspace from this pull request to run the autopilot")
	}
	if rec.HasRun() {
		return false, fmt.Errorf("this workspace already has a loop — resume or restart it instead")
	}
	// Setup may fail after worktreePath is already published; the tree stays
	// so the user can recover files, but there is still nothing to dispatch.
	if rec.Phase == "failed" || rec.StatusLabel == "Couldn't create" {
		return false, fmt.Errorf("this workspace never finished creating — discard it and try again")
	}
	// Empty path is only OK while BeginCreateWorkspace is still allocating.
	// A failed create (or any other path-less record) has nothing to dispatch
	// later — stamping queued would hang the run and block Delete/Remove.
	if rec.WorktreePath == "" {
		if _, inflight := e.creating.Load(workspaceID); !inflight {
			return false, fmt.Errorf("this workspace never finished creating — discard it and try again")
		}
	}
	// Worktree still allocating (BeginCreateWorkspace): stamp the run and let
	// allocateWorkspaceWorktree dispatch once setup finishes. dispatch's
	// LoadOrStore guard prevents a double-start if both paths race.

	autonomy := e.Store.Policy().Autonomy
	switch autonomy {
	case domain.AutonomyCautious, domain.AutonomyBalanced, domain.AutonomyAggressive:
	default:
		autonomy = domain.AutonomyBalanced
	}
	if strings.TrimSpace(loopID) != "" || strings.TrimSpace(policyID) != "" {
		if _, reason := e.resolveGuard(loopID, policyID, autonomy); reason != "" {
			return false, fmt.Errorf("%s", reason)
		}
	}

	updated, ok := e.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		// failCreating may have landed between the in-flight check and this
		// stamp; don't overwrite a failed create with a queued run that
		// nothing will dispatch. WorktreePath can already be set — setup
		// failure keeps the tree so the user can recover files.
		if l.Phase == "failed" || l.LastError != "" {
			return
		}
		l.RunStarted = true
		l.Group = domain.GroupActive
		if isUnnamedWorkspace(*l) {
			if t := provisionalWorkspaceTitle(goal); t != "" {
				l.Goal = t
				l.Short = truncate(t, 48)
			} else {
				l.Goal = goal
				l.Short = truncate(goal, 48)
			}
		}
		l.State = domain.StateQueued
		l.StatusLabel = "Queued"
		l.Phase = "waiting for a slot"
		l.Iteration = 0
		l.Progress = 0
		l.Autonomy = autonomy
		l.LoopID = strings.TrimSpace(loopID)
		l.PolicyID = strings.TrimSpace(policyID)
		l.Last = "queued"
		hasMA := false
		for _, c := range l.AgentCodes {
			if c == "MA" {
				hasMA = true
				break
			}
		}
		if !hasMA {
			l.AgentCodes = append(l.AgentCodes, "MA")
		}
	})
	if !ok {
		return false, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if !updated.HasRun() {
		return false, fmt.Errorf("this workspace never finished creating — discard it and try again")
	}
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	e.stampMaestroSimplifyIfUnset(updated.ID)

	if updated.WorktreePath == "" {
		return true, nil // still creating; allocateWorkspaceWorktree will dispatch
	}
	if _, inflight := e.creating.Load(workspaceID); inflight {
		return true, nil // path landed, setup_cmd still running
	}

	if !e.gov.TryAcquireLoop() {
		e.publishResource()
		return true, nil // queued; the loopExited drain starts it when a slot frees
	}
	if err := e.dispatch(updated); err != nil {
		e.gov.ReleaseLoop()
		if benignDispatchErr(err) {
			return false, nil // winner owns it, or the conductor closed
		}
		return false, err
	}
	e.publishResource()
	return false, nil
}

// firstNonEmptyStr returns a if non-empty (trimmed), else b.
func firstNonEmptyStr(a, b string) string {
	if strings.TrimSpace(a) != "" {
		return a
	}
	return b
}

// dispatch starts a real loop goroutine from a durable loop record (D23). The
// caller must already hold a loop-slot (TryAcquireLoop). It builds the loop's own
// git.Manager off the project's repo, ensures the worktree exists (creating it if
// missing), persists the now-running state, registers the goroutine in the loop
// map, and launches it under a fresh cancellable context. This single path backs
// StartWorkspace, queued-loop drain (loopExited), and Resume of a not-running loop, so
// "running" always means there is an actual goroutine.
func (e *Engine) dispatch(rec domain.Workspace) error {
	proj, ok := e.Store.Project(rec.ProjectID)
	if !ok {
		return fmt.Errorf("project %q not found", rec.ProjectID)
	}

	mgr := e.gitManager(proj.RepoPath)

	// Belt-and-suspenders against a double-dispatch (D26): reserve the loop's map
	// entry ATOMICALLY before doing any expensive side effects (worktree add). If an
	// entry already existed, a concurrent dispatch beat us to it — bail out so we
	// don't run a second goroutine on the same worktree or race two
	// `git worktree add`. The caller releases the just-acquired loop-slot on error.
	// The dispatchMu serialization (drainQueued/Resume) already prevents this race;
	// the LoadOrStore is a second, lock-independent guard so dispatch is correct even
	// if a future caller forgets the mutex.
	e.stampMaestroSimplifyIfUnset(rec.ID)
	// CloseWorkspaceChat can clear LoopChatCode after a snapshot (drain/Resume/
	// StartRun). Trust the live row. A named conductor that is now empty is a
	// close — do not register an invisible walk.
	want := strings.TrimSpace(rec.LoopChatCode)
	if live, ok := e.Store.Workspace(rec.ID); ok {
		rec.LoopChatCode = live.LoopChatCode
	}
	lp := newLoop(e, rec)
	if lp.missingGuard == "" {
		// Stamp the resolved [[loop]] / [[policy]] so the row never carries an
		// empty id again (dock + row agree; resume re-resolves the same ids).
		rec.LoopID, rec.PolicyID = lp.LoopID, lp.PolicyID
	}
	unlockLife := func() {}
	code := strings.TrimSpace(rec.LoopChatCode)
	if want != "" && code == "" {
		return errLoopOwnerGone
	}
	if code != "" {
		// Hold through LoadOrStore so close cannot miss an in-progress
		// dispatch (stopLoopByChat only sees registered loops). Every
		// explicit conductor, not only MA.
		unlock, held := e.lockChatLife(rec.ID, code)
		unlockLife = unlock
		if !held {
			unlockLife()
			return errLoopOwnerGone
		}
		if live, ok := e.Store.Workspace(rec.ID); ok {
			rec.LoopChatCode = live.LoopChatCode
			lp.ChatCode = strings.TrimSpace(rec.LoopChatCode)
		}
		if lp.ChatCode != code {
			unlockLife()
			return errLoopOwnerGone
		}
		// MA is created on first StartWorkspace; pair tabs must already exist
		// or this is a close that restored LoopChatCode after DeleteAgent.
		if code != maestroAgentCode {
			if _, ok := e.Store.Agent(rec.ID, code); !ok {
				unlockLife()
				return errLoopOwnerGone
			}
		} else {
			e.ensureMaestroAgentWithStatus(rec.ID, rec.GoalLabel(), "idle")
		}
	}
	lp.git = mgr
	lp.mapKey = e.loopMapKey(rec)
	ctx, cancel := context.WithCancel(context.Background())
	if _, loaded := e.loops.LoadOrStore(lp.mapKey, &runningLoop{loop: lp, cancel: cancel}); loaded {
		unlockLife()
		cancel()
		return errAlreadyDispatched
	}
	unlockLife()

	// From here a map entry exists; any failure must remove it (and the caller
	// releases the slot) so accounting stays balanced and a later Resume can retry.
	cleanup := func() { e.loops.Delete(lp.mapKey); cancel() }

	// Slot is held; git worktree add is next. Publish creating so the UI does
	// not keep "waiting for a slot" through allocate (drainQueued / resume).
	if rec.WorktreePath == "" {
		if updated, ok := e.Store.UpdateWorkspace(rec.ID, func(w *domain.Workspace) {
			if w.WorktreePath != "" || w.Phase == "failed" || w.LastError != "" {
				return
			}
			w.Phase = phaseCreating
			w.StatusLabel = statusCreating
			w.Last = "creating"
		}); ok {
			e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
		}
	}

	// Brand-new loops (StartWorkspace / first dispatch of a queued loop) use
	// Add so a leftover prefix/<id> is never checked out. Remove resume, an
	// existing worktree, and checkpoint re-entry Restore the recorded branch.
	base := rec.BaseBranch
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		base = "main"
	}
	ctxAdd, cancelAdd := context.WithTimeout(context.Background(), 60*time.Second)
	var (
		wt      gitpkg.Worktree
		created bool
		err     error
	)
	if rec.Archived || rec.WorktreePath != "" || lp.resume != nil {
		wt, created, err = mgr.Restore(ctxAdd, rec.ID, rec.Branch, base)
	} else {
		wt, created, err = mgr.Add(ctxAdd, rec.ID, gitpkg.WorkspaceSlug(rec.ID), base)
	}
	cancelAdd()
	if err != nil {
		cleanup()
		return fmt.Errorf("allocate worktree: %w", err)
	}

	rec.WorktreePath = wt.Path
	rec.Branch = wt.Branch
	rec.BaseBranch = base
	if created && rec.Port <= 0 {
		port, perr := runconfig.FreePort()
		if perr != nil {
			cleanup()
			_ = mgr.Remove(context.Background(), wt.Path)
			return fmt.Errorf("allocate port: %w", perr)
		}
		rec.Port = port
	}

	// A loop with a durable checkpoint is RESUMING: it re-enters the state machine at
	// the recorded phase/iteration with its agents' sessions restored, so we must NOT
	// reset it to planning here (that reset is what made Resume replay the whole loop
	// from scratch with cold agents — D55). Only a fresh loop starts at planning.
	if lp.resume != nil {
		rec.State = lp.resume.State
		rec.StatusLabel = statusLabel(lp.resume.State)
		rec.Phase = fmt.Sprintf("resuming at iteration %d", lp.resume.Iteration)
		rec.Iteration = lp.resume.Iteration
	} else {
		rec.State = domain.StatePlanning
		rec.StatusLabel = "Planning"
		rec.Phase = "starting"
	}
	rec.Group = domain.GroupActive
	// Preserve live LoopChatCode so a close during worktree add is not undone,
	// and live Issue so a link set during worktree add survives.
	// lp.ChatCode stays as published — concurrent stop/sibling checks read it.
	if updated, ok := e.Store.UpdateWorkspace(rec.ID, func(w *domain.Workspace) {
		keep, issue := w.LoopChatCode, w.Issue
		*w = rec
		w.LoopChatCode = keep
		w.Issue = issue
	}); ok {
		rec.LoopChatCode = updated.LoopChatCode
		rec.Issue = updated.Issue
	} else if _, err := e.Store.AddWorkspace(rec); err != nil {
		cleanup()
		return err
	}
	if created {
		if err := e.copyProjectFiles(ctx, proj, rec.WorktreePath); err != nil {
			_ = mgr.Remove(context.Background(), wt.Path)
			_ = mgr.DeleteBranchIfAtBase(context.Background(), wt.Branch, base)
			_, _ = e.Store.UpdateWorkspace(rec.ID, func(lp *domain.Workspace) {
				lp.WorktreePath = ""
				lp.State = domain.StateFailed
				lp.StatusLabel = "Failed"
				lp.Phase = "setup failed"
				lp.Last = "copy files failed"
				lp.LastError = err.Error()
				lp.Group = domain.GroupNeeds
			})
			cleanup()
			return fmt.Errorf("copy files: %w", err)
		}
		if err := e.runProjectHook(proj.SetupCmd, rec, proj, "setup"); err != nil {
			e.runTeardownHook(rec, proj)
			_ = mgr.Remove(context.Background(), wt.Path)
			_ = mgr.DeleteBranchIfAtBase(context.Background(), wt.Branch, base)
			_, _ = e.Store.UpdateWorkspace(rec.ID, func(lp *domain.Workspace) {
				lp.WorktreePath = ""
				lp.State = domain.StateFailed
				lp.StatusLabel = "Failed"
				lp.Phase = "setup failed"
				lp.Last = "setup command failed"
				lp.LastError = err.Error()
				lp.Group = domain.GroupNeeds
			})
			cleanup()
			return fmt.Errorf("setup command failed: %w", err)
		}
	}
	// Reflect the resolved worktree/branch on the live loop the goroutine will run.
	lp.Worktree = wt.Path
	lp.Branch = wt.Branch

	e.publishLoopUpdated(rec.ID, rec.State, rec.Phase, rec.Iteration)
	e.publishResource()

	e.loopWG.Add(1)
	go lp.Run(ctx)
	return nil
}

// dispatchAdditionalChatRun starts a second walk on a workspace that already
// has a live goroutine. It shares the worktree but does not reset the sibling
// run's persisted state or resume its checkpoint.
func (e *Engine) dispatchAdditionalChatRun(rec domain.Workspace) error {
	code := strings.TrimSpace(rec.LoopChatCode)
	if code == "" {
		return fmt.Errorf("additional chat run needs a loop chat code")
	}
	unlock, held := e.lockChatLife(rec.ID, code)
	defer unlock()
	if !held {
		return errLoopOwnerGone
	}
	if _, ok := e.Store.Agent(rec.ID, code); !ok {
		return errLoopOwnerGone
	}
	if rec.WorktreePath == "" {
		return fmt.Errorf("workspace %q has no worktree", rec.ID)
	}
	proj, ok := e.Store.Project(rec.ProjectID)
	if !ok {
		return fmt.Errorf("project %q not found", rec.ProjectID)
	}
	rec.Iteration = 0
	lp := newLoop(e, rec)
	lp.git = e.gitManager(proj.RepoPath)
	lp.Worktree = rec.WorktreePath
	lp.Branch = rec.Branch
	lp.ChatCode = code
	lp.mapKey = rec.ID + loopMapSep + code
	lp.freshWalk = true
	lp.resume = nil
	ctx, cancel := context.WithCancel(context.Background())
	if _, loaded := e.loops.LoadOrStore(lp.mapKey, &runningLoop{loop: lp, cancel: cancel}); loaded {
		cancel()
		return errAlreadyDispatched
	}
	e.loopWG.Add(1)
	go lp.Run(ctx)
	return nil
}

// startAdditionalChatWalk starts this chat's walk while a sibling keeps running.
func (e *Engine) startAdditionalChatWalk(rec domain.Workspace) string {
	if strings.TrimSpace(rec.LoopChatCode) == "" {
		return e.dispatchIdleWalk(rec.ID)
	}
	if !e.gov.TryAcquireLoop() {
		return "Refused: every loop slot is busy."
	}
	if err := e.dispatchAdditionalChatRun(rec); err != nil {
		e.gov.ReleaseLoop()
		return "Refused: " + err.Error()
	}
	e.publishResource()
	e.emitWorkspaceEvent(rec.ID, "MA", "Maestro", "Restarting the loop over the current worktree.")
	return "Loop restarting — it continues from the current worktree, steered by the recorded plan."
}

// Pause delivers a pause to a running loop (PRD §6.2). On a NOT-running loop it is
// a no-op: there is no goroutine to pause, and fabricating a paused state for a
// queued/terminal loop would be dishonest (D23). Serialized with leaveStateOnShutdown
// via teardownMu so a concurrent quit either sees the accepted pause or persistPause
// records it after shutdown has already snapshotted.
func (e *Engine) Pause(id string) {
	var running []*runningLoop
	e.forEachRunningLoop(id, func(rl *runningLoop) {
		running = append(running, rl)
	})
	e.teardownMu.Lock()
	for _, rl := range running {
		rl.loop.send(Command{Kind: CmdPause})
	}
	if e.shuttingDown {
		e.persistPause(id)
	}
	e.teardownMu.Unlock()
}

// Resume continues a loop (PRD §6.2). If the loop is running it forwards CmdResume.
// If it is NOT running but is resumable (queued, or paused/interrupted), it
// re-dispatches a real goroutine from the durable record — acquiring a loop-slot
// first; if none is free the loop is left queued so the drain on loopExited picks
// it up later (D23). Resume of a terminal loop is a no-op.
func (e *Engine) Resume(id string) {
	e.resume(id, "")
}

func (e *Engine) resume(id, wantOwner string) {
	n := 0
	e.forEachRunningLoop(id, func(rl *runningLoop) {
		n++
		e.resumeLiveLoop(id, rl.loop)
	})
	if n > 0 {
		return
	}

	// Not-running re-dispatch runs under dispatchMu (D26): it serializes two
	// concurrent Resume(id) calls (so only one promotes the loop) and, because Stop's
	// not-running branch takes the same mutex, it closes the Resume-vs-Stop TOCTOU
	// (NEW-2) — we RE-READ the persisted state under the lock and bail if a Stop has
	// since made it terminal/non-resumable, so Resume can't resurrect a just-stopped
	// loop. The expensive worktree add inside dispatch happens under the mutex, which
	// is acceptable: dispatch never calls back into Resume/drainQueued, so no deadlock.
	e.dispatchMu.Lock()
	defer e.dispatchMu.Unlock()
	e.resumeNotRunningLocked(id, wantOwner)
}

// resumeNotRunningLocked re-dispatches id from the durable record. Caller holds dispatchMu.
func (e *Engine) resumeNotRunningLocked(id, wantOwner string) {
	if e.runningLoopByID(id) != nil {
		// A concurrent Resume already promoted it while we waited for the lock.
		return
	}
	rec, ok := e.Store.Workspace(id)
	if !ok || !resumable(rec) {
		return
	}
	if c := strings.TrimSpace(wantOwner); c != "" {
		rec.LoopChatCode = c
	}
	if e.promoteHook != nil {
		e.promoteHook() // test-only window widener (no-op in production)
	}
	unlockOwner := func() {}
	if want := strings.TrimSpace(wantOwner); want != "" {
		unlock, held := e.lockChatLife(id, want)
		unlockOwner = unlock
		if !held || !e.expectedOwnerLive(id, want) {
			unlockOwner()
			return
		}
	}
	if !e.gov.TryAcquireLoop() {
		// No slot: keep it queued so loopExited drains it when a slot frees.
		// Hold lockChatLife so a concurrent close cannot clear LoopChatCode and
		// leave this queued as a legacy empty-owner walk for drainQueued.
		e.setWorkspaceState(id, domain.StateQueued, "waiting for a slot")
		e.markGroup(id, domain.GroupActive)
		unlockOwner()
		return
	}
	unlockOwner()
	if err := e.dispatch(rec); err != nil {
		e.gov.ReleaseLoop()
		if !benignDispatchErr(err) {
			e.setWorkspaceState(id, domain.StateFailed, "resume failed: "+err.Error())
		}
	}
}

func (e *Engine) resumeLiveLoop(id string, lp *Loop) {
	// Stuck after a pre-fix answer: goroutine is still blocked in awaitAnswer
	// but the escalation is already persisted as answered. Re-deliver that answer
	// so the loop wakes (plain CmdResume does not clear escalated).
	if esc, has := e.Store.Escalation(id); has && esc.Status == "answered" {
		lp.mu.Lock()
		blocked := lp.escalated
		lp.mu.Unlock()
		if blocked {
			e.markGroup(id, domain.GroupActive)
			ans := restoreEscalationAnswer(esc)
			if ans == nil {
				ans = &EscalationAnswer{Choice: esc.Answer, Resolved: esc.Answer}
			}
			lp.send(Command{Kind: CmdAnswerEscalation, Answer: *ans})
			if rec, found := e.Store.Workspace(id); found && rec.State == domain.StateEscalated {
				e.setWorkspaceState(id, domain.StateCoding, "resuming")
			}
			e.Bus.Publish(Event{Name: "escalation.resolved", Data: map[string]any{"workspaceId": id, "answer": esc.Answer}})
			return
		}
	}
	lp.send(Command{Kind: CmdResume})
}

// expectedOwnerLive reports whether want still owns the workspace. Caller must
// hold lockChatLife(id, want). Empty want is a legacy walk (no conductor).
func (e *Engine) expectedOwnerLive(id, want string) bool {
	want = strings.TrimSpace(want)
	if want == "" {
		return true
	}
	live, ok := e.Store.Workspace(id)
	if !ok || strings.TrimSpace(live.LoopChatCode) != want {
		return false
	}
	if want == maestroAgentCode {
		return true
	}
	_, ok = e.Store.Agent(id, want)
	return ok
}

// resumable reports whether a not-running loop can be (re-)started by Resume: it is
// queued, it was paused (including an interrupted-on-restart loop, D23), it is
// escalated (an escalation answered after a restart re-dispatches the loop — M4), or
// it failed on a TRANSIENT provider error and still carries its checkpoint (D56). A
// loop that failed for any other reason stays terminal.
func resumable(rec domain.Workspace) bool {
	switch rec.State {
	case domain.StateQueued, domain.StatePaused, domain.StateEscalated:
		return true
	case domain.StateFailed:
		return rec.Retryable
	default:
		return false
	}
}

// Stop cancels a running loop's context, which kills its agents (PRD §6.1). On a
// NOT-running loop it transitions to stopped ONLY if the persisted state is
// non-terminal — a late Stop must not downgrade a loop that already finished
// (pr_open/merged/failed/stopped) (D-stop / fix #7).
func (e *Engine) Stop(id string) {
	var running []*runningLoop
	e.forEachRunningLoop(id, func(rl *runningLoop) {
		running = append(running, rl)
	})
	if len(running) > 0 {
		// Hold teardownMu so this send is atomic with leaveStateOnShutdown's
		// snapshot: either Stop sets l.stopped first (finishStopped records it)
		// or finishStopped already preserved and we persist Stop here.
		e.teardownMu.Lock()
		for _, rl := range running {
			rl.loop.send(Command{Kind: CmdStop})
			rl.cancel()
		}
		if e.shuttingDown {
			e.persistStop(id)
		}
		e.teardownMu.Unlock()
		return
	}
	// Not-running stop takes dispatchMu (D26) so it is serialized with Resume's
	// re-dispatch: a Stop that marks the loop stopped here will be observed by a
	// racing Resume's re-read (and vice versa), closing the resurrect-after-stop
	// TOCTOU (NEW-2). Re-check membership under the lock in case a Resume promoted
	// it to running while we waited.
	e.dispatchMu.Lock()
	defer e.dispatchMu.Unlock()
	var again []*runningLoop
	e.forEachRunningLoop(id, func(rl *runningLoop) {
		again = append(again, rl)
	})
	if len(again) > 0 {
		e.teardownMu.Lock()
		for _, rl := range again {
			rl.loop.send(Command{Kind: CmdStop})
			rl.cancel()
		}
		if e.shuttingDown {
			e.persistStop(id)
		}
		e.teardownMu.Unlock()
		return
	}
	e.persistStop(id)
}

// persistStop records StateStopped if the workspace is still non-terminal.
func (e *Engine) persistStop(id string) {
	if rec, found := e.Store.Workspace(id); found && !terminal(rec.State) {
		e.setWorkspaceState(id, domain.StateStopped, "stopped")
		// Recovered unanswered questions have no goroutine to unwind from
		// awaitAnswer, so the live retireWalkEscalation path never runs.
		e.retireOpenEscalations(id)
	}
}

// persistPause records StatePaused when Shutdown already won the snapshot and
// the workspace is still in-flight. Escalated/queued/terminal loops are left
// alone — Pause is not a Stop, and Recover must keep an unanswered question.
func (e *Engine) persistPause(id string) {
	if rec, found := e.Store.Workspace(id); found && interrupted(rec.State) {
		e.setWorkspaceState(id, domain.StatePaused, "paused")
	}
}

// terminal reports whether a loop state is a settled outcome that a late control
// command must not overwrite.
func terminal(s domain.WorkspaceState) bool {
	switch s {
	case domain.StatePROpen, domain.StateMerged, domain.StateFailed, domain.StateStopped:
		return true
	default:
		return false
	}
}

// Shutdown cancels every running loop (app quit), then JOINS their goroutines with
// a bounded deadline so we don't return while a claude process group is still being
// reaped (D24). If the deadline passes it returns anyway rather than hang quit.
// The 5s join is shorter than waitAlloc (30s) and teardown_cmd (10min), so quit
// can abandon an in-flight DeleteWorkspace disk teardown; leftover worktrees are
// cleaned on the next create for that kitchen id.
func (e *Engine) Shutdown() {
	e.teardownMu.Lock()
	e.shuttingDown = true
	e.teardownMu.Unlock()
	e.scheduleMu.Lock()
	if e.scheduleStop != nil {
		select {
		case <-e.scheduleStop:
		default:
			close(e.scheduleStop)
		}
	}
	e.scheduleMu.Unlock()
	if e.scheduleDone != nil {
		<-e.scheduleDone
	}
	e.WatchWorkspace("") // stop the worktree watcher
	e.creating.Range(func(_, v any) bool {
		v.(*creatingAlloc).cancel()
		return true
	})
	e.loops.Range(func(_, v any) bool {
		v.(*runningLoop).cancel()
		return true
	})
	var turnsEnded []<-chan struct{}
	e.chats.Range(func(_, v any) bool {
		if turn, ok := v.(*chatTurn); ok && turn != nil && turn.cancel != nil {
			turn.cancel()
			if turn.ended != nil {
				turnsEnded = append(turnsEnded, turn.ended)
			}
		}
		return true
	})
	done := make(chan struct{})
	go func() {
		e.scheduleWG.Wait()
		e.loopWG.Wait()
		e.teardownWG.Wait()
		for _, ended := range turnsEnded {
			<-ended
		}
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
	}
	// Close the MCP server (if started) so its listener/goroutine are released on
	// quit. Safe if never started.
	if e.mcpSrv != nil {
		_ = e.mcpSrv.Close()
	}
	// Stop the Cursor bridge subprocess (if one was ever started) so quitting
	// Recur does not leave a stray Node process behind.
	e.cursor.Close()
	// Close (not Stop): drop any assertion and refuse late Set(true) from
	// still-unwinding agent/chat goroutines so quit cannot respawn caffeinate.
	// -w on the child also covers crash/force-quit paths that skip Shutdown.
	e.keepAwake.Close()
}

// DeleteProject stops any running loops in the project, then cascades the delete in
// the store (the project + its loops/agents/feed/findings/escalations/PRs + inbox
// rows). The frontend refreshes its lists afterward. Policy/settings are untouched.
func (e *Engine) DeleteProject(id string) error {
	for _, l := range e.Store.Workspaces() {
		if l.ProjectID == id {
			e.Stop(l.ID) // cancels the goroutine if running; harmless otherwise
		}
	}
	if _, err := e.Store.DeleteProject(id); err != nil {
		return err
	}
	e.publishResource()
	return nil
}

// checkLoopSettled returns an error unless the loop id has no live goroutine AND
// has reached one of the resting states eligible for Archive/Delete (Phase 3.4):
// merged, stopped, failed, pr_open, or a completed pause (idle/artifact). Anything
// else (planning/coding/reviewing/verifying/escalated/queued, or a user pause)
// means there is still live or resumable work, so the caller must stop the loop first.
func (e *Engine) checkLoopSettled(id string, l domain.Workspace) error {
	if _, running := e.loops.Load(id); running {
		return fmt.Errorf("workspace %q is running — stop it first", id)
	}
	if e.workspaceHasLiveChat(id) {
		return fmt.Errorf("workspace %q is running — stop it first", id)
	}
	// A workspace whose autopilot was never engaged has no state machine — with
	// no running goroutine (checked above) it is always safe to archive/discard.
	if !l.HasRun() {
		return nil
	}
	switch l.State {
	case domain.StateMerged, domain.StateStopped, domain.StateFailed, domain.StatePROpen:
		return nil
	case domain.StatePaused:
		if scheduledPauseSettled(l.Phase) {
			return nil
		}
		return fmt.Errorf("workspace %q is running — stop it first", id)
	default:
		return fmt.Errorf("workspace %q is running — stop it first", id)
	}
}

// removeLoopWorktree tears down a loop's worktree on disk. Best-effort: a failure
// is noted on the loop's feed but never blocks the Archive/Delete it backs — a
// worktree that can't be removed (e.g. the repo moved) shouldn't wedge lifecycle
// management, just leave stray disk usage for the user to clean up by hand.
func (e *Engine) removeLoopWorktree(l domain.Workspace) {
	if l.WorktreePath == "" {
		return
	}
	// The main working copy's "worktree" is the user's real checkout — never
	// remove it (belt-and-suspenders; callers also skip this for IsMain).
	// PR chat hosts use the same checkout until they are promoted.
	if l.IsMain || l.IsPRChatHost() {
		return
	}
	if proj, ok := e.Store.Project(l.ProjectID); ok {
		wt := filepath.Clean(strings.TrimSpace(l.WorktreePath))
		repo := filepath.Clean(strings.TrimSpace(proj.RepoPath))
		if wt != "" && repo != "" && wt == repo {
			return
		}
	}
	// Drop any warm Cursor bridge bound to this path before the directory goes
	// away — otherwise the next ensureRunner may briefly reuse a dead cwd.
	e.cursor.invalidate(l.WorktreePath)
	if err := cursor.RemoveCursorProjectDirs(l.WorktreePath); err != nil {
		if _, ok := e.Store.Workspace(l.ID); ok {
			e.emitWorkspaceEvent(l.ID, "MA", "Maestro", "Could not remove Cursor project dirs: "+html.EscapeString(err.Error()))
		} else {
			log.Printf("recur: removing Cursor project dirs for deleted workspace %s: %v", l.ID, err)
		}
	}
	proj, ok := e.Store.Project(l.ProjectID)
	if !ok || proj.RepoPath == "" {
		return
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := mgr.Remove(ctx, l.WorktreePath); err != nil {
		if _, ok := e.Store.Workspace(l.ID); ok {
			e.emitWorkspaceEvent(l.ID, "MA", "Maestro", "Could not remove the worktree: "+html.EscapeString(err.Error()))
		} else {
			log.Printf("recur: removing worktree %s for deleted workspace %s: %v", l.WorktreePath, l.ID, err)
		}
	}
}

// removeDeletedWorkspaceDisk runs teardown_cmd and removes the worktree + data
// dir for a workspace whose store record is already gone. Path is taken from
// the pre-delete snapshot, falling back to worktrees/<id> if allocate landed
// a tree after the snapshot was taken.
func (e *Engine) removeDeletedWorkspaceDisk(l domain.Workspace) {
	if !l.IsMain && !l.IsPRChatHost() {
		path := l.WorktreePath
		if path == "" && e.worktreeRoot != "" && l.ID != "" {
			path = filepath.Join(e.worktreeRoot, l.ID)
		}
		if path != "" {
			l.WorktreePath = path
			if st, err := os.Stat(path); err == nil && st.IsDir() {
				if proj, ok := e.Store.Project(l.ProjectID); ok {
					e.runTeardownHook(l, proj)
				}
			}
			e.removeLoopWorktree(l)
		}
		// Recycle a still-at-base recur/* branch (failed setup, unused
		// checkout). Branches with unique commits are left alone, except a
		// child workspace's, which is always deleted (see
		// shouldForceDeleteBranchOnRemove). Remove does not use this path —
		// it keeps the branch unless the project has DeleteBranchOnRemove.
		if l.Branch != "" {
			if proj, ok := e.Store.Project(l.ProjectID); ok && proj.RepoPath != "" {
				base := l.BaseBranch
				if base == "" {
					base = proj.BaseBranch
				}
				mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
				bctx, bcancel := context.WithTimeout(context.Background(), 30*time.Second)
				if strings.TrimSpace(l.ParentID) != "" && l.Branch != base {
					_ = mgr.DeleteLocalBranch(bctx, l.Branch)
				} else {
					_ = mgr.DeleteBranchIfAtBase(bctx, l.Branch, base)
				}
				bcancel()
			}
		}
	}
	if proj, ok := e.Store.Project(l.ProjectID); ok {
		e.pruneCheckpoints(proj.RepoPath, l.ID)
	}
	if root, err := store.DataDir(); err == nil {
		_ = os.RemoveAll(filepath.Join(root, "workspaces", l.ID))
	}
}

// RemoveWorkspace hides a settled workspace from the default lists while keeping its
// full history (feed, PRs, agents) — only the worktree is torn down. The store
// is marked archived and the RPC returns before disk
// teardown so the UI can leave immediately; teardown_cmd, worktree removal, and
// optional DeleteBranchOnRemove run in the background (same pattern as DeleteWorkspace).
func (e *Engine) RemoveWorkspace(id string) (domain.Workspace, error) {
	l, ok := e.Store.Workspace(id)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("loop %q not found", id)
	}
	if l.IsMain || l.IsPRChatHost() {
		return domain.Workspace{}, fmt.Errorf("the main working copy can't be removed — it tracks the project's real checkout")
	}
	if err := e.checkLoopSettled(id, l); err != nil {
		return domain.Workspace{}, err
	}
	// Reserve before waiting on in-flight create. That allocate can hoist leftover
	// PR chats onto this id after publishing its worktree; waiting first then
	// reserving would archive those chats with the destination. reserveDeleting
	// also refuses if a hoist already covers this id.
	if !e.reserveDeleting(id) {
		return domain.Workspace{}, fmt.Errorf("workspace %q is being removed", id)
	}
	// Snapshot before clearing WorktreePath — background teardown needs the path.
	// Wait for any in-flight create to exit first so allocate cannot resurrect
	// WorktreePath/Port on the archived row (and so the snapshot sees a real path).
	alloc := e.cancelCreating(id)
	waitAlloc(alloc)
	if refreshed, ok := e.Store.Workspace(id); ok {
		l = refreshed
	}
	// A queued run may have dispatched while we waited — refuse if no longer settled.
	if err := e.checkLoopSettled(id, l); err != nil {
		e.deleting.Delete(id)
		return domain.Workspace{}, err
	}
	// Tombstone already held so a send that is still in MCP/model setup cannot
	// register a chat turn against a worktree that is about to vanish. Wait for
	// in-flight lockChatLife prologues so a remount that already passed its
	// generation check is either refused as live or observes deleting before
	// mutating.
	if e.workspaceHasLiveChat(id) {
		e.deleting.Delete(id)
		return domain.Workspace{}, fmt.Errorf("workspace %q is running — stop it first", id)
	}
	e.waitChatLifeIdle(id)
	if refreshed, ok := e.Store.Workspace(id); ok {
		l = refreshed
	}
	if err := e.checkLoopSettled(id, l); err != nil {
		e.deleting.Delete(id)
		return domain.Workspace{}, err
	}
	e.closeChatChildOnRemove(l)
	snapshot := l
	e.clearAskUsersForWorkspace(id)
	e.clearModeContinuesForWorkspace(id)
	e.dropChatLifeForWorkspace(id)
	e.clearInboxForWorkspace(id)
	updated, ok := e.Store.UpdateWorkspace(id, func(lp *domain.Workspace) {
		lp.Archived = true
		lp.WorktreePath = ""
		lp.Port = 0
	})
	if !ok {
		e.deleting.Delete(id)
		return domain.Workspace{}, fmt.Errorf("loop %q not found", id)
	}
	// Stop painting the worktree into Observatory before files start vanishing.
	if e.WatchedWorkspace() == id {
		e.WatchWorkspace("")
	}
	e.publishLoopUpdated(id, updated.State, updated.Phase, updated.Iteration)
	// Archived workspaces drop out of AttentionCounts — push a rollup so the
	// dock/project badges do not stay stuck on unreachable Unseen tabs.
	e.publishAttention(id, "", false)
	// Reuse the same app-layer forget path as Delete (editor/term/index) without
	// implying the store row is gone — frontend also prunes client caches.
	e.Bus.Publish(Event{Name: "workspace.removed", Data: map[string]any{"workspaceId": id}})
	e.startRemovedWorkspaceTeardown(id, snapshot, nil) // create already waited
	return updated, nil
}

func (e *Engine) closeChatChildOnRemove(l domain.Workspace) {
	if !isChatChild(l) {
		return
	}
	out := ""
	if l.FanOut != nil {
		out = l.FanOut.Outcome
	}
	if out == childOutcomeMerged || out == childOutcomeClosed {
		return
	}
	e.setChatChildOutcome(l.ParentID, l.ID, childOutcomeClosed, "")
}

func (e *Engine) lockMergeChild(parentID string) func() {
	v, _ := e.mergeChildLocks.LoadOrStore(parentID, new(sync.Mutex))
	mu := v.(*sync.Mutex)
	mu.Lock()
	return mu.Unlock
}

// startRemovedWorkspaceTeardown runs remove disk teardown in the background
// unless Shutdown has begun (then inline), matching DeleteWorkspace.
func (e *Engine) startRemovedWorkspaceTeardown(id string, l domain.Workspace, alloc *creatingAlloc) {
	e.teardownMu.Lock()
	if e.shuttingDown {
		e.teardownMu.Unlock()
		waitAlloc(alloc)
		e.removeRemovedWorkspaceDisk(l)
		e.deleting.Delete(id)
		return
	}
	e.teardownWG.Add(1)
	e.teardownMu.Unlock()
	go func() {
		defer e.teardownWG.Done()
		defer e.deleting.Delete(id)
		waitAlloc(alloc)
		e.removeRemovedWorkspaceDisk(l)
		e.removeChildrenOf(id)
	}()
}

// removeChildrenOf archives a removed parent's live children (fan-out or
// subagent) so they do not linger as orphans: a running child is stopped
// first and given a bounded wait to settle.
func (e *Engine) removeChildrenOf(parentID string) {
	for _, c := range e.Store.Workspaces() {
		if c.Archived || c.ParentID != parentID || e.isShuttingDown() {
			continue
		}
		e.Stop(c.ID)
		e.chats.Range(func(k, _ any) bool {
			if key, ok := k.(string); ok && strings.HasPrefix(key, c.ID+"\x00") {
				e.CancelChat(c.ID, strings.TrimPrefix(key, c.ID+"\x00"))
			}
			return true
		})
		deadline := time.Now().Add(15 * time.Second)
		for time.Now().Before(deadline) {
			cur, ok := e.Store.Workspace(c.ID)
			if !ok || (e.checkLoopSettled(c.ID, cur) == nil && !e.workspaceHasLiveChat(c.ID)) {
				break
			}
			time.Sleep(100 * time.Millisecond)
		}
		if _, err := e.RemoveWorkspace(c.ID); err != nil {
			log.Printf("remove %s: child %s: %v", parentID, c.ID, err)
		}
	}
}

// removeRemovedWorkspaceDisk tears down an archived workspace's on-disk tree
// (and optionally its local branch). The store row is already Archived with
// WorktreePath cleared — use the pre-clear snapshot for paths.
func (e *Engine) removeRemovedWorkspaceDisk(l domain.Workspace) {
	if l.IsMain || l.IsPRChatHost() {
		return
	}
	if l.WorktreePath == "" {
		e.cleanupPartialWorktree(l)
	} else {
		// Mirror delete: only run teardown_cmd when the directory still exists.
		// Otherwise runProjectHook blanks cwd and the script runs in the app dir.
		if st, err := os.Stat(l.WorktreePath); err == nil && st.IsDir() {
			if proj, ok := e.Store.Project(l.ProjectID); ok {
				e.runTeardownHook(l, proj)
			}
		}
		e.removeLoopWorktree(l)
	}
	e.cleanupBranchOnRemove(l)
}

// cleanupBranchOnRemove recycles a still-at-base branch (failed/unused create),
// or force-deletes the local branch when the project opts into DeleteBranchOnRemove
// — unless an open (unmerged) PR still needs that branch for mergeLocal / rework.
func (e *Engine) cleanupBranchOnRemove(l domain.Workspace) {
	branch := strings.TrimSpace(l.Branch)
	if branch == "" || l.IsMain || l.IsPRChatHost() {
		return
	}
	proj, ok := e.Store.Project(l.ProjectID)
	if !ok || proj.RepoPath == "" {
		return
	}
	base := strings.TrimSpace(l.BaseBranch)
	if base == "" {
		base = strings.TrimSpace(proj.BaseBranch)
	}
	if branch == base {
		return
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	bctx, bcancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer bcancel()
	var err error
	if e.shouldForceDeleteBranchOnRemove(l) {
		err = mgr.DeleteLocalBranch(bctx, branch)
	} else {
		err = mgr.DeleteBranchIfAtBase(bctx, branch, base)
	}
	if err != nil {
		e.emitWorkspaceEvent(l.ID, "MA", "Maestro", "Could not delete branch "+html.EscapeString(branch)+": "+html.EscapeString(err.Error()))
	}
}

// shouldForceDeleteBranchOnRemove is true when the project opts in and no open
// PR still depends on the local branch (Remove keeps PR rows mergeable). A
// child workspace's branch (fan-out or subagent) is always deleted: it is
// never PR'd, its only exits are merge into the parent or an explicit close,
// and an invisible ref per subagent run would pile up otherwise.
func (e *Engine) shouldForceDeleteBranchOnRemove(l domain.Workspace) bool {
	if strings.TrimSpace(l.ParentID) == "" {
		proj, ok := e.Store.Project(l.ProjectID)
		if !ok || !proj.DeleteBranchOnRemove {
			return false
		}
	}
	if pr, ok := e.Store.PRByWorkspace(l.ID); ok && pr.State != "Merged" && pr.Group != "merged" {
		return false
	}
	return true
}

// clearInboxForWorkspace drops inbox rows that deep-link into this workspace
// so Remove does not leave badges / deep-links to a hidden archived workspace.
func (e *Engine) clearInboxForWorkspace(id string) {
	n := e.Store.ClearInboxForWorkspace(id)
	if n > 0 {
		e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	}
}

// WorkspaceDiscardRisk returns a short phrase when removing the workspace would
// throw away uncommitted or unmerged work, or (when DeleteBranchOnRemove is on)
// delete a local branch that still has unique commits. Empty means clean /
// missing worktree and no branch-delete warning. The main working copy always
// returns "" — it is never removed this way. Live children of this workspace are
// included: Remove/Delete cascade-stops them and deletes their branches.
func (e *Engine) WorkspaceDiscardRisk(id string) string {
	l, ok := e.Store.Workspace(id)
	if !ok || l.IsMain {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	parts := e.workspaceOwnDiscardRisk(ctx, l)
	running := 0
	for _, c := range e.Store.Workspaces() {
		if c.Archived || c.ParentID != id {
			continue
		}
		if e.runningLoopByID(c.ID) != nil || e.workspaceHasLiveChat(c.ID) || e.chatResuming(c.ID) || c.ScheduledPromptPending {
			running++
		}
		parts = append(parts, e.workspaceOwnDiscardRisk(ctx, c)...)
	}
	if running > 0 {
		parts = append([]string{fmt.Sprintf("%d running child workspace(s) will be stopped", running)}, parts...)
	}
	return strings.Join(parts, "; ")
}

func (e *Engine) workspaceOwnDiscardRisk(ctx context.Context, l domain.Workspace) []string {
	var parts []string
	if l.WorktreePath != "" {
		if s := gitpkg.InspectDiscardRisk(ctx, l.WorktreePath).Summary(); s != "" {
			parts = append(parts, s)
		}
	}
	if e.shouldForceDeleteBranchOnRemove(l) {
		if proj, ok := e.Store.Project(l.ProjectID); ok && proj.RepoPath != "" {
			branch := strings.TrimSpace(l.Branch)
			base := strings.TrimSpace(l.BaseBranch)
			if base == "" {
				base = strings.TrimSpace(proj.BaseBranch)
			}
			if branch != "" && branch != base {
				mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
				if mgr.CommitExists(ctx, branch) {
					parts = append(parts, "branch will be deleted")
				}
			}
		}
	}
	return parts
}

// DeleteWorkspace permanently removes a settled loop and its entire cascade — PRs,
// agents, feed, findings, events, escalation, inbox rows, and its on-disk
// workspaces/<workspaceID>/ — plus the worktree (same not-running/settled precondition as
// RemoveWorkspace). The store record is dropped and workspace.deleted / inbox.updated
// fire before disk teardown so the UI can leave immediately; worktree removal,
// teardown_cmd, and the data dir run in the background.
func (e *Engine) DeleteWorkspace(id string) error {
	l, ok := e.Store.Workspace(id)
	if !ok {
		return fmt.Errorf("loop %q not found", id)
	}
	if err := e.checkLoopSettled(id, l); err != nil {
		return err
	}
	// Reserve the kitchen id until background teardown finishes so a recycled
	// create cannot land on a path that is still being removed. Refuses if a
	// hoist covers this id so chats that just moved here are not deleted.
	if !e.reserveDeleting(id) {
		return fmt.Errorf("workspace %q is being removed", id)
	}
	e.waitChatLifeIdle(id)
	if refreshed, ok := e.Store.Workspace(id); ok {
		l = refreshed
	}
	if err := e.checkLoopSettled(id, l); err != nil {
		e.deleting.Delete(id)
		return err
	}
	alloc := e.cancelCreating(id)
	e.clearAskUsersForWorkspace(id)
	e.clearModeContinuesForWorkspace(id)
	e.dropChatLifeForWorkspace(id)
	if err := e.Store.DeleteWorkspace(id); err != nil {
		if _, still := e.Store.Workspace(id); still {
			e.deleting.Delete(id)
		}
		return err
	}
	if e.mcpSrv != nil {
		e.mcpSrv.DropWorkspace(id)
	}
	// Stop painting the worktree into Observatory before files start vanishing.
	// After the store delete so a failed DeleteWorkspace does not leave a live
	// workspace unwatched / unfocused.
	if e.WatchedWorkspace() == id {
		e.WatchWorkspace("")
	}
	e.Bus.Publish(Event{Name: "workspace.deleted", Data: map[string]any{"workspaceId": id}})
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	e.publishResource()
	e.startDeletedWorkspaceTeardown(id, l, alloc)
	return nil
}

// startDeletedWorkspaceTeardown runs disk teardown in the background unless
// Shutdown has begun, in which case it runs inline so we never Add to
// teardownWG concurrent with Wait.
func (e *Engine) startDeletedWorkspaceTeardown(id string, l domain.Workspace, alloc *creatingAlloc) {
	e.teardownMu.Lock()
	if e.shuttingDown {
		e.teardownMu.Unlock()
		waitAlloc(alloc)
		e.removeDeletedWorkspaceDisk(l)
		e.removeChildrenOf(id)
		e.deleting.Delete(id)
		return
	}
	e.teardownWG.Add(1)
	e.teardownMu.Unlock()
	go func() {
		defer e.teardownWG.Done()
		defer e.deleting.Delete(id)
		waitAlloc(alloc)
		e.removeDeletedWorkspaceDisk(l)
		e.removeChildrenOf(id)
	}()
}

// ClearAll stops every running loop and wipes all data (keeping policy/settings),
// returning the app to its clean first-run state — "clear the data".
func (e *Engine) ClearAll() error {
	e.loops.Range(func(_, v any) bool {
		v.(*runningLoop).cancel()
		return true
	})
	e.chats.Range(func(_, v any) bool {
		if turn, ok := v.(*chatTurn); ok && turn != nil && turn.cancel != nil {
			turn.cancel()
		}
		return true
	})
	if err := e.Store.ClearAll(); err != nil {
		return err
	}
	e.chatLifeMu.Lock()
	e.chatLife = map[string]*lifeLock{}
	e.chatLifeMu.Unlock()
	e.continueGenMu.Lock()
	e.liveContinues = map[uint64]continueID{}
	e.continueGenMu.Unlock()
	e.publishResource()
	return nil
}

// loopExited is called by a loop goroutine on return: release its slot, drop it
// from the registry, and PROMOTE the oldest queued loop into the freed slot so
// queued work actually drains (D23 / fix #4).
func (e *Engine) loopExited(l *Loop) {
	key := l.mapKey
	if key == "" {
		key = l.ID
	}
	e.loops.Delete(key)
	e.gov.ReleaseLoop()
	e.publishResource()
	l.emitSettledWrapUp()
	e.maybeFlagLoopFinished(l)
	e.drainQueued()
}

// drainQueued promotes the oldest persisted queued loop (if any) into a free loop
// slot by re-dispatching it. Called after a slot is released (loopExited). A
// closed conductor is skipped so the rest of the queue can still drain; otherwise
// at most one loop is promoted per call — each promoted loop will itself drain
// the next on its own exit.
//
// The whole select-target → check-not-running → acquire-slot → register critical
// section runs under dispatchMu (D26) so two concurrent loopExited drains can never
// both pick and dispatch the same queued loop. dispatch only LAUNCHES a goroutine
// (it never calls back into drainQueued/loopExited synchronously), so holding
// dispatchMu across it can't deadlock.
func (e *Engine) drainQueued() {
	e.dispatchMu.Lock()
	defer e.dispatchMu.Unlock()

	seen := map[string]struct{}{}
	for {
		var next *domain.Workspace
		for _, l := range e.Store.Workspaces() {
			if l.State != domain.StateQueued {
				continue
			}
			if _, running := e.loops.Load(l.ID); running {
				continue
			}
			if _, already := seen[l.ID]; already {
				continue
			}
			lc := l
			next = &lc
			break // Loops() is stored oldest-first (append order) → oldest queued wins.
		}
		if next == nil {
			return
		}
		seen[next.ID] = struct{}{}
		if e.promoteHook != nil {
			e.promoteHook() // test-only window widener (no-op in production)
		}
		if !e.gov.TryAcquireLoop() {
			return // No slot actually free (raced with another start); try again later.
		}
		if err := e.dispatch(*next); err != nil {
			e.gov.ReleaseLoop()
			if errors.Is(err, errAlreadyDispatched) {
				return // Benign: the winner owns the loop; leave it alone.
			}
			if errors.Is(err, errLoopOwnerGone) {
				if rec, ok := e.Store.Workspace(next.ID); ok && rec.State == domain.StateQueued {
					e.setWorkspaceState(next.ID, domain.StatePaused, "idle")
				}
				continue
			}
			e.setWorkspaceState(next.ID, domain.StateFailed, "drain failed: "+err.Error())
		}
		return
	}
}

// Recover reconciles work that was persisted live when the app last exited
// (crash / force-quit / quit that reaped the process before defers ran):
//
//   - Loops in a running state have no goroutine but the UI would show them
//     "live" (PRD §6.5). Each is marked paused/interrupted and moved to the
//     "needs you" bucket so Resume / AutoResume can pick them up (D23).
//   - Chat agents left working/directing/planning with no live turn are idled
//     and their streaming messages closed. PA*/MA tabs are queued for
//     AutoResume so closing Recur continues the conversation instead of
//     wedging the composer (busy forever, Stop is a no-op, follow-ups queue).
//
// It also heals a stuck post-answer escalation: state=escalated with status=answered
// (or no open escalation) and no live goroutine — the human answered before the
// delivery/resume path ran, so leave an honest paused resting state Resume can pick up.
//
// Call once at startup, after the store + engine are constructed and before the UI runs.
func (e *Engine) Recover() {
	for _, l := range e.Store.Workspaces() {
		if interrupted(l.State) || (l.ScheduledPromptPending && l.State == domain.StateQueued) {
			updated, ok := e.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) {
				// Preserve WHERE it was before overwriting State with the resting value —
				// that phase is the resume position (D55). setWorkspaceState checkpoints every
				// in-flight transition, so this is normally already set; it is re-derived
				// here so a loop persisted by an older build (no checkpoint) still resumes
				// from its phase rather than from planning.
				if lp.Checkpoint == nil {
					lp.Checkpoint = &domain.Checkpoint{
						State: lp.State, Iteration: lp.Iteration, At: time.Now().Format(time.RFC3339),
					}
				}
				lp.State = domain.StatePaused
				lp.Phase = phaseInterrupted
				if l.ScheduledPromptPending {
					lp.Phase = "scheduled run interrupted"
				}
				lp.Group = domain.GroupNeeds
				lp.StatusLabel = statusLabel(domain.StatePaused)
				lp.Last = "interrupted"
			})
			if !ok {
				continue
			}
			e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
			continue
		}
		if l.State != domain.StateEscalated {
			// Terminal loops left in "needs you" (e.g. ship-with-caveats blocked by
			// rail #3 after an escalation) — clear the stale badge.
			if (l.State == domain.StateStopped || l.State == domain.StateFailed) && l.Group == domain.GroupNeeds {
				updated, ok := e.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) {
					lp.Group = domain.GroupActive
					if lp.ElapsedLabel == "running" {
						lp.ElapsedLabel = string(lp.State)
					}
				})
				if ok {
					e.Store.RemoveInboxByTarget("workspace:" + l.ID)
					e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
				}
			}
			continue
		}
		if _, running := e.loops.Load(l.ID); running {
			continue
		}
		esc, hasEsc := e.Store.Escalation(l.ID)
		// Still waiting on the human — leave it alone.
		if hasEsc && esc.Status != "answered" {
			continue
		}
		updated, ok := e.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) {
			lp.State = domain.StatePaused
			lp.Phase = "answered — waiting to resume"
			lp.Group = domain.GroupActive
			lp.StatusLabel = statusLabel(domain.StatePaused)
			lp.Last = "answered"
		})
		if !ok {
			continue
		}
		e.Store.RemoveInboxByTarget("workspace:" + l.ID)
		e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	}
	e.recoverInterruptedChats()
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
}

// isShuttingDown reports whether Engine.Shutdown has started. Chat turns that
// unwind because quit cancelled them leave status live so Recover + AutoResume
// treat the exit like a loop interrupt instead of a human Stop.
func (e *Engine) isShuttingDown() bool {
	if e == nil {
		return false
	}
	e.teardownMu.Lock()
	defer e.teardownMu.Unlock()
	return e.shuttingDown
}

// phaseInterrupted is the resting phase Recover writes on a loop that was in flight
// when the app last exited. AutoResume looks for exactly this phase, so a loop the
// HUMAN paused (any other phase) is never auto-restarted (D55).
const phaseInterrupted = "interrupted"

// interrupted reports whether a persisted state implies an in-flight loop that has
// no goroutine after a restart and must be reconciled (PRD §6.5).
func interrupted(s domain.WorkspaceState) bool {
	switch s {
	case domain.StatePlanning, domain.StateCoding, domain.StateReviewing,
		domain.StateVerifying:
		return true
	default:
		return false
	}
}

// ---- store + bus write-through helpers used by the loop goroutine ----

// setWorkspaceState updates the loop's coarse state + phase, persists, and emits
// loop.updated. It also keeps the human StatusLabel and progress roughly in sync.
// Every IN-FLIGHT transition also records the durable resume checkpoint (D55), so
// this one function keeps "where the loop is" and "where a resume should re-enter"
// in lockstep — there is no second place that has to remember to checkpoint.
func (e *Engine) setWorkspaceState(id string, state domain.WorkspaceState, phase string) {
	prev, _ := e.Store.Workspace(id)
	l, ok := e.Store.UpdateWorkspace(id, func(l *domain.Workspace) {
		l.State = state
		l.Phase = phase
		l.StatusLabel = statusLabel(state)
		l.Progress = progressFor(state)
		// Real progress once structured plan steps exist.
		if p, ok := runProgress(state, l.Plan); ok {
			l.Progress = p
		}
		if interrupted(state) {
			prevID, prevResume := "", ""
			if l.Checkpoint != nil {
				prevID = l.Checkpoint.PhaseID
				prevResume = l.Checkpoint.ResumePhaseID
			}
			l.Checkpoint = &domain.Checkpoint{
				State: state, Iteration: l.Iteration, At: time.Now().Format(time.RFC3339),
				PhaseID: prevID, ResumePhaseID: prevResume,
			}
		}
		switch state {
		case domain.StateStopped, domain.StateFailed:
			l.Last = string(state)
			l.ElapsedLabel = string(state)
			// Leaving "needs you" — a terminal loop is no longer waiting on the human,
			// unless it failed retryably and the human must clear it and Retry (D56).
			// Same update as State so nobody observes failed-but-not-needs.
			if state == domain.StateFailed && l.Retryable {
				l.Group = domain.GroupNeeds
			} else if l.Group == domain.GroupNeeds {
				l.Group = domain.GroupActive
			}
			// A stopped loop is done by the human's choice — drop its resume position. A
			// FAILED loop keeps its checkpoint: if the failure was transient, Retry
			// re-enters there (D56). failRetryable sets LastError/Retryable.
			if state == domain.StateStopped {
				l.Checkpoint = nil
			}
		case domain.StatePROpen, domain.StateMerged:
			l.ElapsedLabel = string(state)
			l.Checkpoint = nil
		case domain.StatePaused, domain.StateEscalated:
			l.ElapsedLabel = "paused"
		case domain.StateQueued:
			l.ElapsedLabel = "queued"
		default:
			l.ElapsedLabel = "running"
		}
	})
	if !ok {
		return
	}
	e.publishLoopUpdated(id, l.State, l.Phase, l.Iteration)
	if prev.State != l.State || prev.Phase != l.Phase {
		sum := strings.TrimSpace(l.Last)
		if sum == "" {
			sum = string(l.State)
		}
		e.touchWorkspaceActivity(id, sum, e.now())
	}
	// Terminal / resting transitions change NeedsYou — refresh the status bar.
	switch state {
	case domain.StateStopped, domain.StateFailed, domain.StatePaused,
		domain.StateEscalated, domain.StatePROpen, domain.StateMerged:
		e.publishResource()
	}
}

// markGroup moves a loop between list buckets (needs/active/ready) and emits.
func (e *Engine) markGroup(id string, g domain.Group) {
	l, ok := e.Store.UpdateWorkspace(id, func(l *domain.Workspace) { l.Group = g })
	if !ok {
		return
	}
	e.publishLoopUpdated(id, l.State, l.Phase, l.Iteration)
}

// updateLoopDiff refreshes the workspace's sidebar DiffStat from git after a
// commit. Falls back to accumulating the commit's numstat when the worktree
// cannot be measured (tests without a real base, etc.).
func (e *Engine) updateLoopDiff(id string, add, del int) {
	if e.syncWorkspaceDiff(id, true) {
		return
	}
	l, ok := e.Store.UpdateWorkspace(id, func(ws *domain.Workspace) {
		ws.Diff.Add += add
		ws.Diff.Del += del
	})
	if !ok {
		return
	}
	e.publishLoopUpdated(id, l.State, l.Phase, l.Iteration)
}

// emitWorkspaceEvent appends a feed item, persists it durably (append-only events
// table, PRD §6.5 / D13), and emits loop.event.
func (e *Engine) emitWorkspaceEvent(workspaceID, code, name, textHTML string) {
	e.emitWorkspaceEventItem(workspaceID, domain.FeedItem{
		ID:        fmt.Sprintf("ev-%d", time.Now().UnixNano()),
		AgentCode: code,
		Name:      name,
		Text:      textHTML,
		Time:      "now",
	})
}

// emitWorkspaceEventItem prepends a fully-formed feed item (used for tool rows that
// carry ToolUseID so a later tool_result can patch them).
func (e *Engine) emitWorkspaceEventItem(workspaceID string, item domain.FeedItem) {
	if item.ID == "" {
		item.ID = fmt.Sprintf("ev-%d", time.Now().UnixNano())
	}
	if item.Time == "" {
		item.Time = "now"
	}
	// Stamp the typed kind at the source so the UI never has to regex-classify
	// rendered HTML. A caller that already set Kind (a note or a
	// deviation) is honored; otherwise a ToolUseID marks a tool row and everything
	// else an engine-authored outcome (the common emitWorkspaceEvent milestone path).
	if item.Kind == "" {
		switch {
		case item.IsDeviation:
			item.Kind = domain.FeedKindDeviation
		case item.ToolUseID != "":
			item.Kind = domain.FeedKindTool
		default:
			item.Kind = domain.FeedKindOutcome
		}
	}
	if item.ClickAgent == "" && item.AgentCode != "" && item.AgentCode != "MA" {
		item.ClickAgent = item.AgentCode
	}
	e.stampFeedChatCode(workspaceID, &item)
	e.Store.PrependFeed(workspaceID, item)

	payload := map[string]any{"workspaceId": workspaceID, "event": item}
	e.persistEvent(workspaceID, "", "workspace.event", payload)
	e.Bus.Publish(Event{Name: "workspace.event", Data: payload})
}

// emitWorkspaceCard publishes a typed cockpit card (FeedItem.Card + Data) under a
// stable ID: the first emit prepends, later emits replace — live subscribers and
// the reload projection both dedupe by ID, so a progressing card is one row.
func (e *Engine) emitWorkspaceCard(workspaceID string, item domain.FeedItem) {
	if item.ID == "" || item.Card == "" {
		return
	}
	if item.Time == "" {
		item.Time = "now"
	}
	if item.Kind == "" {
		item.Kind = domain.FeedKindOutcome
	}
	e.stampFeedChatCode(workspaceID, &item)
	e.Store.UpsertFeed(workspaceID, item)
	payload := map[string]any{"workspaceId": workspaceID, "event": item}
	e.persistEvent(workspaceID, "", "workspace.event", payload)
	e.Bus.Publish(Event{Name: "workspace.event", Data: payload})
}

// stampFeedChatCode fills ChatCode from the unique running loop, or the
// workspace conductor when none is running — so PA2/PA3 tabs own their cards.
func (e *Engine) stampFeedChatCode(workspaceID string, item *domain.FeedItem) {
	if item == nil || item.ChatCode != "" {
		return
	}
	var owner string
	n := 0
	e.forEachRunningLoop(workspaceID, func(rl *runningLoop) {
		code := strings.TrimSpace(rl.loop.ChatCode)
		if code == "" {
			return
		}
		n++
		owner = code
	})
	switch n {
	case 1:
		item.ChatCode = owner
	case 0:
		if ws, ok := e.Store.Workspace(workspaceID); ok {
			item.ChatCode = strings.TrimSpace(ws.LoopChatCode)
		}
	}
}

// attachToolResult patches the feed row for toolUseID with the tool_result text
// and re-emits loop.event so the live UI (and ReconstructFeed dedupe) picks it up.
func (e *Engine) attachToolResult(workspaceID, toolUseID, result string, isError bool) {
	if toolUseID == "" {
		return
	}
	item := e.Store.PatchFeedByToolUseID(workspaceID, toolUseID, func(it *domain.FeedItem) {
		it.ToolResult = truncate(result, feedInputLimit)
		it.IsToolError = isError
	})
	if item == nil {
		return
	}
	payload := map[string]any{"workspaceId": workspaceID, "event": *item}
	e.persistEvent(workspaceID, "", "workspace.event", payload)
	e.Bus.Publish(Event{Name: "workspace.event", Data: payload})
}

// emitDeviation appends a DEVIATION feed item — the engine departed from a default
// (PRD §7.4: {rule, departure, why}) — persists it durably, and emits loop.event so
// the frontend's .ev.dev styling renders it specially. textHTML is the departure
// description; rule is the default departed from; why is the justification.
func (e *Engine) emitDeviation(workspaceID, textHTML, rule, why string) {
	item := domain.FeedItem{
		ID:          fmt.Sprintf("ev-%d", time.Now().UnixNano()),
		Name:        "Maestro",
		Text:        textHTML,
		Time:        "now",
		Kind:        domain.FeedKindDeviation,
		IsDeviation: true,
		Rule:        rule,
		Why:         why,
	}
	e.Store.PrependFeed(workspaceID, item)
	payload := map[string]any{"workspaceId": workspaceID, "event": item}
	e.persistEvent(workspaceID, "", "workspace.event", payload)
	e.Bus.Publish(Event{Name: "workspace.event", Data: payload})
}

// publishLoopUpdated persists + emits a loop.updated event. It carries every field
// the frontend merges into its live list (group/statusLabel/progress) so a loop that
// reaches pr_open / GroupReady doesn't stay painted as "active / working" until reload.
func (e *Engine) publishLoopUpdated(workspaceID string, state domain.WorkspaceState, phase string, iter int) {
	l, _ := e.Store.Workspace(workspaceID)
	payload := map[string]any{
		"workspaceId": workspaceID, "state": state, "phase": phase,
		"iteration": iter, "convergence": l.Convergence,
		"archived": l.Archived,
		"group":    l.Group, "statusLabel": l.StatusLabel, "progress": l.Progress,
		// goal/short so a rename_workspace call paints the sidebar/crumb live.
		"goal": l.Goal, "short": l.Short,
		// The structured plan + its prose mirror, so a submit_plan from a Plan-mode
		// chat (or the loop planner) reaches the open Plan panel live — without
		// these the store's record stays stale until a full refetch.
		"plan": l.Plan, "definitionOfDone": l.DefinitionOfDone,
		// branch so a mid-session rename/checkout paints the crumb without reload.
		"branch": l.Branch,
		// baseBranch so a post-creation retarget paints the titlebar picker live.
		"baseBranch": l.BaseBranch,
		// diff so sidebar +N/−M stays live after commits / fswatch refreshes.
		"diff": l.Diff,
		// behindBase so the titlebar Update affordance paints without a refetch.
		"behindBase": l.BehindBase,
		// worktreePath/port/lastError so eager create can paint the Observatory
		// once git+setup finish (or show the failure) without a full refetch.
		"worktreePath": l.WorktreePath, "port": l.Port, "lastError": l.LastError,
		"hostPrId":     l.HostPRID,
		"loopChatCode": l.LoopChatCode,
		"scheduleId":   l.ScheduleID,
	}
	e.persistEvent(workspaceID, "", "workspace.updated", payload)
	e.Bus.Publish(Event{Name: "workspace.updated", Data: payload})
}

// persistEvent appends a state-changing event to the durable log (D13). Cosmetic
// volume is low in M2 so we persist both loop.event and loop.updated; reload
// reconstructs the feed from the persisted loop.event rows.
func (e *Engine) persistEvent(workspaceID, agentID, typ string, payload any) {
	raw, err := json.Marshal(payload)
	if err != nil {
		return
	}
	_, _ = e.Store.AppendEvent(domain.StoredEvent{
		WorkspaceID: workspaceID,
		AgentID:     agentID,
		Type:        typ,
		PayloadJSON: string(raw),
		TS:          time.Now().UnixMilli(),
	})
}

// ReconstructFeed rebuilds a loop's activity feed for the reload path. It is now a
// projection over the session store + events table — see feedproject.go.

// ---- agent record helpers ----

func (e *Engine) upsertAgent(workspaceID string, a domain.Agent) {
	if a.Code == maestroAgentCode {
		// Update only — creating MA is ensureMaestroAgent (open/reopen/dispatch).
		// UpdateAgent is a no-op if CloseWorkspaceChat already deleted the row,
		// so a planner upsert cannot resurrect a closed tab (PA owner or empty).
		wrote := false
		e.Store.UpdateAgent(workspaceID, a.Code, func(ag *domain.Agent) {
			*ag = a
			wrote = true
		})
		if !wrote {
			return
		}
	} else {
		e.Store.UpsertAgent(workspaceID, a)
	}
	e.finishAgentWrite(workspaceID, a)
}

func (e *Engine) putAgent(workspaceID string, a domain.Agent) {
	e.Store.UpsertAgent(workspaceID, a)
	e.finishAgentWrite(workspaceID, a)
}

func (e *Engine) finishAgentWrite(workspaceID string, a domain.Agent) {
	e.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		for _, c := range l.AgentCodes {
			if c == a.Code {
				return
			}
		}
		l.AgentCodes = append(l.AgentCodes, a.Code)
	})
	e.Bus.Publish(Event{Name: "agent.updated", Data: map[string]any{
		"workspaceId": workspaceID, "agentId": a.ID, "status": a.Status,
	}})
}

func (e *Engine) updateAgentStatus(workspaceID, code, status string) {
	becameUnseen := false
	if status == "idle" && isChatAttentionCode(code) {
		e.chatUnseenMu.Lock()
		defer e.chatUnseenMu.Unlock()
	}
	e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) {
		prev := a.Status
		a.Status = status
		// Chat tabs only: a turn that was live (working/directing) and just went
		// idle means there's a response the human may not have looked at yet —
		// unless that tab is really visible (selected + app focused + no UI
		// occlusion; see isChatFocused / D-attention).
		// Specialists (CO/RV/…) are never chat tabs — leave Unseen alone.
		if status == "idle" && (prev == "working" || prev == "directing") && isChatAttentionCode(code) {
			if e.pendingModeContinueFor(workspaceID, code) {
				a.Unseen = false
			} else if e.isChatFocused(workspaceID, code) {
				a.Unseen = false
			} else {
				a.Unseen = true
				becameUnseen = true
			}
		}
	})
	e.Bus.Publish(Event{Name: "agent.updated", Data: map[string]any{
		"workspaceId": workspaceID, "agentId": workspaceID + "-" + code, "status": status,
	}})
	if becameUnseen {
		if e.emitChatInbox(workspaceID, code, false) {
			e.publishInboxUpdated()
		}
		e.publishAttention(workspaceID, code, true)
	} else if status == "idle" && isChatAttentionCode(code) && e.isChatFocused(workspaceID, code) {
		e.clearChatInbox(workspaceID, code, true)
	}
}

// isChatAttentionCode reports whether an agent code is a human-facing chat tab
// that participates in unseen-attention badges (PA/PA2/… and Maestro MA).
func isChatAttentionCode(code string) bool {
	return code == maestroAgentCode || isPairChatCode(code)
}

func (e *Engine) isChatFocused(workspaceID, code string) bool {
	e.chatFocusMu.Lock()
	defer e.chatFocusMu.Unlock()
	return e.chatSurfaceAttentiveLocked() &&
		e.chatFocus.workspaceID == workspaceID && e.chatFocus.code == code
}

// chatSurfaceAttentiveLocked reports whether the human can actually see the
// focused chat (app key + no occluding UI). Caller must hold chatFocusMu.
func (e *Engine) chatSurfaceAttentiveLocked() bool {
	return e.appFocused && !e.uiOccludesChat
}

// SetAppFocused records whether the Recur window is the OS key/focused window.
// When focus returns and a chat tab is selected with no UI occlusion, that tab
// is marked seen (the response is on screen again).
func (e *Engine) SetAppFocused(focused bool) {
	e.chatFocusMu.Lock()
	was := e.appFocused
	e.appFocused = focused
	ws, code := e.chatFocus.workspaceID, e.chatFocus.code
	markApp := focused && !was && e.chatSurfaceAttentiveLocked() && isAppChatCode(code) && ws == ""
	markSeen := focused && !was && e.chatSurfaceAttentiveLocked() && ws != "" && code != ""
	e.chatFocusMu.Unlock()
	if markApp {
		e.MarkAppChatSeen(code)
	} else if markSeen {
		e.MarkWorkspaceChatSeen(ws, code)
	}
	// File-index correctness backstop: after backgrounding, FSEvents may have
	// dropped updates — ask listeners to fully reindex the watched workspace.
	// Do not call WatchedWorkspace here: WatchWorkspace holds watchMu across
	// SetWorkspaceChatFocus (chatFocusMu), so nesting the locks can deadlock.
	if focused && !was {
		e.Bus.Publish(Event{Name: "app.focused", Data: map[string]any{}})
	}
}

// SetChatUIOccluded records whether a modal/dialog covers the chat surface
// (New workspace, command palettes, permission prompts, …). When occlusion
// clears and the app is focused with a selected tab, that tab is marked seen.
func (e *Engine) SetChatUIOccluded(occluded bool) {
	e.chatFocusMu.Lock()
	was := e.uiOccludesChat
	e.uiOccludesChat = occluded
	ws, code := e.chatFocus.workspaceID, e.chatFocus.code
	markApp := !occluded && was && e.chatSurfaceAttentiveLocked() && isAppChatCode(code) && ws == ""
	markSeen := !occluded && was && e.chatSurfaceAttentiveLocked() && ws != "" && code != ""
	e.chatFocusMu.Unlock()
	if markApp {
		e.MarkAppChatSeen(code)
	} else if markSeen {
		e.MarkWorkspaceChatSeen(ws, code)
	}
}

// SetWorkspaceChatFocus records which chat tab the human is viewing and clears
// its unseen badge when the surface is really visible. Empty code clears focus
// for that workspace only when it matches the current focus workspace.
func (e *Engine) SetWorkspaceChatFocus(workspaceID, code string) {
	code = strings.TrimSpace(code)
	e.chatFocusMu.Lock()
	if workspaceID == "" || code == "" {
		if workspaceID == "" || e.chatFocus.workspaceID == workspaceID {
			e.chatFocus.workspaceID = ""
			e.chatFocus.code = ""
		}
		e.chatFocusMu.Unlock()
		return
	}
	e.chatFocus.workspaceID = workspaceID
	e.chatFocus.code = code
	attentive := e.chatSurfaceAttentiveLocked()
	e.chatFocusMu.Unlock()
	if attentive {
		e.MarkWorkspaceChatSeen(workspaceID, code)
	}
}

// MarkWorkspaceChatSeen clears the unseen flag for a chat tab the human just
// viewed. No-op when already seen or the agent is missing.
func (e *Engine) MarkWorkspaceChatSeen(workspaceID, code string) {
	code = strings.TrimSpace(code)
	if code == "" {
		code = pairAgentCode
	}
	if !isChatAttentionCode(code) {
		return
	}
	e.chatUnseenMu.Lock()
	defer e.chatUnseenMu.Unlock()
	changed := false
	e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) {
		if !a.Unseen {
			return
		}
		a.Unseen = false
		changed = true
	})
	// Always drop the inbox row: Unseen may already be clear (desync / closed
	// tab / emit-then-focus race) while the row is still sitting in Inbox.
	removed := e.clearChatInbox(workspaceID, code, true)
	if changed || removed {
		e.publishAttention(workspaceID, code, false)
	}
}

// AttentionCounts returns the current unseen + inbox rollup for badges/dock.
func (e *Engine) AttentionCounts() domain.AttentionCounts {
	return e.Store.AttentionCounts()
}

// publishAttention emits attention.updated with full rollup counts plus the
// tab that changed (so the open workspace can patch its chip without refetch).
func (e *Engine) publishAttention(workspaceID, code string, unseen bool) {
	e.publishAttentionEvt(workspaceID, code, unseen, false)
}

// publishAttentionEvt is publishAttention plus an optional clearAllUnseen
// flag so Mark all read can patch every cached chat chip in one event.
func (e *Engine) publishAttentionEvt(workspaceID, code string, unseen, clearAll bool) {
	c := e.Store.AttentionCounts()
	data := map[string]any{
		"unseen":      c.Unseen,
		"inboxUnread": c.InboxUnread,
		"dock":        c.Dock,
		"byWorkspace": c.ByWorkspace,
		"byProject":   c.ByProject,
		"appUnseen":   c.AppUnseen,
		"workspaceId": workspaceID,
		"code":        code,
		"tabUnseen":   unseen,
	}
	if clearAll {
		data["clearAllUnseen"] = true
	}
	e.Bus.Publish(Event{Name: "attention.updated", Data: data})
}

// setAgentSession records the agent's claude session id (for later inspection /
// resume in M7). Stored on the agent's SessionID field.
func (e *Engine) setAgentSession(workspaceID, code, sessionID string) {
	if sessionID == "" {
		return
	}
	e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) { a.SessionID = sessionID })
}

func (e *Engine) clearAgentSession(workspaceID, code string) {
	e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) { a.SessionID = "" })
}

func (e *Engine) appendAgentStep(workspaceID, code string, step domain.AgentStep) {
	e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) {
		a.Steps = append(a.Steps, step)
	})
	e.Bus.Publish(Event{Name: "agent.step", Data: map[string]any{
		"workspaceId": workspaceID, "agentId": workspaceID + "-" + code, "step": step,
	}})
}

// publishResource recomputes the status-bar rollup from the governor + store and
// emits resource.updated (PRD §6.4, §15.2).
func (e *Engine) publishResource() {
	r := e.Store.Resource()
	r.LoopsRunning = e.gov.LoopsRunning()
	r.NeedsYou = e.Store.CountGroup(domain.GroupNeeds)
	// "iters today" = the seeded/persisted baseline + real review iterations completed
	// since boot (machine-global via the governor, D6/§6.4).
	r.ItersToday = e.itersBase + e.gov.Iters()
	warn := e.gitWarning
	r.GitWarning = ""
	e.Store.SetResource(r)
	r.GitWarning = warn
	e.Bus.Publish(Event{Name: "resource.updated", Data: r})
}

// ---- id helpers ----

func (e *Engine) newWorkspaceID(projectID string, mgr *gitpkg.Manager) string {
	e.idMu.Lock()
	defer e.idMu.Unlock()
	return e.mintWorkspaceID(projectID, mgr)
}

// mintWorkspaceID picks a free `{project}.{kitchen}` id. Caller must hold idMu.
func (e *Engine) mintWorkspaceID(projectID string, mgr *gitpkg.Manager) string {
	repoPath := ""
	if p, ok := e.Store.Project(projectID); ok {
		repoPath = repoKey(p.RepoPath)
	}

	takenBranch := map[string]bool{}
	prefix := e.branchNamePrefix()
	if mgr != nil {
		prefix = mgr.Prefix()
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if names, err := mgr.ListLocalBranches(ctx); err != nil {
			log.Printf("recur: listing local branches while allocating workspace id: %v", err)
		} else {
			cut := ""
			if prefix != "" {
				cut = prefix + "/"
			}
			for _, name := range names {
				if cut == "" {
					takenBranch[name] = true
					continue
				}
				if slug, ok := strings.CutPrefix(name, cut); ok {
					takenBranch[slug] = true
				}
			}
		}
	}
	e.markSameRepoKitchenTaken(repoPath, prefix, takenBranch)

	kitchen := allocKitchenID(func(slug string) bool {
		id := composeWorkspaceID(projectID, slug)
		if _, ok := e.Store.Workspace(id); ok {
			return true
		}
		if _, busy := e.deleting.Load(id); busy {
			return true
		}
		return takenBranch[slug]
	})
	return composeWorkspaceID(projectID, kitchen)
}

// repoKey canonicalizes a project RepoPath for same-repo kitchen-slug claims.
// ExpandHome + Clean is not enough: macOS /tmp → /private/tmp, and a symlink
// alias of the same checkout must compare equal. EvalSymlinks fails when the
// path is missing, so we fall back to Abs/Clean.
func repoKey(p string) string {
	p = filepath.Clean(gitpkg.ExpandHome(strings.TrimSpace(p)))
	if p == "" || p == "." {
		return p
	}
	abs, err := filepath.Abs(p)
	if err != nil {
		return p
	}
	if resolved, err := filepath.EvalSymlinks(abs); err == nil {
		return resolved
	}
	return abs
}

// markSameRepoKitchenTaken records kitchen slugs already claimed by a worktree
// workspace on the same git checkout. Main working copies do not use
// prefix/<slug> and are skipped.
func (e *Engine) markSameRepoKitchenTaken(repoPath, prefix string, taken map[string]bool) {
	if repoPath == "" || repoPath == "." {
		return
	}
	cut := ""
	if prefix != "" {
		cut = prefix + "/"
	}
	for _, ws := range e.Store.Workspaces() {
		if ws.IsMain || ws.IsPRChatHost() {
			continue
		}
		p, ok := e.Store.Project(ws.ProjectID)
		if !ok || repoKey(p.RepoPath) != repoPath {
			continue
		}
		if slug := gitpkg.WorkspaceSlug(ws.ID); slug != "" {
			taken[slug] = true
		}
		if prefix == "" {
			if b := strings.TrimSpace(ws.Branch); b != "" {
				taken[b] = true
			}
		} else if slug, ok := strings.CutPrefix(ws.Branch, cut); ok && slug != "" {
			taken[slug] = true
		}
	}
}

func statusLabel(s domain.WorkspaceState) string {
	switch s {
	case domain.StateQueued:
		return "Queued"
	case domain.StatePlanning:
		return "Planning"
	case domain.StateCoding:
		return "Coding"
	case domain.StateReviewing:
		return "Reviewing"
	case domain.StateVerifying:
		return "Verifying"
	case domain.StatePROpen:
		return "Ready"
	case domain.StatePaused:
		return "Paused"
	case domain.StateStopped:
		return "Stopped"
	case domain.StateFailed:
		return "Failed"
	default:
		return string(s)
	}
}

func progressFor(s domain.WorkspaceState) float64 {
	switch s {
	case domain.StatePlanning:
		return 0.15
	case domain.StateCoding:
		return 0.5
	case domain.StateReviewing:
		return 0.7
	case domain.StateVerifying:
		return 0.85
	case domain.StatePROpen:
		return 1
	default:
		return 0
	}
}
