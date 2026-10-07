package mcp

import (
	"context"
	"encoding/json"
	"strings"

	"recur/internal/agenttools"
)

// toolName is the bare MCP tool name (the engine scopes it as
// mcp__recur__<name> in --allowedTools; headless ref §5).
const (
	toolReportFinding  = "report_finding"
	toolListFindings   = "list_findings"
	toolUpdateFinding  = "update_finding"
	toolDone           = "done"
	toolReportResult   = "report_result"
	toolPlan           = "plan"
	toolEscalate       = "escalate"
	toolAttachEvidence = "attach_evidence"
	toolSendBack       = "send_back"
	// Maestro hands
	toolEditPlan           = "edit_plan"
	toolRerunReview        = "rerun_review"
	toolDispatchSpecialist = "dispatch_specialist"
	toolExtendIterations   = "extend_iterations"
	toolExtendRounds       = "extend_rounds"
	toolReenterPhase       = "reenter_phase"
	toolRunPhase           = "run_phase"
	toolResumeLoop         = "resume_loop"
	// Structured plan capture
	toolSubmitPlan     = "submit_plan"
	toolUpdatePlanStep = "update_plan_step"
	// Permission prompt (--permission-prompt-tool). Claude Code calls this one
	// itself when a turn runs with --permission-mode default, so it is never
	// listed in a role's --allowedTools.
	toolPermissionRequest = "permission_request"
	// Agent Skills progressive disclosure (model-driven; user /slash is host-side)
	toolListSkills    = "list_skills"
	toolActivateSkill = "activate_skill"

	toolAskUser         = "ask_user"
	toolSwitchMode      = "switch_mode"
	toolCreateArtifact  = "create_artifact"
	toolListArtifacts   = "list_artifacts"
	toolReadArtifact    = "read_artifact"
	toolPresentArtifact = "present_artifact"
	toolRenameSession   = "rename_session"
	toolRenameWorkspace = "rename_workspace"
	toolRenameBranch    = "rename_branch"
	toolOpenPR          = "open_pr"
	toolListTerminals   = "list_terminals"
	toolReadTerminal    = "read_terminal"
)

// toolSpec is the MCP tools/list + tools/call wrapper around a Recur host tool.
type toolSpec struct {
	name        string
	description string
	schema      json.RawMessage
	handle      func(ctx context.Context, rep Reporter, b binding, args json.RawMessage) (string, error)
}

// recurMCPPrefix is how runner allowlists name Recur MCP tools
// (mcp__recur__done → done). Headless ref §5.
const recurMCPPrefix = "mcp__recur__"

// NamesFromAllowedTools extracts bare Recur MCP tool names from a runner
// --allowedTools list. Always returns a non-nil slice (possibly empty) so
// Register can tell "scope to nothing" from "no scope / unrestricted". Does
// not include permission_request — that is Claude Code–only (required in
// tools/list for --permission-prompt-tool) and is appended by the engine at
// registration via AppendPermissionRequest. Cursor bindings must omit it.
func NamesFromAllowedTools(allowedTools []string) []string {
	out := make([]string, 0)
	seen := map[string]bool{}
	for _, t := range allowedTools {
		name, ok := strings.CutPrefix(t, recurMCPPrefix)
		if !ok || name == "" || seen[name] {
			continue
		}
		seen[name] = true
		out = append(out, name)
	}
	return out
}

// AppendPermissionRequest adds permission_request if missing. Claude Code
// resolves --permission-prompt-tool against tools/list at startup, so Claude
// bindings must include it; Cursor must not (see engine.registerMCP). Always
// returns a non-nil slice.
func AppendPermissionRequest(names []string) []string {
	for _, n := range names {
		if n == toolPermissionRequest {
			return names
		}
	}
	out := make([]string, len(names)+1)
	copy(out, names)
	out[len(names)] = toolPermissionRequest
	return out
}

// wrapTool exposes an agenttools.Tool on the MCP HTTP surface. The tool owns
// validation and dispatch; this only fills Call and invokes Execute.
func wrapTool(t agenttools.Tool) toolSpec {
	return toolSpec{
		name:        t.Name(),
		description: t.Description(),
		schema:      t.Schema(),
		handle: func(ctx context.Context, rep Reporter, b binding, args json.RawMessage) (string, error) {
			return t.Execute(agenttools.Call{
				Ctx:         ctx,
				WorkspaceID: b.workspaceID,
				AgentCode:   b.agentCode,
				ChatCode:    b.chatCode,
				Args:        args,
				Host:        rep,
			})
		},
	}
}

// allTools wraps Recur host tools for MCP tools/list and tools/call.
// Coding tools are harness-only and are not registered here.
func allTools() []toolSpec {
	src := agenttools.HostTools()
	out := make([]toolSpec, 0, len(src))
	for _, t := range src {
		out = append(out, wrapTool(t))
	}
	return out
}
