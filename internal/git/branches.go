package git

import (
	"context"
	"fmt"
	"sort"
	"strings"
	"time"
)

// LocalBranchGroup is the Select group heading for local heads in the
// new-workspace base-branch picker.
const LocalBranchGroup = "Local"

// RemoteFetchTimeout bounds a single remote fetch so a hung network /
// credential prompt cannot consume the whole worktree-creation budget.
const RemoteFetchTimeout = 15 * time.Second

// Remotes returns configured remote names (e.g. origin, upstream).
func (m *Manager) Remotes(ctx context.Context) ([]string, error) {
	out, err := m.run(ctx, "remote")
	if err != nil {
		return nil, err
	}
	var remotes []string
	for _, line := range strings.Split(out, "\n") {
		name := strings.TrimSpace(line)
		if name == "" {
			continue
		}
		remotes = append(remotes, name)
	}
	return remotes, nil
}

// ListRemoteBranches returns remote-tracking ref short names (origin/main),
// newest tip first. Symbolic remote HEADs (origin/HEAD) are omitted.
func (m *Manager) ListRemoteBranches(ctx context.Context) ([]string, error) {
	out, err := m.run(ctx, "for-each-ref", "--format=%(refname:short)", "--sort=-committerdate", "refs/remotes/")
	if err != nil {
		return nil, err
	}
	var branches []string
	for _, line := range strings.Split(out, "\n") {
		name := strings.TrimSpace(line)
		if name == "" || strings.HasSuffix(name, "/HEAD") {
			continue
		}
		branches = append(branches, name)
	}
	return branches, nil
}

// ParseRemoteTrackingRef matches ref against an already-loaded remotes list
// (longest remote-name prefix wins). Does not spawn git — callers that already
// have Remotes() should use this to avoid N+1 subprocesses.
func ParseRemoteTrackingRef(remotes []string, ref string) (remote, branch string, ok bool) {
	ref = strings.TrimSpace(ref)
	if ref == "" || len(remotes) == 0 {
		return "", "", false
	}
	ref = strings.TrimPrefix(ref, "refs/remotes/")
	ordered := append([]string(nil), remotes...)
	sort.SliceStable(ordered, func(i, j int) bool {
		return len(ordered[i]) > len(ordered[j])
	})
	for _, r := range ordered {
		prefix := r + "/"
		if strings.HasPrefix(ref, prefix) {
			b := strings.TrimPrefix(ref, prefix)
			if b == "" || b == "HEAD" {
				return "", "", false
			}
			return r, b, true
		}
	}
	return "", "", false
}

// ParseRemoteTracking reports whether ref is remote/branch for a configured
// remote (longest remote-name prefix wins so nested names stay correct).
// The tracking ref need not exist yet — fetch can create it.
func (m *Manager) ParseRemoteTracking(ctx context.Context, ref string) (remote, branch string, ok bool) {
	remotes, err := m.Remotes(ctx)
	if err != nil || len(remotes) == 0 {
		return "", "", false
	}
	return ParseRemoteTrackingRef(remotes, ref)
}

// ShortBranchName returns the branch part of a remote-tracking ref (origin/main
// → main). Local refs and unknown shapes are returned trimmed unchanged.
func (m *Manager) ShortBranchName(ctx context.Context, ref string) string {
	ref = strings.TrimSpace(ref)
	if _, branch, ok := m.ParseRemoteTracking(ctx, ref); ok {
		return branch
	}
	return ref
}

// defaultRemote prefers origin when present, otherwise the first configured remote.
func (m *Manager) defaultRemote(ctx context.Context) string {
	remotes, err := m.Remotes(ctx)
	if err != nil || len(remotes) == 0 {
		return ""
	}
	for _, r := range remotes {
		if r == "origin" {
			return r
		}
	}
	return remotes[0]
}

// CompareRef is the ref CommitsBehind / Update-with-base should use for base.
// A remote-tracking name (origin/develop) is kept. A local name (develop)
// becomes origin/develop when that tracking ref already exists, so a stale
// local branch does not hide that the remote merge base has moved. Local-only
// repos stay on the local name.
func (m *Manager) CompareRef(ctx context.Context, base string) string {
	base = strings.TrimSpace(base)
	if base == "" {
		return ""
	}
	if _, _, ok := m.ParseRemoteTracking(ctx, base); ok {
		return base
	}
	short := strings.TrimPrefix(base, "refs/heads/")
	remote := m.defaultRemote(ctx)
	if remote == "" {
		return base
	}
	ref := remote + "/" + short
	if m.commitExists(ctx, ref) {
		return ref
	}
	return base
}

// FetchCompareRef fetches the remote-tracking counterpart of base (best-effort)
// and returns CompareRef. Call this before CommitsBehind when the UI needs to
// know whether GitHub's merge base has moved — local main / a stale
// origin/main will otherwise report behind=0.
func (m *Manager) FetchCompareRef(ctx context.Context, base string) string {
	base = strings.TrimSpace(base)
	if base == "" {
		return ""
	}
	remote, branch, ok := m.ParseRemoteTracking(ctx, base)
	if !ok {
		remote = m.defaultRemote(ctx)
		branch = strings.TrimPrefix(base, "refs/heads/")
	}
	if remote != "" && branch != "" {
		_ = m.FetchRemoteBranch(ctx, remote, branch)
	}
	return m.CompareRef(ctx, base)
}

// BehindBase is how far worktree HEAD is behind the newest tip of base. When
// base is a local name (main) that also has origin/main, both tips are counted
// and the farther one wins — so a GitHub merge-base advance and a local-main
// commit are both visible. fetch updates the remote-tracking tip first.
// The returned ref is the one Update-with-base should merge.
func (m *Manager) BehindBase(ctx context.Context, worktree, base string, fetch bool) (ref string, behind int, err error) {
	base = strings.TrimSpace(base)
	if worktree == "" || base == "" {
		return "", 0, fmt.Errorf("behind-base: worktree and base are required")
	}
	compare := base
	if fetch {
		compare = m.FetchCompareRef(ctx, base)
	} else {
		compare = m.CompareRef(ctx, base)
	}
	nStored, errStored := m.CommitsBehind(ctx, worktree, base)
	if compare == "" || compare == base {
		return base, nStored, errStored
	}
	nRemote, errRemote := m.CommitsBehind(ctx, worktree, compare)
	if errRemote != nil {
		return base, nStored, errStored
	}
	if errStored != nil || nRemote > nStored {
		return compare, nRemote, nil
	}
	return base, nStored, nil
}

// FetchRemoteBranch updates the remote-tracking tip for remote/branch
// (`git fetch <remote> <branch>`). Disables terminal credential prompts so a
// hung askpass cannot block forever inside the caller's timeout.
func (m *Manager) FetchRemoteBranch(ctx context.Context, remote, branch string) error {
	remote = strings.TrimSpace(remote)
	branch = strings.TrimSpace(branch)
	if remote == "" || branch == "" {
		return fmt.Errorf("fetch: remote and branch are required")
	}
	if _, err := m.runEnv(ctx, []string{"GIT_TERMINAL_PROMPT=0"}, "fetch", "--end-of-options", remote, branch); err != nil {
		return fmt.Errorf("git fetch %s %s: %w", remote, branch, err)
	}
	return nil
}

// FetchPRHead fetches GitHub's pull/N/head (works for same-repo and fork PRs)
// into refs/recur/pr/N and returns that ref. Used to spin a Recur worktree off
// a pull request that Recur did not open.
func (m *Manager) FetchPRHead(ctx context.Context, number int) (string, error) {
	if number <= 0 {
		return "", fmt.Errorf("fetch PR head: invalid number %d", number)
	}
	remote := m.defaultRemote(ctx)
	if remote == "" {
		remote = "origin"
	}
	dst := PRHeadRef(number)
	src := fmt.Sprintf("+pull/%d/head:%s", number, dst)
	if _, err := m.runEnv(ctx, []string{"GIT_TERMINAL_PROMPT=0"}, "fetch", "--end-of-options", remote, src); err != nil {
		return "", fmt.Errorf("git fetch %s pull/%d/head: %w", remote, number, err)
	}
	if !m.commitExists(ctx, dst) {
		return "", fmt.Errorf("git fetch %s pull/%d/head: missing %s", remote, number, dst)
	}
	return dst, nil
}

// PRHeadRef is where FetchPRHead stores GitHub's pull/N/head.
func PRHeadRef(number int) string {
	return fmt.Sprintf("refs/recur/pr/%d", number)
}

// Upstream is a branch's configured remote-tracking ref (origin/main).
type Upstream struct {
	Remote string // origin
	Branch string // main
	Ref    string // origin/main
}

// Upstream returns the worktree HEAD's tracking ref (`@{upstream}`). ok is
// false when HEAD is detached or the branch has no upstream.
func (m *Manager) Upstream(ctx context.Context, worktree string) (Upstream, bool) {
	if strings.TrimSpace(worktree) == "" {
		return Upstream{}, false
	}
	out, err := m.runIn(ctx, worktree, "rev-parse", "--abbrev-ref", "@{upstream}")
	if err != nil {
		return Upstream{}, false
	}
	ref := strings.TrimSpace(out)
	if ref == "" {
		return Upstream{}, false
	}
	remote, branch, ok := m.ParseRemoteTracking(ctx, ref)
	if !ok {
		return Upstream{}, false
	}
	return Upstream{Remote: remote, Branch: branch, Ref: ref}, true
}

// FetchUpstream fetches the worktree's tracking remote/branch. Returns the
// upstream even when fetch fails so callers can still compare against the
// last-known tracking tip.
func (m *Manager) FetchUpstream(ctx context.Context, worktree string) (Upstream, error) {
	up, ok := m.Upstream(ctx, worktree)
	if !ok {
		return Upstream{}, fmt.Errorf("this branch is not tracking a remote")
	}
	return up, m.FetchRemoteBranch(ctx, up.Remote, up.Branch)
}

// PushUpstream pushes HEAD to the configured upstream branch
// (`git push <remote> HEAD:refs/heads/<branch>`). The local checkout name may
// differ from the upstream branch (e.g. `work` tracking `origin/main`); pushing
// the upstream branch name alone would send a different local ref — or fail.
// Disables terminal credential prompts so a hung askpass cannot block forever.
func (m *Manager) PushUpstream(ctx context.Context, worktree string) error {
	up, ok := m.Upstream(ctx, worktree)
	if !ok {
		return fmt.Errorf("this branch is not tracking a remote")
	}
	spec := "HEAD:refs/heads/" + up.Branch
	if _, err := m.runInEnv(ctx, worktree, []string{"GIT_TERMINAL_PROMPT=0"}, "push", "--end-of-options", up.Remote, spec); err != nil {
		return fmt.Errorf("git push %s %s: %w", up.Remote, spec, err)
	}
	return nil
}

// PushBranch pushes the local branch ref to remote (refs/heads/<branch>).
// Used after a workspace land onto a remote-tracking base (origin/main → push main).
func (m *Manager) PushBranch(ctx context.Context, remote, branch string) error {
	remote = strings.TrimSpace(remote)
	branch = strings.TrimSpace(branch)
	if remote == "" || branch == "" {
		return fmt.Errorf("push: remote and branch are required")
	}
	spec := "refs/heads/" + branch + ":refs/heads/" + branch
	if _, err := m.runEnv(ctx, []string{"GIT_TERMINAL_PROMPT=0"}, "push", "--end-of-options", remote, spec); err != nil {
		return fmt.Errorf("git push %s %s: %w", remote, spec, err)
	}
	return nil
}

// SuggestBaseBranch picks a sensible default base for a new project:
// origin/main → local main → origin/master → local master → origin's HEAD
// symbolic ref → current checkout → first local → first remote → "main".
func (m *Manager) SuggestBaseBranch(ctx context.Context) string {
	pick := func(refs ...string) string {
		for _, ref := range refs {
			if ref != "" && m.commitExists(ctx, ref) {
				return ref
			}
		}
		return ""
	}
	if b := pick("origin/main", "main", "origin/master", "master"); b != "" {
		return b
	}
	// origin/HEAD often tracks the remote's default branch (origin/develop, etc.).
	if out, err := m.run(ctx, "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"); err == nil {
		if b := strings.TrimSpace(out); b != "" && m.commitExists(ctx, b) {
			return b
		}
	}
	if cur, err := m.run(ctx, "symbolic-ref", "--quiet", "--short", "HEAD"); err == nil {
		if b := strings.TrimSpace(cur); b != "" && m.commitExists(ctx, b) {
			return b
		}
	}
	if locals, err := m.ListLocalBranches(ctx); err == nil {
		for _, b := range locals {
			if b != "" {
				return b
			}
		}
	}
	if remotes, err := m.ListRemoteBranches(ctx); err == nil {
		for _, b := range remotes {
			if b != "" {
				return b
			}
		}
	}
	return "main"
}

// SuggestBaseBranchForRepo is SuggestBaseBranch for an arbitrary checkout path.
// Non-git / empty paths return "main".
func SuggestBaseBranchForRepo(ctx context.Context, repoPath string) string {
	repoPath = strings.TrimSpace(repoPath)
	if repoPath == "" || !IsRepo(repoPath) {
		return "main"
	}
	return NewManager(repoPath, "").SuggestBaseBranch(ctx)
}

// refreshRemoteBaseIfNeeded fetches a remote-tracking base before spinning a
// worktree off it. The fetch uses a short sub-timeout so a hung network cannot
// burn the whole worktree-creation budget; on failure, a fresh context checks
// whether a stale tip is still usable.
func (m *Manager) refreshRemoteBaseIfNeeded(ctx context.Context, baseBranch string) error {
	remote, branch, ok := m.ParseRemoteTracking(ctx, baseBranch)
	if !ok {
		return nil
	}
	fetchCtx, cancel := context.WithTimeout(ctx, RemoteFetchTimeout)
	err := m.FetchRemoteBranch(fetchCtx, remote, branch)
	cancel()
	if err == nil {
		return nil
	}
	// Parent ctx may already be expired after a hung fetch — don't reuse it.
	checkCtx, checkCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer checkCancel()
	if m.commitExists(checkCtx, baseBranch) {
		return nil
	}
	return err
}
