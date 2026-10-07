package engine

import (
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

func TestAttachUserMCPCopiesSharedAllowlist(t *testing.T) {
	orig := roles[domain.RoleReviewer].allowedTools
	shared := make([]string, len(orig), len(orig)+8)
	copy(shared, orig)
	set := domain.Settings{MCPServers: []domain.MCPServer{
		{ID: "acme", Enabled: false, Transport: "http", URL: "https://acme.example/mcp"},
		{ID: "other", Enabled: false, Transport: "http", URL: "https://other.example/mcp"},
	}}
	projA := &domain.Project{EnabledMCP: []string{"acme"}}
	projB := &domain.Project{EnabledMCP: []string{"other"}}

	const n = 32
	got := make([]AgentSpec, 2*n)
	var wg sync.WaitGroup
	wg.Add(2 * n)
	for i := 0; i < n; i++ {
		go func(i int) {
			defer wg.Done()
			spec := AgentSpec{AllowedTools: shared}
			attachUserMCP(&spec, domain.ChatModeAsk, set, projA)
			got[i] = spec
		}(i)
		go func(i int) {
			defer wg.Done()
			spec := AgentSpec{AllowedTools: shared}
			attachUserMCP(&spec, domain.ChatModeAsk, set, projB)
			got[n+i] = spec
		}(i)
	}
	wg.Wait()

	if len(shared) != len(orig) {
		t.Errorf("attachUserMCP mutated shared allowlist len = %d, want %d", len(shared), len(orig))
	}
	for i := 0; i < n; i++ {
		if !hasTool(got[i], "mcp__acme__*") || hasTool(got[i], "mcp__other__*") {
			t.Errorf("attachUserMCP(projA) tools = %v, want mcp__acme__* only", got[i].AllowedTools)
		}
		if !hasTool(got[n+i], "mcp__other__*") || hasTool(got[n+i], "mcp__acme__*") {
			t.Errorf("attachUserMCP(projB) tools = %v, want mcp__other__* only", got[n+i].AllowedTools)
		}
	}
}

func TestSendAppMessageOmitsUserMCP(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("app-mcp", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	set := st.Settings()
	set.MCPServers = []domain.MCPServer{{
		ID: "acme", Enabled: true, Transport: "http", URL: "https://example.com/mcp",
	}}
	st.SetSettings(set)

	eng.SendAppMessage("", "What needs me?", "sonnet", nil)
	spec, ok := runner.specWithRole(domain.RoleMaestro)
	if !ok {
		t.Fatal("Ask Recur turn did not run")
	}
	if len(spec.UserMCP) != 0 {
		t.Errorf("SendAppMessage UserMCP = %+v, want none", spec.UserMCP)
	}
	if hasTool(spec, "mcp__acme__*") {
		t.Errorf("SendAppMessage tools = %v, must not include custom MCP", spec.AllowedTools)
	}
	if len(spec.AllowedTools) != 3 {
		t.Errorf("SendAppMessage AllowedTools = %v, want the three briefing tools", spec.AllowedTools)
	}
}

func TestAttachUserMCPLinearOffByDefault(t *testing.T) {
	spec := AgentSpec{AllowedTools: []string{"Read"}}
	set := domain.Settings{BundleValues: map[string]string{domain.SettingLinearAPIKey: "lin_api_x"}}
	attachUserMCP(&spec, domain.ChatModeAsk, set, nil)
	if len(spec.UserMCP) != 0 {
		t.Fatalf("linear default off: %+v", spec.UserMCP)
	}
}

func TestAttachUserMCPLinearReadonlyVsWrite(t *testing.T) {
	set := domain.Settings{
		MCPServers:   []domain.MCPServer{{ID: "linear", Enabled: true}},
		BundleValues: map[string]string{domain.SettingLinearAPIKey: "lin_api_x", domain.SettingLinearTeamKey: "ENG"},
	}
	ask := AgentSpec{}
	attachUserMCP(&ask, domain.ChatModeAsk, set, nil)
	if len(ask.UserMCP) != 1 || ask.UserMCP[0].URL != domain.LinearMCPReadOnlyURL {
		t.Fatalf("ask=%+v", ask.UserMCP)
	}
	if ask.UserMCP[0].Headers["Authorization"] != "Bearer lin_api_x" {
		t.Fatalf("header=%v", ask.UserMCP[0].Headers)
	}
	if !strings.Contains(ask.AppendSystemPrompt, "Default team key: ENG") {
		t.Fatalf("missing team hint: %q", ask.AppendSystemPrompt)
	}
	if !hasTool(ask, "mcp__linear__*") {
		t.Fatal("missing linear wildcard")
	}
	if strings.Contains(ask.AppendSystemPrompt, "lin_api_x") {
		t.Fatalf("api key leaked into prompt: %q", ask.AppendSystemPrompt)
	}

	for _, mode := range []string{domain.ChatModePlan, domain.ChatModeReview} {
		var spec AgentSpec
		attachUserMCP(&spec, mode, set, nil)
		if len(spec.UserMCP) != 1 || spec.UserMCP[0].URL != domain.LinearMCPReadOnlyURL {
			t.Fatalf("%s=%+v", mode, spec.UserMCP)
		}
	}

	agent := AgentSpec{}
	attachUserMCP(&agent, domain.ChatModeAgent, set, nil)
	if len(agent.UserMCP) != 1 || agent.UserMCP[0].URL != domain.LinearMCPWriteURL {
		t.Fatalf("agent=%+v", agent.UserMCP)
	}

	loop := AgentSpec{}
	attachUserMCP(&loop, domain.ChatModeLoop, set, nil)
	if len(loop.UserMCP) != 1 || loop.UserMCP[0].URL != domain.LinearMCPWriteURL {
		t.Fatalf("loop=%+v", loop.UserMCP)
	}
}

func TestAttachUserMCPLinearNoKeySkips(t *testing.T) {
	t.Setenv("LINEAR_API_KEY", "")
	spec := AgentSpec{}
	set := domain.Settings{MCPServers: []domain.MCPServer{{ID: "linear", Enabled: true}}}
	attachUserMCP(&spec, domain.ChatModeAgent, set, nil)
	if len(spec.UserMCP) != 0 {
		t.Fatalf("no key should skip: %+v", spec.UserMCP)
	}
}

func TestAttachUserMCPLinearKeyFromEnv(t *testing.T) {
	t.Setenv("LINEAR_API_KEY", "lin_env")
	spec := AgentSpec{}
	set := domain.Settings{MCPServers: []domain.MCPServer{{ID: "linear", Enabled: true}}}
	attachUserMCP(&spec, domain.ChatModeAsk, set, nil)
	if len(spec.UserMCP) != 1 || spec.UserMCP[0].Headers["Authorization"] != "Bearer lin_env" {
		t.Fatalf("env key: %+v", spec.UserMCP)
	}
}

func TestAttachUserMCPProjectForceOff(t *testing.T) {
	set := domain.Settings{
		MCPServers:   []domain.MCPServer{{ID: "linear", Enabled: true}},
		BundleValues: map[string]string{domain.SettingLinearAPIKey: "lin_api_x"},
	}
	proj := &domain.Project{DisabledMCP: []string{domain.MCPIDLinear}}
	spec := AgentSpec{}
	attachUserMCP(&spec, domain.ChatModeAgent, set, proj)
	if len(spec.UserMCP) != 0 {
		t.Fatalf("project disable: %+v", spec.UserMCP)
	}
}

func TestAttachUserMCPProjectForceOn(t *testing.T) {
	set := domain.Settings{BundleValues: map[string]string{domain.SettingLinearAPIKey: "lin_api_x"}}
	proj := &domain.Project{EnabledMCP: []string{domain.MCPIDLinear}}
	spec := AgentSpec{}
	attachUserMCP(&spec, domain.ChatModeAsk, set, proj)
	if len(spec.UserMCP) != 1 || spec.UserMCP[0].URL != domain.LinearMCPReadOnlyURL {
		t.Fatalf("project enable: %+v", spec.UserMCP)
	}
}

func TestAttachUserMCPCustomHTTP(t *testing.T) {
	set := domain.Settings{MCPServers: []domain.MCPServer{{
		ID: "acme", Name: "Acme", Enabled: true, Transport: "http",
		URL: "https://example.com/mcp", Headers: map[string]string{"Authorization": "Bearer z"},
	}}}
	spec := AgentSpec{}
	attachUserMCP(&spec, domain.ChatModeAsk, set, nil)
	if len(spec.UserMCP) != 1 || spec.UserMCP[0].Name != "acme" || spec.UserMCP[0].URL != "https://example.com/mcp" {
		t.Fatalf("custom=%+v", spec.UserMCP)
	}
	if !hasTool(spec, "mcp__acme__*") {
		t.Fatal("missing acme wildcard")
	}
}

func TestAttachUserMCPCustomStdio(t *testing.T) {
	set := domain.Settings{MCPServers: []domain.MCPServer{{
		ID: "fs", Name: "Filesystem", Enabled: true, Transport: "stdio",
		Command: "npx", Args: []string{"-y", "@modelcontextprotocol/server-filesystem"},
		Env: map[string]string{"FOO": "bar"},
	}}}
	spec := AgentSpec{}
	attachUserMCP(&spec, domain.ChatModeAsk, set, nil)
	if len(spec.UserMCP) != 1 || spec.UserMCP[0].Name != "fs" || spec.UserMCP[0].Command != "npx" {
		t.Fatalf("stdio=%+v", spec.UserMCP)
	}
	if spec.UserMCP[0].URL != "" || spec.UserMCP[0].Env["FOO"] != "bar" {
		t.Fatalf("stdio should not be http: %+v", spec.UserMCP)
	}
	if !hasTool(spec, "mcp__fs__*") {
		t.Fatal("missing fs wildcard")
	}
	raw := mcpConfigJSON(AgentSpec{UserMCP: spec.UserMCP})
	if !strings.Contains(raw, `"command":"npx"`) || !strings.Contains(raw, `"FOO":"bar"`) {
		t.Fatalf("stdio mcp-config=%s", raw)
	}
}

func TestMCPConfigJSONUserMCP(t *testing.T) {
	raw := mcpConfigJSON(AgentSpec{
		MCPURL: "http://127.0.0.1:9/tok/mcp",
		UserMCP: []MCPAttach{{
			Name:    "linear",
			URL:     domain.LinearMCPReadOnlyURL,
			Headers: map[string]string{"Authorization": "Bearer lin_api_x"},
		}},
	})
	if !strings.Contains(raw, `"linear"`) || !strings.Contains(raw, domain.LinearMCPReadOnlyURL) {
		t.Fatalf("config=%s", raw)
	}
	if !strings.Contains(raw, `"Authorization":"Bearer lin_api_x"`) && !strings.Contains(raw, `"Authorization": "Bearer lin_api_x"`) {
		t.Fatalf("missing auth header: %s", raw)
	}
	if !strings.Contains(raw, `"recur"`) {
		t.Fatalf("recur missing: %s", raw)
	}
	empty := mcpConfigJSON(AgentSpec{})
	if empty != "" {
		t.Fatalf("empty spec should omit mcp-config json, got %q", empty)
	}
}

func enableLinearMCP(st *store.Store) {
	set := st.Settings()
	set.MCPServers = []domain.MCPServer{{ID: domain.MCPIDLinear, Enabled: true}}
	if set.BundleValues == nil {
		set.BundleValues = map[string]string{}
	}
	set.BundleValues[domain.SettingLinearAPIKey] = "lin_api_x"
	st.SetSettings(set)
}

func assertLinearReadonly(t *testing.T, spec AgentSpec) {
	t.Helper()
	if len(spec.UserMCP) != 1 || spec.UserMCP[0].URL != domain.LinearMCPReadOnlyURL {
		t.Fatalf("linear mcp = %+v, want readonly %s", spec.UserMCP, domain.LinearMCPReadOnlyURL)
	}
	if !hasTool(spec, "mcp__linear__*") {
		t.Fatalf("missing mcp__linear__*; tools=%v", spec.AllowedTools)
	}
}

func assertLinearWrite(t *testing.T, spec AgentSpec) {
	t.Helper()
	if len(spec.UserMCP) != 1 || spec.UserMCP[0].URL != domain.LinearMCPWriteURL {
		t.Fatalf("linear mcp = %+v, want write %s", spec.UserMCP, domain.LinearMCPWriteURL)
	}
	if !hasTool(spec, "mcp__linear__*") {
		t.Fatalf("missing mcp__linear__*; tools=%v", spec.AllowedTools)
	}
}

func TestLinearModeFor(t *testing.T) {
	if got := linearModeFor(AgentSpec{Role: domain.RoleMaestro, AllowedTools: maestroTools}); got != domain.ChatModeLoop {
		t.Fatalf("maestro conductor = %s, want loop", got)
	}
	if got := linearModeFor(AgentSpec{Role: domain.RoleMaestro, AllowedTools: maestroPlanTools}); got != domain.ChatModeAsk {
		t.Fatalf("planner = %s, want readonly", got)
	}
	if got := linearModeFor(AgentSpec{AllowedTools: []string{"Read", "Edit", "Write"}}); got != domain.ChatModeAgent {
		t.Fatalf("editor = %s, want agent", got)
	}
	if got := linearModeFor(AgentSpec{Role: domain.RoleReviewer, AllowedTools: []string{"Read", "Bash"}}); got != domain.ChatModeAsk {
		t.Fatalf("reviewer = %s, want readonly", got)
	}
}

func TestPlanModeChatAttachesLinearReadonly(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("plan-lin", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	enableLinearMCP(st)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "plan-lin", "")
	if err != nil {
		t.Fatal(err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "plan ACME-1", "sonnet", nil, nil, domain.ChatModePlan, "")
	s, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no plan-mode turn")
	}
	assertLinearReadonly(t, s)
}

func TestAgentModeChatAttachesLinearWrite(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("agent-lin", "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	enableLinearMCP(st)
	ws, err := eng.CreateWorkspace(addProject(t, st, initRepo(t)), "agent-lin", "")
	if err != nil {
		t.Fatal(err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "fix ACME-1", "sonnet", nil, nil, domain.ChatModeAgent, "")
	s, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no agent-mode turn")
	}
	assertLinearWrite(t, s)
}

func TestSkipThinkingOmitsUserMCP(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("skip-lin", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	enableLinearMCP(st)
	if _, err := eng.runAgent(t.Context(), AgentSpec{
		WorkspaceID: "ws", SkipThinking: true, AllowedTools: []string{"Read", "Edit"},
	}, nil); err != nil {
		t.Fatal(err)
	}
	s, ok := runner.specAt(0)
	if !ok {
		t.Fatal("no turn")
	}
	if len(s.UserMCP) != 0 {
		t.Fatalf("one-shot should skip user MCP: %+v", s.UserMCP)
	}
}

func TestLoopSpecialistsAttachLinear(t *testing.T) {
	runner := &planBriefRunner{planText: "Summary: fix it.\n\nRelevant files:\n- a.go"}
	eng, st, _, _ := newTestEngine(t, runner)
	enableLinearMCP(st)
	id, err := eng.StartWorkspace(addProject(t, st, initRepo(t)), "ACME-1 fix it", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
	runner.mu.Lock()
	planSpec := runner.planSpec
	coderCalls := append([]AgentSpec(nil), runner.coderCalls...)
	reviewCalls := append([]AgentSpec(nil), runner.reviewCalls...)
	runner.mu.Unlock()
	if planSpec == nil {
		t.Fatal("planner never ran")
	}
	assertLinearReadonly(t, *planSpec)
	if len(coderCalls) == 0 {
		t.Fatal("coder never ran")
	}
	for _, c := range coderCalls {
		assertLinearWrite(t, c)
	}
	if len(reviewCalls) == 0 {
		t.Fatal("reviewer never ran")
	}
	for _, r := range reviewCalls {
		assertLinearReadonly(t, r)
	}
}

func TestMaestroChatAttachesLinearWrite(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("maestro-lin", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	enableLinearMCP(st)
	id := startedLoop(t, eng, addProject(t, st, initRepo(t)))
	_ = eng.SendMaestroMessage(id, "status?", nil)
	s, ok := runner.specWithRole(domain.RoleMaestro)
	if !ok {
		t.Fatal("no maestro turn")
	}
	assertLinearWrite(t, s)
}
