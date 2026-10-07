package engine

import (
	"testing"
	"time"

	"recur/internal/domain"
)

// TestEveryToolBearingTurnCanAskPermission is the invariant that keeps
// permissionArgs' dontAsk fallback unreachable in production.
//
// With BypassPermissions off, a turn runs with --permission-mode default, and
// Claude Code reaches the human through a prompt tool served by that turn's own
// Recur MCP endpoint. A turn that can call tools but has no endpoint has
// nowhere to ask, so the runner downgrades it to dontAsk and its tool calls are
// silently auto-denied — the exact failure this whole change exists to remove.
// It is invisible at the call site (one missing MCPURL among a dozen AgentSpec
// literals), so it is asserted over every turn a real loop produces.
func TestEveryToolBearingTurnCanAskPermission(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{1, 0}}
	eng, st, _, _ := newTestEngine(t, runner)

	set := st.Settings()
	set.BypassPermissions = false
	st.SetSettings(set)

	projID := addProject(t, st, repo)
	id, err := eng.StartWorkspace(projID, "Build the feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to settle at pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	specs := runner.capturedSpecs()
	// Assert the loop actually produced the turns being checked, so a harness
	// change that stops capturing cannot make this pass vacuously.
	seen := map[domain.AgentRole]bool{}
	for _, s := range specs {
		seen[s.Role] = true
	}
	for _, want := range []domain.AgentRole{domain.RoleCoder, domain.RoleReviewer} {
		if !seen[want] {
			t.Fatalf("expected the loop to run a %s turn; captured roles: %v", want, seen)
		}
	}

	for _, s := range specs {
		// A turn with no tools has no permission to request, so it is exempt
		// (and pins dontAsk deliberately).
		if s.Tools != nil && *s.Tools == "" {
			continue
		}
		if s.PermissionMode != domain.PermissionModeAsk {
			t.Errorf("%s turn has PermissionMode %q; permissions are global, so it should follow Settings",
				s.Role, s.PermissionMode)
			continue
		}
		if s.MCPURL == "" {
			t.Errorf("%s turn can call tools but has no MCP endpoint — its permission prompts "+
				"would be auto-denied instead of reaching the human", s.Role)
		}
	}
}

// A loop only exercises the autopilot roles, so the chat modes are checked
// directly: each still needs an MCP endpoint, including Ask, which allowlists
// no Recur tools and so looks like it needs none.
func TestEveryChatModeCanAskPermission(t *testing.T) {
	set := domain.Settings{BypassPermissions: false}
	for _, mode := range []string{"ask", "plan", "review", "agent", ""} {
		shape := chatModeShape(mode, set, false)
		if shape.permission != domain.PermissionModeAsk {
			t.Errorf("chat mode %q permission = %q; permissions are global, so it should follow Settings",
				mode, shape.permission)
		}
		if !shape.mcp {
			t.Errorf("chat mode %q has no MCP endpoint, so its permission prompts cannot reach the human", mode)
		}
	}

	// And with bypass on, every mode skips checks — no mode opts itself out.
	on := domain.Settings{BypassPermissions: true}
	for _, mode := range []string{"ask", "plan", "review", "agent"} {
		if got := chatModeShape(mode, on, false).permission; got != domain.PermissionModeBypass {
			t.Errorf("chat mode %q with bypass on = %q, want bypassPermissions", mode, got)
		}
	}
}
