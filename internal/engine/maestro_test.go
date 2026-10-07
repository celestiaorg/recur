package engine

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
)

// maestroFixture returns a canonical Maestro chat transcript: init (carries the
// session id) → assistant text (the reply) → success result (carries session id +
// cost). The same parser the production runner uses parses these lines.
func maestroFixture(sessionID, reply string) [][]byte {
	return [][]byte{
		[]byte(`{"type":"system","subtype":"init","session_id":"` + sessionID + `","model":"claude-opus-4-6","mcp_servers":[]}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"text","text":"` + reply + `"}]}}`),
		[]byte(`{"type":"result","subtype":"success","session_id":"` + sessionID + `","duration_ms":900,"is_error":false,"num_turns":1,"result":"` + reply + `","total_cost_usd":0.05}`),
	}
}

// captureRunner is a ScriptedRunner-style test double that records every AgentSpec it
// was Run with (so a test can assert which session id a turn resumed) and replays a
// per-call sequence of canned lines. It is concurrency-safe (the chat turn and any
// loop goroutine may both run agents).
type captureRunner struct {
	mu    sync.Mutex
	specs []AgentSpec
	// linesFor returns the stream-json lines to replay for a given call index; if nil,
	// a benign success is returned.
	linesFor func(call int) [][]byte
}

func (r *captureRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	call := len(r.specs)
	r.specs = append(r.specs, spec)
	r.mu.Unlock()

	var lines [][]byte
	if r.linesFor != nil {
		lines = r.linesFor(call)
	}
	sr := &ScriptedRunner{Lines: lines}
	return sr.Run(ctx, spec, emit)
}

func (r *captureRunner) specAt(i int) (AgentSpec, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if i < 0 || i >= len(r.specs) {
		return AgentSpec{}, false
	}
	return r.specs[i], true
}

func (r *captureRunner) specWithRole(role domain.AgentRole) (AgentSpec, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, s := range r.specs {
		if s.Role == role {
			return s, true
		}
	}
	return AgentSpec{}, false
}

func (r *captureRunner) specsWithRole(role domain.AgentRole) []AgentSpec {
	r.mu.Lock()
	defer r.mu.Unlock()
	var out []AgentSpec
	for _, s := range r.specs {
		if s.Role == role {
			out = append(out, s)
		}
	}
	return out
}

func (r *captureRunner) specsWithAgentID(agentID string) []AgentSpec {
	r.mu.Lock()
	defer r.mu.Unlock()
	var out []AgentSpec
	for _, s := range r.specs {
		if s.AgentID == agentID && s.Role != "" {
			out = append(out, s)
		}
	}
	return out
}

// startedLoop persists a minimal running-ish loop record directly in the store and
// returns its id, so a Maestro-chat test can target a loop WITHOUT running its
// goroutine (the chat is an independent turn — it doesn't need the loop running).
func startedLoop(t *testing.T, eng *Engine, projID string) string {
	t.Helper()
	id := eng.newWorkspaceID(projID, nil)
	rec := domain.Workspace{
		ID: id, ProjectID: projID, Goal: "Add OAuth provider config",
		State: domain.StateCoding, Phase: "iteration 2", Iteration: 2,
		Group: domain.GroupActive, Autonomy: domain.AutonomyBalanced,
	}
	if _, err := eng.Store.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	return id
}

// TestMaestroChatReplyAndResume is the M7-A test: SendMaestroMessage returns the
// streamed reply, and a SECOND message RESUMES the same session (the captured
// session id from turn 1 is passed as --resume on turn 2). No real claude.
func TestMaestroChatReplyAndResume(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			switch call {
			case 0:
				return maestroFixture("maestro-sess-1", "Code and review are done — converged to 0 findings.")
			default:
				return maestroFixture("maestro-sess-1", "Google is implemented and covered by passing tests.")
			}
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	// Turn 1: first message starts a session; the reply is returned.
	reply1 := eng.SendMaestroMessage(workspaceID, "where are we?", nil)
	if !strings.Contains(reply1, "converged to 0 findings") {
		t.Errorf("turn 1 reply: got %q, want it to contain the scripted reply", reply1)
	}
	// The Maestro turn must NOT have resumed anything (no prior session).
	s0, _ := runner.specAt(0)
	if s0.ResumeSession != "" {
		t.Errorf("turn 1 should not resume; got ResumeSession=%q", s0.ResumeSession)
	}
	if s0.Role != domain.RoleMaestro {
		t.Errorf("turn 1 role: got %q, want maestro", s0.Role)
	}
	// Maestro hands: READ access (Read/Grep + read-only git
	// Bash), but NEVER write tools — its write path is engine-mediated MCP actions.
	has := func(tool string) bool {
		for _, tl := range s0.AllowedTools {
			if tl == tool {
				return true
			}
		}
		return false
	}
	for _, tool := range []string{"Edit", "Write", "Bash", "NotebookEdit"} {
		if has(tool) {
			t.Errorf("Maestro must not have write tool %q (its writes are engine-mediated)", tool)
		}
	}
	for _, tool := range []string{"Read", "Grep", "Bash(git diff:*)"} {
		if !has(tool) {
			t.Errorf("Maestro should have read tool %q", tool)
		}
	}
	for _, tool := range []string{
		"mcp__recur__edit_plan",
		"mcp__recur__dispatch_specialist", mcpExtendRounds, mcpReenterPhase, mcpRunPhase,
		mcpListTerminals, mcpReadTerminal,
	} {
		if !has(tool) {
			t.Errorf("Maestro AllowedTools must include %s", tool)
		}
	}
	// The session id was persisted on the Maestro agent record.
	if got := eng.maestroSession(workspaceID); got != "maestro-sess-1" {
		t.Errorf("persisted Maestro session: got %q, want maestro-sess-1", got)
	}
	// The transcript is persisted via the session store (single source of truth), so
	// the UI reloads history across launches.
	// contentItems drops the turn footer (metadata, asserted separately).
	chat1 := contentItems(eng.MaestroChatItems(workspaceID))
	if len(chat1) != 2 || chat1[0].Role != "user" || chat1[0].Text != "where are we?" || chat1[1].Role != "assistant" {
		t.Fatalf("transcript after turn 1: %+v", chat1)
	}

	// Turn 2: a second message RESUMES the captured session (multi-turn continuity).
	reply2 := eng.SendMaestroMessage(workspaceID, "what about google?", nil)
	if !strings.Contains(reply2, "Google is implemented") {
		t.Errorf("turn 2 reply: got %q", reply2)
	}
	if got := len(contentItems(eng.MaestroChatItems(workspaceID))); got != 4 {
		t.Errorf("transcript after turn 2: got %d msgs, want 4", got)
	}
	// Each turn closes with its own footer, so two turns produce two.
	var footers int
	for _, it := range eng.MaestroChatItems(workspaceID) {
		if it.Kind == "turn" {
			footers++
		}
	}
	if footers != 2 {
		t.Errorf("got %d turn footers after 2 turns, want 2", footers)
	}
	s1, ok := runner.specAt(1)
	if !ok {
		t.Fatal("expected a second Maestro turn")
	}
	if s1.ResumeSession != "maestro-sess-1" {
		t.Errorf("turn 2 must resume the session: got ResumeSession=%q, want maestro-sess-1", s1.ResumeSession)
	}
	if !s0.IncludePartials {
		t.Error("Maestro turns must request --include-partial-messages for chat streaming")
	}
}

func TestMaestroRefreshStandingWhenContextChanges(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture(fmt.Sprintf("maestro-ctx-%d", call), "ok")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	_ = eng.SendMaestroMessage(workspaceID, "where are we?", nil)
	_ = eng.SendMaestroMessage(workspaceID, "still there?", nil)
	s1, ok := runner.specAt(1)
	if !ok {
		t.Fatal("expected a second Maestro turn")
	}
	if s1.RefreshStandingInstructions {
		t.Error("unchanged loop context must not RefreshStandingInstructions")
	}

	st.UpdateWorkspace(workspaceID, func(lp *domain.Workspace) {
		lp.State = domain.StateReviewing
		lp.Iteration = 9
	})
	_ = eng.SendMaestroMessage(workspaceID, "and now?", nil)
	s2, ok := runner.specAt(2)
	if !ok {
		t.Fatal("expected a third Maestro turn")
	}
	if !s2.RefreshStandingInstructions {
		t.Error("changed Maestro context must RefreshStandingInstructions")
	}
	if !strings.Contains(s2.SystemPrompt, "iteration 9") {
		t.Errorf("SystemPrompt = %q, want live iteration 9", s2.SystemPrompt)
	}
}

func TestMaestroSystemPromptConductorNotSpecialist(t *testing.T) {
	mission := "CHARTER: do not open a PR."
	prompt := maestroSystemPrompt(MaestroContext{
		Goal: "Review & fix", Mission: mission, State: "reviewing",
	})
	for _, want := range []string{
		"do not do the loop's phase work yourself",
		"Read/Grep/Glob only to answer the human",
		"the engine walks the graph",
		"pick by whether the walk is live",
		"paused at a checkpoint",
		"escalated at the round cap",
		mission,
		"Task: Review & fix",
		mcpMessageChild,
	} {
		if !strings.Contains(prompt, want) {
			t.Errorf("maestroSystemPrompt() missing %q, got %q", want, prompt)
		}
	}
	if strings.Contains(prompt, "specialists execute this") {
		t.Errorf("maestroSystemPrompt() still tells the conductor specialists execute the mission, got %q", prompt)
	}
	if strings.Contains(prompt, "idle (stopped/failed/paused/settled)") {
		t.Errorf("maestroSystemPrompt() still treats every paused state as idle, got %q", prompt)
	}
	livePaused := maestroSystemPrompt(MaestroContext{State: "paused", Live: true})
	if !strings.Contains(livePaused, "paused (live walk)") || !strings.Contains(livePaused, "Resume continues this walk") {
		t.Errorf("live paused prompt = %q, want (live walk) + Resume", livePaused)
	}
	dup := maestroSystemPrompt(MaestroContext{Goal: mission, Mission: mission, State: "reviewing"})
	if strings.Count(dup, mission) != 1 {
		t.Errorf("identical Goal and Mission should appear once, got %q", dup)
	}
}

func TestMaestroFollowUpIncludesLoopPrompt(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("charter", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	installTestPack(t, "CharterLoop", `
[bundle]
name = "Charter"
id = "test.charter"
version = "1.0.0"

[[loop]]
id = "test.charter.loop"
name = "Charter"
conductor = "recur.roles.maestro"
prompt = "CHARTER: do not open a PR."
entry = "converge"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
`)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if tabs := eng.ListWorkspaceChats(ws.ID); len(tabs) != 1 || tabs[0].Code != "PA" {
		t.Fatalf("tabs = %+v, want [PA]", tabs)
	}
	eng.RememberChatLoopPolicy(ws.ID, "PA", "test.charter.loop", "")
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "go", "", nil, nil, "loop", "Review & fix")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "where are we?", "", nil, nil, "loop", "")
	specs := runner.specsWithRole(domain.RoleMaestro)
	if len(specs) != 1 {
		t.Fatalf("Maestro specs = %d, want 1 follow-up", len(specs))
	}
	got := specs[0].SystemPrompt
	if !strings.Contains(got, "CHARTER: do not open a PR.") {
		t.Errorf("follow-up SystemPrompt missing [[loop]] prompt, got %q", got)
	}
}

func TestMaestroFailureKeepsDisplayTextWithoutDuplicateUser(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return [][]byte{
				[]byte(`{"type":"system","subtype":"init","session_id":"m-fail","model":"m","mcp_servers":[]}`),
				[]byte(`{"type":"result","subtype":"error_during_execution","session_id":"m-fail","is_error":true,"num_turns":1,"duration_ms":12}`),
			}
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	wire := "full wire prompt about the findings list"
	_ = eng.sendMaestroMessage(workspaceID, maestroAgentCode, wire, nil, "Fix findings")
	s0, ok := runner.specWithRole(domain.RoleMaestro)
	if !ok {
		t.Fatal("no maestro turn")
	}
	if s0.DisplayText != "Fix findings" {
		t.Fatalf("DisplayText=%q, want prompt-action label", s0.DisplayText)
	}
	items := contentItems(eng.MaestroChatItems(workspaceID))
	users := 0
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" {
			users++
			if it.Text != "Fix findings" {
				t.Fatalf("user bubble=%q, want display label", it.Text)
			}
		}
	}
	if users != 1 {
		t.Fatalf("user bubbles=%d, want 1 (items=%+v)", users, items)
	}
}

// TestMaestroChatPublishesDeltas asserts text chunks are fanned out on the Bus as
// session.delta (tagged code=MA by the mirror broadcaster) while the turn runs (so the UI can stream without waiting on the
// bound-method return).
func TestMaestroChatPublishesDeltas(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte {
			return maestroFixture("maestro-sess-d", "Status: **merged** on `main`.")
		},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	var deltas []string
	eng.Bus.Subscribe(func(ev Event) {
		if ev.Name != "session.delta" {
			return
		}
		data, _ := ev.Data.(map[string]any)
		if data["workspaceId"] != workspaceID {
			return
		}
		if text, ok := data["text"].(string); ok {
			deltas = append(deltas, text)
		}
	})

	reply := eng.SendMaestroMessage(workspaceID, "where are we?", nil)
	if len(deltas) == 0 {
		t.Fatal("expected at least one session.delta during the turn")
	}
	joined := strings.Join(deltas, "")
	if !strings.Contains(joined, "merged") {
		t.Errorf("deltas=%q, want them to carry the reply text", joined)
	}
	if !strings.Contains(reply, "merged") {
		t.Errorf("final reply: got %q", reply)
	}
}

// TestMaestroChatDoesNotBlockRunningLoop asserts a Maestro chat turn runs
// CONCURRENTLY with a live loop goroutine and never wedges it (the chat is an
// independent turn; the loop keeps driving its state machine). The loop runs a
// blocking agent we release after the chat turn has completed — proving the chat did
// not depend on, or block, the loop goroutine.
func TestMaestroChatDoesNotBlockRunningLoop(t *testing.T) {
	repo := initRepo(t)

	// The loop uses a blocking runner; the chat uses a fast scripted reply. Both share
	// the same engine.runner, so route by role: the Maestro turn replays a canned
	// reply, the loop's specialist turns block until released.
	gate := make(chan struct{})
	runner := &roleRouterRunner{
		gate:    gate,
		maestro: maestroFixture("sess-conc", "We're mid-coding — iteration 2, going well."),
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	workspaceID, err := eng.StartWorkspace(projID, "long running task", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	// Wait until the loop's specialist agent is actually running (blocked in the gate),
	// so we know the loop goroutine is live and busy.
	waitFor(t, 3*time.Second, "loop agent running", func() bool {
		return eng.Governor().AgentsRunning() >= 1
	})

	// Send a chat message WHILE the loop is blocked mid-agent. It must complete on its
	// own (the chat is independent of the loop goroutine).
	done := make(chan string, 1)
	go func() { done <- eng.SendMaestroMessage(workspaceID, "status?", nil) }()

	select {
	case reply := <-done:
		if !strings.Contains(reply, "iteration 2") {
			t.Errorf("chat reply during a running loop: got %q", reply)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Maestro chat blocked while the loop was running (deadlock?)")
	}

	// Now release the loop so it finishes; the test ends cleanly.
	close(gate)
	waitFor(t, 5*time.Second, "loop to finish", func() bool {
		l, ok := st.Workspace(workspaceID)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateStopped || l.State == domain.StateFailed)
	})
}

// TestMaestroSteeringWithinRails is the M7-A steering test: when the Maestro re-plans
// via chat (a mcp__recur__plan tool call), the engine records a plan-change
// DEVIATION and queues the steer; there is NO sentence-level rail matching — rails
// hold where actions execute. The steer arrives via the SAME
// typed Reporter seam the live MCP handler uses (engineReporter.Plan), so the test
// exercises the real steering path, not a parallel mock.
func TestMaestroSteeringWithinRails(t *testing.T) {
	runner := &captureRunner{linesFor: func(int) [][]byte {
		return maestroFixture("sess-steer", "Sounds good — re-planning.")
	}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	rep := engineReporter{eng: eng}

	t.Run("within rails applies a plan-change deviation", func(t *testing.T) {
		before := len(sink.byName("workspace.event"))
		rep.Plan(workspaceID, maestroAgentCode, "Split this into two PRs and prioritize the config change first.")

		// A deviation event was emitted (a deviation is a loop.event with IsDeviation).
		var sawDeviation bool
		for _, ev := range sink.byName("workspace.event") {
			m, _ := ev.Data.(map[string]any)
			item, _ := m["event"].(domain.FeedItem)
			if item.IsDeviation && strings.Contains(item.Text, "Re-planned from your chat") {
				sawDeviation = true
			}
		}
		if !sawDeviation {
			t.Errorf("expected a plan-change deviation from the within-rails steer (events grew from %d)", before)
		}
	})

	t.Run("no keyword rejection: rails hold at execution, not at the sentence", func(t *testing.T) {
		// steerCrossesRail is gone. A "skip verify" steer is
		// recorded like any steer — the GUARD still refuses a rail-crossing outcome
		// where it executes (gateVerify / CanOpenPR), which the rails tests cover.
		rep.Plan(workspaceID, maestroAgentCode, "Skip the browser verify and open the PR now.")

		var sawDeviation, sawKeywordRejection bool
		for _, ev := range sink.byName("workspace.event") {
			m, _ := ev.Data.(map[string]any)
			item, _ := m["event"].(domain.FeedItem)
			if item.IsDeviation && strings.Contains(item.Text, "Skip the browser verify") {
				sawDeviation = true
			}
			if strings.Contains(item.Text, "Can't do that from chat") {
				sawKeywordRejection = true
			}
		}
		if !sawDeviation {
			t.Error("the steer should be recorded as a deviation (no sentence-matching gate)")
		}
		if sawKeywordRejection {
			t.Error("keyword-based chat rejection should be gone (rails hold at execution)")
		}
	})
}

// TestMaestroSteerReachesNextAgentTurn asserts D49 steering delivery:
//   - in-flight specialist receives the steer on Inject (user-message path), NOT
//     via Prompt mutation (prompt cache intact)
//   - newly spawned specialists get the steer in their initial Prompt
func TestMaestroSteerReachesNextAgentTurn(t *testing.T) {
	repo := initRepo(t)
	gate := make(chan struct{})
	runner := &steerCaptureRunner{gate: gate, gated: make(chan struct{}, 1)}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	workspaceID, err := eng.StartWorkspace(projID, "implement file watcher", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	select {
	case <-runner.gated:
	case <-time.After(5 * time.Second):
		t.Fatal("coder turn never blocked in gate")
	}

	const steer = "Build on the existing in-progress worktree changes; do not start from scratch."
	engineReporter{eng: eng}.Plan(workspaceID, maestroAgentCode, steer)

	// Release the in-flight Coder: it should drain Inject after its tool_result.
	close(gate)
	waitForState(t, st, workspaceID, "loop to finish", domain.StatePROpen, domain.StateStopped, domain.StateFailed)

	specs := runner.allSpecs()
	if len(specs) < 2 {
		t.Fatalf("expected at least coder+reviewer specs, got %d", len(specs))
	}
	// In-flight Coder Prompt must NOT be rewritten (cache-safe); steer arrives via Inject.
	if strings.Contains(specs[0].Prompt, steer) {
		t.Errorf("in-flight coder prompt must not be mutated with the steer:\n%s", specs[0].Prompt)
	}
	injected := runner.allInjected()
	if len(injected) == 0 || !strings.Contains(injected[0], steer) {
		t.Errorf("in-flight coder must receive steer on Inject after tool_result; got %v", injected)
	}
	// Newly spawned Reviewer gets the steer in its initial Prompt.
	var sawPrompt bool
	for _, s := range specs[1:] {
		if strings.Contains(s.Prompt, "Maestro steer") && strings.Contains(s.Prompt, steer) {
			sawPrompt = true
			break
		}
	}
	if !sawPrompt {
		t.Errorf("expected a newly spawned specialist Prompt to carry %q; specs=%v", steer, promptsOf(specs))
	}

	// There is no keyword-based rejection of "rail-crossing"
	// steer SENTENCES anymore — every steer travels; the rails hold where actions
	// execute (the guard refuses the outcome and names the rail). Guard behavior is
	// covered by the rails tests.
}

func promptsOf(specs []AgentSpec) []string {
	out := make([]string, len(specs))
	for i, s := range specs {
		out[i] = string(s.Role) + ":" + s.Prompt
	}
	return out
}

// steerCaptureRunner records every AgentSpec, blocks until gate, then emits a
// tool_result and drains Inject (mirroring ClaudeRunner D49 timing) so tests can
// assert in-flight steers arrive as user-message injects.
type steerCaptureRunner struct {
	mu       sync.Mutex
	specs    []AgentSpec
	injected []string
	gate     chan struct{}
	gated    chan struct{} // signalled once a specialist (Inject bound) waits on gate
}

func (r *steerCaptureRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	r.specs = append(r.specs, spec)
	r.mu.Unlock()
	if spec.Role == domain.RoleMaestro {
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	}
	if emit != nil {
		emit(ParsedEvent{Kind: KindStep, Tool: "Bash", ToolInput: "working", ToolUseID: "t1"})
	}
	select {
	case r.gated <- struct{}{}:
	default:
	}
	select {
	case <-r.gate:
	case <-ctx.Done():
		return Result{}, ctx.Err()
	}
	if emit != nil {
		emit(ParsedEvent{Kind: KindToolResult, ToolUseID: "t1", Text: "ok"})
	}
	if spec.Inject != nil {
		select {
		case msg := <-spec.Inject:
			r.mu.Lock()
			r.injected = append(r.injected, msg)
			r.mu.Unlock()
		default:
		}
	}
	return Result{Subtype: "success", SessionID: spec.AgentID}, nil
}

func (r *steerCaptureRunner) allSpecs() []AgentSpec {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]AgentSpec(nil), r.specs...)
}

func (r *steerCaptureRunner) allInjected() []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]string(nil), r.injected...)
}

// roleRouterRunner replays a canned Maestro reply for the Maestro role and BLOCKS
// (until gate or ctx) for every other role, so a test can hold a loop's specialist
// turn open while exercising a concurrent Maestro chat turn.
type roleRouterRunner struct {
	gate    chan struct{}
	maestro [][]byte
}

func (r *roleRouterRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if spec.Role == domain.RoleMaestro {
		sr := &ScriptedRunner{Lines: r.maestro}
		return sr.Run(ctx, spec, emit)
	}
	if emit != nil {
		emit(ParsedEvent{Kind: KindStep, Tool: "Bash", ToolInput: "working"})
	}
	select {
	case <-r.gate:
	case <-ctx.Done():
		return Result{}, ctx.Err()
	}
	return Result{Subtype: "success", SessionID: spec.AgentID}, nil
}
