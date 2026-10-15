// Package app holds the Wails-bound service structs — the only surface the
// frontend may call (PRD §15.1). Each service reads/mutates the store and
// publishes events on the bus for live mutations. The frontend never reaches
// into the engine directly (PRD §4 frontend note).
//
// M0: getters return seeded data; mutations are clearly-labeled stubs that still
// drive the correct events so the UI behaves end-to-end. Real orchestration is M2+.
package app

import (
	"context"
	"fmt"
	"log"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"

	"recur/internal/bundles"
	"recur/internal/code"
	"recur/internal/codex"
	"recur/internal/copyfiles"
	"recur/internal/cursor"
	"recur/internal/domain"
	"recur/internal/engine"
	gitpkg "recur/internal/git"
	"recur/internal/lsp"
	"recur/internal/observe"
	"recur/internal/osfonts"
	"recur/internal/store"
	"recur/internal/term"
	"recur/internal/usage"
)

// base carries the shared dependencies into every service.
type base struct {
	eng *engine.Engine
}

func (b base) st() *store.Store { return b.eng.Store }

// Services bundles all bound services so main can register them in one place.
type Services struct {
	Projects      *ProjectsService
	Workspaces    *WorkspacesService
	Maestro       *MaestroService
	Escalation    *EscalationService
	Permissions   *PermissionsService
	Questions     *QuestionsService
	Artifacts     *ArtifactsService
	Agents        *AgentsService
	PRs           *PRsService
	Policy        *PolicyService
	Inbox         *InboxService
	Settings      *SettingsService
	Code          *CodeService
	Observatory   *ObservatoryService
	Terminal      *TerminalService
	Bundle        *BundleService
	EditorSession *EditorSessionService
	Shell         *ShellService
	LSP           *LSPService
	Briefing      *BriefingService
}

// NewServices wires every service to the engine.
func NewServices(eng *engine.Engine) *Services {
	b := base{eng: eng}
	// Terminal output rides the same Bus → Wails emitter path as every other event
	// (PRD §6.3) rather than opening a second channel to the frontend. A WebSocket
	// would be the textbook transport for a PTY, but the Wails asset server only
	// supports them in dev mode, and standing up a separate localhost listener buys
	// a port and an auth token for a stream the bus already carries — the term
	// package coalesces output into ~8ms chunks precisely so it can.
	termMgr := term.NewManager(func(name string, data any) {
		eng.Bus.Publish(engine.Event{Name: name, Data: data})
	})
	eng.SetTerminals(termMgr)
	termSvc := &TerminalService{base: b, mgr: termMgr, persistDirty: map[string]bool{}, chrome: map[string]terminalDockChrome{}}
	termMgr.SetOnDirty(termSvc.markDirty)
	dataDir, _ := store.DataDir()
	loose := newLooseFiles()
	lspMgr := lsp.NewManager(func(name string, data any) {
		eng.Bus.Publish(engine.Event{Name: name, Data: data})
	}, dataDir)
	codeSvc := &CodeService{base: b, loose: loose}
	// Status-only: search ranks in Go via SearchFiles — do not ship the full
	// path corpus across the bridge (that froze @-mentions on large trees).
	codeSvc.index = code.NewIndexHub(dataDir, func(workspaceID string, snap code.Snapshot) {
		eng.Bus.Publish(engine.Event{Name: "workspace.file_index", Data: map[string]any{
			"workspaceId": workspaceID,
			"gen":         snap.Gen,
			"ready":       snap.Ready,
			"building":    snap.Building,
		}})
	})
	editorSession := &EditorSessionService{base: b}
	codeSvc.sessions = editorSession
	// File-index lifecycle: pin watched workspace, splice/dirty on FS events,
	// rebuild on focus only when dirty (cold workspaces stay unloaded).
	eng.Bus.Subscribe(func(ev engine.Event) {
		if ev.Name == "workspace.deleted" || ev.Name == "workspace.removed" {
			data, _ := ev.Data.(map[string]any)
			if wsID, _ := data["workspaceId"].(string); wsID != "" {
				editorSession.forget(wsID)
				termSvc.forget(wsID)
				if codeSvc.index != nil {
					codeSvc.index.Drop(wsID)
				}
			}
		}
		if codeSvc.index == nil {
			return
		}
		switch ev.Name {
		case "workspace.watched":
			data, _ := ev.Data.(map[string]any)
			wsID, _ := data["workspaceId"].(string)
			codeSvc.index.SetPinned(wsID)
		case "workspace.fs_changed":
			data, _ := ev.Data.(map[string]any)
			wsID, _ := data["workspaceId"].(string)
			if wsID == "" {
				return
			}
			root, _, err := codeSvc.resolveRoot("", wsID)
			if err != nil {
				return
			}
			rescan, _ := data["rescan"].(bool)
			var rels []string
			if raw, ok := data["paths"].([]string); ok {
				rels = raw
			} else if raw, ok := data["paths"].([]any); ok {
				for _, v := range raw {
					if s, ok := v.(string); ok {
						rels = append(rels, s)
					}
				}
			}
			codeSvc.index.ApplyChanges(context.Background(), wsID, root, rels, rescan)
		case "app.focused":
			wsID := eng.WatchedWorkspace()
			if wsID == "" {
				return
			}
			root, _, err := codeSvc.resolveRoot("", wsID)
			if err != nil {
				return
			}
			codeSvc.index.RefreshIfDirty(context.Background(), wsID, root)
		}
	})
	svc := &Services{
		Projects:      &ProjectsService{b},
		Workspaces:    &WorkspacesService{base: b, loose: loose},
		Maestro:       &MaestroService{b},
		Escalation:    &EscalationService{b},
		Permissions:   &PermissionsService{b},
		Questions:     &QuestionsService{b},
		Artifacts:     &ArtifactsService{b},
		Agents:        &AgentsService{b},
		PRs:           &PRsService{b},
		Policy:        &PolicyService{b},
		Inbox:         &InboxService{b},
		Settings:      &SettingsService{base: b, lspMgr: lspMgr},
		Code:          codeSvc,
		Observatory:   &ObservatoryService{b},
		Terminal:      termSvc,
		Bundle:        &BundleService{base: b},
		EditorSession: editorSession,
		Shell:         &ShellService{},
		LSP:           &LSPService{base: b, mgr: lspMgr},
		Briefing:      &BriefingService{b},
	}
	return svc
}

// ---- Projects (PRD §15.1) ----

type ProjectsService struct{ base }

// isGitProbeConcurrency caps parallel IsRepo probes in ListProjects so boot
// doesn't spawn one git process per project unbounded (network mounts, etc.).
const isGitProbeConcurrency = 8

func (s *ProjectsService) ListProjects() []domain.Project {
	out := s.st().Projects()
	stampIsGit(out)
	return out
}

func (s *ProjectsService) GetProject(id string) (domain.Project, error) {
	p, ok := s.st().Project(id)
	if !ok {
		return domain.Project{}, fmt.Errorf("project %q not found", id)
	}
	return withIsGit(p), nil
}

// withIsGit stamps the computed IsGit flag (not persisted) for UI gating of
// worktree-based workspace creation.
func withIsGit(p domain.Project) domain.Project {
	p.IsGit = gitpkg.IsRepo(p.RepoPath)
	return p
}

// stampIsGit fills IsGit on each project concurrently (bounded), so ListProjects
// on boot doesn't serialize N git probes.
func stampIsGit(projects []domain.Project) {
	if len(projects) == 0 {
		return
	}
	sem := make(chan struct{}, isGitProbeConcurrency)
	var wg sync.WaitGroup
	for i := range projects {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			projects[i].IsGit = gitpkg.IsRepo(projects[i].RepoPath)
		}(i)
	}
	wg.Wait()
}

func (s *ProjectsService) ListProjectGroups() []domain.ProjectGroup {
	return s.st().ProjectGroups()
}

// AddProjectGroup creates a sidebar project group. A UUID is assigned when id is
// empty; new groups default to Open=true.
func (s *ProjectsService) AddProjectGroup(g domain.ProjectGroup) (domain.ProjectGroup, error) {
	if g.Name == "" {
		g.Name = "New group"
	}
	if g.ID == "" {
		g.ID = uuid.Must(uuid.NewV7()).String()
	}
	if !s.st().HasProjectGroup(g.ID) {
		g.Open = true
	}
	return s.st().AddProjectGroup(g)
}

// UpdateProjectGroup persists name/open edits for an existing group.
func (s *ProjectsService) UpdateProjectGroup(g domain.ProjectGroup) (domain.ProjectGroup, error) {
	if strings.TrimSpace(g.Name) == "" {
		return domain.ProjectGroup{}, fmt.Errorf("group name is required")
	}
	return s.st().UpdateProjectGroup(g)
}

// DeleteProjectGroup removes a group; its projects become ungrouped.
func (s *ProjectsService) DeleteProjectGroup(id string) error {
	return s.st().DeleteProjectGroup(id)
}

// ReorderProjectGroups persists a new sidebar group order.
func (s *ProjectsService) ReorderProjectGroups(ids []string) error {
	return s.st().ReorderProjectGroups(ids)
}

// SetProjectGroup assigns a project to a group (empty groupID = ungrouped).
func (s *ProjectsService) SetProjectGroup(projectID, groupID string) (domain.Project, error) {
	p, ok := s.st().Project(projectID)
	if !ok {
		return domain.Project{}, fmt.Errorf("project %q not found", projectID)
	}
	if groupID != "" && !s.st().HasProjectGroup(groupID) {
		return domain.Project{}, fmt.Errorf("project group %q not found", groupID)
	}
	p.GroupID = groupID
	saved, err := s.st().UpdateProjectRunConfig(p)
	if err != nil {
		return saved, err
	}
	return withIsGit(saved), nil
}

// AddProject persists a new project and returns the saved copy (PRD §15.1). A
// stable slug id is derived from the name (or repo path) and de-duplicated; the
// avatar letter and a color are assigned if the caller left them empty.
// New projects default to expanded in the sidebar tree (Open=true).
// Empty BaseBranch is filled via SuggestBaseBranch (origin/main → main → …).
func (s *ProjectsService) AddProject(p domain.Project) (domain.Project, error) {
	if p.ID == "" {
		p.ID = s.uniqueProjectID(slugify(firstNonEmpty(p.Name, p.RepoPath)))
	}
	if p.Name == "" {
		p.Name = p.ID
	}
	if p.Pic == "" && p.Name != "" {
		// First rune (not first byte) so multibyte names get a sensible avatar.
		r, _ := utf8.DecodeRuneInString(p.Name)
		p.Pic = strings.ToUpper(string(r))
	}
	if p.Color == "" {
		p.Color = projectColors[len(s.st().Projects())%len(projectColors)]
	}
	if p.GroupID != "" && !s.st().HasProjectGroup(p.GroupID) {
		return domain.Project{}, fmt.Errorf("project group %q not found", p.GroupID)
	}
	// Sidebar default: expanded. Only apply on create — upserts keep the caller's Open.
	if !s.st().HasProject(p.ID) {
		p.Open = true
	}
	if strings.TrimSpace(p.BaseBranch) == "" {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		p.BaseBranch = gitpkg.SuggestBaseBranchForRepo(ctx, p.RepoPath)
		cancel()
	}
	saved, err := s.st().AddProject(p)
	if err != nil {
		return saved, err
	}
	saved = withIsGit(saved)
	// Seed the working-copy row from the real checkout so the sidebar does not
	// fall back to Project.BaseBranch (often origin/…, not HEAD).
	_, _ = s.eng.OpenMainWorkspace(saved.ID)
	return saved, nil
}

// SuggestBaseBranch returns the preferred default base for a repo path (used by
// the new-project form before the project exists). Non-git paths return "main".
func (s *ProjectsService) SuggestBaseBranch(repoPath string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return gitpkg.SuggestBaseBranchForRepo(ctx, repoPath)
}

// DeleteProject removes a project and everything under it (loops, PRs, inbox rows),
// stopping any running loops first. The frontend re-fetches its lists afterward.
func (s *ProjectsService) DeleteProject(id string) error {
	return s.eng.DeleteProject(id)
}

// UpdateProjectRunConfig persists edits to an existing project (PRD §15.1, §18).
func (s *ProjectsService) UpdateProjectRunConfig(p domain.Project) (domain.Project, error) {
	if p.GroupID != "" && !s.st().HasProjectGroup(p.GroupID) {
		return domain.Project{}, fmt.Errorf("project group %q not found", p.GroupID)
	}
	saved, err := s.st().UpdateProjectRunConfig(p)
	if err != nil {
		return saved, err
	}
	return withIsGit(saved), nil
}

// PreviewCopyFiles lists files in the project's checkout that match the given
// glob patterns (same syntax as Project.CopyFiles). Used by project settings
// to preview what will be copied into new workspaces.
func (s *ProjectsService) PreviewCopyFiles(projectID, patterns string) domain.CopyFilesPreview {
	p, err := s.GetProject(projectID)
	if err != nil {
		return domain.CopyFilesPreview{Error: err.Error()}
	}
	return copyfiles.Preview(context.Background(), p.RepoPath, patterns)
}

// ReorderProjects persists a new sidebar project order. ids must be a complete
// permutation of the current project ids.
func (s *ProjectsService) ReorderProjects(ids []string) error {
	return s.st().ReorderProjects(ids)
}

// uniqueProjectID returns base, or base-2, base-3… if base is already taken.
func (s *ProjectsService) uniqueProjectID(base string) string {
	if base == "" {
		base = "project"
	}
	id := base
	for i := 2; s.takenProjectID(id); i++ {
		id = fmt.Sprintf("%s-%d", base, i)
	}
	return id
}

func (s *ProjectsService) takenProjectID(id string) bool {
	if id == domain.FilesProjectID {
		return true
	}
	return s.st().HasProject(id)
}

// projectColors are the sidebar avatar tints assigned round-robin to new projects.
var projectColors = []string{"var(--cyan)", "var(--amber)", "var(--indigo)", "var(--emerald)", "var(--coral)"}

var slugRE = regexp.MustCompile(`[^a-z0-9]+`)

// slugify lowercases s and collapses non-alphanumerics into single hyphens.
func slugify(s string) string {
	s = strings.ToLower(strings.TrimSpace(s))
	// drop a leading "~/code/" style path, keep the final segment if it's a path.
	if i := strings.LastIndexByte(s, '/'); i >= 0 {
		s = s[i+1:]
	}
	return strings.Trim(slugRE.ReplaceAllString(s, "-"), "-")
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if strings.TrimSpace(v) != "" {
			return v
		}
	}
	return ""
}

// ---- Loops (PRD §15.1) ----

type WorkspacesService struct {
	base
	loose  *looseFiles
	openMu sync.Mutex
}

// ListWorkspaces returns every workspace immediately (cached DiffStat). A
// background single-flight refresh recomputes live git +N/−M and publishes
// workspace.updated when counts change so the sidebar paints without blocking
// boot / OverviewPanel / onLoopEngaged on sequential git shells.
//
// Order is newest-first for the sidebar and Go to palette. The store itself
// stays oldest-first so queued-loop drain still picks the oldest waiting run.
func (s *WorkspacesService) ListWorkspaces() []domain.Workspace {
	s.eng.ScheduleRefreshAllWorkspaceDiffs()
	list := s.st().Workspaces()
	for i, j := 0, len(list)-1; i < j; i, j = i+1, j-1 {
		list[i], list[j] = list[j], list[i]
	}
	return list
}

func (s *WorkspacesService) GetWorkspace(id string) (domain.Workspace, error) {
	l, ok := s.st().Workspace(id)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("loop %q not found", id)
	}
	return l, nil
}

// GetWorkspaceFeed returns the loop's activity feed (newest first), backing the
// Activity tab (PRD §16.4). Live items arrive via the loop.event stream. For a
// loop that ran in a prior session, the in-memory feed is reconstructed from the
// durable append-only events table on first access so a reload shows history
// (PRD §6.5, D13). The M0 seeded feeds are returned as-is.
func (s *WorkspacesService) GetWorkspaceFeed(workspaceID string) []domain.FeedItem {
	feed := s.st().Feed(workspaceID)
	if len(feed) > 0 {
		return feed
	}
	// No cached feed — reconstruct from persisted events (reload path).
	return s.eng.ReconstructFeed(workspaceID)
}

// LastAssistantText returns the last assistant text block in this workspace's
// latest session (newest message, then newest text content). Prefers
// display_text over wire text. Empty when that session has no assistant text yet.
func (s *WorkspacesService) LastAssistantText(workspaceID string) string {
	return s.st().LastAssistantText(workspaceID)
}

// StartWorkspace creates and starts a real loop: a persisted Loop record, a worktree +
// branch off baseBranch (or the project default when empty), and a loop goroutine
// running a Coder agent (PRD §15.1, §11, §19 M2). Returns the new loop id, or ""
// on error (the UI keeps the user on the new-task screen).
//
// autonomy is stamped onto this loop (cautious|balanced|aggressive). Empty uses
// the global policy default; running loops keep their own value if the dial moves.
func (s *WorkspacesService) StartWorkspace(projectID, task, baseBranch, autonomy string) (string, error) {
	return s.eng.StartWorkspace(projectID, task, baseBranch, domain.AutonomyLevel(autonomy))
}

// CreateWorkspace is the single creation path:
// one workspace noun, the MODE decides what starts inside it. mode="loop"
// engages the autopilot immediately — prompt is the run's task (required) and
// autonomy overrides the global default. Any other mode creates the workspace
// and lets the chat dock drive (the frontend seeds prompt as the first chat
// message in that mode). simplify is the New Workspace chip for mode=loop
// (nil → Settings / newest pair-chat inherit); other modes ignore it and seed
// the first chat after create. Returns the stored record immediately; the git
// worktree (and setup command) finish in the background and paint via
// workspace.updated so the UI is not blocked on `git worktree add`.
func (s *WorkspacesService) CreateWorkspace(projectID, name, baseBranch, mode, prompt, autonomy string, simplify *bool) (domain.Workspace, error) {
	return s.CreateWorkspaceWith(projectID, name, baseBranch, mode, prompt, autonomy, "", "", simplify, nil)
}

// CreateWorkspaceWith is CreateWorkspace plus the dock's loop + policy ids.
// Empty loopID / policyID resolve to Settings defaults (or shipped To a PR /
// Balanced). Refuses a missing or disabled id. simplify is the New Workspace
// chip for mode=loop (nil → Settings / inherit). For mode=loop, name is the
// short GoalDisplay label when prompt is expanded prompt-action wire.
// attachments are opening image pastes (mode=loop) so the planner can see them.
func (s *WorkspacesService) CreateWorkspaceWith(projectID, name, baseBranch, mode, prompt, autonomy, loopID, policyID string, simplify *bool, attachments []domain.ChatAttachment) (domain.Workspace, error) {
	if domain.NormalizeChatMode(mode) == domain.ChatModeLoop {
		loopID, policyID = s.resolveEngageIDs(loopID, policyID, autonomy)
		return s.eng.BeginStartWorkspaceWith(projectID, prompt, name, baseBranch, domain.AutonomyLevel(autonomy), loopID, policyID, simplify, attachments)
	}
	ws, err := s.eng.BeginCreateWorkspace(projectID, name, baseBranch)
	if err != nil {
		return ws, err
	}
	if strings.TrimSpace(loopID) != "" || strings.TrimSpace(policyID) != "" {
		return s.SetWorkspaceLoopPolicy(ws.ID, loopID, policyID)
	}
	return ws, nil
}

func (s *WorkspacesService) resolveEngageIDs(loopID, policyID, autonomy string) (string, string) {
	loopID = strings.TrimSpace(loopID)
	policyID = strings.TrimSpace(policyID)
	set := s.eng.Store.Settings()
	if loopID == "" {
		loopID = strings.TrimSpace(set.DefaultLoop)
	}
	if loopID == "" {
		loopID = bundles.DefaultLoopID
	}
	if policyID == "" {
		policyID = strings.TrimSpace(set.DefaultPolicy)
	}
	if policyID == "" {
		policyID = bundles.PolicyFromAutonomy(autonomy)
	}
	return loopID, policyID
}

// SetWorkspaceBaseBranch changes the branch this workspace's diffs, PRs, and
// "update with base" compare against. It does not rebase the worktree. The main
// working copy is refused (it has no merge base).
func (s *WorkspacesService) SetWorkspaceBaseBranch(workspaceID, baseBranch string) (domain.Workspace, error) {
	return s.eng.SetWorkspaceBaseBranch(workspaceID, baseBranch)
}

// SetWorkspaceIssue links a workspace to a tracker issue (replacing any link)
// and assigns the issue to the user when nobody owns it yet. The claim runs
// before returning so the caller can refresh the issue list afterwards.
func (s *WorkspacesService) SetWorkspaceIssue(workspaceID string, issue domain.IssueRef) (domain.Workspace, error) {
	if issue.Source == "" || issue.ID == "" {
		return domain.Workspace{}, fmt.Errorf("issue source and id are required")
	}
	ws, ok := s.st().UpdateWorkspace(workspaceID, func(w *domain.Workspace) { w.Issue = &issue })
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	claimIssue(issue, s.st().Settings())
	return ws, nil
}

// OpenMainWorkspace returns the project's singleton "main working copy"
// workspace — the Observatory scoped to the project's real checkout (not a
// managed worktree) — creating it on first open. It has no Loop mode and no PRs;
// edits and commits land directly in the user's repo. Returns the stored record.
func (s *WorkspacesService) OpenMainWorkspace(projectID string) (domain.Workspace, error) {
	return s.eng.OpenMainWorkspace(projectID)
}

// MergeWorkspace lands a hand-driven workspace onto its merge target. An
// open GitHub PR is refused (merge from the PR page). When the merge
// target has a remote-tracking counterpart (origin/main, including a
// local "main" that tracks it), the land is pushed; a purely local base
// stays on disk. method is "merge"/"squash"/"rebase"; empty =
// project/settings prefs. Returns the updated workspace (state=merged).
func (s *WorkspacesService) MergeWorkspace(workspaceID string, method string) (domain.Workspace, error) {
	return s.eng.MergeWorkspace(workspaceID, domain.MergeMethod(method))
}

// UpdateWorkspaceWithBase brings a hand-driven workspace up to date with its
// merge base. Clean git merge/rebase (no LLM); Status=conflict means markers
// were left and the UI should invoke /update. Empty method uses project/settings
// UpdateBaseMethod prefs.
func (s *WorkspacesService) UpdateWorkspaceWithBase(workspaceID string, method string) (domain.UpdateWorkspaceResult, error) {
	return s.eng.UpdateWorkspaceWithBase(workspaceID, method)
}

// WorkspaceLandState is the titlebar Commit / Update / Merge (or Push) switch.
func (s *WorkspacesService) WorkspaceLandState(id string) domain.WorkspaceLandState {
	return s.eng.WorkspaceLandState(id)
}

// FetchWorkingCopyUpstream refreshes the remote tip the titlebar compares
// against: the main working copy's tracking branch, a workspace's merge base,
// and an open GitHub PR head. Best-effort — never an error.
func (s *WorkspacesService) FetchWorkingCopyUpstream(id string) domain.WorkspaceLandState {
	return s.eng.FetchWorkingCopyUpstream(id)
}

// UpdateWorkingCopyFromRemote fetches the main working copy's upstream and
// merges or rebases it into HEAD. method is "merge" or "rebase"; empty uses
// project/settings UpdateBaseMethod prefs. On conflict the checkout is aborted
// back to its previous state.
func (s *WorkspacesService) UpdateWorkingCopyFromRemote(workspaceID string, method string) (domain.UpdateWorkspaceResult, error) {
	return s.eng.UpdateWorkingCopyFromRemote(workspaceID, domain.UpdateBaseMethod(method))
}

// PushWorkingCopy pushes the main working copy's HEAD to its tracking remote.
func (s *WorkspacesService) PushWorkingCopy(workspaceID string) (domain.Workspace, error) {
	return s.eng.PushWorkingCopy(workspaceID)
}

// OpenWorkspacePR opens a Local (or GitHub, when the project opts in) PR from a
// hand-driven workspace's committed changes. Title/body may be agent-written
// via mcp__recur__open_pr (OpenWorkspacePRWith); the Wails path generates a
// workspace-wide summary versus the base (same job as /open-pr). If a PR is already open, this pushes
// new commits and refreshes the snapshot (titlebar Push updates). force is
// --force-with-lease (titlebar Force push). Merge is the shared
// PRsService.MergePR (a PR is owned by its WorkspaceID either way).
// Returns the PR id. Hand-driven workspaces tracking a local branch use
// MergeWorkspace instead; this remains for GitHub-opt-in projects. The
// titlebar Open PR button invokes /open-pr rather than this method.
func (s *WorkspacesService) OpenWorkspacePR(workspaceID string, force bool) (string, error) {
	return s.eng.OpenWorkspacePR(workspaceID, force)
}

// PullWorkspacePR fetches the open GitHub PR's remote head into the worktree
// when GitHub is ahead of local HEAD (titlebar Pull updates).
func (s *WorkspacesService) PullWorkspacePR(workspaceID string) (domain.UpdateWorkspaceResult, error) {
	return s.eng.PullWorkspacePR(workspaceID)
}

// SendWorkspaceMessage runs one turn of a workspace coding-agent chat tab: an
// interactive Claude Code agent shaped by the tab's chat MODE — agent (default)
// can edit the worktree; ask/plan/review are read-only tool scopes (no loop, no
// Maestro system prompt). code selects the tab (PA, PA2, …; empty → PA). model
// is a qualified ref ("" = that mode's Settings → Models default); flags are the
// dock's per-model dimensions (nil/empty = that mode's defaults); attachments
// are images pasted into the composer (nil/empty = text-only); mode is the
// dock's mode ("" = the tab's persisted mode, else agent); displayText is an
// optional UI label for the user bubble ("" for normal composer sends).
// freshSession, when true, starts a new provider session on this tab instead
// of resuming (prompt actions). Tokens also stream on the bus
// (session.delta, tagged with the chat code); returns the final reply.
func (s *WorkspacesService) SendWorkspaceMessage(workspaceID, code, text, model string, flags map[string]string, attachments []domain.ChatAttachment, mode, displayText string, freshSession bool) string {
	return s.eng.SendWorkspaceTurn(workspaceID, code, text, model, flags, attachments, mode, displayText, freshSession)
}

// ApplyProvisionalChatTitles names an untitled workspace and chat tab from a
// message queued while the worktree is still being set up.
func (s *WorkspacesService) ApplyProvisionalChatTitles(workspaceID, code, text string) {
	s.eng.ApplyProvisionalTitles(workspaceID, code, text)
}

// CancelWorkspaceMessage interrupts this tab's in-flight chat turn and, on a
// loop-engaged tab, parks that chat's walk idle so the conductor can continue.
func (s *WorkspacesService) CancelWorkspaceMessage(workspaceID, code string) {
	s.eng.CancelChat(workspaceID, code)
}

// ListWorkspaceChats returns open coding-agent chat tabs for a solo/pair workspace.
func (s *WorkspacesService) ListWorkspaceChats(workspaceID string) []domain.WorkspaceChatTab {
	return s.eng.ListWorkspaceChats(workspaceID)
}

// CreateWorkspaceChat opens a new coding-agent chat tab on a solo/pair workspace.
func (s *WorkspacesService) CreateWorkspaceChat(workspaceID string) domain.WorkspaceChatTab {
	return s.eng.CreateWorkspaceChat(workspaceID)
}

// ForkWorkspaceChat branches a chat tab at the given turn's message id and
// opens a new PA* tab with the copied transcript. Empty tab if the fork point
// is unknown.
func (s *WorkspacesService) ForkWorkspaceChat(workspaceID, code, messageID string) domain.WorkspaceChatTab {
	return s.eng.ForkWorkspaceChat(workspaceID, code, messageID)
}

// ForkWorkspaceChatToWorkspace creates a new workspace in the same project and
// copies this chat through the given turn onto its default tab.
func (s *WorkspacesService) ForkWorkspaceChatToWorkspace(workspaceID, code, messageID string) (domain.Workspace, error) {
	return s.eng.ForkWorkspaceChatToWorkspace(workspaceID, code, messageID)
}

// WorkspaceChatTurnDiff is the worktree change the agent made during the turn
// closed by messageID (its start vs end checkpoint), shaped as a Commit for
// the Observatory review surface.
func (s *WorkspacesService) WorkspaceChatTurnDiff(workspaceID, code, messageID string) (domain.Commit, error) {
	return s.eng.WorkspaceChatTurnDiff(workspaceID, code, messageID)
}

// ResetWorkspaceChat drops every message after the turn closed by messageID
// and, when a later turn changed files, restores the worktree to that turn's end.
func (s *WorkspacesService) ResetWorkspaceChat(workspaceID, code, messageID string) error {
	return s.eng.ResetWorkspaceChat(workspaceID, code, messageID)
}

// CloseWorkspaceChat closes a coding-agent chat tab. History stays in the session
// store (reopen via ReopenWorkspaceChat / recents). Closing the last tab resets
// to a fresh empty chat. Returns remaining tabs.
func (s *WorkspacesService) CloseWorkspaceChat(workspaceID, code string) []domain.WorkspaceChatTab {
	return s.eng.CloseWorkspaceChat(workspaceID, code)
}

// ListClosedWorkspaceChats returns pair-chat tabs that were closed but still
// have a durable transcript (the Agent tab-strip recents menu).
func (s *WorkspacesService) ListClosedWorkspaceChats(workspaceID string) []domain.WorkspaceChatTab {
	return s.eng.ListClosedWorkspaceChats(workspaceID)
}

// DeleteClosedWorkspaceChat permanently removes a closed chat's transcript.
// Open tabs and in-flight turns are ignored (the latter returns an error).
// Returns the remaining closed chats.
func (s *WorkspacesService) DeleteClosedWorkspaceChat(workspaceID, code string) ([]domain.WorkspaceChatTab, error) {
	return s.eng.DeleteClosedWorkspaceChat(workspaceID, code)
}

// ReopenWorkspaceChat restores a closed pair-chat tab onto the strip. Returns
// the open tabs after reopen.
func (s *WorkspacesService) ReopenWorkspaceChat(workspaceID, code string) []domain.WorkspaceChatTab {
	return s.eng.ReopenWorkspaceChat(workspaceID, code)
}

// ReorderWorkspaceChats persists a new Agent-panel tab strip order. codes must
// be a permutation of the currently open tab codes.
func (s *WorkspacesService) ReorderWorkspaceChats(workspaceID string, codes []string) error {
	_, err := s.eng.ReorderWorkspaceChats(workspaceID, codes)
	return err
}

// SetWorkspaceChatFocus records the chat tab the human is viewing and clears
// its unseen badge. Empty workspaceID clears focus (navigated away).
func (s *WorkspacesService) SetWorkspaceChatFocus(workspaceID, code string) {
	s.eng.SetWorkspaceChatFocus(workspaceID, code)
}

// SetChatUIOccluded tells the engine a modal/dialog is covering the chat
// (New workspace, palettes, permission prompts). While occluded, a finish on
// the selected tab still raises unseen / dock attention (D-attention).
func (s *WorkspacesService) SetChatUIOccluded(occluded bool) {
	s.eng.SetChatUIOccluded(occluded)
}

// MarkWorkspaceChatSeen clears the unseen badge for a chat tab the human just
// viewed (selected / focused after a background turn finished).
func (s *WorkspacesService) MarkWorkspaceChatSeen(workspaceID, code string) {
	s.eng.MarkWorkspaceChatSeen(workspaceID, code)
}

// AttentionCounts returns unseen chat + inbox unread rollups for sidebar and
// dock badges (dock = unseen + non-chat inbox unread; unseen chats are inbox rows).
func (s *WorkspacesService) AttentionCounts() domain.AttentionCounts {
	return s.eng.AttentionCounts()
}

// WatchWorkspace makes workspaceID the single watched workspace: its worktree is
// watched via FSEvents for on-disk changes (from agents, editors, terminals —
// anything) and workspace.fs_changed fires on change so the Observatory tracks
// the disk live. Empty id stops watching (call on leaving the workspace).
func (s *WorkspacesService) WatchWorkspace(workspaceID string) {
	s.eng.WatchWorkspace(workspaceID)
}

// WorkspaceSessions returns a chat's specialist transcripts. An empty agentCode
// loads all its specialists; live completion refreshes only the affected code.
func (s *WorkspacesService) WorkspaceSessions(workspaceID, ownerCode, agentCode string) []domain.SessionView {
	return s.eng.WorkspaceSessions(workspaceID, ownerCode, agentCode)
}

// WorkspaceFacts returns loop-level facts (outcomes, deviations) with real
// timestamps, interleaved into the Loop chat cockpit by the frontend.
func (s *WorkspacesService) WorkspaceFacts(workspaceID string) []domain.FeedItem {
	return s.eng.WorkspaceFacts(workspaceID)
}

// RestartLoop re-runs a stopped or failed loop over its existing worktree — the
// human's version of the Maestro's mcp__recur__resume_loop hand. Returns the
// engine's status string ("Loop restarting…" or a named refusal).
func (s *WorkspacesService) RestartLoop(id string) string {
	return s.eng.Restart(id)
}

// UpdatePlan rewrites a loop's plan (DefinitionOfDone) from the plan editor —
// the same engine path as the Maestro's edit_plan tool: logged
// as a plan-change deviation and steered into subsequent specialist turns.
// Returns the engine's status string ("Plan updated…" or a named refusal).
func (s *WorkspacesService) UpdatePlan(workspaceID, plan string) string {
	return s.eng.EditPlan(workspaceID, plan)
}

// ApprovePlan marks the workspace's structured plan approved (the Plan panel's
// Approve action). Returns a status string.
func (s *WorkspacesService) ApprovePlan(workspaceID string) string {
	return s.eng.ApprovePlan(workspaceID)
}

// SubmitPlan writes a structured plan from the Plan panel (source=human) —
// headline summary, optional detailed body, and checklist steps. stepIDs is
// aligned with steps (blank for a new row) so a retitle keeps status/note.
// Same engine path Plan-mode chats and the loop planner use. Returns a status string.
func (s *WorkspacesService) SubmitPlan(workspaceID, summary, body string, steps, stepIDs []string) string {
	return s.eng.SubmitHumanPlan(workspaceID, summary, body, steps, stepIDs)
}

// UpdatePlanStep changes one plan step's status from the Plan panel (the
// human's version of the agent's update_plan_step hand). Returns a status string.
func (s *WorkspacesService) UpdatePlanStep(workspaceID, stepID, status string) string {
	return s.eng.UpdateHumanPlanStep(workspaceID, stepID, status)
}

// EngageLoop engages the autopilot on an EXISTING workspace with goal as the
// run's task — the Plan panel's "Run as loop" handoff and the chat dock's Loop
// mode share this path. Returns a status string.
func (s *WorkspacesService) EngageLoop(workspaceID, goal string) string {
	loopID, policyID := "", ""
	if ws, ok := s.eng.Store.Workspace(workspaceID); ok {
		loopID, policyID = ws.LoopID, ws.PolicyID
	}
	return s.EngageLoopWith(workspaceID, goal, loopID, policyID)
}

// EngageLoopWith engages the autopilot with explicit loop + policy ids.
// Empty ids fall back to the workspace stamp, then Settings / shipped defaults.
func (s *WorkspacesService) EngageLoopWith(workspaceID, goal, loopID, policyID string) string {
	autonomy := ""
	if ws, ok := s.eng.Store.Workspace(workspaceID); ok {
		if strings.TrimSpace(loopID) == "" {
			loopID = ws.LoopID
		}
		if strings.TrimSpace(policyID) == "" {
			policyID = ws.PolicyID
		}
		autonomy = string(ws.Autonomy)
	}
	loopID, policyID = s.resolveEngageIDs(loopID, policyID, autonomy)
	queued, err := s.eng.StartRunOnWorkspaceWith(workspaceID, goal, loopID, policyID)
	if err != nil {
		return "Refused: " + err.Error()
	}
	if queued {
		return "Loop queued — it starts when a slot frees."
	}
	return "Loop engaged — planning has started."
}

// SetWorkspaceLoopPolicy stamps the dock pickers onto a workspace that has
// not engaged yet. After HasRun() the ids are pinned — switching is a restart.
func (s *WorkspacesService) SetWorkspaceLoopPolicy(workspaceID, loopID, policyID string) (domain.Workspace, error) {
	ws, ok := s.eng.Store.Workspace(workspaceID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if ws.HasRun() {
		return ws, fmt.Errorf("loop and policy are pinned after the run starts — restart to switch")
	}
	loopID = strings.TrimSpace(loopID)
	policyID = strings.TrimSpace(policyID)
	if loopID != "" || policyID != "" {
		if reason := s.eng.GuardReason(loopID, policyID, ws.Autonomy); reason != "" {
			return ws, fmt.Errorf("%s", reason)
		}
	}
	updated, ok := s.eng.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
		if loopID != "" {
			w.LoopID = loopID
		}
		if policyID != "" {
			w.PolicyID = policyID
		}
	})
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	return updated, nil
}

// WorkspaceChatItems returns the FULL agent-chat transcript for one chat tab —
// text, reasoning, and the agent's tool calls, in order — from the session store
// (the single source of truth). code selects the tab (PA, PA2, …; empty → PA).
func (s *WorkspacesService) WorkspaceChatItems(workspaceID, code string) []domain.ChatItem {
	return s.eng.WorkspaceChatItems(workspaceID, code)
}

// WorkspaceChatSelection is the last model + flags + mode the solo/pair chat
// dock used for a chat tab. Empty Model means fall back to that mode's Settings
// → Models default; empty Mode means default to agent. Simplify is the
// composer chip for this session (nil = never set → Settings default).
type WorkspaceChatSelection struct {
	Model    string            `json:"model"`
	Flags    map[string]string `json:"flags,omitempty"`
	Mode     string            `json:"mode,omitempty"`
	Simplify *bool             `json:"simplify,omitempty"`
}

// WorkspaceChatSelection returns the last dock selection for a chat tab
// (code PA, PA2, …; empty → PA).
func (s *WorkspacesService) WorkspaceChatSelection(workspaceID, code string) WorkspaceChatSelection {
	model, flags, mode := s.eng.WorkspaceChatSelection(workspaceID, code)
	return WorkspaceChatSelection{
		Model: model, Flags: flags, Mode: mode,
		Simplify: s.eng.ChatSimplify(workspaceID, code),
	}
}

// RememberWorkspaceChatSelection persists the dock's model + flags + mode + simplify for a
// chat tab without sending a turn — so switching to Review (etc.) survives
// reload even before the first message. simplify nil freezes Settings on first write.
func (s *WorkspacesService) RememberWorkspaceChatSelection(workspaceID, code, model string, flags map[string]string, mode string, simplify *bool) {
	s.eng.RememberWorkspaceChatSelection(workspaceID, code, model, flags, mode, simplify)
}

// RememberWorkspaceChatSimplify persists the composer Simplify chip for a chat
// tab (PA* or Loop/MA) without sending a turn. Settings is only the default for
// new sessions; this is the per-session override.
func (s *WorkspacesService) RememberWorkspaceChatSimplify(workspaceID, code string, on bool) {
	s.eng.RememberWorkspaceChatSimplify(workspaceID, code, on)
}

// RememberWorkspaceChatPromptGroup stamps a [[prompt]] tab_group on this chat tab.
func (s *WorkspacesService) RememberWorkspaceChatPromptGroup(workspaceID, code, group string) {
	s.eng.RememberWorkspaceChatPromptGroup(workspaceID, code, group)
}

// RememberChatLoopPolicy stores a tab's [[loop]] + [[policy]] before that chat engages.
func (s *WorkspacesService) RememberChatLoopPolicy(workspaceID, code, loopID, policyID string) {
	s.eng.RememberChatLoopPolicy(workspaceID, code, loopID, policyID)
}

// WorkspaceChatModels returns the Claude Code model aliases offered in the
// workspace chat's model switcher, with the default first.
func (s *WorkspacesService) WorkspaceChatModels() []string {
	return []string{"sonnet", "opus", "haiku", "fable"}
}

// ListBranches returns base-branch picker choices for a project repo: remote-
// tracking refs first (one group per remote name), then local heads under
// "Local". Within each group tips are newest-first; the project's configured
// BaseBranch is promoted to the front of its group when present. An empty /
// unborn repo returns the intended base branch as a Local choice —
// StartWorkspace will create an empty initial commit on it.
func (s *ProjectsService) ListBranches(projectID string) ([]domain.BranchChoice, error) {
	p, ok := s.st().Project(projectID)
	if !ok {
		return nil, fmt.Errorf("project %q not found", projectID)
	}
	def := p.BaseBranch
	if def == "" {
		def = "main"
	}
	// Saved project config may name a deleted branch — still show it so the
	// user can see/change the setting.
	return listBranchChoices(p.RepoPath, def, true)
}

// ListBranchesForPath is ListBranches for an unsaved new-project form: the
// picker needs remotes/locals before a project id exists. preferred is only
// used to promote an already-existing tip; it is never injected as a phantom
// choice (empty/stale preferred must not invent "main" or a prior repo's base).
func (s *ProjectsService) ListBranchesForPath(repoPath, preferred string) ([]domain.BranchChoice, error) {
	return listBranchChoices(repoPath, strings.TrimSpace(preferred), false)
}

// listBranchChoices builds the grouped base-branch picker for a checkout.
// preferred (def) is promoted within its group — never prepended at index 0 —
// so Select doesn't render a split "Local" heading. When foldMissing is true
// (saved project), a configured default absent from the repo is still listed;
// when false (new-project path), only refs that resolve are folded/promoted.
func listBranchChoices(repoPath, def string, foldMissing bool) ([]domain.BranchChoice, error) {
	def = strings.TrimSpace(def)
	if repoPath == "" || !gitpkg.IsRepo(repoPath) {
		// Non-git folders aren't workspace hosts — empty list so the UI doesn't
		// invent a fake "main" base for a path that can't create worktrees.
		return []domain.BranchChoice{}, nil
	}
	mgr := gitpkg.NewManager(repoPath, "")
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	remotes, _ := mgr.Remotes(ctx)
	sort.Strings(remotes)
	remoteBranches, _ := mgr.ListRemoteBranches(ctx)
	localBranches, err := mgr.ListLocalBranches(ctx)
	if err != nil {
		if foldMissing && def != "" {
			return []domain.BranchChoice{{Name: def, Group: gitpkg.LocalBranchGroup}}, nil
		}
		return []domain.BranchChoice{}, nil
	}

	byRemote := map[string][]string{}
	for _, b := range remoteBranches {
		remote, _, ok := gitpkg.ParseRemoteTrackingRef(remotes, b)
		if !ok {
			continue
		}
		byRemote[remote] = append(byRemote[remote], b)
	}

	// Fold preferred into its group only when it's a saved config (foldMissing)
	// or the tip actually resolves — never invent phantoms for new-project path.
	inject := def != "" && (foldMissing || mgr.CommitExists(ctx, def))
	if inject {
		if remote, _, ok := gitpkg.ParseRemoteTrackingRef(remotes, def); ok {
			if !containsStr(byRemote[remote], def) {
				byRemote[remote] = append([]string{def}, byRemote[remote]...)
			}
			if !containsStr(remotes, remote) {
				remotes = append(remotes, remote)
				sort.Strings(remotes)
			}
		} else if !containsStr(localBranches, def) {
			localBranches = append([]string{def}, localBranches...)
		}
	}

	var out []domain.BranchChoice
	seen := map[string]bool{}
	appendGroup := func(group string, names []string) {
		if len(names) == 0 {
			return
		}
		// Promote preferred default to the front of its group.
		ordered := make([]string, 0, len(names))
		for _, n := range names {
			if n == def {
				ordered = append(ordered, n)
				break
			}
		}
		for _, n := range names {
			if n == def {
				continue
			}
			ordered = append(ordered, n)
		}
		for _, n := range ordered {
			if seen[n] {
				continue
			}
			seen[n] = true
			out = append(out, domain.BranchChoice{Name: n, Group: group})
		}
	}

	for _, remote := range remotes {
		appendGroup(remote, byRemote[remote])
	}
	var extraRemotes []string
	for remote := range byRemote {
		if !containsStr(remotes, remote) {
			extraRemotes = append(extraRemotes, remote)
		}
	}
	sort.Strings(extraRemotes)
	for _, remote := range extraRemotes {
		appendGroup(remote, byRemote[remote])
	}

	appendGroup(gitpkg.LocalBranchGroup, localBranches)

	if len(out) == 0 {
		if foldMissing && def != "" {
			return []domain.BranchChoice{{Name: def, Group: gitpkg.LocalBranchGroup}}, nil
		}
		return []domain.BranchChoice{}, nil
	}
	return out, nil
}

func containsStr(ss []string, want string) bool {
	for _, s := range ss {
		if s == want {
			return true
		}
	}
	return false
}

// PauseLoop / ResumeLoop / StopLoop deliver control commands to the running loop
// via the engine's command channel (PRD §6.2). Stop cancels the loop's context,
// killing its agents (PRD §6.1).
func (s *WorkspacesService) PauseLoop(id string)  { s.eng.Pause(id) }
func (s *WorkspacesService) ResumeLoop(id string) { s.eng.Resume(id) }
func (s *WorkspacesService) StopLoop(id string)   { s.eng.Stop(id) }

// RetryLoop re-runs a loop that stopped on a TRANSIENT provider failure — a spend or
// rate limit, an overload, a dropped connection — from its last durable checkpoint
// (D56). It is a no-op on a loop that failed for any other reason: that would just
// fail again. The loop's Retryable flag says which case it is.
func (s *WorkspacesService) RetryLoop(id string) { s.eng.Retry(id) }

// RemoveWorkspace hides a settled workspace from the default lists while keeping
// its history. Returns after the store is marked archived; worktree teardown
// continues in the background.
func (s *WorkspacesService) RemoveWorkspace(id string) (domain.Workspace, error) {
	return s.eng.RemoveWorkspace(id)
}

// DeleteWorkspace permanently removes a settled loop and everything under it (PRs,
// agents, feed, findings, escalation, inbox rows, on-disk artifacts + worktree).
// Returns after the store record is gone and workspace.deleted has fired; disk
// teardown continues in the background. Used for NewTask cancel / failed create —
// not the titlebar Remove action.
func (s *WorkspacesService) DeleteWorkspace(id string) error {
	return s.eng.DeleteWorkspace(id)
}

// RetryWorkspaceSetup re-runs setup_cmd on a hand workspace whose setup failed
// after the worktree landed. Loop-start failures that deleted the tree must be
// removed and created again.
func (s *WorkspacesService) RetryWorkspaceSetup(id string) error {
	return s.eng.RetryWorkspaceSetup(id)
}

// WorkspaceDiscardRisk returns a short warning when Remove would throw away
// uncommitted or unmerged work (empty when the worktree is clean/missing).
func (s *WorkspacesService) WorkspaceDiscardRisk(id string) string {
	return s.eng.WorkspaceDiscardRisk(id)
}

// ---- Maestro chat (PRD §15.1, §8.5) ----

type MaestroService struct{ base }

// SendMaestroMessage runs a real Maestro chat turn for a loop and returns the
// streamed reply (PRD §8.5, §15.1, M7). It runs an INDEPENDENT `claude -p` turn via
// the engine's AgentRunner (ClaudeRunner in prod, ScriptedRunner in tests) on the
// caller's goroutine — it never blocks a running loop's goroutine. Continuity is via
// --resume <maestro_session> (the engine persists the session id per loop); steering
// (a plan/escalate MCP tool call) is applied by the engine ONLY within the rails — a
// rail can never be crossed via chat (enforced in Go, not the prompt). The turn is
// mirrored to the durable session store (GetMaestroChat reads it back).
func (s *MaestroService) SendMaestroMessage(workspaceID, text string, attachments []domain.ChatAttachment) string {
	return s.eng.SendMaestroMessage(workspaceID, text, attachments)
}

// CancelMaestroMessage interrupts an in-flight Maestro chat turn. No-op if
// nothing is running.
func (s *MaestroService) CancelMaestroMessage(workspaceID string) {
	s.eng.CancelChat(workspaceID, "MA")
}

// GetMaestroChat returns the loop's Maestro transcript (text, reasoning, tool calls)
// from the session store — the single source of truth every chat surface reads — so
// the UI reloads full history across app launches (PRD §8.5).
func (s *MaestroService) GetMaestroChat(workspaceID string) []domain.ChatItem {
	return s.eng.MaestroChatItems(workspaceID)
}

// ---- Escalation (PRD §13) ----

type EscalationService struct{ base }

func (s *EscalationService) GetEscalation(workspaceID string) (domain.Escalation, error) {
	e, ok := s.st().Escalation(workspaceID)
	if !ok {
		return domain.Escalation{}, fmt.Errorf("no escalation for loop %q", workspaceID)
	}
	return e, nil
}

// AnswerEscalation delivers the human's decision to the RUNNING loop and resumes it
// (PRD §13). It routes through the engine, which persists answer + status=answered,
// clears the "needs you" inbox row, emits escalation.resolved, and delivers
// Command{CmdAnswerEscalation} to the loop's command channel so it resumes from the
// blocked checkpoint (re-dispatching first if the loop was interrupted). For the
// seeded LOOP-2218 demo escalation (no live goroutine) the engine still persists
// the answer and moves it out of "needs you" so the UI is honest.
func (s *EscalationService) AnswerEscalation(workspaceID, choice, freeText, agentCode string) (domain.Escalation, error) {
	e, ok := s.eng.AnswerEscalationFor(workspaceID, agentCode, choice, freeText)
	if !ok {
		return domain.Escalation{}, fmt.Errorf("escalation for loop %q is missing or already answered", workspaceID)
	}
	return e, nil
}

// ---- Permissions (--permission-prompt-tool) ----

// PermissionsService answers the tool-permission prompts agents raise while
// Settings.BypassPermissions is off. Unlike an escalation these are not
// persisted: an agent subprocess is holding an open request while it waits, so
// nothing here outlives the turn that asked.
type PermissionsService struct{ base }

// ListPendingPermissions rebuilds the prompt queue after a UI reload, which
// matters because the requests only ever arrived as events.
func (s *PermissionsService) ListPendingPermissions() []domain.PermissionRequest {
	return s.eng.PendingPermissions()
}

// AnswerPermission delivers the human's verdict to the waiting agent. It errors
// when the request is already gone — answered in another window, timed out, or
// its turn ended — so the UI can drop a stale prompt instead of appearing to
// have done something.
func (s *PermissionsService) AnswerPermission(id string, allow bool, reason string, remember bool) error {
	ok := s.eng.AnswerPermission(id, domain.PermissionDecision{
		Allow:    allow,
		Reason:   reason,
		Remember: remember,
	})
	if !ok {
		return fmt.Errorf("permission request %q is no longer waiting", id)
	}
	return nil
}

// ---- Ask-user questions (interactive agents) ----

// QuestionsService answers parked ask_user prompts. Not durable across
// restarts; answering resumes the chat with a new turn.
type QuestionsService struct{ base }

func (s *QuestionsService) ListPendingQuestions() []domain.UserQuestion {
	return s.eng.PendingAskUsers()
}

func (s *QuestionsService) AnswerQuestion(id, choice, freeText string, attachments []domain.ChatAttachment) error {
	if !s.eng.AnswerAskUser(id, choice, freeText, attachments) {
		return fmt.Errorf("question %q is no longer waiting", id)
	}
	return nil
}

// ---- Artifacts ----

type ArtifactsService struct{ base }

func (s *ArtifactsService) ListArtifacts(workspaceID string) ([]domain.Artifact, error) {
	return s.eng.ListArtifacts(workspaceID)
}

func (s *ArtifactsService) PresentArtifact(workspaceID, id string) error {
	msg := s.eng.PresentArtifact(workspaceID, id)
	if strings.HasPrefix(msg, "Artifact not found") || strings.HasPrefix(msg, "Could not") {
		return fmt.Errorf("%s", msg)
	}
	return nil
}

func (s *ArtifactsService) RenameArtifact(workspaceID, id, title string) (domain.Artifact, error) {
	return s.eng.RenameArtifact(workspaceID, id, title)
}

func (s *ArtifactsService) DeleteArtifact(workspaceID, id string) error {
	return s.eng.DeleteArtifact(workspaceID, id)
}

func (s *ArtifactsService) RevealArtifact(workspaceID, id string) error {
	full, err := s.eng.ArtifactAbsPath(workspaceID, id)
	if err != nil {
		return err
	}
	return code.RevealInFinder(full)
}

// ---- Agents (read-only inspection, PRD §8.5) ----

type AgentsService struct{ base }

func (s *AgentsService) ListLoopAgents(workspaceID string) []domain.Agent {
	return s.st().Agents(workspaceID)
}

func (s *AgentsService) GetAgentTrace(workspaceID, code string) (domain.Agent, error) {
	a, ok := s.st().Agent(workspaceID, code)
	if !ok {
		return domain.Agent{}, fmt.Errorf("agent %q not found on loop %q", code, workspaceID)
	}
	return a, nil
}

// ---- PRs (PRD §12, §15.1) ----

type PRsService struct{ base }

func (s *PRsService) ListPRs() []domain.PR { return s.st().PRs() }

// SyncGitHubPRs lists GitHub pull requests for every Recur git project and
// merges them into the dashboard (including PRs Recur did not open).
func (s *PRsService) SyncGitHubPRs() []domain.PR {
	return s.eng.SyncGitHubPRs()
}

// CreateWorkspaceFromPR creates a Recur workspace on this PR's head. Import
// already attaches a workspace that is already on the PR's GitHub head branch.
func (s *PRsService) CreateWorkspaceFromPR(prID string) (domain.Workspace, error) {
	return s.eng.CreateWorkspaceFromPR(prID)
}

// EnsurePRChatHost returns a workspace the PR-detail agent dock can bind to
// when the PR has no Recur worktree yet. CreateWorkspaceFromPR promotes that
// same row in place so chat history survives.
func (s *PRsService) EnsurePRChatHost(prID string) (domain.Workspace, error) {
	return s.eng.EnsurePRChatHost(prID)
}

func (s *PRsService) GetPR(id string) (domain.PR, error) {
	if _, ok := s.st().PR(id); !ok {
		return domain.PR{}, fmt.Errorf("PR %q not found", id)
	}
	// Behind must finish before commits: RefreshPRBehind fetches a stale
	// remote-tracking base (origin/main) that RefreshPRCommits then logs
	// against. Checks are independent. Conversation + diff stay backgrounded.
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		s.eng.RefreshPRBehind(id)
		s.eng.RefreshPRCommits(id)
	}()
	go func() { defer wg.Done(); s.eng.RefreshPRChecks(id) }()
	wg.Wait()
	s.eng.ScheduleRefreshPRDiff(id)
	s.eng.ScheduleRefreshPRConversation(id)
	p, _ := s.st().PR(id)
	s.ensurePREnriched(&p)
	return p, nil
}

func (s *PRsService) GetPRFiles(id string) []domain.FileDiff {
	p, _ := s.st().PR(id)
	s.ensurePREnriched(&p)
	return p.Files
}

func (s *PRsService) GetPRCommits(id string) []domain.Commit {
	p, _ := s.st().PR(id)
	s.ensurePREnriched(&p)
	return p.Commits
}

func (s *PRsService) GetCommitDiff(id, hash string) (domain.Commit, error) {
	return s.eng.EnsureCommitDiff(id, hash)
}

func (s *PRsService) SearchPRMentionableUsers(prID, query string) []domain.PRParticipant {
	return s.eng.SearchPRMentionableUsers(prID, query)
}

// ListPRFindings returns reviewer findings for the PR — the linked workspace
// when one exists, otherwise the hidden PR chat host (imported GitHub PRs).
func (s *PRsService) ListPRFindings(prID string) []domain.Finding {
	p, ok := s.st().PR(prID)
	if !ok {
		return nil
	}
	id := s.eng.PRFindingsWorkspaceID(p)
	if id == "" {
		return nil
	}
	return s.st().Findings(id)
}

// ensurePREnriched backfills syntax highlighting and projects open reviewer
// findings onto diff ExtraHTML (ADE phase 0). Persists when anything changes.
func (s *PRsService) ensurePREnriched(p *domain.PR) {
	s.ensurePRHighlighted(p)
}

// ensurePRHighlighted backfills syntax highlighting onto PRs that were persisted
// before highlighting shipped (their CodeHTML is plain escaped text). Already
// highlighted lines are left alone; when anything changes we write through so
// the next load is cheap. Also projects open findings onto ExtraHTML.
func (s *PRsService) ensurePRHighlighted(p *domain.PR) {
	changed := gitpkg.HighlightFiles(p.Files)
	for i := range p.Commits {
		if gitpkg.HighlightFiles(p.Commits[i].Files) {
			changed = true
		}
	}
	findings := s.st().Findings(s.eng.PRFindingsWorkspaceID(*p))
	if observe.ProjectFindingsOntoDiff(p.Files, findings) {
		changed = true
	}
	if !changed {
		return
	}
	files, commits, impact := p.Files, p.Commits, p.Impact
	if _, ok := s.st().UpdatePR(p.ID, func(pr *domain.PR) {
		pr.Files = files
		pr.Commits = commits
		if len(impact) > 0 {
			pr.Impact = impact
		}
	}); !ok {
		return
	}
}

// AddPRComment records a review note. A file+line note stays pending until
// SubmitReview (or SendBackToLoop). Conversation-only notes (no file) still
// post immediately. startLine is the first line of a multiline comment (0 for
// a single line). findingID tags a line draft queued from an observatory
// finding. Returns the persisted comment (or a stub after a pending add).
func (s *PRsService) AddPRComment(prID, file string, line int, body string, startLine int, findingID string) (domain.PRComment, error) {
	if pr, ok := s.st().PR(prID); ok && pr.ConversationLocked() && strings.TrimSpace(body) != "" {
		return domain.PRComment{}, fmt.Errorf("this pull request is closed")
	}
	c, ok := s.eng.AddPRComment(prID, file, line, body, startLine, findingID)
	if !ok {
		if _, exists := s.st().PR(prID); exists {
			return domain.PRComment{}, fmt.Errorf("could not post the comment to GitHub")
		}
		return domain.PRComment{}, fmt.Errorf("PR %q not found", prID)
	}
	return c, nil
}

// SubmitReview persists a review (Comment/Approve/Request changes) as a thread entry
// and emits pr.comment/pr.updated (PRD §12.2, §14). Approve/Request-changes also
// reflect on the PR state. Returns the updated PR.
func (s *PRsService) SubmitReview(prID, kind, body string) (domain.PR, error) {
	if pr, ok := s.st().PR(prID); ok && pr.ConversationLocked() {
		return domain.PR{}, fmt.Errorf("this pull request is closed")
	}
	p, ok := s.eng.SubmitReview(prID, kind, body)
	if !ok {
		if _, exists := s.st().PR(prID); exists {
			return domain.PR{}, fmt.Errorf("could not submit the review to GitHub")
		}
		return domain.PR{}, fmt.Errorf("PR %q not found", prID)
	}
	return p, nil
}

// UpdatePRDetails edits the title and description. GitHub PRs require the
// viewer to be the author or have write access.
func (s *PRsService) UpdatePRDetails(prID, title, body string) (domain.PR, error) {
	p, ok := s.eng.UpdatePRDetails(prID, title, body)
	if !ok {
		if _, exists := s.st().PR(prID); exists {
			return domain.PR{}, fmt.Errorf("could not update the pull request")
		}
		return domain.PR{}, fmt.Errorf("PR %q not found", prID)
	}
	return p, nil
}

// DeletePRComment removes a line note or standalone conversation comment the
// viewer is allowed to delete. Submitted reviews cannot be deleted.
func (s *PRsService) DeletePRComment(prID, commentID string) (domain.PR, error) {
	p, ok := s.eng.DeletePRComment(prID, commentID)
	if !ok {
		if _, exists := s.st().PR(prID); exists {
			return domain.PR{}, fmt.Errorf("could not delete the comment")
		}
		return domain.PR{}, fmt.Errorf("PR %q not found", prID)
	}
	return p, nil
}

// ReplyPRComment posts a reply on a line-note thread. Conversation comments
// cannot be replied to in-thread.
func (s *PRsService) ReplyPRComment(prID, commentID, body string) (domain.PRComment, error) {
	if pr, ok := s.st().PR(prID); ok && pr.ConversationLocked() {
		return domain.PRComment{}, fmt.Errorf("this pull request is closed")
	}
	c, ok := s.eng.ReplyPRComment(prID, commentID, body)
	if !ok {
		if _, exists := s.st().PR(prID); exists {
			return domain.PRComment{}, fmt.Errorf("could not reply to the comment")
		}
		return domain.PRComment{}, fmt.Errorf("PR %q not found", prID)
	}
	return c, nil
}

// TogglePRCommentReaction adds or removes the viewer's GitHub reaction on a
// conversation item (description, comment, review, or line note).
func (s *PRsService) TogglePRCommentReaction(prID, commentID, content string) (domain.PR, error) {
	p, ok := s.eng.TogglePRCommentReaction(prID, commentID, content)
	if !ok {
		if _, exists := s.st().PR(prID); exists {
			return domain.PR{}, fmt.Errorf("could not update the reaction")
		}
		return domain.PR{}, fmt.Errorf("PR %q not found", prID)
	}
	return p, nil
}

// SetPRThreadResolved marks a line-note conversation resolved or reopens it.
// GitHub: PR author or write access. Local: always allowed.
func (s *PRsService) SetPRThreadResolved(prID, threadID string, resolved bool) (domain.PR, error) {
	p, ok := s.eng.SetPRThreadResolved(prID, threadID, resolved)
	if !ok {
		if _, exists := s.st().PR(prID); exists {
			return domain.PR{}, fmt.Errorf("could not update the conversation")
		}
		return domain.PR{}, fmt.Errorf("PR %q not found", prID)
	}
	return p, nil
}

// UpdatePRComment edits a conversation comment the viewer owns.
func (s *PRsService) UpdatePRComment(prID, commentID, body string) (domain.PRComment, error) {
	c, ok := s.eng.UpdatePRComment(prID, commentID, body)
	if !ok {
		if _, exists := s.st().PR(prID); exists {
			return domain.PRComment{}, fmt.Errorf("could not update the comment")
		}
		return domain.PRComment{}, fmt.Errorf("PR %q not found", prID)
	}
	return c, nil
}

// SendBackToLoop is the PR Send Back control: a conductor turn with the review
// thread attached. Maestro answers or calls reenter_phase.
func (s *PRsService) SendBackToLoop(prID string, comments []string) (domain.PR, error) {
	p, ok := s.eng.SendBackToLoop(prID, comments)
	if !ok {
		return domain.PR{}, fmt.Errorf("send-back failed — the PR or its loop is unavailable")
	}
	return p, nil
}

// UpdateWithBase integrates the latest base tip into the PR worktree (D54).
// Clean path is a direct git merge/rebase; conflicts fall back to a single Coder
// turn. method is "merge" or "rebase";
// empty uses project/settings UpdateBaseMethod prefs.
func (s *PRsService) UpdateWithBase(prID string, method string) (domain.PR, error) {
	p, ok := s.eng.UpdatePRWithBase(prID, method)
	if !ok {
		return domain.PR{}, fmt.Errorf("update-with-base failed — the PR or its loop is unavailable")
	}
	return p, nil
}

// ReadyPR marks a GitHub draft PR ready for review (`gh pr ready`).
func (s *PRsService) ReadyPR(prID string) (domain.PR, error) {
	return s.SetPRDraft(prID, false)
}

// SetPRDraft converts a PR to a draft or marks it ready for review.
// GitHub: `gh pr ready` / `gh pr ready --undo`. Local: persist the flag.
func (s *PRsService) SetPRDraft(prID string, draft bool) (domain.PR, error) {
	p, ok := s.eng.SetPRDraft(prID, draft)
	if !ok {
		if p.ID == "" {
			return domain.PR{}, fmt.Errorf("PR %q not found", prID)
		}
		if draft {
			return p, fmt.Errorf("could not convert the PR to a draft")
		}
		return p, fmt.Errorf("could not mark the PR ready for review")
	}
	return p, nil
}

// MergePR merges the PR (PRD §12.1, D53). Local = merge the branch into base
// ON DISK; the user is the sole gate. method is the merge strategy
// ("merge"/"squash"/"rebase"); empty = resolve from project/settings prefs.
func (s *PRsService) MergePR(prID string, method string) (domain.PR, error) {
	if _, ok := s.st().PR(prID); !ok {
		return domain.PR{}, fmt.Errorf("PR %q not found", prID)
	}
	merged, ok := s.eng.MergePR(prID, domain.MergeMethod(method))
	if !ok {
		return merged, fmt.Errorf("merge did not complete — see the loop activity feed for the reason")
	}
	return merged, nil
}

// ---- Policy (single global row, PRD §9, §18) ----

type PolicyService struct{ base }

func (s *PolicyService) GetPolicy() domain.Policy { return s.st().Policy() }

func (s *PolicyService) SetPolicy(p domain.Policy) domain.Policy {
	s.st().SetPolicy(p)
	return p
}

// ---- Inbox (PRD §16.11) ----

type InboxService struct{ base }

func (s *InboxService) GetInbox() []domain.InboxItem { return s.st().Inbox() }

func (s *InboxService) MarkInboxRead(id string) int {
	if ws, code, ok := engine.ParseChatInboxID(id); ok {
		s.eng.MarkWorkspaceChatSeen(ws, code)
		// Unconditional: MarkWorkspaceChatSeen is a no-op when the agent is
		// already gone, but the row must still be dismissable.
		s.st().RemoveInboxItem(id)
		return s.emitInbox()
	}
	s.st().MarkInboxRead(id)
	return s.emitInbox()
}

func (s *InboxService) MarkAllInboxRead() int {
	s.eng.MarkAllChatsSeen()
	s.st().MarkAllInboxRead()
	return s.emitInbox()
}

func (s *InboxService) emitInbox() int {
	n := s.st().UnreadCount()
	s.eng.Bus.Publish(engine.Event{Name: "inbox.updated", Data: map[string]any{"unread": n}})
	return n
}

// ---- Settings / provider / resource (PRD §18) ----

type SettingsService struct {
	base
	lspMgr     *lsp.Manager
	catalogMu  sync.Mutex
	lastCursor []cursor.Model
	lastCodex  []codex.Model

	usageMu     sync.Mutex
	usagePoller *usage.Poller
}

func (s *SettingsService) GetSettings() domain.Settings {
	set := s.st().Settings().MigrateBundleValues()
	p := gitpkg.NormalizeBranchPrefix(set.EffectiveBranchNamePrefix())
	set.BranchNamePrefix = &p
	return set
}

func (s *SettingsService) SetSettings(set domain.Settings) (domain.Settings, error) {
	if err := domain.ValidateMCPServers(set.MCPServers); err != nil {
		return s.st().Settings(), err
	}
	if err := domain.ValidateCompatibleProviders(set.CompatibleProviders); err != nil {
		return s.st().Settings(), err
	}
	set = set.MigrateBundleValues()
	// Persist the effective agent-chat turn ceiling so the UI round-trips a
	// concrete minutes value (0/unset → default; out-of-range clamped).
	set.AgentTurnTimeoutMinutes = domain.EffectiveAgentTurnTimeoutMinutes(set.AgentTurnTimeoutMinutes)
	set.MaxLoops = domain.EffectiveMaxLoops(set.MaxLoops)
	set.MaxAgents = domain.EffectiveMaxAgents(set.MaxAgents)
	set = persistBranchNamePrefix(set)
	set.MCPServers = domain.NormalizeMCPServers(set.MCPServers)
	prev := s.st().Settings()
	set = set.NormalizeProviders()
	enablementChanged := providerEnablementChanged(prev, set)
	if enablementChanged {
		set = s.ensureRunnableCatalogue(set)
		if set.NeedsCatalogFallback() {
			set = applyCatalogFallbacks(set, s.catalogForFallback(set))
		}
	}
	turningCursorOff := prev.ProviderEnabled(domain.ProviderCursor) && !set.ProviderEnabled(domain.ProviderCursor)
	if turningCursorOff {
		// Recur owns the Connect session. Off means Disconnect, not a parked login.
		set.CursorAPIKey = ""
	}
	s.st().SetSettings(set)
	if turningCursorOff {
		s.revokeCursorConnect()
	}
	if enablementChanged && s.eng != nil {
		s.eng.RemapChatsOffDisabledProviders(set)
		if turningCursorOff {
			s.eng.CloseCursorBridges()
		} else if set.ProviderEnabled(domain.ProviderCursor) {
			s.eng.CancelDeferredCursorClose()
		}
	}
	lsp.SetDisabledServers(set.DisabledLanguageServers)
	if s.lspMgr != nil {
		s.lspMgr.CloseServers(set.DisabledLanguageServers)
	}
	// Bridge agents read attribution toggles from this file on every turn; keep
	// it in sync so a settings flip applies without restarting the bridge.
	if dataDir, err := store.DataDir(); err == nil {
		if werr := cursor.WriteAttributionConfig(
			cursor.AttributionConfigPath(dataDir),
			set.CursorAttributeCommits,
			set.CursorAttributePRs,
		); werr != nil {
			// Warm bridges only re-read this file — a failed write leaves them
			// on stale toggles until the next cold start.
			log.Printf("recur: warning: write cursor attribution config: %v", werr)
		}
	}
	// Apply prevent-sleep immediately so toggling off drops caffeinate even
	// while agents are still running, and toggling on covers in-flight turns.
	s.eng.SyncKeepAwake()
	s.eng.ApplyGovernorCaps(set.MaxLoops, set.MaxAgents)
	if set.ScheduledWorkspaceKeep != prev.ScheduledWorkspaceKeep {
		go s.eng.PruneScheduledWorkspaces("")
	}
	return set, nil
}

func providerEnablementChanged(a, b domain.Settings) bool {
	for _, p := range []string{domain.ProviderClaudeCode, domain.ProviderCursor, domain.ProviderCodex} {
		if a.ProviderEnabled(p) != b.ProviderEnabled(p) {
			return true
		}
	}
	if a.CursorConfigured() != b.CursorConfigured() || a.CodexConfigured() != b.CodexConfigured() {
		return true
	}
	if len(a.CompatibleProviders) != len(b.CompatibleProviders) {
		return true
	}
	seen := map[string]bool{}
	for _, p := range b.CompatibleProviders {
		seen[p.ID] = true
		if a.ProviderEnabled(p.ID) != b.ProviderEnabled(p.ID) {
			return true
		}
		prev := a.CompatibleByID(p.ID)
		if prev == nil || prev.Configured() != p.Configured() {
			return true
		}
	}
	for _, p := range a.CompatibleProviders {
		if !seen[p.ID] {
			return true
		}
	}
	return false
}

// ensureRunnableCatalogue re-enables Claude Code when every remaining "on"
// provider is missing its binary/bridge, so pickers cannot go fully empty.
func (s *SettingsService) ensureRunnableCatalogue(set domain.Settings) domain.Settings {
	if s.providerRunnable(set, domain.ProviderClaudeCode) ||
		s.providerRunnable(set, domain.ProviderCursor) ||
		s.providerRunnable(set, domain.ProviderCodex) {
		return set
	}
	for _, p := range set.CompatibleProviders {
		if s.providerRunnable(set, p.ID) {
			return set
		}
	}
	var kept []string
	for _, p := range set.DisabledProviders {
		if p != domain.ProviderClaudeCode {
			kept = append(kept, p)
		}
	}
	set.DisabledProviders = kept
	return set.NormalizeProviders()
}

func (s *SettingsService) providerRunnable(set domain.Settings, provider string) bool {
	if !set.ProviderEnabled(provider) {
		return false
	}
	switch provider {
	case domain.ProviderClaudeCode:
		return true
	case domain.ProviderCursor:
		if !set.CursorConfigured() {
			return false
		}
		dataDir, _ := store.DataDir()
		_, err := cursor.Locate(dataDir)
		return err == nil
	case domain.ProviderCodex:
		if !set.CodexConfigured() {
			return false
		}
		_, err := codex.Locate(set.CodexBinPath)
		return err == nil
	default:
		p := set.CompatibleByID(provider)
		return p != nil && p.Configured()
	}
}

// GetResource returns the machine-global status-bar counters (PRD §6.4, D6).
func (s *SettingsService) GetResource() domain.Resource {
	r := s.st().Resource()
	r.GitWarning = s.eng.GitWarning()
	return r
}

// GetInstanceLabel returns the trimmed RECUR_INSTANCE_LABEL process env.
// Empty means the titlebar chip is hidden.
func (s *SettingsService) GetInstanceLabel() string {
	return InstanceLabel()
}

func persistBranchNamePrefix(set domain.Settings) domain.Settings {
	if set.BranchNamePrefix == nil {
		p := set.EffectiveBranchNamePrefix()
		set.BranchNamePrefix = &p
	}
	n := gitpkg.NormalizeBranchPrefix(*set.BranchNamePrefix)
	set.BranchNamePrefix = &n
	// Empty string is "none". Keep the legacy mode as a belt-and-suspenders
	// so a dropped empty pointer cannot silently revert to recur.
	if n == "" {
		set.BranchPrefixMode = domain.BranchPrefixNone
	} else {
		set.BranchPrefixMode = ""
	}
	set.BranchPrefixCustom = ""
	return set
}

// GitHubLogin returns the authenticated GitHub username from `gh`, or "" when
// gh is missing or logged out. Used by Settings → General to fill the prefix
// field once; it is not consulted when creating workspaces.
func (s *SettingsService) GitHubLogin() string {
	return engine.GitHubLogin()
}

// LogsPath returns the Recur process log path (<dataDir>/logs/recur.log).
func (s *SettingsService) LogsPath() string {
	root, err := store.DataDir()
	if err != nil {
		return ""
	}
	return store.AppLogPath(root)
}

// RevealLogs shows recur.log in the OS file manager (parent folder if the
// file has not been created yet).
func (s *SettingsService) RevealLogs() error {
	root, err := store.DataDir()
	if err != nil {
		return err
	}
	if _, err := store.LogsDir(root); err != nil {
		return err
	}
	return code.RevealInFinder(store.AppLogPath(root))
}

// ClearAllData wipes all projects/loops/PRs/inbox/history (keeping your policy +
// settings), returning the app to a clean slate.
func (s *SettingsService) ClearAllData() error { return s.eng.ClearAll() }

// ListMonospaceFonts returns installed monospace font family names for the
// editor font picker. Native enumeration is required because the webview's
// canvas font probe often cannot see user-installed faces (e.g. ~/Library/Fonts).
func (s *SettingsService) ListMonospaceFonts() []string {
	return osfonts.MonospaceFamilies()
}
