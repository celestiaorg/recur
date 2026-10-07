package engine

import (
	"strings"

	"recur/internal/bundles"
	"recur/internal/domain"
	"recur/internal/store"
)

// resolvedGuard is the policy applied to a run (catalog or autonomy fallback).
type resolvedGuard struct {
	Policy     bundles.Policy
	Loop       bundles.LoopDef
	FromBundle bool
}

func (e *Engine) discoverLoopCatalog() (bundles.Catalog, error) {
	def, err := store.BundlesDir()
	if err != nil {
		return bundles.Catalog{}, err
	}
	user, err := store.UserBundlesDir()
	if err != nil {
		return bundles.Catalog{}, err
	}
	return bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
}

// GuardReason is the refuse string when loopID/policyID cannot be engaged
// (missing, disabled, or a referenced role is disabled). Empty is OK.
func (e *Engine) GuardReason(loopID, policyID string, autonomy domain.AutonomyLevel) string {
	_, reason := e.resolveGuard(loopID, policyID, autonomy)
	return reason
}

func (e *Engine) resolveGuard(loopID, policyID string, autonomy domain.AutonomyLevel) (resolvedGuard, string) {
	loopID = strings.TrimSpace(loopID)
	policyID = strings.TrimSpace(policyID)
	settings := e.Store.Settings()
	if loopID == "" {
		loopID = strings.TrimSpace(settings.DefaultLoop)
	}
	if loopID == "" {
		loopID = bundles.DefaultLoopID
	}
	if policyID == "" {
		policyID = strings.TrimSpace(settings.DefaultPolicy)
	}
	if policyID == "" {
		policyID = bundles.PolicyFromAutonomy(string(autonomy))
	}
	if containsID(settings.DisabledBundleLoops, loopID) {
		return resolvedGuard{}, "loop " + loopID + " is disabled"
	}
	if containsID(settings.DisabledBundlePolicies, policyID) {
		return resolvedGuard{}, "policy " + policyID + " is disabled"
	}

	cat, err := e.discoverLoopCatalog()
	if err != nil {
		return resolvedGuard{}, "could not load bundles: " + err.Error()
	}
	g := resolvedGuard{
		Policy: syntheticPolicy(policyID, autonomy),
		Loop:   syntheticPRLoop(),
	}
	if p, ok := bundles.FindPolicy(cat.Policies, policyID); ok {
		g.Policy = p
		g.FromBundle = true
	}
	if lp, ok := bundles.FindLoop(cat.Loops, loopID); ok {
		if reason := e.loopRolesOK(cat, lp, settings); reason != "" {
			return resolvedGuard{}, reason
		}
		g.Loop = lp
		g.FromBundle = true
	} else if loopID == bundles.StepLoopID {
		g.Loop = syntheticStepLoop(bundles.LoopPhase{Roles: map[string]string{
			"implementer": "recur.roles.coder", "reviewer": "recur.roles.reviewer"}})
	} else if loopID != bundles.DefaultLoopID {
		return resolvedGuard{}, "loop " + loopID + " not found"
	}
	if policyID != bundles.DefaultPolicyID && policyID != "recur.policies.cautious" && policyID != "recur.policies.aggressive" {
		if _, ok := bundles.FindPolicy(cat.Policies, policyID); !ok {
			return resolvedGuard{}, "policy " + policyID + " not found"
		}
	}
	return g, ""
}

func (e *Engine) loopRolesOK(cat bundles.Catalog, lp bundles.LoopDef, settings domain.Settings) string {
	ids := map[string]bool{}
	for _, id := range lp.Roles {
		ids[id] = true
	}
	for _, ph := range lp.Phases {
		for _, id := range ph.Roles {
			ids[id] = true
		}
	}
	for id := range ids {
		if containsID(settings.DisabledBundleRoles, id) {
			return "role " + id + " is disabled"
		}
		if _, ok := bundles.FindRole(cat.Roles, id); !ok {
			if strings.HasPrefix(id, "recur.roles.") {
				continue // first-party fallback in roles.go
			}
			return "role " + id + " not found"
		}
	}
	return ""
}

func syntheticPolicy(id string, autonomy domain.AutonomyLevel) bundles.Policy {
	if id == "" {
		id = bundles.PolicyFromAutonomy(string(autonomy))
	}
	switch id {
	case "recur.policies.cautious":
		return bundles.Policy{
			ID: id, Name: "Cautious", MaxRounds: 2, OnMaxRounds: "escalate",
			FindingsThreshold: bundles.FindingsThreshold{AllZero: true},
			PlanApproval:      "always", ExtendRounds: 1, OnFail: "fail",
		}
	case "recur.policies.aggressive":
		return bundles.Policy{
			ID: id, Name: "Aggressive", MaxRounds: 6, OnMaxRounds: "proceed",
			FindingsThreshold: bundles.FindingsThreshold{Levels: map[string]int{"critical": 0, "high": 0}},
			PlanApproval:      "never", ExtendRounds: 3, OnFail: "fail",
		}
	default:
		return bundles.Policy{
			ID: bundles.DefaultPolicyID, Name: "Balanced", MaxRounds: 4, OnMaxRounds: "escalate",
			FindingsThreshold: bundles.FindingsThreshold{Levels: map[string]int{"critical": 0, "high": 0, "medium": 0}},
			PlanApproval:      "if_structured", ExtendRounds: 2, OnFail: "fail",
		}
	}
}

// guardForWorkspace resolves the run's guard. A fan-out child does not walk a
// bundle loop: its loop is derived from the parent's converge phase (same role
// slots, prompt slots, and phase overrides) so a Docs loop fans out into
// Writers and Editors and a custom loop's children behave exactly like its
// converge phase would have inline. Everything else resolves by id.
func (e *Engine) guardForWorkspace(ws domain.Workspace) (resolvedGuard, string) {
	if strings.TrimSpace(ws.ParentID) == "" {
		return e.resolveGuard(ws.LoopID, ws.PolicyID, ws.Autonomy)
	}
	parent, ok := e.Store.Workspace(ws.ParentID)
	if !ok {
		return e.resolveGuard(ws.LoopID, ws.PolicyID, ws.Autonomy)
	}
	g, reason := e.resolveGuard(parent.LoopID, ws.PolicyID, ws.Autonomy)
	if reason != "" {
		return resolvedGuard{}, reason
	}
	ph, ok := g.Loop.PhaseByID(strings.TrimSpace(ws.ParentPhaseID))
	if !ok || ph.Uses != bundles.VerbConverge {
		for _, p := range g.Loop.Phases {
			if p.Uses == bundles.VerbConverge && p.Dispatch != "on_demand" {
				ph, ok = p, true
				break
			}
		}
	}
	if !ok {
		ph = bundles.LoopPhase{Roles: map[string]string{
			"implementer": "recur.roles.coder", "reviewer": "recur.roles.reviewer"}}
	}
	g.Loop = childLoopDef(g.Loop, ph)
	return g, ""
}

// childLoopDef is the parent converge phase as a two-phase loop: converge (the
// parent's roles, prompts, rounds, bar, cap, timeout, retries, on_fail — with
// start/gate/dispatch cleared so the child always implements first), then an
// artifact phase so it never opens a PR. The parent merges it back.
func childLoopDef(parent bundles.LoopDef, ph bundles.LoopPhase) bundles.LoopDef {
	conv := ph
	conv.ID, conv.Uses, conv.Next = "converge", bundles.VerbConverge, "done"
	conv.Dispatch, conv.When, conv.Gate, conv.Start, conv.OnReject, conv.Optional = "", "", "", "", "", false
	roles := map[string]string{}
	for k, v := range parent.Roles {
		roles[k] = v
	}
	if roles["conductor"] == "" {
		roles["conductor"] = "recur.roles.maestro"
	}
	return bundles.LoopDef{
		ID: bundles.StepLoopID, Name: strings.TrimSpace(parent.Name) + " step", Entry: "converge",
		Reenter: []string{"converge"}, Roles: roles,
		Prompts: map[string]string{"mission": "You conduct one child workspace that owns a slice of a parent plan. " +
			"It implements and is reviewed under the parent's policy; the parent merges this branch back and " +
			"reviews the aggregate diff. Answer where the slice stands; do not widen scope beyond its steps."},
		Phases:     []bundles.LoopPhase{conv, {ID: "done", Uses: bundles.VerbArtifact}},
		BundleName: parent.BundleName, BundleID: parent.BundleID, SourceRoot: parent.SourceRoot, BundleDir: parent.BundleDir,
	}
}

// syntheticStepLoop is the child loop when the parent cannot be consulted
// (guard validation at create time): coder + reviewer converge, then artifact.
func syntheticStepLoop(ph bundles.LoopPhase) bundles.LoopDef {
	return childLoopDef(bundles.LoopDef{Name: "Plan"}, ph)
}

func syntheticPRLoop() bundles.LoopDef {
	return bundles.LoopDef{
		ID: bundles.DefaultLoopID, Name: "To a PR", Entry: "planning",
		Reenter: []string{"planning", "converge"},
		Roles:   map[string]string{"conductor": "recur.roles.maestro"},
		Phases: []bundles.LoopPhase{
			{ID: "planning", Uses: bundles.VerbPlan, Gate: "plan_approval", Next: "converge",
				Roles: map[string]string{"planner": "recur.roles.planner"}},
			{ID: "converge", Uses: bundles.VerbConverge, Next: "pr",
				Roles: map[string]string{"implementer": "recur.roles.coder", "reviewer": "recur.roles.reviewer"}},
			{ID: "pr", Uses: bundles.VerbEnsurePR},
		},
	}
}

func containsID(ids []string, id string) bool {
	for _, x := range ids {
		if x == id {
			return true
		}
	}
	return false
}

func (e *Engine) openFindingsCounts(workspaceID string) map[string]int {
	counts := map[string]int{}
	for _, f := range e.Store.Findings(workspaceID) {
		if f.Status != "open" {
			continue
		}
		lv := strings.ToLower(strings.TrimSpace(string(f.Severity)))
		if lv == "" {
			lv = "medium"
		}
		counts[lv]++
	}
	return counts
}

func (e *Engine) findingsBarBlocks(workspaceID string, th bundles.FindingsThreshold) (bool, string) {
	counts := e.openFindingsCounts(workspaceID)
	if !bundles.FindingsBlock(th, counts) {
		return false, ""
	}
	n := countableFindings(th, counts)
	return true, plural(n, "countable finding", "countable findings")
}

func countableFindings(th bundles.FindingsThreshold, counts map[string]int) int {
	n := 0
	if th.AllZero {
		for _, c := range counts {
			n += c
		}
		return n
	}
	for lv := range th.Levels {
		n += counts[lv]
	}
	return n
}

func (g resolvedGuard) phaseThreshold(ph bundles.LoopPhase) bundles.FindingsThreshold {
	pol := g.Policy.FindingsThreshold
	if ph.Findings == nil {
		return pol
	}
	if err := bundles.SameOrStricter(*ph.Findings, pol); err != nil {
		return pol
	}
	return *ph.Findings
}

func (g resolvedGuard) phaseMaxRounds(ph bundles.LoopPhase) int {
	if ph.MaxRounds != nil && *ph.MaxRounds > 0 {
		return *ph.MaxRounds
	}
	if g.Policy.MaxRounds > 0 {
		return g.Policy.MaxRounds
	}
	return 4
}

func (g resolvedGuard) phaseOnMaxRounds(ph bundles.LoopPhase) string {
	if ph.OnMaxRounds != "" {
		return ph.OnMaxRounds
	}
	if g.Policy.OnMaxRounds != "" {
		return g.Policy.OnMaxRounds
	}
	return "escalate"
}

func skipPlanApproval(pol bundles.Policy, ws domain.Workspace) bool {
	if ws.Plan != nil && ws.Plan.Approved {
		return true
	}
	switch pol.PlanApproval {
	case "never":
		return true
	case "always":
		return false
	default: // if_structured
		return ws.Plan == nil || len(ws.Plan.Steps) == 0
	}
}
