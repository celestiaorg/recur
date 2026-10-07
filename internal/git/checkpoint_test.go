package git

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// A checkpoint captures tracked edits + untracked files, without touching the
// branch, the real index, the stash stack, or the working tree.
func TestCheckpointIsInvisibleAndComplete(t *testing.T) {
	ctx := context.Background()
	dir := initRepo(t)
	idx := filepath.Join(t.TempDir(), "ckpt")
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("edited\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "new.txt"), []byte("untracked\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitOut(t, dir, "stash", "push", "-m", "user-stash", "--", "README.md") // user's own stash
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("edited again\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	headBefore := gitOut(t, dir, "rev-parse", "HEAD")
	statusBefore := gitOut(t, dir, "status", "--porcelain")
	stashBefore := gitOut(t, dir, "stash", "list")

	ref := CheckpointRefPrefix + "ws1/msg1/start"
	oid, err := Checkpoint(ctx, dir, idx, ref)
	if err != nil {
		t.Fatalf("Checkpoint: %v", err)
	}

	if got := gitOut(t, dir, "rev-parse", "HEAD"); got != headBefore {
		t.Fatalf("HEAD moved: %s → %s", headBefore, got)
	}
	if got := gitOut(t, dir, "status", "--porcelain"); got != statusBefore {
		t.Fatalf("status changed:\n%s\n→\n%s", statusBefore, got)
	}
	if !strings.Contains(statusBefore, "?? new.txt") {
		t.Fatalf("fixture: new.txt should be untracked: %s", statusBefore)
	}
	if got := gitOut(t, dir, "stash", "list"); got != stashBefore {
		t.Fatalf("stash list changed:\n%s\n→\n%s", stashBefore, got)
	}
	if got := gitOut(t, dir, "log", "--oneline", "--branches", "--tags"); strings.Contains(got, "recur checkpoint") {
		t.Fatalf("checkpoint visible in branch/tag log:\n%s", got)
	}
	tree := gitOut(t, dir, "ls-tree", "-r", "--name-only", oid)
	if !strings.Contains(tree, "new.txt") {
		t.Fatalf("untracked file missing from checkpoint tree:\n%s", tree)
	}
	if got := gitOut(t, dir, "show", oid+":README.md"); got != "edited again\n" {
		t.Fatalf("tracked edit not captured: %q", got)
	}
	if refs := ListCheckpoints(ctx, dir, CheckpointRefPrefix+"ws1/"); refs[ref] != oid {
		t.Fatalf("ListCheckpoints = %v", refs)
	}
	if _, err := os.Stat(filepath.Join(idx, "index")); err != nil {
		t.Fatalf("private index not in data dir: %v", err)
	}
	same := CheckpointRefPrefix + "ws1/msg1/same"
	if _, err := Checkpoint(ctx, dir, idx, same); err != nil {
		t.Fatalf("Checkpoint same: %v", err)
	}
	trees := ListCheckpointTrees(ctx, dir, CheckpointRefPrefix+"ws1/")
	if want := strings.TrimSpace(gitOut(t, dir, "rev-parse", oid+"^{tree}")); trees[ref] != want {
		t.Fatalf("checkpoint tree = %q, want %q", trees[ref], want)
	}
	if trees[ref] == "" || trees[ref] != trees[same] {
		t.Fatal("identical snapshots should not differ")
	}

	// Later edits diff against the checkpoint, and restore rewinds them —
	// new.txt comes back untracked, the agent's extra file is deleted.
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("agent\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "agent.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	end := CheckpointRefPrefix + "ws1/msg1/end"
	if _, err := Checkpoint(ctx, dir, idx, end); err != nil {
		t.Fatalf("Checkpoint end: %v", err)
	}
	trees = ListCheckpointTrees(ctx, dir, CheckpointRefPrefix+"ws1/")
	if trees[end] == "" || trees[ref] == trees[end] {
		t.Fatal("edited snapshot should differ")
	}
	files, err := DiffCheckpoints(ctx, dir, ref, end)
	if err != nil {
		t.Fatalf("DiffCheckpoints: %v", err)
	}
	var names []string
	for _, f := range files {
		names = append(names, f.Path)
	}
	if len(files) != 2 || !strings.Contains(strings.Join(names, ","), "agent.txt") {
		t.Fatalf("turn diff files = %v", names)
	}
	if err := RestoreCheckpoint(ctx, dir, ref); err != nil {
		t.Fatalf("Restore: %v", err)
	}
	if got := gitOut(t, dir, "status", "--porcelain"); got != statusBefore {
		t.Fatalf("status after restore:\n%s\nwant:\n%s", got, statusBefore)
	}
	if _, err := os.Stat(filepath.Join(dir, "agent.txt")); !os.IsNotExist(err) {
		t.Fatal("agent.txt should be removed by restore")
	}

	if err := DeleteCheckpoints(ctx, dir, CheckpointRefPrefix+"ws1/"); err != nil {
		t.Fatal(err)
	}
	if refs := ListCheckpoints(ctx, dir, CheckpointRefPrefix); len(refs) != 0 {
		t.Fatalf("refs left after delete: %v", refs)
	}
}
