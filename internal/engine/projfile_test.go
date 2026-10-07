package engine

import (
	"strings"
	"testing"

	"recur/internal/domain"
	"recur/internal/projfile"
	"recur/internal/store"
)

// addRunProject writes run-config to .recur/project.toml when present, then
// AddProject. Overlay now treats that file as the only source of run-config,
// so tests that pass SetupCmd/RunCommands without a file would otherwise get empty
// commands back from List/Get/CreateWorkspace.
func addRunProject(t *testing.T, st *store.Store, p domain.Project) domain.Project {
	t.Helper()
	if repo := strings.TrimSpace(p.RepoPath); repo != "" && hasRunConfig(p) {
		if err := projfile.Write(p); err != nil {
			t.Fatalf("projfile.Write(%q): %v", repo, err)
		}
	}
	saved, err := st.AddProject(p)
	if err != nil {
		t.Fatalf("AddProject(%q): %v", p.ID, err)
	}
	return saved
}

func hasRunConfig(p domain.Project) bool {
	return p.SetupCmd != "" || p.TeardownCmd != "" || p.AppURL != "" || p.Env != "" || p.CopyFiles != "" || len(p.RunCommands) > 0
}
