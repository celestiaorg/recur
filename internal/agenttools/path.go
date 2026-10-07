package agenttools

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/code"
)

func resolvePath(worktree, p string) (string, error) {
	wt, err := absEval(strings.TrimSpace(worktree))
	if err != nil {
		return "", fmt.Errorf("worktree: %w", err)
	}
	p = strings.TrimSpace(p)
	if p == "" {
		return "", fmt.Errorf("path is required")
	}
	var abs string
	if filepath.IsAbs(p) {
		abs, err = absEvalExisting(p)
		if err != nil {
			return "", err
		}
	} else {
		abs, err = code.SafeJoin(wt, p)
		if err != nil {
			return "", err
		}
		abs, err = absEvalExisting(abs)
		if err != nil {
			return "", err
		}
	}
	rel, err := filepath.Rel(wt, abs)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", fmt.Errorf("path escapes worktree")
	}
	return abs, nil
}

func resolveWritePath(worktree, p string) (string, error) {
	return resolvePath(worktree, p)
}

func absEval(p string) (string, error) {
	abs, err := filepath.Abs(p)
	if err != nil {
		return "", err
	}
	if eval, err := filepath.EvalSymlinks(abs); err == nil {
		return eval, nil
	}
	return abs, nil
}

// absEvalExisting canonicalizes p, walking up to the nearest existing ancestor
// so a missing leaf under a symlink cannot fail-open to a lexical in-worktree
// path that MkdirAll would follow outside the sandbox.
func absEvalExisting(p string) (string, error) {
	abs, err := filepath.Abs(p)
	if err != nil {
		return "", err
	}
	if eval, err := filepath.EvalSymlinks(abs); err == nil {
		return eval, nil
	}
	var rest []string
	cur := abs
	for {
		info, err := os.Lstat(cur)
		if err == nil && info.Mode()&os.ModeSymlink != 0 {
			return "", fmt.Errorf("path escapes worktree")
		}
		parent := filepath.Dir(cur)
		if parent == cur {
			if len(rest) == 0 {
				return abs, nil
			}
			return filepath.Join(append([]string{cur}, rest...)...), nil
		}
		rest = append([]string{filepath.Base(cur)}, rest...)
		if eval, err := filepath.EvalSymlinks(parent); err == nil {
			return filepath.Join(append([]string{eval}, rest...)...), nil
		}
		cur = parent
	}
}
