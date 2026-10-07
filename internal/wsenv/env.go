// Package wsenv builds the per-workspace environment Recur injects into setup,
// teardown, dev_cmd, verify boots, and Observatory terminals.
package wsenv

import (
	"os"
	"strconv"
	"strings"

	"recur/internal/domain"
)

// Vars is the workspace-scoped environment Recur owns. Project.Env (dotenv text)
// values may reference these via $NAME / ${NAME}, plus host vars and keys from
// .env files next to the project / worktree.
type Vars struct {
	Port         int
	WorkspaceID  string
	WorktreePath string
	RepoPath     string // project checkout; .env files are loaded from here too
	ProjectID    string
	Branch       string            // workspace git branch; injected as $RECUR_BRANCH when set
	ProjectEnv   map[string]string // parsed from project Env dotenv text
}

// FromWorkspace builds Vars from a stored workspace + its project.
func FromWorkspace(ws domain.Workspace, proj domain.Project) Vars {
	return Vars{
		Port:         ws.Port,
		WorkspaceID:  ws.ID,
		WorktreePath: ws.WorktreePath,
		RepoPath:     proj.RepoPath,
		ProjectID:    ws.ProjectID,
		Branch:       strings.TrimSpace(ws.Branch),
		ProjectEnv:   projectEnv(proj),
	}
}

func projectEnv(proj domain.Project) map[string]string {
	env := ParseDotenv(proj.Env)
	if proj.HasProjectFile {
		return dropProtectedEnv(env)
	}
	return env
}

// RecurMap returns the RECUR_* (and PORT) bindings for expansion / injection
// ($RECUR_WORKSPACE_ID, $RECUR_PROJECT_ID, $RECUR_WORKTREE, $RECUR_BRANCH,
// $RECUR_PORT / $PORT). RECUR_BRANCH is always present so $RECUR_BRANCH
// expands to empty instead of remaining as a literal.
func (v Vars) RecurMap() map[string]string {
	m := map[string]string{
		"RECUR_WORKSPACE_ID": v.WorkspaceID,
		"RECUR_PROJECT_ID":   v.ProjectID,
		"RECUR_WORKTREE":     v.WorktreePath,
		"RECUR_BRANCH":       strings.TrimSpace(v.Branch),
	}
	if v.Port > 0 {
		p := strconv.Itoa(v.Port)
		m["RECUR_PORT"] = p
		m["PORT"] = p
	}
	return m
}

// Environ returns base merged with workspace RECUR_* vars and the project's
// Env map (values expanded against Recur vars, .env keys, and the host
// environment). Later sources win on key conflict: host → Recur vars →
// project Env (those keys stripped only when env came from project.toml).
// Gitignored .env files are read for that expansion; their keys are not
// merged into the returned environ.
func (v Vars) Environ(base []string) []string {
	if base == nil {
		base = os.Environ()
	}
	recur := v.RecurMap()
	files := dropProtectedEnv(LoadDotenvFiles(v.RepoPath, v.WorktreePath))
	files = expandMap(files, recur, nil)
	out := mergeEnv(base, recur)
	if len(v.ProjectEnv) == 0 {
		return out
	}
	expanded := expandMap(v.ProjectEnv, recur, files)
	return mergeEnv(out, expanded)
}

// protectedEnvKey is true for process-critical names a repo .env or
// project.toml env must not override (PATH/HOME/SHELL and loader preload
// paths). Recur's own RECUR_* / PORT bindings are reserved separately in
// Environ. Keys the user types into Recur's Environment field (no project.toml)
// are not stripped.
func protectedEnvKey(k string) bool {
	u := strings.ToUpper(strings.TrimSpace(k))
	switch u {
	case "PATH", "HOME", "SHELL":
		return true
	}
	return strings.HasPrefix(u, "LD_") || strings.HasPrefix(u, "DYLD_")
}

func dropProtectedEnv(in map[string]string) map[string]string {
	if len(in) == 0 {
		return in
	}
	out := make(map[string]string, len(in))
	for k, v := range in {
		if protectedEnvKey(k) {
			continue
		}
		out[k] = v
	}
	return out
}

// ExpandKnown substitutes $KEY / ${KEY} using vars first, then the process
// environment (so $HOME works in project env values). Unknown references are
// left as literal text so a typo doesn't silently become empty.
func ExpandKnown(s string, vars map[string]string) string {
	return expandKnown(s, vars, nil)
}

func expandKnown(s string, vars, extra map[string]string) string {
	if s == "" {
		return s
	}
	return os.Expand(s, func(key string) string {
		if v, ok := vars[key]; ok {
			return v
		}
		if v, ok := extra[key]; ok {
			return v
		}
		if v, ok := os.LookupEnv(key); ok {
			return v
		}
		// Preserve the reference for the shell / later expansion.
		return "$" + key
	})
}

func expandMap(in map[string]string, recur, extra map[string]string) map[string]string {
	if len(in) == 0 {
		return in
	}
	rawByKey := make(map[string]string, len(in))
	pass1 := make(map[string]string, len(in))
	for k, raw := range in {
		k = strings.TrimSpace(k)
		if k == "" {
			continue
		}
		rawByKey[k] = raw
		pass1[k] = expandKnown(raw, recur, extra)
	}
	// Expand the original text against sibling pass-1 values, not the pass-1
	// output. Re-expanding substituted Recur/env values would interpolate
	// `$FOO` inside a git branch or worktree path.
	lookup := mergeMaps(extra, pass1)
	out := make(map[string]string, len(pass1))
	for k, raw := range rawByKey {
		out[k] = expandKnown(raw, recur, lookup)
	}
	return out
}

func mergeMaps(base, overlay map[string]string) map[string]string {
	if len(overlay) == 0 {
		return base
	}
	if len(base) == 0 {
		return overlay
	}
	out := make(map[string]string, len(base)+len(overlay))
	for k, v := range base {
		out[k] = v
	}
	for k, v := range overlay {
		out[k] = v
	}
	return out
}

// mergeEnv copies base and overlays vals (KEY=VALUE), replacing existing keys.
func mergeEnv(base []string, vals map[string]string) []string {
	if len(vals) == 0 {
		out := make([]string, len(base))
		copy(out, base)
		return out
	}
	drop := make(map[string]bool, len(vals))
	for k := range vals {
		drop[k] = true
	}
	out := make([]string, 0, len(base)+len(vals))
	for _, kv := range base {
		if i := strings.IndexByte(kv, '='); i > 0 && drop[kv[:i]] {
			continue
		}
		out = append(out, kv)
	}
	for k, v := range vals {
		out = append(out, k+"="+v)
	}
	return out
}
