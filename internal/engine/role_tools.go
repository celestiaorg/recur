package engine

import (
	"strings"

	"recur/internal/bundles"
	"recur/internal/domain"
)

// ModeTools is the closed tool set for a [[role]].mode. Specialists never receive
// interactive MCP tools (switch_mode, ask_user,
// rename_workspace) but do get list_skills / activate_skill — Recur is the only
// skill loader. The conductor (mode=loop) gets rename tools plus loop-flow hands.
func ModeTools(mode string) []string {
	ask := append(append([]string{}, readExploreTools...), gitReadTools...)
	ask = append(ask, mcpListFindings)
	ask = append(ask, skillMCPTools...)
	switch strings.ToLower(strings.TrimSpace(mode)) {
	case bundles.RoleModePlan:
		return append(append([]string{}, ask...), mcpSubmitPlan)
	case bundles.RoleModeAgent:
		return append(append(append([]string{}, ask...), editTools...), mcpUpdateFinding)
	case bundles.RoleModeReview:
		return append(append([]string{}, ask...), "Bash", mcpReportFinding, mcpUpdateFinding)
	case bundles.RoleModeLoop:
		return append(append([]string{}, ask...),
			mcpPlan, mcpEditPlan, mcpSubmitPlan, mcpUpdatePlanStep,
			mcpDispatchSpecialist, mcpReenterPhase, mcpRunPhase, mcpExtendRounds,
			mcpMessageChild,
			mcpCreateArtifact, mcpListArtifacts, mcpReadArtifact, mcpPresentArtifact,
			mcpListTerminals, mcpReadTerminal,
			mcpRenameSession, mcpRenameWorkspace, mcpRenameBranch,
		)
	default: // ask
		return ask
	}
}

// loopFlowTools are done / escalate / list_findings — only on non-loop roles.
func loopFlowTools() []string {
	return []string{mcpDone, mcpEscalate, mcpListFindings}
}

// RoleAllowlist is mode_tools ∪ loop_flow ∪ tools − disabled_tools. The reserved
// token "playwright" turns on the browser MCP server (mcp__playwright__*) rather
// than appearing as a Recur tool name.
func RoleAllowlist(role bundles.Role) []string {
	seen := map[string]bool{}
	var out []string
	add := func(t string) {
		t = strings.TrimSpace(t)
		if t == "" || seen[t] {
			return
		}
		seen[t] = true
		out = append(out, t)
	}
	for _, t := range ModeTools(role.Mode) {
		add(t)
	}
	if role.Mode != bundles.RoleModeLoop {
		for _, t := range loopFlowTools() {
			add(t)
		}
	}
	playwright := false
	for _, raw := range role.Tools {
		if strings.TrimSpace(raw) == "playwright" {
			playwright = true
			continue
		}
		add(mapRoleTool(raw))
	}
	if playwright {
		add(mcpPlaywrightAll)
	}
	deny := map[string]bool{}
	for _, d := range role.DisabledTools {
		deny[mapRoleTool(d)] = true
		deny[strings.TrimSpace(d)] = true
	}
	filtered := out[:0]
	for _, t := range out {
		if deny[t] {
			continue
		}
		filtered = append(filtered, t)
	}
	return filtered
}

// RoleWantsPlaywright reports whether the role listed the reserved playwright token.
func RoleWantsPlaywright(role bundles.Role) bool {
	for _, t := range role.Tools {
		if strings.TrimSpace(t) == "playwright" {
			return true
		}
	}
	return false
}

func firstPartyQARole() bundles.Role {
	return bundles.Role{
		Mode:  bundles.RoleModeAsk,
		Tools: []string{"attach_evidence", "playwright", "report_result"},
	}
}

func mapRoleTool(name string) string {
	name = strings.TrimSpace(name)
	if name == "" || name == "playwright" {
		return ""
	}
	if strings.HasPrefix(name, "mcp__") || strings.Contains(name, "(") {
		return name
	}
	switch name {
	case "Read", "Glob", "Grep", "LSP", "WebFetch", "WebSearch",
		"Edit", "Write", "Bash", "NotebookEdit":
		return name
	default:
		return "mcp__recur__" + name
	}
}

// slotRoleSpec resolves a phase slot to a roleSpec (catalog allowlist + code/name
// when the run has a bundle; otherwise the first-party fallback).
func (l *Loop) slotRoleSpec(slot string, fallback roleSpec) roleSpec {
	if l == nil {
		return fallback
	}
	id := l.phaseRoleID(slot)
	if id == "" {
		return fallback
	}
	cat := l.loopCatalog()
	role, ok := bundles.FindRole(cat.Roles, id)
	if !ok {
		return fallback
	}
	spec := fallback
	if role.Code != "" {
		spec.code = role.Code
	}
	if role.Name != "" {
		spec.name = role.Name
	}
	spec.allowedTools = RoleAllowlist(role)
	spec.modelSlot = role.ModelSlot
	spec.mode = role.Mode
	return spec
}

func (l *Loop) loopCatalog() bundles.Catalog {
	if l == nil {
		return bundles.Catalog{}
	}
	if len(l.catalog.Roles) > 0 || len(l.catalog.Loops) > 0 || len(l.catalog.Policies) > 0 {
		return l.catalog
	}
	if l.eng == nil {
		return bundles.Catalog{}
	}
	cat, err := l.eng.discoverLoopCatalog()
	if err != nil {
		return bundles.Catalog{}
	}
	l.catalog = cat
	return cat
}

func (l *Loop) roleModel(rs roleSpec, fallbackModel string, fallbackFlags map[string]string) (string, map[string]string) {
	if l == nil || l.eng == nil {
		return fallbackModel, fallbackFlags
	}
	return modelForSlot(l.eng.settings(), rs, fallbackModel, fallbackFlags)
}

func modelForSlot(set domain.Settings, rs roleSpec, fallbackModel string, fallbackFlags map[string]string) (string, map[string]string) {
	slot := strings.TrimSpace(rs.modelSlot)
	if slot == "" {
		slot = strings.TrimSpace(rs.mode)
	}
	if slot == "" {
		return fallbackModel, fallbackFlags
	}
	switch strings.ToLower(slot) {
	case bundles.RoleModePlan:
		return set.PlanModel().String(), set.PlanFlags()
	case bundles.RoleModeAgent:
		return set.AgentModel().String(), set.AgentFlags()
	case bundles.RoleModeReview:
		return set.ReviewModel().String(), set.ReviewFlags()
	case bundles.RoleModeLoop:
		return set.LoopModel().String(), set.LoopFlags()
	case bundles.RoleModeAsk:
		return set.AskModel().String(), set.AskFlags()
	default:
		return fallbackModel, fallbackFlags
	}
}
