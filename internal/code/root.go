// Package code implements the observatory code browser: tree, read/write, search,
// outline, and heuristic symbol index. Paths are always validated under a root
// (project repo or loop worktree) so the UI cannot escape the sandbox.
package code

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	gitpkg "recur/internal/git"
)

// ErrNotFound marks a path that genuinely doesn't exist under root (deleted,
// renamed, or never existed) as opposed to a transient I/O failure. The
// Observatory's live-refresh polling (every ~600ms while an agent works) needs
// this distinction: an actual deletion should close the tab / drop the dir
// from the tree, but a momentary error (e.g. the engine mid-rewrite of the
// worktree) must not. Go callers can use errors.Is(err, ErrNotFound); a Wails
// binding error only survives the trip to the frontend as its Error()
// string, so notFoundError below wraps it in a stable "not found: <rel>"
// form the frontend sniffs for instead.
var ErrNotFound = errors.New("not found")

// ErrWorktreeMissing marks a workspace whose recorded WorktreePath is no
// longer a directory (removed, discarded, teardown racing a poll). Callers
// must not fall back to the project checkout — that would paint another
// repo's uncommitted state as this workspace's. A Wails binding error only
// survives the trip to the frontend as its Error() string, so wraps must
// keep the stable "worktree is not a directory" form the frontend sniffs
// for (see isWorktreeMissing).
var ErrWorktreeMissing = errors.New("worktree is not a directory")

// notFoundError wraps ErrNotFound with the offending path, in the stable
// "not found: <rel>" form the frontend matches on (see ErrNotFound doc).
func notFoundError(rel string) error {
	return fmt.Errorf("%w: %s", ErrNotFound, rel)
}

// ResolveRoot returns the absolute root to browse for a project/loop pair.
// When loopWorktree is non-empty and exists, it wins (loop-scoped observatory).
// When it is non-empty but missing, the result is ErrWorktreeMissing — never
// the project checkout. An empty loopWorktree uses the project's repo path.
func ResolveRoot(repoPath, loopWorktree string) (string, error) {
	root, _, err := ResolveRootInfo(repoPath, loopWorktree)
	return root, err
}

// ResolveRootInfo is ResolveRoot plus whether the root it picked is the loop
// worktree. Callers need that to decide whether editing is allowed, and the
// flag is free here: without it they had to call ResolveRoot a second time
// (another Abs + Stat, on every single code-browser binding call) just to
// re-derive an answer this function already knew.
func ResolveRootInfo(repoPath, loopWorktree string) (root string, isWorktree bool, err error) {
	if wt := strings.TrimSpace(loopWorktree); wt != "" {
		abs, err := filepath.Abs(gitpkg.ExpandHome(wt))
		if err != nil {
			return "", false, err
		}
		if st, err := os.Stat(abs); err == nil && st.IsDir() {
			return abs, true, nil
		}
		return "", false, fmt.Errorf("%w: %s", ErrWorktreeMissing, wt)
	}
	repo := strings.TrimSpace(repoPath)
	if repo == "" {
		return "", false, fmt.Errorf("no repository path")
	}
	abs, err := filepath.Abs(gitpkg.ExpandHome(repo))
	if err != nil {
		return "", false, err
	}
	if st, err := os.Stat(abs); err != nil || !st.IsDir() {
		return "", false, fmt.Errorf("repository path %q is not a directory", repo)
	}
	return abs, false, nil
}

// SafeJoin joins root + rel and ensures the result stays under root.
// rel uses forward slashes; empty rel means the root itself.
func SafeJoin(root, rel string) (string, error) {
	rootAbs, err := filepath.Abs(root)
	if err != nil {
		return "", err
	}
	rel = strings.TrimPrefix(filepath.ToSlash(rel), "/")
	rel = filepath.FromSlash(rel)
	if rel == "." || rel == "" {
		return rootAbs, nil
	}
	if strings.Contains(rel, "..") {
		return "", fmt.Errorf("path escapes root")
	}
	full := filepath.Join(rootAbs, rel)
	fullAbs, err := filepath.Abs(full)
	if err != nil {
		return "", err
	}
	prefix := rootAbs + string(os.PathSeparator)
	if fullAbs != rootAbs && !strings.HasPrefix(fullAbs, prefix) {
		return "", fmt.Errorf("path escapes root")
	}
	return fullAbs, nil
}

// RelSlash returns the forward-slash path of abs relative to root.
func RelSlash(root, abs string) string {
	rel, err := filepath.Rel(root, abs)
	if err != nil {
		return filepath.ToSlash(abs)
	}
	return filepath.ToSlash(rel)
}

// skipDir reports directories that search/symbol/metrics walks should not descend into.
// The file-tree UI does not use this — it applies user files.exclude globs instead.
func skipDir(name string) bool {
	switch name {
	case ".git", "node_modules", "dist", "build", "vendor", ".next",
		"coverage", "__pycache__", ".pytest_cache", ".mypy_cache", ".tox",
		".venv", "venv", ".turbo", ".cache", "bin", "target",
		"Pods", ".gradle", ".idea", "out", "obj":
		return true
	}
	return strings.HasPrefix(name, ".")
}

func skipFile(name string) bool {
	ext := strings.ToLower(filepath.Ext(name))
	switch ext {
	case ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".pdf",
		".zip", ".gz", ".tar", ".woff", ".woff2", ".ttf", ".eot",
		".mp4", ".mov", ".wasm", ".exe", ".dylib", ".so", ".a",
		".lock", ".map", ".min.js", ".min.css":
		return true
	}
	return false
}
