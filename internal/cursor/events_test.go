package cursor

import (
	"encoding/json"
	"testing"
)

// The fixtures below are VERBATIM frames captured from a real cursor-sdk-bridge
// (v1.0.0, sdk 1.0.26) via SdkAgentService/Send. Recur's install pin has since
// moved (see pinnedSDKVersion); these bytes were not re-captured. They pin the
// wire quirks that cost real debugging time and that no amount of reading the
// docs would reveal:
//
//   - the payload is nested one level under `message`
//   - payload keys are snake_case (agent_id, call_id, run_id)
//   - nested structs like `usage` are camelCase
//   - int64s are JSON strings in the result envelope, numbers in usage messages
//   - in-flight tool status is "running" (the CLI's stream-json says "started")
//   - every MCP tool reports name "mcp"; the real one is in args
const (
	frameStatusRunning = `{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-1","run_id":"run-1","status":"RUNNING"}},"offset":"1"}`

	frameAssistant = `{"sdkMessage":{"type":"assistant","message":{"type":"assistant","agent_id":"agent-1","run_id":"run-1","message":{"role":"assistant","content":[{"type":"text","text":"OK"}]}}},"offset":"5"}`

	frameThinking = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","agent_id":"agent-1","run_id":"run-1","text":"considering"}},"offset":"2"}`

	frameUsage = `{"sdkMessage":{"type":"usage","message":{"type":"usage","agent_id":"agent-1","run_id":"run-1","usage":{"inputTokens":11288,"outputTokens":33,"cacheReadTokens":8352,"cacheWriteTokens":0,"totalTokens":19673}}},"offset":"6"}`

	frameToolRunning = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","agent_id":"agent-1","run_id":"run-1","call_id":"tool_abc","name":"mcp","status":"running","args":{"providerIdentifier":"recur","toolName":"magic_number","args":{}}}},"offset":"13"}`

	frameToolCompleted = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","agent_id":"agent-1","run_id":"run-1","call_id":"tool_abc","name":"mcp","status":"completed","args":{"providerIdentifier":"recur","toolName":"magic_number","args":{}},"result":{"status":"success","value":{"content":[{"text":{"text":"The magic number is 4271."}}],"isError":false}}}},"offset":"14"}`

	frameToolErrored = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","agent_id":"agent-1","run_id":"run-1","call_id":"tool_def","name":"shell","status":"completed","args":{},"result":{"status":"success","value":{"isError":true}}}},"offset":"15"}`

	frameResult = `{"result":{"agentId":"agent-1","runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"OK","model":{"id":"composer-2.5"},"durationMs":"2391","usage":{"inputTokens":"11288","outputTokens":"33","cacheReadTokens":"8352","totalTokens":"19673"}}},"offset":"8"}`

	frameDone = `{"done":{"agentId":"agent-1","runId":"run-1"},"offset":"9"}`
)

func decodeFrame(t *testing.T, raw string) RunStreamEvent {
	t.Helper()
	var ev RunStreamEvent
	if err := json.Unmarshal([]byte(raw), &ev); err != nil {
		t.Fatalf("decode: %v", err)
	}
	return ev
}

func TestDecodeStatusFrame(t *testing.T) {
	ev := decodeFrame(t, frameStatusRunning)
	if ev.SDKMessage == nil {
		t.Fatal("no sdkMessage")
	}
	m := ev.SDKMessage
	if m.Type != MsgStatus || m.Status != StatusRunning {
		t.Errorf("type/status = %q/%q, want %q/%q", m.Type, m.Status, MsgStatus, StatusRunning)
	}
	// snake_case keys from the nested payload must reach the flat struct.
	if m.AgentID != "agent-1" || m.RunID != "run-1" {
		t.Errorf("agent/run = %q/%q, want agent-1/run-1", m.AgentID, m.RunID)
	}
}

func TestDecodeTaskFrame(t *testing.T) {
	// SDKTaskMessage is nested under message like assistant/thinking. Recur
	// must flatten type/text; it does not treat this as compaction.
	const frame = `{"sdkMessage":{"type":"task","message":{"type":"task","agent_id":"agent-1","run_id":"run-1","status":"running","text":"working on the nested call"}},"offset":"12"}`
	m := decodeFrame(t, frame).SDKMessage
	if m == nil || m.Type != MsgTask {
		t.Fatalf("Decode task frame type = %+v, want type %q", m, MsgTask)
	}
	if m.Text != "working on the nested call" {
		t.Errorf("Decode task frame Text = %q, want working on the nested call", m.Text)
	}
	if m.Status != "running" {
		t.Errorf("Decode task frame Status = %q, want running", m.Status)
	}
}

func TestDecodeAssistantFrame(t *testing.T) {
	ev := decodeFrame(t, frameAssistant)
	m := ev.SDKMessage
	if m == nil || m.Type != MsgAssistant {
		t.Fatalf("want assistant, got %+v", m)
	}
	// The body is nested TWO levels down (sdkMessage.message.message), which is
	// the easiest thing to get wrong.
	if got := m.Content.TextContent(); got != "OK" {
		t.Errorf("text = %q, want OK", got)
	}
}

func TestDecodeThinkingFrame(t *testing.T) {
	m := decodeFrame(t, frameThinking).SDKMessage
	if m == nil || m.Type != MsgThinking || m.Text != "considering" {
		t.Errorf("thinking frame decoded as %+v", m)
	}
}

func TestDecodeUsageFrameNumericTokens(t *testing.T) {
	m := decodeFrame(t, frameUsage).SDKMessage
	if m == nil || m.Usage == nil {
		t.Fatal("no usage")
	}
	if got := m.Usage.InputTokens.Int(); got != 11288 {
		t.Errorf("inputTokens = %d, want 11288", got)
	}
	if got := m.Usage.TotalTokens.Int(); got != 19673 {
		t.Errorf("totalTokens = %d, want 19673", got)
	}
}

func TestDecodeToolCallRewritesMCPLabel(t *testing.T) {
	m := decodeFrame(t, frameToolRunning).SDKMessage
	if m == nil || m.Type != MsgToolCall {
		t.Fatalf("want tool_call, got %+v", m)
	}
	if m.Status != ToolRunning {
		t.Errorf("status = %q, want %q", m.Status, ToolRunning)
	}
	if m.Name != ToolNameMCP {
		t.Errorf("raw name = %q, want %q", m.Name, ToolNameMCP)
	}
	// Every MCP tool reports name "mcp"; the engine needs the specific tool.
	if got := m.ToolLabel(); got != "mcp__recur__magic_number" {
		t.Errorf("label = %q, want mcp__recur__magic_number", got)
	}
	if m.CallID != "tool_abc" {
		t.Errorf("callId = %q, want tool_abc", m.CallID)
	}
}

func TestDecodeToolCallIDFallbacks(t *testing.T) {
	cases := []struct {
		name string
		raw  string
	}{
		{"id", `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","id":"tool_from_id","name":"read","status":"running"}}}`},
		{"toolCallId", `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","toolCallId":"tool_from_camel","name":"read","status":"running"}}}`},
		{"tool_call_id", `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","tool_call_id":"tool_from_snake","name":"read","status":"running"}}}`},
	}
	want := map[string]string{
		"id": "tool_from_id", "toolCallId": "tool_from_camel", "tool_call_id": "tool_from_snake",
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			m := decodeFrame(t, tc.raw).SDKMessage
			if m == nil || m.CallID != want[tc.name] {
				t.Errorf("CallID = %q, want %q", m.CallID, want[tc.name])
			}
		})
	}
}

func TestDecodeToolCallPrefersCallIDOverID(t *testing.T) {
	raw := `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"real","id":"other","name":"read","status":"running"}}}`
	m := decodeFrame(t, raw).SDKMessage
	if m == nil || m.CallID != "real" {
		t.Errorf("CallID = %q, want real (call_id wins)", m.CallID)
	}
}

func TestDecodeGrok46CallIDWithNewline(t *testing.T) {
	// Live grok-4.6 provider_tool_use_id from Recur's session store: Cursor
	// joins call-… and fc_… with a newline. Decode must keep that string intact
	// so emitStepOnce / session.step correlation still matches.
	const id = "call-9812a46f-b258-4ed8-8d09-55441deb1733-5\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_0"
	raw, err := json.Marshal(map[string]any{
		"sdkMessage": map[string]any{
			"type": "tool_call",
			"message": map[string]any{
				"type":    "tool_call",
				"call_id": id,
				"name":    "glob",
				"status":  "running",
				"args":    json.RawMessage(`{"globPattern":"TODO.md"}`),
			},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	m := decodeFrame(t, string(raw)).SDKMessage
	if m == nil {
		t.Fatal("no sdkMessage")
	}
	if m.CallID != id {
		t.Errorf("CallID = %q, want grok-4.6 newline id", m.CallID)
	}
	if m.Name != "glob" || m.Status != ToolRunning {
		t.Errorf("name/status = %q/%q", m.Name, m.Status)
	}
}

func TestDecodeBuiltinToolKeepsName(t *testing.T) {
	m := decodeFrame(t, frameToolErrored).SDKMessage
	if got := m.ToolLabel(); got != "shell" {
		t.Errorf("label = %q, want shell (built-ins are not rewritten)", got)
	}
}

func TestToolResultIsError(t *testing.T) {
	ok := decodeFrame(t, frameToolCompleted).SDKMessage
	if ok.ToolResultIsError() {
		t.Error("successful tool call reported as error")
	}
	// A protocol-level success carrying an application-level isError must still
	// count as a failure, or the feed shows a green tick on a failed tool.
	bad := decodeFrame(t, frameToolErrored).SDKMessage
	if !bad.ToolResultIsError() {
		t.Error("isError:true not detected")
	}
	// A tool_call with no result at all is in-flight, not failed.
	if decodeFrame(t, frameToolRunning).SDKMessage.ToolResultIsError() {
		t.Error("in-flight tool call reported as error")
	}
}

func TestDecodeResultFrameStringInts(t *testing.T) {
	ev := decodeFrame(t, frameResult)
	if ev.Result == nil || ev.Result.Result == nil {
		t.Fatal("no result")
	}
	r := ev.Result.Result
	if r.Result != "OK" {
		t.Errorf("text = %q, want OK", r.Result)
	}
	if r.IsError() {
		t.Error("finished run reported as error")
	}
	// int64 proto fields arrive as JSON STRINGS here but as numbers in the usage
	// message above — both must decode.
	if got := r.DurationMS.Int(); got != 2391 {
		t.Errorf("durationMs = %d, want 2391", got)
	}
	if r.Usage == nil || r.Usage.InputTokens.Int() != 11288 {
		t.Errorf("usage = %+v, want inputTokens 11288", r.Usage)
	}
	if r.Model == nil || r.Model.ID != "composer-2.5" {
		t.Errorf("model = %+v", r.Model)
	}
}

func TestDecodeDoneFrame(t *testing.T) {
	ev := decodeFrame(t, frameDone)
	if len(ev.Done) == 0 {
		t.Error("done frame not decoded")
	}
	if ev.Result != nil || ev.SDKMessage != nil {
		t.Error("done frame should carry neither result nor sdkMessage")
	}
}

func TestRunResultIsError(t *testing.T) {
	cases := []struct {
		name string
		res  *RunResult
		want bool
	}{
		{"nil is an error", nil, true},
		{"finished", &RunResult{Status: RunFinished}, false},
		{"errored", &RunResult{Status: RunError_}, true},
		{"cancelled", &RunResult{Status: RunCancelled}, true},
		{"error detail wins", &RunResult{Status: RunFinished, Error: &RunError{Message: "boom"}}, true},
		{"unknown status is not fatal", &RunResult{}, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := tc.res.IsError(); got != tc.want {
				t.Errorf("IsError() = %v, want %v", got, tc.want)
			}
		})
	}
}

func TestUnknownFrameIsIgnorable(t *testing.T) {
	// Cursor adds event types without a version bump, so an unknown type must
	// decode cleanly and simply carry no interesting fields.
	ev := decodeFrame(t, `{"sdkMessage":{"type":"brand_new","message":{"type":"brand_new","agent_id":"a"}},"offset":"3"}`)
	if ev.SDKMessage == nil || ev.SDKMessage.Type != "brand_new" {
		t.Fatalf("unknown frame mangled: %+v", ev.SDKMessage)
	}
	if ev.SDKMessage.Content != nil || ev.SDKMessage.Usage != nil {
		t.Error("unknown frame invented content")
	}
}

func TestFlexInt(t *testing.T) {
	var v struct {
		A flexInt `json:"a"`
		B flexInt `json:"b"`
		C flexInt `json:"c"`
		D flexInt `json:"d"`
	}
	if err := json.Unmarshal([]byte(`{"a":42,"b":"42","c":null,"d":""}`), &v); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if v.A != 42 || v.B != 42 {
		t.Errorf("number/string = %d/%d, want 42/42", v.A, v.B)
	}
	if v.C != 0 || v.D != 0 {
		t.Errorf("null/empty = %d/%d, want 0/0", v.C, v.D)
	}
}

func TestMCPServerMarshalsAsOneofWrapper(t *testing.T) {
	// The bridge rejects a flat {url,type} — the transport must be wrapped in
	// its oneof field name.
	b, err := json.Marshal(NewHTTPMCPServer("http://127.0.0.1:9/tok/mcp"))
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	want := `{"http":{"type":"HTTP_MCP_TRANSPORT_TYPE_HTTP","url":"http://127.0.0.1:9/tok/mcp"}}`
	if string(b) != want {
		t.Errorf("got  %s\nwant %s", b, want)
	}
}

func TestLocalOptionsCwdIsRepeated(t *testing.T) {
	// Sending cwd as a bare string is rejected by the bridge with
	// `cannot decode field sdk.v1.LocalAgentOptions.cwd`.
	b, err := json.Marshal(LocalOptions{Cwd: []string{"/tmp/wt"}})
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if string(b) != `{"cwd":["/tmp/wt"]}` {
		t.Errorf("got %s, want {\"cwd\":[\"/tmp/wt\"]}", b)
	}
}

func TestParseInteractionTypeNestedSummaryBody(t *testing.T) {
	raw := json.RawMessage(`{"type":"summary","update":{"type":"summary","summary":"older turns folded"}}`)
	typ, body := ParseInteractionType(raw)
	if typ != DeltaSummary {
		t.Fatalf("ParseInteractionType type = %q, want %q", typ, DeltaSummary)
	}
	var p struct {
		Summary string `json:"summary"`
	}
	if err := json.Unmarshal(body, &p); err != nil {
		t.Fatalf("ParseInteractionType body unmarshal: %v", err)
	}
	if p.Summary != "older turns folded" {
		t.Errorf("ParseInteractionType nested body summary = %q, want %q", p.Summary, "older turns folded")
	}
}

func TestParseInteractionType(t *testing.T) {
	cases := []struct {
		name string
		raw  string
		want string
	}{
		{"sdk tagged", `{"type":"summary-started"}`, DeltaSummaryStarted},
		{"nested update", `{"update":{"type":"summary-completed"}}`, DeltaSummaryCompleted},
		{"oneof started", `{"summaryStarted":{}}`, DeltaSummaryStarted},
		{"oneof completed", `{"summaryCompleted":{}}`, DeltaSummaryCompleted},
		{"oneof summary", `{"summary":{"summary":"older turns folded"}}`, DeltaSummary},
		{"oneof turn ended", `{"turnEnded":{"usage":{"inputTokens":10}}}`, DeltaTurnEnded},
		{"token delta ignored", `{"type":"token-delta","tokens":1}`, "token-delta"},
		{"empty", `{}`, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, _ := ParseInteractionType(json.RawMessage(tc.raw))
			if got != tc.want {
				t.Errorf("ParseInteractionType(%s) = %q, want %q", tc.raw, got, tc.want)
			}
		})
	}
}

func TestSubUsageClampsAtZero(t *testing.T) {
	got := SubUsage(
		TokenUsage{InputTokens: 100, CacheReadTokens: 50, TotalTokens: 160},
		TokenUsage{InputTokens: 80, CacheReadTokens: 40, TotalTokens: 120},
	)
	if got.InputTokens.Int() != 20 || got.CacheReadTokens.Int() != 10 || got.TotalTokens.Int() != 40 {
		t.Errorf("SubUsage() = %+v, want input=20 cacheRead=10 total=40", got)
	}
	neg := SubUsage(
		TokenUsage{InputTokens: 10},
		TokenUsage{InputTokens: 40},
	)
	if neg.InputTokens.Int() != 0 {
		t.Errorf("SubUsage() input = %d, want 0 (clamped)", neg.InputTokens.Int())
	}
}
