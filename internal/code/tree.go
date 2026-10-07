package code

import (
	"context"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// GitStatus returns path → m|a|d|u for changes at root.
// When baseBranch is non-empty (loop worktree), status is versus the merge-base
// with that base — the spin-off point. Otherwise porcelain versus HEAD.
func GitStatus(ctx context.Context, root, baseBranch string) map[string]string {
	return gitpkg.ChangeStatus(ctx, root, baseBranch)
}

// FileDiff returns the diff of path versus baseBranch's merge-base (or HEAD).
func FileDiff(ctx context.Context, root, baseBranch, rel string) (domain.FileDiff, error) {
	return gitpkg.WorkingDiff(ctx, root, baseBranch, rel)
}

// FileAtBase returns path contents at the merge-base of baseBranch and HEAD.
// Empty when the file did not exist at the spin-off point (added/untracked).
func FileAtBase(ctx context.Context, root, baseBranch, rel string) (string, error) {
	content, err := gitpkg.FileAtBase(ctx, root, baseBranch, rel)
	if err != nil {
		return "", err
	}
	if len(content) > maxEditBytes {
		return "", fmt.Errorf("file too large (%d bytes)", len(content))
	}
	return content, nil
}

// ListTree returns a tree under root/rel. depth 1 = immediate children only
// (the CodeService / UI path — expand dirs by calling again with that path).
// depth 0 defaults to 1; values > 1 recurse for tests/tools. A genuinely
// missing rel (the agent deleted the dir) comes back as ErrNotFound so
// live-refresh polling can tell that apart from a transient read failure.
// Entries are annotated with change status versus baseBranch when set.
//
// This computes git status for the single listing. Refreshing several
// directories at once should use ListTrees, which shares one status pass.
func ListTree(ctx context.Context, root, rel string, depth int, baseBranch string) ([]domain.TreeEntry, error) {
	out, err := listTreeWith(root, rel, depth, newStatusIndex(GitStatus(ctx, root, baseBranch)))
	if err != nil {
		return nil, err
	}
	annotateIgnored(ctx, root, out)
	return out, nil
}

// ListTrees lists several directories in one call, computing the git change
// status and the check-ignore set EXACTLY ONCE each for the whole batch.
//
// This is the Observatory's live-refresh path: while an agent works, the UI
// re-lists every expanded directory on each coalesced FSEvents batch. Per-
// directory ListTree meant three git subprocesses (merge-base + diff
// --name-status + status --porcelain) per directory per tick — ~50ms each even
// on a small repo, so ten open directories could spend most of the refresh
// interval forking git. The status is still recomputed on every batch (never
// cached across calls): live-refresh correctness depends on it being fresh.
//
// Directories that cannot be listed (typically: the agent just deleted them)
// are simply absent from the returned map rather than failing the batch, so one
// stale path from the frontend's expanded set doesn't blank the whole tree.
func ListTrees(ctx context.Context, root string, rels []string, depth int, baseBranch string) (map[string][]domain.TreeEntry, error) {
	out := make(map[string][]domain.TreeEntry, len(rels))
	if len(rels) == 0 {
		return out, nil
	}
	idx := newStatusIndex(GitStatus(ctx, root, baseBranch))
	seen := make(map[string]bool, len(rels))
	for _, rel := range rels {
		key := strings.TrimPrefix(filepath.ToSlash(rel), "/")
		if key == "." {
			key = ""
		}
		if seen[key] {
			continue
		}
		seen[key] = true
		if err := ctx.Err(); err != nil {
			return out, err
		}
		kids, err := listTreeWith(root, key, depth, idx)
		if err != nil {
			// Missing/unreadable dir: omit the key (see doc comment).
			continue
		}
		out[key] = kids
	}
	annotateIgnoredMap(ctx, root, out)
	return out, nil
}

// listTreeWith is the shared body of ListTree/ListTrees with the change-status
// index injected, so a batch pays for git status exactly once. Ignore
// annotation is applied by the callers after all listings are in hand.
func listTreeWith(root, rel string, depth int, idx *statusIndex) ([]domain.TreeEntry, error) {
	base, err := SafeJoin(root, rel)
	if err != nil {
		return nil, err
	}
	if depth <= 0 {
		depth = 1
	}
	out, err := readDir(root, base, depth, idx)
	if err != nil && os.IsNotExist(err) {
		return nil, notFoundError(rel)
	}
	return out, err
}

// statusIndex is a change-status map plus a one-time index of the deleted paths
// by parent directory. injectDeleted needs "which deleted files belonged to
// THIS directory"; without the index each directory rescanned the entire status
// map, which is O(directories × changed files) per refresh tick.
type statusIndex struct {
	byPath    map[string]string
	deletedBy map[string][]string // parent dir ("" = repo root) → deleted child paths
}

func newStatusIndex(statuses map[string]string) *statusIndex {
	idx := &statusIndex{byPath: statuses}
	if len(statuses) == 0 {
		return idx
	}
	for p, st := range statuses {
		if st != "d" || p == "" {
			continue
		}
		idx.deletedBy = appendDeleted(idx.deletedBy, path.Dir(p), p)
	}
	return idx
}

func appendDeleted(m map[string][]string, parent, p string) map[string][]string {
	if parent == "." {
		parent = "" // repo root
	}
	if m == nil {
		m = map[string][]string{}
	}
	m[parent] = append(m[parent], p)
	return m
}

func (idx *statusIndex) status(p string) string {
	if idx == nil {
		return ""
	}
	return idx.byPath[p]
}

func (idx *statusIndex) deletedIn(parent string) []string {
	if idx == nil {
		return nil
	}
	return idx.deletedBy[parent]
}

func annotateIgnored(ctx context.Context, root string, ents []domain.TreeEntry) {
	ignored := gitpkg.IgnoredSet(ctx, root, collectTreePaths(ents))
	if len(ignored) == 0 {
		return
	}
	markTreeIgnored(ents, ignored)
}

func annotateIgnoredMap(ctx context.Context, root string, trees map[string][]domain.TreeEntry) {
	var paths []string
	for _, ents := range trees {
		paths = append(paths, collectTreePaths(ents)...)
	}
	ignored := gitpkg.IgnoredSet(ctx, root, paths)
	if len(ignored) == 0 {
		return
	}
	for _, ents := range trees {
		markTreeIgnored(ents, ignored)
	}
}

func collectTreePaths(ents []domain.TreeEntry) []string {
	var paths []string
	var walk func([]domain.TreeEntry)
	walk = func(es []domain.TreeEntry) {
		for i := range es {
			if p := es[i].Path; p != "" && p != "." {
				paths = append(paths, p)
			}
			if len(es[i].Children) > 0 {
				walk(es[i].Children)
			}
		}
	}
	walk(ents)
	return paths
}

func markTreeIgnored(ents []domain.TreeEntry, ignored map[string]bool) {
	for i := range ents {
		if ignored[ents[i].Path] {
			ents[i].Ignored = true
		}
		if len(ents[i].Children) > 0 {
			markTreeIgnored(ents[i].Children, ignored)
		}
	}
}

func readDir(root, abs string, depth int, idx *statusIndex) ([]domain.TreeEntry, error) {
	entries, err := os.ReadDir(abs)
	if err != nil {
		return nil, err
	}
	parent := RelSlash(root, abs)
	if parent == "." {
		parent = ""
	}
	seen := make(map[string]bool, len(entries))
	var out []domain.TreeEntry
	for _, e := range entries {
		name := e.Name()
		// Unfiltered listing — the UI applies files.exclude globs. Search/symbol
		// walks still use skipDir so heavy dirs aren't traversed there.
		full := filepath.Join(abs, name)
		isDir := entryIsDir(e, full)
		ent := domain.TreeEntry{
			Name: name,
			Path: RelSlash(root, full),
			Dir:  isDir,
		}
		if st := idx.status(ent.Path); st != "" {
			ent.Status = st
		}
		seen[ent.Path] = true
		if isDir && depth > 1 {
			kids, err := readDir(root, full, depth-1, idx)
			if err == nil {
				ent.Children = kids
			}
		}
		out = append(out, ent)
	}
	out = injectDeleted(out, parent, idx, seen)
	sort.Slice(out, func(i, j int) bool {
		if out[i].Dir != out[j].Dir {
			return out[i].Dir
		}
		return strings.ToLower(out[i].Name) < strings.ToLower(out[j].Name)
	})
	return out, nil
}

// entryIsDir reports whether e is a directory, following a symlink to its
// target. os.DirEntry.IsDir does not follow links, so a symlink to a dir
// would otherwise show up as a file in the explorer.
func entryIsDir(e os.DirEntry, full string) bool {
	if e.IsDir() {
		return true
	}
	if e.Type()&os.ModeSymlink == 0 {
		return false
	}
	st, err := os.Stat(full)
	return err == nil && st.IsDir()
}

// injectDeleted adds status=d entries for paths deleted from parent that are
// missing from the filesystem listing (so the explorer can still show them).
func injectDeleted(out []domain.TreeEntry, parent string, idx *statusIndex, seen map[string]bool) []domain.TreeEntry {
	for _, p := range idx.deletedIn(parent) {
		if seen[p] {
			continue
		}
		out = append(out, domain.TreeEntry{
			Name:   path.Base(p),
			Path:   p,
			Dir:    false,
			Status: "d",
		})
	}
	return out
}
