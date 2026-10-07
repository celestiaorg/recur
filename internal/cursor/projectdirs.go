package cursor

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// cursorProjectSlugRe matches Cursor's project-dir sanitizer in @cursor/sdk:
// replace /[^a-zA-Z0-9]/g with "-", collapse dashes, trim edge dashes.
var (
	cursorProjectSlugRe  = regexp.MustCompile(`[^a-zA-Z0-9]+`)
	cursorProjectDashRun = regexp.MustCompile(`-+`)
)

// CursorProjectSlug is the directory name under ~/.cursor/projects/ for a
// workspace path. Must stay byte-compatible with the SDK — agents are told to
// look at ~/.cursor/projects/<slug>/terminals for prior shell output.
func CursorProjectSlug(workspace string) string {
	workspace = strings.TrimSpace(workspace)
	if workspace == "" {
		return ""
	}
	s := cursorProjectSlugRe.ReplaceAllString(workspace, "-")
	s = cursorProjectDashRun.ReplaceAllString(s, "-")
	return strings.Trim(s, "-")
}

// cursorProjectRoot returns ~/.cursor/projects/<slug> for workspace, or "" when
// the path cannot be resolved safely.
func cursorProjectRoot(workspace string) (string, error) {
	slug := CursorProjectSlug(workspace)
	if slug == "" || slug == "." || strings.Contains(slug, "..") {
		return "", nil
	}
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return "", err
	}
	projects := filepath.Join(home, ".cursor", "projects")
	root := filepath.Join(projects, slug)
	// Refuse anything that escapes ~/.cursor/projects (symlink / Clean tricks).
	rel, err := filepath.Rel(projects, root)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", fmt.Errorf("cursor project path %q escapes projects dir", root)
	}
	if rel == "." {
		return "", nil
	}
	return root, nil
}

// EnsureCursorProjectDirs creates the Cursor project dirs agents commonly pass
// as Shell working_directory — especially `terminals/`.
//
// Node's child_process.spawn reports a missing cwd as
// `Error: spawn /bin/zsh ENOENT` (path is the executable, not the directory).
// Cursor's ZshState does not catch that, so the exception kills the whole
// cursor-sdk-bridge and leaves the agent with a stuck active run. Recur-only
// worktrees never get these dirs from the SDK, so Recur mkdir's them when
// the bridge starts.
func EnsureCursorProjectDirs(workspace string) error {
	root, err := cursorProjectRoot(workspace)
	if err != nil || root == "" {
		return err
	}
	for _, name := range []string{"terminals", "agent-transcripts"} {
		dir := filepath.Join(root, name)
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return err
		}
	}
	return nil
}

// RemoveCursorProjectDirs deletes the whole ~/.cursor/projects/<slug>/ tree for
// a worktree (terminals, agent-transcripts, sdk-agent-store, …). Called when
// Recur removes the worktree on archive/delete so Cursor metadata does not
// accumulate for discarded loops. Missing dirs are fine (idempotent).
func RemoveCursorProjectDirs(workspace string) error {
	root, err := cursorProjectRoot(workspace)
	if err != nil || root == "" {
		return err
	}
	if err := os.RemoveAll(root); err != nil && !os.IsNotExist(err) {
		return err
	}
	return nil
}
