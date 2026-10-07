package engine

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"recur/internal/codex"
	"recur/internal/domain"
)

// Live Codex app-server smoke. Requires a native binary and auth:
//
//	RECUR_CODEX_E2E=1 RECUR_CODEX=/path/to/codex go test ./internal/engine -run CodexE2E -count=1
func TestCodexE2ESmoke(t *testing.T) {
	if os.Getenv("RECUR_CODEX_E2E") != "1" {
		t.Skip("set RECUR_CODEX_E2E=1 to run live Codex smoke")
	}
	// Prefer $RECUR_CODEX; Locate rejects the broken npm JS shim.
	bin, err := codex.Locate("")
	if err != nil {
		t.Fatalf("locate codex: %v — install `brew install --cask codex` or set RECUR_CODEX to the native binary (full package layout, not a lone copied file)", err)
	}
	wt := t.TempDir()
	if err := exec.Command("git", "init", wt).Run(); err != nil {
		t.Fatal(err)
	}
	r := &CodexRunner{
		Bin:         bin,
		ChatGPTAuth: true,
		Home:        t.TempDir(),
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()

	var steps int
	res, err := r.Run(ctx, AgentSpec{
		Worktree:     wt,
		Model:        "gpt-5.6-luna",
		Role:         domain.RoleCoder,
		SystemPrompt: "You are a concise coding agent.",
		Prompt:       "Create a file named hello-recur.txt containing exactly: ok\nThen stop. Do not do anything else.",
	}, func(ev ParsedEvent) {
		if ev.Kind == KindStep {
			steps++
		}
	})
	if err != nil {
		t.Fatalf("Run: %v\nstderr: %s", err, res.Stderr)
	}
	if res.SessionID == "" {
		t.Fatal("empty session/thread id")
	}
	if res.DurationMS <= 0 {
		t.Fatalf("DurationMS = %d, want > 0 so the chat turn footer (and findings digest) can render", res.DurationMS)
	}
	body, err := os.ReadFile(filepath.Join(wt, "hello-recur.txt"))
	if err != nil {
		t.Fatalf("expected worktree edit: %v (text=%q steps=%d)", err, res.Text, steps)
	}
	if string(body) != "ok\n" && string(body) != "ok" {
		t.Fatalf("file body %q", body)
	}
	// Isolation: no .codex dir in the worktree.
	if _, err := os.Stat(filepath.Join(wt, ".codex")); err == nil {
		t.Fatal("worktree must not contain .codex/")
	}
}
