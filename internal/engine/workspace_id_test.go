package engine

import (
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

var kitchenIDRE = regexp.MustCompile(`^[a-z]+-[a-z]+(-\d+)?$`)
var composedIDRE = regexp.MustCompile(`^[a-z0-9-]+\.[a-z]+-[a-z]+(-\d+)?$`)

func TestAllocKitchenIDFormat(t *testing.T) {
	taken := map[string]bool{}
	for i := 0; i < 200; i++ {
		id := allocKitchenID(func(s string) bool { return taken[s] })
		if !kitchenIDRE.MatchString(id) {
			t.Fatalf("id %q does not match adj-dish(-n)", id)
		}
		parts := strings.Split(id, "-")
		if len(parts) < 2 {
			t.Fatalf("id %q: want at least adj-dish", id)
		}
		adj, dish := parts[0], parts[1]
		if !kitchenWordIn(kitchenAdjectives, adj) {
			t.Errorf("unknown adjective %q in %q", adj, id)
		}
		if !kitchenWordIn(kitchenDishes, dish) {
			t.Errorf("unknown dish %q in %q", dish, id)
		}
		if len(parts) == 3 {
			n, err := strconv.Atoi(parts[2])
			if err != nil || n < 2 {
				t.Errorf("suffix must be >= 2, got %q", parts[2])
			}
		}
		taken[id] = true
	}
}

func TestAllocKitchenIDCollisionSuffix(t *testing.T) {
	// Saturate every un-numbered pair so the allocator must suffix.
	taken := map[string]bool{}
	for _, adj := range kitchenAdjectives {
		for _, dish := range kitchenDishes {
			taken[adj+"-"+dish] = true
		}
	}
	id := allocKitchenID(func(s string) bool { return taken[s] })
	if !strings.HasSuffix(id, "-2") {
		t.Fatalf("expected -2 suffix when all pairs taken, got %q", id)
	}
	if !kitchenIDRE.MatchString(id) {
		t.Fatalf("suffixed id %q bad format", id)
	}
	taken[id] = true
	id3 := allocKitchenID(func(s string) bool { return taken[s] })
	// Next claim may be another pair's -2 or this pair's -3.
	if !kitchenIDRE.MatchString(id3) || !strings.Contains(id3, "-") {
		t.Fatalf("second collision id %q", id3)
	}
	suf := id3[strings.LastIndex(id3, "-")+1:]
	n, err := strconv.Atoi(suf)
	if err != nil || n < 2 {
		t.Fatalf("expected numeric suffix >= 2, got %q", id3)
	}
}

func TestNewWorkspaceIDUsesKitchenNames(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	id := eng.newWorkspaceID("demo", nil)
	if !composedIDRE.MatchString(id) {
		t.Fatalf("newWorkspaceID = %q, want project.adj-dish", id)
	}
	if !strings.HasPrefix(id, "demo.") {
		t.Fatalf("newWorkspaceID = %q, want demo. prefix", id)
	}
	if strings.HasPrefix(strings.ToUpper(id), "LOOP-") {
		t.Fatalf("still using LOOP- format: %q", id)
	}
}

func TestCreateWorkspaceBranchIsRecurWorkspaceID(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	proj, err := st.AddProject(domain.Project{
		ID: "p-branch", Name: "p-branch", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	ws, err := eng.CreateWorkspace(proj.ID, "Auth refresh", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	want := "recur/" + gitpkg.WorkspaceSlug(ws.ID)
	if ws.Branch != want {
		t.Fatalf("Branch = %q, want %q", ws.Branch, want)
	}
	if !strings.HasPrefix(ws.ID, proj.ID+".") {
		t.Fatalf("ID = %q, want %s.<kitchen>", ws.ID, proj.ID)
	}
	wantPath := filepath.Join(eng.worktreeRoot, proj.ID, gitpkg.WorkspaceSlug(ws.ID))
	if ws.WorktreePath != wantPath {
		t.Fatalf("WorktreePath = %q, want %q", ws.WorktreePath, wantPath)
	}
}

func TestCreateWorkspaceHonorsBranchPrefixSetting(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	proj, err := st.AddProject(domain.Project{
		ID: "p-pfx", Name: "p-pfx", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}

	st.SetSettings(domain.Settings{BranchNamePrefix: domain.PrefixString("josh")})
	ws, err := eng.CreateWorkspace(proj.ID, "custom prefix", "")
	if err != nil {
		t.Fatalf("CreateWorkspace custom: %v", err)
	}
	want := "josh/" + gitpkg.WorkspaceSlug(ws.ID)
	if ws.Branch != want {
		t.Fatalf("CreateWorkspace(custom) Branch = %q, want %q", ws.Branch, want)
	}

	st.SetSettings(domain.Settings{BranchNamePrefix: domain.PrefixString("")})
	none, err := eng.CreateWorkspace(proj.ID, "no prefix", "")
	if err != nil {
		t.Fatalf("CreateWorkspace none: %v", err)
	}
	if none.Branch != gitpkg.WorkspaceSlug(none.ID) {
		t.Fatalf("CreateWorkspace(none) Branch = %q, want %q", none.Branch, gitpkg.WorkspaceSlug(none.ID))
	}
}

func TestDefaultBranchPrefixConstantsMatch(t *testing.T) {
	if gitpkg.DefaultBranchPrefix != domain.DefaultBranchNamePrefix {
		t.Fatalf("git.DefaultBranchPrefix %q != domain.DefaultBranchNamePrefix %q",
			gitpkg.DefaultBranchPrefix, domain.DefaultBranchNamePrefix)
	}
}

func TestNewWorkspaceIDAvoidsExisting(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	first := eng.newWorkspaceID("demo", nil)
	if _, err := st.AddWorkspace(domainWorkspace(first)); err != nil {
		t.Fatal(err)
	}
	// Force the same pair to be "preferred" by marking everything else free but
	// ensuring alloc skips the taken id — generate many and confirm uniqueness.
	seen := map[string]bool{first: true}
	for i := 0; i < 50; i++ {
		id := eng.newWorkspaceID("demo", nil)
		if seen[id] {
			t.Fatalf("duplicate workspace id %q", id)
		}
		seen[id] = true
		if _, err := st.AddWorkspace(domainWorkspace(id)); err != nil {
			t.Fatal(err)
		}
	}
}

func TestNewWorkspaceIDSkipsExistingRecurBranch(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	repo := initRepo(t)
	runGitIn(t, repo, "branch", "recur/bubbly-pretzel")

	eng, _, _, _ := newTestEngine(t, nil)
	mgr := gitpkg.NewManager(repo, t.TempDir())
	id := eng.newWorkspaceID("p", mgr)
	if id != "p.bubbly-pretzel-2" {
		t.Fatalf("newWorkspaceID(leftover recur/bubbly-pretzel) = %q, want p.bubbly-pretzel-2", id)
	}
}

func TestNewWorkspaceIDSkipsExistingUnprefixedBranch(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	repo := initRepo(t)
	runGitIn(t, repo, "branch", "bubbly-pretzel")

	eng, st, _, _ := newTestEngine(t, nil)
	st.SetSettings(domain.Settings{BranchNamePrefix: domain.PrefixString("")})
	mgr := gitpkg.NewManager(repo, t.TempDir()).WithBranchPrefix("")
	id := eng.newWorkspaceID("p", mgr)
	if id != "p.bubbly-pretzel-2" {
		t.Fatalf("newWorkspaceID(leftover bubbly-pretzel, none prefix) = %q, want p.bubbly-pretzel-2", id)
	}
}

func TestCreateWorkspaceDoesNotReuseLeftoverBranch(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	proj, err := st.AddProject(domain.Project{
		ID: "p-leftover", Name: "p-leftover", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}

	first, err := eng.CreateWorkspace(proj.ID, "old work", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if first.ID != "p-leftover.bubbly-pretzel" {
		t.Fatalf("first ID = %q, want p-leftover.bubbly-pretzel", first.ID)
	}
	if err := os.WriteFile(filepath.Join(first.WorktreePath, "stale.txt"), []byte("old work\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, first.WorktreePath, "add", "stale.txt")
	runGitIn(t, first.WorktreePath, "commit", "-m", "stale leftover")

	if err := eng.DeleteWorkspace(first.ID); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}

	second, err := eng.CreateWorkspace(proj.ID, "fresh work", "")
	if err != nil {
		t.Fatalf("CreateWorkspace after delete: %v", err)
	}
	if second.ID != "p-leftover.bubbly-pretzel-2" {
		t.Errorf("CreateWorkspace ID = %q, want p-leftover.bubbly-pretzel-2 (leftover branch must not recycle the id)", second.ID)
	}
	if second.Branch != "recur/bubbly-pretzel-2" {
		t.Errorf("CreateWorkspace Branch = %q, want recur/bubbly-pretzel-2", second.Branch)
	}
	if _, err := os.Stat(filepath.Join(second.WorktreePath, "stale.txt")); !os.IsNotExist(err) {
		t.Fatal("new workspace opened a leftover checkout (stale.txt present)")
	}
}

func TestComposeWorkspaceID(t *testing.T) {
	if got := composeWorkspaceID("recur", "flaky-ramen"); got != "recur.flaky-ramen" {
		t.Fatalf("got %q", got)
	}
	if got := composeWorkspaceID("", "flaky-ramen"); got != "flaky-ramen" {
		t.Fatalf("empty project: got %q", got)
	}
}

func TestCreateWorkspaceNestsUnderProject(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	proj, err := st.AddProject(domain.Project{
		ID: "kitchen", Name: "kitchen", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	ws, err := eng.CreateWorkspace(proj.ID, "nested", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.ID != "kitchen.bubbly-pretzel" {
		t.Fatalf("ID = %q, want kitchen.bubbly-pretzel", ws.ID)
	}
	want := filepath.Join(eng.worktreeRoot, "kitchen", "bubbly-pretzel")
	if ws.WorktreePath != want {
		t.Fatalf("WorktreePath = %q, want %q", ws.WorktreePath, want)
	}
	if _, err := os.Stat(want); err != nil {
		t.Fatalf("nested worktree missing: %v", err)
	}
}

func TestTwoProjectsShareKitchenSlug(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	eng, st, _, _ := newTestEngine(t, nil)
	p1, err := st.AddProject(domain.Project{
		ID: "alpha", Name: "alpha", RepoPath: initRepo(t), BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	p2, err := st.AddProject(domain.Project{
		ID: "beta", Name: "beta", RepoPath: initRepo(t), BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	a, err := eng.CreateWorkspace(p1.ID, "one", "")
	if err != nil {
		t.Fatalf("alpha: %v", err)
	}
	b, err := eng.CreateWorkspace(p2.ID, "two", "")
	if err != nil {
		t.Fatalf("beta: %v", err)
	}
	if a.ID != "alpha.bubbly-pretzel" || b.ID != "beta.bubbly-pretzel" {
		t.Fatalf("ids = %q, %q", a.ID, b.ID)
	}
	if a.Branch != "recur/bubbly-pretzel" || b.Branch != "recur/bubbly-pretzel" {
		t.Fatalf("branches = %q, %q", a.Branch, b.Branch)
	}
}

func TestSameRepoProjectsDoNotReuseKitchenSlug(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	p1, err := st.AddProject(domain.Project{
		ID: "alpha", Name: "alpha", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	p2, err := st.AddProject(domain.Project{
		ID: "beta", Name: "beta", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	// Persist-before-allocate (BeginCreateWorkspace): the first worktree is not
	// on disk yet, so git branch listing cannot see the claim. The store +
	// same-repo scan must still refuse to reuse bubbly-pretzel.
	a, err := eng.BeginCreateWorkspace(p1.ID, "one", "")
	if err != nil {
		t.Fatalf("alpha: %v", err)
	}
	b, err := eng.BeginCreateWorkspace(p2.ID, "two", "")
	if err != nil {
		t.Fatalf("beta: %v", err)
	}
	if a.ID != "alpha.bubbly-pretzel" {
		t.Fatalf("alpha ID = %q, want alpha.bubbly-pretzel", a.ID)
	}
	if b.ID != "beta.bubbly-pretzel-2" {
		t.Fatalf("beta ID = %q, want beta.bubbly-pretzel-2 (same repo must not reuse the kitchen slug)", b.ID)
	}
	if a.Branch != "recur/bubbly-pretzel" || b.Branch != "recur/bubbly-pretzel-2" {
		t.Fatalf("branches = %q, %q", a.Branch, b.Branch)
	}
}

func TestRepoKeyResolvesSymlinks(t *testing.T) {
	dir := t.TempDir()
	link := filepath.Join(t.TempDir(), "alias")
	if err := os.Symlink(dir, link); err != nil {
		t.Skipf("symlink: %v", err)
	}
	if repoKey(dir) != repoKey(link) {
		t.Fatalf("repoKey(dir)=%q repoKey(link)=%q, want equal", repoKey(dir), repoKey(link))
	}
}

func TestSameRepoSymlinkPathsDoNotReuseKitchenSlug(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	link := filepath.Join(t.TempDir(), "alias")
	if err := os.Symlink(repo, link); err != nil {
		t.Skipf("symlink: %v", err)
	}
	p1, err := st.AddProject(domain.Project{
		ID: "alpha", Name: "alpha", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	p2, err := st.AddProject(domain.Project{
		ID: "beta", Name: "beta", RepoPath: link, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	a, err := eng.BeginCreateWorkspace(p1.ID, "one", "")
	if err != nil {
		t.Fatalf("alpha: %v", err)
	}
	b, err := eng.BeginCreateWorkspace(p2.ID, "two", "")
	if err != nil {
		t.Fatalf("beta: %v", err)
	}
	if a.ID != "alpha.bubbly-pretzel" {
		t.Fatalf("alpha ID = %q, want alpha.bubbly-pretzel", a.ID)
	}
	if b.ID != "beta.bubbly-pretzel-2" {
		t.Fatalf("beta ID = %q, want beta.bubbly-pretzel-2 (symlink alias is the same repo)", b.ID)
	}
}

func TestDispatchFreshLoopDoesNotRestoreLeftoverBranch(t *testing.T) {
	repo := initRepo(t)
	runGitIn(t, repo, "checkout", "-b", "recur/bubbly-pretzel")
	if err := os.WriteFile(filepath.Join(repo, "stale.txt"), []byte("old\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, repo, "add", "stale.txt")
	runGitIn(t, repo, "commit", "-m", "stale leftover")
	runGitIn(t, repo, "checkout", "main")

	eng, st, _, _ := newTestEngine(t, &convergeRunner{findingsPerIter: []int{0}})
	projID := addProject(t, st, repo)
	rec := domain.Workspace{
		ID: "bubbly-pretzel", ProjectID: projID, Goal: "fresh",
		RunStarted: true, Branch: "recur/bubbly-pretzel", BaseBranch: "main",
		State: domain.StateQueued, StatusLabel: "Queued", Group: domain.GroupActive,
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatal(err)
	}
	if !eng.gov.TryAcquireLoop() {
		t.Fatal("TryAcquireLoop failed")
	}
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	ws, ok := st.Workspace("bubbly-pretzel")
	if !ok {
		t.Fatal("workspace missing after dispatch")
	}
	if ws.Branch != "recur/bubbly-pretzel-2" {
		t.Errorf("dispatch Branch = %q, want recur/bubbly-pretzel-2", ws.Branch)
	}
	if _, err := os.Stat(filepath.Join(ws.WorktreePath, "stale.txt")); !os.IsNotExist(err) {
		t.Fatal("fresh dispatch restored leftover checkout")
	}
}

func TestDispatchKeepsIssueLinkedDuringWorktreeAdd(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &convergeRunner{findingsPerIter: []int{0}})
	projID := addProject(t, st, initRepo(t))
	rec := domain.Workspace{
		ID: "issue-link", ProjectID: projID, Goal: "linked",
		RunStarted: true, BaseBranch: "main",
		State: domain.StateQueued, StatusLabel: "Queued", Group: domain.GroupActive,
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatal(err)
	}
	issue := &domain.IssueRef{Source: "linear", ID: "lin-1", Key: "ENG-1"}
	st.UpdateWorkspace(rec.ID, func(w *domain.Workspace) { w.Issue = issue })
	if !eng.gov.TryAcquireLoop() {
		t.Fatal("TryAcquireLoop failed")
	}
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	ws, _ := st.Workspace(rec.ID)
	if ws.Issue == nil || *ws.Issue != *issue {
		t.Fatalf("Issue after dispatch = %+v, want %+v", ws.Issue, issue)
	}
}

func pinKitchenPair(t *testing.T, adj, dish string) {
	t.Helper()
	oldAdj, oldDish := kitchenAdjectives, kitchenDishes
	kitchenAdjectives = []string{adj}
	kitchenDishes = []string{dish}
	t.Cleanup(func() {
		kitchenAdjectives, kitchenDishes = oldAdj, oldDish
	})
}

func runGitIn(t *testing.T, dir string, args ...string) {
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

func domainWorkspace(id string) domain.Workspace {
	return domain.Workspace{ID: id, ProjectID: "p", Goal: "g", Type: domain.WorkspaceSolo}
}

func kitchenWordIn(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}
