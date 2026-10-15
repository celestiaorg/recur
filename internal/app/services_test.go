package app

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

// newTestServices wires a fresh engine + services and a capturing event sink.
func newTestServices(t *testing.T) (*Services, *captureSink) {
	t.Helper()
	eng := engine.New(store.NewInMemory())
	cap := &captureSink{}
	eng.Bus.Subscribe(cap.sink)
	return NewServices(eng), cap
}

type captureSink struct {
	mu     sync.Mutex
	events []engine.Event
}

func (c *captureSink) sink(ev engine.Event) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.events = append(c.events, ev)
}

func mustSetSettings(t *testing.T, svc *SettingsService, set domain.Settings) domain.Settings {
	t.Helper()
	got, err := svc.SetSettings(set)
	if err != nil {
		t.Fatalf("SetSettings: %v", err)
	}
	return got
}

func (c *captureSink) names() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]string, len(c.events))
	for i, e := range c.events {
		out[i] = e.Name
	}
	return out
}

func contains(ss []string, want string) bool {
	for _, s := range ss {
		if s == want {
			return true
		}
	}
	return false
}

func TestSeedDataShape(t *testing.T) {
	svc, _ := newTestServices(t)

	if got := len(svc.Workspaces.ListWorkspaces()); got != 5 {
		t.Errorf("loops: got %d, want 5", got)
	}
	if got := len(svc.Projects.ListProjects()); got != 3 {
		t.Errorf("projects: got %d, want 3", got)
	}
	if got := len(svc.PRs.ListPRs()); got != 3 {
		t.Errorf("prs: got %d, want 3", got)
	}
	// The escalated loop has an escalation; convergence series is recorded.
	esc, err := svc.Escalation.GetEscalation("LOOP-2218")
	if err != nil || esc.Status != "open" || len(esc.Options) != 3 {
		t.Errorf("escalation LOOP-2218: status=%q options=%d err=%v, want open/3", esc.Status, len(esc.Options), err)
	}
	l, err := svc.Workspaces.GetWorkspace("LOOP-2231")
	if err != nil || len(l.Convergence) != 3 || l.Convergence[2].Findings != 0 {
		t.Errorf("LOOP-2231 convergence not [..,0]: %+v (err=%v)", l.Convergence, err)
	}
	pr, err := svc.PRs.GetPR("482")
	if err != nil || pr.Number != 482 || len(svc.PRs.GetPRFiles("482")) != 3 {
		t.Errorf("PR #482 files: got %d, want 3 (err=%v)", len(svc.PRs.GetPRFiles("482")), err)
	}
}

func TestBusFanOut(t *testing.T) {
	eng := engine.New(store.NewInMemory())
	var got []string
	eng.Bus.Subscribe(func(ev engine.Event) { got = append(got, ev.Name) })
	eng.Bus.Subscribe(func(ev engine.Event) { got = append(got, ev.Name+"#2") })
	eng.Bus.Publish(engine.Event{Name: "workspace.updated"})
	if len(got) != 2 || got[0] != "workspace.updated" || got[1] != "workspace.updated#2" {
		t.Errorf("fan-out to both sinks failed: %v", got)
	}
}

func TestAnswerEscalationResumesLoop(t *testing.T) {
	svc, cap := newTestServices(t)
	before := svc.Settings.GetResource().NeedsYou

	e, err := svc.Escalation.AnswerEscalation("LOOP-2218", "Token bucket with refill", "", "")
	if err != nil || e.Status != "answered" || e.Answer != "Token bucket with refill" {
		t.Fatalf("escalation not answered: %+v (err=%v)", e, err)
	}
	// Loop leaves "needs you" (engine decides, not the agent — PRD §13). The seeded
	// demo loop has no real worktree, so NO goroutine starts: the honest resting
	// state is paused (waiting to resume), NOT a phantom "coding" with no goroutine.
	l, err := svc.Workspaces.GetWorkspace("LOOP-2218")
	if err != nil {
		t.Fatalf("GetWorkspace: %v", err)
	}
	if l.Group != domain.GroupActive {
		t.Errorf("loop did not leave needs-you: group=%q", l.Group)
	}
	if l.State != domain.StatePaused {
		t.Errorf("seeded demo loop with no worktree should rest at paused, got state=%q", l.State)
	}
	if after := svc.Settings.GetResource().NeedsYou; after != before-1 {
		t.Errorf("needsYou: got %d, want %d", after, before-1)
	}
	if !contains(cap.names(), "escalation.resolved") {
		t.Errorf("expected escalation.resolved event, got %v", cap.names())
	}
	// No phantom running loop was created (no goroutine, no slot held).
	if running := svc.Workspaces.eng.Governor().LoopsRunning(); running != 0 {
		t.Errorf("seeded answer must not start a goroutine: LoopsRunning=%d, want 0", running)
	}

	// A SECOND answer is an idempotent no-op (the escalation is already answered):
	// the store/engine must not re-deliver or re-dispatch, so it returns an error.
	e2, err2 := svc.Escalation.AnswerEscalation("LOOP-2218", "Token bucket", "100 req/min per key", "")
	if err2 == nil || e2.Status != "" {
		t.Errorf("second answer must be a no-op (already answered), got %+v (err=%v)", e2, err2)
	}
	if running := svc.Workspaces.eng.Governor().LoopsRunning(); running != 0 {
		t.Errorf("second answer must not start a goroutine: LoopsRunning=%d, want 0", running)
	}
}

// TestSendBackToLoopIsConductorTurn: the bound Send Back control starts a
// conductor turn with the notes on the thread. It does not leak a loop slot.
func TestSendBackToLoopIsConductorTurn(t *testing.T) {
	svc, cap := newTestServices(t)
	svc.Workspaces.eng.SetRunner(&engine.ScriptedRunner{})

	if _, err := svc.Projects.UpdateProjectRunConfig(domain.Project{
		ID: "storefront", Name: "storefront", RepoPath: t.TempDir(), BaseBranch: "main",
	}); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}

	beforePR, err := svc.PRs.GetPR("482")
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	before := len(beforePR.Comments)
	p, err := svc.PRs.SendBackToLoop("482", []string{"fix the secret"})
	if err != nil {
		t.Fatalf("SendBackToLoop: %v", err)
	}
	if p.State != "In review" || p.Group != "you" {
		t.Errorf("PR state=%q group=%q, want In review/you", p.State, p.Group)
	}
	got, err := svc.PRs.GetPR("482")
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if got.State != "In review" {
		t.Errorf("persisted PR state = %q, want In review", got.State)
	}
	if len(got.Comments) <= before {
		t.Errorf("SendBackToLoop() comments = %d, want the send-back note appended (before %d)", len(got.Comments), before)
	}
	foundNote := false
	for _, c := range got.Comments {
		if strings.Contains(c.BodyMD, "fix the secret") || strings.Contains(c.BodyHTML, "fix the secret") {
			foundNote = true
		}
	}
	if !foundNote {
		t.Errorf("SendBackToLoop() thread missing send-back note, comments=%d", len(got.Comments))
	}
	if !contains(cap.names(), "pr.updated") {
		t.Errorf("expected pr.updated, got %v", cap.names())
	}
	if running := svc.Workspaces.eng.Governor().LoopsRunning(); running != 0 {
		t.Errorf("SendBackToLoop() LoopsRunning=%d, want 0", running)
	}
}

// TestSubmitReviewAndComment exercises pending line notes + SubmitReview flush.
func TestSubmitReviewAndComment(t *testing.T) {
	svc, _ := newTestServices(t)
	// Local persist path — GitHub PRs post via gh (covered in engine tests).
	local := domain.PR{
		ID: "10", Number: 10, Source: domain.SourceLocal,
		Title: "local", State: "In review", Group: "you",
	}
	if _, err := svc.PRs.st().AddPR(local); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	beforePR, err := svc.PRs.GetPR("10")
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	before := len(beforePR.Comments)
	if _, err := svc.PRs.AddPRComment("10", "src/auth/oauth.ts", 12, "please confirm the secrets client init order", 0, ""); err != nil {
		t.Fatalf("AddPRComment: %v", err)
	}
	afterPR, err := svc.PRs.GetPR("10")
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if got := len(afterPR.Comments); got != before {
		t.Errorf("AddPRComment posted immediately: before=%d after=%d, want pending", before, got)
	}
	if len(afterPR.PendingComments) != 1 || afterPR.PendingComments[0].Line != 12 {
		t.Errorf("AddPRComment pending = %+v, want one line note", afterPR.PendingComments)
	}

	if _, err := svc.PRs.SubmitReview("10", "approve", "looks good"); err != nil {
		t.Fatalf("SubmitReview: %v", err)
	}
	got, err := svc.PRs.GetPR("10")
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if len(got.PendingComments) != 0 {
		t.Errorf("SubmitReview left pending comments: %+v", got.PendingComments)
	}
	if len(got.Comments) != before+2 {
		t.Errorf("SubmitReview did not persist pending + review: %d comments", len(got.Comments))
	}
	var reviewID, childParent string
	for _, c := range got.Comments[before:] {
		if c.ParentID == "" && strings.Contains(c.BodyHTML, "looks good") {
			reviewID = c.ID
		}
		if c.ParentID != "" {
			childParent = c.ParentID
		}
	}
	if reviewID == "" || childParent != reviewID {
		t.Errorf("SubmitReview parent/child = %q %q, want line note nested under review", reviewID, childParent)
	}
	var lineID string
	for _, c := range got.Comments {
		if c.ParentID == reviewID && c.IsLineNote() {
			lineID = c.ID
			break
		}
	}
	if lineID == "" {
		t.Fatal("SubmitReview missing line note")
	}
	reply, err := svc.PRs.ReplyPRComment("10", lineID, "thread reply")
	if err != nil {
		t.Fatalf("ReplyPRComment: %v", err)
	}
	if reply.InReplyToID != lineID {
		t.Errorf("ReplyPRComment InReplyToID = %q, want %s", reply.InReplyToID, lineID)
	}
	if _, err := svc.PRs.SetPRThreadResolved("10", lineID, true); err != nil {
		t.Fatalf("SetPRThreadResolved: %v", err)
	}
	marked, err := svc.PRs.GetPR("10")
	if err != nil {
		t.Fatalf("GetPR after resolve: %v", err)
	}
	for _, c := range marked.Comments {
		if c.ID == lineID && !c.Resolved {
			t.Error("SetPRThreadResolved left the line note unresolved")
		}
	}
	if _, err := svc.PRs.DeletePRComment("10", lineID); err != nil {
		t.Fatalf("DeletePRComment: %v", err)
	}
	afterDel, err := svc.PRs.GetPR("10")
	if err != nil {
		t.Fatalf("GetPR after delete: %v", err)
	}
	for _, c := range afterDel.Comments {
		if c.ID == lineID {
			t.Error("DeletePRComment left the line note in the thread")
		}
	}

	if _, err := svc.PRs.AddPRComment("10", "", 0, "plain conversation note", 0, ""); err != nil {
		t.Fatalf("AddPRComment conversation: %v", err)
	}
	if _, err := svc.PRs.UpdatePRDetails("10", "local retitled", "new description"); err != nil {
		t.Fatalf("UpdatePRDetails: %v", err)
	}
	named, err := svc.PRs.GetPR("10")
	if err != nil {
		t.Fatalf("GetPR after details: %v", err)
	}
	if named.Title != "local retitled" {
		t.Errorf("UpdatePRDetails title = %q, want local retitled", named.Title)
	}
	you := ""
	for _, c := range named.Comments {
		if c.Author == "You" && c.Kind != "description" {
			you = c.ID
			break
		}
	}
	if you == "" {
		t.Fatal("AddPRComment conversation did not persist a You comment")
	}
	if _, err := svc.PRs.UpdatePRComment("10", you, "edited conversation note"); err != nil {
		t.Fatalf("UpdatePRComment: %v", err)
	}
	reacted, err := svc.PRs.TogglePRCommentReaction("10", you, "+1")
	if err != nil {
		t.Fatalf("TogglePRCommentReaction: %v", err)
	}
	found := false
	for _, c := range reacted.Comments {
		if c.ID == you && domain.ReactionMine(c.Reactions, "+1") {
			found = true
		}
	}
	if !found {
		t.Error("TogglePRCommentReaction did not record the viewer's +1")
	}
}

func TestClosedPRRejectsComments(t *testing.T) {
	svc, _ := newTestServices(t)
	if _, err := svc.PRs.st().AddPR(domain.PR{
		ID: "471", Number: 471, Source: domain.SourceGitHub,
		Title: "merged", State: "Merged", Group: "merged",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, err := svc.PRs.AddPRComment("471", "", 0, "too late", 0, ""); err == nil {
		t.Error("AddPRComment on merged PR: expected error")
	} else if !strings.Contains(err.Error(), "closed") {
		t.Errorf("AddPRComment error = %v, want closed", err)
	}
	if _, err := svc.PRs.SubmitReview("471", "approve", "lgtm"); err == nil {
		t.Error("SubmitReview on merged PR: expected error")
	} else if !strings.Contains(err.Error(), "closed") {
		t.Errorf("SubmitReview error = %v, want closed", err)
	}
	if _, err := svc.PRs.ReplyPRComment("471", "c1", "too late"); err == nil {
		t.Error("ReplyPRComment on merged PR: expected error")
	} else if !strings.Contains(err.Error(), "closed") {
		t.Errorf("ReplyPRComment error = %v, want closed", err)
	}
	if _, err := svc.PRs.UpdatePRDetails("471", "nope", "nope"); err == nil {
		t.Error("UpdatePRDetails on merged PR: expected error")
	}
}

func TestInboxMarkRead(t *testing.T) {
	svc, cap := newTestServices(t)
	if n := unread(svc); n != 4 {
		t.Fatalf("seed unread: got %d, want 4", n)
	}
	left := svc.Inbox.MarkInboxRead("ib1")
	if left != 3 {
		t.Errorf("after mark one read: got %d, want 3", left)
	}
	if all := svc.Inbox.MarkAllInboxRead(); all != 0 {
		t.Errorf("after mark all read: got %d, want 0", all)
	}
	if !contains(cap.names(), "inbox.updated") {
		t.Errorf("expected inbox.updated, got %v", cap.names())
	}
}

func TestPolicyToggleRoundTrip(t *testing.T) {
	svc, _ := newTestServices(t)
	pol := svc.Policy.GetPolicy()
	if pol.Autonomy != domain.AutonomyBalanced {
		t.Fatalf("seed policy: %+v", pol)
	}

	// Phase 3.1: the autonomy dial is a real, persisted setting.
	pol2 := svc.Policy.GetPolicy()
	pol2.Autonomy = domain.AutonomyAggressive
	got2 := svc.Policy.SetPolicy(pol2)
	if got2.Autonomy != domain.AutonomyAggressive {
		t.Errorf("autonomy change did not round-trip: got %q", got2.Autonomy)
	}
	if svc.Policy.GetPolicy().Autonomy != domain.AutonomyAggressive {
		t.Errorf("autonomy change did not persist in store")
	}
}

func TestAddProjectAssignsSlugAndDefaults(t *testing.T) {
	svc, _ := newTestServices(t)

	before := len(svc.Projects.ListProjects())
	p, err := svc.Projects.AddProject(domain.Project{
		Name: "My New App", RepoPath: "~/code/my-new-app", BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if p.ID != "my-new-app" {
		t.Errorf("slug id: got %q, want my-new-app", p.ID)
	}
	if p.Pic != "M" {
		t.Errorf("avatar: got %q, want M", p.Pic)
	}
	if p.Color == "" {
		t.Errorf("expected an assigned color")
	}
	if !p.Open {
		t.Errorf("new project should default to sidebar-expanded (open=true)")
	}
	if got := len(svc.Projects.ListProjects()); got != before+1 {
		t.Errorf("project not added: got %d, want %d", got, before+1)
	}
	// A second add with the same name must get a de-duplicated id.
	p2, err := svc.Projects.AddProject(domain.Project{Name: "My New App"})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if p2.ID != "my-new-app-2" {
		t.Errorf("dedup id: got %q, want my-new-app-2", p2.ID)
	}
}

func TestPreviewCopyFiles(t *testing.T) {
	svc, _ := newTestServices(t)
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, "apps"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, ".env"), []byte("A=1\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "apps", ".env.local"), []byte("B=2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	p, err := svc.Projects.AddProject(domain.Project{Name: "copy-preview", RepoPath: repo, BaseBranch: "main"})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	got := svc.Projects.PreviewCopyFiles(p.ID, ".env*")
	if got.Error != "" {
		t.Fatalf("PreviewCopyFiles error: %s", got.Error)
	}
	if got.Count != 2 {
		t.Fatalf("PreviewCopyFiles count = %d files=%v, want 2", got.Count, got.Files)
	}
	missing := svc.Projects.PreviewCopyFiles("nope", ".env*")
	if missing.Error == "" {
		t.Fatal("PreviewCopyFiles missing project should set Error")
	}
}

func TestProjectGroupAssignAndDelete(t *testing.T) {
	svc, _ := newTestServices(t)
	g, err := svc.Projects.AddProjectGroup(domain.ProjectGroup{Name: "Side projects"})
	if err != nil {
		t.Fatalf("AddProjectGroup: %v", err)
	}
	if _, err := uuid.Parse(g.ID); err != nil || !g.Open {
		t.Fatalf("expected UUID id, got %+v (%v)", g, err)
	}
	if g.Name != "Side projects" {
		t.Fatalf("name: got %q", g.Name)
	}
	projs := svc.Projects.ListProjects()
	if len(projs) == 0 {
		t.Fatal("need a project")
	}
	saved, err := svc.Projects.SetProjectGroup(projs[0].ID, g.ID)
	if err != nil {
		t.Fatalf("SetProjectGroup: %v", err)
	}
	if saved.GroupID != g.ID {
		t.Fatalf("group id: got %q want %q", saved.GroupID, g.ID)
	}
	if _, err := svc.Projects.SetProjectGroup(projs[0].ID, "missing"); err == nil {
		t.Fatal("expected error for unknown group")
	}
	if err := svc.Projects.DeleteProjectGroup(g.ID); err != nil {
		t.Fatalf("DeleteProjectGroup: %v", err)
	}
	got, err := svc.Projects.GetProject(projs[0].ID)
	if err != nil {
		t.Fatalf("GetProject: %v", err)
	}
	if got.GroupID != "" {
		t.Fatalf("expected ungrouped after delete, got %q", got.GroupID)
	}
}

func unread(svc *Services) int {
	n := 0
	for _, it := range svc.Inbox.GetInbox() {
		if it.Unread {
			n++
		}
	}
	return n
}

// TestStartWorkspaceServiceEndToEnd drives the bound WorkspacesService.StartWorkspace with a
// ScriptedRunner (no real claude) against a temp git repo, asserting a loop id is
// returned, the loop runs to a ready state, the activity feed has live items, and
// Stop is wired through. Mirrors the M2 demo via the actual binding surface.
func TestStartWorkspaceServiceEndToEnd(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)

	st, err := store.New()
	if err != nil {
		t.Fatalf("store.New: %v", err)
	}
	t.Cleanup(func() { st.Close() })

	// RAIL #1 (verify_browser) seeds ON and gates pr_open behind a browser verify
	// (PRD §10). This test asserts the StartWorkspace service plumbing converges to
	// pr_open WITHOUT a run config, so disable rail #1 (it is exercised in the engine
	// verify tests). Without this the converged loop would block on the absent verify.
	disableVerifyRail(t, st)

	eng := engine.New(st)
	eng.SetRunner(&engine.ScriptedRunner{
		Edits: map[string]string{"svc.txt": "from coder\n"},
		Lines: [][]byte{
			[]byte(`{"type":"system","subtype":"init","session_id":"s1","model":"sonnet","mcp_servers":[]}`),
			[]byte(`{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"svc.txt"}}]}}`),
			[]byte(`{"type":"result","subtype":"success","session_id":"s1","duration_ms":10,"is_error":false,"num_turns":1,"result":"ok","total_cost_usd":0}`),
		},
	})
	eng.SetWorktreeRoot(filepath.Join(dataDir, "worktrees"))
	t.Cleanup(eng.Shutdown)

	repo := initGitRepo(t)
	saved := svcAddProject(t, eng, repo)

	svc := NewServices(eng)
	id, err := svc.Workspaces.StartWorkspace(saved, "Add a service file", "", "")
	if err != nil || id == "" {
		t.Fatalf("StartWorkspace returned empty id (err=%v)", err)
	}

	// Loop should reach a ready (pr_open) state.
	deadline := time.Now().Add(5 * time.Second)
	var l domain.Workspace
	for time.Now().Before(deadline) {
		l, _ = svc.Workspaces.GetWorkspace(id)
		if l.State == domain.StatePROpen {
			break
		}
		time.Sleep(5 * time.Millisecond)
	}
	if l.State != domain.StatePROpen {
		t.Fatalf("loop did not reach pr_open: state=%q", l.State)
	}

	// The activity feed has live items (real events, not the seeded feed).
	if got := svc.Workspaces.GetWorkspaceFeed(id); len(got) == 0 {
		t.Error("expected a non-empty live activity feed")
	}

	// The new loop appears in the list (sidebar / all-loops).
	found := false
	for _, x := range svc.Workspaces.ListWorkspaces() {
		if x.ID == id {
			found = true
		}
	}
	if !found {
		t.Error("new loop not present in ListWorkspaces")
	}

	// Stop is wired through (no panic on a finished loop).
	svc.Workspaces.StopLoop(id)
}

// initGitRepo creates a one-commit git repo on main and returns its path.
func initGitRepo(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "user.name", "test")
	run("config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "initial")
	return dir
}

func svcAddProject(t *testing.T, eng *engine.Engine, repo string) string {
	t.Helper()
	p, err := eng.Store.AddProject(domain.Project{ID: "p", Name: "p", RepoPath: repo, BaseBranch: "main"})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	return p.ID
}

// waitWorkspaceReady polls until BeginCreateWorkspace has allocated a worktree
// (the Wails CreateWorkspace path returns before git finishes).
func waitWorkspaceReady(t *testing.T, eng *engine.Engine, id string) domain.Workspace {
	t.Helper()
	deadline := time.Now().Add(15 * time.Second)
	for time.Now().Before(deadline) {
		ws, ok := eng.Store.Workspace(id)
		if ok && ws.LastError != "" && ws.WorktreePath == "" {
			t.Fatalf("workspace create failed: %s", ws.LastError)
		}
		if ok && ws.WorktreePath != "" && ws.Phase != "creating" {
			return ws
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("timed out waiting for workspace worktree")
	return domain.Workspace{}
}

// disableVerifyRail flips rail #1 (verify_browser) OFF in the seeded policy so a
// service-plumbing test can assert convergence→pr_open without a run config (the
// browser-verify gate is exercised in the engine verify tests).
func disableVerifyRail(t *testing.T, st *store.Store) {
	t.Helper()
	p := st.Policy()
	for i := range p.Rails {
		if p.Rails[i].Key == "verify_browser" {
			p.Rails[i].On = false
		}
	}
	st.SetPolicy(p)
}

// TestGettersErrorOnMissing asserts every getter that now returns (T, error)
// reports a non-nil error for a bogus id, instead of silently returning a zero
// struct (the M0-era swallow this phase removes).
func TestGettersErrorOnMissing(t *testing.T) {
	svc, _ := newTestServices(t)
	const bogus = "does-not-exist"

	if _, err := svc.Projects.GetProject(bogus); err == nil {
		t.Error("GetProject: expected error for missing project")
	}
	if _, err := svc.Workspaces.GetWorkspace(bogus); err == nil {
		t.Error("GetWorkspace: expected error for missing loop")
	}
	if _, err := svc.Escalation.GetEscalation(bogus); err == nil {
		t.Error("GetEscalation: expected error for missing loop")
	}
	if _, err := svc.Agents.GetAgentTrace(bogus, "CO"); err == nil {
		t.Error("GetAgentTrace: expected error for missing agent")
	}
	if _, err := svc.PRs.GetPR(bogus); err == nil {
		t.Error("GetPR: expected error for missing PR")
	}
	if _, err := svc.PRs.GetCommitDiff("482", bogus); err == nil {
		t.Error("GetCommitDiff: expected error for missing commit")
	}
	if _, err := svc.PRs.AddPRComment(bogus, "", 0, "hi", 0, ""); err == nil {
		t.Error("AddPRComment: expected error for missing PR")
	}
	if _, err := svc.PRs.SubmitReview(bogus, "comment", "hi"); err == nil {
		t.Error("SubmitReview: expected error for missing PR")
	}
	if _, err := svc.PRs.UpdatePRDetails(bogus, "t", "b"); err == nil {
		t.Error("UpdatePRDetails: expected error for missing PR")
	}
	if _, err := svc.PRs.UpdatePRComment(bogus, "c1", "hi"); err == nil {
		t.Error("UpdatePRComment: expected error for missing PR")
	}
	if _, err := svc.PRs.SendBackToLoop(bogus, []string{"x"}); err == nil {
		t.Error("SendBackToLoop: expected error for missing PR")
	}
	if _, err := svc.PRs.MergePR(bogus, ""); err == nil {
		t.Error("MergePR: expected error for missing PR")
	}
}

// TestGetCommitDiffFillsLazyFiles is the Commits-tab drill-in: LogCommits leaves
// Files empty, GetCommitDiff runs git show and persists the per-commit diff.
func TestGetCommitDiffFillsLazyFiles(t *testing.T) {
	svc, _ := newTestServices(t)
	repo := initGitRepo(t)
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	if err := os.WriteFile(filepath.Join(repo, "feat.txt"), []byte("hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "feat.txt")
	run("commit", "-m", "add feat")
	cmd := exec.Command("git", "rev-parse", "HEAD")
	cmd.Dir = repo
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("rev-parse HEAD: %v", err)
	}
	full := strings.TrimSpace(string(out))
	if len(full) < 7 {
		t.Fatalf("HEAD hash too short: %q", full)
	}
	short := full[:7]

	if _, err := svc.Projects.UpdateProjectRunConfig(domain.Project{
		ID: "storefront", Name: "storefront", RepoPath: repo, BaseBranch: "main",
	}); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	if _, ok := svc.Workspaces.eng.Store.UpdateWorkspace("LOOP-2231", func(w *domain.Workspace) {
		w.WorktreePath = repo
	}); !ok {
		t.Fatal("LOOP-2231 missing")
	}
	if _, ok := svc.PRs.eng.Store.UpdatePR("482", func(p *domain.PR) {
		p.Base = "main"
		p.Commits = []domain.Commit{{Hash: short, SHA: full, Message: "add feat"}}
	}); !ok {
		t.Fatal("PR 482 missing")
	}

	got, err := svc.PRs.GetCommitDiff("482", short)
	if err != nil {
		t.Fatalf("GetCommitDiff: %v", err)
	}
	if len(got.Files) == 0 {
		t.Fatal("GetCommitDiff returned no files")
	}
	found := false
	for _, f := range got.Files {
		if f.Path == "feat.txt" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("GetCommitDiff files = %+v, want feat.txt", got.Files)
	}

	// Full SHA also matches, and the diff is persisted so a second call is a store hit.
	again, err := svc.PRs.GetCommitDiff("482", full)
	if err != nil {
		t.Fatalf("GetCommitDiff by SHA: %v", err)
	}
	if len(again.Files) == 0 {
		t.Fatal("persisted commit diff was empty")
	}
	pr, ok := svc.PRs.eng.Store.PR("482")
	if !ok || len(pr.Commits) == 0 || len(pr.Commits[0].Files) == 0 {
		t.Fatal("GetCommitDiff did not persist files onto the PR")
	}
}

func TestGetCommitDiffFallsBackToRepo(t *testing.T) {
	svc, _ := newTestServices(t)
	repo := initGitRepo(t)
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	if err := os.WriteFile(filepath.Join(repo, "feat.txt"), []byte("hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "feat.txt")
	run("commit", "-m", "add feat")
	cmd := exec.Command("git", "rev-parse", "HEAD")
	cmd.Dir = repo
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("rev-parse HEAD: %v", err)
	}
	full := strings.TrimSpace(string(out))
	short := full[:7]

	if _, err := svc.Projects.UpdateProjectRunConfig(domain.Project{
		ID: "storefront", Name: "storefront", RepoPath: repo, BaseBranch: "main",
	}); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	if _, ok := svc.Workspaces.eng.Store.UpdateWorkspace("LOOP-2231", func(w *domain.Workspace) {
		w.WorktreePath = ""
	}); !ok {
		t.Fatal("LOOP-2231 missing")
	}
	if _, ok := svc.PRs.eng.Store.UpdatePR("482", func(p *domain.PR) {
		p.Base = "main"
		p.Commits = []domain.Commit{{Hash: short, SHA: full, Message: "add feat"}}
	}); !ok {
		t.Fatal("PR 482 missing")
	}

	got, err := svc.PRs.GetCommitDiff("482", short)
	if err != nil {
		t.Fatalf("GetCommitDiff: %v", err)
	}
	found := false
	for _, f := range got.Files {
		if f.Path == "feat.txt" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("GetCommitDiff() files = %+v, want feat.txt via repo fallback", got.Files)
	}

	stale := t.TempDir()
	if _, ok := svc.Workspaces.eng.Store.UpdateWorkspace("LOOP-2231", func(w *domain.Workspace) {
		w.WorktreePath = stale
	}); !ok {
		t.Fatal("LOOP-2231 missing")
	}
	if _, ok := svc.PRs.eng.Store.UpdatePR("482", func(p *domain.PR) {
		p.Commits = []domain.Commit{{Hash: short, SHA: full, Message: "add feat"}}
	}); !ok {
		t.Fatal("PR 482 missing")
	}
	again, err := svc.PRs.GetCommitDiff("482", short)
	if err != nil {
		t.Fatalf("GetCommitDiff stale worktree: %v", err)
	}
	found = false
	for _, f := range again.Files {
		if f.Path == "feat.txt" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("GetCommitDiff() files = %+v, want feat.txt after stale worktree retry", again.Files)
	}
}

func TestGetCommitDiffCachesEmpty(t *testing.T) {
	svc, _ := newTestServices(t)
	repo := initGitRepo(t)
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("commit", "--allow-empty", "-m", "empty")
	cmd := exec.Command("git", "rev-parse", "HEAD")
	cmd.Dir = repo
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("rev-parse HEAD: %v", err)
	}
	full := strings.TrimSpace(string(out))
	short := full[:7]

	if _, err := svc.Projects.UpdateProjectRunConfig(domain.Project{
		ID: "storefront", Name: "storefront", RepoPath: repo, BaseBranch: "main",
	}); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	if _, ok := svc.Workspaces.eng.Store.UpdateWorkspace("LOOP-2231", func(w *domain.Workspace) {
		w.WorktreePath = repo
	}); !ok {
		t.Fatal("LOOP-2231 missing")
	}
	if _, ok := svc.PRs.eng.Store.UpdatePR("482", func(p *domain.PR) {
		p.Base = "main"
		p.Commits = []domain.Commit{{Hash: short, SHA: full, Message: "empty"}}
	}); !ok {
		t.Fatal("PR 482 missing")
	}

	got, err := svc.PRs.GetCommitDiff("482", short)
	if err != nil {
		t.Fatalf("GetCommitDiff: %v", err)
	}
	if got.Files == nil {
		t.Fatal("GetCommitDiff() Files = nil, want empty slice so a later open is a cache hit")
	}

	if _, ok := svc.Workspaces.eng.Store.UpdateWorkspace("LOOP-2231", func(w *domain.Workspace) {
		w.WorktreePath = ""
	}); !ok {
		t.Fatal("LOOP-2231 missing")
	}
	if _, err := svc.Projects.UpdateProjectRunConfig(domain.Project{
		ID: "storefront", Name: "storefront", RepoPath: t.TempDir(), BaseBranch: "main",
	}); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}

	again, err := svc.PRs.GetCommitDiff("482", short)
	if err != nil {
		t.Fatalf("GetCommitDiff cached empty: %v", err)
	}
	if again.Files == nil {
		t.Fatal("cached empty commit diff was lost")
	}
}

// TestStartWorkspaceUnknownProjectErrors asserts StartWorkspace propagates the engine's
// error for an unknown project id rather than returning a blank id.
func TestStartWorkspaceUnknownProjectErrors(t *testing.T) {
	svc, _ := newTestServices(t)
	id, err := svc.Workspaces.StartWorkspace("no-such-project", "do something", "", "")
	if err == nil || id != "" {
		t.Errorf("StartWorkspace: expected error + empty id, got id=%q err=%v", id, err)
	}
}

// TestRemoveAndDeleteWorkspaceService exercises RemoveWorkspace then hard
// DeleteWorkspace against the seeded settled loop LOOP-2224 (pr_open), and
// asserts the not-running precondition refuses both on a live loop.
func TestRemoveAndDeleteWorkspaceService(t *testing.T) {
	svc, _ := newTestServices(t)

	removed, err := svc.Workspaces.RemoveWorkspace("LOOP-2224")
	if err != nil {
		t.Fatalf("RemoveWorkspace: %v", err)
	}
	if !removed.Archived {
		t.Error("expected Archived=true")
	}

	// Remove's background teardown holds the id until it finishes.
	deadline := time.Now().Add(5 * time.Second)
	for {
		err = svc.Workspaces.DeleteWorkspace("LOOP-2224")
		if err == nil || !strings.Contains(err.Error(), "being removed") || time.Now().After(deadline) {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	if _, err := svc.Workspaces.GetWorkspace("LOOP-2224"); err == nil {
		t.Error("expected the loop to be gone after delete")
	}

	// A settled seeded loop with no live goroutine, but the wrong state (verifying)
	// is refused.
	if _, err := svc.Workspaces.RemoveWorkspace("LOOP-2231"); err == nil {
		t.Error("expected RemoveWorkspace to refuse a loop that hasn't settled")
	}
	if err := svc.Workspaces.DeleteWorkspace("LOOP-2231"); err == nil {
		t.Error("expected DeleteWorkspace to refuse a loop that hasn't settled")
	}
}

func TestSettingsLogsPath(t *testing.T) {
	root := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", root)
	svc, _ := newTestServices(t)
	got := svc.Settings.LogsPath()
	want := store.AppLogPath(root)
	if got != want {
		t.Errorf("LogsPath() = %q, want %q", got, want)
	}
}
