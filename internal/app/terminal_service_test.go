package app

import (
	"encoding/base64"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

// The terminal must land in the SAME tree the code browser is showing. A shell
// that opens in the project checkout while the editor beside it is showing a
// loop's worktree would run every command against the wrong branch — which is the
// one mistake this dock has to be incapable of making.
func TestTerminalCwdPrefersTheWorktreeThenTheRepo(t *testing.T) {
	repo := t.TempDir()
	worktree := t.TempDir()

	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	tests := []struct {
		name         string
		workspaceID  string
		worktreePath string
		want         string
		wantErr      bool
	}{
		{
			name:         "live worktree wins",
			workspaceID:  "ws-worktree",
			worktreePath: worktree,
			want:         worktree,
		},
		{
			name:        "no worktree recorded falls back to the project checkout",
			workspaceID: "ws-no-worktree",
			want:        repo,
		},
		{
			name:         "worktree removed from disk does not fall back to the project checkout",
			workspaceID:  "ws-stale-worktree",
			worktreePath: filepath.Join(worktree, "archived-and-deleted"),
			wantErr:      true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if _, err := eng.Store.AddWorkspace(domain.Workspace{
				ID:           tt.workspaceID,
				ProjectID:    projectID,
				WorktreePath: tt.worktreePath,
			}); err != nil {
				t.Fatalf("AddWorkspace: %v", err)
			}
			got, err := svc.Terminal.terminalCwd(tt.workspaceID)
			if tt.wantErr {
				if err == nil {
					t.Fatalf("terminalCwd(%q) = %q, want error (must not use the project checkout)", tt.workspaceID, got)
				}
				return
			}
			if err != nil {
				t.Fatalf("terminalCwd: %v", err)
			}
			// The resolver returns an absolute, symlink-resolved path; t.TempDir on
			// macOS hands back /var/... which is a symlink to /private/var.
			if !sameDir(t, got, tt.want) {
				t.Errorf("terminalCwd = %q, want %q", got, tt.want)
			}
		})
	}
}

func TestTerminalCwdRejectsUnknownWorkspace(t *testing.T) {
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)

	if _, err := svc.Terminal.terminalCwd("nope"); err == nil {
		t.Fatal("terminalCwd on an unknown workspace succeeded, want error")
	} else if !strings.Contains(err.Error(), "nope") {
		t.Errorf("error %q should name the missing workspace", err)
	}
}

// OpenTerminal must refuse rather than silently starting a shell somewhere
// arbitrary when neither the worktree nor the project path resolves.
func TestOpenTerminalFailsWithoutAResolvableDirectory(t *testing.T) {
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)

	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-nowhere", ProjectID: "ghost"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if _, err := svc.Terminal.OpenTerminal("ws-nowhere", 80, 24); err == nil {
		t.Fatal("OpenTerminal with no resolvable directory succeeded, want error")
	}
}

// A round trip through the bound surface: open a shell, attach, run a command,
// and read it back out of the replay buffer. This is the contract the dock relies
// on when it re-adopts sessions after the drawer has been closed.
func TestOpenAttachAndReplayThroughTheBoundService(t *testing.T) {
	repo := t.TempDir()
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "ws-1", ProjectID: projectID}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	info, err := svc.Terminal.OpenTerminal("ws-1", 100, 30)
	if err != nil {
		t.Fatalf("OpenTerminal: %v", err)
	}
	t.Cleanup(func() { _ = svc.Terminal.CloseTerminal(info.ID) })

	if got := svc.Terminal.ListTerminals("ws-1"); len(got) != 1 || got[0].ID != info.ID {
		t.Fatalf("ListTerminals = %v, want the one session just opened", got)
	}

	if _, err := svc.Terminal.AttachTerminal(info.ID); err != nil {
		t.Fatalf("AttachTerminal: %v", err)
	}
	if err := svc.Terminal.WriteTerminal(info.ID, "echo bound-service-marker\n"); err != nil {
		t.Fatalf("WriteTerminal: %v", err)
	}
	if err := svc.Terminal.ResizeTerminal(info.ID, 120, 40); err != nil {
		t.Fatalf("ResizeTerminal: %v", err)
	}

	// Detach and re-attach to read the replay buffer, which is where the output
	// the drawer missed has to be waiting.
	waitForReplay(t, svc, info.ID, "bound-service-marker")

	if err := svc.Terminal.ClearTerminal(info.ID); err != nil {
		t.Fatalf("ClearTerminal: %v", err)
	}
	if err := svc.Terminal.DetachTerminal(info.ID); err != nil {
		t.Fatalf("DetachTerminal: %v", err)
	}
	replay, err := svc.Terminal.AttachTerminal(info.ID)
	if err != nil {
		t.Fatalf("AttachTerminal after clear: %v", err)
	}
	if decoded := decodeB64(t, replay); strings.Contains(decoded, "bound-service-marker") {
		t.Error("replay still holds output the user cleared")
	}
}

// ---- helpers ----

func sameDir(t *testing.T, a, b string) bool {
	t.Helper()
	ra, err := filepath.EvalSymlinks(a)
	if err != nil {
		return false
	}
	rb, err := filepath.EvalSymlinks(b)
	if err != nil {
		return false
	}
	return ra == rb
}

func decodeB64(t *testing.T, s string) string {
	t.Helper()
	b, err := base64.StdEncoding.DecodeString(s)
	if err != nil {
		t.Fatalf("decode replay: %v", err)
	}
	return string(b)
}

// waitForReplay polls the replay buffer until want shows up. Detach/attach is the
// only way to read it, and it is idempotent, so polling that pair is safe.
func waitForReplay(t *testing.T, svc *Services, id, want string) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if err := svc.Terminal.DetachTerminal(id); err != nil {
			t.Fatalf("DetachTerminal: %v", err)
		}
		replay, err := svc.Terminal.AttachTerminal(id)
		if err != nil {
			t.Fatalf("AttachTerminal: %v", err)
		}
		if strings.Contains(decodeB64(t, replay), want) {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %q in the replay buffer", want)
}
