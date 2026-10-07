package engine

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	"recur/internal/store"
)

func TestFixturePackParses(t *testing.T) {
	parsed, err := bundles.ParseManifestFull(t.TempDir(), []byte(testLoopsTOML))
	if err != nil {
		t.Fatalf("ParseManifestFull: %v", err)
	}
	if _, ok := bundles.FindLoop(parsed.Loops, "test.loops.docs"); !ok {
		t.Fatalf("docs loop missing; loops=%d", len(parsed.Loops))
	}
	if _, ok := bundles.FindRole(parsed.Roles, "test.roles.writer"); !ok {
		t.Fatalf("writer role missing; roles=%d", len(parsed.Roles))
	}
}

const testLoopsTOML = `
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[role]]
id = "test.roles.qa"
name = "QA"
code = "QA"
mode = "ask"
tools = ["attach_evidence", "playwright", "report_result"]

[[role]]
id = "test.roles.writer"
name = "Writer"
code = "WR"
mode = "agent"

[[role]]
id = "test.roles.editor"
name = "Editor"
code = "ED"
mode = "review"

[[policy]]
id = "test.policies.fail"
name = "Fail at cap"
max_rounds = 4
on_max_rounds = "fail"
findings_threshold = { critical = 0, high = 0, medium = 0 }
plan_approval = "never"
extend_rounds = 1
on_fail = "fail"

[[loop]]
id = "test.loops.verify"
name = "Verify fixture"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
next = "verify"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[[loop.phase]]
id = "verify"
uses = "verify"
role = "test.roles.qa"
next = "pr"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"

[[loop.phase]]
id = "ondemand"
uses = "verify"
dispatch = "on_demand"
role = "test.roles.qa"

[[loop]]
id = "test.loops.verify-reject"
name = "Verify on_reject"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
next = "verify"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[[loop.phase]]
id = "verify"
uses = "verify"
on_reject = "converge"
role = "test.roles.qa"
next = "pr"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"

[[loop]]
id = "test.loops.converge"
name = "Converge reviews"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[[loop]]
id = "test.loops.converge-pr"
name = "Converge then PR"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
next = "pr"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"

[[loop]]
id = "test.loops.docs"
name = "Docs loop"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
findings_threshold = 0
[loop.phase.roles]
implementer = "test.roles.writer"
reviewer = "test.roles.editor"
`

func installFixturePack(t *testing.T) {
	t.Helper()
	installTestPack(t, "TestLoops", testLoopsTOML)
}

func installShippedLoopPack(t *testing.T) {
	t.Helper()
	src := filepath.Join("..", "..", "bundles", "Loop")
	if _, err := os.Stat(filepath.Join(src, "bundle.toml")); err != nil {
		t.Skip(err)
	}
	dstRoot, err := store.BundlesDir()
	if err != nil {
		t.Fatalf("BundlesDir: %v", err)
	}
	dst := filepath.Join(dstRoot, "Loop")
	if err := os.CopyFS(dst, os.DirFS(src)); err != nil {
		t.Fatalf("copy shipped Loop pack: %v", err)
	}
}

func TestConvergeStartReviewSkipsFirstCoder(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{1, 0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "Review the current diff", "", "", "test.loops.converge", "")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 15*time.Second, "converge loop to idle", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePaused || l.State == domain.StateStopped)
	})
	l, _ := st.Workspace(id)
	if l.State == domain.StatePROpen {
		t.Fatal("StartWorkspaceWith(test.loops.converge) opened a PR; want idle after the bar")
	}
	runner.mu.Lock()
	coderRuns, reviewRuns := runner.coderRuns, runner.reviewRuns
	runner.mu.Unlock()
	if coderRuns != 1 {
		t.Errorf("StartWorkspaceWith(test.loops.converge) coderRuns = %d, want 1 (fix only)", coderRuns)
	}
	if reviewRuns != 2 {
		t.Errorf("StartWorkspaceWith(test.loops.converge) reviewRuns = %d, want 2", reviewRuns)
	}
}

// start=review with a clean first review never runs the Coder. The engine must
// still commit the uncommitted worktree before ensure_pr, or DiffPR is empty.
func TestConvergePRStartReviewCommitsUncommittedDiff(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}, commitText: "Add human.txt"}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := addProject(t, st, repo)

	ws, err := eng.CreateWorkspace(projID, "Ship the human diff", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.WorktreePath == "" {
		t.Fatal("CreateWorkspace WorktreePath = empty")
	}
	humanFile := filepath.Join(ws.WorktreePath, "human.txt")
	if err := os.WriteFile(humanFile, []byte("human change\n"), 0o644); err != nil {
		t.Fatalf("WriteFile(%s): %v", humanFile, err)
	}

	eng.RememberChatLoopPolicy(ws.ID, "PA", "test.loops.converge-pr", "")
	if reply := eng.SendWorkspaceMessage(ws.ID, "PA", "review and open a PR", "", nil, nil, "loop", "Review & fix to a PR"); contains(reply, "I couldn't engage") {
		t.Fatalf("engage = %q", reply)
	}

	waitFor(t, 15*time.Second, "converge-pr loop to reach pr_open", func() bool {
		l, ok := st.Workspace(ws.ID)
		return ok && l.State == domain.StatePROpen
	})

	runner.mu.Lock()
	coderRuns, reviewRuns := runner.coderRuns, runner.reviewRuns
	runner.mu.Unlock()
	if coderRuns != 0 {
		t.Errorf("test.loops.converge-pr coderRuns = %d, want 0 (start=review, zero findings)", coderRuns)
	}
	if reviewRuns != 1 {
		t.Errorf("test.loops.converge-pr reviewRuns = %d, want 1", reviewRuns)
	}

	pr, ok := st.PRByWorkspace(ws.ID)
	if !ok {
		t.Fatal("test.loops.converge-pr opened no PR")
	}
	foundHuman := false
	for _, f := range pr.Files {
		if f.Path == "human.txt" {
			foundHuman = true
			if f.Diff.Add == 0 {
				t.Errorf("human.txt PR diff = %+v, want additions", f.Diff)
			}
		}
	}
	if !foundHuman {
		t.Errorf("PR files = %+v, want human.txt from the uncommitted worktree", pr.Files)
	}
	if len(pr.Commits) == 0 {
		t.Error("PR commits = [], want the engine commit of the reviewed worktree")
	}

	logOut, err := gitOut(repo, ws.WorktreePath, "log", "--oneline", "main..HEAD")
	if err != nil {
		t.Fatalf("git log: %v", err)
	}
	if strings.TrimSpace(logOut) == "" {
		t.Error("git log main..HEAD is empty, want the engine commit of human.txt")
	}
	if !strings.Contains(logOut, "Add human.txt") {
		t.Errorf("git log main..HEAD = %q, want the generated commit message", logOut)
	}
	statusOut, err := gitOut(repo, ws.WorktreePath, "status", "--porcelain")
	if err != nil {
		t.Fatalf("git status: %v", err)
	}
	if strings.TrimSpace(statusOut) != "" {
		t.Errorf("git status --porcelain = %q, want clean after the engine commit", statusOut)
	}
}

// Leftover opens from a prior iteration must fail the policy bar even when this
// review turn files nothing new (start=review restart / "no new findings").
func TestConvergeBarUsesAllOpenFindings(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0, 0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := addProject(t, st, repo)

	ws, err := eng.CreateWorkspace(projID, "leftover medium finding", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "old-medium", WorkspaceID: ws.ID, Iteration: 8,
		File: "engine.go", Line: 1, Severity: domain.SevMedium,
		Message: "queued close race", Status: string(domain.FindingOpen),
	})
	eng.RememberChatLoopPolicy(ws.ID, "PA", "test.loops.converge", "test.policies.fail")
	if reply := eng.SendWorkspaceMessage(ws.ID, "PA", "fix leftover", "", nil, nil, "loop", "Review & fix"); contains(reply, "I couldn't engage") {
		t.Fatalf("engage = %q", reply)
	}

	waitFor(t, 15*time.Second, "converge loop to idle after leftover fix", func() bool {
		l, ok := st.Workspace(ws.ID)
		return ok && (l.State == domain.StatePaused || l.State == domain.StateStopped)
	})
	runner.mu.Lock()
	coderRuns, reviewRuns := runner.coderRuns, runner.reviewRuns
	runner.mu.Unlock()
	if reviewRuns < 1 {
		t.Fatalf("reviewer never ran: coderRuns=%d reviewRuns=%d", coderRuns, reviewRuns)
	}
	if coderRuns < 1 {
		t.Errorf("convergeBarPassed(leftover medium, 0 new) coderRuns = %d, want >= 1 (send back to coder)", coderRuns)
	}
	if reviewRuns < 2 {
		t.Errorf("convergeBarPassed(leftover medium, 0 new) reviewRuns = %d, want >= 2 (re-review after fix)", reviewRuns)
	}
	if open := st.OpenFindingsCount(ws.ID); open != 0 {
		t.Errorf("OpenFindingsCount(%s) = %d, want 0 after the coder checklist", ws.ID, open)
	}
}

func TestDocsLoopConvergesWithoutPR(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "Write the README", "", "", "test.loops.docs", "test.policies.fail")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 15*time.Second, "docs loop to idle", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePaused || l.State == domain.StateStopped)
	})
	l, _ := st.Workspace(id)
	if l.State == domain.StatePROpen {
		t.Fatal("docs loop must not open a PR")
	}
	codes := map[string]bool{}
	for _, a := range st.Agents(id) {
		codes[a.Code] = true
	}
	if codes["CO"] {
		t.Error("docs loop must not spawn the first-party Coder")
	}
	if !codes["WR"] || !codes["ED"] {
		t.Errorf("docs loop should spawn writer+editor, agents=%v", codes)
	}
}

func TestShippedSecurityLoopUsesSecurityReviewer(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "Security look at this diff", "", "", bundles.SecurityLoopID, "")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 15*time.Second, "security loop to idle", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePaused || l.State == domain.StateStopped)
	})
	l, _ := st.Workspace(id)
	if l.State == domain.StatePROpen {
		t.Fatal("security loop must not open a PR")
	}
	codes := map[string]bool{}
	for _, a := range st.Agents(id) {
		codes[a.Code] = true
	}
	if codes["RV"] {
		t.Error("security loop must not spawn the first-party Reviewer")
	}
	if !codes["SR"] {
		t.Errorf("security loop should spawn SR, agents=%v", codes)
	}
}

func TestVerifyOnRejectReentersConverge(t *testing.T) {
	repo := initRepo(t)
	failRun := []stepEvidence{{step: "sign-in 500", shot: "/shots/a.png", passed: false}}
	passRun := []stepEvidence{{step: "sign-in ok", shot: "/shots/b.png", passed: true}}
	runner := &verifyRunner{
		findingsPerIter: []int{0, 0},
		evidencePerRun:  [][]stepEvidence{failRun, passRun},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := verifyDevProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "verify then fix", "", "", "test.loops.verify-reject", "")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 40*time.Second, "on_reject loop to open a PR", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
	runner.mu.Lock()
	qa := runner.qaRuns
	runner.mu.Unlock()
	if qa < 2 {
		t.Errorf("on_reject=converge should re-verify, qaRuns=%d", qa)
	}
}

func TestEngageMissingLoopRefused(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, repo)
	if _, err := eng.StartWorkspaceWith(projID, "gone loop", "", "", "test.loops.does-not-exist", ""); err == nil {
		t.Fatal("StartWorkspaceWith with a missing loop id should refuse")
	}
}

func TestResumeMissingLoopIdles(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &convergeRunner{findingsPerIter: []int{0}})
	installFixturePack(t)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "docs then delete the pack", "", "", "test.loops.docs", "")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 15*time.Second, "docs loop to idle", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePaused
	})

	dir, err := store.BundlesDir()
	if err != nil {
		t.Fatalf("BundlesDir: %v", err)
	}
	if err := os.RemoveAll(filepath.Join(dir, "TestLoops")); err != nil {
		t.Fatalf("remove pack: %v", err)
	}

	eng.Resume(id)
	waitFor(t, 10*time.Second, "resume of deleted loop to idle", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePaused
	})
}
