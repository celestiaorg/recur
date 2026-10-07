package git

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/domain"
)

// Checkpoints are hidden snapshots of a worktree — tracked changes plus
// untracked (non-ignored) files — stored as commit objects under a private ref
// namespace (refs/recur/checkpoints/…). Nothing on the working branch,
// the real index, the stash stack, the reflog, or the working tree changes:
// staging happens in a private copy of the index (GIT_INDEX_FILE) that lives
// in Recur's data dir, so untracked files stay untracked for the user.
// `git log`/`branch`/`tag`/`stash list` never show a checkpoint; only
// `show-ref`, `for-each-ref` and `log --all` do. `git push --mirror` would
// publish them, plain push/fetch do not.

// CheckpointRefPrefix is the namespace every checkpoint ref lives under.
const CheckpointRefPrefix = "refs/recur/checkpoints/"

// checkpointIdentity gives commit-tree an author even when the repo/user has
// none configured — checkpoints are Recur's, not the user's.
var checkpointIdentity = []string{
	"GIT_AUTHOR_NAME=Recur", "GIT_AUTHOR_EMAIL=checkpoint@recur.local",
	"GIT_COMMITTER_NAME=Recur", "GIT_COMMITTER_EMAIL=checkpoint@recur.local",
}

// Checkpoint snapshots worktree into ref and returns the commit oid. indexDir
// holds the private index copy (created if needed); it must be outside the
// worktree. ref must start with CheckpointRefPrefix.
func Checkpoint(ctx context.Context, worktree, indexDir, ref string) (string, error) {
	if !strings.HasPrefix(ref, CheckpointRefPrefix) {
		return "", fmt.Errorf("checkpoint ref %q outside %s", ref, CheckpointRefPrefix)
	}
	if err := os.MkdirAll(indexDir, 0o755); err != nil {
		return "", err
	}
	privIndex := filepath.Join(indexDir, "index")
	// Seed from the real index so `add -A` only re-hashes files whose stat
	// changed (milliseconds on a large repo instead of a full rehash).
	realIndex, err := run(ctx, worktree, nil, "rev-parse", "--path-format=absolute", "--git-path", "index")
	if err != nil {
		return "", err
	}
	if data, err := os.ReadFile(strings.TrimSpace(realIndex)); err == nil {
		if err := os.WriteFile(privIndex, data, 0o644); err != nil {
			return "", err
		}
	} else {
		_ = os.Remove(privIndex) // unborn repo: start from an empty index
	}
	env := []string{"GIT_INDEX_FILE=" + privIndex}
	if _, err := run(ctx, worktree, env, "add", "-A", "--", "."); err != nil {
		return "", err
	}
	tree, err := run(ctx, worktree, env, "write-tree")
	if err != nil {
		return "", err
	}
	args := []string{"commit-tree", strings.TrimSpace(tree), "-m", "recur checkpoint"}
	if commitRefExists(ctx, worktree, "HEAD") {
		args = append(args, "-p", "HEAD")
	}
	oid, err := run(ctx, worktree, checkpointIdentity, args...)
	if err != nil {
		return "", err
	}
	oid = strings.TrimSpace(oid)
	if _, err := run(ctx, worktree, nil, "update-ref", ref, oid); err != nil {
		return "", err
	}
	return oid, nil
}

// ListCheckpoints returns ref → oid for every checkpoint under prefix
// (a full ref path such as CheckpointRefPrefix+workspaceID+"/").
func ListCheckpoints(ctx context.Context, worktree, prefix string) map[string]string {
	return listCheckpointRefs(ctx, worktree, prefix, "objectname")
}

// ListCheckpointTrees returns ref → tree oid in one Git call. Checkpoints
// are commits; comparing their trees ignores changes to commit metadata.
func ListCheckpointTrees(ctx context.Context, worktree, prefix string) map[string]string {
	return listCheckpointRefs(ctx, worktree, prefix, "tree")
}

func listCheckpointRefs(ctx context.Context, worktree, prefix, field string) map[string]string {
	out, err := run(ctx, worktree, nil, "for-each-ref", "--format=%(refname) %("+field+")", prefix)
	if err != nil {
		return nil
	}
	refs := map[string]string{}
	for _, line := range strings.Split(strings.TrimSpace(out), "\n") {
		if name, oid, ok := strings.Cut(line, " "); ok {
			refs[name] = oid
		}
	}
	return refs
}

// DeleteCheckpoints removes every checkpoint ref under prefix. Objects become
// unreachable and are reclaimed by the user's normal git gc.
func DeleteCheckpoints(ctx context.Context, repo, prefix string) error {
	for ref := range ListCheckpoints(ctx, repo, prefix) {
		if _, err := run(ctx, repo, nil, "update-ref", "-d", ref); err != nil {
			return err
		}
	}
	return nil
}

// DiffCheckpoints is the unified diff from→to (either may be a checkpoint ref
// or any commit-ish), parsed and highlighted like the Commits tab.
func DiffCheckpoints(ctx context.Context, worktree, from, to string) ([]domain.FileDiff, error) {
	out, err := run(ctx, worktree, nil, "diff", "--no-color", from, to)
	if err != nil {
		return nil, err
	}
	return parseHighlightedPatch(out), nil
}

// RestoreCheckpoint makes the working tree match ref: tracked files are
// restored/removed (no-overlay), then untracked files that did not exist at
// the checkpoint are deleted. The index is not touched, so files that were
// untracked at capture time come back untracked.
//
// simplify: index state (what was staged) is not snapshotted, so a file that
// was staged at capture time comes back as an unstaged modification; commits
// the agent made after the checkpoint stay on the branch. Upgrade path: pin a
// second tree of the real index per checkpoint and read-tree it here.
func RestoreCheckpoint(ctx context.Context, worktree, ref string) error {
	if _, err := run(ctx, worktree, nil, "restore", "--source="+ref, "--worktree", "--no-overlay", "--", "."); err != nil {
		return err
	}
	inCkpt, err := run(ctx, worktree, nil, "ls-tree", "-r", "-z", "--name-only", ref)
	if err != nil {
		return err
	}
	keep := map[string]bool{}
	for _, p := range strings.Split(inCkpt, "\x00") {
		if p != "" {
			keep[p] = true
		}
	}
	others, err := run(ctx, worktree, nil, "ls-files", "-z", "--others", "--exclude-standard")
	if err != nil {
		return err
	}
	for _, p := range strings.Split(others, "\x00") {
		if p == "" || keep[filepath.ToSlash(p)] {
			continue
		}
		if err := os.Remove(filepath.Join(worktree, p)); err != nil && !os.IsNotExist(err) {
			return err
		}
	}
	return nil
}

func run(ctx context.Context, dir string, extraEnv []string, args ...string) (string, error) {
	cmd := commandWithEnv(ctx, extraEnv, append([]string{"-C", dir}, args...)...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		if msg := strings.TrimSpace(stderr.String()); msg != "" {
			return stdout.String(), fmt.Errorf("git %s: %s: %w", args[0], msg, err)
		}
		return stdout.String(), fmt.Errorf("git %s: %w", args[0], err)
	}
	return stdout.String(), nil
}
