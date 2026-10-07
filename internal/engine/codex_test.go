package engine

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/codex"
	"recur/internal/domain"
)

func TestCodexUserTextOmitsRoleWrap(t *testing.T) {
	got := codexUserPrompt(AgentSpec{
		SystemPrompt: "You are the Maestro — long charter.",
		Prompt:       "where are we?",
	}, false)
	if got != "where are we?" {
		t.Errorf("codexUserPrompt = %q, want the user turn only (role is developerInstructions)", got)
	}
	if strings.Contains(got, "role-instructions") {
		t.Errorf("must not prepend role onto the user turn, got %q", got)
	}
	nudge := codexUserPrompt(AgentSpec{
		SystemPrompt:       "You are the Maestro.",
		AppendSystemPrompt: "Name the chat.",
		Prompt:             "hi",
	}, false)
	if nudge != "hi\n\nName the chat." {
		t.Errorf("AppendSystemPrompt should still ride on a fresh thread, got %q", nudge)
	}
}

func TestCodexFileChangeStep(t *testing.T) {
	tool, path, raw := codexFileChangeStep([]byte(`{
		"changes":[{"path":"/tmp/a.ts","kind":{"type":"add"},"diff":"export const a = 1;\n"}]
	}`))
	if tool != "write" || path != "/tmp/a.ts" {
		t.Fatalf("add: tool=%q path=%q", tool, path)
	}
	if !strings.Contains(string(raw), `"content"`) || !strings.Contains(string(raw), "export const a") {
		t.Fatalf("add input missing content: %s", raw)
	}
	if got := codexFileChangeResultText([]byte(`{"changes":[{"diff":"export const a = 1;\n"}]}`), tool); got != "" {
		t.Fatalf("write result should be empty, got %q", got)
	}

	tool, path, raw = codexFileChangeStep([]byte(`{
		"changes":[{"path":"/tmp/a.ts","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-a\n+b\n"}]
	}`))
	if tool != "edit" || path != "/tmp/a.ts" {
		t.Fatalf("update: tool=%q path=%q", tool, path)
	}
	if strings.Contains(string(raw), "content") {
		t.Fatalf("edit input should be path-only: %s", raw)
	}
	diff := codexFileChangeResultText([]byte(`{"changes":[{"diff":"@@ -1 +1 @@\n-a\n+b\n"}]}`), tool)
	if !strings.Contains(diff, "@@") {
		t.Fatalf("edit result should be hunk diff: %q", diff)
	}

	tool, path, raw = codexFileChangeStep([]byte(`{
		"changes":[{"path":"/tmp/gone.ts","kind":{"type":"delete"}}]
	}`))
	if tool != "delete" || path != "/tmp/gone.ts" {
		t.Fatalf("delete: tool=%q path=%q", tool, path)
	}
	if got := summarizeToolInput(canonicalTool(tool), raw); got != "/tmp/gone.ts" {
		t.Fatalf("delete summary = %q, want the path not JSON", got)
	}
	if got := codexFileChangeResultText([]byte(`{"changes":[{"path":"/tmp/gone.ts","kind":{"type":"delete"}}]}`), tool); got != "" {
		t.Fatalf("delete result should be empty, got %q", got)
	}
}

func TestCodexReasoningSummaryText(t *testing.T) {
	cases := []struct {
		raw  string
		want string
	}{
		{`{"summary":["**Plan**","Next step"]}`, "**Plan**\n\nNext step"},
		{`{"summary":[{"text":"one"},{"text":"two"}]}`, "one\n\ntwo"},
		{`{"summary":[]}`, ""},
		{`{"summary":null}`, ""},
		{`{}`, ""},
	}
	for _, tc := range cases {
		if got := codexReasoningSummaryText([]byte(tc.raw)); got != tc.want {
			t.Errorf("codexReasoningSummaryText(%s) = %q, want %q", tc.raw, got, tc.want)
		}
	}
}

func TestCodexUserPromptOmitsPlanAndResumeStanding(t *testing.T) {
	plan := "Maestro plan: touch foo.go"
	first := codexUserPrompt(AgentSpec{
		SystemPrompt:       "You are the Coder.\n\n" + plan,
		AppendSystemPrompt: "This chat is in AGENT mode.",
		Prompt:             "do the thing",
	}, false)
	if strings.Contains(first, plan) {
		t.Errorf("codexUserPrompt must not copy SystemPrompt/plan into user text, got: %q", first)
	}
	if !strings.Contains(first, "AGENT mode") || !strings.Contains(first, "do the thing") {
		t.Errorf("fresh thread should still carry AppendSystemPrompt: %q", first)
	}

	resumed := codexUserPrompt(AgentSpec{
		SystemPrompt:       "You are the Coder.\n\n" + plan,
		AppendSystemPrompt: "This chat is in AGENT mode.",
		Prompt:             "and persist it",
		ResumeSession:      "thread-1",
	}, true)
	if resumed != "and persist it" {
		t.Errorf("codexUserPrompt(resumed) = %q, want task only", resumed)
	}

	withSubject := codexUserPrompt(AgentSpec{
		AppendSystemPrompt: "This chat is in AGENT mode.",
		TurnAppendPrompt:   "The operator is looking at GitHub pull request #3.",
		Prompt:             "what changed?",
		ResumeSession:      "thread-1",
	}, true)
	if strings.Contains(withSubject, "AGENT mode") {
		t.Errorf("codexUserPrompt(resumed) must omit standing notes, got %q", withSubject)
	}
	if !strings.Contains(withSubject, "GitHub pull request #3") || !strings.Contains(withSubject, "what changed?") {
		t.Errorf("codexUserPrompt(resumed) must keep per-turn subject, got %q", withSubject)
	}

	remount := codexUserPrompt(AgentSpec{
		AppendSystemPrompt:          "This chat is in PLAN mode.",
		Prompt:                      "continue",
		ResumeSession:               "thread-1",
		RefreshStandingInstructions: true,
	}, true)
	if !strings.Contains(remount, "PLAN mode") {
		t.Errorf("mode remount must re-attach append notes: %q", remount)
	}
}

func TestCodexSandboxFollowsAllowedTools(t *testing.T) {
	sandbox, policy := codexSandbox(AgentSpec{
		Role:         domain.RoleCoder,
		AllowedTools: []string{"Read", "Grep", "Bash"},
	}, false)
	if sandbox != "read-only" || policy == nil || policy.Type != "readOnly" {
		t.Errorf("codexSandbox(RoleCoder, no Edit) = %q %+v, want read-only", sandbox, policy)
	}

	sandbox, policy = codexSandbox(AgentSpec{
		Role:         domain.RoleCoder,
		AllowedTools: []string{"Read", "Edit", "Write", "Bash"},
	}, false)
	if sandbox != "workspace-write" || policy == nil || policy.Type != "workspaceWrite" {
		t.Errorf("codexSandbox(RoleCoder, Edit) = %q %+v, want workspace-write", sandbox, policy)
	}

	sandbox, policy = codexSandbox(AgentSpec{Role: domain.RoleReviewer}, false)
	if sandbox != "read-only" || policy == nil || policy.Type != "readOnly" || !policy.NetworkAccess {
		t.Errorf("codexSandbox(RoleReviewer) = %q %+v, want read-only with network", sandbox, policy)
	}
}

func TestCodexSandboxNetworkAccess(t *testing.T) {
	edit := AgentSpec{
		Role:         domain.RoleCoder,
		AllowedTools: []string{"Read", "Edit", "Write", "Bash"},
	}
	_, off := codexSandbox(edit, false)
	if off.NetworkAccess {
		t.Errorf("codexSandbox(edit, false).NetworkAccess = true, want false")
	}
	raw, err := json.Marshal(off)
	if err != nil {
		t.Fatalf("json.Marshal(off): %v", err)
	}
	if strings.Contains(string(raw), "networkAccess") {
		t.Errorf("codexSandbox(edit, false) marshal = %s, want no networkAccess", raw)
	}

	_, on := codexSandbox(edit, true)
	if !on.NetworkAccess {
		t.Errorf("codexSandbox(edit, true).NetworkAccess = false, want true")
	}
	raw, err = json.Marshal(on)
	if err != nil {
		t.Fatalf("json.Marshal(on): %v", err)
	}
	if !strings.Contains(string(raw), `"networkAccess":true`) {
		t.Errorf("codexSandbox(edit, true) marshal = %s, want networkAccess true", raw)
	}

	_, review := codexSandbox(AgentSpec{Role: domain.RoleReviewer}, false)
	if review.Type != "readOnly" || !review.NetworkAccess {
		t.Errorf("codexSandbox(reviewer, false) = %+v, want readOnly with network", review)
	}
	raw, err = json.Marshal(review)
	if err != nil {
		t.Fatalf("json.Marshal(review): %v", err)
	}
	if !strings.Contains(string(raw), `"networkAccess":true`) {
		t.Errorf("codexSandbox(reviewer) marshal = %s, want networkAccess true", raw)
	}

	ask := AgentSpec{Role: domain.RoleCoder, AllowedTools: []string{"Read", "Grep", "Bash"}}
	_, askPol := codexSandbox(ask, false)
	if askPol.Type != "readOnly" || !askPol.NetworkAccess {
		t.Errorf("codexSandbox(ask, false) = %+v, want readOnly with network", askPol)
	}
}

func TestCodexSandboxBypassIsDangerFullAccess(t *testing.T) {
	edit := AgentSpec{
		Role:           domain.RoleCoder,
		AllowedTools:   []string{"Read", "Edit", "Write", "Bash"},
		PermissionMode: domain.PermissionModeBypass,
	}
	sandbox, policy := codexSandbox(edit, true)
	if sandbox != "danger-full-access" || policy == nil || policy.Type != "dangerFullAccess" || policy.NetworkAccess {
		t.Errorf("codexSandbox(edit, bypass) = %q %+v, want danger-full-access", sandbox, policy)
	}

	sandbox, policy = codexSandbox(AgentSpec{
		Role:           domain.RoleReviewer,
		PermissionMode: domain.PermissionModeBypass,
	}, false)
	if sandbox != "read-only" || policy.Type != "readOnly" || !policy.NetworkAccess {
		t.Errorf("codexSandbox(reviewer, bypass) = %q %+v, want read-only with network", sandbox, policy)
	}
}

func TestCodexApprovalPolicy(t *testing.T) {
	if got := codexApprovalPolicy(domain.PermissionModeBypass); got != "never" {
		t.Errorf("codexApprovalPolicy(bypass) = %q, want never", got)
	}
	if got := codexApprovalPolicy(domain.PermissionModeAsk); got != "on-request" {
		t.Errorf("codexApprovalPolicy(ask) = %q, want on-request", got)
	}
	if got := codexApprovalPolicy(""); got != "on-request" {
		t.Errorf("codexApprovalPolicy(\"\") = %q, want on-request", got)
	}
	if got := codexApprovalPolicy(domain.PermissionModeDeny); got != "never" {
		t.Errorf("codexApprovalPolicy(dontAsk) = %q, want never", got)
	}
}

func TestCodexApprovalResponse(t *testing.T) {
	cmd := "item/commandExecution/requestApproval"
	got := codexApprovalResponse(cmd, domain.PermissionDecision{Allow: true, Remember: true}, nil)
	if m, _ := got.(map[string]any); m == nil || m["decision"] != "acceptForSession" {
		t.Errorf("codexApprovalResponse(cmd, allow+remember) = %v, want acceptForSession", got)
	}
	got = codexApprovalResponse(cmd, domain.PermissionDecision{Allow: true}, nil)
	if m, _ := got.(map[string]any); m == nil || m["decision"] != "accept" {
		t.Errorf("codexApprovalResponse(cmd, allow) = %v, want accept", got)
	}
	got = codexApprovalResponse(cmd, domain.PermissionDecision{}, nil)
	if m, _ := got.(map[string]any); m == nil || m["decision"] != "decline" {
		t.Errorf("codexApprovalResponse(cmd, deny) = %v, want decline", got)
	}

	params, _ := json.Marshal(map[string]any{
		"permissions": map[string]any{"fileSystem": map[string]any{"write": []string{"/tmp"}}},
	})
	got = codexApprovalResponse("item/permissions/requestApproval", domain.PermissionDecision{Allow: true, Remember: true}, params)
	m, _ := got.(map[string]any)
	if m == nil || m["scope"] != "session" {
		t.Errorf("codexApprovalResponse(perms, allow+remember) = %v, want scope session", got)
	}
	got = codexApprovalResponse("item/permissions/requestApproval", domain.PermissionDecision{}, params)
	m, _ = got.(map[string]any)
	if m == nil {
		t.Fatalf("codexApprovalResponse(perms, deny) = %v", got)
	}
	if _, ok := m["permissions"].(map[string]any); !ok {
		t.Errorf("codexApprovalResponse(perms, deny) permissions = %v, want empty object", m["permissions"])
	}
}

func TestHandleCodexServerRequestAsksHuman(t *testing.T) {
	spec := AgentSpec{WorkspaceID: "W1", AgentID: "W1-PA"}
	params, _ := json.Marshal(map[string]any{"command": "curl localhost:3000"})
	var asked domain.PermissionRequest
	ask := func(_ context.Context, req domain.PermissionRequest) (domain.PermissionDecision, error) {
		asked = req
		return domain.PermissionDecision{Allow: true, Remember: true}, nil
	}
	got, err := handleCodexServerRequest(context.Background(), spec, ask, "item/commandExecution/requestApproval", params, nil)
	if err != nil {
		t.Fatalf("handleCodexServerRequest: %v", err)
	}
	if asked.WorkspaceID != "W1" || asked.AgentCode != "PA" || asked.Tool != "Bash" {
		t.Errorf("ask = %+v, want W1/PA/Bash", asked)
	}
	if !strings.Contains(asked.Summary, "curl localhost:3000") {
		t.Errorf("summary = %q, want command", asked.Summary)
	}
	if m, _ := got.(map[string]any); m == nil || m["decision"] != "acceptForSession" {
		t.Errorf("result = %v, want acceptForSession", got)
	}
}

func TestHandleCodexElicitationAutoAccepts(t *testing.T) {
	asked := false
	ask := func(context.Context, domain.PermissionRequest) (domain.PermissionDecision, error) {
		asked = true
		return domain.PermissionDecision{}, nil
	}
	got, err := handleCodexServerRequest(context.Background(), AgentSpec{}, ask, "mcpServer/elicitation/request", nil, nil)
	if err != nil {
		t.Fatalf("handleCodexServerRequest: %v", err)
	}
	if asked {
		t.Error("MCP elicitation must not prompt the human")
	}
	if m, _ := got.(map[string]any); m == nil || m["action"] != "accept" {
		t.Errorf("elicitation result = %v, want action accept", got)
	}
}

func TestHandleCodexServerRequestDenyMode(t *testing.T) {
	// GenerateCommitMessage uses PermissionModeDeny so an unattended Codex
	// title turn cannot raise a permission card or wait out the 45s timeout.
	asked := false
	ask := func(context.Context, domain.PermissionRequest) (domain.PermissionDecision, error) {
		asked = true
		return domain.PermissionDecision{Allow: true}, nil
	}
	spec := AgentSpec{PermissionMode: domain.PermissionModeDeny, WorkspaceID: "W1"}
	got, err := handleCodexServerRequest(context.Background(), spec, ask, "item/fileChange/requestApproval", json.RawMessage(`{"itemId":"fc1"}`), nil)
	if err != nil {
		t.Fatalf("handleCodexServerRequest: %v", err)
	}
	if asked {
		t.Error("handleCodexServerRequest(dontAsk) must not prompt the human")
	}
	if m, _ := got.(map[string]any); m == nil || m["decision"] != "decline" {
		t.Errorf("handleCodexServerRequest(dontAsk) = %v, want decline", got)
	}

	got, err = handleCodexServerRequest(context.Background(), spec, ask, "mcpServer/elicitation/request", nil, nil)
	if err != nil {
		t.Fatalf("handleCodexServerRequest elicitation: %v", err)
	}
	if asked {
		t.Error("handleCodexServerRequest(dontAsk) elicitation must not prompt")
	}
	if m, _ := got.(map[string]any); m == nil || m["action"] != "accept" {
		t.Errorf("handleCodexServerRequest(dontAsk) elicitation = %v, want action accept", got)
	}
}

func TestHandleCodexServerRequestFileChangeIncludesPatch(t *testing.T) {
	fileItems := &sync.Map{}
	var r CodexRunner
	r.handleItemStarted([]byte(`{"item":{"id":"fc1","type":"fileChange","changes":[{"path":"/tmp/a.ts","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-a\n+b\n"}]}}`), map[string]string{}, nil, fileItems, func(ParsedEvent) {})

	params, _ := json.Marshal(map[string]any{
		"threadId": "thr",
		"turnId":   "turn",
		"itemId":   "fc1",
		"reason":   "sandbox write",
	})
	var asked domain.PermissionRequest
	ask := func(_ context.Context, req domain.PermissionRequest) (domain.PermissionDecision, error) {
		asked = req
		return domain.PermissionDecision{Allow: true}, nil
	}
	got, err := handleCodexServerRequest(context.Background(), AgentSpec{WorkspaceID: "W1", AgentID: "W1-C"}, ask, "item/fileChange/requestApproval", params, fileItems)
	if err != nil {
		t.Fatalf("handleCodexServerRequest: %v", err)
	}
	if asked.Tool != "Edit" {
		t.Errorf("handleCodexServerRequest tool = %q, want Edit", asked.Tool)
	}
	if !strings.Contains(asked.Summary, "/tmp/a.ts") {
		t.Errorf("handleCodexServerRequest summary = %q, want path", asked.Summary)
	}
	if !strings.Contains(asked.Input, "/tmp/a.ts") || !strings.Contains(asked.Input, "@@") {
		t.Errorf("handleCodexServerRequest input = %q, want path and diff", asked.Input)
	}
	if !strings.Contains(asked.Input, `"kind"`) || !strings.Contains(asked.Input, `"update"`) {
		t.Errorf("handleCodexServerRequest input = %q, want structured change kind", asked.Input)
	}
	if m, _ := got.(map[string]any); m == nil || m["decision"] != "accept" {
		t.Errorf("handleCodexServerRequest result = %v, want accept", got)
	}
}

func TestCodexFileChangePermissionInputPreservesKind(t *testing.T) {
	item := []byte(`{"changes":[
		{"path":"/tmp/old.ts","kind":{"type":"update","movePath":"/tmp/new.ts"},"diff":"@@ -1 +1 @@\n-a\n+b\n"},
		{"path":"/tmp/a.ts","kind":{"type":"add"},"diff":"hello\n"},
		{"path":"/tmp/gone.ts","kind":{"type":"delete"}}
	]}`)
	got := codexFileChangePermissionInput(item)
	if got["path"] != "/tmp/old.ts, /tmp/a.ts, /tmp/gone.ts" {
		t.Errorf("codexFileChangePermissionInput() path = %v, want joined source paths", got["path"])
	}
	raw, err := json.Marshal(got["changes"])
	if err != nil {
		t.Fatalf("json.Marshal(changes): %v", err)
	}
	s := string(raw)
	if !strings.Contains(s, `"type":"update"`) || !strings.Contains(s, `"movePath":"/tmp/new.ts"`) {
		t.Errorf("codexFileChangePermissionInput() changes = %s, want update movePath", s)
	}
	if !strings.Contains(s, `"type":"add"`) || !strings.Contains(s, `"type":"delete"`) {
		t.Errorf("codexFileChangePermissionInput() changes = %s, want add and delete kinds", s)
	}
	if got := codexFileChangePermissionInput([]byte(`{}`)); got != nil {
		t.Errorf("codexFileChangePermissionInput({}) = %v, want nil", got)
	}
}

func TestCodexNetworkOverrides(t *testing.T) {
	if got := codexNetworkOverrides(false); got != nil {
		t.Errorf("codexNetworkOverrides(false) = %v, want nil", got)
	}
	got := codexNetworkOverrides(true)
	want := `sandbox_workspace_write.network_access=true`
	if len(got) != 1 || got[0] != want {
		t.Errorf("codexNetworkOverrides(true) = %v, want [%s]", got, want)
	}
}

func TestSubTokenUsage(t *testing.T) {
	got := subTokenUsage(
		codex.TokenUsage{TotalTokens: 100, InputTokens: 80, OutputTokens: 20},
		codex.TokenUsage{TotalTokens: 40, InputTokens: 30, OutputTokens: 10},
	)
	if got.TotalTokens != 60 || got.InputTokens != 50 || got.OutputTokens != 10 {
		t.Fatalf("subTokenUsage = %+v", got)
	}
}

func TestApplyCodexMeterUsageSplitsInclusiveCache(t *testing.T) {
	// apollo.lemony-tacos PA3 last request (Codex GPT-5.6 Sol). inputTokens
	// includes cachedInputTokens (totalTokens == input+output).
	u := codex.TokenUsage{
		TotalTokens:       156713,
		InputTokens:       155843,
		CachedInputTokens: 154240,
		OutputTokens:      870,
	}
	var res Result
	applyCodexMeterUsage(&res, u, 258400)
	if res.InputTokens != 1603 || res.CacheReadTokens != 154240 {
		t.Fatalf("meter in=%d cacheRead=%d; want fresh 1603 / cache 154240 (not inclusive 155843+154240)",
			res.InputTokens, res.CacheReadTokens)
	}
	if got := contextUsedTokens(&res); got != 155843 {
		t.Errorf("contextUsedTokens = %d, want 155843 (last-request fill, not 7.9M)", got)
	}
	if res.ContextWindow != 258400 {
		t.Errorf("ContextWindow = %d, want 258400", res.ContextWindow)
	}
}

func TestStampCodexTurnUsagePrefersLastOverCumulative(t *testing.T) {
	// Same session: turn-cumulative billing is millions; last request fits the window.
	last := codex.TokenUsage{
		TotalTokens:       156713,
		InputTokens:       155843,
		CachedInputTokens: 154240,
		OutputTokens:      870,
	}
	costU := codex.TokenUsage{
		TotalTokens:       4034944,
		InputTokens:       4020424,
		CachedInputTokens: 3859456,
		OutputTokens:      14520,
	}
	var res Result
	stampCodexTurnUsage(&res, last, costU, 258400)
	if got := contextUsedTokens(&res); got != 155843 {
		t.Fatalf("meter used = %d, want last-request 155843 not cumulative 4020424+3859456", got)
	}
	if res.InputTokens != 1603 || res.CacheReadTokens != 154240 {
		t.Errorf("meter fields in=%d cacheRead=%d; want 1603/154240", res.InputTokens, res.CacheReadTokens)
	}
}

func TestObserveCodexTokenUsageKeepsLastForMeter(t *testing.T) {
	params := []byte(`{"tokenUsage":{"total":{"totalTokens":4034944,"inputTokens":4020424,"cachedInputTokens":3859456,"outputTokens":14520},"last":{"totalTokens":156713,"inputTokens":155843,"cachedInputTokens":154240,"outputTokens":870},"modelContextWindow":258400}}`)
	var st codexUsageState
	st.observe(params)
	if st.last.InputTokens != 155843 || st.last.CachedInputTokens != 154240 {
		t.Fatalf("last = %+v, want last-request 155843/154240", st.last)
	}
	if st.total.InputTokens != 4020424 {
		t.Errorf("total input = %d, want thread total 4020424", st.total.InputTokens)
	}
	if st.window != 258400 {
		t.Errorf("window = %d, want 258400", st.window)
	}
	var usage Result
	applyCodexMeterUsage(&usage, st.last, st.window)
	if got := contextUsedTokens(&usage); got != 155843 {
		t.Errorf("live meter used = %d, want 155843", got)
	}
}

func TestCodexServiceTierFromFlags(t *testing.T) {
	cases := []struct {
		flags map[string]string
		want  string
	}{
		{nil, ""},
		{map[string]string{}, ""},
		{map[string]string{"fast": "false"}, ""},
		{map[string]string{"fast": "true"}, "priority"},
		{map[string]string{"fast": "on"}, "priority"},
		{map[string]string{"fast": "priority"}, "priority"}, // catalogue On value
		{map[string]string{"fast": "turbo"}, "turbo"},
		{map[string]string{"serviceTier": "standard"}, ""},
		{map[string]string{"serviceTier": "priority"}, "priority"},
		// Legacy serviceTier wins over fast when both present.
		{map[string]string{"serviceTier": "priority", "fast": "false"}, "priority"},
	}
	for _, tc := range cases {
		if got := codexServiceTierFromFlags(tc.flags); got != tc.want {
			t.Errorf("codexServiceTierFromFlags(%v) = %q, want %q", tc.flags, got, tc.want)
		}
	}
}

func TestElapsedDurationMS(t *testing.T) {
	got := elapsedDurationMS(time.Now().Add(-1500 * time.Millisecond))
	if got < 1400 {
		t.Errorf("elapsedDurationMS(1.5s ago) = %d, want >= 1400", got)
	}
	if got := elapsedDurationMS(time.Now().Add(time.Second)); got != 1 {
		t.Errorf("elapsedDurationMS(future) = %d, want 1 (floor so the turn footer still renders)", got)
	}
}

func TestStampCodexDuration(t *testing.T) {
	res := Result{SessionID: "thread-1"}
	stampCodexDuration(time.Now().Add(-2*time.Second), &res)
	if res.DurationMS < 1900 {
		t.Errorf("stampCodexDuration(2s ago) DurationMS = %d, want >= 1900", res.DurationMS)
	}
	stampCodexDuration(time.Now().Add(-2*time.Second), nil) // must not panic
}

func TestCodexMCPReportFindingToolName(t *testing.T) {
	var r CodexRunner
	open := map[string]string{}
	args := map[string]json.RawMessage{}
	var got []ParsedEvent
	r.handleItemStarted([]byte(`{"item":{"type":"mcpToolCall","id":"i1","server":"recur","tool":"report_finding","arguments":{"file":"store.go","line":10,"message":"nil deref","severity":"high"}}}`), open, args, nil, func(ev ParsedEvent) {
		got = append(got, ev)
	})
	if len(got) != 1 {
		t.Fatalf("handleItemStarted events = %d, want 1", len(got))
	}
	if got[0].Tool != "mcp__recur__report_finding" {
		t.Errorf("handleItemStarted tool = %q, want mcp__recur__report_finding", got[0].Tool)
	}
}

func TestCodexTurnFooterFromStampedDuration(t *testing.T) {
	// The findings digest is hosted on the turn footer, which flattenTranscript
	// only emits when DurationMS > 0. Codex used to leave DurationMS at 0.
	msgs := []domain.Message{{
		Role:       "assistant",
		Provider:   domain.ProviderCodex,
		DurationMS: 8400,
		Content: []domain.ContentBlock{
			domain.ToolUseBlock("t1", "i1", "mcp__recur__report_finding", json.RawMessage(`{"file":"store.go","line":10,"message":"nil deref","severity":"high"}`)),
			domain.TextBlock("Filed 1 finding."),
		},
	}}
	items := flattenTranscript(msgs, "", nil)
	var footer *domain.ChatItem
	for i := range items {
		if items[i].Kind == "turn" {
			footer = &items[i]
		}
	}
	if footer == nil {
		t.Fatalf("flattenTranscript(%+v) missing turn footer, want DurationMS-stamped Codex turn", items)
	}
	if footer.DurationMS != 8400 {
		t.Errorf("turn footer DurationMS = %d, want 8400", footer.DurationMS)
	}
}

func TestCodexLongContextOverrides(t *testing.T) {
	if got := codexLongContextOverrides(nil); got != nil {
		t.Errorf("nil flags = %v, want nil (default 272K)", got)
	}
	if got := codexLongContextOverrides(map[string]string{"context": "272k"}); got != nil {
		t.Errorf("272k = %v, want nil", got)
	}
	got := strings.Join(codexLongContextOverrides(map[string]string{"context": "1m"}), "\n")
	for _, want := range []string{
		`model_context_window=1000000`,
		`model_auto_compact_token_limit=900000`,
	} {
		if !strings.Contains(got, want) {
			t.Errorf("1m overrides missing %q\n%s", want, got)
		}
	}
}
