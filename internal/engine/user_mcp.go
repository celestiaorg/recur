package engine

import (
	"os"
	"strings"

	"recur/internal/domain"
)

// attachUserMCP adds Settings → MCP servers that are on for this workspace
// onto spec (UserMCP + mcp__<id>__* allowlist). Mode picks Linear readonly vs write.
func attachUserMCP(spec *AgentSpec, mode string, set domain.Settings, project *domain.Project) {
	if spec == nil {
		return
	}
	var projOff, projOn []string
	if project != nil {
		projOff, projOn = project.DisabledMCP, project.EnabledMCP
	}
	globalOff := domain.MCPGlobalDisabled(set.MCPServers)
	linearHint := false
	var extra []string
	for _, s := range domain.MCPCatalog(set.MCPServers) {
		if !domain.EffectiveSkillEnabled(s.ID, globalOff, projOff, projOn) {
			continue
		}
		att, hint, ok := buildMCPAttach(s, mode, set)
		if !ok {
			continue
		}
		spec.UserMCP = append(spec.UserMCP, att)
		extra = append(extra, "mcp__"+s.ID+"__*")
		linearHint = linearHint || hint
	}
	if len(extra) > 0 {
		spec.AllowedTools = append(append([]string(nil), spec.AllowedTools...), extra...)
	}
	if linearHint {
		if team := linearTeamKey(set); team != "" {
			spec.AppendSystemPrompt = joinAppendPrompt(spec.AppendSystemPrompt,
				"Linear MCP is connected. Default team key: "+team+". Prefer that team unless the user names another.")
		}
	}
}

func buildMCPAttach(s domain.MCPServer, mode string, set domain.Settings) (MCPAttach, bool, bool) {
	if s.ID == domain.MCPIDLinear || s.Preset == domain.MCPPresetLinear {
		key := linearAPIKey(set)
		if key == "" {
			return MCPAttach{}, false, false
		}
		return MCPAttach{
			Name:    s.ID,
			URL:     domain.LinearMCPURL(mode),
			Headers: map[string]string{"Authorization": "Bearer " + key},
		}, true, true
	}
	if s.Transport == domain.MCPTransportStdio {
		if s.Command == "" {
			return MCPAttach{}, false, false
		}
		return MCPAttach{Name: s.ID, Command: s.Command, Args: s.Args, Env: s.Env}, false, true
	}
	if s.URL == "" {
		return MCPAttach{}, false, false
	}
	return MCPAttach{Name: s.ID, URL: s.URL, Headers: s.Headers}, false, true
}

func linearAPIKey(set domain.Settings) string {
	if k := set.BundleValue(domain.SettingLinearAPIKey); k != "" {
		return k
	}
	return strings.TrimSpace(os.Getenv("LINEAR_API_KEY"))
}

func linearTeamKey(set domain.Settings) string {
	if k := set.BundleValue(domain.SettingLinearTeamKey); k != "" {
		return k
	}
	return strings.TrimSpace(os.Getenv("LINEAR_TEAM_KEY"))
}

// linearModeFor picks Linear readonly vs write. Cursor has no per-run
// allowlist, so read-only turns (Ask/Plan/Review/QA/planner) get /mcp/readonly.
// Agent and the Maestro conductor get the write URL.
func linearModeFor(spec AgentSpec) string {
	if spec.Role == domain.RoleMaestro && specHasTool(spec, mcpAskUser) {
		return domain.ChatModeLoop
	}
	if isReadOnlyRole(spec) {
		return domain.ChatModeAsk
	}
	return domain.ChatModeAgent
}

func specHasTool(spec AgentSpec, name string) bool {
	for _, t := range spec.AllowedTools {
		if t == name {
			return true
		}
	}
	return false
}

// attachWorkspaceUserMCP wires Settings → MCP onto a turn for this workspace.
// Workspace-less Ask Recur stays on its three briefing tools — custom MCP is
// not filtered to readonly and must not leak into that coordinator.
func (e *Engine) attachWorkspaceUserMCP(spec *AgentSpec, workspaceID, mode string) {
	if e == nil || spec == nil || workspaceID == "" {
		return
	}
	var proj *domain.Project
	if ws, ok := e.Store.Workspace(workspaceID); ok {
		if p, ok := e.Store.Project(ws.ProjectID); ok {
			proj = &p
		}
	}
	attachUserMCP(spec, mode, e.settings(), proj)
}

// withoutLinearAPIKey copies env without LINEAR_API_KEY so provider processes
// (and their inherited MCP subprocesses) cannot read the host fallback key.
func withoutLinearAPIKey(env []string) []string {
	const prefix = "LINEAR_API_KEY="
	out := make([]string, 0, len(env))
	for _, e := range env {
		if strings.HasPrefix(e, prefix) {
			continue
		}
		out = append(out, e)
	}
	return out
}
