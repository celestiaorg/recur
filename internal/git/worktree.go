// Package git manages per-loop git worktrees (PRD §11): one worktree + branch
// per loop, created off the project's base branch, so loops run in isolation
// and never touch the user's checkout. The default branch is `recur/<slug>`;
// Settings.BranchNamePrefix can substitute a custom prefix or none. Worktrees
// live at <root>/<project-slug>/<workspace-slug> (workspace id
// `{project}.{slug}`). It shells out to the system `git` via os/exec rather
// than embedding a git library, matching how agents see the repo.
package git

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"
)

// Worktree describes one checked-out worktree of a repository.
type Worktree struct {
	Path   string // absolute path to the worktree directory
	Branch string // branch checked out there, e.g. "recur/rate-limit"
	Head   string // current commit hash (may be empty for a brand-new branch)
}

// Manager creates and tears down worktrees for a single repository.
type Manager struct {
	// RepoPath is the source checkout. May contain a leading "~"; it is expanded.
	RepoPath string
	// Root is where worktrees are created (Recur's managed worktrees/ dir).
	Root string

	// branchPrefix is prepended to new workspace branches as prefix/slug.
	// Empty with branchPrefixSet means no prefix. Unset (the zero value) uses
	// DefaultBranchPrefix so existing NewManager callers keep recur/<slug>.
	branchPrefix    string
	branchPrefixSet bool
}

// NewManager returns a Manager for repoPath, with worktrees created under root.
// repoPath's leading "~" is expanded. New branches use DefaultBranchPrefix
// until WithBranchPrefix is called.
func NewManager(repoPath, root string) *Manager {
	return &Manager{RepoPath: ExpandHome(repoPath), Root: root}
}

// WithBranchPrefix sets the prefix for new workspace branches (prefix/slug).
// An empty prefix means no prefix — the branch is just the slug. Always call
// this when the prefix comes from Settings so "none" is distinct from the
// NewManager default.
func (m *Manager) WithBranchPrefix(prefix string) *Manager {
	if m == nil {
		return m
	}
	m.branchPrefix = NormalizeBranchPrefix(prefix)
	m.branchPrefixSet = true
	return m
}

// Prefix is the path segment prepended to new workspace branches. Empty means
// no prefix.
func (m *Manager) Prefix() string {
	if m != nil && m.branchPrefixSet {
		return m.branchPrefix
	}
	return DefaultBranchPrefix
}

// ExpandHome expands a leading "~" (or "~/") in p to the user's home dir. Other
// "~user" forms are left untouched (rare; not worth the extra lookup in v1).
func ExpandHome(p string) string {
	if p == "~" || strings.HasPrefix(p, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, strings.TrimPrefix(p, "~"))
		}
	}
	return p
}

// isRepoTimeout bounds a single IsRepo git probe. Matches ListBranches /
// OpenMainWorkspace so a hung network mount can't block boot forever.
const isRepoTimeout = 10 * time.Second

// IsRepo reports whether path is the root of a git work tree (checkout or
// linked worktree), not merely a folder nested inside some enclosing repo.
// Empty / missing paths and plain directories return false. Leading "~" is
// expanded. Used to gate worktree-based workspace creation — non-git project
// folders may still open as a "working copy".
func IsRepo(path string) bool {
	path = strings.TrimSpace(ExpandHome(path))
	if path == "" {
		return false
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		return false
	}
	abs = filepath.Clean(abs)

	ctx, cancel := context.WithTimeout(context.Background(), isRepoTimeout)
	defer cancel()
	// --show-toplevel: must equal the project path so a docs/ subfolder (or a
	// folder under a git-tracked $HOME) is not treated as a workspace host.
	out, err := Command(ctx, "-C", abs, "rev-parse", "--show-toplevel").CombinedOutput()
	if err != nil {
		return false
	}
	top := filepath.Clean(strings.TrimSpace(string(out)))
	if top == "" {
		return false
	}
	topAbs, err := filepath.Abs(top)
	if err != nil {
		return false
	}
	return sameDirPath(abs, topAbs)
}

// CanonPath returns an absolute, cleaned path with a leading "~" expanded and
// symlinks resolved when the path exists. Missing paths still return Abs+Clean
// so callers can compare identity before Stat.
func CanonPath(p string) (string, error) {
	p = strings.TrimSpace(ExpandHome(p))
	if p == "" {
		return "", fmt.Errorf("empty path")
	}
	abs, err := filepath.Abs(p)
	if err != nil {
		return "", err
	}
	abs = filepath.Clean(abs)
	if ev, err := filepath.EvalSymlinks(abs); err == nil {
		return ev, nil
	}
	return abs, nil
}

// PathHasPrefix reports whether path is root or a descendant of root.
// Both arguments should already be CanonPath results. On darwin/windows a
// case-folded prefix is accepted only when the two prefixes are the same
// inode (case-insensitive volume). Distinct /Docs vs /docs on a
// case-sensitive APFS volume do not match.
func PathHasPrefix(path, root string) bool {
	if path == "" || root == "" {
		return false
	}
	if SamePath(path, root) {
		return true
	}
	prefix := root + string(os.PathSeparator)
	if strings.HasPrefix(path, prefix) {
		return true
	}
	if foldPathCase() && len(path) >= len(prefix) && strings.EqualFold(path[:len(prefix)], prefix) {
		return samePathOnDisk(path[:len(root)], root)
	}
	return false
}

// SamePath reports whether a and b name the same location. Byte-equal
// paths match without touching disk. A case-folded pair matches only
// when both exist and are the same inode.
func SamePath(a, b string) bool {
	if a == b {
		return true
	}
	if !foldPathCase() || a == "" || b == "" || !strings.EqualFold(a, b) {
		return false
	}
	return samePathOnDisk(a, b)
}

// AlignUnderRoot rewrites abs to use root's exact prefix bytes when
// PathHasPrefix(abs, root). filepath.Rel is byte-exact; without this a
// case-folded match yields `../../OtherCase/...` instead of a relative path.
func AlignUnderRoot(abs, root string) string {
	if !PathHasPrefix(abs, root) {
		return abs
	}
	if SamePath(abs, root) {
		return root
	}
	if len(abs) < len(root) {
		return root
	}
	return root + abs[len(root):]
}

func foldPathCase() bool {
	return runtime.GOOS == "darwin" || runtime.GOOS == "windows"
}

func samePathOnDisk(a, b string) bool {
	if a == b {
		return true
	}
	fa, err1 := os.Stat(a)
	fb, err2 := os.Stat(b)
	if err1 != nil || err2 != nil {
		return false
	}
	return os.SameFile(fa, fb)
}

// RepoRoot returns the git toplevel that contains path (file or directory).
// Unlike IsRepo, a nested folder still resolves to the enclosing checkout.
func RepoRoot(path string) (string, bool) {
	path = strings.TrimSpace(ExpandHome(path))
	if path == "" {
		return "", false
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", false
	}
	abs = filepath.Clean(abs)
	if st, err := os.Stat(abs); err == nil && !st.IsDir() {
		abs = filepath.Dir(abs)
	}
	ctx, cancel := context.WithTimeout(context.Background(), isRepoTimeout)
	defer cancel()
	out, err := Command(ctx, "-C", abs, "rev-parse", "--show-toplevel").CombinedOutput()
	if err != nil {
		return "", false
	}
	top := filepath.Clean(strings.TrimSpace(string(out)))
	if top == "" {
		return "", false
	}
	topAbs, err := filepath.Abs(top)
	if err != nil {
		return "", false
	}
	if ev, err := filepath.EvalSymlinks(topAbs); err == nil {
		return ev, true
	}
	return topAbs, true
}

// sameDirPath reports whether a and b name the same directory, resolving
// symlinks when possible so a project path and git's toplevel agree.
func sameDirPath(a, b string) bool {
	if a == b {
		return true
	}
	ae, err1 := filepath.EvalSymlinks(a)
	be, err2 := filepath.EvalSymlinks(b)
	if err1 != nil || err2 != nil {
		return false
	}
	return ae == be
}

// DefaultBranchPrefix is the historical workspace branch namespace.
const DefaultBranchPrefix = "recur"

// PrefixedBranch joins prefix and slug as prefix/slug. An empty prefix returns
// the slug alone. Both sides are trimmed of slashes.
func PrefixedBranch(prefix, slug string) string {
	prefix = strings.Trim(prefix, "/")
	slug = strings.Trim(slug, "/")
	if prefix == "" {
		return slug
	}
	if slug == "" {
		return prefix
	}
	return prefix + "/" + slug
}

// BranchForSlug returns the default workspace branch name (recur/<slug>).
// Callers that honor Settings should use PrefixedBranch with the resolved
// prefix instead. The kitchen slug is the leaf of a `{project}.{slug}` id so
// branches stay unique per repository, independent of project.
func BranchForSlug(slug string) string { return PrefixedBranch(DefaultBranchPrefix, slug) }

// NormalizeBranchPrefix turns a free-form prefix into a git-safe path fragment
// (lowercase, illegal characters → hyphen). Empty input stays empty so "no
// prefix" survives. Unlike SanitizeBranchName it never adds a default namespace.
func NormalizeBranchPrefix(prefix string) string {
	prefix = strings.TrimSpace(prefix)
	if prefix == "" {
		return ""
	}
	name, err := sanitizeGitPath(prefix)
	if err != nil {
		return ""
	}
	return strings.Trim(name, "/")
}

// SplitWorkspaceID splits a `{project}.{slug}` workspace id. Legacy kitchen-only
// ids (no dot) and empty segments return ok=false. Split is from the last dot
// so a project slug that somehow contains a dot still yields the kitchen leaf.
func SplitWorkspaceID(id string) (project, slug string, ok bool) {
	id = strings.TrimSpace(id)
	i := strings.LastIndex(id, ".")
	if i <= 0 || i == len(id)-1 {
		return "", "", false
	}
	project, slug = id[:i], id[i+1:]
	if project == "." || project == ".." || slug == "." || slug == ".." {
		return "", "", false
	}
	return project, slug, true
}

// WorkspaceSlug returns the kitchen leaf of a workspace id (`flaky-ramen` for
// `recur.flaky-ramen`). Legacy ids with no dot are returned unchanged.
func WorkspaceSlug(id string) string {
	if _, slug, ok := SplitWorkspaceID(id); ok {
		return slug
	}
	return strings.TrimSpace(id)
}

// WorktreeRelPath maps a workspace id onto the directory under the worktrees
// root: `{project}.{slug}` → `{project}/{slug}`. Legacy kitchen-only ids stay
// a single segment so existing checkouts keep working.
func WorktreeRelPath(workspaceID string) string {
	workspaceID = strings.TrimSpace(workspaceID)
	if project, slug, ok := SplitWorkspaceID(workspaceID); ok {
		return filepath.Join(project, slug)
	}
	return workspaceID
}

// uniqueBranch returns PrefixedBranch(prefix, slug), or that name plus -N when
// it is already a local head. One `git branch --list '<branch>*'` collects the
// taken names so recycled kitchen ids do not need a show-ref per candidate.
func (m *Manager) uniqueBranch(ctx context.Context, slug string) string {
	branch := PrefixedBranch(m.Prefix(), slug)
	taken := m.listedLocalBranches(ctx, branch+"*")
	inTaken := func(name string) bool {
		if taken != nil {
			return taken[name]
		}
		return m.branchExists(ctx, name)
	}
	if !inTaken(branch) {
		return branch
	}
	for n := 2; ; n++ {
		candidate := fmt.Sprintf("%s-%d", branch, n)
		if !inTaken(candidate) {
			return candidate
		}
	}
}

// listedLocalBranches returns short names matching glob (e.g. "recur/crispy-quiche*").
// A nil map means the list command failed; callers fall back to show-ref.
func (m *Manager) listedLocalBranches(ctx context.Context, glob string) map[string]bool {
	out, err := m.run(ctx, "--no-pager", "branch", "--list", "--format", "%(refname:short)", glob)
	if err != nil {
		return nil
	}
	taken := make(map[string]bool)
	for _, line := range strings.Split(out, "\n") {
		name := strings.TrimSpace(line)
		if name != "" {
			taken[name] = true
		}
	}
	return taken
}

// CurrentBranch returns the checked-out branch name in worktree via
// `git branch --show-current`. An empty string (with nil error) means detached
// HEAD. Callers treat the live worktree branch as source of truth when present —
// humans and agents may rename or check out another branch mid-session.
func CurrentBranch(ctx context.Context, worktree string) (string, error) {
	if strings.TrimSpace(worktree) == "" {
		return "", fmt.Errorf("current branch: empty worktree")
	}
	out, err := Command(ctx, "-C", worktree, "branch", "--show-current").CombinedOutput()
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if msg != "" {
			return "", fmt.Errorf("git branch --show-current: %s: %w", msg, err)
		}
		return "", fmt.Errorf("git branch --show-current: %w", err)
	}
	return strings.TrimSpace(string(out)), nil
}

// SanitizeBranchName turns a free-form topic into a git-safe branch name using
// DefaultBranchPrefix when the topic has no slash.
func SanitizeBranchName(name string) (string, error) {
	return SanitizeBranchNameWithPrefix(DefaultBranchPrefix, name)
}

// SanitizeBranchNameWithPrefix turns a free-form topic into a git-safe branch
// name. Spaces become hyphens; other illegal characters are dropped. A name
// with no slash gets prefix/ prepended (empty prefix leaves the slug as-is).
// The result is checked with `git check-ref-format`.
func SanitizeBranchNameWithPrefix(prefix, name string) (string, error) {
	path, err := sanitizeGitPath(name)
	if err != nil {
		return "", err
	}
	path = applyBranchPrefix(prefix, path)
	if len(path) > 80 {
		path = joinBranchComponents(strings.Trim(path[:80], ".-/"))
		if path == "" {
			return "", fmt.Errorf("invalid branch name")
		}
		path = applyBranchPrefix(prefix, path)
	}
	if err := checkRefFormat(path); err != nil {
		return "", err
	}
	return path, nil
}

func applyBranchPrefix(prefix, name string) string {
	if strings.Contains(name, "/") {
		return name
	}
	out := PrefixedBranch(NormalizeBranchPrefix(prefix), name)
	if out == "" {
		return name
	}
	return out
}

// sanitizeGitPath lowercases name, maps illegal characters to hyphens, and
// collapses empty path components. Empty input is an error.
func sanitizeGitPath(name string) (string, error) {
	name = strings.ToLower(strings.TrimSpace(name))
	if name == "" {
		return "", fmt.Errorf("empty branch name")
	}
	var b strings.Builder
	lastDash := false
	for _, r := range name {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9', r == '.', r == '_', r == '/':
			b.WriteRune(r)
			lastDash = false
		default:
			if !lastDash {
				b.WriteByte('-')
				lastDash = true
			}
		}
	}
	name = strings.Trim(b.String(), "-/")
	for strings.Contains(name, "//") {
		name = strings.ReplaceAll(name, "//", "/")
	}
	for strings.Contains(name, "..") {
		name = strings.ReplaceAll(name, "..", ".")
	}
	name = joinBranchComponents(name)
	if name == "" {
		return "", fmt.Errorf("invalid branch name")
	}
	return name, nil
}

// sanitizeBranchComponent trims leading/trailing dots and hyphens and strips a
// trailing ".lock" so check-ref-format accepts the result.
func sanitizeBranchComponent(p string) string {
	p = strings.Trim(p, ".-")
	for strings.HasSuffix(p, ".lock") {
		p = strings.TrimSuffix(p, ".lock")
		p = strings.Trim(p, ".-")
	}
	return p
}

func joinBranchComponents(name string) string {
	var parts []string
	for _, p := range strings.Split(name, "/") {
		if p = sanitizeBranchComponent(p); p != "" {
			parts = append(parts, p)
		}
	}
	return strings.Join(parts, "/")
}

func checkRefFormat(branch string) error {
	ctx, cancel := context.WithTimeout(context.Background(), isRepoTimeout)
	defer cancel()
	cmd := Command(ctx, "check-ref-format", "--normalize", "refs/heads/"+branch)
	out, err := cmd.CombinedOutput()
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if msg != "" {
			return fmt.Errorf("invalid branch name %q: %s", branch, msg)
		}
		return fmt.Errorf("invalid branch name %q", branch)
	}
	return nil
}

// RenameBranch renames the currently checked-out branch in worktree to newName
// (`git branch -m`). newName is sanitized with DefaultBranchPrefix. Fails on
// detached HEAD, an empty worktree, or when the target name already exists.
func RenameBranch(ctx context.Context, worktree, newName string) (string, error) {
	return RenameBranchWithPrefix(ctx, worktree, DefaultBranchPrefix, newName)
}

// RenameBranchWithPrefix is RenameBranch using prefix when newName has no slash.
func RenameBranchWithPrefix(ctx context.Context, worktree, prefix, newName string) (string, error) {
	if strings.TrimSpace(worktree) == "" {
		return "", fmt.Errorf("rename branch: empty worktree")
	}
	newName, err := SanitizeBranchNameWithPrefix(prefix, newName)
	if err != nil {
		return "", fmt.Errorf("rename branch: %w", err)
	}
	current, err := CurrentBranch(ctx, worktree)
	if err != nil {
		return "", err
	}
	if current == "" {
		return "", fmt.Errorf("rename branch: detached HEAD")
	}
	if current == newName {
		return newName, nil
	}
	exists := Command(ctx, "-C", worktree, "show-ref", "--verify", "--quiet", "refs/heads/"+newName)
	if err := exists.Run(); err == nil {
		return "", fmt.Errorf("rename branch: %q already exists", newName)
	}
	cmd := Command(ctx, "-C", worktree, "branch", "-m", newName)
	out, err := cmd.CombinedOutput()
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if msg != "" {
			return "", fmt.Errorf("git branch -m %s: %s: %w", newName, msg, err)
		}
		return "", fmt.Errorf("git branch -m %s: %w", newName, err)
	}
	return newName, nil
}

// Add creates the worktree for workspaceID on a unique branch prefix/<slug>
// (prefix/<slug>-N when that name is already taken; prefix defaults to recur),
// based on baseBranch, and returns its location. workspaceID `{project}.{slug}`
// lands at <root>/<project>/<slug>; a legacy kitchen-only id stays a single
// segment. slug is the kitchen leaf used for the git branch. created is true
// only when a new worktree was allocated (false when the directory already
// exists) — callers use that to run setup_cmd exactly once per worktree
// lifetime.
//
// A leftover local branch is never checked out: recycled kitchen ids must spin
// off the current base, not an old tip. Restore attaches a worktree to an
// existing branch (archive resume). If the worktree directory is already
// present, Add returns it with created=false.
//
// A brand-new repo with no commits (unborn HEAD) cannot host a worktree until a
// base commit exists. Add creates an empty initial commit on baseBranch when
// needed — it does not stage the user's untracked files.
func (m *Manager) Add(ctx context.Context, workspaceID, slug, baseBranch string) (Worktree, bool, error) {
	return m.add(ctx, workspaceID, slug, baseBranch, "")
}

// AddAtRef creates a worktree starting at startPoint. When branch is a free
// local name it is created off that tip. When the local head already exists
// and is an ancestor of startPoint it is fast-forwarded (same idea as
// `gh pr checkout` without --force). A local branch that is ahead or diverged
// is left alone; Add allocates a unique recur/<slug> instead so unique commits
// stay reachable. If git refuses (branch already in another worktree), Add is
// the same fallback.
func (m *Manager) AddAtRef(ctx context.Context, workspaceID, slug, branch, startPoint string) (Worktree, bool, error) {
	branch = strings.TrimSpace(branch)
	startPoint = strings.TrimSpace(startPoint)
	if branch == "" || startPoint == "" {
		return m.Add(ctx, workspaceID, slug, startPoint)
	}
	if m.branchExists(ctx, branch) && !m.sameCommit(ctx, branch, startPoint) {
		if !m.isAncestor(ctx, branch, startPoint) {
			return m.Add(ctx, workspaceID, slug, startPoint)
		}
		if _, err := m.run(ctx, "branch", "-f", branch, startPoint); err != nil {
			return m.Add(ctx, workspaceID, slug, startPoint)
		}
	}
	wt, created, err := m.add(ctx, workspaceID, slug, startPoint, branch)
	if err == nil {
		return wt, created, nil
	}
	return m.Add(ctx, workspaceID, slug, startPoint)
}

// TrySetUpstream points the worktree HEAD at remote/branch when that tracking
// ref already exists (same-repo PR heads). Fork PRs have no origin/<head>, so
// this is a no-op rather than an error.
func (m *Manager) TrySetUpstream(ctx context.Context, worktree, branch string) {
	branch = strings.TrimSpace(branch)
	worktree = strings.TrimSpace(worktree)
	if branch == "" || worktree == "" {
		return
	}
	remote := m.defaultRemote(ctx)
	if remote == "" {
		remote = "origin"
	}
	ref := remote + "/" + branch
	if !m.commitExists(ctx, ref) {
		return
	}
	_, _ = m.runIn(ctx, worktree, "branch", "--set-upstream-to="+ref)
}

// Restore recreates the worktree at workspaceID on branch (the workspace's
// recorded Branch, which may be prefix/<id>-N). If that local head exists it is
// checked out as-is so archive resume keeps history. If it is missing, the
// recorded name is created off baseBranch. An empty branch falls back to
// recur/<kitchen-slug> — the historical namespace — not the live prefix
// setting, so a later prefix change cannot orphan an existing head.
func (m *Manager) Restore(ctx context.Context, workspaceID, branch, baseBranch string) (Worktree, bool, error) {
	branch = strings.TrimSpace(branch)
	if branch == "" {
		branch = PrefixedBranch(DefaultBranchPrefix, WorkspaceSlug(workspaceID))
	}
	return m.add(ctx, workspaceID, WorkspaceSlug(workspaceID), baseBranch, branch)
}

func (m *Manager) add(ctx context.Context, workspaceID, slug, baseBranch, restoreBranch string) (Worktree, bool, error) {
	if err := validateManagedWorkspaceID(workspaceID); err != nil {
		return Worktree{}, false, err
	}
	branch := restoreBranch
	if branch == "" {
		branch = PrefixedBranch(m.Prefix(), slug)
	}
	wtPath := filepath.Join(m.Root, WorktreeRelPath(workspaceID))

	// If a worktree already exists at the target path, report it as-is — but only
	// when the directory is still on disk. A stale git registration after a
	// partial delete must not block a fresh add for a recycled kitchen id.
	if existing, ok, err := m.worktreeAt(ctx, wtPath); err != nil {
		return Worktree{}, false, err
	} else if ok {
		if _, err := os.Stat(wtPath); err == nil {
			return existing, false, nil
		}
		// Dir gone but still registered (often a locked worktree — prune skips
		// those). Force-unregister so recycled kitchen ids can allocate again.
		_ = m.forceRemoveWorktree(ctx, wtPath)
		_, _ = m.run(ctx, "worktree", "prune")
		if _, still, _ := m.worktreeAt(ctx, wtPath); still {
			return Worktree{}, false, fmt.Errorf("stale worktree registration at %q could not be cleared", wtPath)
		}
	}

	// Path exists on disk but isn't registered (common after the managed worktrees
	// root was renamed, e.g. Reloop → Recur). Repair the gitdir linkage and reuse
	// when valid; otherwise wipe the leftover dir so recycled workspace ids don't
	// inherit a severely out-of-date checkout.
	if st, err := os.Stat(wtPath); err == nil && st.IsDir() {
		_, _ = m.run(ctx, "worktree", "repair", wtPath)
		if existing, ok, err := m.worktreeAt(ctx, wtPath); err != nil {
			return Worktree{}, false, err
		} else if ok {
			return existing, false, nil
		}
		if err := os.RemoveAll(wtPath); err != nil {
			return Worktree{}, false, fmt.Errorf("remove stale worktree path %q: %w", wtPath, err)
		}
	}

	if err := os.MkdirAll(filepath.Dir(wtPath), 0o755); err != nil {
		return Worktree{}, false, fmt.Errorf("create worktrees root: %w", err)
	}

	args := []string{"worktree", "add"}
	if restoreBranch != "" && m.branchExists(ctx, branch) {
		// Remove resume: check out the recorded branch as-is so history is kept.
		args = append(args, wtPath, branch)
	} else {
		if restoreBranch == "" {
			branch = m.uniqueBranch(ctx, slug)
		}
		if blocked := m.blockingRefForBranch(ctx, branch); blocked != "" {
			return Worktree{}, false, fmt.Errorf("cannot create branch %q: existing branch %q blocks it (git cannot nest refs under a branch of the same name) — pick a different prefix in Settings or rename that branch", branch, blocked)
		}
		if baseBranch == "" {
			baseBranch = m.defaultBranch(ctx)
		}
		// Remote bases: fetch the tip once at worktree creation so spin-off
		// doesn't require a manual pull (picker may list origin/main etc.).
		if err := m.refreshRemoteBaseIfNeeded(ctx, baseBranch); err != nil {
			return Worktree{}, false, fmt.Errorf("refresh base %q: %w", baseBranch, err)
		}
		if !m.commitExists(ctx, baseBranch) {
			if m.hasAnyCommit(ctx) {
				return Worktree{}, false, fmt.Errorf("base branch %q does not exist (or has no commits)", baseBranch)
			}
			if _, _, isRemote := m.ParseRemoteTracking(ctx, baseBranch); isRemote {
				return Worktree{}, false, fmt.Errorf("base branch %q is a remote ref but is missing — fetch failed or remote is empty", baseBranch)
			}
			if err := m.ensureInitialCommit(ctx, baseBranch); err != nil {
				return Worktree{}, false, err
			}
		}
		// Create the branch off the base in the new worktree.
		args = append(args, "-b", branch, wtPath, baseBranch)
	}
	if _, err := m.run(ctx, args...); err != nil {
		return Worktree{}, false, fmt.Errorf("git worktree add: %w", err)
	}

	head, _ := m.run(ctx, "-C", wtPath, "rev-parse", "HEAD")
	return Worktree{Path: wtPath, Branch: branch, Head: strings.TrimSpace(head)}, true, nil
}

// ensureInitialCommit creates an empty first commit on baseBranch so worktrees
// can be allocated from a fresh `git init`. Untracked files are left alone.
func (m *Manager) ensureInitialCommit(ctx context.Context, baseBranch string) error {
	if baseBranch == "" {
		baseBranch = "main"
	}
	// Align the unborn HEAD name with the requested base before committing.
	if cur, err := m.run(ctx, "symbolic-ref", "--short", "HEAD"); err == nil {
		cur = strings.TrimSpace(cur)
		if cur != "" && cur != baseBranch {
			if _, err := m.run(ctx, "checkout", "-B", baseBranch); err != nil {
				return fmt.Errorf("prepare base branch %q: %w", baseBranch, err)
			}
		}
	}
	name, email, err := Identity(ctx, m.RepoPath)
	if err != nil {
		return err
	}
	args := []string{
		"-c", "user.name=" + name,
		"-c", "user.email=" + email,
		"commit", "--allow-empty",
		"-m", "Initial commit",
		"--author", fmt.Sprintf("%s <%s>", name, email),
	}
	if _, err := m.run(ctx, args...); err != nil {
		return fmt.Errorf("create initial commit on %s: %w", baseBranch, err)
	}
	// If HEAD was already on a different born branch somehow, ensure baseBranch exists.
	if !m.commitExists(ctx, baseBranch) {
		if _, err := m.run(ctx, "branch", "-M", baseBranch); err != nil {
			return fmt.Errorf("rename initial branch to %s: %w", baseBranch, err)
		}
	}
	return nil
}

// defaultBranch returns the repo's current branch name, or "main".
func (m *Manager) defaultBranch(ctx context.Context) string {
	if cur, err := m.run(ctx, "symbolic-ref", "--short", "HEAD"); err == nil {
		if t := strings.TrimSpace(cur); t != "" {
			return t
		}
	}
	return "main"
}

// CommitResult reports the outcome of committing a worktree's changes.
type CommitResult struct {
	Hash      string // the new commit's short hash ("" if nothing was committed)
	Committed bool   // false when the worktree had no changes to commit
	Additions int
	Deletions int
}

// Commit stages all changes in the worktree (`git add -A`) and commits them with
// the given author and message (PRD §11: messages encode role/iteration; the
// ENGINE commits, not the agent, so it is deterministic and testable).
// authorName/authorEmail set author and committer; pass the configured git user
// (D17), not a role name. If the worktree has no changes, it is a no-op
// returning Committed=false (so an agent turn that changed nothing doesn't
// create an empty commit).
func (m *Manager) Commit(ctx context.Context, worktree, message, authorName, authorEmail string) (CommitResult, error) {
	if _, err := m.runIn(ctx, worktree, "add", "-A"); err != nil {
		return CommitResult{}, fmt.Errorf("git add: %w", err)
	}

	clean, err := m.isClean(ctx, worktree)
	if err != nil {
		return CommitResult{}, err
	}
	if clean {
		// A merge whose resolved tree equals HEAD is porcelain-clean but still
		// needs a two-parent commit to clear MERGE_HEAD.
		if out, merr := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", "MERGE_HEAD"); merr != nil || strings.TrimSpace(out) == "" {
			return CommitResult{Committed: false}, nil
		}
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

// ChangedDiff returns the loop branch's changes versus baseBranch: the list of
// changed file paths and the unified diff body (PRD §11, used by rail #5 to inspect
// whether a committed diff touches auth/secret-ish material). It diffs against the
// merge-base of the worktree HEAD and the base so only the loop's own changes show.
// Best-effort: on any git error it returns what it has (possibly empty) so a rail
// check never wedges the loop.
func (m *Manager) ChangedDiff(ctx context.Context, worktree, baseBranch string) (paths []string, body string) {
	base := baseBranch
	if mb, err := m.runIn(ctx, worktree, "merge-base", "HEAD", baseBranch); err == nil {
		if t := strings.TrimSpace(mb); t != "" {
			base = t
		}
	}
	if names, err := m.runIn(ctx, worktree, "diff", "--name-only", base, "HEAD"); err == nil {
		for _, p := range strings.Split(strings.TrimSpace(names), "\n") {
			if p = strings.TrimSpace(p); p != "" {
				paths = append(paths, p)
			}
		}
	}
	if d, err := m.runIn(ctx, worktree, "diff", base, "HEAD"); err == nil {
		body = d
	}
	return paths, body
}

// MergeResult reports the outcome of merging a loop branch into its base (PRD
// §12.1: Local merge = merge the branch into base ON DISK; the user is the sole
// gate). NewHead is the base ref's hash after the merge advanced.
type MergeResult struct {
	NewHead     string
	FastForward bool
}

// DirtyBaseError is returned when a Local merge cannot land because the shared
// base checkout (the project folder, not the workspace worktree) has uncommitted
// changes git would overwrite.
type DirtyBaseError struct {
	Base  string
	Files []string
}

func (e DirtyBaseError) Error() string {
	base := e.Base
	if base == "" {
		base = "base"
	}
	var b strings.Builder
	fmt.Fprintf(&b, "the project folder (%s) has uncommitted changes that would be overwritten", base)
	if n := len(e.Files); n > 0 {
		shown := e.Files
		extra := 0
		if n > 3 {
			shown = e.Files[:3]
			extra = n - 3
		}
		b.WriteString(" (")
		b.WriteString(strings.Join(shown, ", "))
		if extra > 0 {
			fmt.Fprintf(&b, ", and %d more", extra)
		}
		b.WriteString(")")
	}
	b.WriteString(" — commit, stash, or discard them there, then try again")
	return b.String()
}

func dirtyBaseFromGit(base string, err error) error {
	if err == nil || !strings.Contains(err.Error(), "would be overwritten by") {
		return nil
	}
	return DirtyBaseError{Base: base, Files: overwrittenPaths(err.Error())}
}

func mergeOpError(base string, err, fallback error) error {
	if d := dirtyBaseFromGit(base, err); d != nil {
		return d
	}
	return fallback
}

// overwrittenPaths pulls the file list out of git's "would be overwritten by
// merge/checkout" stderr. Returns nil when the marker is absent.
func overwrittenPaths(msg string) []string {
	const marker = "would be overwritten by"
	i := strings.Index(msg, marker)
	if i < 0 {
		return nil
	}
	rest := msg[i:]
	nl := strings.IndexByte(rest, '\n')
	if nl < 0 {
		return nil
	}
	var files []string
	for _, line := range strings.Split(rest[nl+1:], "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		if strings.HasPrefix(line, "Please ") || strings.HasPrefix(line, "Aborting") || strings.HasPrefix(line, "error:") {
			break
		}
		files = append(files, line)
	}
	return files
}

// Merge integrates the loop branch into baseBranch ON DISK in the source
// repository (PRD §12.1 Local PR merge, D53 merge methods). method controls
// the strategy: "merge" (--no-ff), "squash", or "rebase". It runs against the
// main checkout (m.RepoPath), NOT the loop's worktree, so the base ref actually
// advances where the user works. Author/committer are whatever git would use
// (repo/global user.name + user.email); Recur does not override them. On any
// failure it aborts/resets so the repo is never left in a half-merged state.
//
// CONCURRENCY/SAFETY: this touches the shared checkout's HEAD, so a caller
// should treat it as a serialized operation per repo.
func (m *Manager) Merge(ctx context.Context, branch, baseBranch, message, method string) (MergeResult, error) {
	if branch == "" || baseBranch == "" {
		return MergeResult{}, fmt.Errorf("merge: branch and base are required")
	}

	// Land onto a local branch. Remote-tracking bases must not rely on git DWIM
	// (multi-remote repos can create main from the wrong remote or fail as
	// ambiguous) — create/checkout the short name from the selected ref.
	if remote, short, ok := m.ParseRemoteTracking(ctx, baseBranch); ok {
		if m.branchExists(ctx, short) {
			if _, err := m.run(ctx, "checkout", short); err != nil {
				return MergeResult{}, mergeOpError(short, err, fmt.Errorf("git checkout %s: %w", short, err))
			}
		} else {
			start := baseBranch
			if !m.commitExists(ctx, start) {
				start = "refs/remotes/" + remote + "/" + short
			}
			if _, err := m.run(ctx, "checkout", "-B", short, start); err != nil {
				return MergeResult{}, mergeOpError(short, err, fmt.Errorf("git checkout -B %s %s: %w", short, start, err))
			}
		}
		baseBranch = short
	} else if _, err := m.run(ctx, "checkout", baseBranch); err != nil {
		return MergeResult{}, mergeOpError(baseBranch, err, fmt.Errorf("git checkout %s: %w", baseBranch, err))
	}

	switch method {
	case "squash":
		return m.mergeSquash(ctx, branch, baseBranch, message)
	case "rebase":
		return m.mergeRebase(ctx, branch, baseBranch)
	default:
		return m.mergeCommit(ctx, branch, baseBranch, message)
	}
}

// mergeCommit creates a merge commit (--no-ff) so the branch topology is preserved.
func (m *Manager) mergeCommit(ctx context.Context, branch, baseBranch, message string) (MergeResult, error) {
	args := []string{"merge", "--no-edit", "--no-ff"}
	if message != "" {
		args = append(args, "-m", message)
	}
	args = append(args, branch)

	if _, err := m.run(ctx, args...); err != nil {
		_, _ = m.run(ctx, "merge", "--abort")
		return MergeResult{}, mergeOpError(baseBranch, err, fmt.Errorf("git merge %s into %s: %w", branch, baseBranch, err))
	}

	head, _ := m.run(ctx, "rev-parse", "HEAD")
	return MergeResult{NewHead: strings.TrimSpace(head)}, nil
}

// mergeSquash squashes all branch commits into a single commit on the base.
func (m *Manager) mergeSquash(ctx context.Context, branch, baseBranch, message string) (MergeResult, error) {
	if _, err := m.run(ctx, "merge", "--squash", branch); err != nil {
		_, _ = m.run(ctx, "merge", "--abort")
		return MergeResult{}, mergeOpError(baseBranch, err, fmt.Errorf("git merge --squash %s into %s: %w", branch, baseBranch, err))
	}
	if message == "" {
		message = fmt.Sprintf("Squash-merge %s into %s", branch, baseBranch)
	}
	args := []string{"commit", "--no-edit", "-m", message}
	if _, err := m.run(ctx, args...); err != nil {
		_, _ = m.run(ctx, "reset", "--hard", "HEAD")
		return MergeResult{}, fmt.Errorf("git commit (squash %s): %w", branch, err)
	}

	head, _ := m.run(ctx, "rev-parse", "HEAD")
	return MergeResult{NewHead: strings.TrimSpace(head)}, nil
}

// mergeRebase replays branch commits onto the base linearly. It uses
// `git rebase --onto` with explicit commit ranges so the branch ref (which may
// be checked out in a worktree) is never directly checked out in the main repo.
func (m *Manager) mergeRebase(ctx context.Context, branch, baseBranch string) (MergeResult, error) {
	origBase, _ := m.run(ctx, "rev-parse", baseBranch)
	origBase = strings.TrimSpace(origBase)

	// Find the merge-base so we know which commits to replay.
	mbOut, err := m.run(ctx, "merge-base", baseBranch, branch)
	if err != nil {
		return MergeResult{}, fmt.Errorf("git merge-base %s %s: %w", baseBranch, branch, err)
	}
	mb := strings.TrimSpace(mbOut)

	// Replay commits from (merge-base, branch] onto baseBranch. Using
	// --onto baseBranch mb branch would try to checkout branch (blocked by a
	// worktree), so instead we cherry-pick the range.
	args := []string{"cherry-pick", mb + ".." + branch}
	if _, err := m.run(ctx, args...); err != nil {
		_, _ = m.run(ctx, "cherry-pick", "--abort")
		head, _ := m.run(ctx, "rev-parse", "HEAD")
		if strings.TrimSpace(head) != origBase {
			// Abort couldn't restore (HEAD still moved). Skip when abort
			// succeeded: reset --hard would wipe uncommitted project-folder
			// edits that git's abort preserves.
			_, _ = m.run(ctx, "reset", "--hard", origBase)
		}
		return MergeResult{}, mergeOpError(baseBranch, err, fmt.Errorf("git rebase (cherry-pick) %s onto %s: %w", branch, baseBranch, err))
	}

	head, _ := m.run(ctx, "rev-parse", "HEAD")
	return MergeResult{
		NewHead:     strings.TrimSpace(head),
		FastForward: true,
	}, nil
}

// UpdateWorktreeWithBase brings a loop worktree up to date with baseBranch using
// either a merge commit or a rebase (D54). On conflict the git op is aborted so
// the worktree is left clean, and an error is returned.
//
// method is "merge" (default) or "rebase". Unlike Merge (which lands a branch
// onto the base checkout), this operates inside the worktree and leaves the
// shared base branch untouched.
func (m *Manager) UpdateWorktreeWithBase(ctx context.Context, worktree, baseBranch, method string) error {
	if err := m.BeginUpdateWithBase(ctx, worktree, baseBranch, method); err != nil {
		_ = m.AbortUpdateWithBase(ctx, worktree, method)
		return err
	}
	return nil
}

// BeginUpdateWithBase starts a merge or rebase of baseBranch into the worktree.
// Author/committer are whatever git would use (repo/global user.name + user.email);
// Recur does not override them. On conflict it leaves the worktree in the
// conflicted state (does NOT abort) so a Coder can resolve markers; call
// FinishUpdateWithBase or AbortUpdateWithBase.
func (m *Manager) BeginUpdateWithBase(ctx context.Context, worktree, baseBranch, method string) error {
	return m.beginUpdateWithBase(ctx, worktree, baseBranch, method, false)
}

// BeginUpdateWithBaseAutostash is BeginUpdateWithBase with git --autostash, so
// a dirty working copy can pull without first committing leftover edits.
func (m *Manager) BeginUpdateWithBaseAutostash(ctx context.Context, worktree, baseBranch, method string) error {
	return m.beginUpdateWithBase(ctx, worktree, baseBranch, method, true)
}

// StashTip is the SHA of refs/stash, or "" when the stash is empty.
func (m *Manager) StashTip(ctx context.Context, worktree string) string {
	out, err := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", "refs/stash")
	if err != nil {
		return ""
	}
	return strings.TrimSpace(out)
}

// AutostashLeftInStash reports whether git --autostash stored a new stash
// because reapplying leftover edits conflicted after a successful merge/rebase.
// Git still exits 0 in that case; the working tree may contain stash-apply
// conflict markers even though MERGE_HEAD is absent. The leftover edits live
// in the stash until the user pops them.
func (m *Manager) AutostashLeftInStash(ctx context.Context, worktree, tipBefore string) bool {
	tip := m.StashTip(ctx, worktree)
	if tip == "" || tip == tipBefore {
		return false
	}
	subj, err := m.runIn(ctx, worktree, "log", "-1", "--format=%s", "refs/stash")
	if err != nil {
		return false
	}
	return strings.Contains(strings.ToLower(strings.TrimSpace(subj)), "autostash")
}

// ResetWorktreeToHead discards uncommitted tracked changes so the tree matches
// HEAD. Used after an autostash-apply conflict: leftover edits are already in
// the stash. Untracked files are left alone.
func (m *Manager) ResetWorktreeToHead(ctx context.Context, worktree string) error {
	if _, err := m.runIn(ctx, worktree, "reset", "--hard", "HEAD"); err != nil {
		return fmt.Errorf("git reset --hard HEAD: %w", err)
	}
	return nil
}

func (m *Manager) beginUpdateWithBase(ctx context.Context, worktree, baseBranch, method string, autostash bool) error {
	if worktree == "" || baseBranch == "" {
		return fmt.Errorf("update-with-base: worktree and base are required")
	}
	switch method {
	case "rebase":
		args := []string{"rebase"}
		if autostash {
			args = append(args, "--autostash")
		}
		args = append(args, baseBranch)
		if _, err := m.runIn(ctx, worktree, args...); err != nil {
			return fmt.Errorf("git rebase onto %s: %w", baseBranch, err)
		}
		return nil
	default:
		args := []string{"merge", "--no-edit"}
		if autostash {
			args = append(args, "--autostash")
		}
		args = append(args, baseBranch)
		if _, err := m.runIn(ctx, worktree, args...); err != nil {
			return fmt.Errorf("git merge %s into worktree: %w", baseBranch, err)
		}
		return nil
	}
}

// FinishUpdateWithBase stages all resolutions and completes an in-progress
// merge or rebase started by BeginUpdateWithBase.
func (m *Manager) FinishUpdateWithBase(ctx context.Context, worktree, method string) error {
	if _, err := m.runIn(ctx, worktree, "add", "-A"); err != nil {
		return fmt.Errorf("git add (resolve): %w", err)
	}
	switch method {
	case "rebase":
		// GIT_EDITOR=true skips the commit-message editor on continue.
		// Author/committer come from the user's git config (no Recur override).
		if _, err := m.runInEnv(ctx, worktree, []string{"GIT_EDITOR=true"}, "rebase", "--continue"); err != nil {
			return fmt.Errorf("git rebase --continue: %w", err)
		}
		return nil
	default:
		if _, err := m.runIn(ctx, worktree, "commit", "--no-edit"); err != nil {
			return fmt.Errorf("git commit (merge resolve): %w", err)
		}
		return nil
	}
}

// AbortInProgressUpdate aborts whatever merge or rebase is in progress, based
// on MERGE_HEAD / REBASE_HEAD rather than the caller's intended method.
func (m *Manager) AbortInProgressUpdate(ctx context.Context, worktree string) error {
	if out, err := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", "MERGE_HEAD"); err == nil && strings.TrimSpace(out) != "" {
		return m.AbortUpdateWithBase(ctx, worktree, "merge")
	}
	if out, err := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", "REBASE_HEAD"); err == nil && strings.TrimSpace(out) != "" {
		return m.AbortUpdateWithBase(ctx, worktree, "rebase")
	}
	return m.AbortUpdateWithBase(ctx, worktree, "merge")
}

// AbortUpdateWithBase aborts an in-progress merge or rebase in the worktree.
func (m *Manager) AbortUpdateWithBase(ctx context.Context, worktree, method string) error {
	switch method {
	case "rebase":
		_, err := m.runIn(ctx, worktree, "rebase", "--abort")
		return err
	default:
		_, err := m.runIn(ctx, worktree, "merge", "--abort")
		return err
	}
}

// UpdateInProgress reports whether a merge or rebase is in progress in the
// worktree (e.g. after BeginUpdateWithBase hit conflicts and left markers).
func (m *Manager) UpdateInProgress(ctx context.Context, worktree string) bool {
	if out, err := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", "MERGE_HEAD"); err == nil && strings.TrimSpace(out) != "" {
		return true
	}
	if out, err := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", "REBASE_HEAD"); err == nil && strings.TrimSpace(out) != "" {
		return true
	}
	st, err := m.runIn(ctx, worktree, "status", "--porcelain")
	if err != nil {
		return false
	}
	for _, line := range strings.Split(st, "\n") {
		if len(line) >= 2 {
			a, b := line[0], line[1]
			if a == 'U' || b == 'U' || (a == 'A' && b == 'A') || (a == 'D' && b == 'D') {
				return true
			}
		}
	}
	return false
}

// RefHash returns the commit hash a ref currently points at (best effort; "" on
// error). Used by tests/callers to assert the base ref advanced after a merge.
func (m *Manager) RefHash(ctx context.Context, ref string) string {
	out, err := m.run(ctx, "rev-parse", ref)
	if err != nil {
		return ""
	}
	return strings.TrimSpace(out)
}

// isClean reports whether the worktree has no staged or unstaged changes.
func (m *Manager) isClean(ctx context.Context, worktree string) (bool, error) {
	out, err := m.runIn(ctx, worktree, "status", "--porcelain")
	if err != nil {
		return false, fmt.Errorf("git status: %w", err)
	}
	return strings.TrimSpace(out) == "", nil
}

// DiscardRisk describes uncommitted / conflicted state that discard/delete would
// throw away. Missing or unreadable worktrees report zero risk (nothing to lose).
type DiscardRisk struct {
	DirtyCount int
	Unmerged   bool
}

// InspectDiscardRisk reports dirty/unmerged state for a worktree path.
func InspectDiscardRisk(ctx context.Context, worktree string) DiscardRisk {
	if strings.TrimSpace(worktree) == "" {
		return DiscardRisk{}
	}
	if _, err := os.Stat(worktree); err != nil {
		return DiscardRisk{}
	}
	st := StatusMap(ctx, worktree)
	risk := DiscardRisk{DirtyCount: len(st)}
	risk.Unmerged = worktreeUnmerged(ctx, worktree)
	return risk
}

// worktreeUnmerged reports merge/rebase conflicts or an in-progress merge/rebase.
func worktreeUnmerged(ctx context.Context, worktree string) bool {
	for _, ref := range []string{"MERGE_HEAD", "REBASE_HEAD"} {
		cmd := Command(ctx, "-C", worktree, "rev-parse", "-q", "--verify", ref)
		if out, err := cmd.Output(); err == nil && strings.TrimSpace(string(out)) != "" {
			return true
		}
	}
	cmd := Command(ctx, "-C", worktree, "status", "--porcelain")
	out, err := cmd.Output()
	if err != nil {
		return false
	}
	for _, line := range strings.Split(string(out), "\n") {
		if len(line) < 2 {
			continue
		}
		a, b := line[0], line[1]
		if a == 'U' || b == 'U' || (a == 'A' && b == 'A') || (a == 'D' && b == 'D') {
			return true
		}
	}
	return false
}

// Summary returns a short phrase for UI confirm copy, or "" when clean.
func (r DiscardRisk) Summary() string {
	switch {
	case r.Unmerged && r.DirtyCount > 0:
		return "unmerged + dirty"
	case r.Unmerged:
		return "unmerged changes"
	case r.DirtyCount == 1:
		return "1 uncommitted change"
	case r.DirtyCount > 1:
		return fmt.Sprintf("%d uncommitted changes", r.DirtyCount)
	default:
		return ""
	}
}

// stagedDiffStat returns the additions/deletions of the staged changes. Best
// effort: on parse trouble it returns zeros (the commit still succeeds).
func (m *Manager) stagedDiffStat(ctx context.Context, worktree string) (add, del int) {
	out, err := m.runIn(ctx, worktree, "diff", "--cached", "--numstat")
	if err != nil {
		return 0, 0
	}
	for _, line := range strings.Split(strings.TrimSpace(out), "\n") {
		fields := strings.Fields(line)
		if len(fields) < 2 {
			continue
		}
		a, aerr := strconv.Atoi(fields[0])
		d, derr := strconv.Atoi(fields[1])
		if aerr == nil {
			add += a
		}
		if derr == nil {
			del += d
		}
	}
	return add, del
}

// runIn executes git -C <worktree> ... so commands operate on the worktree rather
// than the source checkout. Returns trimmed stdout or an error with git's stderr.
func (m *Manager) runIn(ctx context.Context, worktree string, args ...string) (string, error) {
	return m.runInEnv(ctx, worktree, nil, args...)
}

// runInEnv is runIn with extra environment variables (e.g. GIT_EDITOR=true).
func (m *Manager) runInEnv(ctx context.Context, worktree string, extraEnv []string, args ...string) (string, error) {
	cmd := commandWithEnv(ctx, extraEnv, append([]string{"-C", worktree}, args...)...)
	cmd.Dir = m.RepoPath
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg != "" {
			return stdout.String(), fmt.Errorf("%s: %w", msg, err)
		}
		return stdout.String(), err
	}
	return stdout.String(), nil
}

// Remove tears down the worktree at the given path (force-removing local edits)
// and deletes the on-disk directory when it lives under the managed worktrees
// root. Kitchen workspace ids are recycled; leaving a leftover working copy
// lets Add repair/reuse a stale checkout.
//
// Paths outside m.Root (or equal to it) are refused — Remove must never be able
// to wipe the user's real checkout if a caller passes the wrong path.
//
// It is a no-op when the path is already gone and unregistered (and under root).
func (m *Manager) Remove(ctx context.Context, path string) error {
	if strings.TrimSpace(path) == "" {
		return nil
	}
	if !m.underManagedRoot(path) {
		return fmt.Errorf("refuse to remove worktree path %q: not under managed root %q", path, m.Root)
	}
	gitErr := m.forceRemoveWorktree(ctx, path)
	// Wipe the directory even when git never knew about it (orphan leftover).
	if rmErr := os.RemoveAll(path); rmErr != nil && !os.IsNotExist(rmErr) {
		return fmt.Errorf("remove worktree dir %q: %w", path, rmErr)
	}
	if _, ok, lerr := m.worktreeAt(ctx, path); lerr == nil && ok {
		_, _ = m.run(ctx, "worktree", "prune")
		// Locked entries survive prune; a second force-force usually clears them.
		_ = m.forceRemoveWorktree(ctx, path)
	}
	if _, ok, lerr := m.worktreeAt(ctx, path); lerr == nil && ok {
		if gitErr != nil {
			return fmt.Errorf("git worktree remove %q: %w", path, gitErr)
		}
		return fmt.Errorf("worktree %q still registered after remove", path)
	}
	// Nested layout <root>/<project>/<slug>: drop the project dir when this was
	// the last worktree in it. os.Remove fails (ignored) if the dir is not empty.
	if parent := filepath.Dir(path); m.underManagedRoot(parent) {
		_ = os.Remove(parent)
	}
	return nil
}

// DeleteBranchIfAtBase deletes a local branch whose tip still equals base — a
// freshly minted worktree branch with no unique commits. Used after a failed
// setup so the next allocation can reuse the natural name instead of minting
// recur/<id>-N. Branches that have moved off base are left alone.
func (m *Manager) DeleteBranchIfAtBase(ctx context.Context, branch, base string) error {
	branch = strings.TrimSpace(branch)
	base = strings.TrimSpace(base)
	if branch == "" || base == "" || !m.branchExists(ctx, branch) {
		return nil
	}
	tip, err := m.run(ctx, "rev-parse", branch)
	if err != nil {
		return fmt.Errorf("rev-parse %s: %w", branch, err)
	}
	baseTip, err := m.run(ctx, "rev-parse", base)
	if err != nil {
		return fmt.Errorf("rev-parse %s: %w", base, err)
	}
	if strings.TrimSpace(tip) != strings.TrimSpace(baseTip) {
		return nil
	}
	return m.DeleteLocalBranch(ctx, branch)
}

// DeleteLocalBranch force-deletes a local branch (`git branch -D`). No-op when
// branch is empty or missing. Does not touch remotes.
func (m *Manager) DeleteLocalBranch(ctx context.Context, branch string) error {
	branch = strings.TrimSpace(branch)
	if branch == "" || !m.branchExists(ctx, branch) {
		return nil
	}
	if _, err := m.run(ctx, "branch", "-D", "--", branch); err != nil {
		return fmt.Errorf("git branch -D %s: %w", branch, err)
	}
	return nil
}

// forceRemoveWorktree runs `git worktree remove --force`, retrying with a second
// `--force` when the worktree is locked (a single --force is refused).
func (m *Manager) forceRemoveWorktree(ctx context.Context, path string) error {
	if _, err := m.run(ctx, "worktree", "remove", "--force", path); err == nil {
		return nil
	}
	if _, err := m.run(ctx, "worktree", "remove", "--force", "--force", path); err != nil {
		return err
	}
	return nil
}

// validateManagedWorkspaceID rejects ids that would escape m.Root when joined
// (slashes, "..", empty). `{project}.{slug}` ids become two segments under
// root; legacy kitchen ids stay one. This is belt-and-suspenders.
func validateManagedWorkspaceID(id string) error {
	id = strings.TrimSpace(id)
	if id == "" || id == "." || id == ".." {
		return fmt.Errorf("invalid worktree id %q", id)
	}
	if strings.Contains(id, string(filepath.Separator)) || strings.Contains(id, "/") || strings.Contains(id, "\\") {
		return fmt.Errorf("invalid worktree id %q: must not contain path separators", id)
	}
	rel := WorktreeRelPath(id)
	if rel == "" || rel == "." || rel == ".." || filepath.IsAbs(rel) {
		return fmt.Errorf("invalid worktree id %q", id)
	}
	for _, seg := range strings.Split(rel, string(filepath.Separator)) {
		if seg == "" || seg == "." || seg == ".." {
			return fmt.Errorf("invalid worktree id %q", id)
		}
	}
	if filepath.Clean(rel) != rel {
		return fmt.Errorf("invalid worktree id %q", id)
	}
	return nil
}

// underManagedRoot reports whether path is strictly inside m.Root (not equal to
// the root itself). Paths are symlink-resolved (including missing leaf
// components via the longest existing prefix) so a link that escapes the
// managed root is rejected before RemoveAll, and macOS /var→/private/var
// still compares equal for not-yet-created children.
func (m *Manager) underManagedRoot(path string) bool {
	if strings.TrimSpace(m.Root) == "" || strings.TrimSpace(path) == "" {
		return false
	}
	root, err := resolvePathPrefix(m.Root)
	if err != nil {
		return false
	}
	p, err := resolvePathPrefix(path)
	if err != nil {
		return false
	}
	if p == root {
		return false
	}
	rel, err := filepath.Rel(root, p)
	if err != nil {
		return false
	}
	if rel == "." || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return false
	}
	return true
}

// resolvePathPrefix returns an absolute, symlink-resolved path. When the final
// component(s) do not exist yet, the longest existing prefix is resolved and
// the missing suffix is re-attached.
func resolvePathPrefix(path string) (string, error) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	abs = filepath.Clean(abs)
	if resolved, err := filepath.EvalSymlinks(abs); err == nil {
		return resolved, nil
	}
	suffix := ""
	cur := abs
	for {
		parent := filepath.Dir(cur)
		if parent == cur {
			return abs, nil
		}
		suffix = filepath.Join(filepath.Base(cur), suffix)
		if resolved, err := filepath.EvalSymlinks(parent); err == nil {
			return filepath.Join(resolved, suffix), nil
		}
		cur = parent
	}
}

// List returns every worktree registered for the repository (including the main
// checkout, which git always reports first).
func (m *Manager) List(ctx context.Context) ([]Worktree, error) {
	out, err := m.run(ctx, "worktree", "list", "--porcelain")
	if err != nil {
		return nil, fmt.Errorf("git worktree list: %w", err)
	}
	return parsePorcelain(out), nil
}

// worktreeAt returns the worktree whose path matches (after symlink/abs cleanup),
// if any.
func (m *Manager) worktreeAt(ctx context.Context, path string) (Worktree, bool, error) {
	list, err := m.List(ctx)
	if err != nil {
		return Worktree{}, false, err
	}
	want := cleanPath(path)
	for _, w := range list {
		if cleanPath(w.Path) == want {
			return w, true, nil
		}
	}
	return Worktree{}, false, nil
}

// ListLocalBranches returns local branch names that resolve to a commit, newest
// tip first. An empty / unborn repo (git init, no commits) returns an empty
// slice — there is no real base to branch off yet.
func (m *Manager) ListLocalBranches(ctx context.Context) ([]string, error) {
	out, err := m.run(ctx, "for-each-ref", "--format=%(refname:short)", "--sort=-committerdate", "refs/heads/")
	if err != nil {
		return nil, err
	}
	var branches []string
	for _, line := range strings.Split(out, "\n") {
		name := strings.TrimSpace(line)
		if name == "" {
			continue
		}
		branches = append(branches, name)
	}
	return branches, nil
}

// commitExists reports whether ref resolves to a commit (false for unborn HEAD /
// missing branches — the empty-repo case after `git init`).
func (m *Manager) commitExists(ctx context.Context, ref string) bool {
	if ref == "" {
		return false
	}
	_, err := m.run(ctx, "rev-parse", "--verify", "--quiet", ref+"^{commit}")
	return err == nil
}

func (m *Manager) sameCommit(ctx context.Context, a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	ha, err := m.run(ctx, "rev-parse", "--verify", "--quiet", a+"^{commit}")
	if err != nil {
		return false
	}
	hb, err := m.run(ctx, "rev-parse", "--verify", "--quiet", b+"^{commit}")
	if err != nil {
		return false
	}
	return strings.TrimSpace(ha) == strings.TrimSpace(hb)
}

func (m *Manager) isAncestor(ctx context.Context, ancestor, desc string) bool {
	if ancestor == "" || desc == "" {
		return false
	}
	_, err := m.run(ctx, "merge-base", "--is-ancestor", ancestor, desc)
	return err == nil
}

// CommitExists is the exported form of commitExists for callers outside this package.
func (m *Manager) CommitExists(ctx context.Context, ref string) bool {
	return m.commitExists(ctx, ref)
}

// hasAnyCommit reports whether the repo has at least one commit on any ref.
func (m *Manager) hasAnyCommit(ctx context.Context) bool {
	out, err := m.run(ctx, "rev-list", "-n", "1", "--all")
	return err == nil && strings.TrimSpace(out) != ""
}

// blockingRefForBranch returns an existing local branch that would D/F-conflict
// with creating refs/heads/<branch> — i.e. a parent path that is already a
// branch (creating josh/foo fails when josh exists).
func (m *Manager) blockingRefForBranch(ctx context.Context, branch string) string {
	branch = strings.Trim(branch, "/")
	acc := ""
	segs := strings.Split(branch, "/")
	for i := 0; i < len(segs)-1; i++ {
		if acc == "" {
			acc = segs[i]
		} else {
			acc += "/" + segs[i]
		}
		if m.branchExists(ctx, acc) {
			return acc
		}
	}
	return ""
}

// branchExists reports whether a local branch already exists.
func (m *Manager) branchExists(ctx context.Context, branch string) bool {
	_, err := m.run(ctx, "show-ref", "--verify", "--quiet", "refs/heads/"+branch)
	return err == nil
}

// BaseBranchExists reports whether name is a local head or a remote-tracking
// branch (origin/main) — the same set ListBranches offers as picker choices.
// Tags, raw SHAs, HEAD, and symbolic remote HEADs (origin/HEAD) are rejected
// so a merge-base cannot collapse against the worktree's own HEAD.
func (m *Manager) BaseBranchExists(ctx context.Context, name string) bool {
	name = strings.TrimSpace(name)
	if name == "" || name == "HEAD" || strings.HasSuffix(name, "/HEAD") {
		return false
	}
	if m.branchExists(ctx, name) {
		return true
	}
	_, err := m.run(ctx, "show-ref", "--verify", "--quiet", "refs/remotes/"+name)
	return err == nil
}

// run executes git in the repository and returns trimmed stdout, or an error
// that includes git's stderr.
func (m *Manager) run(ctx context.Context, args ...string) (string, error) {
	return m.runEnv(ctx, nil, args...)
}

// runEnv is run with extra environment variables (KEY=value) appended to the
// process env. Used for fetch (GIT_TERMINAL_PROMPT=0) and similar.
func (m *Manager) runEnv(ctx context.Context, extraEnv []string, args ...string) (string, error) {
	cmd := commandWithEnv(ctx, extraEnv, args...)
	cmd.Dir = m.RepoPath
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg != "" {
			return stdout.String(), fmt.Errorf("%s: %w", msg, err)
		}
		return stdout.String(), err
	}
	return stdout.String(), nil
}

// parsePorcelain parses `git worktree list --porcelain` output into Worktrees.
func parsePorcelain(out string) []Worktree {
	var list []Worktree
	var cur Worktree
	flush := func() {
		if cur.Path != "" {
			list = append(list, cur)
		}
		cur = Worktree{}
	}
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimRight(line, "\r")
		switch {
		case strings.HasPrefix(line, "worktree "):
			flush()
			cur.Path = strings.TrimPrefix(line, "worktree ")
		case strings.HasPrefix(line, "HEAD "):
			cur.Head = strings.TrimPrefix(line, "HEAD ")
		case strings.HasPrefix(line, "branch "):
			// "branch refs/heads/<name>" → keep the short name.
			cur.Branch = strings.TrimPrefix(strings.TrimPrefix(line, "branch "), "refs/heads/")
		case line == "":
			flush()
		}
	}
	flush()
	return list
}

func cleanPath(p string) string {
	abs, err := filepath.Abs(p)
	if err != nil {
		abs = p
	}
	if resolved, err := filepath.EvalSymlinks(abs); err == nil {
		return resolved
	}
	return abs
}

// UnmergedPaths are files git still marks unmerged in worktree (diff-filter=U).
func (m *Manager) UnmergedPaths(ctx context.Context, worktree string) []string {
	out, err := m.runIn(ctx, worktree, "diff", "--name-only", "--diff-filter=U")
	if err != nil {
		return nil
	}
	var paths []string
	for _, p := range strings.Split(strings.TrimSpace(out), "\n") {
		if p = strings.TrimSpace(p); p != "" {
			paths = append(paths, p)
		}
	}
	return paths
}

// MergeHead reports whether a merge is in progress (MERGE_HEAD is set).
func (m *Manager) MergeHead(ctx context.Context, worktree string) bool {
	out, err := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", "MERGE_HEAD")
	return err == nil && strings.TrimSpace(out) != ""
}

// MergeHeadEquals reports whether the in-progress MERGE_HEAD is the same commit as ref
// (typically a child branch). False when no merge is in progress or ref does not resolve.
func (m *Manager) MergeHeadEquals(ctx context.Context, worktree, ref string) bool {
	if strings.TrimSpace(worktree) == "" || strings.TrimSpace(ref) == "" {
		return false
	}
	head, err := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", "MERGE_HEAD")
	if err != nil {
		return false
	}
	other, err := m.runIn(ctx, worktree, "rev-parse", "-q", "--verify", ref+"^{commit}")
	if err != nil {
		return false
	}
	return strings.TrimSpace(head) == strings.TrimSpace(other)
}

// MergeInto merges branch into the branch checked out at worktree with a
// --no-ff merge commit authored as the given identity. On a content conflict it
// leaves the merge IN PROGRESS (conflict markers in place, MERGE_HEAD set) and
// returns the conflicted paths so the caller can resolve and commit — a later
// Commit on the worktree concludes the merge. Any other failure aborts the
// merge and returns the error.
func (m *Manager) MergeInto(ctx context.Context, worktree, branch, message, authorName, authorEmail string) (conflicts []string, err error) {
	if strings.TrimSpace(worktree) == "" || strings.TrimSpace(branch) == "" {
		return nil, fmt.Errorf("merge into: worktree and branch are required")
	}
	args := []string{
		"-c", "user.name=" + authorName,
		"-c", "user.email=" + authorEmail,
		"merge", "--no-ff", "--no-edit",
	}
	if message != "" {
		args = append(args, "-m", message)
	}
	args = append(args, branch)
	if _, merr := m.runInEnv(ctx, worktree, []string{"GIT_EDITOR=true"}, args...); merr != nil {
		conflicts = m.UnmergedPaths(ctx, worktree)
		if len(conflicts) > 0 {
			return conflicts, nil
		}
		_, _ = m.runIn(ctx, worktree, "merge", "--abort")
		return nil, fmt.Errorf("git merge %s: %w", branch, merr)
	}
	return nil, nil
}
