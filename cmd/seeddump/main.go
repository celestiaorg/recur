// Command seeddump builds a seeded in-memory store and prints the bound-service
// data as one JSON object on stdout. It generates the fixture the Playwright e2e
// suite feeds through its mocked Wails bridge (frontend/e2e/fixtures/seed.json), so
// the UI tests run against the REAL seed shapes without a live Go backend.
//
// Regenerate the fixture after changing the seed or domain types:
//
//	go run ./cmd/seeddump > frontend/e2e/fixtures/seed.json
//
// (or `pnpm run e2e:seed` from frontend/).
package main

import (
	"encoding/json"
	"os"

	"recur/internal/store"
)

func main() {
	// Throwaway HOME so Projects() overlay cannot read ~/code/*/.recur/project.toml —
	// demo repo paths expand into this empty tree, so run-config stays empty.
	// NewInMemory holds the mock dataset without touching the user's Recur DB.
	tmp, err := os.MkdirTemp("", "seeddump")
	if err != nil {
		panic(err)
	}
	defer os.RemoveAll(tmp)
	_ = os.Setenv("HOME", tmp)
	_ = os.Setenv("USERPROFILE", tmp)

	st := store.NewInMemory()

	// Demo repo paths aren't real checkouts (HOME is empty), so IsRepo is
	// false. The live ListProjects service stamps IsGit from disk; e2e
	// treats the seed projects as git repos (⌘N picker, a11y-keys).
	projects := st.Projects()
	for i := range projects {
		projects[i].IsGit = true
	}

	out := map[string]any{
		"projects": projects,
		"loops":    st.Workspaces(),
		"prs":      st.PRs(),
		"inbox":    st.Inbox(),
		"policy":   st.Policy(),
		"settings": st.Settings(),
		"resource": st.Resource(),
	}

	// Per-id lookups for the detail screens (GetWorkspace / GetWorkspaceFeed / ListLoopAgents /
	// GetEscalation / GetPR).
	loopByID := map[string]any{}
	feedByLoop := map[string]any{}
	agentsByLoop := map[string]any{}
	escByLoop := map[string]any{}
	for _, l := range st.Workspaces() {
		if lp, ok := st.Workspace(l.ID); ok {
			loopByID[l.ID] = lp
		}
		feedByLoop[l.ID] = st.Feed(l.ID)
		agentsByLoop[l.ID] = st.Agents(l.ID)
		if e, ok := st.Escalation(l.ID); ok {
			escByLoop[l.ID] = e
		}
	}
	prByID := map[string]any{}
	for _, p := range st.PRs() {
		if pr, ok := st.PR(p.ID); ok {
			prByID[p.ID] = pr
		}
	}
	out["loopById"] = loopByID
	out["feedByLoop"] = feedByLoop
	out["agentsByLoop"] = agentsByLoop
	out["escByLoop"] = escByLoop
	out["prById"] = prByID

	enc := json.NewEncoder(os.Stdout)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(out); err != nil {
		panic(err)
	}
}
