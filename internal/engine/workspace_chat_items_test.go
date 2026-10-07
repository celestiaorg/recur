package engine

import (
	"encoding/json"
	"strings"
	"testing"

	"recur/internal/domain"
)

// TestWorkspaceChatItems proves the workspace Agent chat can reconstruct the FULL
// turn — assistant text AND the tool calls it made — from the session store (the
// turn mirror records both). This is what lets a reload show tool activity, not just
// the text-only maestro_chat fallback.
func TestWorkspaceChatItems(t *testing.T) {
	// A turn that thinks, runs a tool (Bash), then answers with text — the shape of a
	// PR review: reason, gh/reads, prose last. Thinking must be persisted (lossless
	// session store) and surfaced, not dropped.
	lines := [][]byte{
		[]byte(`{"type":"system","subtype":"init","session_id":"ws-tool-1","model":"claude-sonnet","mcp_servers":[]}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Let me look at the diff first.","signature":"sig-abc"}]}}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"gh pr diff 187","description":"Fetch the PR diff"}}]}}`),
		[]byte(`{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"diff --git a/x b/x"}],"is_error":false}]}}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"text","text":"The PR looks good overall."}]}}`),
		[]byte(`{"type":"result","subtype":"success","session_id":"ws-tool-1","duration_ms":900,"is_error":false,"num_turns":1,"result":"The PR looks good overall.","total_cost_usd":0.05,"usage":{"input_tokens":200,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":15000},"modelUsage":{"claude-sonnet":{"contextWindow":200000,"inputTokens":200,"outputTokens":50,"cacheReadInputTokens":15000,"cacheCreationInputTokens":0}}}`),
	}
	runner := &captureRunner{linesFor: func(int) [][]byte { return lines }}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	eng.SendWorkspaceMessage(ws.ID, "", "review pr 187", "sonnet", nil, nil, "", "")

	items := eng.WorkspaceChatItems(ws.ID, "")
	// Expect: user prompt, the thinking, the Bash tool call, the assistant's text,
	// then the turn footer carrying the turn's wall time.
	// Tool RESULT bodies are intentionally omitted (they can be huge diffs).
	if len(items) != 5 {
		t.Fatalf("want 5 items (user, thinking, tool, text, turn), got %d: %+v", len(items), items)
	}
	if items[0].Role != "user" || items[0].Kind != "text" || items[0].Text != "review pr 187" {
		t.Errorf("item0 should be the user prompt, got %+v", items[0])
	}
	if items[0].Ts <= 0 {
		t.Errorf("item0 should carry a timestamp for the Loop cockpit merge, got %+v", items[0])
	}
	if items[1].Kind != "thinking" || items[1].Text != "Let me look at the diff first." {
		t.Errorf("item1 should be the thinking block (persisted, not dropped), got %+v", items[1])
	}
	if items[2].Kind != "tool" || items[2].Tool != "Bash" {
		t.Errorf("item2 should be the Bash tool call, got %+v", items[2])
	}
	if items[2].ToolInput == "" {
		t.Errorf("tool call should carry an input summary (the command), got empty")
	}
	// The expanded detail card needs the agent's description, the full input JSON,
	// and the tool RESULT attached to the call (matched by tool-use id).
	if items[2].Description != "Fetch the PR diff" {
		t.Errorf("tool description: got %q, want the agent's input.description", items[2].Description)
	}
	if !strings.Contains(items[2].Input, "gh pr diff 187") {
		t.Errorf("tool input JSON should be carried for the detail card, got %q", items[2].Input)
	}
	if items[2].Result != "diff --git a/x b/x" || items[2].ResultError {
		t.Errorf("tool result not attached: %+v", items[2])
	}
	if items[3].Role != "assistant" || items[3].Kind != "text" || items[3].Text != "The PR looks good overall." {
		t.Errorf("item3 should be the assistant text, got %+v", items[3])
	}
	// The turn closes with its wall time, so a finished turn keeps the run time
	// that was ticking while it ran instead of losing it.
	if items[4].Kind != "turn" || items[4].DurationMS <= 0 {
		t.Errorf("item4 should be the turn footer with a duration, got %+v", items[4])
	}
	if items[4].MessageID == "" {
		t.Errorf("item4 should carry the fork-point message id, got %+v", items[4])
	}
	// Context usage rides on the same turn footer for the composer meter.
	// Sonnet 5 is always 1M — not modelUsage's stamped ceiling.
	if items[4].InputTokens != 200 || items[4].CacheReadTokens != 15000 || items[4].ContextWindow != 1_000_000 {
		t.Errorf("item4 context usage wrong: %+v", items[4])
	}

	// No session yet for a different workspace → empty, not an error.
	if got := eng.WorkspaceChatItems("nope", ""); got != nil {
		t.Errorf("unknown workspace should have no transcript, got %+v", got)
	}
}

func TestFlattenTranscriptCompact(t *testing.T) {
	msgs := []domain.Message{{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.CompactBlock(domain.CompactDone, "auto", "older turns folded", 167219),
			domain.TextBlock("continuing"),
		},
	}}
	items := flattenTranscript(msgs, "claude-code/sonnet", nil)
	if len(items) < 2 {
		t.Fatalf("flattenTranscript() len = %d, want at least compact+text", len(items))
	}
	if items[0].Kind != "compact" || items[0].Tokens != 167219 || items[0].Description != "auto" {
		t.Errorf("compact item = %+v, want kind=compact tokens=167219 trigger=auto", items[0])
	}
	if items[0].Text != "older turns folded" {
		t.Errorf("compact summary = %q, want older turns folded", items[0].Text)
	}
}

// A steer lands mid-tool, so the next tool_result shares its user row.
func TestFlattenTranscriptSteerWithToolResultShowsDisplay(t *testing.T) {
	msgs := []domain.Message{
		{Role: "assistant", Content: []domain.ContentBlock{
			domain.ToolUseBlock("int-bash", "toolu_1", "Bash", json.RawMessage(`{"command":"npm i"}`)),
		}},
		{Role: "user", Content: []domain.ContentBlock{
			domain.TextBlockDisplay("The operator is looking at this Recur workspace.\n\nuse pnpm", "use pnpm"),
			domain.ToolResultBlock("int-bash", "ok", false),
		}},
	}
	for _, it := range flattenTranscript(msgs, "claude-code/sonnet", nil) {
		if it.Role == "user" && it.Kind == "text" {
			if it.Text != "use pnpm" {
				t.Errorf("steer bubble = %q, want use pnpm", it.Text)
			}
			return
		}
	}
	t.Fatal("no user text item")
}

func TestFlattenTranscriptNestsTaskSubagentTools(t *testing.T) {
	msgs := []domain.Message{{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("int-task", "toolu_task_1", "Task", json.RawMessage(`{"description":"Map UI","subagent_type":"Explore","prompt":"Search frontend"}`)),
			domain.ToolUseBlockNested("int-grep", "t_nested", "toolu_task_1", "Grep", json.RawMessage(`{"pattern":"PRDetail"}`)),
		},
	}, {
		Role: "user",
		Content: []domain.ContentBlock{
			domain.ToolResultBlock("int-grep", "src/PR.svelte", false),
		},
	}}
	items := flattenTranscript(msgs, "", nil)
	var task *domain.ChatItem
	for i := range items {
		if items[i].Tool == "Task" {
			task = &items[i]
		}
		if items[i].Tool == "Grep" {
			t.Fatalf("nested Grep leaked as a sibling row: %+v", items[i])
		}
	}
	if task == nil {
		t.Fatalf("want a parent Task row, got %+v", items)
	}
	if len(task.Children) != 1 {
		t.Fatalf("Task children = %d, want 1: %+v", len(task.Children), task.Children)
	}
	child := task.Children[0]
	if child.Tool != "Grep" {
		t.Errorf("nested tool = %q, want Grep", child.Tool)
	}
	if child.ResultPending {
		t.Errorf("nested Grep still pending after result attached")
	}
	if !strings.Contains(child.Result, "PR.svelte") {
		t.Errorf("nested Grep result = %q, want PR.svelte", child.Result)
	}
}

func TestFlattenTranscriptNestsSubagentOutput(t *testing.T) {
	agentID := "tu-agent"
	grepID := "tu-grep"
	msgs := []domain.Message{
		{
			Role: "assistant",
			Content: []domain.ContentBlock{
				domain.ToolUseBlock(agentID, "toolu_agent", "Agent", json.RawMessage(`{"description":"Explore the repo","prompt":"look around"}`)),
				domain.ToolUseBlock(grepID, "toolu_grep", "Grep", json.RawMessage(`{"pattern":"foo"}`)).WithParent("toolu_agent"),
				domain.TextBlock("found it in foo.go").WithParent("toolu_agent"),
			},
		},
		{
			Role: "user",
			Content: []domain.ContentBlock{
				domain.ToolResultBlock(grepID, "foo.go:1: foo", false).WithParent("toolu_agent"),
				domain.ToolResultBlock(agentID, "explored the repo", false),
			},
		},
	}
	items := flattenTranscript(msgs, "", nil)
	if len(items) != 1 {
		t.Fatalf("want 1 top-level item (the Agent card), got %d: %+v", len(items), items)
	}
	if items[0].Kind != "tool" || items[0].Tool != "Task" {
		t.Fatalf("top-level should be the Agent/Task card, got %+v", items[0])
	}
	if items[0].ToolID != "toolu_agent" {
		t.Errorf("Agent ToolID = %q, want toolu_agent", items[0].ToolID)
	}
	if n := len(items[0].Children); n != 2 {
		t.Fatalf("want 2 nested children (Grep + text), got %d: %+v", n, items[0].Children)
	}
	grep := items[0].Children[0]
	if grep.Kind != "tool" || grep.Tool != "Grep" {
		t.Errorf("first child should be Grep, got %+v", grep)
	}
	if grep.Result != "foo.go:1: foo" {
		t.Errorf("nested Grep result not attached: %+v", grep)
	}
	if items[0].Children[1].Kind != "text" || items[0].Children[1].Text != "found it in foo.go" {
		t.Errorf("second child should be nested text, got %+v", items[0].Children[1])
	}
}

func TestFlattenTranscriptDropsOrphanNestedBlocks(t *testing.T) {
	msgs := []domain.Message{{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("tu-grep", "toolu_grep", "Grep", json.RawMessage(`{"pattern":"foo"}`)).WithParent("toolu_missing"),
			domain.TextBlock("parent agent reply"),
		},
	}}
	items := flattenTranscript(msgs, "", nil)
	if len(items) != 1 || items[0].Kind != "text" {
		t.Fatalf("orphan nested Grep must not leak; want only parent text, got %+v", items)
	}
}

func TestWorkspaceChatItemsNestsSubagentOutput(t *testing.T) {
	lines := [][]byte{
		[]byte(`{"type":"system","subtype":"init","session_id":"ws-sa-1","model":"claude-sonnet","mcp_servers":[]}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_agent","name":"Agent","input":{"description":"Explore","prompt":"look around","subagent_type":"Explore"}}]}}`),
		[]byte(`{"type":"assistant","parent_tool_use_id":"toolu_agent","message":{"content":[{"type":"tool_use","id":"toolu_grep","name":"Grep","input":{"pattern":"foo"}}]}}`),
		[]byte(`{"type":"user","parent_tool_use_id":"toolu_agent","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_grep","content":[{"type":"text","text":"foo.go:1"}],"is_error":false}]}}`),
		[]byte(`{"type":"assistant","parent_tool_use_id":"toolu_agent","message":{"content":[{"type":"text","text":"Found foo.go"}]}}`),
		[]byte(`{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent","content":[{"type":"text","text":"explored"}],"is_error":false}]}}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"text","text":"The subagent finished."}]}}`),
		[]byte(`{"type":"result","subtype":"success","session_id":"ws-sa-1","duration_ms":400,"is_error":false,"num_turns":1,"result":"The subagent finished.","total_cost_usd":0.01}`),
	}
	runner := &captureRunner{linesFor: func(int) [][]byte { return lines }}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	eng.SendWorkspaceMessage(ws.ID, "", "explore foo", "sonnet", nil, nil, "", "")

	items := contentItems(eng.WorkspaceChatItems(ws.ID, ""))
	var greps, texts int
	var agent *domain.ChatItem
	for i := range items {
		it := &items[i]
		if it.Tool == "Grep" {
			greps++
		}
		if it.Kind == "text" && it.Text == "Found foo.go" {
			texts++
		}
		if it.Kind == "tool" && it.Tool == "Task" {
			agent = it
		}
	}
	if greps != 0 {
		t.Fatalf("nested Grep leaked into the main transcript (%d top-level Grep rows)", greps)
	}
	if texts != 0 {
		t.Fatalf("nested subagent text leaked into the main transcript")
	}
	if agent == nil {
		t.Fatalf("want an Agent/Task card in the transcript, got %+v", items)
	}
	if len(agent.Children) < 2 {
		t.Fatalf("Agent card should nest Grep + text, got %+v", agent.Children)
	}
	if agent.Children[0].Tool != "Grep" {
		t.Errorf("first nested child should be Grep, got %+v", agent.Children[0])
	}
	if agent.Children[0].Result != "foo.go:1" {
		t.Errorf("nested Grep result = %q", agent.Children[0].Result)
	}
	foundText := false
	for _, c := range agent.Children {
		if c.Kind == "text" && c.Text == "Found foo.go" {
			foundText = true
		}
	}
	if !foundText {
		t.Errorf("nested subagent text missing from Agent card children: %+v", agent.Children)
	}
}

func TestFlattenTranscriptFallback(t *testing.T) {
	msgs := []domain.Message{{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.FallbackBlock("claude-fable-5", "claude-opus-4-8"),
			domain.TextBlock("continuing"),
		},
	}}
	items := flattenTranscript(msgs, "claude-code/fable", nil)
	if len(items) < 2 {
		t.Fatalf("flattenTranscript() len = %d, want at least fallback+text", len(items))
	}
	if items[0].Kind != "fallback" || items[0].Text != "Opus 4.8" || items[0].Description != "Fable 5" {
		t.Errorf("flattenTranscript(fallback) = %+v, want kind=fallback text=Opus 4.8 description=Fable 5", items[0])
	}
	if items[1].Kind != "text" || items[1].Text != "continuing" {
		t.Errorf("flattenTranscript(fallback)[1] = %+v, want text continuing", items[1])
	}
}
