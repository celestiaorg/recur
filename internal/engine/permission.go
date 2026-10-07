package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
)

// permissionPromptTool is the MCP tool Claude Code calls to ask permission when
// a turn runs with --permission-mode default. Claude Code invokes it itself, so
// unlike every other mcp__recur__* tool it does NOT need to appear in the
// turn's --allowedTools.
const permissionPromptTool = "mcp__recur__permission_request"

// permissionTimeout bounds how long an agent's tool call waits on a human. A
// loop turn can raise one with nobody watching, and the agent subprocess holds
// an open MCP request the whole time, so this cannot be unbounded. On expiry
// the request is denied with a reason the model can act on.
const permissionTimeout = 10 * time.Minute

// editBuiltins are the Claude Code file-mutating tools. When a turn's allowlist
// omits them, ClaudeRunner passes --disallowedTools so they are removed from
// context rather than merely un-approved — allowedTools only auto-approves, so
// under bypassPermissions an omitted edit tool would otherwise still be
// callable (headless ref §1: a bare name removes the tool).
var editBuiltins = []string{"Edit", "Write", "NotebookEdit"}

// askUserBuiltin is Claude Code's interactive question tool. In headless -p it
// cannot collect answers (tool result: "The user did not answer the questions"),
// so ClaudeRunner denies it whenever builtins may be visible and interactive
// prompts use mcp__recur__ask_user instead.
const askUserBuiltin = "AskUserQuestion"

// skillBuiltin is Claude Code's native Skill tool (model-driven SKILL.md load).
// Recur owns catalog + activation via mcp__recur__list_skills / activate_skill,
// so the native tool is always denied when builtins may be visible.
const skillBuiltin = "Skill"

// subagentBuiltins are Claude Code's in-process subagent and worktree tools.
// Delegated work goes through mcp__recur__create_workspace (a child workspace) so it
// is visible, reviewed, and merged by Recur; the native tools are always denied.
var subagentBuiltins = []string{"Agent", "EnterWorktree", "ExitWorktree"}

// disallowedEditTools returns the edit builtins absent from the allowlist. This
// is the best-effort "don't write files" guard for read-only roles; it is
// independent of the permission mode, which is global (Settings). Empty
// allowlist means "no edit restriction intended" (unlike AskUserQuestion).
func disallowedEditTools(allowed []string) []string {
	if len(allowed) == 0 {
		return nil
	}
	have := make(map[string]bool, len(allowed))
	for _, t := range allowed {
		have[t] = true
	}
	var out []string
	for _, t := range editBuiltins {
		if !have[t] {
			out = append(out, t)
		}
	}
	return out
}

// disallowedClaudeTools is what ClaudeRunner passes as --disallowedTools: omitted
// edit builtins (when an allowlist is set) plus AskUserQuestion, Skill, and the
// subagent builtins always.
// A bare name removes the tool from the model context (headless ref §1).
// AskUserQuestion and Skill are included even for an empty allowlist — headless
// cannot answer the former, and Recur owns skill loading for the latter.
func disallowedClaudeTools(allowed []string) []string {
	return append(append(disallowedEditTools(allowed), askUserBuiltin, skillBuiltin), subagentBuiltins...)
}

// permissionArgs renders a turn's permission flags. The mode is machine-global
// (Settings.BypassPermissions), never derived from the role or chat mode:
//
//   - bypassPermissions — skip every check, app-wide. No prompt tool.
//   - default + --permission-prompt-tool — anything outside the turn's
//     --allowedTools becomes a permission request routed to the human. Claude
//     Code invokes the prompt tool itself, so it needs no allowlist entry.
//
// The prompt tool lives on the turn's own Recur MCP endpoint, so asking is only
// possible when one is wired. A turn with tools but no MCPURL therefore falls
// back to dontAsk: under "default" with nowhere to ask, the CLI would block on
// an interactive prompt that can never be answered headlessly, and hanging is a
// worse failure than denying. Engine call sites give every tool-bearing turn an
// MCP endpoint precisely so this fallback stays unreachable.
func permissionArgs(spec AgentSpec) []string {
	mode := spec.PermissionMode
	if mode == "" {
		mode = domain.PermissionModeDeny
	}
	if mode != domain.PermissionModeAsk {
		return []string{"--permission-mode", mode}
	}
	if spec.MCPURL == "" {
		return []string{"--permission-mode", domain.PermissionModeDeny}
	}
	return []string{
		"--permission-mode", domain.PermissionModeAsk,
		"--permission-prompt-tool", permissionPromptTool,
	}
}

// errPermissionAbandoned is returned when a request's turn ends (cancelled,
// crashed, or the agent gave up) before the human answered.
var errPermissionAbandoned = errors.New("permission request abandoned")

// pendingPermission is one in-flight request. done is buffered so answering
// never blocks on a waiter that has already given up.
type pendingPermission struct {
	req  domain.PermissionRequest
	done chan domain.PermissionDecision
}

// permissionBroker routes agent permission requests to a human and blocks the
// asking MCP handler until an answer arrives. It is the engine-side half of
// --permission-prompt-tool.
//
// Concurrency: every method is safe for concurrent use. Requests arrive on MCP
// HTTP goroutines (several may be in flight for one turn) and answers arrive on
// the UI's RPC goroutine.
type permissionBroker struct {
	mu      sync.Mutex
	pending map[string]*pendingPermission
	// approved records tool names the human allowed with "remember" for the
	// rest of a turn, keyed by turn (workspaceID + agentCode).
	approved map[string]map[string]bool
	seq      int

	// now and timeout are injectable so tests do not wait on wall-clock.
	now     func() time.Time
	timeout time.Duration

	// notify announces a new request; resolved announces it left the queue.
	notify   func(domain.PermissionRequest)
	resolved func(id string, d domain.PermissionDecision)
}

func newPermissionBroker() *permissionBroker {
	return &permissionBroker{
		pending:  map[string]*pendingPermission{},
		approved: map[string]map[string]bool{},
		now:      time.Now,
		timeout:  permissionTimeout,
	}
}

// turnKey identifies one agent turn for the remembered-approval set.
func turnKey(workspaceID, agentCode string) string { return workspaceID + "/" + agentCode }

// request blocks until the human answers, the turn's context is cancelled, or
// the timeout expires. A cancelled turn returns errPermissionAbandoned; an
// expired one returns a denial (not an error) so the model gets a usable
// message instead of a tool crash.
func (b *permissionBroker) request(ctx context.Context, req domain.PermissionRequest) (domain.PermissionDecision, error) {
	key := turnKey(req.WorkspaceID, req.AgentCode)

	b.mu.Lock()
	if b.approved[key][req.Tool] {
		b.mu.Unlock()
		return domain.PermissionDecision{Allow: true, Reason: "already approved for this turn"}, nil
	}
	b.seq++
	req.ID = fmt.Sprintf("perm-%d-%d", b.now().UnixNano(), b.seq)
	p := &pendingPermission{req: req, done: make(chan domain.PermissionDecision, 1)}
	b.pending[req.ID] = p
	notify := b.notify
	timeout := b.timeout
	b.mu.Unlock()

	if notify != nil {
		notify(req)
	}

	timer := time.NewTimer(timeout)
	defer timer.Stop()

	select {
	case d := <-p.done:
		return d, nil
	case <-ctx.Done():
		// The agent went away mid-wait. Retract the prompt too, or the human is
		// left staring at a question nobody is waiting on any more.
		b.forget(req.ID)
		d := domain.PermissionDecision{Reason: "the agent stopped before this was answered"}
		b.publishResolved(req.ID, d)
		return domain.PermissionDecision{}, errPermissionAbandoned
	case <-timer.C:
		b.forget(req.ID)
		d := domain.PermissionDecision{
			Reason: fmt.Sprintf("no answer within %s — denied automatically", timeout),
		}
		b.publishResolved(req.ID, d)
		return d, nil
	}
}

// Answer resolves a pending request. It is idempotent: answering an unknown or
// already-answered id reports false rather than erroring, so a double-click in
// the UI is harmless.
func (b *permissionBroker) Answer(id string, d domain.PermissionDecision) bool {
	b.mu.Lock()
	p, ok := b.pending[id]
	if ok {
		delete(b.pending, id)
		if d.Allow && d.Remember {
			key := turnKey(p.req.WorkspaceID, p.req.AgentCode)
			if b.approved[key] == nil {
				b.approved[key] = map[string]bool{}
			}
			b.approved[key][p.req.Tool] = true
		}
	}
	b.mu.Unlock()
	if !ok {
		return false
	}
	p.done <- d
	b.publishResolved(id, d)
	return true
}

// Pending lists unanswered requests, newest last, so a UI that reconnects can
// rebuild its queue without missing anything.
func (b *permissionBroker) Pending() []domain.PermissionRequest {
	b.mu.Lock()
	defer b.mu.Unlock()
	out := make([]domain.PermissionRequest, 0, len(b.pending))
	for _, p := range b.pending {
		out = append(out, p.req)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

// endTurn abandons anything still pending for a finished turn and drops its
// remembered approvals, so a later turn by the same agent starts clean.
func (b *permissionBroker) endTurn(workspaceID, agentCode string) {
	key := turnKey(workspaceID, agentCode)
	b.mu.Lock()
	delete(b.approved, key)
	var stale []*pendingPermission
	for id, p := range b.pending {
		if p.req.WorkspaceID == workspaceID && p.req.AgentCode == agentCode {
			stale = append(stale, p)
			delete(b.pending, id)
		}
	}
	b.mu.Unlock()

	for _, p := range stale {
		d := domain.PermissionDecision{Reason: "the turn ended before this was answered"}
		p.done <- d
		b.publishResolved(p.req.ID, d)
	}
}

func (b *permissionBroker) forget(id string) {
	b.mu.Lock()
	delete(b.pending, id)
	b.mu.Unlock()
}

func (b *permissionBroker) publishResolved(id string, d domain.PermissionDecision) {
	b.mu.Lock()
	fn := b.resolved
	b.mu.Unlock()
	if fn != nil {
		fn(id, d)
	}
}

// recurToolLabels are human titles for mcp__recur__* tools shown in permission
// prompts and inbox rows. Raw mcp__recur__list_findings is fine for the model;
// the human should see "List findings".
var recurToolLabels = map[string]string{
	"report_finding":        "Report finding",
	"list_findings":         "List findings",
	"update_finding":        "Update finding",
	"done":                  "Complete turn",
	"report_result":         "Report result",
	"plan":                  "Record plan",
	"escalate":              "Escalate",
	"ask_user":              "Ask user",
	"switch_mode":           "Switch mode",
	"create_artifact":       "Create artifact",
	"list_artifacts":        "List artifacts",
	"read_artifact":         "Read artifact",
	"present_artifact":      "Present artifact",
	"rename_session":        "Rename chat",
	"rename_workspace":      "Rename workspace",
	"rename_branch":         "Rename branch",
	"open_pr":               "Open PR",
	"create_workspace":      "Create workspace",
	"merge_child":           "Merge child workspace",
	"read_child":            "Read child workspace",
	"archive_child":         "Archive child workspace",
	"message_child":         "Message child workspace",
	"attach_evidence":       "Attach evidence",
	"edit_plan":             "Edit plan",
	"rerun_review":          "Re-run review",
	"dispatch_specialist":   "Dispatch specialist",
	"extend_iterations":     "Extend iterations",
	"resume_loop":           "Resume loop",
	"submit_plan":           "Submit plan",
	"update_plan_step":      "Update plan step",
	"list_skills":           "List skills",
	"activate_skill":        "Activate skill",
	"list_terminals":        "List terminals",
	"read_terminal":         "Read terminal",
	"list_workspace_briefs": "List workspaces",
	"get_workspace_context": "Get workspace",
	"present_workspace":     "Present workspace",
}

// permissionToolLabel is the human-facing tool name for prompts. Recur MCP tools
// get a short title; other mcp__server__tool names fall back to a spaced tool
// segment; everything else is unchanged (Bash, Write, …).
func permissionToolLabel(tool string) string {
	tool = strings.TrimSpace(tool)
	if tool == "" {
		return tool
	}
	if name, ok := strings.CutPrefix(tool, "mcp__recur__"); ok {
		if label, ok := recurToolLabels[name]; ok {
			return label
		}
		return humanizeSnake(name)
	}
	if strings.HasPrefix(tool, "mcp__") {
		parts := strings.Split(tool, "__")
		return humanizeSnake(parts[len(parts)-1])
	}
	return tool
}

func humanizeSnake(s string) string {
	s = strings.TrimSpace(strings.ReplaceAll(s, "_", " "))
	if s == "" {
		return s
	}
	// Title-case the first rune only — "list findings", not "List Findings".
	r := []rune(s)
	if r[0] >= 'a' && r[0] <= 'z' {
		r[0] = r[0] - 'a' + 'A'
	}
	return string(r)
}

// permissionSummary renders a tool call as one human-readable line. The whole
// point of the prompt is an informed yes/no, so the command or path has to be
// visible without expanding anything.
func permissionSummary(tool string, input json.RawMessage) string {
	label := permissionToolLabel(tool)
	var fields map[string]any
	if err := json.Unmarshal(input, &fields); err != nil || len(fields) == 0 {
		return label
	}
	// Prefer the field that actually tells the human what will happen.
	for _, k := range []string{
		"command", "file_path", "path", "url", "pattern", "query",
		"status", "summary", "message", "finding_id", "name", "role",
		"instruction", "step_id", "plan", "title",
	} {
		if v, ok := fields[k]; ok {
			if s, ok := v.(string); ok && strings.TrimSpace(s) != "" {
				return label + ": " + truncateLine(s, 160)
			}
		}
	}
	keys := make([]string, 0, len(fields))
	for k := range fields {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return label + " (" + strings.Join(keys, ", ") + ")"
}

func truncateLine(s string, max int) string {
	s = strings.TrimSpace(s)
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i] + " …"
	}
	if len(s) > max {
		s = s[:max] + "…"
	}
	return s
}
