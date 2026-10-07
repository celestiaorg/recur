package app

import (
	"context"
	"errors"
	"fmt"
	"html"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"recur/internal/code"
	"recur/internal/domain"
	"recur/internal/engine"
	gitpkg "recur/internal/git"
)

// CodeService is the observatory code browser / edit surface (ADE phases 1–2, 4).
//
// Methods that walk the tree, read files, or shell out to git take a
// context.Context as their first parameter. Wails injects it per call and
// cancels it when the frontend cancels the returned promise, so navigating away
// from a search — or firing a pathological regex at a monorepo — actually stops
// the work instead of leaving it to run to completion. The ctx parameter is
// invisible in the generated TypeScript.
type CodeService struct {
	base
	index    *code.IndexHub
	loose    *looseFiles
	sessions *EditorSessionService
}

// resolveReadPath is the sandbox root + relative path for a code-browser
// read. Absolute paths are only allowed when the user opened them (loose
// allowlist, including tabs restored from the Files editor session).
func (s *CodeService) resolveReadPath(projectID, workspaceID, path string) (root, rel string, err error) {
	if filepath.IsAbs(path) {
		if !s.ensureLoose(workspaceID, path) {
			return "", "", fmt.Errorf("path is outside the workspace")
		}
		return filepath.Dir(path), filepath.Base(path), nil
	}
	root, _, err = s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return "", "", err
	}
	return root, path, nil
}

// ensureLoose reports whether abs is on the loose-file allowlist, seeding
// from the workspace's persisted editor session so restored tabs work
// after a restart (the in-memory map does not survive process exit).
func (s *CodeService) ensureLoose(workspaceID, abs string) bool {
	if s.loose.ok(abs) {
		return true
	}
	s.seedLooseFromWorkspace(workspaceID)
	return s.loose.ok(abs)
}

func (s *CodeService) seedLooseFromWorkspace(workspaceID string) {
	if s.sessions == nil || workspaceID == "" {
		return
	}
	sess, err := s.sessions.GetEditorSession(workspaceID)
	if err != nil || sess == nil {
		return
	}
	for _, f := range sess.OpenFiles {
		if filepath.IsAbs(f) {
			s.loose.allow(f)
		}
	}
}

// resolveRoot picks the worktree when workspaceID has a live directory, or the
// project repo when no worktree path is recorded (eager create, project browse).
// A recorded WorktreePath whose directory is gone is ErrWorktreeMissing — it
// does not fall back to the project checkout.
func (s *CodeService) resolveRoot(projectID, workspaceID string) (root string, writable bool, err error) {
	root, writable, _, err = s.resolveScope(projectID, workspaceID)
	return root, writable, err
}

// resolveScope resolves the browse root, whether it is writable, and the base
// branch the loop (or project) was spun off — the Changed/Diff baseline — in ONE
// pass. Every code-browser binding needs the root and most also need the base;
// resolving them separately meant two Loop() + two Project() lookups (four
// acquisitions of the store's global RLock) per call, on a surface the
// Observatory polls several times a second.
func (s *CodeService) resolveScope(projectID, workspaceID string) (root string, writable bool, base string, err error) {
	var repo, wt string
	// mainCheckout: the workspace IS the project's real checkout (no worktree,
	// no spin-off base). Its Changed/Diff baseline must stay uncommitted-vs-HEAD
	// (empty base), so it must NOT fall back to the project's base branch below.
	mainCheckout := false
	if workspaceID != "" {
		if l, ok := s.st().Workspace(workspaceID); ok {
			wt = l.WorktreePath
			base = l.BaseBranch
			mainCheckout = l.IsMain
			if projectID == "" {
				projectID = l.ProjectID
			}
		}
	}
	if projectID != "" {
		p, ok := s.st().Project(projectID)
		if !ok {
			return "", false, "", fmt.Errorf("project %q not found", projectID)
		}
		repo = p.RepoPath
		if base == "" && !mainCheckout {
			base = p.BaseBranch
		}
	}
	// Editing is only allowed when we resolved to a live worktree directory.
	// A recorded-but-missing path is ErrWorktreeMissing (no project-repo
	// fallback); ResolveRootInfo reports writable so the path isn't Stat'd again.
	root, writable, err = code.ResolveRootInfo(repo, wt)
	if err != nil {
		return "", false, "", err
	}
	return root, writable, base, nil
}

// emptyIfWorktreeMissing reports a recorded worktree path whose directory is
// gone. Live-refresh reads must return empty (not the project checkout) so
// Observatory does not paint another repo's dirty set as this workspace's.
func emptyIfWorktreeMissing(err error) bool {
	return errors.Is(err, code.ErrWorktreeMissing)
}

// ListTree returns one level of the file tree under path (repo-relative).
// Depth is always 1 so large repos stay usable — the UI lazy-loads children
// when a directory is expanded (pass that dir's path on the next call).
// Entries carry change status versus the loop/project base branch when set;
// deleted paths that are missing on disk are still listed.
//
// Refreshing several directories at once should use ListTrees: this computes
// git status for the one listing it returns.
func (s *CodeService) ListTree(ctx context.Context, projectID, workspaceID, path string) ([]domain.TreeEntry, error) {
	root, _, base, err := s.resolveScope(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	return code.ListTree(ctx, root, path, 1, base)
}

// ListTrees lists several directories in one call, sharing a single git status
// pass across all of them. This is the live-refresh path: re-listing N expanded
// directories one ListTree at a time forked three git processes per directory
// per tick.
//
// The result maps each requested directory (normalized to a forward-slash
// repo-relative path, "" for the repo root) to its children. A directory that
// could not be listed — typically because the agent just deleted it — is absent
// from the map rather than failing the whole batch.
func (s *CodeService) ListTrees(ctx context.Context, projectID, workspaceID string, dirs []string) (map[string][]domain.TreeEntry, error) {
	root, _, base, err := s.resolveScope(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	return code.ListTrees(ctx, root, dirs, 1, base)
}

// GitStatus returns path → m/a/d/u for the resolved root. For a loop worktree
// this is versus the merge-base with the base branch (spin-off point), including
// committed loop work and the working tree. For a bare project checkout with no
// base, porcelain versus HEAD.
func (s *CodeService) GitStatus(ctx context.Context, projectID, workspaceID string) (map[string]string, error) {
	root, _, base, err := s.resolveScope(projectID, workspaceID)
	if emptyIfWorktreeMissing(err) {
		return map[string]string{}, nil
	}
	if err != nil {
		return nil, err
	}
	st := code.GitStatus(ctx, root, base)
	if st == nil {
		return map[string]string{}, nil
	}
	return st, nil
}

// ReadFile returns file metadata (and inlined text for small files) for preview/edit.
// Images and large text leave Content empty; the webview streams those from /workspace-file.
func (s *CodeService) ReadFile(projectID, workspaceID, path string) (domain.FileContent, error) {
	if filepath.IsAbs(path) {
		root, rel, err := s.resolveReadPath(projectID, workspaceID, path)
		if err != nil {
			return domain.FileContent{}, err
		}
		fc, err := code.ReadFile(root, rel, true)
		if err != nil {
			return fc, err
		}
		fc.Path = filepath.ToSlash(path)
		return fc, nil
	}
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return domain.FileContent{}, err
	}
	return code.ReadFile(root, path, writable)
}

// WriteFile saves edits into a loop worktree (refused on the bare project checkout).
// Successful human edits are recorded on the loop activity feed.
func (s *CodeService) WriteFile(projectID, workspaceID, path, content string) error {
	if filepath.IsAbs(path) {
		root, rel, err := s.resolveReadPath(projectID, workspaceID, path)
		if err != nil {
			return err
		}
		if err := s.runLock(workspaceID); err != nil {
			return err
		}
		if err := code.WriteFile(root, rel, content); err != nil {
			return err
		}
		s.recordHumanEdit(workspaceID, "Patched <code>"+html.EscapeString(path)+"</code> (not committed).")
		return nil
	}
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	if !writable {
		return fmt.Errorf("editing is only allowed in a loop worktree — open a loop or use an external editor")
	}
	if err := s.runLock(workspaceID); err != nil {
		return err
	}
	if err := code.WriteFile(root, path, content); err != nil {
		return err
	}
	s.recordHumanEdit(workspaceID, "Patched <code>"+html.EscapeString(path)+"</code> in the worktree (not committed).")
	return nil
}

// ListChangedFiles returns paths changed in the loop RIGHT NOW — computed live
// off disk (including a coding agent's not-yet-committed edits and brand-new
// untracked files) versus the loop's base branch (merge-base with HEAD), so
// the Observatory's "Changed in loop" list tracks the worktree in
// near-real-time — a PR (pr.Files) doesn't exist until the loop converges
// (openPR), which is exactly the window (agent actively editing) this is
// meant to cover.
//
// Live membership matches git.ChangeStatus (and therefore DiffVsBase /
// DiffWorkingVsBase) for an intact worktree. The last-known PR snapshot is
// unioned on top so files stay listed once a loop converges and its worktree
// is torn down — DiffVsBase errors in that degenerate state, so the extra
// paths are intentional and do not apply to a live checkout.
func (s *CodeService) ListChangedFiles(ctx context.Context, workspaceID string) []string {
	seen := map[string]bool{}
	var out []string
	add := func(p string) {
		if p == "" || seen[p] {
			return
		}
		seen[p] = true
		out = append(out, p)
	}
	if l, ok := s.st().Workspace(workspaceID); ok {
		if root, _, base, err := s.resolveScope(l.ProjectID, workspaceID); err == nil {
			for p := range code.GitStatus(ctx, root, base) {
				add(p)
			}
		}
	}
	if pr, ok := s.st().PRByWorkspace(workspaceID); ok {
		for _, f := range pr.Files {
			add(f.Path)
		}
	}
	sort.Strings(out)
	return out
}

// FileDiff returns the diff of path versus the loop/project base branch
// (merge-base with HEAD, working tree included). Untracked files are a full add.
func (s *CodeService) FileDiff(ctx context.Context, projectID, workspaceID, path string) (domain.FileDiff, error) {
	if filepath.IsAbs(path) {
		root, rel, err := s.resolveReadPath(projectID, workspaceID, path)
		if err != nil {
			return domain.FileDiff{}, err
		}
		return code.FileDiff(ctx, root, "", rel)
	}
	root, _, base, err := s.resolveScope(projectID, workspaceID)
	if err != nil {
		return domain.FileDiff{}, err
	}
	return code.FileDiff(ctx, root, base, path)
}

// DiffVsBase returns the Observatory files-changed diff: working tree versus
// the workspace's base branch (merge-base with HEAD, uncommitted included).
// An empty base (main working copy) diffs versus HEAD. Non-git roots return
// an empty list, not an error. Truncated is set when untracked paths were capped.
func (s *CodeService) DiffVsBase(ctx context.Context, projectID, workspaceID string) (domain.FileDiffResult, error) {
	root, _, base, err := s.resolveScope(projectID, workspaceID)
	if err != nil {
		return domain.FileDiffResult{}, err
	}
	if !gitpkg.IsRepo(root) {
		return emptyFileDiffResult(), nil
	}
	pr, err := (&gitpkg.Manager{}).DiffWorkingVsBase(ctx, root, base)
	if err != nil {
		return domain.FileDiffResult{}, err
	}
	return fileDiffResult(pr), nil
}

// DiffWorking returns the Observatory uncommitted stacked diff: working tree
// (staged + unstaged tracked, plus untracked text) versus HEAD. Independent of
// the workspace base branch. Non-git / missing worktree → empty list, not error.
// Truncated is set when untracked paths were capped.
func (s *CodeService) DiffWorking(ctx context.Context, projectID, workspaceID string) (domain.FileDiffResult, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if emptyIfWorktreeMissing(err) {
		return emptyFileDiffResult(), nil
	}
	if err != nil {
		return domain.FileDiffResult{}, err
	}
	if !gitpkg.IsRepo(root) {
		return emptyFileDiffResult(), nil
	}
	pr, err := (&gitpkg.Manager{}).DiffWorkingVsBase(ctx, root, "")
	if err != nil {
		return domain.FileDiffResult{}, err
	}
	return fileDiffResult(pr), nil
}

func emptyFileDiffResult() domain.FileDiffResult {
	return domain.FileDiffResult{Files: []domain.FileDiff{}}
}

func fileDiffResult(pr gitpkg.PRDiff) domain.FileDiffResult {
	files := pr.Files
	if files == nil {
		files = []domain.FileDiff{}
	}
	return domain.FileDiffResult{Files: files, Truncated: pr.Truncated}
}

// ListCommits returns commits for the Observatory Commits tab, newest first.
// With a base branch this is unique-to-this-workspace (merge-base..HEAD).
// The main working copy has no merge-base, so this is recent HEAD history.
// Empty when the root is not a git repo. Per-commit file diffs stay empty
// until CommitDiff.
func (s *CodeService) ListCommits(ctx context.Context, projectID, workspaceID string) ([]domain.Commit, error) {
	root, _, base, err := s.resolveScope(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	if !gitpkg.IsRepo(root) {
		return nil, nil
	}
	commits, err := (&gitpkg.Manager{}).LogCommits(ctx, root, base)
	if err != nil {
		return nil, err
	}
	if commits == nil {
		return []domain.Commit{}, nil
	}
	return commits, nil
}

// CommitDiff returns one commit's unified diff (`git show --first-parent`)
// for the Commits-tab drill-in. Hash may be the short or full SHA.
func (s *CodeService) CommitDiff(ctx context.Context, projectID, workspaceID, hash string) ([]domain.FileDiff, error) {
	hash = strings.TrimSpace(hash)
	if hash == "" {
		return nil, fmt.Errorf("commit hash is required")
	}
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	if !gitpkg.IsRepo(root) {
		return nil, fmt.Errorf("not a git repository")
	}
	files, err := (&gitpkg.Manager{}).CommitDiffFirstParent(ctx, root, hash)
	if err != nil {
		return nil, err
	}
	if files == nil {
		return []domain.FileDiff{}, nil
	}
	return files, nil
}

// FileAtBase returns the contents of path at the loop/project base merge-base.
// Empty string when the file did not exist then (added/untracked). Used by the
// observatory CodeMirror unified merge view.
func (s *CodeService) FileAtBase(ctx context.Context, projectID, workspaceID, path string) (string, error) {
	root, _, base, err := s.resolveScope(projectID, workspaceID)
	if err != nil {
		return "", err
	}
	return code.FileAtBase(ctx, root, base, path)
}

// Search finds substring matches under the project/worktree.
func (s *CodeService) Search(ctx context.Context, projectID, workspaceID, query string) ([]domain.SearchHit, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	return code.Search(ctx, root, query)
}

// SearchAdvanced finds matches with regex/glob/pagination options. The result
// also reports how many files were skipped for exceeding the per-file size cap,
// so the UI can say so rather than implying there were no matches.
func (s *CodeService) SearchAdvanced(ctx context.Context, projectID, workspaceID, query string, opts domain.SearchOpts) (domain.SearchResult, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return domain.SearchResult{}, err
	}
	return code.SearchOpts(ctx, root, query, opts)
}

// ReplaceInFiles replaces matches across the loop worktree (refused on bare project).
func (s *CodeService) ReplaceInFiles(ctx context.Context, projectID, workspaceID, query, replacement string, opts domain.SearchOpts) (domain.ReplaceResult, error) {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return domain.ReplaceResult{}, err
	}
	if !writable {
		return domain.ReplaceResult{}, fmt.Errorf("editing is only allowed in a loop worktree — open a loop or use an external editor")
	}
	if err := s.runLock(workspaceID); err != nil {
		return domain.ReplaceResult{}, err
	}
	res, err := code.ReplaceInFiles(ctx, root, query, replacement, opts)
	if err != nil {
		return domain.ReplaceResult{}, err
	}
	if res.Replacements > 0 {
		s.recordHumanEdit(workspaceID, fmt.Sprintf(
			"Replaced %d occurrence(s) in %d file(s) in the worktree (not committed).",
			res.Replacements, res.FilesChanged,
		))
	}
	return res, nil
}

// ListFiles returns a flat list of repo-relative file paths. Prefer SearchFiles
// for @-mentions / Quick Open — ranking stays in Go and avoids shipping the
// full corpus. May return a partial list while the first build is streaming.
func (s *CodeService) ListFiles(ctx context.Context, projectID, workspaceID string) ([]string, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	if s.index == nil {
		return code.ListFiles(ctx, root)
	}
	key := workspaceID
	if key == "" {
		key = projectID
	}
	return s.index.Paths(ctx, key, root), nil
}

// SearchFiles ranks workspace paths for query (basename / camelCase / fuzzy).
// The index already applies files.exclude (SetFileIndexExclude) plus always-on
// .git / node_modules — @-mentions and Quick Open share that corpus.
// limit <= 0 defaults to 50. filesOnly skips directory hits (Quick Open).
// While the index is still building, Hits may be partial — Building/Ready tell
// the UI whether to keep refreshing.
func (s *CodeService) SearchFiles(ctx context.Context, projectID, workspaceID, query string, limit int, filesOnly bool) (domain.FileSearchResult, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return domain.FileSearchResult{}, err
	}
	if limit <= 0 {
		limit = 50
	}
	key := workspaceID
	if key == "" {
		key = projectID
	}
	if s.index == nil {
		paths, lerr := code.ListFiles(ctx, root)
		if lerr != nil {
			return domain.FileSearchResult{}, lerr
		}
		bases := make([]string, len(paths))
		for i, p := range paths {
			bases[i] = filepath.Base(p)
		}
		return domain.FileSearchResult{
			Hits:  code.RankHits(ctx, paths, bases, query, limit, filesOnly),
			Ready: true,
		}, nil
	}
	return s.index.Search(ctx, key, root, query, limit, filesOnly), nil
}

// SetFileIndexExclude installs VS Code–style files.exclude patterns on the
// path index (same list as the Observatory explorer). Rebuilds warm indexes
// when the set changes so SearchFiles matches explorer-visible files.
func (s *CodeService) SetFileIndexExclude(_ context.Context, patterns []string) error {
	if s.index == nil {
		return nil
	}
	s.index.SetExclude(patterns)
	return nil
}

// RefreshFileIndex forces a full reindex of the workspace (window-focus
// backstop / manual). Returns the current snapshot (possibly still building).
func (s *CodeService) RefreshFileIndex(ctx context.Context, projectID, workspaceID string) ([]string, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	if s.index == nil {
		return code.ListFiles(ctx, root)
	}
	key := workspaceID
	if key == "" {
		key = projectID
	}
	return s.index.Refresh(ctx, key, root).Paths, nil
}

// ListSkills returns Agent Skills (SKILL.md) for the composer `/` menu
// (user-invocable only). Discovery is provider-agnostic. Skills the user
// disabled in Settings → Skills are omitted.
func (s *CodeService) ListSkills(ctx context.Context, projectID, workspaceID string) ([]domain.AgentSkill, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	all, err := code.ListSkillsForMenu(ctx, root)
	if err != nil {
		return nil, err
	}
	globalDisabled := s.st().Settings().DisabledSkills
	var projDisabled, projEnabled []string
	if projectID != "" {
		if proj, ok := s.st().Project(projectID); ok {
			projDisabled = proj.DisabledSkills
			projEnabled = proj.EnabledSkills
		}
	}
	out := make([]domain.AgentSkill, 0, len(all))
	for _, sk := range all {
		if !domain.EffectiveSkillEnabled(sk.Name, globalDisabled, projDisabled, projEnabled) {
			continue
		}
		out = append(out, sk)
	}
	return out, nil
}

// ListAllSkills returns every discovered Agent Skill (including model-only /
// disable-model-invocation) for Settings → Skills toggles. Disabled skills
// remain in the list so toggles stay visible. Empty project/workspace still
// returns user + shipped skills from home roots. A non-empty project/workspace
// that fails to resolve surfaces the error (no silent home-only fallback).
func (s *CodeService) ListAllSkills(ctx context.Context, projectID, workspaceID string) ([]domain.AgentSkill, error) {
	root := ""
	if projectID != "" || workspaceID != "" {
		r, _, err := s.resolveRoot(projectID, workspaceID)
		if err != nil {
			return nil, err
		}
		root = r
	}
	return code.ListSkills(ctx, root)
}

// WorkingStatus returns uncommitted porcelain status (path → m/a/d/u) versus HEAD.
func (s *CodeService) WorkingStatus(ctx context.Context, projectID, workspaceID string) (map[string]string, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if emptyIfWorktreeMissing(err) {
		return map[string]string{}, nil
	}
	if err != nil {
		return nil, err
	}
	st := gitpkg.WorkingStatus(ctx, root)
	if st == nil {
		return map[string]string{}, nil
	}
	return st, nil
}

// Commit stages all worktree changes and commits with the user's git identity.
func (s *CodeService) Commit(ctx context.Context, projectID, workspaceID, message string) (domain.WorktreeCommitResult, error) {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return domain.WorktreeCommitResult{}, err
	}
	if !writable {
		return domain.WorktreeCommitResult{}, fmt.Errorf("commits are only allowed in a loop worktree")
	}
	if !gitpkg.IsRepo(root) {
		return domain.WorktreeCommitResult{}, fmt.Errorf("not a git repository")
	}
	if err := s.runLock(workspaceID); err != nil {
		return domain.WorktreeCommitResult{}, err
	}
	message = strings.TrimSpace(message)
	if message == "" {
		return domain.WorktreeCommitResult{}, fmt.Errorf("commit message is required")
	}
	name, email, err := gitpkg.Identity(ctx, root)
	if err != nil {
		return domain.WorktreeCommitResult{}, err
	}
	m := &gitpkg.Manager{}
	res, err := m.Commit(ctx, root, message, name, email)
	if err != nil {
		return domain.WorktreeCommitResult{}, err
	}
	out := domain.WorktreeCommitResult{
		Hash: res.Hash, Committed: res.Committed,
		Additions: res.Additions, Deletions: res.Deletions,
	}
	if out.Committed {
		s.recordHumanEdit(workspaceID, "Committed <code>"+html.EscapeString(out.Hash)+"</code>: "+html.EscapeString(message))
	}
	return out, nil
}

// GenerateCommitMessage asks the workspace title model for a commit message
// based on the currently staged diff. Requires a writable worktree with staged changes.
func (s *CodeService) GenerateCommitMessage(ctx context.Context, projectID, workspaceID string) (string, error) {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return "", err
	}
	if !writable {
		return "", fmt.Errorf("commit messages can only be generated in a loop worktree")
	}
	if !gitpkg.IsRepo(root) {
		return "", fmt.Errorf("not a git repository")
	}
	diff, err := gitpkg.StagedDiff(ctx, root)
	if err != nil {
		return "", err
	}
	if strings.TrimSpace(diff) == "" {
		return "", fmt.Errorf("stage changes to generate a commit message")
	}
	subjects := gitpkg.RecentCommitSubjects(ctx, root, 8)
	return s.eng.GenerateCommitMessage(ctx, workspaceID, root, diff, subjects)
}

// CommitStaged commits only staged changes with the user's git identity.
func (s *CodeService) CommitStaged(ctx context.Context, projectID, workspaceID, message string) (domain.WorktreeCommitResult, error) {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return domain.WorktreeCommitResult{}, err
	}
	if !writable {
		return domain.WorktreeCommitResult{}, fmt.Errorf("commits are only allowed in a loop worktree")
	}
	if !gitpkg.IsRepo(root) {
		return domain.WorktreeCommitResult{}, fmt.Errorf("not a git repository")
	}
	if err := s.runLock(workspaceID); err != nil {
		return domain.WorktreeCommitResult{}, err
	}
	message = strings.TrimSpace(message)
	if message == "" {
		return domain.WorktreeCommitResult{}, fmt.Errorf("commit message is required")
	}
	name, email, err := gitpkg.Identity(ctx, root)
	if err != nil {
		return domain.WorktreeCommitResult{}, err
	}
	m := &gitpkg.Manager{}
	res, err := m.CommitStaged(ctx, root, message, name, email)
	if err != nil {
		return domain.WorktreeCommitResult{}, err
	}
	out := domain.WorktreeCommitResult{
		Hash: res.Hash, Committed: res.Committed,
		Additions: res.Additions, Deletions: res.Deletions,
	}
	if out.Committed {
		s.recordHumanEdit(workspaceID, "Committed staged <code>"+html.EscapeString(out.Hash)+"</code>: "+html.EscapeString(message))
	}
	return out, nil
}

// DiscardFile restores path to HEAD or deletes it when untracked (worktree only).
func (s *CodeService) DiscardFile(ctx context.Context, projectID, workspaceID, path string) error {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	if !writable {
		return fmt.Errorf("discard is only allowed in a loop worktree")
	}
	if err := s.runLock(workspaceID); err != nil {
		return err
	}
	if err := gitpkg.DiscardFile(ctx, root, path); err != nil {
		return err
	}
	s.recordHumanEdit(workspaceID, "Discarded changes to <code>"+html.EscapeString(path)+"</code>")
	return nil
}

// DiscardAll resets the worktree to HEAD and removes untracked files.
func (s *CodeService) DiscardAll(ctx context.Context, projectID, workspaceID string) error {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	if !writable {
		return fmt.Errorf("discard is only allowed in a loop worktree")
	}
	if err := s.runLock(workspaceID); err != nil {
		return err
	}
	if err := gitpkg.DiscardAll(ctx, root); err != nil {
		return err
	}
	s.recordHumanEdit(workspaceID, "Discarded all uncommitted worktree changes")
	return nil
}

// Stage adds paths to the git index (worktree only).
func (s *CodeService) Stage(ctx context.Context, projectID, workspaceID string, paths []string) error {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	if !writable {
		return fmt.Errorf("staging is only allowed in a loop worktree")
	}
	if err := s.runLock(workspaceID); err != nil {
		return err
	}
	return gitpkg.Stage(ctx, root, paths)
}

// Unstage removes paths from the git index (worktree only).
func (s *CodeService) Unstage(ctx context.Context, projectID, workspaceID string, paths []string) error {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	if !writable {
		return fmt.Errorf("unstaging is only allowed in a loop worktree")
	}
	if err := s.runLock(workspaceID); err != nil {
		return err
	}
	return gitpkg.Unstage(ctx, root, paths)
}

// StagedFiles returns paths currently staged in the worktree index.
// Best-effort read: a non-writable root (project checkout, or a workspace
// whose worktree is not on disk yet — eager create / mid-switch), a
// recorded worktree whose directory is gone, and a plain-folder working
// copy all return an empty list. Observatory live-refresh calls this
// after every save and on workspace switch; a 422 here used to toast
// "staging is only allowed in a loop worktree".
func (s *CodeService) StagedFiles(ctx context.Context, projectID, workspaceID string) ([]string, error) {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if emptyIfWorktreeMissing(err) {
		return []string{}, nil
	}
	if err != nil {
		return nil, err
	}
	if !writable {
		return []string{}, nil
	}
	paths, err := gitpkg.StagedPaths(ctx, root)
	if err != nil {
		return nil, err
	}
	if paths == nil {
		return []string{}, nil
	}
	return paths, nil
}

// Outline returns top-level symbols for a file.
func (s *CodeService) Outline(projectID, workspaceID, path string) ([]domain.OutlineSymbol, error) {
	root, rel, err := s.resolveReadPath(projectID, workspaceID, path)
	if err != nil {
		return nil, err
	}
	return code.Outline(root, rel)
}

// FindDefs returns definition sites for a symbol name.
func (s *CodeService) FindDefs(ctx context.Context, projectID, workspaceID, name string) ([]domain.SymbolDef, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	return code.FindDefs(ctx, root, name)
}

// FindRefs returns references (and defs) for a symbol name.
func (s *CodeService) FindRefs(ctx context.Context, projectID, workspaceID, name string) ([]domain.SymbolRef, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return nil, err
	}
	return code.FindRefs(ctx, root, name)
}

// OpenInEditor opens path:line in an external editor.
func (s *CodeService) OpenInEditor(projectID, workspaceID, path string, line int) error {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	full, err := code.SafeJoin(root, path)
	if err != nil {
		return err
	}
	return code.OpenInEditor(full, line)
}

// RevealInFinder shows the path in the OS file manager.
func (s *CodeService) RevealInFinder(projectID, workspaceID, path string) error {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	full, err := code.SafeJoin(root, path)
	if err != nil {
		return err
	}
	return code.RevealInFinder(full)
}

// AbsPath returns the absolute filesystem path for a repo-relative path.
func (s *CodeService) AbsPath(projectID, workspaceID, path string) (string, error) {
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return "", err
	}
	return code.AbsPath(root, path)
}

// CreateFile creates an empty file in a loop worktree.
func (s *CodeService) CreateFile(projectID, workspaceID, path string) error {
	return s.mutateWorktree(projectID, workspaceID, path, "Created", func(root string) error {
		return code.CreateFile(root, path)
	})
}

// Mkdir creates a directory in a loop worktree.
func (s *CodeService) Mkdir(projectID, workspaceID, path string) error {
	return s.mutateWorktree(projectID, workspaceID, path, "Created folder", func(root string) error {
		return code.Mkdir(root, path)
	})
}

// Rename moves a file or directory within a loop worktree.
func (s *CodeService) Rename(projectID, workspaceID, from, to string) error {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	if !writable {
		return fmt.Errorf("editing is only allowed in a loop worktree — open a loop or use an external editor")
	}
	if err := s.runLock(workspaceID); err != nil {
		return err
	}
	if err := code.Rename(root, from, to); err != nil {
		return err
	}
	s.recordHumanEdit(workspaceID, "Renamed <code>"+html.EscapeString(from)+"</code> → <code>"+html.EscapeString(to)+"</code>")
	return nil
}

// Delete removes a file or directory tree from a loop worktree.
func (s *CodeService) Delete(projectID, workspaceID, path string) error {
	return s.mutateWorktree(projectID, workspaceID, path, "Deleted", func(root string) error {
		return code.Remove(root, path)
	})
}

// Copy duplicates a file or directory within a loop worktree.
func (s *CodeService) Copy(projectID, workspaceID, from, to string) error {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	if !writable {
		return fmt.Errorf("editing is only allowed in a loop worktree — open a loop or use an external editor")
	}
	if err := s.runLock(workspaceID); err != nil {
		return err
	}
	if err := code.Copy(root, from, to); err != nil {
		return err
	}
	s.recordHumanEdit(workspaceID, "Copied <code>"+html.EscapeString(from)+"</code> → <code>"+html.EscapeString(to)+"</code>")
	return nil
}

func (s *CodeService) mutateWorktree(projectID, workspaceID, path, verb string, fn func(root string) error) error {
	root, writable, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return err
	}
	if !writable {
		return fmt.Errorf("editing is only allowed in a loop worktree — open a loop or use an external editor")
	}
	if err := s.runLock(workspaceID); err != nil {
		return err
	}
	if err := fn(root); err != nil {
		return err
	}
	s.recordHumanEdit(workspaceID, verb+" <code>"+html.EscapeString(path)+"</code> in the worktree (not committed).")
	return nil
}

// runLock refuses worktree mutations while the workspace's autopilot actively
// owns the worktree (decision #3: refuse with
// the reason named, never warn-and-allow). Paused/escalated/settled runs
// unlock — no specialist is running at a checkpoint.
func (s *CodeService) runLock(workspaceID string) error {
	if workspaceID == "" {
		return nil
	}
	ws, ok := s.st().Workspace(workspaceID)
	if !ok {
		return nil
	}
	if ws.HasRun() && domain.RunActive(ws.State) {
		return fmt.Errorf("a loop is running in this workspace — the run owns the worktree; pause or stop it before editing")
	}
	return nil
}

func (s *CodeService) recordHumanEdit(workspaceID, text string) {
	if workspaceID == "" {
		return
	}
	item := domain.FeedItem{
		ID:        fmt.Sprintf("human-edit-%d", time.Now().UnixNano()),
		AgentCode: "AR",
		Name:      "You",
		Text:      text,
		Time:      "now",
	}
	s.st().PrependFeed(workspaceID, item)
	s.eng.Bus.Publish(engine.Event{
		Name: "workspace.event",
		Data: map[string]any{"workspaceId": workspaceID, "event": item},
	})
}
