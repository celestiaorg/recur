package git

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Identity returns git's effective author in dir (worktree or repo).
// It uses `git var GIT_AUTHOR_IDENT` so env, repo, and global config match a
// plain `git commit`. No synthetic fallback: if git cannot resolve an identity
// (e.g. user.useConfigOnly with no user.email), Identity returns an error.
func Identity(ctx context.Context, dir string) (name, email string, err error) {
	out, err := runGit(ctx, dir, "var", "GIT_AUTHOR_IDENT")
	if err != nil {
		return "", "", fmt.Errorf("git identity: %w", err)
	}
	name, email, ok := parseGitIdent(strings.TrimSpace(out))
	if !ok {
		return "", "", fmt.Errorf("git identity: unparseable %q", strings.TrimSpace(out))
	}
	return name, email, nil
}

// parseGitIdent splits `Name <email> timestamp tz` from git var GIT_*_IDENT.
func parseGitIdent(s string) (name, email string, ok bool) {
	i := strings.LastIndex(s, "> ")
	if i < 0 {
		return "", "", false
	}
	lt := strings.LastIndex(s[:i], " <")
	if lt < 0 {
		return "", "", false
	}
	name = strings.TrimSpace(s[:lt])
	email = s[lt+2 : i]
	if name == "" || email == "" {
		return "", "", false
	}
	return name, email, true
}

// WorkingStatus returns porcelain status versus HEAD (uncommitted only).
func WorkingStatus(ctx context.Context, dir string) map[string]string {
	return StatusMap(ctx, dir)
}

// DiscardFile restores a tracked path to HEAD, or removes an untracked path.
func DiscardFile(ctx context.Context, dir, rel string) error {
	if !IsRepo(dir) {
		return fmt.Errorf("not a git repository")
	}
	rel = filepath.ToSlash(strings.TrimPrefix(rel, "/"))
	if rel == "" || strings.Contains(rel, "..") {
		return fmt.Errorf("invalid path")
	}
	st := statusMapNow(ctx, dir)
	letter, ok := st[rel]
	if !ok {
		return nil // already clean versus HEAD
	}
	full := filepath.Join(dir, filepath.FromSlash(rel))
	if letter == "u" {
		return os.RemoveAll(full)
	}
	// restore tracked file (also resurrects deleted)
	if _, err := runGit(ctx, dir, "restore", "--source=HEAD", "--staged", "--worktree", "--", rel); err != nil {
		// Older git: checkout
		if _, err2 := runGit(ctx, dir, "checkout", "HEAD", "--", rel); err2 != nil {
			return fmt.Errorf("discard %s: %w", rel, err)
		}
	}
	return nil
}

// DiscardAll hard-resets to HEAD and removes untracked files.
func DiscardAll(ctx context.Context, dir string) error {
	if !IsRepo(dir) {
		return fmt.Errorf("not a git repository")
	}
	if _, err := runGit(ctx, dir, "reset", "--hard", "HEAD"); err != nil {
		return fmt.Errorf("git reset --hard: %w", err)
	}
	if _, err := runGit(ctx, dir, "clean", "-fd"); err != nil {
		return fmt.Errorf("git clean: %w", err)
	}
	return nil
}

// Stage adds paths to the index.
func Stage(ctx context.Context, dir string, paths []string) error {
	if len(paths) == 0 {
		return nil
	}
	if !IsRepo(dir) {
		return fmt.Errorf("not a git repository")
	}
	args := append([]string{"add", "--"}, paths...)
	if _, err := runGit(ctx, dir, args...); err != nil {
		return fmt.Errorf("git add: %w", err)
	}
	return nil
}

// Unstage removes paths from the index, keeping the working tree.
func Unstage(ctx context.Context, dir string, paths []string) error {
	if len(paths) == 0 {
		return nil
	}
	if !IsRepo(dir) {
		return fmt.Errorf("not a git repository")
	}
	args := append([]string{"restore", "--staged", "--"}, paths...)
	if _, err := runGit(ctx, dir, args...); err != nil {
		args = append([]string{"reset", "HEAD", "--"}, paths...)
		if _, err2 := runGit(ctx, dir, args...); err2 != nil {
			return fmt.Errorf("git unstage: %w", err)
		}
	}
	return nil
}

// CommitStaged commits only the index (no git add -A). No-op when nothing staged.
func (m *Manager) CommitStaged(ctx context.Context, worktree, message, authorName, authorEmail string) (CommitResult, error) {
	if !IsRepo(worktree) {
		return CommitResult{}, fmt.Errorf("not a git repository")
	}
	// Detect staged changes via diff --cached --quiet (exit 1 = has changes).
	_, err := runGit(ctx, worktree, "diff", "--cached", "--quiet")
	if err == nil {
		return CommitResult{Committed: false}, nil
	}
	add, del := m.stagedDiffStat(ctx, worktree)
	args := []string{
		"-c", "user.name=" + authorName,
		"-c", "user.email=" + authorEmail,
		"commit", "-m", message,
		"--author", fmt.Sprintf("%s <%s>", authorName, authorEmail),
	}
	if _, err := m.runIn(ctx, worktree, args...); err != nil {
		return CommitResult{}, fmt.Errorf("git commit: %w", err)
	}
	hash, _ := m.runIn(ctx, worktree, "rev-parse", "--short", "HEAD")
	return CommitResult{
		Hash:      strings.TrimSpace(hash),
		Committed: true,
		Additions: add,
		Deletions: del,
	}, nil
}

// StagedPaths returns paths currently in the index (staged for commit).
// Best-effort: a plain (non-git) folder returns nil, nil — never the
// `git diff --no-index` / "unknown option cached" failure, which the Observatory
// used to toast as "Staged files failed" on every save in a non-git working copy.
func StagedPaths(ctx context.Context, dir string) ([]string, error) {
	if dir == "" || !IsRepo(dir) {
		return nil, nil
	}
	out, err := runGit(ctx, dir, "diff", "--cached", "--name-only", "-z")
	if err != nil {
		return nil, err
	}
	if strings.TrimSpace(out) == "" {
		return nil, nil
	}
	var paths []string
	for _, p := range strings.Split(out, "\x00") {
		p = strings.TrimSpace(p)
		if p != "" {
			paths = append(paths, filepath.ToSlash(p))
		}
	}
	return paths, nil
}

// StagedDiff returns the unified diff of the index versus HEAD (`git diff --cached`).
// Empty string means nothing is staged (or dir is not a git work tree).
func StagedDiff(ctx context.Context, dir string) (string, error) {
	if dir == "" || !IsRepo(dir) {
		return "", nil
	}
	out, err := runGit(ctx, dir, "diff", "--cached", "--")
	if err != nil {
		return "", fmt.Errorf("git diff --cached: %w", err)
	}
	return out, nil
}

// RecentCommitSubjects returns up to n recent commit subject lines (newest first).
// Best-effort: on failure returns nil so callers can still generate without style hints.
func RecentCommitSubjects(ctx context.Context, dir string, n int) []string {
	if n <= 0 {
		return nil
	}
	out, err := runGit(ctx, dir, "log", fmt.Sprintf("-%d", n), "--pretty=%s")
	if err != nil || strings.TrimSpace(out) == "" {
		return nil
	}
	var subjects []string
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimSpace(line)
		if line != "" {
			subjects = append(subjects, line)
		}
	}
	return subjects
}

func runGit(ctx context.Context, dir string, args ...string) (string, error) {
	m := &Manager{}
	return m.runIn(ctx, dir, args...)
}
