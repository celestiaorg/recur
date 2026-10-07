package engine

import (
	"context"
	"strings"
	"testing"

	"recur/internal/domain"
	"recur/internal/store"
)

// maestroToolFixture is a maestro turn that reads a file: init → assistant
// (text + a tool_use) → user (tool_result) → success. It exercises every block
// type the S1 mirror projects.
func maestroToolFixture(sessionID string) [][]byte {
	return [][]byte{
		[]byte(`{"type":"system","subtype":"init","session_id":"` + sessionID + `","model":"claude-opus-4-6","mcp_servers":[]}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"text","text":"Let me check."},{"type":"tool_use","id":"toolu_read1","name":"Read","input":{"file_path":"auth.go"}}]}}`),
		[]byte(`{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_read1","is_error":false,"content":"package auth"}]}}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"text","text":"Auth looks fine."}]}}`),
		[]byte(`{"type":"result","subtype":"success","session_id":"` + sessionID + `","duration_ms":900,"is_error":false,"num_turns":1,"result":"Auth looks fine.","total_cost_usd":0.05}`),
	}
}

// TestWrapperMirrorsSpecialistTurn proves the mirroringRunner covers non-maestro
// roles: a coder turn routed through the engine's (wrapped) runner creates a
// kind=coder session and mirrors its transcript — no per-call-site wiring.
func TestWrapperMirrorsSpecialistTurn(t *testing.T) {
	runner := &ScriptedRunner{Lines: maestroToolFixture("coder-sess-1")}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	spec := AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "claude-sonnet-5",
		Prompt:      "implement the feature",
	}
	// runAgent is the real dispatch seam: it resolves the provider from the spec's
	// model ref and lands on the mirroringRunner wrapping the ScriptedRunner.
	// Reaching for eng.runner directly would bypass SetRunner and hit the
	// PRODUCTION ClaudeRunner — a live, billed API call from a unit test.
	if _, err := eng.runAgent(context.Background(), spec, nil); err != nil {
		t.Fatalf("run: %v", err)
	}

	sess, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	if !ok {
		t.Fatalf("no coder session created")
	}
	if sess.Kind != "coder" || sess.Provider != domain.ProviderClaudeCode {
		t.Errorf("session wrong: kind=%q provider=%q", sess.Kind, sess.Provider)
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("transcript: %v", err)
	}
	if len(msgs) == 0 || msgs[0].Role != "user" {
		t.Fatalf("transcript should open with the user prompt, got %d msgs", len(msgs))
	}
	if txt, _ := msgs[0].Content[0].AsText(); txt.Text != "implement the feature" {
		t.Errorf("prompt not mirrored: %+v", msgs[0].Content)
	}
	// A second turn for the same role reuses the same session (one coder conversation).
	if _, err := eng.runAgent(context.Background(), spec, nil); err != nil {
		t.Fatalf("second run: %v", err)
	}
	if again, _ := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder)); again.ID != sess.ID {
		t.Errorf("second coder turn should reuse session %q, got %q", sess.ID, again.ID)
	}
}

// TestMaestroTurnMirrorsToSession is the M9/S1 acceptance test: a maestro chat turn
// is mirrored losslessly into the session store — typed content blocks, the raw
// provider lines, and the rotating session ref — and the transcript reloads from a
// fresh store handle (proving it survives an app restart).
func TestMaestroTurnMirrorsToSession(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroToolFixture("maestro-sess-mirror") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	reply := eng.SendMaestroMessage(workspaceID, "is auth ok?", nil)
	if !strings.Contains(reply, "Auth looks fine") {
		t.Fatalf("reply: got %q", reply)
	}

	// A durable maestro session now exists for the loop, with the rotated ref.
	sess, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleMaestro))
	if !ok {
		t.Fatalf("no maestro session created for %q", workspaceID)
	}
	if sess.Provider != domain.ProviderClaudeCode || sess.ContextMode != domain.ContextModeProvider {
		t.Errorf("session provider/mode wrong: %+v", sess)
	}
	if sess.ProviderSessionRef != "maestro-sess-mirror" {
		t.Errorf("session ref not mirrored: got %q", sess.ProviderSessionRef)
	}

	assertTranscript := func(t *testing.T, msgs []domain.Message, label string) {
		t.Helper()
		// user prompt, assistant (text + tool_use), user (tool_result), assistant (final text)
		if len(msgs) != 4 {
			t.Fatalf("%s: got %d messages, want 4", label, len(msgs))
		}
		if msgs[0].Role != "user" || msgs[1].Role != "assistant" || msgs[2].Role != "user" || msgs[3].Role != "assistant" {
			t.Fatalf("%s: roles %s/%s/%s/%s", label, msgs[0].Role, msgs[1].Role, msgs[2].Role, msgs[3].Role)
		}
		// 1) the human prompt
		if txt, _ := msgs[0].Content[0].AsText(); txt.Text != "is auth ok?" {
			t.Errorf("%s: prompt not mirrored: %+v", label, msgs[0].Content)
		}
		// 2) the assistant turn: text + tool_use, model-stamped
		asst := msgs[1]
		if asst.ModelID == "" {
			t.Errorf("%s: assistant not model-stamped", label)
		}
		if len(asst.Content) != 2 {
			t.Fatalf("%s: assistant blocks: got %d, want 2", label, len(asst.Content))
		}
		tu, ok := asst.Content[1].AsToolUse()
		if !ok || tu.Name != "Read" || tu.ProviderToolUseID != "toolu_read1" {
			t.Fatalf("%s: tool_use block wrong: %+v ok=%v", label, tu, ok)
		}
		if tu.ID == "" || tu.ID == tu.ProviderToolUseID {
			t.Errorf("%s: tool_use should carry a distinct internal id, got %q", label, tu.ID)
		}
		if !strings.Contains(string(tu.Input), "auth.go") {
			t.Errorf("%s: tool_use input not mirrored verbatim: %s", label, tu.Input)
		}
		// 3) the tool_result, referencing the SAME internal id (dual-ID replay)
		tr, ok := msgs[2].Content[0].AsToolResult()
		if !ok || tr.Result != "package auth" {
			t.Fatalf("%s: tool_result wrong: %+v ok=%v", label, tr, ok)
		}
		if tr.ToolUseID != tu.ID {
			t.Errorf("%s: tool_result should reference the internal tool_use id %q, got %q", label, tu.ID, tr.ToolUseID)
		}
		// 4) the assistant's final answer
		if txt, _ := msgs[3].Content[0].AsText(); txt.Text != "Auth looks fine." {
			t.Errorf("%s: final answer not mirrored: %+v", label, msgs[3].Content)
		}
	}

	live, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	assertTranscript(t, live, "live")

	// Reload from a SECOND store handle on the same data dir — proves the transcript
	// is durable, not just in the live handle (RECUR_DATA_DIR is still set).
	st2, err := store.New()
	if err != nil {
		t.Fatalf("reopen store: %v", err)
	}
	defer st2.Close()
	reloaded, err := st2.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("reopen transcript: %v", err)
	}
	assertTranscript(t, reloaded, "reloaded")
}

// TestNonClaudeProviderTurnIsMirrored is the regression guard for the bug that
// made Cursor runs look broken in the UI: the Cursor runner was returned from
// runnerFor UNWRAPPED, so its turns bypassed the session mirror entirely. The
// run itself succeeded, but no session, no transcript, no tool-call blocks and
// no session.delta events were ever written — so the Activity panel showed
// neither streaming nor tool calls, and the DB held nothing for the turn.
//
// It also pins the second half of that bug: runAgent replaces the qualified ref
// with the BARE model id before the runner (and therefore the mirror) sees the
// spec, so the mirror must read spec.Provider. Re-deriving it from spec.Model
// would silently record every Cursor turn as claude-code.
//
// A ScriptedRunner stands in for CursorRunner — what is under test is the
// dispatch path, not the bridge.
func TestNonClaudeProviderTurnIsMirrored(t *testing.T) {
	runner := &ScriptedRunner{Lines: maestroToolFixture("cursor-sess-1")}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	spec := AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "cursor/composer-2.5",
		Prompt:      "implement the feature",
	}
	if _, err := eng.runAgent(context.Background(), spec, nil); err != nil {
		t.Fatalf("run: %v", err)
	}

	sess, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	if !ok {
		t.Fatal("no session created for a Cursor turn — the runner bypassed the mirror")
	}
	if sess.Provider != domain.ProviderCursor {
		t.Errorf("session provider = %q, want %q", sess.Provider, domain.ProviderCursor)
	}

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("transcript: %v", err)
	}
	if len(msgs) == 0 {
		t.Fatal("empty transcript")
	}

	// The assistant turn must carry the bare provider model id and the provider
	// that actually ran it.
	var sawAssistant, sawTool bool
	for _, m := range msgs {
		if m.Role != "assistant" {
			continue
		}
		sawAssistant = true
		if m.Provider != domain.ProviderCursor {
			t.Errorf("assistant message provider = %q, want %q", m.Provider, domain.ProviderCursor)
		}
		if m.ModelID != "composer-2.5" {
			t.Errorf("assistant model_id = %q, want the bare id composer-2.5", m.ModelID)
		}
		for _, c := range m.Content {
			if c.Type == domain.BlockToolUse {
				sawTool = true
			}
		}
	}
	if !sawAssistant {
		t.Error("no assistant message mirrored")
	}
	// Tool calls are the visible half of the symptom: without them the Activity
	// panel renders a bare reply with no steps.
	if !sawTool {
		t.Error("no tool_use block mirrored — the Activity panel would show no tool calls")
	}
}

// TestCursorTurnOverwritesClaudeSessionProvider: a session created as
// claude-code (dock default / mode-only remember) must flip to cursor when a
// Cursor turn actually runs.
func TestCursorTurnOverwritesClaudeSessionProvider(t *testing.T) {
	runner := &ScriptedRunner{Lines: maestroToolFixture("cursor-sess-2")}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	if _, ok := eng.ensureAgentSession(workspaceID, string(domain.RoleCoder), "CO", "Coder", domain.ProviderClaudeCode); !ok {
		t.Fatal("ensureAgentSession failed")
	}
	seed, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	if !ok || seed.Provider != domain.ProviderClaudeCode {
		t.Fatalf("pre-seed session.Provider = %+v ok=%v, want %q", seed, ok, domain.ProviderClaudeCode)
	}

	spec := AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "cursor/composer-2.5",
		Prompt:      "implement the feature",
	}
	if _, err := eng.runAgent(context.Background(), spec, nil); err != nil {
		t.Fatalf("runAgent: %v", err)
	}

	sess, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	if !ok {
		t.Fatal("session missing after Cursor turn")
	}
	if sess.Provider != domain.ProviderCursor {
		t.Errorf("ensureAgentSession after Cursor turn session.Provider = %q, want %q",
			sess.Provider, domain.ProviderCursor)
	}
}

// Claude stream-json can re-emit the same tool_use (same toolu_… id) on multiple
// assistant NDJSON lines — commonly three when thinking + text + tool_use are
// split. Without dedupe, each KindStep mints a tool_use row and a live
// session.step, so the chat shows the call in triplicate and only the last clone
// gets the tool_result.
func TestClaudeRepeatedToolUseYieldsOneStep(t *testing.T) {
	const toolID = "toolu_dup1"
	dupLine := []byte(`{"type":"assistant","message":{"content":[{"type":"tool_use","id":"` + toolID + `","name":"Read","input":{"file_path":"auth.go"}}]}}`)
	lines := [][]byte{
		[]byte(`{"type":"system","subtype":"init","session_id":"claude-dedupe","model":"claude-sonnet-4-6","mcp_servers":[]}`),
		dupLine, dupLine, dupLine,
		[]byte(`{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"` + toolID + `","is_error":false,"content":"ok"}]}}`),
		[]byte(`{"type":"result","subtype":"success","session_id":"claude-dedupe","duration_ms":100,"is_error":false,"num_turns":1,"result":"done","total_cost_usd":0.01}`),
	}
	runner := &ScriptedRunner{Lines: lines}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	var steps int
	eng.Bus.Subscribe(func(ev Event) {
		if ev.Name != "session.step" {
			return
		}
		data, _ := ev.Data.(map[string]any)
		if data["workspaceId"] != workspaceID {
			return
		}
		steps++
	})

	spec := AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "claude-sonnet-5",
		Prompt:      "read auth",
	}
	var emittedSteps int
	if _, err := eng.runAgent(context.Background(), spec, func(ev ParsedEvent) {
		if ev.Kind == KindStep {
			emittedSteps++
		}
	}); err != nil {
		t.Fatalf("run: %v", err)
	}

	if steps != 1 {
		t.Errorf("session.step bus events: got %d, want 1", steps)
	}
	if emittedSteps != 1 {
		t.Errorf("inner KindStep emits: got %d, want 1", emittedSteps)
	}

	sess, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	if !ok {
		t.Fatal("no coder session")
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("transcript: %v", err)
	}
	var toolUses, toolResults int
	var internalID string
	for _, m := range msgs {
		for _, c := range m.Content {
			if tu, ok := c.AsToolUse(); ok {
				toolUses++
				if tu.ProviderToolUseID != toolID {
					t.Errorf("provider id = %q, want %q", tu.ProviderToolUseID, toolID)
				}
				internalID = tu.ID
			}
			if tr, ok := c.AsToolResult(); ok {
				toolResults++
				if tr.ToolUseID != internalID {
					t.Errorf("tool_result id = %q, want internal %q", tr.ToolUseID, internalID)
				}
			}
		}
	}
	if toolUses != 1 {
		t.Errorf("persisted tool_use blocks: got %d, want 1", toolUses)
	}
	if toolResults != 1 {
		t.Errorf("persisted tool_result blocks: got %d, want 1", toolResults)
	}
}

type emitSeqRunner struct {
	events []ParsedEvent
}

func (r emitSeqRunner) Run(_ context.Context, _ AgentSpec, emit func(ParsedEvent)) (Result, error) {
	for _, ev := range r.events {
		if emit != nil {
			emit(ev)
		}
	}
	return Result{Subtype: "success"}, nil
}

func TestKindUserResetsToolIDDedupe(t *testing.T) {
	const toolID = "c1"
	runner := emitSeqRunner{events: []ParsedEvent{
		{Kind: KindStep, Tool: "read", ToolUseID: toolID, ToolInputRaw: []byte(`{"path":"a.go"}`)},
		{Kind: KindUser, Text: "Maestro steer (follow this; it supersedes earlier plan guidance):\nstop"},
		{Kind: KindStep, Tool: "read", ToolUseID: toolID, ToolInputRaw: []byte(`{"path":"b.go"}`)},
		{Kind: KindResult, Result: &Result{Subtype: "success", DurationMS: 10}},
	}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	var steps int
	eng.Bus.Subscribe(func(ev Event) {
		if ev.Name != "session.step" {
			return
		}
		data, _ := ev.Data.(map[string]any)
		if data["workspaceId"] != workspaceID {
			return
		}
		steps++
	})

	spec := AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "cursor/composer-2.5",
		Prompt:      "do the thing",
	}
	var emittedSteps int
	if _, err := eng.runAgent(context.Background(), spec, func(ev ParsedEvent) {
		if ev.Kind == KindStep {
			emittedSteps++
		}
	}); err != nil {
		t.Fatalf("run: %v", err)
	}

	if steps != 2 {
		t.Errorf("session.step bus events: got %d, want 2 (KindUser must reset live dedupe)", steps)
	}
	if emittedSteps != 2 {
		t.Errorf("inner KindStep emits: got %d, want 2", emittedSteps)
	}

	sess, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	if !ok {
		t.Fatal("no coder session")
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("transcript: %v", err)
	}
	var toolUses int
	for _, m := range msgs {
		for _, c := range m.Content {
			if _, ok := c.AsToolUse(); ok {
				toolUses++
			}
		}
	}
	if toolUses != 2 {
		t.Errorf("persisted tool_use blocks: got %d, want 2 (KindUser must reset toolIDs)", toolUses)
	}
}

// Claude/Codex live inject emits KindUser before the triggering KindToolResult.
// That must not drop the outstanding provider→internal id, or flattenTranscript
// cannot attach the result on reload.
func TestKindUserPreservesOutstandingToolIDs(t *testing.T) {
	const toolID = "toolu_steer1"
	for _, provider := range []string{domain.ProviderClaudeCode, domain.ProviderCodex} {
		t.Run(provider, func(t *testing.T) {
			_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
			sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: provider})
			if err != nil {
				t.Fatalf("CreateSession: %v", err)
			}
			m := newSessionMirror(st, sess.ID, "claude-sonnet-5", provider)
			m.Event(ParsedEvent{Kind: KindStep, Tool: "Read", ToolUseID: toolID, ToolInputRaw: []byte(`{"file_path":"a.go"}`)})
			m.Event(ParsedEvent{Kind: KindUser, Text: "also check tests"})
			m.Event(ParsedEvent{Kind: KindToolResult, ToolUseID: toolID, Text: "package a"})
			m.finish()

			msgs, err := st.SessionTranscript(sess.ID)
			if err != nil {
				t.Fatalf("transcript: %v", err)
			}
			var internal string
			var resultID string
			for _, msg := range msgs {
				for _, c := range msg.Content {
					if tu, ok := c.AsToolUse(); ok {
						internal = tu.ID
					}
					if tr, ok := c.AsToolResult(); ok {
						resultID = tr.ToolUseID
					}
				}
			}
			if internal == "" {
				t.Fatal("missing tool_use")
			}
			if resultID != internal {
				t.Errorf("tool_result id = %q, want internal %q (not provider %q)", resultID, internal, toolID)
			}
			items := flattenTranscript(msgs, "", nil)
			var attached bool
			for _, it := range items {
				if it.Kind == "tool" && it.Result == "package a" && !it.ResultError {
					attached = true
				}
			}
			if !attached {
				t.Errorf("WorkspaceChatItems did not attach tool result after KindUser inject: %+v", items)
			}
		})
	}
}

func TestKindUserLiveSteerPreservesCursorToolIDs(t *testing.T) {
	const toolID = "c1"
	_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: domain.ProviderCursor})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	m := newSessionMirror(st, sess.ID, "composer-2.5", domain.ProviderCursor)
	m.Event(ParsedEvent{Kind: KindStep, Tool: "read", ToolUseID: toolID, ToolInputRaw: []byte(`{"path":"a.go"}`)})
	m.Event(ParsedEvent{Kind: KindUser, Text: "also check tests", LiveSteer: true})
	m.Event(ParsedEvent{Kind: KindToolResult, ToolUseID: toolID, Text: `{"status":"success","value":{"content":"package a"}}`})
	m.finish()

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("transcript: %v", err)
	}
	var internal string
	var resultID string
	for _, msg := range msgs {
		for _, c := range msg.Content {
			if tu, ok := c.AsToolUse(); ok {
				internal = tu.ID
			}
			if tr, ok := c.AsToolResult(); ok {
				resultID = tr.ToolUseID
			}
		}
	}
	if internal == "" {
		t.Fatal("missing tool_use")
	}
	if resultID != internal {
		t.Errorf("tool_result id = %q, want internal %q (not provider %q)", resultID, internal, toolID)
	}
	items := flattenTranscript(msgs, "", nil)
	var attached bool
	for _, it := range items {
		if it.Kind == "tool" && it.Result == "package a" && !it.ResultError {
			attached = true
		}
	}
	if !attached {
		t.Errorf("WorkspaceChatItems did not attach tool result after live Cursor steer: %+v", items)
	}
}

// Distinct tool ids in one turn must each get their own step — dedupe is by id,
// not by tool name.
func TestClaudeParallelToolUsesEachPersist(t *testing.T) {
	lines := [][]byte{
		[]byte(`{"type":"system","subtype":"init","session_id":"claude-parallel","model":"claude-sonnet-4-6","mcp_servers":[]}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_a","name":"Read","input":{"file_path":"a.go"}},{"type":"tool_use","id":"toolu_b","name":"Read","input":{"file_path":"b.go"}}]}}`),
		[]byte(`{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_a","is_error":false,"content":"a"},{"type":"tool_result","tool_use_id":"toolu_b","is_error":false,"content":"b"}]}}`),
		[]byte(`{"type":"result","subtype":"success","session_id":"claude-parallel","duration_ms":100,"is_error":false,"num_turns":1,"result":"done","total_cost_usd":0.01}`),
	}
	runner := &ScriptedRunner{Lines: lines}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	spec := AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "claude-sonnet-5",
		Prompt:      "read both",
	}
	if _, err := eng.runAgent(context.Background(), spec, nil); err != nil {
		t.Fatalf("run: %v", err)
	}

	sess, _ := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("transcript: %v", err)
	}
	ids := map[string]bool{}
	for _, m := range msgs {
		for _, c := range m.Content {
			if tu, ok := c.AsToolUse(); ok {
				ids[tu.ProviderToolUseID] = true
			}
		}
	}
	if len(ids) != 2 || !ids["toolu_a"] || !ids["toolu_b"] {
		t.Errorf("tool ids = %v, want toolu_a and toolu_b", ids)
	}
}

func TestRequiresDisplayText(t *testing.T) {
	if !RequiresDisplayText(`<skill_content name="x">body</skill_content>`) {
		t.Fatal("skill_content should require display_text")
	}
	if !RequiresDisplayText("The user has attached these files. Read them before proceeding.") {
		t.Fatal("attached-files preamble should require display_text")
	}
	if !RequiresDisplayText("The user activated skill(s) via /name. Follow each skill's instructions below.") {
		t.Fatal("skill activation instruction should require display_text")
	}
	if RequiresDisplayText("just a normal human message") {
		t.Fatal("plain prompt must not require display_text")
	}
	if !RequiresDisplayText(forkPreambleLead + " Continue from the <transcript> below.") {
		t.Fatal("fork transcript inject should require display_text")
	}
}

func TestUserBubbleTextPrefersDisplay(t *testing.T) {
	if got := userBubbleText("WIRE long review body", "Review the current diff"); got != "Review the current diff" {
		t.Errorf("display: %q", got)
	}
	if got := userBubbleText("You are the Coder agent. A reviewer found issues in pong", ""); got != coderFixDisplay {
		t.Errorf("coder label: %q", got)
	}
	if got := userBubbleText("create a cli game of pong", ""); got != "create a cli game of pong" {
		t.Errorf("bare prompt: %q", got)
	}
}

// TestRecordUserDisplayTextInvariant: recording wrap-marker wire without
// DisplayText violates RequiresDisplayText; the happy path stores Prompt as
// text + DisplayText and flatten collapses to the label.
func TestRecordUserDisplayTextInvariant(t *testing.T) {
	wire := "<system_instruction>\nThe user activated skill(s) via /name.\n</system_instruction>\n\n" +
		`<skill_content name="demo">body</skill_content>` + "\n\nplease review"
	emptyDisplay := ""
	if !RequiresDisplayText(wire) || emptyDisplay != "" {
		t.Fatal("wraps + empty DisplayText must fail the invariant")
	}

	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{Lines: maestroToolFixture("disp-1")})
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)
	display := "/demo please review"
	spec := AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "claude-sonnet-5",
		Prompt:      wire,
		DisplayText: display,
	}
	if _, err := eng.runAgent(context.Background(), spec, nil); err != nil {
		t.Fatalf("run: %v", err)
	}
	sess, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	if !ok {
		t.Fatal("no session")
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil || len(msgs) == 0 {
		t.Fatalf("transcript: %v len=%d", err, len(msgs))
	}
	p, ok := msgs[0].Content[0].AsText()
	if !ok {
		t.Fatalf("want text block, got %+v", msgs[0].Content[0])
	}
	if p.Text != wire {
		t.Fatalf("store text should be full wire, got %q", p.Text)
	}
	if p.DisplayText != display {
		t.Fatalf("display_text=%q, want %q", p.DisplayText, display)
	}
	it, ok := flattenUserMessage(msgs[0])
	if !ok || it.Text != display {
		t.Fatalf("flatten=%q ok=%v, want display %q", it.Text, ok, display)
	}
}

// TestMirrorFallbackDedupAfterToolResult: a Fable→Opus hop must persist once
// even when sticky usage.iterations re-emits KindFallback on a later assistant
// message after a tool_result (which closes the first assistant row).
func TestMirrorFallbackDedupAfterToolResult(t *testing.T) {
	_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	m := newSessionMirror(st, sess.ID, "claude-fable-5", domain.ProviderClaudeCode)
	m.Event(ParsedEvent{Kind: KindFallback, FromModel: "claude-fable-5", ToModel: "claude-opus-4-8"})
	m.Event(ParsedEvent{Kind: KindText, Text: "checking"})
	m.Event(ParsedEvent{Kind: KindStep, Tool: "Bash", ToolUseID: "t1", ToolInputRaw: []byte(`{}`)})
	m.Event(ParsedEvent{Kind: KindToolResult, ToolUseID: "t1", Text: "ok"})
	m.Event(ParsedEvent{Kind: KindFallback, FromModel: "claude-fable-5", ToModel: "claude-opus-4-8"})
	m.Event(ParsedEvent{Kind: KindText, Text: "done"})
	m.finish()

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	n := 0
	for _, msg := range msgs {
		for _, b := range msg.Content {
			if p, ok := b.AsFallback(); ok {
				n++
				if p.From != "claude-fable-5" || p.To != "claude-opus-4-8" {
					t.Errorf("fallback payload = %+v, want fable→opus", p)
				}
			}
		}
	}
	if n != 1 {
		t.Errorf("fallback blocks = %d, want 1 across the turn", n)
	}
}

func TestMirrorFallbackStampsServingModel(t *testing.T) {
	_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	m := newSessionMirror(st, sess.ID, "claude-fable-5", domain.ProviderClaudeCode)
	m.Event(ParsedEvent{Kind: KindFallback, FromModel: "claude-fable-5", ToModel: "claude-opus-4-8"})
	m.Event(ParsedEvent{Kind: KindText, Text: "served by Opus"})
	m.finish()

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	var asst *domain.Message
	for i := range msgs {
		if msgs[i].Role == "assistant" {
			asst = &msgs[i]
			break
		}
	}
	if asst == nil {
		t.Fatal("SessionTranscript() missing assistant message")
	}
	if asst.ModelID != "claude-opus-4-8" {
		t.Errorf("assistant ModelID = %q, want claude-opus-4-8", asst.ModelID)
	}
}

func TestMirrorRetractsRefusedAssistant(t *testing.T) {
	_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	m := newSessionMirror(st, sess.ID, "claude-fable-5", domain.ProviderClaudeCode)
	m.Event(ParsedEvent{Kind: KindText, Text: "refused fable output", UUID: "fable-frame"})
	m.Event(ParsedEvent{
		Kind: KindFallback, FromModel: "claude-fable-5", ToModel: "claude-opus-4-8",
		RetractedUUIDs: []string{"fable-frame"}, UUID: "opus-frame",
	})
	m.Event(ParsedEvent{Kind: KindText, Text: "opus replacement", UUID: "opus-frame"})
	m.finish()

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	var texts []string
	var models []string
	nFallback := 0
	for _, msg := range msgs {
		if msg.Role == "assistant" {
			models = append(models, msg.ModelID)
		}
		for _, b := range msg.Content {
			if p, ok := b.AsText(); ok {
				texts = append(texts, p.Text)
			}
			if _, ok := b.AsFallback(); ok {
				nFallback++
			}
		}
	}
	if nFallback != 1 {
		t.Errorf("fallback blocks = %d, want 1", nFallback)
	}
	for _, ttext := range texts {
		if ttext == "refused fable output" {
			t.Errorf("SessionTranscript() still has retracted text %q", ttext)
		}
	}
	found := false
	for _, ttext := range texts {
		if ttext == "opus replacement" {
			found = true
		}
	}
	if !found {
		t.Errorf("SessionTranscript() texts = %v, want opus replacement", texts)
	}
	for _, model := range models {
		if model == "claude-fable-5" {
			t.Errorf("assistant ModelID still fable after retraction: %v", models)
		}
	}
}

func TestMirrorSupersedesOpensFreshReplacement(t *testing.T) {
	_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	m := newSessionMirror(st, sess.ID, "claude-fable-5", domain.ProviderClaudeCode)
	m.Event(ParsedEvent{Kind: KindText, Text: "refused fable output", UUID: "fable-frame"})
	m.Event(ParsedEvent{
		Kind: KindFallback, FromModel: "claude-fable-5", ToModel: "claude-opus-4-8",
		RetractedUUIDs: []string{"fable-frame"}, UUID: "opus-frame",
	})
	m.Event(ParsedEvent{Kind: KindText, Text: "opus replacement", UUID: "opus-frame"})
	m.Event(ParsedEvent{
		Kind: KindFallback, FromModel: "claude-fable-5", ToModel: "claude-opus-4-8",
		RetractedUUIDs: []string{"fable-frame"},
	})
	m.finish()

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	var texts []string
	nFallback := 0
	var asst *domain.Message
	for i := range msgs {
		if msgs[i].Role == "assistant" && asst == nil {
			asst = &msgs[i]
		}
		for _, b := range msgs[i].Content {
			if p, ok := b.AsText(); ok {
				texts = append(texts, p.Text)
			}
			if _, ok := b.AsFallback(); ok {
				nFallback++
			}
		}
	}
	if nFallback != 1 {
		t.Errorf("fallback blocks = %d, want 1", nFallback)
	}
	if asst == nil {
		t.Fatal("SessionTranscript() missing assistant message")
	}
	if asst.ModelID != "claude-opus-4-8" {
		t.Errorf("assistant ModelID = %q, want claude-opus-4-8", asst.ModelID)
	}
	sawFallback, sawText := false, false
	for _, b := range asst.Content {
		if _, ok := b.AsFallback(); ok {
			if sawText {
				t.Error("fallback block after replacement text, want notice before answer")
			}
			sawFallback = true
		}
		if p, ok := b.AsText(); ok && p.Text == "opus replacement" {
			sawText = true
		}
	}
	if !sawFallback {
		t.Error("assistant content missing fallback block before replacement")
	}
	found := false
	for _, ttext := range texts {
		if ttext == "refused fable output" {
			t.Errorf("SessionTranscript() still has retracted text %q", ttext)
		}
		if ttext == "opus replacement" {
			found = true
		}
	}
	if !found {
		t.Errorf("SessionTranscript() texts = %v, want opus replacement kept after later audit retract", texts)
	}
}

func TestMirrorFallbackEnrichesIncompleteFrom(t *testing.T) {
	_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	m := newSessionMirror(st, sess.ID, "claude-fable-5", domain.ProviderClaudeCode)
	m.Event(ParsedEvent{Kind: KindText, Text: "refused fable output", UUID: "fable-frame"})
	m.Event(ParsedEvent{
		Kind: KindFallback, ToModel: "claude-opus-4-8",
		RetractedUUIDs: []string{"fable-frame"}, UUID: "opus-frame",
	})
	m.Event(ParsedEvent{Kind: KindText, Text: "opus replacement", UUID: "opus-frame"})
	m.Event(ParsedEvent{
		Kind: KindFallback, FromModel: "claude-fable-5", ToModel: "claude-opus-4-8",
		RetractedUUIDs: []string{"fable-frame"},
	})
	m.finish()

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	var asst *domain.Message
	nFallback := 0
	var from, to string
	sawFallback, sawText := false, false
	for i := range msgs {
		if msgs[i].Role == "assistant" && asst == nil {
			asst = &msgs[i]
		}
		for _, b := range msgs[i].Content {
			if p, ok := b.AsFallback(); ok {
				nFallback++
				from, to = p.From, p.To
			}
		}
	}
	if nFallback != 1 {
		t.Errorf("fallback blocks = %d, want 1", nFallback)
	}
	if from != "claude-fable-5" || to != "claude-opus-4-8" {
		t.Errorf("fallback payload from=%q to=%q, want fable→opus", from, to)
	}
	if asst == nil {
		t.Fatal("SessionTranscript() missing assistant message")
	}
	if asst.ModelID != "claude-opus-4-8" {
		t.Errorf("assistant ModelID = %q, want claude-opus-4-8", asst.ModelID)
	}
	for _, b := range asst.Content {
		if _, ok := b.AsFallback(); ok {
			if sawText {
				t.Error("fallback block after replacement text, want notice before answer")
			}
			sawFallback = true
		}
		if p, ok := b.AsText(); ok && p.Text == "opus replacement" {
			sawText = true
		}
	}
	if !sawFallback {
		t.Error("assistant content missing fallback block before replacement")
	}
}

func TestMirrorRetractClearsTokenOnlyThinking(t *testing.T) {
	_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: domain.ProviderClaudeCode})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	m := newSessionMirror(st, sess.ID, "claude-fable-5", domain.ProviderClaudeCode)
	m.Event(ParsedEvent{Kind: KindThinkingTokens, Tokens: 120, UUID: "fable-frame"})
	m.Event(ParsedEvent{
		Kind: KindFallback, FromModel: "claude-fable-5", ToModel: "claude-opus-4-8",
		RetractedUUIDs: []string{"fable-frame"}, UUID: "opus-frame",
	})
	m.Event(ParsedEvent{Kind: KindThinkingTokens, Tokens: 8, UUID: "opus-frame"})
	m.Event(ParsedEvent{Kind: KindThinking, Text: "", Signature: "sig", UUID: "opus-frame"})
	m.finish()

	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	var tokens []int64
	for _, msg := range msgs {
		for _, b := range msg.Content {
			if p, ok := b.AsThinking(); ok {
				tokens = append(tokens, p.EstimatedTokens)
			}
		}
	}
	if len(tokens) != 1 || tokens[0] != 8 {
		t.Errorf("thinking EstimatedTokens = %v, want [8] (refused fable estimate must not carry over)", tokens)
	}
}

func TestMirrorCompactWritesSessionContext(t *testing.T) {
	_, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	sess, err := st.CreateSession(domain.Session{Kind: "pair", Provider: domain.ProviderCursor})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	m := newSessionMirror(st, sess.ID, "composer-2.5", domain.ProviderCursor)
	m.Event(ParsedEvent{Kind: KindText, Text: "pre compact reply"})
	m.Event(ParsedEvent{
		Kind:           KindCompact,
		CompactStatus:  domain.CompactDone,
		CompactTrigger: "auto",
		Text:           "older turns folded",
		Tokens:         800_000,
	})
	m.finish()

	ctx, ok := st.ActiveSessionContext(sess.ID)
	if !ok {
		t.Fatal("ActiveSessionContext missing after KindCompact done with summary")
	}
	if ctx.Summary != "older turns folded" {
		t.Errorf("ActiveSessionContext.Summary = %q, want %q", ctx.Summary, "older turns folded")
	}
	header, ok := st.GetSession(sess.ID)
	if !ok || header.ActiveContextID != ctx.ID {
		t.Errorf("GetSession.ActiveContextID = %q, want %s", header.ActiveContextID, ctx.ID)
	}
	if ctx.LastCompactedMessageID == "" {
		t.Error("ActiveSessionContext.LastCompactedMessageID is empty")
	}

	m2 := newSessionMirror(st, sess.ID, "composer-2.5", domain.ProviderCursor)
	m2.Event(ParsedEvent{
		Kind:          KindCompact,
		CompactStatus: domain.CompactDone,
		Text:          "",
	})
	m2.finish()
	after, ok := st.ActiveSessionContext(sess.ID)
	if !ok || after.ID != ctx.ID {
		t.Errorf("empty-summary compact should leave ActiveSessionContext id = %s, got %+v ok=%v", ctx.ID, after, ok)
	}
}
