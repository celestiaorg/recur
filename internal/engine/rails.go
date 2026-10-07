package engine

import (
	"strconv"

	"recur/internal/domain"
)

// CanOpenPR is the policy findings bar at the PR door. It uses the workspace's
// selected [[policy]] when one is stamped, otherwise
// the autonomy → shipped-policy map (Balanced: medium+). A leftover low finding
// is allowed under Balanced. Always returns a human reason when blocked.
func (e *Engine) CanOpenPR(workspaceID string) (ok bool, reason string) {
	th := syntheticPolicy("", domain.AutonomyBalanced).FindingsThreshold
	if ws, found := e.Store.Workspace(workspaceID); found {
		g, reason := e.resolveGuard(ws.LoopID, ws.PolicyID, ws.Autonomy)
		if reason != "" {
			return false, reason + " — cannot evaluate the policy bar"
		}
		th = g.Policy.FindingsThreshold
	}
	if blocked, why := e.findingsBarBlocks(workspaceID, th); blocked {
		return false, why + " — policy bar blocks opening a PR"
	}
	return true, ""
}

// plural formats n with the singular or plural noun.
func plural(n int, one, many string) string {
	if n == 1 {
		return "1 " + one
	}
	return strconv.Itoa(n) + " " + many
}
