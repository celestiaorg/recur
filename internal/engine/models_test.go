package engine

import (
	"context"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
)

// modelSpyRunner is a TEST DOUBLE that records the Model/Flags each turn was given,
// keyed by role, and behaves like convergeRunner otherwise (Coder edits a file;
// Reviewer files 0 findings so the loop converges on the first pass).
type modelSpyRunner struct {
	mu          sync.Mutex
	modelByRole map[domain.AgentRole][]string
	flagsByRole map[domain.AgentRole][]map[string]string
}

func (r *modelSpyRunner) record(spec AgentSpec) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.modelByRole == nil {
		r.modelByRole = map[domain.AgentRole][]string{}
	}
	if r.flagsByRole == nil {
		r.flagsByRole = map[domain.AgentRole][]map[string]string{}
	}
	r.modelByRole[spec.Role] = append(r.modelByRole[spec.Role], spec.Model)
	r.flagsByRole[spec.Role] = append(r.flagsByRole[spec.Role], cloneFlagMap(spec.Flags))
}

func (r *modelSpyRunner) models(role domain.AgentRole) []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]string(nil), r.modelByRole[role]...)
}

func (r *modelSpyRunner) flags(role domain.AgentRole) []map[string]string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]map[string]string(nil), r.flagsByRole[role]...)
}

func (r *modelSpyRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.record(spec)
	switch spec.Role {
	case domain.RoleCoder:
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "iteration edit\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	default:
		// Reviewer/Maestro/etc: no findings filed, so the loop converges immediately.
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	}
}

func cloneFlagMap(m map[string]string) map[string]string {
	if len(m) == 0 {
		return nil
	}
	out := make(map[string]string, len(m))
	for k, v := range m {
		out[k] = v
	}
	return out
}

// TestModelSettingsWireToAgentTurns is the Phase 3.2 regression test for model/
// flag settings (PRD §18): planner uses Maestro, coder uses Coder, reviewer uses
// Reviewer — asserted end to end through a real loop run (ScriptedRunner-backed).
func TestModelSettingsWireToAgentTurns(t *testing.T) {
	repo := initRepo(t)
	runner := &modelSpyRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	set := st.Settings()
	set.ModelLoop = "opus"
	set.ModelAgent = "haiku"
	set.ModelReview = "fable"
	set.FlagsLoop = map[string]string{"effort": "medium"}
	set.FlagsAgent = map[string]string{"effort": "low"}
	set.FlagsReview = map[string]string{"effort": "xhigh"}
	st.SetSettings(set)

	id, err := eng.StartWorkspace(projID, "Build a feature with configured models", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed)
	})
	l, _ := st.Workspace(id)
	if l.State != domain.StatePROpen {
		t.Fatalf("loop did not converge: state=%q", l.State)
	}

	assertRole := func(role domain.AgentRole, wantModel, wantEffort string) {
		t.Helper()
		models := runner.models(role)
		if len(models) == 0 {
			t.Fatalf("no %s turns observed", role)
		}
		for _, m := range models {
			if m != wantModel {
				t.Errorf("%s turn used model %q, want %s", role, m, wantModel)
			}
		}
		for _, f := range runner.flags(role) {
			if f["effort"] != wantEffort {
				t.Errorf("%s turn used effort %q, want %s", role, f["effort"], wantEffort)
			}
		}
	}

	assertRole(domain.RoleCoder, "haiku", "low")
	assertRole(domain.RoleMaestro, "opus", "medium")
	assertRole(domain.RoleReviewer, "fable", "xhigh")
}

func TestClaudeRunnerAppendsRoleNotReplace(t *testing.T) {
	r := &ClaudeRunner{}
	args := r.args(AgentSpec{
		Prompt:             "hi",
		SystemPrompt:       "You are the Maestro.",
		AppendSystemPrompt: "Name the chat.",
	})
	if containsFlag(args, "--system-prompt") {
		t.Fatalf("role must not replace Claude's default prompt: %v", args)
	}
	if !containsPair(args, "--append-system-prompt", "You are the Maestro.\n\nName the chat.") {
		t.Fatalf("role+notes should append together, got %v", args)
	}
}

func TestClaudeRunnerArgsIncludeEffort(t *testing.T) {
	r := &ClaudeRunner{}
	args := r.args(AgentSpec{
		Prompt: "hi",
		Model:  "sonnet",
		Flags:  map[string]string{"effort": "high"},
	})
	if !containsPair(args, "--model", "sonnet") {
		t.Fatalf("args missing --model sonnet: %v", args)
	}
	if !containsPair(args, "--effort", "high") {
		t.Fatalf("args missing --effort high: %v", args)
	}
}

func TestClaudeRunnerArgsApply1MContext(t *testing.T) {
	r := &ClaudeRunner{}
	args := r.args(AgentSpec{
		Prompt: "hi",
		Model:  "claude-sonnet-4-6",
		Flags:  map[string]string{"effort": "high", "context": "1m"},
	})
	if !containsPair(args, "--model", "claude-sonnet-4-6[1m]") {
		t.Fatalf("args missing --model claude-sonnet-4-6[1m]: %v", args)
	}
}

func TestResolveClaudeModel(t *testing.T) {
	// Always-1M Sonnet 5: stale context=1m must not append [1m].
	if got := ResolveClaudeModel("sonnet", map[string]string{"context": "1m"}); got != "sonnet" {
		t.Errorf("sonnet+1m = %q, want bare sonnet", got)
	}
	if got := ResolveClaudeModel("claude-sonnet-4-6", map[string]string{"context": "1m"}); got != "claude-sonnet-4-6[1m]" {
		t.Errorf("4.6+1m = %q", got)
	}
	if got := ResolveClaudeModel("claude-sonnet-4-6", map[string]string{"context": "200k"}); got != "claude-sonnet-4-6" {
		t.Errorf("200k = %q", got)
	}
	if got := ResolveClaudeModel("sonnet", map[string]string{"context": "300k"}); got != "sonnet" {
		t.Errorf("legacy 300k = %q", got)
	}
	if got := ResolveClaudeModel("sonnet", nil); got != "sonnet" {
		t.Errorf("nil = %q", got)
	}
	// Pinned always-1M ids carry [1m] so an older Claude Code that does not
	// recognize the id still uses a 1M auto-compact window.
	if got := ResolveClaudeModel("claude-fable-5-1", map[string]string{"effort": "high"}); got != "claude-fable-5-1[1m]" {
		t.Errorf("fable 5.1 = %q", got)
	}
	if got := ResolveClaudeModel("claude-opus-5-5", nil); got != "claude-opus-5-5[1m]" {
		t.Errorf("opus 5.5 = %q", got)
	}
	if got := ResolveClaudeModel("claude-sonnet-5-5", nil); got != "claude-sonnet-5-5[1m]" {
		t.Errorf("sonnet 5.5 = %q", got)
	}
	if got := ResolveClaudeModel("claude-opus-4-6", nil); got != "claude-opus-4-6" {
		t.Errorf("selectable opus 4.6 without flag = %q", got)
	}
}

// TestClaudeRunnerArgsForkSession covers the Claude CLI flag. Recur chat
// forks inject a transcript and do not set AgentSpec.ForkSession.
func TestClaudeRunnerArgsForkSession(t *testing.T) {
	r := &ClaudeRunner{}
	args := r.args(AgentSpec{Prompt: "hi", ResumeSession: "sess-1", ForkSession: true})
	if !containsPair(args, "--resume", "sess-1") || !containsFlag(args, "--fork-session") {
		t.Fatalf("fork resume missing flags: %v", args)
	}
	argsOff := r.args(AgentSpec{Prompt: "hi", ResumeSession: "sess-1"})
	if containsFlag(argsOff, "--fork-session") {
		t.Fatalf("--fork-session should require ForkSession: %v", argsOff)
	}
	argsNoResume := r.args(AgentSpec{Prompt: "hi", ForkSession: true})
	if containsFlag(argsNoResume, "--fork-session") {
		t.Fatalf("--fork-session without --resume is meaningless: %v", argsNoResume)
	}
}

func TestClaudeRunnerArgsPromptOnStdin(t *testing.T) {
	r := &ClaudeRunner{}
	const prompt = "huge-fork-history"
	args := r.args(AgentSpec{Prompt: prompt, PromptOnStdin: true})
	for _, a := range args {
		if a == prompt {
			t.Fatalf("prompt must not be on argv: %v", args)
		}
	}
	if !containsPair(args, "--input-format", "stream-json") {
		t.Fatalf("stdin prompt missing stream-json input: %v", args)
	}
}

func TestClaudeRunnerArgsIncludePartials(t *testing.T) {
	r := &ClaudeRunner{}
	args := r.args(AgentSpec{Prompt: "hi", IncludePartials: true})
	if !containsFlag(args, "--include-partial-messages") {
		t.Fatalf("args missing --include-partial-messages: %v", args)
	}
	argsOff := r.args(AgentSpec{Prompt: "hi"})
	if containsFlag(argsOff, "--include-partial-messages") {
		t.Fatalf("partials should be off by default: %v", argsOff)
	}
}

func TestClaudeRunnerArgsInjectUsesStreamJSONInput(t *testing.T) {
	r := &ClaudeRunner{}
	ch := make(chan string, 1)
	args := r.args(AgentSpec{Prompt: "do the thing", Inject: ch})
	if !containsFlag(args, "--input-format") || !containsPair(args, "--input-format", "stream-json") {
		t.Fatalf("Inject mode must use --input-format stream-json: %v", args)
	}
	// Prompt is written on stdin, not as a -p argument (keeps mid-turn injects on the same pipe).
	for i, a := range args {
		if a == "-p" && i+1 < len(args) && args[i+1] == "do the thing" {
			t.Fatalf("Inject mode must not pass Prompt as -p arg: %v", args)
		}
	}
}

func TestClaudeRunnerArgsPermissionMode(t *testing.T) {
	r := &ClaudeRunner{}
	args := r.args(AgentSpec{Prompt: "hi"})
	if !containsPair(args, "--permission-mode", "dontAsk") {
		t.Fatalf("default permission mode should be dontAsk: %v", args)
	}
	bypass := r.args(AgentSpec{Prompt: "hi", PermissionMode: "bypassPermissions"})
	if !containsPair(bypass, "--permission-mode", "bypassPermissions") {
		t.Fatalf("bypass permission mode missing: %v", bypass)
	}
}

func TestClaudeRunnerArgsDisallowedEditTools(t *testing.T) {
	r := &ClaudeRunner{}
	ro := r.args(AgentSpec{
		Prompt:         "hi",
		AllowedTools:   []string{"Read", "Bash", "mcp__recur__report_finding"},
		PermissionMode: "bypassPermissions",
	})
	if !containsPair(ro, "--disallowedTools", "Edit,Write,NotebookEdit,AskUserQuestion,Skill,Agent,EnterWorktree,ExitWorktree") {
		t.Fatalf("read-only allowlist must disallow edit builtins + AskUserQuestion+Skill: %v", ro)
	}
	coder := r.args(AgentSpec{
		Prompt:       "hi",
		AllowedTools: []string{"Read", "Edit", "Write", "Bash", "NotebookEdit"},
	})
	// Full edit allowlist still denies AskUserQuestion (headless cannot answer it)
	// and Skill (Recur owns skill loading).
	if !containsPair(coder, "--disallowedTools", "AskUserQuestion,Skill,Agent,EnterWorktree,ExitWorktree") {
		t.Fatalf("edit allowlist must still disallow AskUserQuestion+Skill: %v", coder)
	}
	// No allowlist + default toolbox: still deny AskUserQuestion+Skill.
	bare := r.args(AgentSpec{Prompt: "hi"})
	if !containsPair(bare, "--disallowedTools", "AskUserQuestion,Skill,Agent,EnterWorktree,ExitWorktree") {
		t.Fatalf("default toolbox must disallow AskUserQuestion+Skill: %v", bare)
	}
	if !containsFlag(bare, "--disable-slash-commands") {
		t.Fatalf("default toolbox must pass --disable-slash-commands: %v", bare)
	}
}

func TestClaudeRunnerArgsPermissionPromptTool(t *testing.T) {
	r := &ClaudeRunner{}

	bypass := r.args(AgentSpec{Prompt: "hi", PermissionMode: "bypassPermissions", MCPURL: "http://127.0.0.1:1/t/mcp"})
	if !containsPair(bypass, "--permission-mode", "bypassPermissions") {
		t.Errorf("bypass mode not passed: %v", bypass)
	}
	if containsFlag(bypass, "--permission-prompt-tool") {
		t.Errorf("bypass must not wire a prompt tool: %v", bypass)
	}

	ask := r.args(AgentSpec{Prompt: "hi", PermissionMode: "default", MCPURL: "http://127.0.0.1:1/t/mcp"})
	if !containsPair(ask, "--permission-mode", "default") {
		t.Errorf("ask mode not passed: %v", ask)
	}
	// Claude Code invokes the prompt tool itself, so it must be reachable over
	// the turn's MCP endpoint but must NOT be in --allowedTools.
	if !containsPair(ask, "--permission-prompt-tool", "mcp__recur__permission_request") {
		t.Errorf("ask mode must wire the prompt tool: %v", ask)
	}
}

func TestClaudeRunnerArgsCompletionTurn(t *testing.T) {
	r := &ClaudeRunner{}
	noTools := ""
	args := r.args(AgentSpec{
		Prompt:       "hi",
		Tools:        &noTools,
		MaxTurns:     1,
		SkipThinking: true,
	})
	if !containsPair(args, "--tools", "") {
		t.Fatalf("completion turn must pass --tools \"\": %v", args)
	}
	if containsFlag(args, "--disallowedTools") {
		t.Fatalf("completion turn with --tools \"\" needs no --disallowedTools: %v", args)
	}
	if !containsFlag(args, "--disable-slash-commands") {
		t.Fatalf("completion turn still disables native skills: %v", args)
	}
	if !containsPair(args, "--max-turns", "1") {
		t.Fatalf("completion turn must pass --max-turns 1: %v", args)
	}
	if containsFlag(args, "--thinking") {
		t.Fatalf("SkipThinking should omit --thinking: %v", args)
	}
	// Default coding turns still get thinking + no --tools/--max-turns.
	normal := r.args(AgentSpec{Prompt: "hi"})
	if !containsPair(normal, "--thinking", "adaptive") {
		t.Fatalf("default turn should enable thinking: %v", normal)
	}
	if containsFlag(normal, "--tools") || containsFlag(normal, "--max-turns") {
		t.Fatalf("default turn should not set --tools/--max-turns: %v", normal)
	}
}

func TestSettingsPermissionMode(t *testing.T) {
	// Off is "default" — the mode that raises a permission request, which the
	// runner pairs with --permission-prompt-tool. dontAsk would auto-deny
	// instead of asking, which is the behavior this setting exists to replace.
	var off domain.Settings
	if got := off.PermissionMode(); got != "default" {
		t.Fatalf("BypassPermissions=false PermissionMode: got %q, want default", got)
	}
	on := domain.Settings{BypassPermissions: true}
	if got := on.PermissionMode(); got != "bypassPermissions" {
		t.Fatalf("bypass PermissionMode: got %q, want bypassPermissions", got)
	}
}

func containsFlag(args []string, flag string) bool {
	for _, a := range args {
		if a == flag {
			return true
		}
	}
	return false
}

func containsPair(args []string, flag, val string) bool {
	for i := 0; i+1 < len(args); i++ {
		if args[i] == flag && args[i+1] == val {
			return true
		}
	}
	return false
}
