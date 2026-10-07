package engine

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	"recur/internal/cursor"
	"recur/internal/domain"
)

// Fixtures are VERBATIM args/results captured from a live cursor-sdk-bridge run
// (internal/cursor probe). Cursor documents neither its tool list nor these
// shapes, so pinning real traffic is the only way to keep the mapping honest.
const (
	argsShell     = `{"command":"echo hi","timeout":30000}`
	argsRead      = `{"path":"/wt/hello.txt"}`
	argsEdit      = `{"path":"/wt/hello.txt"}`
	argsGlob      = `{"globPattern":"*.md","targetDirectory":"/wt"}`
	argsGrep      = `{"offset":0,"path":"/wt","pattern":"bravo"}`
	argsDelete    = `{"path":"/wt/out.txt"}`
	argsTodos     = `{"todos":[{"content":"List the directory","status":"completed"}]}`
	argsPlan      = `{"plan":"Summary: touch work.txt.\n\nSteps:\n1. Write the marker\n\nDefinition of done: work.txt exists."}`
	argsPlanEmpty = `{"plan":""}`

	resShell  = `{"status":"success","value":{"executionTime":467,"exitCode":0,"signal":"","stderr":"","stdout":"hi\n"}}`
	resPlan   = `{"status":"success","value":{}}`
	resShellE = `{"status":"success","value":{"exitCode":2,"stderr":"boom","stdout":"partial\n"}}`
	resRead   = `{"status":"success","value":{"content":"alpha\nbravo\ncharlie\n","fileSize":20,"totalLines":4}}`
	resEdit   = `{"status":"success","value":{"diffString":"--- a/hello.txt\n+++ b/hello.txt\n@@ -1,3 +1,3 @@\n alpha\n-bravo\n+BRAVO\n charlie","linesAdded":1,"linesRemoved":1}}`
	resGlob   = `{"status":"success","value":{"clientTruncated":false,"files":["./notes.md","./hello.txt"],"totalFiles":2}}`
	resGrep   = `{"status":"success","value":{"workspaceResults":{"/wt":{"output":{"matches":[{"file":"./hello.txt"}],"totalMatches":0},"type":"content"}}}}`
	resMCP    = `{"status":"success","value":{"content":[{"text":{"text":"The magic number is 4271."}}],"isError":false}}`
)

func TestCanonicalToolNames(t *testing.T) {
	// Cursor's names are lowercase and differ from Recur's vocabulary, so without
	// this map every tool renders with a generic icon and a raw name.
	cases := map[string]string{
		"shell":       "Bash",
		"read":        "Read",
		"edit":        "Edit",
		"write":       "Write",
		"glob":        "Glob",
		"grep":        "Grep",
		"delete":      "Delete",
		"updateTodos": "TodoWrite",
		"task":        "Task",
		"createPlan":  "CreatePlan",
		// A tool Cursor adds later must pass through, not vanish.
		"somethingNew": "somethingNew",
	}
	for in, want := range cases {
		if got := canonicalTool(in); got != want {
			t.Errorf("canonicalTool(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestCursorToolInputIsCanonical(t *testing.T) {
	cases := []struct {
		name string
		args string
		want map[string]any
	}{
		{"shell", argsShell, map[string]any{"command": "echo hi"}},
		{"read", argsRead, map[string]any{"file_path": "/wt/hello.txt"}},
		{"edit", argsEdit, map[string]any{"file_path": "/wt/hello.txt"}},
		{"delete", argsDelete, map[string]any{"file_path": "/wt/out.txt"}},
		{"glob", argsGlob, map[string]any{"pattern": "*.md", "path": "/wt"}},
		{"grep", argsGrep, map[string]any{"pattern": "bravo", "path": "/wt"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			raw := cursorToolInput(tc.name, json.RawMessage(tc.args))
			var got map[string]any
			if err := json.Unmarshal(raw, &got); err != nil {
				t.Fatalf("decode %s: %v", raw, err)
			}
			for k, want := range tc.want {
				if got[k] != want {
					t.Errorf("%s: %q = %v, want %v", tc.name, k, got[k], want)
				}
			}
			// Cursor plumbing must not leak into the card.
			if _, ok := got["timeout"]; ok {
				t.Error("timeout leaked into canonical input")
			}
			if _, ok := got["offset"]; ok {
				t.Error("offset leaked into canonical input")
			}
		})
	}
}

// The canonical input must be exactly what the Claude summarizer and the tool
// cards already read, or the mapping is pointless.
func TestCursorInputFeedsClaudeSummarizer(t *testing.T) {
	cases := []struct{ tool, args, want string }{
		{"shell", argsShell, "echo hi"},
		{"read", argsRead, "/wt/hello.txt"},
		{"edit", argsEdit, "/wt/hello.txt"},
		{"delete", argsDelete, "/wt/out.txt"},
		{"glob", argsGlob, "*.md"},
	}
	for _, tc := range cases {
		canonical := canonicalTool(tc.tool)
		raw := cursorToolInput(tc.tool, json.RawMessage(tc.args))
		if got := summarizeToolInput(canonical, raw); got != tc.want {
			t.Errorf("%s → %s: summary = %q, want %q", tc.tool, canonical, got, tc.want)
		}
	}
}

func TestCursorToolInputPassesUnknownThrough(t *testing.T) {
	raw := cursorToolInput("brandNew", json.RawMessage(`{"foo":"bar"}`))
	if !strings.Contains(string(raw), `"foo"`) {
		t.Errorf("unknown tool args dropped: %s", raw)
	}
}

func TestCursorToolResultText(t *testing.T) {
	cases := []struct {
		name   string
		tool   string
		result string
		want   string
	}{
		{"shell stdout", "shell", resShell, "hi\n"},
		{"read content", "read", resRead, "alpha\nbravo\ncharlie\n"},
		{"glob files", "glob", resGlob, "./notes.md\n./hello.txt"},
		{"grep matches", "grep", resGrep, "./hello.txt"},
		{"mcp text", cursor.ToolNameMCP, resMCP, "The magic number is 4271."},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := cursorToolResultText(tc.tool, json.RawMessage(tc.result)); got != tc.want {
				t.Errorf("got %q, want %q", got, tc.want)
			}
		})
	}
}

// A failing shell command must surface stderr AND the exit code, or the terminal
// card shows a silent failure.
func TestCursorShellResultIncludesStderrAndExit(t *testing.T) {
	got := cursorToolResultText("shell", json.RawMessage(resShellE))
	for _, want := range []string{"partial", "boom", "exit 2"} {
		if !strings.Contains(got, want) {
			t.Errorf("result %q missing %q", got, want)
		}
	}
}

// Cursor's edit args carry ONLY the path — the actual change lives in the
// result's unified diff, so this is the sole place the edit is visible.
func TestCursorEditResultCarriesDiff(t *testing.T) {
	got := cursorToolResultText("edit", json.RawMessage(resEdit))
	if !strings.Contains(got, "-bravo") || !strings.Contains(got, "+BRAVO") {
		t.Errorf("edit result lost the diff: %q", got)
	}
}

func TestCursorToolResultEmptyInputs(t *testing.T) {
	if got := cursorToolResultText("shell", nil); got != "" {
		t.Errorf("nil result = %q, want empty", got)
	}
	if got := cursorToolResultText("shell", json.RawMessage(`not json`)); got != "" {
		t.Errorf("garbage result = %q, want empty", got)
	}
}

// --- end-to-end through the runner's event handler ---------------------------

func cursorToolFrame(name, status, args, result string) string {
	return cursorToolFrameID("c1", name, status, args, result)
}

func cursorToolFrameID(callID, name, status, args, result string) string {
	payload := map[string]any{
		"type": "tool_call", "call_id": callID, "name": name, "status": status,
	}
	if args != "" {
		payload["args"] = json.RawMessage(args)
	}
	if result != "" {
		payload["result"] = json.RawMessage(result)
	}
	b, _ := json.Marshal(map[string]any{"sdkMessage": map[string]any{"type": "tool_call", "message": payload}})
	return string(b)
}

// The runner emits RAW provider data — that is what the session store records.
// Rendering happens later, at the display boundary.
func TestCursorTurnEmitsRawProviderToolData(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("shell", "running", argsShell, ""),
		cursorToolFrame("shell", "completed", argsShell, resShell),
	)
	if len(got) != 2 {
		t.Fatalf("got %d events: %+v", len(got), got)
	}
	step, res := got[0], got[1]

	if step.Kind != KindStep || step.Tool != "shell" {
		t.Errorf("step tool = %q, want Cursor's own %q", step.Tool, "shell")
	}
	// Verbatim args, Cursor's field names and plumbing included.
	if !strings.Contains(string(step.ToolInputRaw), `"timeout"`) {
		t.Errorf("ToolInputRaw = %s, want Cursor's verbatim args", step.ToolInputRaw)
	}
	// Every event must be tagged, or a display layer cannot interpret the above.
	if step.Provider != domain.ProviderCursor {
		t.Errorf("step provider = %q, want %q", step.Provider, domain.ProviderCursor)
	}
	if res.Provider != domain.ProviderCursor {
		t.Errorf("result provider = %q", res.Provider)
	}
	// The result keeps Cursor's envelope; unpacking is a display concern.
	if !strings.Contains(res.Text, `"status"`) {
		t.Errorf("result text = %q, want Cursor's verbatim envelope", res.Text)
	}
	// The feed's one-line summary IS a display concern, so it is already common.
	if step.ToolInput != "echo hi" {
		t.Errorf("ToolInput summary = %q, want the common summary", step.ToolInput)
	}
}

// …and the display boundary turns that raw data into the common vocabulary.
func TestDisplayToolRendersCommonVocabulary(t *testing.T) {
	d := displayTool(domain.ProviderCursor, "shell", json.RawMessage(argsShell))
	if d.Name != "Bash" {
		t.Errorf("name = %q, want Bash", d.Name)
	}
	if !strings.Contains(string(d.Input), `"command":"echo hi"`) {
		t.Errorf("input = %s, want the common command field", d.Input)
	}
	if strings.Contains(string(d.Input), "timeout") {
		t.Errorf("provider plumbing leaked into the display input: %s", d.Input)
	}
	if got := displayToolResult(domain.ProviderCursor, "shell", resShell); got != "hi\n" {
		t.Errorf("result = %q, want the stdout", got)
	}
}

// Claude Code defined the common vocabulary, so its calls must pass through
// untouched — no accidental double-mapping.
func TestDisplayToolPassesClaudeThrough(t *testing.T) {
	in := json.RawMessage(`{"command":"ls","description":"list"}`)
	d := displayTool(domain.ProviderClaudeCode, "Bash", in)
	if d.Name != "Bash" || string(d.Input) != string(in) {
		t.Errorf("claude call was rewritten: %+v", d)
	}
	if got := displayToolResult(domain.ProviderClaudeCode, "Bash", "raw output"); got != "raw output" {
		t.Errorf("claude result rewritten: %q", got)
	}
}

// MCP tools keep their mcp__server__tool label and their own parameters, and are
// NOT run through the built-in canonical map.
func TestCursorMCPToolDisplay(t *testing.T) {
	args := `{"providerIdentifier":"recur","toolName":"done","args":{"summary":"all done"}}`
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("mcp", "running", args, ""),
		cursorToolFrame("mcp", "completed", args, resMCP),
	)
	if len(got) != 2 {
		t.Fatalf("got %d events: %+v", len(got), got)
	}
	// Stored raw: Cursor calls every MCP tool "mcp" and routes via the args.
	if got[0].Tool != "mcp" {
		t.Errorf("stored tool = %q, want Cursor's raw %q", got[0].Tool, "mcp")
	}
	if !strings.Contains(string(got[0].ToolInputRaw), "providerIdentifier") {
		t.Errorf("routing envelope must be preserved in the raw input: %s", got[0].ToolInputRaw)
	}

	// Displayed common: the specific tool, with its own params.
	d := displayToolFor(got[0])
	if d.Name != "mcp__recur__done" {
		t.Errorf("display name = %q, want mcp__recur__done", d.Name)
	}
	if !strings.Contains(string(d.Input), `"summary"`) {
		t.Errorf("display input = %s, want the tool's own params", d.Input)
	}
	if strings.Contains(string(d.Input), "providerIdentifier") {
		t.Errorf("routing envelope leaked into the display input: %s", d.Input)
	}
	if got := displayToolResult(domain.ProviderCursor, "mcp", resMCP); got != "The magic number is 4271." {
		t.Errorf("mcp display result = %q", got)
	}
}

// --- thinking ----------------------------------------------------------------

func cursorThinkingFrame(text string, durationMS int) string {
	payload := map[string]any{"type": "thinking", "text": text}
	if durationMS > 0 {
		payload["thinking_duration_ms"] = durationMS
	}
	b, _ := json.Marshal(map[string]any{"sdkMessage": map[string]any{"type": "thinking", "message": payload}})
	return string(b)
}

// Cursor streams reasoning as a run of text frames terminated by an empty one.
// Without accumulation the transcript gets no thinking block at all, and the
// Activity panel shows an unexplained gap before each tool call.
func TestCursorThinkingBecomesOneBlock(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorThinkingFrame("I should ", 0),
		cursorThinkingFrame("read the file.", 0),
		cursorThinkingFrame("", 42),
	)
	var thinking []ParsedEvent
	for _, e := range got {
		if e.Kind == KindThinking {
			thinking = append(thinking, e)
		}
	}
	if len(thinking) != 1 {
		t.Fatalf("got %d thinking events, want 1: %+v", len(thinking), got)
	}
	if thinking[0].Text != "I should read the file." {
		t.Errorf("thinking text = %q", thinking[0].Text)
	}
}

// A tool call closes the reasoning that led to it, so transcript order matches
// what actually happened.
func TestCursorThinkingFlushesBeforeToolCall(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorThinkingFrame("let me look", 0),
		cursorToolFrame("read", "running", argsRead, ""),
	)
	if len(got) < 2 {
		t.Fatalf("got %+v", got)
	}
	if got[0].Kind != KindThinking {
		t.Errorf("first = %q, want thinking before the tool call", got[0].Kind)
	}
	if got[1].Kind != KindStep || got[1].Tool != "read" {
		t.Errorf("second = %+v, want Cursor's raw read step", got[1])
	}
	if d := displayToolFor(got[1]); d.Name != "Read" {
		t.Errorf("display name = %q, want Read", d.Name)
	}
}

// Reasoning still in flight when the run ends must not be dropped.
func TestCursorThinkingFlushedOnResult(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorThinkingFrame("half a thought", 0),
		fResultOK,
	)
	found := false
	for _, e := range got {
		if e.Kind == KindThinking && e.Text == "half a thought" {
			found = true
		}
	}
	if !found {
		t.Errorf("unterminated reasoning dropped: %+v", got)
	}
}

// Token-level reasoning must stay out of the feed for normal roles; only the
// Maestro chat opts in.
func TestCursorThinkingDeltasOnlyWithPartials(t *testing.T) {
	_, quiet := feed(t, AgentSpec{}, cursorThinkingFrame("hmm", 0), cursorThinkingFrame("", 1))
	for _, e := range quiet {
		if e.Kind == KindTextDelta || e.Kind == KindThinkingDelta {
			t.Errorf("delta leaked into a non-chat turn: %+v", quiet)
		}
	}
	_, chat := feed(t, AgentSpec{IncludePartials: true}, cursorThinkingFrame("hmm", 0), cursorThinkingFrame("", 1))
	sawDelta := false
	for _, e := range chat {
		if e.Kind == KindThinkingDelta {
			sawDelta = true
		}
	}
	if !sawDelta {
		t.Errorf("Maestro chat lost its live thinking deltas: %+v", chat)
	}
}

// TestTranscriptStoresRawRendersCommon is the contract test for the split: the
// session store keeps the provider's own vocabulary, and the UI-facing ChatItem
// shows the common one.
//
// Deriving the display at READ time (not write time) is what lets a mapping fix
// re-render existing transcripts. If this ever inverts — canonical names in the
// blocks — the DB stops being a record of what the provider actually did.
func TestTranscriptStoresRawRendersCommon(t *testing.T) {
	msgs := []domain.Message{{
		Role:     "assistant",
		Provider: domain.ProviderCursor,
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "call-1", "shell", json.RawMessage(argsShell)),
		},
	}, {
		Role:     "user",
		Provider: domain.ProviderCursor,
		Content: []domain.ContentBlock{
			domain.ToolResultBlock("t1", resShell, false),
		},
	}}

	// Stored: Cursor's own name and args, verbatim.
	p, ok := msgs[0].Content[0].AsToolUse()
	if !ok || p.Name != "shell" {
		t.Fatalf("block name = %q, want the raw shell", p.Name)
	}
	if !strings.Contains(string(p.Input), "timeout") {
		t.Errorf("block input = %s, want Cursor's verbatim args", p.Input)
	}

	// Rendered: the common vocabulary.
	items := flattenTranscript(msgs, "", nil)
	var tool *domain.ChatItem
	for i := range items {
		if items[i].Kind == "tool" {
			tool = &items[i]
		}
	}
	if tool == nil {
		t.Fatal("no tool item rendered")
	}
	if tool.Tool != "Bash" {
		t.Errorf("ChatItem.Tool = %q, want Bash", tool.Tool)
	}
	if !strings.Contains(tool.Input, `"command"`) {
		t.Errorf("ChatItem.Input = %s, want the common command field", tool.Input)
	}
	if strings.Contains(tool.Input, "timeout") {
		t.Errorf("provider plumbing reached the UI: %s", tool.Input)
	}
	if tool.ToolInput != "echo hi" {
		t.Errorf("ChatItem.ToolInput = %q", tool.ToolInput)
	}
	// The result is stored in Cursor's envelope and unpacked for display, which
	// needs the ORIGINATING call's provider+tool — a result block alone cannot
	// say how to read itself.
	if tool.Result != "hi\n" {
		t.Errorf("ChatItem.Result = %q, want the stdout", tool.Result)
	}
}

// A Claude transcript must render identically to before — the display layer is
// a no-op for the provider that defined the vocabulary.
func TestTranscriptClaudeUnchanged(t *testing.T) {
	in := `{"command":"ls -la","description":"List files"}`
	msgs := []domain.Message{{
		Role:     "assistant",
		Provider: domain.ProviderClaudeCode,
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "toolu_1", "Bash", json.RawMessage(in)),
		},
	}, {
		Role:     "user",
		Provider: domain.ProviderClaudeCode,
		Content:  []domain.ContentBlock{domain.ToolResultBlock("t1", "total 0", false)},
	}}

	items := flattenTranscript(msgs, "", nil)
	if len(items) == 0 || items[0].Tool != "Bash" {
		t.Fatalf("items = %+v", items)
	}
	if items[0].Description != "List files" {
		t.Errorf("description = %q, want the agent's own label", items[0].Description)
	}
	if items[0].Result != "total 0" {
		t.Errorf("result = %q, want the raw text unchanged", items[0].Result)
	}
}

// Cursor delete args are {path}; the common shape is {file_path}. The one-line
// summary must be the path — the chat header used to dump truncated JSON.
func TestTranscriptDeleteSummarizesPath(t *testing.T) {
	msgs := []domain.Message{{
		Role:     "assistant",
		Provider: domain.ProviderCursor,
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "c1", "delete", json.RawMessage(argsDelete)),
		},
	}, {
		Role:     "user",
		Provider: domain.ProviderCursor,
		Content: []domain.ContentBlock{
			domain.ToolResultBlock("t1", `{"status":"success","value":{}}`, false),
		},
	}}
	items := flattenTranscript(msgs, "", nil)
	var tool *domain.ChatItem
	for i := range items {
		if items[i].Kind == "tool" {
			tool = &items[i]
		}
	}
	if tool == nil {
		t.Fatal("no tool item rendered")
	}
	if tool.Tool != "Delete" {
		t.Errorf("ChatItem.Tool = %q, want Delete", tool.Tool)
	}
	if tool.ToolInput != "/wt/out.txt" {
		t.Errorf("ChatItem.ToolInput = %q, want the path not JSON", tool.ToolInput)
	}
	if strings.Contains(tool.ToolInput, "{") {
		t.Errorf("Delete summary dumped JSON: %q", tool.ToolInput)
	}
	if tool.Result != "" {
		t.Errorf("ChatItem.Result = %q, want empty (Cursor delete is an ack)", tool.Result)
	}
}

// --- tool call lifecycle -----------------------------------------------------

// Cursor re-reports progress for some tools: `edit` sends `running` twice before
// `completed`. Emitting a KindStep per frame minted TWO tool_use blocks, and
// because the id map keeps only the last, the result attached to the second —
// so the first rendered as a bare input card with no diff. That is exactly the
// "edit sometimes just shows a file path" symptom.
func TestCursorRepeatedRunningYieldsOneStep(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("edit", "running", argsEdit, ""),
		cursorToolFrame("edit", "running", argsEdit, ""),
		cursorToolFrame("edit", "completed", argsEdit, resEdit),
	)
	var steps, results int
	for _, e := range got {
		switch e.Kind {
		case KindStep:
			steps++
		case KindToolResult:
			results++
		}
	}
	if steps != 1 {
		t.Errorf("got %d steps, want exactly 1 (a duplicate orphans the result)", steps)
	}
	if results != 1 {
		t.Errorf("got %d results, want exactly 1", results)
	}
}

// Live grok-4.6 ids from Recur's session store join call-… and fc_… with a
// newline. Repeating running frames under that id must still be one KindStep.
func TestCursorGrok46NewlineIDRepeatedRunningYieldsOneStep(t *testing.T) {
	const id = "call-9812a46f-b258-4ed8-8d09-55441deb1733-5\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_0"
	args := `{"globPattern":"TODO.md"}`
	_, got := feed(t, AgentSpec{},
		cursorToolFrameID(id, "glob", "running", args, ""),
		cursorToolFrameID(id, "glob", "running", args, ""),
		cursorToolFrameID(id, "glob", "running", args, ""),
		cursorToolFrameID(id, "glob", "completed", args, resGlob),
	)
	var steps int
	var stepID string
	for _, e := range got {
		if e.Kind == KindStep {
			steps++
			stepID = e.ToolUseID
		}
	}
	if steps != 1 {
		t.Errorf("got %d steps, want 1", steps)
	}
	if stepID != id {
		t.Errorf("ToolUseID = %q, want grok-4.6 newline id", stepID)
	}
}

// The same grok-4.6 batch (TODO.md / README* / Makefile) used three distinct
// newline ids — those must not collapse into one step.
func TestCursorGrok46ParallelGlobsEachGetAStep(t *testing.T) {
	ids := []string{
		"call-9812a46f-b258-4ed8-8d09-55441deb1733-5\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_0",
		"call-9812a46f-b258-4ed8-8d09-55441deb1733-6\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_1",
		"call-9812a46f-b258-4ed8-8d09-55441deb1733-7\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_2",
	}
	args := []string{
		`{"globPattern":"TODO.md"}`,
		`{"globPattern":"README*"}`,
		`{"globPattern":"Makefile"}`,
	}
	var frames []string
	for i, id := range ids {
		frames = append(frames, cursorToolFrameID(id, "glob", "running", args[i], ""))
	}
	for i, id := range ids {
		frames = append(frames, cursorToolFrameID(id, "glob", "completed", args[i], resGlob))
	}
	_, got := feed(t, AgentSpec{}, frames...)
	seen := map[string]bool{}
	for _, e := range got {
		if e.Kind == KindStep {
			seen[e.ToolUseID] = true
		}
	}
	if len(seen) != 3 {
		t.Errorf("steps=%d ids=%v, want 3", len(seen), seen)
	}
}

// Parallel calls must not dedupe each other — the id is what distinguishes them.
func TestCursorParallelCallsEachGetAStep(t *testing.T) {
	frame := func(id, status, result string) string {
		payload := map[string]any{
			"type": "tool_call", "call_id": id, "name": "read", "status": status,
			"args": json.RawMessage(argsRead),
		}
		if result != "" {
			payload["result"] = json.RawMessage(result)
		}
		b, _ := json.Marshal(map[string]any{"sdkMessage": map[string]any{"type": "tool_call", "message": payload}})
		return string(b)
	}
	_, got := feed(t, AgentSpec{},
		frame("a", "running", ""), frame("b", "running", ""), frame("c", "running", ""),
		frame("a", "completed", resRead), frame("b", "completed", resRead), frame("c", "completed", resRead),
	)
	steps, results := map[string]bool{}, map[string]bool{}
	for _, e := range got {
		switch e.Kind {
		case KindStep:
			steps[e.ToolUseID] = true
		case KindToolResult:
			results[e.ToolUseID] = true
		}
	}
	if len(steps) != 3 || len(results) != 3 {
		t.Errorf("steps=%v results=%v, want 3 of each", steps, results)
	}
}

// Matching only completed/failed meant an undocumented terminal status silently
// dropped the result, leaving a call with no output — indistinguishable from a
// tool that legitimately returned nothing. Any non-running status is terminal.
func TestCursorUnknownTerminalStatusStillProducesResult(t *testing.T) {
	for _, status := range []string{"failed", "aborted", "rejected", "error", "cancelled"} {
		t.Run(status, func(t *testing.T) {
			_, got := feed(t, AgentSpec{},
				cursorToolFrame("read", "running", argsRead, ""),
				cursorToolFrame("read", status, argsRead, resRead),
			)
			var res *ParsedEvent
			for i := range got {
				if got[i].Kind == KindToolResult {
					res = &got[i]
				}
			}
			if res == nil {
				t.Fatalf("status %q produced no result: %+v", status, got)
			}
			// Anything that is not a clean completion is a failure.
			if !res.IsError {
				t.Errorf("status %q not flagged as an error", status)
			}
		})
	}
}

// A completed frame with no preceding running frame must still yield the call,
// or the result would attach to a tool_use block that does not exist.
func TestCursorTerminalWithoutRunningStillEmitsStep(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("read", "completed", argsRead, resRead),
	)
	if len(got) != 2 || got[0].Kind != KindStep || got[1].Kind != KindToolResult {
		t.Fatalf("got %+v, want a step followed by its result", got)
	}
	if got[0].ToolUseID != got[1].ToolUseID {
		t.Errorf("step/result ids differ: %q vs %q", got[0].ToolUseID, got[1].ToolUseID)
	}
}

// A clean completion must NOT be flagged as an error.
func TestCursorCompletedIsNotAnError(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		cursorToolFrame("read", "completed", argsRead, resRead),
	)
	for _, e := range got {
		if e.Kind == KindToolResult && e.IsError {
			t.Errorf("clean completion flagged as an error: %+v", e)
		}
	}
}

// --- assistant text streaming ------------------------------------------------

func cursorAssistantFrame(text string) string {
	b, _ := json.Marshal(map[string]any{"sdkMessage": map[string]any{
		"type": "assistant",
		"message": map[string]any{
			"type": "assistant",
			"message": map[string]any{
				"role":    "assistant",
				"content": []any{map[string]any{"type": "text", "text": text}},
			},
		},
	}})
	return string(b)
}

// Cursor streams a reply as a run of assistant frames, each carrying the next
// CHUNK — not a complete message the way Claude Code does. Storing a text block
// per frame split replies mid-sentence and mid-markdown.
//
// The chunk boundaries below are VERBATIM from a real session: a fenced code
// block opened in one frame and closed in the next, so the reloaded transcript
// rendered "```bash\ngo" and "run .\n```" as two broken fragments.
func TestCursorAssistantChunksBecomeOneBlock(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorAssistantFrame("CLI Tetris is ready. Run it with:\n\n```bash\ngo"),
		cursorAssistantFrame(" run .\n```\n\n**Controls:** arrows move"),
		cursorAssistantFrame(" · Q quit\n\nTests pass."),
		fResultOK,
	)

	var blocks []string
	var deltas []string
	for _, e := range got {
		switch e.Kind {
		case KindText:
			blocks = append(blocks, e.Text)
		case KindTextDelta:
			deltas = append(deltas, e.Text)
		}
	}
	if len(blocks) != 1 {
		t.Fatalf("got %d text blocks, want 1 — a split block breaks markdown: %q", len(blocks), blocks)
	}
	want := "CLI Tetris is ready. Run it with:\n\n```bash\ngo run .\n```\n\n**Controls:** arrows move · Q quit\n\nTests pass."
	if blocks[0] != want {
		t.Errorf("block =\n%q\nwant\n%q", blocks[0], want)
	}
	// The code fence must survive reassembly, since that is what broke.
	if strings.Count(blocks[0], "```") != 2 {
		t.Errorf("code fence not intact: %q", blocks[0])
	}
	// Chunks still stream live, or the reply appears all at once at the end.
	if len(deltas) != 3 {
		t.Errorf("got %d deltas, want one per chunk", len(deltas))
	}
}

// The Result must carry the WHOLE reply. Overwriting per chunk left only the
// last fragment (" speed-ups. Tests pass (`go test ./...`).") as the turn's text.
func TestCursorResultTextIsTheWholeReply(t *testing.T) {
	turn, _ := feed(t, AgentSpec{},
		cursorAssistantFrame("The"),
		cursorAssistantFrame(" O piece has empty left columns."),
	)
	turn.flushText()
	if got := turn.lastText; got != "The O piece has empty left columns." {
		t.Errorf("lastText = %q, want the whole reply", got)
	}
}

// A tool call ends the current run of text, so a reply interrupted by tools
// stores two blocks in the right order rather than one merged blob.
func TestCursorTextFlushesAroundToolCalls(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorAssistantFrame("Reading the file"),
		cursorAssistantFrame(" first."),
		cursorToolFrame("read", "running", argsRead, ""),
		cursorToolFrame("read", "completed", argsRead, resRead),
		cursorAssistantFrame("Done."),
		fResultOK,
	)
	var kinds []ParsedKind
	var texts []string
	for _, e := range got {
		if e.Kind == KindTextDelta {
			continue // live-only
		}
		kinds = append(kinds, e.Kind)
		if e.Kind == KindText {
			texts = append(texts, e.Text)
		}
	}
	if len(texts) != 2 || texts[0] != "Reading the file first." || texts[1] != "Done." {
		t.Fatalf("texts = %q, want the run split at the tool call", texts)
	}
	// Order must match what happened: text, call, result, text, then the
	// terminal KindResult that stamps the turn duration.
	want := []ParsedKind{KindText, KindStep, KindToolResult, KindText, KindResult}
	if len(kinds) != len(want) {
		t.Fatalf("kinds = %v, want %v", kinds, want)
	}
	for i := range want {
		if kinds[i] != want[i] {
			t.Fatalf("kinds = %v, want %v", kinds, want)
		}
	}
}

// Reasoning must stream to the THINKING pane. As a text delta it would splice
// the model's private reasoning into the assistant's visible reply.
func TestCursorThinkingStreamsAsThinkingDelta(t *testing.T) {
	_, got := feed(t, AgentSpec{IncludePartials: true}, cursorThinkingFrame("weighing options", 0))
	for _, e := range got {
		if e.Kind == KindTextDelta {
			t.Errorf("reasoning leaked into the assistant reply stream: %+v", e)
		}
	}
	found := false
	for _, e := range got {
		if e.Kind == KindThinkingDelta && e.Text == "weighing options" {
			found = true
		}
	}
	if !found {
		t.Errorf("no thinking delta emitted: %+v", got)
	}
}

// Text and reasoning interleave; each closes the other so transcript order
// matches what actually happened.
func TestCursorTextAndThinkingInterleaveInOrder(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorAssistantFrame("Let me check."),
		cursorThinkingFrame("the O piece", 0),
		cursorThinkingFrame(" has empty columns", 0),
		cursorAssistantFrame("Fixing it."),
		fResultOK,
	)
	var seq []string
	for _, e := range got {
		switch e.Kind {
		case KindText:
			seq = append(seq, "text:"+e.Text)
		case KindThinking:
			seq = append(seq, "think:"+e.Text)
		}
	}
	want := []string{"text:Let me check.", "think:the O piece has empty columns", "text:Fixing it."}
	if len(seq) != len(want) {
		t.Fatalf("seq = %v, want %v", seq, want)
	}
	for i := range want {
		if seq[i] != want[i] {
			t.Fatalf("seq = %v, want %v", seq, want)
		}
	}
}

// --- todos -------------------------------------------------------------------

// Statuses captured live: Cursor sends pending | inProgress | completed, and
// only the camelCase one differs from the common vocabulary (Claude Code's
// TodoWrite sends in_progress). Without normalizing, the checklist would read
// every in-flight item as pending.
func TestCursorTodoStatusesNormalized(t *testing.T) {
	const args = `{"todos":[
		{"content":"Define CLI requirements","status":"completed"},
		{"content":"Set up scaffolding","status":"inProgress"},
		{"content":"Implement handlers","status":"pending"}
	]}`
	raw := cursorToolInput("updateTodos", json.RawMessage(args))

	var got struct {
		Todos []struct {
			Content string `json:"content"`
			Status  string `json:"status"`
		} `json:"todos"`
	}
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatalf("decode %s: %v", raw, err)
	}
	if len(got.Todos) != 3 {
		t.Fatalf("got %d todos, want 3", len(got.Todos))
	}
	want := []string{"completed", "in_progress", "pending"}
	for i, w := range want {
		if got.Todos[i].Status != w {
			t.Errorf("todo %d status = %q, want %q", i, got.Todos[i].Status, w)
		}
	}
	if got.Todos[0].Content != "Define CLI requirements" {
		t.Errorf("content lost: %q", got.Todos[0].Content)
	}
}

// An unrecognised status passes through rather than being silently read as
// pending, so a new Cursor status shows as-is instead of rendering wrong.
func TestCursorTodoUnknownStatusPassesThrough(t *testing.T) {
	raw := cursorToolInput("updateTodos", json.RawMessage(`{"todos":[{"content":"x","status":"blocked"}]}`))
	if !strings.Contains(string(raw), `"blocked"`) {
		t.Errorf("unknown status rewritten: %s", raw)
	}
}

func TestCursorTodoMalformedIsSafe(t *testing.T) {
	// A non-array todos value must not panic or be mangled.
	raw := cursorToolInput("updateTodos", json.RawMessage(`{"todos":"nope"}`))
	if !strings.Contains(string(raw), `"nope"`) {
		t.Errorf("malformed todos dropped: %s", raw)
	}
}

// The collapsed row can't show a raw todo array — it was rendering
// {"todos":[{"content":"Create tetromino pieces and board lo… as the summary.
func TestTodoRowSummaryIsReadable(t *testing.T) {
	in := map[string]any{"todos": []any{
		map[string]any{"content": "Define requirements", "status": "completed"},
		map[string]any{"content": "Set up scaffolding", "status": "in_progress"},
		map[string]any{"content": "Implement handlers", "status": "pending"},
	}}
	got := summarizeTodos(in)
	if !strings.HasPrefix(got, "1/3") {
		t.Errorf("summary = %q, want it to lead with progress", got)
	}
	if !strings.Contains(got, "Set up scaffolding") {
		t.Errorf("summary = %q, want the in-progress item", got)
	}
	if strings.Contains(got, "{") || strings.Contains(got, "\"") {
		t.Errorf("summary is still raw JSON: %q", got)
	}
}

// A list with nothing running shows progress alone rather than a stray separator.
func TestTodoRowSummaryWithoutActiveItem(t *testing.T) {
	in := map[string]any{"todos": []any{
		map[string]any{"content": "a", "status": "completed"},
		map[string]any{"content": "b", "status": "pending"},
	}}
	if got := summarizeTodos(in); got != "1/2" {
		t.Errorf("summary = %q, want 1/2", got)
	}
}

// summarizeToolInput must route TodoWrite through the readable summary, since
// that is what the feed row actually calls.
func TestSummarizeToolInputHandlesTodoWrite(t *testing.T) {
	raw := cursorToolInput("updateTodos", json.RawMessage(
		`{"todos":[{"content":"first","status":"completed"},{"content":"second","status":"inProgress"}]}`))
	got := summarizeToolInput(canonicalTool("updateTodos"), raw)
	if !strings.HasPrefix(got, "1/2") || !strings.Contains(got, "second") {
		t.Errorf("summary = %q, want progress plus the active item", got)
	}
}

// Task args from a live Cursor turn (description + prompt). The shape matches
// Claude Code's Task tool so both providers share one card.
const (
	argsTask = `{"description":"Explore provider error surfacing","prompt":"Find how provider errors are surfaced in the chat UI.","subagent_type":"explore"}`
	resTask  = `{"status":"success","value":{"text":"Errors land in the tool card banner."}}`
)

func TestCursorTaskMapsToCanonical(t *testing.T) {
	if got := canonicalTool("task"); got != "Task" {
		t.Fatalf("canonicalTool(task) = %q, want Task", got)
	}
	raw := cursorToolInput("task", json.RawMessage(argsTask))
	var got map[string]any
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if got["description"] != "Explore provider error surfacing" {
		t.Errorf("description = %v", got["description"])
	}
	if got["prompt"] == "" {
		t.Error("prompt dropped")
	}
	if got["subagent_type"] != "explore" {
		t.Errorf("subagent_type = %v", got["subagent_type"])
	}
}

func TestCursorTaskResultText(t *testing.T) {
	got := cursorToolResultText("task", json.RawMessage(resTask))
	if got != "Errors land in the tool card banner." {
		t.Errorf("result = %q", got)
	}
}

// Task meta must not dump JSON or repeat the description (that is the row title).
func TestSummarizeToolInputHandlesTask(t *testing.T) {
	raw := cursorToolInput("task", json.RawMessage(argsTask))
	got := summarizeToolInput(canonicalTool("task"), raw)
	if got != "explore" {
		t.Errorf("summary = %q, want subagent_type", got)
	}
	if strings.Contains(got, "{") || strings.Contains(got, "Explore provider") {
		t.Errorf("summary leaked JSON or description: %q", got)
	}

	// No type → short prompt preview.
	raw2 := cursorToolInput("task", json.RawMessage(
		`{"description":"Check diffs","prompt":"Review the open PR diff for regressions in tool cards."}`))
	got2 := summarizeToolInput("Task", raw2)
	if got2 == "" || strings.Contains(got2, "{") || strings.Contains(got2, "Check diffs") {
		t.Errorf("prompt-only summary = %q", got2)
	}
}

// Claude Code TaskCreate's description is the task BODY. The feed/chat header
// must use the subject, never that paragraph and never the raw JSON envelope.
func TestSummarizeToolInputHandlesTaskCreate(t *testing.T) {
	raw := json.RawMessage(`{"subject":"Guard theme re-apply (registry + host)","description":"setActiveTheme memoize; applyColorMap no-op on identical maps.","activeForm":"Guarding theme re-apply"}`)
	got := summarizeToolInput("TaskCreate", raw)
	if got != "Guard theme re-apply (registry + host)" {
		t.Errorf("TaskCreate summary = %q, want the subject", got)
	}
	if strings.Contains(got, "{") || strings.Contains(got, "setActiveTheme") || strings.Contains(got, "activeForm") {
		t.Errorf("TaskCreate summary leaked JSON or body: %q", got)
	}
	if desc := toolDescription("TaskCreate", raw); desc != "" {
		t.Errorf("toolDescription(TaskCreate) = %q, want empty (body is not a row title)", desc)
	}
}

func TestSummarizeToolInputHandlesTaskUpdate(t *testing.T) {
	got := summarizeToolInput("TaskUpdate", json.RawMessage(`{"taskId":"1","status":"in_progress"}`))
	if got != "#1 → in_progress" {
		t.Errorf("TaskUpdate summary = %q, want #1 → in_progress", got)
	}
	if strings.Contains(got, "{") || strings.Contains(got, "taskId") {
		t.Errorf("TaskUpdate summary leaked JSON: %q", got)
	}
}

func TestCreatePlanBodyAndMapping(t *testing.T) {
	if got := createPlanBody(json.RawMessage(argsPlan)); !strings.Contains(got, "touch work.txt") {
		t.Errorf("createPlanBody = %q, want the plan markdown", got)
	}
	if got := createPlanBody(json.RawMessage(argsPlanEmpty)); got != "" {
		t.Errorf("empty plan body = %q, want empty", got)
	}
	if !isCreatePlanTool("createPlan") || !isCreatePlanTool("CreatePlan") {
		t.Error("isCreatePlanTool should accept both spellings")
	}
	if isCreatePlanTool("updateTodos") {
		t.Error("isCreatePlanTool must not match unrelated tools")
	}

	raw := cursorToolInput("createPlan", json.RawMessage(argsPlan))
	var got map[string]any
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if _, ok := got["plan"].(string); !ok || got["plan"] == "" {
		t.Errorf("canonical createPlan input missing plan: %s", raw)
	}
	if cursorToolResultText("createPlan", json.RawMessage(resPlan)) != "" {
		t.Error("createPlan result should be an empty ack for display")
	}
	sum := summarizeToolInput(canonicalTool("createPlan"), raw)
	if !strings.Contains(sum, "Summary:") || strings.Contains(sum, "\n") {
		t.Errorf("CreatePlan summary = %q, want first line only", sum)
	}
}

// submit_plan / update_plan_step feed meta must be human-readable — never the
// raw JSON dump that used to dominate chat tool-card headers.
func TestSummarizeToolInputHandlesSubmitAndUpdatePlan(t *testing.T) {
	sub := summarizeToolInput("mcp__recur__submit_plan", json.RawMessage(
		`{"summary":"Polish plan tool cards","steps":["Add helpers","Render checklist"]}`))
	if sub != "Polish plan tool cards" {
		t.Errorf("submit_plan summary = %q, want the plan summary only", sub)
	}
	if strings.Contains(sub, "{") || strings.Contains(sub, "steps") {
		t.Errorf("submit_plan summary leaked JSON: %q", sub)
	}

	upd := summarizeToolInput("mcp__recur__update_plan_step", json.RawMessage(
		`{"step_id":"S2","status":"done","note":"wired"}`))
	if upd != "S2 → done · wired" {
		t.Errorf("update_plan_step summary = %q, want S2 → done · wired", upd)
	}
	if strings.Contains(upd, "{") || strings.Contains(upd, "step_id") {
		t.Errorf("update_plan_step summary leaked JSON keys: %q", upd)
	}

	bare := summarizeToolInput("mcp__recur__update_plan_step", json.RawMessage(
		`{"step_id":"S1","status":"doing"}`))
	if bare != "S1 → doing" {
		t.Errorf("update_plan_step summary = %q, want S1 → doing", bare)
	}
}

func TestSummarizeToolInputEditPlan(t *testing.T) {
	plan := summarizeToolInput("mcp__recur__edit_plan", json.RawMessage(
		`{"plan":"Add teal paddles.\nKeep scores white."}`))
	if plan != "Add teal paddles." {
		t.Errorf("edit_plan summary = %q, want first line only", plan)
	}
	if strings.Contains(plan, "{") || strings.Contains(plan, `"plan"`) {
		t.Errorf("edit_plan summary leaked JSON: %q", plan)
	}
}

func TestSummarizeToolInputAskUserUsesQuestion(t *testing.T) {
	got := summarizeToolInput("mcp__recur__ask_user", json.RawMessage(
		`{"question":"Which color?","context":"follow-up","options":[{"text":"Blue"}]}`))
	if got != "Which color?" {
		t.Errorf("ask_user summary = %q, want the question only", got)
	}
	if strings.Contains(got, "{") || strings.Contains(got, "options") {
		t.Errorf("ask_user summary leaked JSON: %q", got)
	}
}

func TestSummarizeToolInputSwitchModeUsesMode(t *testing.T) {
	got := summarizeToolInput("mcp__recur__switch_mode", json.RawMessage(`{"mode":"plan"}`))
	if got != "plan" {
		t.Errorf("switch_mode summary = %q, want plan", got)
	}
}

func TestSummarizeToolInputOpenPRUsesTitle(t *testing.T) {
	got := summarizeToolInput("mcp__recur__open_pr", json.RawMessage(
		`{"title":"Align screening allow cache TTL to 24 hours","body":"## Summary\nSet ALLOW_TTL.","draft":true}`))
	if got != "draft · Align screening allow cache TTL to 24 hours" {
		t.Errorf("open_pr summary = %q, want draft · title", got)
	}
	if strings.Contains(got, "{") || strings.Contains(got, "Summary") || strings.Contains(got, "\\n") {
		t.Errorf("open_pr summary leaked JSON or body: %q", got)
	}
}

func TestSummarizeToolInputWebFetchUsesURL(t *testing.T) {
	const url = "https://docs.cdp.coinbase.com/api-reference/rest-api/onramp-offramp/create-session-token"
	got := summarizeToolInput("WebFetch", json.RawMessage(
		`{"url":"`+url+`","prompt":"List the request body fields. Is clientIp required?"}`))
	if got != url {
		t.Errorf("WebFetch summary = %q, want url", got)
	}
	if strings.Contains(got, "{") || strings.Contains(got, "clientIp") || strings.Contains(got, "prompt") {
		t.Errorf("WebFetch summary leaked JSON or prompt: %q", got)
	}
}

func TestSummarizeToolInputWebSearchUsesQuery(t *testing.T) {
	got := summarizeToolInput("WebSearch", json.RawMessage(`{"query":"onramp session token clientIp"}`))
	if got != "onramp session token clientIp" {
		t.Errorf("WebSearch summary = %q, want query", got)
	}
	got = summarizeToolInput("WebSearch", json.RawMessage(`{"search_term":"partnerUserRef"}`))
	if got != "partnerUserRef" {
		t.Errorf("WebSearch search_term summary = %q, want search_term", got)
	}
}

func TestSummarizeToolInputLinearUsesQuery(t *testing.T) {
	got := summarizeToolInput("mcp__linear__list_projects", json.RawMessage(`{"query":"Apollo"}`))
	if got != "Apollo" {
		t.Errorf("list_projects summary = %q, want Apollo", got)
	}
	if strings.Contains(got, "{") {
		t.Errorf("list_projects summary leaked JSON: %q", got)
	}
	team := summarizeToolInput("mcp__linear__list_issue_labels", json.RawMessage(`{"team":"ACME"}`))
	if team != "ACME" {
		t.Errorf("list_issue_labels summary = %q, want ACME", team)
	}
	empty := summarizeToolInput("mcp__plugin_linear_linear__list_teams", json.RawMessage(`{}`))
	if empty != "" {
		t.Errorf("empty linear args summary = %q, want blank", empty)
	}
}

// A thorough plan-mode brief routinely exceeds chatDetailLimit. Byte-slicing the
// JSON mid-string broke the frontend parser and the CreatePlan card rendered
// nothing; chatToolInput must keep a parseable {"plan":…} envelope.
func TestChatToolInputKeepsCreatePlanParseable(t *testing.T) {
	body := strings.Repeat("plan step — with detail. ", 900) // > chatDetailLimit as JSON
	raw, err := json.Marshal(map[string]string{"plan": body})
	if err != nil {
		t.Fatal(err)
	}
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}

	// The old path: truncate mid-JSON → unparseable → empty card.
	if json.Valid([]byte(truncate(string(raw), chatDetailLimit))) {
		t.Fatal("expected mid-JSON truncate to be invalid; fixture may need adjusting")
	}

	got := chatToolInput("CreatePlan", raw)
	if !json.Valid([]byte(got)) {
		t.Fatalf("chatToolInput produced invalid JSON (%d bytes)", len(got))
	}
	var out struct {
		Plan string `json:"plan"`
	}
	if err := json.Unmarshal([]byte(got), &out); err != nil || out.Plan == "" {
		t.Fatalf("chatToolInput lost the plan body: %v %q", err, got[:min(80, len(got))])
	}
	if !strings.Contains(out.Plan, "plan step") {
		t.Errorf("plan body missing expected content")
	}
	// Other tools still get the plain byte bound.
	if got := chatToolInput("Bash", json.RawMessage(argsShell)); got != argsShell {
		t.Errorf("small Bash input rewritten: %q", got)
	}
}

func TestDisplayToolResultHidesSubagentLaunchMeta(t *testing.T) {
	ack := "Async agent launched successfully.\n(This tool result is internal metadata – never quote or paste any part of it, including the agentId below, into a user-facing reply.)\n\nagentId: abc\n"
	if got := displayToolResult(domain.ProviderClaudeCode, "Task", ack); got != "" {
		t.Errorf("Claude Task launch ack should be hidden, got %q", got)
	}
	if got := displayToolResult(domain.ProviderClaudeCode, "Task", "tabs live in PRDetail.svelte"); got != "tabs live in PRDetail.svelte" {
		t.Errorf("real Task result hidden: %q", got)
	}
	mentioned := "isSubagentLaunchMeta reports Async agent launched successfully plus agentId and internal metadata"
	if got := displayToolResult(domain.ProviderClaudeCode, "Read", mentioned); got != mentioned {
		t.Errorf("Read of a file mentioning launch metadata was blanked: %q", got)
	}
	if got := chatToolResult("Read", mentioned); got != mentioned {
		t.Errorf("chatToolResult blanked a Read mentioning launch metadata: %q", got)
	}
}

func TestChatToolInputKeepsTaskParseable(t *testing.T) {
	prompt := strings.Repeat("Search the Svelte frontend for PR detail. ", 600)
	raw, err := json.Marshal(map[string]string{
		"description":   "Map PR detail UI structure",
		"subagent_type": "Explore",
		"prompt":        prompt,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}
	if json.Valid([]byte(truncate(string(raw), chatDetailLimit))) {
		t.Fatal("expected mid-JSON truncate to be invalid; fixture may need adjusting")
	}

	got := chatToolInput("Task", raw)
	if !json.Valid([]byte(got)) {
		t.Fatalf("chatToolInput produced invalid JSON (%d bytes)", len(got))
	}
	var out struct {
		Description  string `json:"description"`
		SubagentType string `json:"subagent_type"`
		Prompt       string `json:"prompt"`
	}
	if err := json.Unmarshal([]byte(got), &out); err != nil {
		t.Fatalf("chatToolInput lost the Task envelope: %v %q", err, got[:min(80, len(got))])
	}
	if out.Description != "Map PR detail UI structure" || out.SubagentType != "Explore" {
		t.Errorf("Task meta: %+v", out)
	}
	if !strings.Contains(out.Prompt, "Svelte frontend") {
		t.Errorf("prompt missing expected content")
	}
}

// A list_findings payload with long messages routinely exceeds chatDetailLimit.
// Byte-slicing the JSON mid-string broke the frontend parser and the card dumped
// the minified blob; chatToolResult must keep a parseable {status,count,findings}.
func TestChatToolResultKeepsListFindingsParseable(t *testing.T) {
	type row struct {
		ID       string `json:"id"`
		File     string `json:"file"`
		Line     int    `json:"line"`
		Severity string `json:"severity"`
		Message  string `json:"message"`
		Status   string `json:"status"`
	}
	findings := make([]row, 0, 15)
	for i := 0; i < 15; i++ {
		findings = append(findings, row{
			ID:       fmt.Sprintf("F-%02d", i),
			File:     "docs/bundles.md",
			Line:     294 + i,
			Severity: "high",
			Message:  strings.Repeat("Deadlock between qa, verify, and rail #1. ", 40),
			Status:   "open",
		})
	}
	raw, err := json.Marshal(map[string]any{
		"status":   "all",
		"count":    len(findings),
		"findings": findings,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}
	if json.Valid([]byte(truncate(string(raw), chatDetailLimit))) {
		t.Fatal("expected mid-JSON truncate to be invalid; fixture may need adjusting")
	}

	got := chatToolResult("mcp__recur__list_findings", string(raw))
	if !json.Valid([]byte(got)) {
		t.Fatalf("chatToolResult produced invalid JSON (%d bytes)", len(got))
	}
	if len(got) > chatDetailLimit {
		t.Fatalf("chatToolResult still over budget (%d > %d)", len(got), chatDetailLimit)
	}
	var out struct {
		Status   string `json:"status"`
		Count    int    `json:"count"`
		Findings []row  `json:"findings"`
	}
	if err := json.Unmarshal([]byte(got), &out); err != nil {
		t.Fatalf("chatToolResult lost the findings envelope: %v", err)
	}
	if out.Status != "all" || out.Count != 15 {
		t.Errorf("envelope status/count = %q/%d, want all/15", out.Status, out.Count)
	}
	if len(out.Findings) == 0 {
		t.Fatal("chatToolResult dropped every finding")
	}
	if out.Findings[0].ID == "" || out.Findings[0].File == "" {
		t.Errorf("first finding missing id/file: %+v", out.Findings[0])
	}
	// Other tools still get the plain byte bound.
	if got := chatToolResult("Bash", "ok"); got != "ok" {
		t.Errorf("small Bash result rewritten: %q", got)
	}
}

// A long list_artifacts payload exceeds chatDetailLimit the same way findings
// do. Byte-slicing mid-JSON would dump the blob; keep a parseable {count,artifacts}.
func TestChatToolResultKeepsListArtifactsParseable(t *testing.T) {
	type row struct {
		ID          string `json:"id"`
		WorkspaceID string `json:"workspaceId"`
		Title       string `json:"title"`
		Kind        string `json:"kind"`
		Path        string `json:"path"`
		CreatedAt   string `json:"createdAt"`
		CreatedBy   string `json:"createdBy"`
	}
	artifacts := make([]row, 0, 80)
	for i := 0; i < 80; i++ {
		id := fmt.Sprintf("aaaaaaaa-bbbb-4ccc-8ddd-%012d", i)
		artifacts = append(artifacts, row{
			ID:          id,
			WorkspaceID: "recur.bubbly-parfait",
			Title:       fmt.Sprintf("Modes as bundle items %02d — long agent title", i),
			Kind:        "html",
			Path:        "workspaces/recur.bubbly-parfait/artifacts/" + id + ".html",
			CreatedAt:   "2026-08-20T04:32:46Z",
			CreatedBy:   "PA",
		})
	}
	raw, err := json.Marshal(map[string]any{
		"count":     len(artifacts),
		"artifacts": artifacts,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}
	if json.Valid([]byte(truncate(string(raw), chatDetailLimit))) {
		t.Fatal("expected mid-JSON truncate to be invalid; fixture may need adjusting")
	}

	got := chatToolResult("mcp__recur__list_artifacts", string(raw))
	if !json.Valid([]byte(got)) {
		t.Fatalf("chatToolResult produced invalid JSON (%d bytes)", len(got))
	}
	if len(got) > chatDetailLimit {
		t.Fatalf("chatToolResult still over budget (%d > %d)", len(got), chatDetailLimit)
	}
	var out struct {
		Count     int   `json:"count"`
		Artifacts []row `json:"artifacts"`
	}
	if err := json.Unmarshal([]byte(got), &out); err != nil {
		t.Fatalf("chatToolResult lost the artifacts envelope: %v", err)
	}
	if out.Count != 80 {
		t.Errorf("chatToolResult count = %d, want 80", out.Count)
	}
	if len(out.Artifacts) == 0 {
		t.Fatal("chatToolResult dropped every artifact")
	}
	if out.Artifacts[0].ID == "" || out.Artifacts[0].Path == "" {
		t.Errorf("first artifact missing id/path: %+v", out.Artifacts[0])
	}
}

func TestCompactListArtifactsRejectsNonEnvelope(t *testing.T) {
	blob := `{"error":"` + strings.Repeat("x", chatDetailLimit) + `"}`
	got, ok := compactListArtifactsResult(blob)
	if ok {
		t.Fatalf("compactListArtifactsResult(non-envelope) = %q, want ok=false", got)
	}
	got, ok = compactListArtifactsResult(`{"count":0,"artifacts":[]}`)
	if !ok || !strings.Contains(got, `"artifacts"`) {
		t.Errorf("compactListArtifactsResult(empty list) = %q ok=%t, want the empty envelope", got, ok)
	}
}

func TestChatToolResultKeepsListWorkspaceBriefsParseable(t *testing.T) {
	workspaces := make([]domain.WorkspaceBrief, 0, 40)
	for i := 0; i < 40; i++ {
		workspaces = append(workspaces, domain.WorkspaceBrief{
			WorkspaceID: fmt.Sprintf("recur.workspace-%02d", i),
			ProjectName: "recur",
			Title:       fmt.Sprintf("Account linking architecture %02d", i),
			Branch:      "mycodecrafting/multi-account-linking",
			Bucket:      domain.BriefBucketPickup,
			Reasons:     []string{domain.BriefReasonUnfinishedPlan, domain.BriefReasonRecentChat},
			Now:         "Plan is ready but not approved",
			Next:        "Review and approve the plan",
			PlanSummary: strings.Repeat("Identity per provider, not per account. ", 80),
			Primary:     domain.BriefAction{Label: "Review plan", Target: fmt.Sprintf("workspace:recur.workspace-%02d", i)},
		})
	}
	raw, err := json.Marshal(domain.BriefingSnapshot{
		Counts:     domain.BriefingCounts{Needs: 3, Pickup: 40},
		Workspaces: workspaces,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}
	if json.Valid([]byte(truncate(string(raw), chatDetailLimit))) {
		t.Fatal("expected mid-JSON truncate to be invalid; fixture may need adjusting")
	}

	got := chatToolResult("mcp__recur__list_workspace_briefs", string(raw))
	if !json.Valid([]byte(got)) {
		t.Fatalf("chatToolResult produced invalid JSON (%d bytes)", len(got))
	}
	if len(got) > chatDetailLimit {
		t.Fatalf("chatToolResult still over budget (%d > %d)", len(got), chatDetailLimit)
	}
	var out domain.BriefingSnapshot
	if err := json.Unmarshal([]byte(got), &out); err != nil {
		t.Fatalf("chatToolResult lost the briefing envelope: %v", err)
	}
	if out.Counts.Pickup != 40 {
		t.Errorf("chatToolResult counts.pickup = %d, want 40", out.Counts.Pickup)
	}
	if len(out.Workspaces) == 0 {
		t.Fatal("chatToolResult dropped every workspace")
	}
	if out.Workspaces[0].WorkspaceID == "" || out.Workspaces[0].Title == "" {
		t.Errorf("first brief missing id/title: %+v", out.Workspaces[0])
	}
}

func TestCompactListWorkspaceBriefsRejectsNonEnvelope(t *testing.T) {
	blob := `{"error":"` + strings.Repeat("x", chatDetailLimit) + `"}`
	got, ok := compactListWorkspaceBriefsResult(blob)
	if ok {
		t.Fatalf("compactListWorkspaceBriefsResult(non-envelope) = %q, want ok=false", got)
	}
	got, ok = compactListWorkspaceBriefsResult(`{"counts":{"needs":0,"working":0,"pickup":0},"workspaces":[]}`)
	if !ok || !strings.Contains(got, `"workspaces"`) {
		t.Errorf("compactListWorkspaceBriefsResult(empty list) = %q ok=%t, want the empty envelope", got, ok)
	}
}

func TestChatToolResultKeepsWorkspaceContextParseable(t *testing.T) {
	ctx := domain.WorkspaceContext{
		Brief: domain.WorkspaceBrief{
			WorkspaceID: "recur.zesty-quiche",
			Title:       "Account linking architecture",
			Bucket:      domain.BriefBucketNeeds,
			Now:         "Plan is ready but not approved",
			Next:        "Review and approve the plan",
			PlanSummary: strings.Repeat("Identity per provider. ", 900),
			Primary:     domain.BriefAction{Label: "Review plan", Target: "workspace:recur.zesty-quiche"},
		},
		Conversation: []domain.ConversationLine{{
			Role: "assistant",
			Text: strings.Repeat("plan step with detail. ", 400),
		}},
	}
	raw, err := json.Marshal(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}

	got := chatToolResult("mcp__recur__get_workspace_context", string(raw))
	if !json.Valid([]byte(got)) {
		t.Fatalf("chatToolResult produced invalid JSON (%d bytes)", len(got))
	}
	if len(got) > chatDetailLimit {
		t.Fatalf("chatToolResult still over budget (%d > %d)", len(got), chatDetailLimit)
	}
	var out domain.WorkspaceContext
	if err := json.Unmarshal([]byte(got), &out); err != nil {
		t.Fatalf("chatToolResult lost the context envelope: %v", err)
	}
	if out.Brief.WorkspaceID != "recur.zesty-quiche" || out.Brief.Title == "" {
		t.Errorf("brief missing: %+v", out.Brief)
	}
}

func TestCompactListArtifactsKeepsRowWhenTitleIsHuge(t *testing.T) {
	// A single title larger than chatDetailLimit used to drop the only row
	// while count stayed 1, so the card said "No artifacts yet."
	type row struct {
		ID    string `json:"id"`
		Title string `json:"title"`
		Kind  string `json:"kind"`
		Path  string `json:"path"`
	}
	raw, err := json.Marshal(map[string]any{
		"count": 1,
		"artifacts": []row{{
			ID:    "aaaaaaaa-bbbb-4ccc-8ddd-000000000001",
			Title: strings.Repeat("Modes as bundle items — ", 900),
			Kind:  "html",
			Path:  "workspaces/recur.bubbly-parfait/artifacts/aaaaaaaa-bbbb-4ccc-8ddd-000000000001.html",
		}},
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}
	got, ok := compactListArtifactsResult(string(raw))
	if !ok {
		t.Fatal("compactListArtifactsResult(huge title) ok=false")
	}
	if len(got) > chatDetailLimit {
		t.Fatalf("compactListArtifactsResult(huge title) = %d bytes, want <= %d", len(got), chatDetailLimit)
	}
	var out struct {
		Count     int   `json:"count"`
		Artifacts []row `json:"artifacts"`
	}
	if err := json.Unmarshal([]byte(got), &out); err != nil {
		t.Fatalf("compactListArtifactsResult(huge title) invalid JSON: %v", err)
	}
	if out.Count != 1 {
		t.Errorf("compactListArtifactsResult(huge title) count = %d, want 1", out.Count)
	}
	if len(out.Artifacts) != 1 {
		t.Fatalf("compactListArtifactsResult(huge title) kept %d rows, want 1", len(out.Artifacts))
	}
	if out.Artifacts[0].ID == "" || out.Artifacts[0].Title == "" {
		t.Errorf("compactListArtifactsResult(huge title) first row missing id/title: %+v", out.Artifacts[0])
	}
	if len(out.Artifacts[0].Title) >= len(strings.Repeat("Modes as bundle items — ", 900)) {
		t.Errorf("compactListArtifactsResult(huge title) did not shrink title (%d bytes)", len(out.Artifacts[0].Title))
	}
}

func largeListFindingsJSON(t *testing.T, n int, extra map[string]any) string {
	t.Helper()
	type row struct {
		ID           string `json:"id"`
		File         string `json:"file"`
		Line         int    `json:"line"`
		Severity     string `json:"severity"`
		Message      string `json:"message"`
		Summary      string `json:"summary,omitempty"`
		Status       string `json:"status"`
		StatusReason string `json:"statusReason,omitempty"`
		Notes        []any  `json:"notes,omitempty"`
	}
	findings := make([]row, 0, n)
	for i := 0; i < n; i++ {
		r := row{
			ID:       fmt.Sprintf("F-%02d", i),
			File:     "docs/bundles.md",
			Line:     294 + i,
			Severity: "high",
			Message:  strings.Repeat("Deadlock between qa, verify, and rail #1. ", 40),
			Status:   "open",
		}
		if extra != nil {
			if s, ok := extra["summary"].(string); ok {
				r.Summary = s
			}
			if s, ok := extra["statusReason"].(string); ok {
				r.StatusReason = s
			}
			if notes, ok := extra["notes"].([]any); ok {
				r.Notes = notes
			}
		}
		findings = append(findings, r)
	}
	raw, err := json.Marshal(map[string]any{
		"status":   "all",
		"count":    len(findings),
		"findings": findings,
	})
	if err != nil {
		t.Fatal(err)
	}
	return string(raw)
}

// Claude Code's KindToolResult has an empty Tool. The live card must still
// compact using the display name recorded on KindStep.
func TestSessionBroadcastCompactsListFindingsWhenResultHasNoToolName(t *testing.T) {
	bus := NewBus()
	var result string
	bus.Subscribe(func(ev Event) {
		if ev.Name != "session.tool_result" {
			return
		}
		d, _ := ev.Data.(map[string]any)
		result, _ = d["result"].(string)
	})
	b := &sessionBroadcast{e: &Engine{Bus: bus}, wsID: "ws-1", code: "PA"}

	raw := largeListFindingsJSON(t, 15, nil)
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}

	b.emit(ParsedEvent{
		Kind: KindStep, Tool: "mcp__recur__list_findings", ToolUseID: "t1",
		ToolInputRaw: json.RawMessage(`{"status":"all"}`),
	})
	// Claude-style result: Tool unset, Text is the findings JSON.
	b.emit(ParsedEvent{Kind: KindToolResult, ToolUseID: "t1", Text: raw})

	if !json.Valid([]byte(result)) {
		t.Fatalf("chatToolResult(live empty Tool) produced invalid JSON (%d bytes)", len(result))
	}
	if len(result) > chatDetailLimit {
		t.Fatalf("chatToolResult(live empty Tool) still over budget (%d > %d)", len(result), chatDetailLimit)
	}
	var out struct {
		Status   string `json:"status"`
		Count    int    `json:"count"`
		Findings []any  `json:"findings"`
	}
	if err := json.Unmarshal([]byte(result), &out); err != nil {
		t.Fatalf("chatToolResult(live empty Tool) lost the findings envelope: %v", err)
	}
	if out.Status != "all" || out.Count != 15 || len(out.Findings) == 0 {
		t.Errorf("chatToolResult(live empty Tool) = %+v, want all/15 with rows", out)
	}
}

func TestCompactListFindingsRejectsNonEnvelope(t *testing.T) {
	// Oversized valid JSON that is not the list_findings shape must not become
	// {"status":"","count":0,"findings":[]} — that made the card say "No findings."
	blob := `{"error":"` + strings.Repeat("x", chatDetailLimit) + `"}`
	got, ok := compactListFindingsResult(blob)
	if ok {
		t.Fatalf("compactListFindingsResult(non-envelope) = %q, want ok=false", got)
	}
	got, ok = compactListFindingsResult(`{"status":"open","count":0,"findings":[]}`)
	if !ok || !strings.Contains(got, `"findings"`) {
		t.Errorf("compactListFindingsResult(empty list) = %q ok=%t, want the empty envelope", got, ok)
	}
}

func TestCompactListFindingsShrinkIsMonotonic(t *testing.T) {
	// Long summaries used to replace Message at budget 0 and grow the payload,
	// so the loop dropped most rows. Shrinking must stay at-or-under the previous
	// step and keep more than a handful of findings.
	summary := strings.Repeat("summary word ", 200) // ~2.4KB
	raw := largeListFindingsJSON(t, 40, map[string]any{"summary": summary})
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}
	got, ok := compactListFindingsResult(raw)
	if !ok {
		t.Fatal("compactListFindingsResult(long summaries) ok=false")
	}
	if len(got) > chatDetailLimit {
		t.Fatalf("compactListFindingsResult(long summaries) = %d bytes, want <= %d", len(got), chatDetailLimit)
	}
	var out struct {
		Count    int `json:"count"`
		Findings []struct {
			Message string `json:"message"`
			Summary string `json:"summary"`
		} `json:"findings"`
	}
	if err := json.Unmarshal([]byte(got), &out); err != nil {
		t.Fatalf("compactListFindingsResult(long summaries) invalid JSON: %v", err)
	}
	if out.Count != 40 {
		t.Errorf("compactListFindingsResult(long summaries) count = %d, want 40", out.Count)
	}
	if n := len(out.Findings); n < 20 {
		t.Errorf("compactListFindingsResult(long summaries) kept %d rows, want at least 20", n)
	}
	for i, f := range out.Findings {
		if len(f.Message) > 800+len("…") || len(f.Summary) > 800+len("…") {
			t.Errorf("finding[%d] texts exceeded budget: msg=%d summary=%d", i, len(f.Message), len(f.Summary))
			break
		}
	}
}

func TestCompactListFindingsKeepsNotesWhenTheyFit(t *testing.T) {
	notes := []any{map[string]any{"id": "n1", "author": "human", "body": "please fix the rail"}}
	// Over the card budget so we re-serialize; short notes must survive the 800-byte step.
	raw := largeListFindingsJSON(t, 15, map[string]any{"notes": notes})
	if len(raw) <= chatDetailLimit {
		t.Fatalf("fixture too small (%d); need > chatDetailLimit (%d)", len(raw), chatDetailLimit)
	}
	got, ok := compactListFindingsResult(raw)
	if !ok {
		t.Fatal("compactListFindingsResult(notes) ok=false")
	}
	var out struct {
		Findings []struct {
			Notes []struct {
				Body string `json:"body"`
			} `json:"notes"`
		} `json:"findings"`
	}
	if err := json.Unmarshal([]byte(got), &out); err != nil {
		t.Fatalf("compactListFindingsResult(notes) invalid JSON: %v", err)
	}
	if len(out.Findings) == 0 || len(out.Findings[0].Notes) == 0 || out.Findings[0].Notes[0].Body != "please fix the rail" {
		t.Errorf("compactListFindingsResult(notes) dropped notes: %+v", out.Findings)
	}
}

// Cursor streams createPlan across running frames (empty → partial → full on
// completed). KindStep freezes args on first emit, so stepping on the first
// non-empty running frame permanently captured a stub. We wait for terminal
// and keep the richest body seen across frames.
func TestCursorCreatePlanWaitsForTerminalArgs(t *testing.T) {
	partial := `{"plan":"Summary: only the opening line"}`
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("createPlan", "running", argsPlanEmpty, ""),
		cursorToolFrame("createPlan", "running", partial, ""),
		cursorToolFrame("createPlan", "completed", argsPlan, resPlan),
	)
	var steps []ParsedEvent
	for _, e := range got {
		if e.Kind == KindStep {
			steps = append(steps, e)
		}
	}
	if len(steps) != 1 {
		t.Fatalf("got %d KindSteps, want exactly 1: %+v", len(steps), got)
	}
	body := createPlanBody(steps[0].ToolInputRaw)
	if !strings.Contains(body, "touch work.txt") {
		t.Errorf("step args = %s, want the completed plan body", steps[0].ToolInputRaw)
	}
	if strings.Contains(body, "only the opening line") && !strings.Contains(body, "Definition of done") {
		t.Errorf("step froze the partial running args: %s", steps[0].ToolInputRaw)
	}
	if d := displayToolFor(steps[0]); d.Name != "CreatePlan" {
		t.Errorf("display name = %q, want CreatePlan", d.Name)
	}
}

// If the terminal frame somehow carries a shorter body than an earlier running
// frame, keep the richest args — length is the only signal we have.
func TestCursorCreatePlanKeepsRichestArgs(t *testing.T) {
	full := argsPlan
	short := `{"plan":"Summary: stub"}`
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("createPlan", "running", full, ""),
		cursorToolFrame("createPlan", "completed", short, resPlan),
	)
	var step *ParsedEvent
	for i := range got {
		if got[i].Kind == KindStep {
			step = &got[i]
			break
		}
	}
	if step == nil {
		t.Fatal("no KindStep")
	}
	if body := createPlanBody(step.ToolInputRaw); !strings.Contains(body, "touch work.txt") {
		t.Errorf("richest args lost: %s", step.ToolInputRaw)
	}
}

// --- calls that never report a result ----------------------------------------

// Cursor STOPS sending frames for a tool call that fails: a read of a missing
// file emits `running` and nothing else, and the run still finishes
// successfully. The failure appears only in the model's prose, never
// machine-readably.
//
// Left unresolved, the transcript stores a tool_use with no tool_result, which
// renders as a bare card dumping {"file_path": …} — indistinguishable from a
// tool that returned nothing. That is the "some reads render as just a file
// path" symptom.
func TestCursorUnreportedCallIsClosedAsError(t *testing.T) {
	turn, _ := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		fResultOK, // run finishes cleanly; the read never reported
	)
	var res []ParsedEvent
	for _, e := range turn.emitted() {
		if e.Kind == KindToolResult {
			res = append(res, e)
		}
	}
	if len(res) != 1 {
		t.Fatalf("got %d results, want the unreported call closed: %+v", len(res), turn.emitted())
	}
	if !res[0].IsError {
		t.Error("an unreported call must be marked failed, not a silent success")
	}
	if res[0].ToolUseID != "c1" {
		t.Errorf("result id = %q, want the originating call", res[0].ToolUseID)
	}
	if res[0].Text == "" {
		t.Error("result text is empty — the card would still fall back to the input dump")
	}
}

// The synthetic result must land BEFORE the closing reply, since the calls
// happened first.
func TestCursorUnreportedCallClosesBeforeFinalText(t *testing.T) {
	turn, _ := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		cursorAssistantFrame("Those files do not exist."),
		fResultOK,
	)
	var order []ParsedKind
	for _, e := range turn.emitted() {
		if e.Kind == KindToolResult || e.Kind == KindText {
			order = append(order, e.Kind)
		}
	}
	want := []ParsedKind{KindToolResult, KindText}
	if len(order) != len(want) || order[0] != want[0] || order[1] != want[1] {
		t.Errorf("order = %v, want the result before the closing text", order)
	}
}

// A call that DID report must not be closed twice.
func TestCursorResolvedCallIsNotDoubleClosed(t *testing.T) {
	turn, _ := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		cursorToolFrame("read", "completed", argsRead, resRead),
		fResultOK,
	)
	var n int
	for _, e := range turn.emitted() {
		if e.Kind == KindToolResult {
			n++
		}
	}
	if n != 1 {
		t.Errorf("got %d results for one call, want 1", n)
	}
}

// Several unreported calls each get closed, in call order.
func TestCursorAllUnreportedCallsAreClosed(t *testing.T) {
	frame := func(id string) string {
		b, _ := json.Marshal(map[string]any{"sdkMessage": map[string]any{
			"type": "tool_call",
			"message": map[string]any{
				"type": "tool_call", "call_id": id, "name": "read",
				"status": "running", "args": json.RawMessage(argsRead),
			},
		}})
		return string(b)
	}
	turn, _ := feed(t, AgentSpec{}, frame("a"), frame("b"), frame("c"), fResultOK)

	var ids []string
	for _, e := range turn.emitted() {
		if e.Kind == KindToolResult {
			ids = append(ids, e.ToolUseID)
		}
	}
	want := []string{"a", "b", "c"}
	if len(ids) != len(want) {
		t.Fatalf("closed %v, want %v", ids, want)
	}
	for i := range want {
		if ids[i] != want[i] {
			t.Fatalf("closed %v, want %v (call order)", ids, want)
		}
	}
}

// A stream that dies without a result envelope still closes its open calls.
func TestCursorUnreportedCallClosedOnFinish(t *testing.T) {
	turn, _ := feed(t, AgentSpec{}, cursorToolFrame("read", "running", argsRead, ""))
	turn.finish()
	var n int
	for _, e := range turn.emitted() {
		if e.Kind == KindToolResult && e.IsError {
			n++
		}
	}
	if n != 1 {
		t.Errorf("got %d closed calls after finish, want 1", n)
	}
}

// --- error status and truncation (SDKToolUseMessage) --------------------------

// The SDK documents status "running" | "completed" | "error". "error" is handled
// as a terminal failure — though note it is NOT reliably sent: a read of a
// missing file emits "running" and then nothing at all, which is why
// closeOpenCalls exists as the backstop.
func TestCursorErrorStatusIsTerminalFailure(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		cursorToolFrame("read", "error", argsRead, `{"status":"error","value":{"message":"ENOENT"}}`),
	)
	var res *ParsedEvent
	for i := range got {
		if got[i].Kind == KindToolResult {
			res = &got[i]
		}
	}
	if res == nil {
		t.Fatalf("no result for an error status: %+v", got)
	}
	if !res.IsError {
		t.Error("status error must be flagged as a failure")
	}
	// The provider's envelope is stored verbatim, so the reason survives.
	if !strings.Contains(res.Text, "ENOENT") {
		t.Errorf("result text = %q, want the provider's error payload", res.Text)
	}
}

// An error frame resolves its call, so closeOpenCalls must not add a second
// result on top of it.
func TestCursorErrorStatusResolvesTheCall(t *testing.T) {
	turn, _ := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		cursorToolFrame("read", "error", argsRead, `{"status":"error","value":{}}`),
		fResultOK,
	)
	var n int
	for _, e := range turn.emitted() {
		if e.Kind == KindToolResult {
			n++
		}
	}
	if n != 1 {
		t.Errorf("got %d results, want 1 — the error frame already closed the call", n)
	}
}

// The SDK sets `truncated` when it clips an oversized args/result. Without
// surfacing it, a clipped file read or command output reads as the whole thing.
func TestCursorTruncatedResultIsFlagged(t *testing.T) {
	frame := func(trunc string) string {
		payload := map[string]any{
			"type": "tool_call", "call_id": "c1", "name": "read", "status": "completed",
			"args": json.RawMessage(argsRead), "result": json.RawMessage(resRead),
		}
		if trunc != "" {
			payload["truncated"] = json.RawMessage(trunc)
		}
		b, _ := json.Marshal(map[string]any{"sdkMessage": map[string]any{"type": "tool_call", "message": payload}})
		return string(b)
	}

	_, clipped := feed(t, AgentSpec{}, frame(`{"result":true}`))
	var got *ParsedEvent
	for i := range clipped {
		if clipped[i].Kind == KindToolResult {
			got = &clipped[i]
		}
	}
	if got == nil || !got.Truncated {
		t.Errorf("truncated result not flagged: %+v", clipped)
	}
	// A truncated payload is not itself a failure.
	if got != nil && got.IsError {
		t.Error("truncation must not be reported as an error")
	}

	_, whole := feed(t, AgentSpec{}, frame(""))
	for _, e := range whole {
		if e.Kind == KindToolResult && e.Truncated {
			t.Error("a complete result must not be flagged truncated")
		}
	}
}

// Truncation is provider fact, so it is stored on the block and survives a
// reload rather than being recomputed.
func TestTruncationSurvivesTheTranscript(t *testing.T) {
	msgs := []domain.Message{{
		Role:     "assistant",
		Provider: domain.ProviderCursor,
		Content:  []domain.ContentBlock{domain.ToolUseBlock("t1", "call-1", "read", json.RawMessage(argsRead))},
	}, {
		Role:     "user",
		Provider: domain.ProviderCursor,
		Content:  []domain.ContentBlock{domain.TruncatedToolResultBlock("t1", resRead, false, true)},
	}}
	items := flattenTranscript(msgs, "", nil)
	var tool *domain.ChatItem
	for i := range items {
		if items[i].Kind == "tool" {
			tool = &items[i]
		}
	}
	if tool == nil {
		t.Fatal("no tool item")
	}
	if !tool.ResultTruncated {
		t.Error("truncation lost on reload — the card would present a fragment as the whole output")
	}
}

// --- in-flight calls ---------------------------------------------------------

// A call with no result yet is IN FLIGHT, and the card needs to know that
// explicitly: an empty result is not the same thing, because delete and
// TodoWrite legitimately return nothing. Without the flag an in-flight edit
// falls back to dumping {"file_path": …}.
func TestTranscriptMarksInFlightCallsPending(t *testing.T) {
	msgs := []domain.Message{{
		Role:     "assistant",
		Provider: domain.ProviderCursor,
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "call-1", "edit", json.RawMessage(argsEdit)),
		},
	}}
	items := flattenTranscript(msgs, "", nil)
	if len(items) != 1 {
		t.Fatalf("items = %+v", items)
	}
	if !items[0].ResultPending {
		t.Error("a call with no result block must be marked pending")
	}
}

func TestTranscriptSettledCallNotPending(t *testing.T) {
	msgs := []domain.Message{{
		Role:         "assistant",
		StreamStatus: domain.StreamComplete,
		Provider:     domain.ProviderCursor,
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "call-1", "edit", json.RawMessage(argsEdit)),
		},
	}}
	items := flattenTranscript(msgs, "", nil)
	if len(items) != 1 {
		t.Fatalf("items = %+v", items)
	}
	if items[0].ResultPending {
		t.Error("a finished transcript must not leave calls spinning")
	}
}

// Once the result lands the row is no longer pending — including when the
// result text is legitimately EMPTY, which is the case this flag exists for.
func TestTranscriptClearsPendingOnEmptyResult(t *testing.T) {
	for _, tc := range []struct {
		name   string
		tool   string
		result string
	}{
		{"empty result", "delete", `{"status":"success","value":{}}`},
		{"todos", "updateTodos", `{"status":"success","value":{}}`},
		{"normal", "read", resRead},
	} {
		t.Run(tc.name, func(t *testing.T) {
			msgs := []domain.Message{{
				Role:     "assistant",
				Provider: domain.ProviderCursor,
				Content:  []domain.ContentBlock{domain.ToolUseBlock("t1", "c", tc.tool, json.RawMessage(`{}`))},
			}, {
				Role:     "user",
				Provider: domain.ProviderCursor,
				Content:  []domain.ContentBlock{domain.ToolResultBlock("t1", tc.result, false)},
			}}
			items := flattenTranscript(msgs, "", nil)
			var tool *domain.ChatItem
			for i := range items {
				if items[i].Kind == "tool" {
					tool = &items[i]
				}
			}
			if tool == nil {
				t.Fatal("no tool item")
			}
			if tool.ResultPending {
				t.Errorf("%s: still pending after its result arrived (result text %q)", tc.tool, tool.Result)
			}
		})
	}
}

// --- when an unreported call is closed ---------------------------------------

// The model cannot reason again until its outstanding tool results are back, so
// the next thinking frame closes any call still open.
//
// Waiting for the run to end left a failed read showing "Reading go.mod…" for
// the rest of the session, with later thoughts and tool calls rendered above it
// — while the very next thought was already "go.mod does not exist."
func TestCursorUnreportedCallClosesAtNextThinking(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		cursorThinkingFrame("go.mod does not exist.", 0),
	)
	var idx = -1
	for i, e := range got {
		if e.Kind == KindToolResult {
			idx = i
		}
	}
	if idx < 0 {
		t.Fatalf("call not closed at the next thinking frame: %+v", got)
	}
	if !got[idx].IsError {
		t.Error("an unreported call must be marked failed")
	}
	// And it must come BEFORE the reasoning it precedes.
	for i, e := range got {
		if e.Kind == KindThinking && i < idx {
			t.Error("the result must be emitted before the thinking that follows it")
		}
	}
}

// An assistant reply is the same boundary, for a turn whose tool call is
// followed directly by text.
func TestCursorUnreportedCallClosesAtNextReply(t *testing.T) {
	_, got := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		cursorAssistantFrame("That file does not exist."),
	)
	var n int
	for _, e := range got {
		if e.Kind == KindToolResult && e.IsError {
			n++
		}
	}
	if n != 1 {
		t.Errorf("got %d closed calls at the reply boundary, want 1: %+v", n, got)
	}
}

// A call that reports normally must NOT be closed early by the next thinking.
func TestCursorReportedCallSurvivesNextThinking(t *testing.T) {
	turn, _ := feed(t, AgentSpec{},
		cursorToolFrame("read", "running", argsRead, ""),
		cursorToolFrame("read", "completed", argsRead, resRead),
		cursorThinkingFrame("now I know the contents", 0),
		fResultOK,
	)
	var results []ParsedEvent
	for _, e := range turn.emitted() {
		if e.Kind == KindToolResult {
			results = append(results, e)
		}
	}
	if len(results) != 1 {
		t.Fatalf("got %d results, want 1: %+v", len(results), results)
	}
	if results[0].IsError {
		t.Error("a completed call was wrongly closed as failed")
	}
	if !strings.Contains(results[0].Text, "alpha") {
		t.Errorf("real result replaced by a synthetic one: %q", results[0].Text)
	}
}

// Only the calls open at that moment are closed; a later call is untouched.
func TestCursorClosingDoesNotAffectLaterCalls(t *testing.T) {
	frame := func(id, name, status, result string) string {
		payload := map[string]any{
			"type": "tool_call", "call_id": id, "name": name, "status": status,
			"args": json.RawMessage(argsRead),
		}
		if result != "" {
			payload["result"] = json.RawMessage(result)
		}
		b, _ := json.Marshal(map[string]any{"sdkMessage": map[string]any{"type": "tool_call", "message": payload}})
		return string(b)
	}
	turn, _ := feed(t, AgentSpec{},
		frame("a", "read", "running", ""),         // never reports
		cursorThinkingFrame("that one failed", 0), // closes "a"
		frame("b", "read", "running", ""),         // a later call
		frame("b", "read", "completed", resRead),  // which does report
		fResultOK,
	)
	byID := map[string]ParsedEvent{}
	for _, e := range turn.emitted() {
		if e.Kind == KindToolResult {
			byID[e.ToolUseID] = e
		}
	}
	if len(byID) != 2 {
		t.Fatalf("got %d results, want one per call: %+v", len(byID), byID)
	}
	if !byID["a"].IsError {
		t.Error("the unreported call should be failed")
	}
	if byID["b"].IsError {
		t.Error("the reported call was wrongly marked failed")
	}
}

// --- turn duration -----------------------------------------------------------

// A finished turn keeps the run time that was ticking while it ran. The duration
// comes from the PROVIDER's result, not our message timestamps, which would time
// the mirror's bookkeeping instead of the turn.
func TestTranscriptEmitsTurnFooterWithDuration(t *testing.T) {
	msgs := []domain.Message{{
		Role:     "assistant",
		Provider: domain.ProviderCursor,
		Content:  []domain.ContentBlock{domain.TextBlock("done")},
		// Stamped on the LAST message of the turn, alongside duration.
		DurationMS: 8460,
	}}
	items := flattenTranscript(msgs, "", nil)

	var footer *domain.ChatItem
	for i := range items {
		if items[i].Kind == "turn" {
			footer = &items[i]
		}
	}
	if footer == nil {
		t.Fatalf("no turn footer emitted: %+v", items)
	}
	if footer.DurationMS != 8460 {
		t.Errorf("duration = %d, want 8460", footer.DurationMS)
	}
	// It closes the turn, so it must come last.
	if items[len(items)-1].Kind != "turn" {
		t.Errorf("footer is not last: %+v", items)
	}
}

// Messages mid-turn carry no duration, so only the turn's END gets a footer —
// one per turn, not one per message.
func TestTranscriptFooterOnlyAtTurnEnd(t *testing.T) {
	msgs := []domain.Message{
		{Role: "assistant", Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "c1", "Bash", json.RawMessage(`{"command":"ls"}`)),
		}},
		{Role: "user", Content: []domain.ContentBlock{domain.ToolResultBlock("t1", "ok", false)}},
		{Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("done")}, DurationMS: 1200},
	}
	var footers int
	for _, it := range flattenTranscript(msgs, "", nil) {
		if it.Kind == "turn" {
			footers++
		}
	}
	if footers != 1 {
		t.Errorf("got %d turn footers, want 1", footers)
	}
}

// A turn with no reported duration gets no footer rather than a bogus "0s".
func TestTranscriptNoFooterWithoutDuration(t *testing.T) {
	msgs := []domain.Message{{
		Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("done")},
	}}
	for _, it := range flattenTranscript(msgs, "", nil) {
		if it.Kind == "turn" {
			t.Errorf("footer emitted with no duration: %+v", it)
		}
	}
}

// The runner must report the provider's duration, or there is nothing to stamp.
func TestCursorResultCarriesDuration(t *testing.T) {
	turn, _ := feed(t, AgentSpec{}, fResultOK)
	if got := turn.result().DurationMS; got != 1500 {
		t.Errorf("DurationMS = %d, want 1500 from the provider's result", got)
	}
}

// KindResult must be emitted (not only returned from Run) so the session mirror
// can stamp duration_ms and publish session.turn_end for the live footer.
func TestCursorEmitsKindResultWithDuration(t *testing.T) {
	_, got := feed(t, AgentSpec{}, fResultOK)
	var result *ParsedEvent
	for i := range got {
		if got[i].Kind == KindResult {
			result = &got[i]
		}
	}
	if result == nil {
		t.Fatalf("no KindResult emitted: %+v", got)
	}
	if result.Result == nil {
		t.Fatal("KindResult carried no Result payload")
	}
	if result.Result.DurationMS != 1500 {
		t.Errorf("DurationMS = %d, want 1500", result.Result.DurationMS)
	}
	if result.Text != "all good" {
		t.Errorf("Text = %q, want %q", result.Text, "all good")
	}
}
