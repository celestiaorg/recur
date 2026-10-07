package engine

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/mcp"
)

// TestRealPermissionPromptE2E drives a REAL `claude -p` through Recur's own MCP
// server with BypassPermissions off, and answers the permission request from a
// goroutine standing in for the human.
//
// The --permission-prompt-tool contract is undocumented in `claude --help`, and
// nothing else in the suite can catch it drifting: every other permission test
// runs against a fake runner, so a CLI change to the request shape, the required
// updatedInput field, or the exemption from --allowedTools would leave a green
// suite and a product where agents hang or get silently denied. SKIPPED unless
// RECUR_E2E=1 (spends credit).
func TestRealPermissionPromptE2E(t *testing.T) {
	if os.Getenv("RECUR_E2E") != "1" {
		t.Skip("set RECUR_E2E=1 to run the real permission-prompt E2E (spends credit)")
	}
	if _, err := exec.LookPath("claude"); err != nil {
		t.Skip("claude not on PATH")
	}

	asked := make(chan mcp.PermissionAsk, 4)
	srv := mcp.New(&promptReporter{asked: asked})
	if err := srv.Start(); err != nil {
		t.Fatalf("start MCP: %v", err)
	}
	defer srv.Close()
	_, mcpURL := srv.Register("W-E2E", "CO", nil)

	dir := t.TempDir()
	target := filepath.Join(dir, "approved.txt")

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()

	// Write is deliberately NOT allowlisted: an allowlisted tool is auto-approved
	// and never reaches the prompt tool, so this is what forces a real request.
	res, err := (&ClaudeRunner{}).Run(ctx, AgentSpec{
		WorkspaceID:    "W-E2E",
		AgentID:        "W-E2E-CO",
		Model:          "haiku", // bare id: provider routing is upstream of the runner
		Prompt:         "Use the Write tool to create the file " + target + " containing exactly: ok",
		Worktree:       dir,
		AllowedTools:   []string{"Read"},
		PermissionMode: domain.Settings{BypassPermissions: false}.PermissionMode(),
		MCPURL:         mcpURL,
		MaxTurns:       4,
	}, nil)
	if err != nil {
		t.Fatalf("claude run: %v", err)
	}
	select {
	case <-asked:
	default:
		// Distinguish contract drift from a run that never got far enough to ask.
		t.Fatalf("claude never called the permission prompt tool — the flag or its "+
			"tool name has drifted, and turns would run unsupervised or hang "+
			"(result: isError=%v subtype=%s text=%q)", res.IsError, res.Subtype, res.Text)
	}
	// The approval has to actually let the tool run: an allow whose updatedInput
	// is malformed is rejected by the CLI and looks like a denial.
	if _, err := os.Stat(target); err != nil {
		t.Errorf("approved Write did not happen: %v", err)
	}
}

// promptReporter stands in for the human and approves everything. Which tool
// the model reaches for (Write vs a Bash redirect) is its choice and not what
// this test is pinning down; the deny shape is covered by the unit tests.
type promptReporter struct {
	mcp.Reporter
	asked chan mcp.PermissionAsk
}

func (r *promptReporter) RequestPermission(_ context.Context, _, _ string, ask mcp.PermissionAsk) (mcp.PermissionVerdict, error) {
	select {
	case r.asked <- ask:
	default:
	}
	return mcp.PermissionVerdict{Allow: true}, nil
}
