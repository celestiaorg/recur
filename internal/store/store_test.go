package store

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/uuid"

	"recur/internal/domain"
	"recur/internal/projfile"
)

// openTemp opens a store backed by a fresh db under a temp dir and registers a
// Close on cleanup. Each call to reopenTemp on the same dir reloads from disk.
func openTemp(t *testing.T, dir string) *Store {
	t.Helper()
	path := filepath.Join(dir, "recur.db")
	_, statErr := os.Stat(path)
	first := os.IsNotExist(statErr)
	s, err := open(path)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	// Persistence tests round-trip the mock dataset. Seed it only on first
	// open so a reopen after ClearAll stays empty (same as a real user DB).
	if first {
		persistDemoSeed(t, s)
	}
	return s
}

func persistDemoSeed(t *testing.T, s *Store) {
	t.Helper()
	s.mu.Lock()
	defer s.mu.Unlock()
	s.seedDemo()
	if s.db != nil {
		if err := s.db.seedAll(s); err != nil {
			t.Fatalf("persist demo seed: %v", err)
		}
	}
}

func TestSeedThenLoadIsStable(t *testing.T) {
	dir := t.TempDir()

	s1 := openTemp(t, dir)
	if got := len(s1.Projects()); got != 3 {
		t.Fatalf("seeded projects: got %d, want 3", got)
	}
	if got := len(s1.Workspaces()); got != 5 {
		t.Fatalf("seeded loops: got %d, want 5", got)
	}
	if got := len(s1.PRs()); got != 3 {
		t.Fatalf("seeded prs: got %d, want 3", got)
	}
	if _, ok := s1.Escalation("LOOP-2218"); !ok {
		t.Fatalf("seeded escalation LOOP-2218 missing")
	}
	if got := len(s1.Feed("LOOP-2231")); got == 0 {
		t.Fatalf("seeded feed for LOOP-2231 missing")
	}
	s1.Close()

	// Reopen: must load from disk (not re-seed) and match the seeded shape.
	s2 := openTemp(t, dir)
	if got := len(s2.Projects()); got != 3 {
		t.Errorf("reloaded projects: got %d, want 3", got)
	}
	if got := len(s2.Workspaces()); got != 5 {
		t.Errorf("reloaded loops: got %d, want 5", got)
	}
	pr, ok := s2.PR("482")
	if !ok || pr.Number != 482 || len(pr.Files) != 3 {
		t.Errorf("reloaded PR #482 wrong: ok=%v files=%d", ok, len(pr.Files))
	}
	if got := len(s2.Feed("LOOP-2231")); got != len(s1.Feed("LOOP-2231")) {
		t.Errorf("reloaded feed count drifted: %d vs %d", got, len(s1.Feed("LOOP-2231")))
	}
}

func TestAddProjectPersistsAcrossReopen(t *testing.T) {
	dir := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)
	repo := filepath.Join(home, "code", "widgets")
	if err := os.MkdirAll(repo, 0o755); err != nil {
		t.Fatal(err)
	}

	s1 := openTemp(t, dir)
	saved, err := s1.AddProject(domain.Project{
		ID: "widgets", Name: "widgets", RepoPath: "~/code/widgets", BaseBranch: "main",
		SetupCmd: "make setup", TeardownCmd: "make clean-wt",
		RunCommands:      []domain.RunCommand{{Name: "dev", Cmd: "make dev PORT=$RECUR_PORT"}},
		AppURL:           "http://localhost:$RECUR_PORT",
		Env:              "WAILS_VITE_PORT=$RECUR_PORT\n# dogfood\nRECUR_DATA_DIR=$HOME/tmp",
		WriteProjectFile: true,
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if saved.ID != "widgets" {
		t.Fatalf("saved id: got %q", saved.ID)
	}
	s1.Close()

	s2 := openTemp(t, dir)
	got, ok := s2.Project("widgets")
	if !ok {
		t.Fatalf("project widgets missing after reopen")
	}
	if domain.VerifyDevCmd(got) != "make dev PORT=$RECUR_PORT" || got.RepoPath != "~/code/widgets" {
		t.Errorf("project fields not persisted: %+v", got)
	}
	if got.TeardownCmd != "make clean-wt" {
		t.Errorf("teardown_cmd not persisted: %q", got.TeardownCmd)
	}
	if !strings.Contains(got.Env, "WAILS_VITE_PORT=$RECUR_PORT") || !strings.Contains(got.Env, "# dogfood") {
		t.Errorf("env script not persisted: %q", got.Env)
	}
}

func TestReorderProjectsPersistsAcrossReopen(t *testing.T) {
	dir := t.TempDir()

	s1 := openTemp(t, dir)
	projs := s1.Projects()
	if len(projs) < 2 {
		t.Fatalf("need at least 2 seeded projects, got %d", len(projs))
	}
	reversed := make([]string, len(projs))
	for i, p := range projs {
		reversed[len(projs)-1-i] = p.ID
	}
	if err := s1.ReorderProjects(reversed); err != nil {
		t.Fatalf("ReorderProjects: %v", err)
	}
	if got := projectIDs(s1.Projects()); !equalStrings(got, reversed) {
		t.Fatalf("in-memory order: got %v want %v", got, reversed)
	}
	s1.Close()

	s2 := openTemp(t, dir)
	if got := projectIDs(s2.Projects()); !equalStrings(got, reversed) {
		t.Fatalf("after reopen: got %v want %v", got, reversed)
	}
}

func TestReorderProjectsRejectsBadIDSet(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	projs := s.Projects()
	if len(projs) < 2 {
		t.Fatalf("need at least 2 seeded projects, got %d", len(projs))
	}
	before := projectIDs(s.Projects())

	if err := s.ReorderProjects([]string{projs[0].ID}); err == nil {
		t.Fatal("expected error for incomplete id list")
	}
	if err := s.ReorderProjects(append(before, "nope")); err == nil {
		t.Fatal("expected error for unknown id")
	}
	if got := projectIDs(s.Projects()); !equalStrings(got, before) {
		t.Fatalf("order mutated after failed reorder: got %v want %v", got, before)
	}
}

func projectIDs(ps []domain.Project) []string {
	out := make([]string, len(ps))
	for i, p := range ps {
		out[i] = p.ID
	}
	return out
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// TestAddProjectDBErrorLeavesCacheUnchanged closes the underlying db and asserts
// AddProject returns the error AND does not leave a phantom project in the
// in-memory cache (the DB write must succeed before the cache mutates).
func TestAddProjectDBErrorLeavesCacheUnchanged(t *testing.T) {
	dir := t.TempDir()

	s := openTemp(t, dir)
	before := len(s.Projects())

	// Force every subsequent write-through to fail by closing the db handle.
	if err := s.db.close(); err != nil {
		t.Fatalf("close db: %v", err)
	}

	_, err := s.AddProject(domain.Project{ID: "ghost", Name: "ghost"})
	if err == nil {
		t.Fatalf("expected AddProject to fail on a closed db")
	}
	if got := len(s.Projects()); got != before {
		t.Errorf("phantom project left in cache: got %d, want %d", got, before)
	}
	if _, ok := s.Project("ghost"); ok {
		t.Errorf("phantom project %q present after failed write", "ghost")
	}
}

// TestPRGetterDeepCopy fetches a PR, mutates the returned nested slices, then
// re-fetches and asserts the store's copy was not aliased (closes the M2 race).
func TestPRGetterDeepCopy(t *testing.T) {
	s := NewInMemory()

	pr, ok := s.PR("482")
	if !ok {
		t.Fatalf("PR 482 missing")
	}
	if len(pr.Files) == 0 || len(pr.Commits) == 0 {
		t.Fatalf("seed PR 482 has no files/commits to test: files=%d commits=%d", len(pr.Files), len(pr.Commits))
	}
	wantFiles := len(pr.Files)
	wantCommits := len(pr.Commits)
	origPath := pr.Files[0].Path

	// Mutate the returned slices in-place and append.
	pr.Files[0].Path = "MUTATED/path"
	pr.Files = append(pr.Files, domain.FileDiff{Path: "extra"})
	pr.Commits[0].Message = "MUTATED message"
	pr.Commits = append(pr.Commits, domain.Commit{Hash: "deadbeef"})
	if len(pr.Files[0].Lines) > 0 {
		pr.Files[0].Lines[0].CodeHTML = "MUTATED html"
	}

	again, _ := s.PR("482")
	if len(again.Files) != wantFiles {
		t.Errorf("store Files count changed via caller mutation: got %d, want %d", len(again.Files), wantFiles)
	}
	if len(again.Commits) != wantCommits {
		t.Errorf("store Commits count changed via caller mutation: got %d, want %d", len(again.Commits), wantCommits)
	}
	if again.Files[0].Path != origPath {
		t.Errorf("store Files[0].Path mutated by caller: got %q, want %q", again.Files[0].Path, origPath)
	}
	if again.Commits[0].Message == "MUTATED message" {
		t.Errorf("store Commits[0].Message mutated by caller")
	}
}

func TestUpdateProjectRunConfigPersists(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()

	s1 := openTemp(t, dir)
	p, err := s1.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo, BaseBranch: "main"})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	p.RunCommands = []domain.RunCommand{{Name: "dev", Cmd: "pnpm dev --port $RECUR_PORT --strictPort"}}
	p.AppURL = "http://127.0.0.1:$RECUR_PORT/health"
	p.WriteProjectFile = true
	if _, err := s1.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	s1.Close()

	s2 := openTemp(t, dir)
	got, ok := s2.Project("app")
	if !ok {
		t.Fatal("project app missing after reopen")
	}
	if domain.VerifyDevCmd(got) != "pnpm dev --port $RECUR_PORT --strictPort" {
		t.Errorf("run not persisted: %q", domain.VerifyDevCmd(got))
	}
	if got.AppURL != "http://127.0.0.1:$RECUR_PORT/health" {
		t.Errorf("app_url not persisted: %q", got.AppURL)
	}
}

func TestDeletedProjectTomlDoesNotResurrectDB(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	s := openTemp(t, dir)
	if _, err := s.AddProject(domain.Project{
		ID: "app", Name: "app", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "make setup", Env: "A=1",
		RunCommands:      []domain.RunCommand{{Name: "dev", Cmd: "make dev"}},
		WriteProjectFile: true,
	}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if err := os.Remove(filepath.Join(repo, projfile.RelPath)); err != nil {
		t.Fatalf("remove %s: %v", projfile.RelPath, err)
	}
	got, ok := s.Project("app")
	if !ok {
		t.Fatal("project app missing")
	}
	if got.SetupCmd != "" || domain.VerifyDevCmd(got) != "" || got.Env != "" {
		t.Errorf("Project after deleting toml: setup=%q verify=%q env=%q, want empty (file is source of truth)",
			got.SetupCmd, domain.VerifyDevCmd(got), got.Env)
	}
	if got.HasProjectFile {
		t.Errorf("HasProjectFile = true, want false after delete")
	}
}

func TestAddProjectSeedsFromProjectToml(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	if err := projfile.Write(domain.Project{
		RepoPath:    repo,
		SetupCmd:    "pnpm install",
		RunCommands: []domain.RunCommand{{Name: "dev", Cmd: "pnpm dev --port $RECUR_PORT"}},
		AppURL:      "http://localhost:$RECUR_PORT",
		Env:         "FOO=bar",
	}); err != nil {
		t.Fatal(err)
	}

	s := openTemp(t, dir)
	saved, err := s.AddProject(domain.Project{
		ID: "app", Name: "app", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if saved.SetupCmd != "pnpm install" || domain.VerifyDevCmd(saved) != "pnpm dev --port $RECUR_PORT" {
		t.Errorf("not seeded from file: %+v", saved)
	}
	if saved.AppURL != "http://localhost:$RECUR_PORT" || saved.Env != "FOO=bar" {
		t.Errorf("url/env not seeded: %+v", saved)
	}
}

func TestAddProjectDoesNotRewriteExistingTomlWithoutOptIn(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	if err := projfile.Write(domain.Project{RepoPath: repo, RunCommands: []domain.RunCommand{{Name: "dev", Cmd: "from-file"}}}); err != nil {
		t.Fatal(err)
	}
	before, err := os.ReadFile(filepath.Join(repo, projfile.RelPath))
	if err != nil {
		t.Fatal(err)
	}
	s := openTemp(t, dir)
	saved, err := s.AddProject(domain.Project{
		ID: "app", Name: "app", RepoPath: repo,
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if !saved.HasProjectFile {
		t.Errorf("AddProject HasProjectFile = false, want true when %s exists", projfile.RelPath)
	}
	if domain.VerifyDevCmd(saved) != "from-file" {
		t.Errorf("AddProject return VerifyDevCmd = %q, want file overlay %q", domain.VerifyDevCmd(saved), "from-file")
	}
	after, err := os.ReadFile(filepath.Join(repo, projfile.RelPath))
	if err != nil {
		t.Fatal(err)
	}
	if string(after) != string(before) {
		t.Fatalf("AddProject without WriteProjectFile rewrote toml:\n%s", after)
	}
}

func TestAddProjectReturnsOverlayWhenFileExists(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	body := "[[run]]\nname = \"dev\"\ncmd = \"from-file\"\n[[run]]\nname = \"test\"\ncmd = \"go test\"\n"
	if err := os.WriteFile(filepath.Join(repo, projfile.RelPath), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	s := openTemp(t, dir)
	saved, err := s.AddProject(domain.Project{
		ID: "app", Name: "app", RepoPath: repo,
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if !saved.HasProjectFile {
		t.Fatal("HasProjectFile = false, want true")
	}
	if len(saved.RunCommands) != 2 || saved.RunCommands[1].Name != "test" {
		t.Fatalf("AddProject RunCommands = %+v, want file [[run]] list", saved.RunCommands)
	}
}

func TestUpdateProjectRunConfigReturnsOverlay(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if err := os.MkdirAll(filepath.Join(repo, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, projfile.RelPath), []byte("[[run]]\nname = \"dev\"\ncmd = \"file-cmd\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	p.Open = false
	saved, err := s.UpdateProjectRunConfig(p)
	if err != nil {
		t.Fatalf("Update: %v", err)
	}
	if !saved.HasProjectFile {
		t.Fatal("Update HasProjectFile = false, want true")
	}
	if domain.VerifyDevCmd(saved) != "file-cmd" {
		t.Errorf("Update VerifyDevCmd = %q, want file overlay %q", domain.VerifyDevCmd(saved), "file-cmd")
	}
}

func TestProjectTomlOverlaysDB(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	s := openTemp(t, dir)
	if _, err := s.AddProject(domain.Project{
		ID: "app", Name: "app", RepoPath: repo,
	}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if err := os.MkdirAll(filepath.Join(repo, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, projfile.RelPath), []byte("[[run]]\nname = \"dev\"\ncmd = \"file-cmd\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	got, ok := s.Project("app")
	if !ok {
		t.Fatal("missing project")
	}
	if domain.VerifyDevCmd(got) != "file-cmd" {
		t.Errorf("file should overlay DB: %q", domain.VerifyDevCmd(got))
	}
}

func TestUpdateProjectRunConfigWritesToml(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	p.SetupCmd = "make setup"
	p.Env = "A=1"
	p.WriteProjectFile = true
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("Update: %v", err)
	}
	got := projfile.Overlay(domain.Project{RepoPath: repo})
	if got.SetupCmd != "make setup" || got.Env != "A=1" {
		t.Errorf("file after create: %+v", got)
	}
	p.SetupCmd = "make setup2"
	p.WriteProjectFile = true
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("Update existing: %v", err)
	}
	got = projfile.Overlay(domain.Project{RepoPath: repo})
	if got.SetupCmd != "make setup2" {
		t.Errorf("settings save should overwrite toml: SetupCmd = %q, want %q", got.SetupCmd, "make setup2")
	}
}

func TestUpdateProjectRunConfigDoesNotCreateTomlWithoutOptIn(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{
		ID: "app", Name: "app", RepoPath: repo, Env: "SECRET=1",
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	p.Open = false
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("Update: %v", err)
	}
	if _, err := os.Stat(filepath.Join(repo, projfile.RelPath)); !os.IsNotExist(err) {
		t.Fatalf("sidebar toggle with env should not create project.toml, err=%v", err)
	}
	p.SetupCmd = "make setup"
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("Update fields: %v", err)
	}
	if _, err := os.Stat(filepath.Join(repo, projfile.RelPath)); !os.IsNotExist(err) {
		t.Fatalf("settings save without WriteProjectFile should not create project.toml, err=%v", err)
	}
}

func TestUpdateProjectOpenDoesNotCreateToml(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo, Open: true})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	p.Open = false
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("Update: %v", err)
	}
	if _, err := os.Stat(filepath.Join(repo, projfile.RelPath)); !os.IsNotExist(err) {
		t.Fatalf("sidebar toggle should not create project.toml, err=%v", err)
	}
}

func TestPinnedRunPersistsWithoutRewritingToml(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	hand := "# keep me\n[[run]]\nname = \"dev\"\ncmd = \"wails3 task dev\"\n"
	if err := os.WriteFile(filepath.Join(repo, projfile.RelPath), []byte(hand), 0o644); err != nil {
		t.Fatal(err)
	}
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	p.PinnedRun = "dev"
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("pin: %v", err)
	}
	got, _ := s.Project("app")
	if got.PinnedRun != "dev" {
		t.Errorf("pinnedRun: %q", got.PinnedRun)
	}
	data, err := os.ReadFile(filepath.Join(repo, projfile.RelPath))
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != hand {
		t.Fatalf("pin rewrote toml:\n%s", data)
	}
	s.Close()
	s2 := openTemp(t, dir)
	again, _ := s2.Project("app")
	if again.PinnedRun != "dev" {
		t.Errorf("pinnedRun after reopen: %q", again.PinnedRun)
	}
}

func TestSkillAndMCPOverridesPersist(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	p.DisabledSkills = []string{"noise-skill"}
	p.EnabledSkills = []string{"special"}
	p.DisabledMCP = []string{"linear"}
	p.EnabledMCP = []string{"acme"}
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("update: %v", err)
	}
	got, _ := s.Project("app")
	if got.DisabledSkills[0] != "noise-skill" || got.EnabledSkills[0] != "special" {
		t.Fatalf("skills=%v %v", got.DisabledSkills, got.EnabledSkills)
	}
	if got.DisabledMCP[0] != "linear" || got.EnabledMCP[0] != "acme" {
		t.Fatalf("mcp=%v %v", got.DisabledMCP, got.EnabledMCP)
	}
	s.Close()
	s2 := openTemp(t, dir)
	again, _ := s2.Project("app")
	if again.DisabledSkills[0] != "noise-skill" || again.EnabledMCP[0] != "acme" {
		t.Fatalf("after reopen skills=%v mcp=%v", again.DisabledSkills, again.EnabledMCP)
	}
}

func TestNamedRunsPersistToTomlAndPinStaysLocal(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	p.RunCommands = []domain.RunCommand{
		{Name: "dev", Cmd: "wails3 task dev"},
		{Name: "test", Cmd: "go test ./..."},
	}
	p.PinnedRun = "test"
	p.WriteProjectFile = true
	saved, err := s.UpdateProjectRunConfig(p)
	if err != nil {
		t.Fatalf("Update: %v", err)
	}
	if saved.PinnedRun != "test" {
		t.Errorf("pin: %q", saved.PinnedRun)
	}
	got := projfile.Overlay(domain.Project{RepoPath: repo})
	if len(got.RunCommands) != 2 || got.RunCommands[1].Name != "test" {
		t.Fatalf("file runs: %+v", got.RunCommands)
	}
	if got.PinnedRun != "" {
		t.Errorf("pin should not be in toml: %q", got.PinnedRun)
	}
	fromDB, _ := s.Project("app")
	if fromDB.PinnedRun != "test" || len(fromDB.RunCommands) != 2 {
		t.Fatalf("read: pin=%q runs=%+v", fromDB.PinnedRun, fromDB.RunCommands)
	}

	p = fromDB
	p.RunCommands = []domain.RunCommand{{Name: "dev", Cmd: "wails3 task dev"}}
	p.PinnedRun = "test"
	cleared, err := s.UpdateProjectRunConfig(p)
	if err != nil {
		t.Fatalf("clear pin: %v", err)
	}
	if cleared.PinnedRun != "" {
		t.Errorf("stale pin should drop: %q", cleared.PinnedRun)
	}
}

func TestProjectTomlDoesNotRewriteHandFileOnOpenToggle(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	hand := "# keep comments\n[[run]]\nname = \"dev\"\ncmd = \"make dev\"\n"
	if err := os.WriteFile(filepath.Join(repo, projfile.RelPath), []byte(hand), 0o644); err != nil {
		t.Fatal(err)
	}
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo, Open: true})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if domain.VerifyDevCmd(p) != "make dev" {
		t.Fatalf("should seed from file: %q", domain.VerifyDevCmd(p))
	}
	p.Open = false
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("Update: %v", err)
	}
	got, err := os.ReadFile(filepath.Join(repo, projfile.RelPath))
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != hand {
		t.Fatalf("sidebar toggle rewrote file:\n%s", got)
	}
}

func TestUpdateProjectOpenDoesNotRevertPulledToml(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	old := "# keep me\n[[run]]\nname = \"dev\"\ncmd = \"old\"\n"
	if err := os.WriteFile(filepath.Join(repo, projfile.RelPath), []byte(old), 0o644); err != nil {
		t.Fatal(err)
	}
	s := openTemp(t, dir)
	p, err := s.AddProject(domain.Project{ID: "app", Name: "app", RepoPath: repo, Open: true})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pulled := "# pulled\n[[run]]\nname = \"dev\"\ncmd = \"new\"\n[[run]]\nname = \"test\"\ncmd = \"go test\"\n"
	if err := os.WriteFile(filepath.Join(repo, projfile.RelPath), []byte(pulled), 0o644); err != nil {
		t.Fatal(err)
	}
	p.Open = false
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("Update: %v", err)
	}
	got, err := os.ReadFile(filepath.Join(repo, projfile.RelPath))
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != pulled {
		t.Fatalf("stale sidebar toggle reverted pulled toml:\n%s", got)
	}
}

func TestAddProjectWriteProjectFileOverwritesExisting(t *testing.T) {
	dir := t.TempDir()
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	hand := "# team\n[[run]]\nname = \"dev\"\ncmd = \"make dev\"\n[[run]]\nname = \"test\"\ncmd = \"go test\"\n"
	if err := os.WriteFile(filepath.Join(repo, projfile.RelPath), []byte(hand), 0o644); err != nil {
		t.Fatal(err)
	}
	s := openTemp(t, dir)
	_, err := s.AddProject(domain.Project{
		ID: "app", Name: "app", RepoPath: repo,
		RunCommands:      []domain.RunCommand{{Name: "dev", Cmd: "from-form"}},
		WriteProjectFile: true,
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	over := projfile.Overlay(domain.Project{RepoPath: repo})
	if len(over.RunCommands) != 1 || over.RunCommands[0].Cmd != "from-form" {
		t.Fatalf("Overlay RunCommands = %+v, want overwritten from-form run", over.RunCommands)
	}
}

// TestProjectOpenPersists covers the sidebar expand/collapse flag surviving a
// store reopen (UI toggles write Open via UpdateProjectRunConfig).
func TestProjectOpenAndHiddenPersist(t *testing.T) {
	dir := t.TempDir()

	s1 := openTemp(t, dir)
	p, ok := s1.Project("storefront")
	if !ok || !p.Open {
		t.Fatalf("seeded storefront should be open, got ok=%v open=%v", ok, p.Open)
	}
	p.Open = false
	p.Hidden = true
	if _, err := s1.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	s1.Close()

	s2 := openTemp(t, dir)
	got, ok := s2.Project("storefront")
	if !ok {
		t.Fatal("storefront missing after reopen")
	}
	if got.Open {
		t.Errorf("open=false not persisted across reopen")
	}
	if !got.Hidden {
		t.Errorf("hidden=true not persisted across reopen")
	}
}

func TestProjectGroupsCRUDPersists(t *testing.T) {
	dir := t.TempDir()

	s1 := openTemp(t, dir)
	g, err := s1.AddProjectGroup(domain.ProjectGroup{ID: "work", Name: "Work", Open: true})
	if err != nil {
		t.Fatalf("AddProjectGroup: %v", err)
	}
	if g.ID != "work" || g.Name != "Work" || !g.Open {
		t.Fatalf("unexpected group: %+v", g)
	}
	if g.CreatedAt == "" || g.UpdatedAt == "" {
		t.Fatalf("expected timestamps on create: %+v", g)
	}
	createdAt := g.CreatedAt
	p, ok := s1.Project("storefront")
	if !ok {
		t.Fatal("storefront missing")
	}
	p.GroupID = "work"
	if _, err := s1.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("assign group: %v", err)
	}
	g.Open = false
	g.Name = "Client work"
	updated, err := s1.UpdateProjectGroup(g)
	if err != nil {
		t.Fatalf("UpdateProjectGroup: %v", err)
	}
	if updated.CreatedAt != createdAt {
		t.Fatalf("created_at changed on update: got %q want %q", updated.CreatedAt, createdAt)
	}
	if updated.UpdatedAt == "" {
		t.Fatalf("updated_at empty after update: %+v", updated)
	}
	s1.Close()

	s2 := openTemp(t, dir)
	groups := s2.ProjectGroups()
	if len(groups) != 1 || groups[0].ID != "work" || groups[0].Name != "Client work" || groups[0].Open {
		t.Fatalf("reloaded groups: %+v", groups)
	}
	if groups[0].CreatedAt != createdAt || groups[0].UpdatedAt == "" {
		t.Fatalf("timestamps not persisted: %+v", groups[0])
	}
	got, _ := s2.Project("storefront")
	if got.GroupID != "work" {
		t.Fatalf("group_id not persisted: %q", got.GroupID)
	}
}

func TestDeleteProjectGroupUngroupsMembers(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	if _, err := s.AddProjectGroup(domain.ProjectGroup{ID: "tmp", Name: "Tmp", Open: true}); err != nil {
		t.Fatalf("AddProjectGroup: %v", err)
	}
	p, _ := s.Project("storefront")
	p.GroupID = "tmp"
	if _, err := s.UpdateProjectRunConfig(p); err != nil {
		t.Fatalf("assign: %v", err)
	}
	if err := s.DeleteProjectGroup("tmp"); err != nil {
		t.Fatalf("DeleteProjectGroup: %v", err)
	}
	if len(s.ProjectGroups()) != 0 {
		t.Fatalf("group still present: %+v", s.ProjectGroups())
	}
	got, _ := s.Project("storefront")
	if got.GroupID != "" {
		t.Fatalf("expected ungrouped, got group_id=%q", got.GroupID)
	}
}

func TestReorderProjectGroupsPersists(t *testing.T) {
	dir := t.TempDir()
	s1 := openTemp(t, dir)
	for _, id := range []string{"a", "b", "c"} {
		if _, err := s1.AddProjectGroup(domain.ProjectGroup{ID: id, Name: id, Open: true}); err != nil {
			t.Fatalf("AddProjectGroup %s: %v", id, err)
		}
	}
	order := []string{"c", "a", "b"}
	if err := s1.ReorderProjectGroups(order); err != nil {
		t.Fatalf("ReorderProjectGroups: %v", err)
	}
	s1.Close()

	s2 := openTemp(t, dir)
	got := make([]string, len(s2.ProjectGroups()))
	for i, g := range s2.ProjectGroups() {
		got[i] = g.ID
	}
	if !equalStrings(got, order) {
		t.Fatalf("after reopen: got %v want %v", got, order)
	}
}

func TestSetPolicyTogglePersists(t *testing.T) {
	dir := t.TempDir()

	s1 := openTemp(t, dir)
	pol := s1.Policy()
	pol.Autonomy = domain.AutonomyAggressive
	s1.SetPolicy(pol)
	s1.Close()

	s2 := openTemp(t, dir)
	got := s2.Policy()
	if got.Autonomy != domain.AutonomyAggressive {
		t.Errorf("autonomy not persisted: %q", got.Autonomy)
	}
}

func TestMarkAllInboxReadPersists(t *testing.T) {
	dir := t.TempDir()

	s1 := openTemp(t, dir)
	if s1.UnreadCount() == 0 {
		t.Fatalf("expected seeded unread inbox items")
	}
	s1.MarkAllInboxRead()
	if s1.UnreadCount() != 0 {
		t.Fatalf("MarkAllInboxRead did not clear unread in memory")
	}
	s1.Close()

	s2 := openTemp(t, dir)
	if got := s2.UnreadCount(); got != 0 {
		t.Errorf("unread not persisted as read after reopen: got %d", got)
	}
}

func TestAnswerEscalationPersists(t *testing.T) {
	dir := t.TempDir()

	s1 := openTemp(t, dir)
	if _, ok := s1.AnswerEscalation("LOOP-2218", "Token bucket with refill", ""); !ok {
		t.Fatalf("AnswerEscalation failed")
	}
	s1.Close()

	s2 := openTemp(t, dir)
	e, ok := s2.Escalation("LOOP-2218")
	if !ok || e.Status != "answered" || e.Answer != "Token bucket with refill" || e.Choice != "Token bucket with refill" {
		t.Errorf("escalation answer not persisted: ok=%v %+v", ok, e)
	}
}

func TestChatEscalationsDoNotClobber(t *testing.T) {
	s := NewInMemory()
	s.AddEscalation(domain.Escalation{ID: "esc-a", WorkspaceID: "W1", AgentCode: "PA", Status: "open", Question: "A"})
	s.AddEscalation(domain.Escalation{ID: "esc-b", WorkspaceID: "W1", AgentCode: "PA2", Status: "open", Question: "B"})
	if _, ok := s.AnswerEscalationFor("W1", "PA2", "keep", ""); !ok {
		t.Fatal("AnswerEscalationFor(PA2) = false, want true")
	}
	got, ok := s.AnswerEscalationFor("W1", "PA", "stop", "")
	if !ok {
		t.Fatal("PA row was overwritten by the sibling answer")
	}
	if got.Question != "A" || got.Answer != "stop" {
		t.Fatalf("AnswerEscalationFor(PA) = %+v, want question A answered stop", got)
	}
}

func TestEscalationPrefersOpenChatRowOverAnsweredWorkspace(t *testing.T) {
	s := NewInMemory()
	s.AddEscalation(domain.Escalation{ID: "esc-old", WorkspaceID: "W1", Status: "answered", Question: "stale", Answer: "done"})
	s.AddEscalation(domain.Escalation{ID: "esc-live", WorkspaceID: "W1", AgentCode: "PA", Status: "open", Question: "live"})
	got, ok := s.Escalation("W1")
	if !ok || got.Question != "live" {
		t.Fatalf("Escalation() = ok=%v %+v, want open chat-keyed live", ok, got)
	}
	if !s.HasOpenEscalation("W1") {
		t.Fatal("HasOpenEscalation(W1) = false, want true")
	}
	ans, ok := s.AnswerEscalation("W1", "yes", "")
	if !ok || ans.Question != "live" || ans.Answer != "yes" {
		t.Fatalf("AnswerEscalation = ok=%v %+v, want live row answered yes", ok, ans)
	}
	if s.HasOpenEscalation("W1") {
		t.Fatal("HasOpenEscalation(W1) = true after answering the only open row")
	}
}

// TestFindingsPersistAndResolve covers the new M3 findings table (PRD §8.3, §14):
// AddFinding records reviewer findings, OpenFindingsCount reflects open ones,
// ResolveFindings flips open→resolved, and the state survives a reopen from disk.
func TestFindingsPersistAndResolve(t *testing.T) {
	dir := t.TempDir()

	s1 := openTemp(t, dir)
	for i := 0; i < 3; i++ {
		s1.AddFinding(domain.Finding{
			ID: "F-1-" + string(rune('a'+i)), WorkspaceID: "L1", Iteration: 1,
			File: "main.go", Line: i + 1, Severity: domain.SevHigh, Message: "bug", Status: "open",
		})
	}
	if got := s1.OpenFindingsCount("L1"); got != 3 {
		t.Fatalf("open findings: got %d, want 3", got)
	}
	s1.ResolveFindings("L1")
	if got := s1.OpenFindingsCount("L1"); got != 0 {
		t.Fatalf("open findings after resolve: got %d, want 0", got)
	}
	// A new finding for iteration 2 stays open alongside the resolved ones.
	s1.AddFinding(domain.Finding{
		ID: "F-2-a", WorkspaceID: "L1", Iteration: 2,
		File: "x.go", Line: 9, Severity: domain.SevLow, Message: "smell", Status: "open",
	})
	if got := s1.OpenFindingsCount("L1"); got != 1 {
		t.Fatalf("open findings after new: got %d, want 1", got)
	}
	s1.Close()

	// Reopen from disk: all 4 findings survive with their statuses.
	s2 := openTemp(t, dir)
	all := s2.Findings("L1")
	if len(all) != 4 {
		t.Fatalf("reloaded findings: got %d, want 4", len(all))
	}
	if got := s2.OpenFindingsCount("L1"); got != 1 {
		t.Errorf("reloaded open findings: got %d, want 1", got)
	}
	resolved := 0
	for _, f := range all {
		if f.Status == "resolved" {
			resolved++
		}
	}
	if resolved != 3 {
		t.Errorf("reloaded resolved findings: got %d, want 3", resolved)
	}
}

// TestSetFindingStatus covers human Ignore / Mark invalid / Reopen, and that
// ResolveFindings leaves ignored/invalid alone.
func TestSetFindingStatus(t *testing.T) {
	s := openTemp(t, t.TempDir())
	s.AddFinding(domain.Finding{
		ID: "F-a", WorkspaceID: "L1", Iteration: 1,
		File: "a.go", Line: 1, Severity: domain.SevHigh, Message: "bug", Status: string(domain.FindingOpen),
	})
	s.AddFinding(domain.Finding{
		ID: "F-b", WorkspaceID: "L1", Iteration: 1,
		File: "b.go", Line: 2, Severity: domain.SevLow, Message: "nit", Status: string(domain.FindingOpen),
	})

	got, err := s.SetFindingStatus("L1", "F-a", string(domain.FindingIgnored))
	if err != nil {
		t.Fatalf("ignore: %v", err)
	}
	if got.Status != string(domain.FindingIgnored) {
		t.Fatalf("ignore status: got %q", got.Status)
	}
	if _, err := s.SetFindingStatus("L1", "F-b", string(domain.FindingInvalid)); err != nil {
		t.Fatalf("invalid: %v", err)
	}
	if got := s.OpenFindingsCount("L1"); got != 0 {
		t.Fatalf("open after human dismiss: got %d, want 0", got)
	}

	// Engine resolve must not clobber human judgments.
	s.AddFinding(domain.Finding{
		ID: "F-c", WorkspaceID: "L1", Iteration: 2,
		File: "c.go", Line: 3, Severity: domain.SevMedium, Message: "x", Status: string(domain.FindingOpen),
	})
	s.ResolveFindings("L1")
	byID := map[string]string{}
	for _, f := range s.Findings("L1") {
		byID[f.ID] = f.Status
	}
	if byID["F-a"] != string(domain.FindingIgnored) || byID["F-b"] != string(domain.FindingInvalid) {
		t.Fatalf("human statuses clobbered: %v", byID)
	}
	if byID["F-c"] != string(domain.FindingResolved) {
		t.Fatalf("open should resolve: got %q", byID["F-c"])
	}

	reopened, err := s.SetFindingStatus("L1", "F-a", string(domain.FindingOpen))
	if err != nil || reopened.Status != string(domain.FindingOpen) {
		t.Fatalf("reopen: got %+v err=%v", reopened, err)
	}
	if _, err := s.SetFindingStatus("L1", "F-a", "bogus"); err == nil {
		t.Fatal("expected error for bogus status")
	}
	if _, err := s.SetFindingStatus("L1", "missing", string(domain.FindingIgnored)); err == nil {
		t.Fatal("expected error for missing id")
	}
}

func TestUpdateFindingStatusEngineResolvesOpenOnly(t *testing.T) {
	s := openTemp(t, t.TempDir())
	s.AddFinding(domain.Finding{
		ID: "F-open", WorkspaceID: "L1", Iteration: 1,
		File: "a.go", Line: 1, Severity: domain.SevHigh, Message: "bug", Status: string(domain.FindingOpen),
	})
	s.AddFinding(domain.Finding{
		ID: "F-ig", WorkspaceID: "L1", Iteration: 1,
		File: "b.go", Line: 2, Severity: domain.SevLow, Message: "nit", Status: string(domain.FindingIgnored),
	})
	s.AddFinding(domain.Finding{
		ID: "F-inv", WorkspaceID: "L1", Iteration: 1,
		File: "c.go", Line: 3, Severity: domain.SevMedium, Message: "x", Status: string(domain.FindingInvalid),
	})

	got, err := s.UpdateFindingStatus("L1", "F-open", string(domain.FindingResolved), "", domain.FindingActorEngine)
	if err != nil || got.Status != string(domain.FindingResolved) || got.UpdatedBy != domain.FindingActorEngine {
		t.Fatalf("UpdateFindingStatus(open→resolved) = %+v err=%v, want resolved by engine", got, err)
	}
	got, err = s.UpdateFindingStatus("L1", "F-ig", string(domain.FindingResolved), "", domain.FindingActorEngine)
	if err != nil || got.Status != string(domain.FindingIgnored) {
		t.Fatalf("UpdateFindingStatus(ignored→resolved) = %+v err=%v, want ignored left intact", got, err)
	}
	got, err = s.UpdateFindingStatus("L1", "F-inv", string(domain.FindingResolved), "", domain.FindingActorEngine)
	if err != nil || got.Status != string(domain.FindingInvalid) {
		t.Fatalf("UpdateFindingStatus(invalid→resolved) = %+v err=%v, want invalid left intact", got, err)
	}
	if _, err := s.UpdateFindingStatus("L1", "F-open", string(domain.FindingIgnored), "", domain.FindingActorEngine); err == nil {
		t.Fatal("UpdateFindingStatus(engine ignore) error = nil, want error")
	}
}

func TestAddFindingNote(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	s.AddFinding(domain.Finding{
		ID: "F-n", WorkspaceID: "L1", Iteration: 1,
		File: "a.go", Line: 1, EndLine: 4, Severity: domain.SevLow, Message: "bug",
		Summary: "short", Status: string(domain.FindingOpen),
	})

	got, err := s.AddFindingNote("L1", "F-n", "  check the timeout path  ", domain.FindingActorHuman)
	if err != nil {
		t.Fatalf("add note: %v", err)
	}
	if len(got.Notes) != 1 {
		t.Fatalf("notes: got %d, want 1", len(got.Notes))
	}
	n := got.Notes[0]
	if n.ID == "" || n.Body != "check the timeout path" || n.Author != domain.FindingActorHuman || n.CreatedAt == "" {
		t.Fatalf("note fields: %+v", n)
	}
	if _, err := s.AddFindingNote("L1", "F-n", "second", ""); err != nil {
		t.Fatalf("second note: %v", err)
	}
	all := s.Findings("L1")
	if len(all) != 1 || len(all[0].Notes) != 2 {
		t.Fatalf("persisted notes: %+v", all)
	}
	if _, err := s.AddFindingNote("L1", "F-n", "   ", domain.FindingActorHuman); err == nil {
		t.Fatal("expected empty body error")
	}
	if _, err := s.AddFindingNote("L1", "missing", "x", domain.FindingActorHuman); err == nil {
		t.Fatal("expected missing finding error")
	}
	s.Close()

	s2, err := open(filepath.Join(dir, "recur.db"))
	if err != nil {
		t.Fatalf("reopen: %v", err)
	}
	defer s2.Close()
	reloaded := s2.Findings("L1")
	if len(reloaded) != 1 {
		t.Fatalf("reloaded findings: got %d, want 1", len(reloaded))
	}
	f := reloaded[0]
	if f.File != "a.go" || f.Line != 1 || f.EndLine != 4 || f.Message != "bug" || f.Summary != "short" {
		t.Errorf("reloaded columns: %+v", f)
	}
	if len(f.Notes) != 2 || f.Notes[0].Body != "check the timeout path" || f.Notes[1].Body != "second" {
		t.Errorf("reloaded notes: %+v", f.Notes)
	}
	var docCol int
	if err := s2.db.sql.QueryRow(`SELECT COUNT(*) FROM pragma_table_info('findings') WHERE name = 'doc_json'`).Scan(&docCol); err != nil {
		t.Fatalf("pragma: %v", err)
	}
	if docCol != 0 {
		t.Error("findings.doc_json should be gone")
	}
}

func TestAddFindingAssignsUUIDAndShortID(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	first := s.AddFinding(domain.Finding{
		WorkspaceID: "L1", Iteration: 1,
		File: "a.go", Line: 1, Severity: domain.SevHigh, Message: "one",
		Status: string(domain.FindingOpen),
	})
	if parsed, err := uuid.Parse(first.ID); err != nil {
		t.Fatalf("AddFinding() id = %q, want a UUID: %v", first.ID, err)
	} else if parsed.Version() != 7 {
		t.Fatalf("AddFinding() id version = %d, want 7", parsed.Version())
	}
	if !domain.ValidID(first.ShortID, domain.TypeFinding) {
		t.Fatalf("AddFinding() short_id = %q, want a valid find_ id", first.ShortID)
	}
	second := s.AddFinding(domain.Finding{
		WorkspaceID: "L1", File: "b.go", Message: "two", Status: "open",
	})
	if first.ShortID == second.ShortID {
		t.Fatalf("AddFinding() reused short_id %q in the same workspace", first.ShortID)
	}
	otherWS := s.AddFinding(domain.Finding{
		WorkspaceID: "L2", ShortID: first.ShortID, File: "c.go", Message: "three", Status: "open",
	})
	if otherWS.ShortID != first.ShortID {
		t.Fatalf("AddFinding() short_id = %q, want %q (unique per workspace)", otherWS.ShortID, first.ShortID)
	}
	s.Close()

	s2, err := open(filepath.Join(dir, "recur.db"))
	if err != nil {
		t.Fatalf("reopen: %v", err)
	}
	defer s2.Close()
	reloaded := s2.Findings("L1")
	if len(reloaded) != 2 {
		t.Fatalf("reloaded findings: got %d, want 2", len(reloaded))
	}
	if reloaded[0].ID != first.ID || reloaded[0].ShortID != first.ShortID {
		t.Errorf("reloaded[0] = %+v, want id=%s shortId=%s", reloaded[0], first.ID, first.ShortID)
	}
	if _, err := s2.SetFindingStatus("L1", first.ShortID, string(domain.FindingIgnored)); err != nil {
		t.Fatalf("SetFindingStatus by short_id: %v", err)
	}
	if _, err := s2.SetFindingStatus("L1", second.ID, string(domain.FindingIgnored)); err != nil {
		t.Fatalf("SetFindingStatus by uuid: %v", err)
	}
}

func TestAddFindingRetriesShortIDCollision(t *testing.T) {
	s := openTemp(t, t.TempDir())
	first := s.AddFinding(domain.Finding{
		WorkspaceID: "L1", ShortID: "find_aaaaaa", File: "a.go", Message: "one", Status: "open",
	})
	if first.ShortID != "find_aaaaaa" {
		t.Fatalf("AddFinding() short_id = %q, want find_aaaaaa", first.ShortID)
	}
	second := s.AddFinding(domain.Finding{
		WorkspaceID: "L1", ShortID: "find_aaaaaa", File: "b.go", Message: "two", Status: "open",
	})
	if second.ShortID == first.ShortID {
		t.Fatalf("AddFinding() did not retry colliding short_id %q", first.ShortID)
	}
	if !domain.ValidID(second.ShortID, domain.TypeFinding) {
		t.Fatalf("AddFinding() retry short_id = %q, want a valid find_ id", second.ShortID)
	}
}

func TestAddFindingRetriesPersistedShortIDCollision(t *testing.T) {
	s := openTemp(t, t.TempDir())
	if _, err := s.db.sql.Exec(
		`INSERT INTO findings (id, short_id, workspace_id, file, message, status, sort) VALUES (?,?,?,?,?,?,?)`,
		"id-disk", "find_aaaaaa", "L1", "a.go", "one", "open", 0,
	); err != nil {
		t.Fatalf("insert disk row: %v", err)
	}
	got := s.AddFinding(domain.Finding{
		WorkspaceID: "L1", ShortID: "find_aaaaaa", File: "b.go", Message: "two", Status: "open",
	})
	if got.ShortID == "find_aaaaaa" {
		t.Fatalf("AddFinding() did not retry disk-colliding short_id %q", got.ShortID)
	}
	if !domain.ValidID(got.ShortID, domain.TypeFinding) {
		t.Fatalf("AddFinding() retry short_id = %q, want a valid find_ id", got.ShortID)
	}
	found := false
	for _, f := range s.Findings("L1") {
		if f.ID == got.ID {
			found = true
			break
		}
	}
	if !found {
		t.Fatal("AddFinding() did not cache after unique retry")
	}
}

func TestAddFindingDoesNotCachePersistFailure(t *testing.T) {
	s := openTemp(t, t.TempDir())
	if err := s.db.sql.Close(); err != nil {
		t.Fatalf("close db: %v", err)
	}
	s.AddFinding(domain.Finding{
		WorkspaceID: "WS-persist-fail", File: "a.go", Message: "x", Status: "open",
	})
	if got := s.Findings("WS-persist-fail"); len(got) != 0 {
		t.Fatalf("cached unpersisted finding: %+v", got)
	}
}

func TestIsUniqueConstraint(t *testing.T) {
	s := openTemp(t, t.TempDir())
	_, notNull := s.db.sql.Exec(
		`INSERT INTO findings (id, short_id, workspace_id, file, message, status, sort) VALUES (?,?,?,?,?,?,?)`,
		"x", "find_bbbbbb", "W", nil, "m", "open", 0,
	)
	if notNull == nil {
		t.Fatal("NULL file: want NOT NULL error")
	}
	if isUniqueConstraint(notNull) {
		t.Errorf("NOT NULL error matched isUniqueConstraint: %v", notNull)
	}

	if _, err := s.db.sql.Exec(
		`INSERT INTO findings (id, short_id, workspace_id, file, message, status, sort) VALUES (?,?,?,?,?,?,?)`,
		"a", "find_cccccc", "W", "a.go", "one", "open", 0,
	); err != nil {
		t.Fatalf("insert first: %v", err)
	}
	_, unique := s.db.sql.Exec(
		`INSERT INTO findings (id, short_id, workspace_id, file, message, status, sort) VALUES (?,?,?,?,?,?,?)`,
		"b", "find_cccccc", "W", "b.go", "two", "open", 1,
	)
	if unique == nil {
		t.Fatal("duplicate short_id: want unique error")
	}
	if !isUniqueConstraint(unique) {
		t.Errorf("unique error did not match isUniqueConstraint: %v", unique)
	}
}

func TestFindingIdentityBackfill(t *testing.T) {
	dir := t.TempDir()
	s1 := openTemp(t, dir)
	if _, err := s1.db.sql.Exec(
		`INSERT INTO findings (id, short_id, workspace_id, file, message, status, sort) VALUES (?,?,?,?,?,?,?)`,
		"F-legacy-123", "", "WS1", "a.go", "bug", "open", 0,
	); err != nil {
		t.Fatalf("insert legacy: %v", err)
	}
	s1.Close()

	s2, err := open(filepath.Join(dir, "recur.db"))
	if err != nil {
		t.Fatalf("reopen: %v", err)
	}
	defer s2.Close()
	got := s2.Findings("WS1")
	if len(got) != 1 {
		t.Fatalf("backfilled findings: got %d, want 1", len(got))
	}
	if got[0].ID != "F-legacy-123" {
		t.Errorf("backfill id = %q, want F-legacy-123 (transcript refs)", got[0].ID)
	}
	if !domain.ValidID(got[0].ShortID, domain.TypeFinding) {
		t.Errorf("backfill short_id = %q, want a valid find_ id", got[0].ShortID)
	}
}

func TestFindingsColumnsMigrateFromDocJSON(t *testing.T) {
	dir := t.TempDir()
	s1 := openTemp(t, dir)
	path := filepath.Join(dir, "recur.db")
	s1.Close()

	raw, err := openDB(path)
	if err != nil {
		t.Fatalf("reopen raw: %v", err)
	}
	stmts := []string{
		`DROP TABLE finding_notes`,
		`DROP TABLE findings`,
		`CREATE TABLE findings (
			id TEXT PRIMARY KEY,
			workspace_id TEXT NOT NULL,
			doc_json TEXT NOT NULL,
			sort INTEGER NOT NULL DEFAULT 0
		)`,
		`INSERT INTO findings (id, workspace_id, doc_json, sort) VALUES (
			'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
			'WS1',
			'{"id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","shortId":"find_abcxyz","workspaceId":"WS1","iteration":2,"file":"x.go","line":9,"endLine":12,"severity":"high","message":"nil","summary":"n","status":"open","notes":[{"id":"note-1","author":"human","body":"look","createdAt":"2026-01-01T00:00:01Z"}]}',
			0
		)`,
		`DELETE FROM __migrations WHERE version = 18`,
	}
	for _, q := range stmts {
		if _, err := raw.sql.Exec(q); err != nil {
			raw.close()
			t.Fatalf("simulate pre-columns (%q): %v", q, err)
		}
	}
	raw.close()

	s2, err := open(path)
	if err != nil {
		t.Fatalf("reopen after columns migration: %v", err)
	}
	defer s2.Close()
	got := s2.Findings("WS1")
	if len(got) != 1 {
		t.Fatalf("migrated findings: got %d, want 1", len(got))
	}
	f := got[0]
	if f.ID != "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee" || f.ShortID != "find_abcxyz" {
		t.Errorf("migrated ids: %+v", f)
	}
	if f.Iteration != 2 || f.File != "x.go" || f.Line != 9 || f.EndLine != 12 {
		t.Errorf("migrated location: %+v", f)
	}
	if string(f.Severity) != "high" || f.Message != "nil" || f.Summary != "n" || f.Status != "open" {
		t.Errorf("migrated body: %+v", f)
	}
	if len(f.Notes) != 1 || f.Notes[0].ID != "note-1" || f.Notes[0].Body != "look" {
		t.Errorf("migrated notes: %+v", f.Notes)
	}
}

func TestDeleteFindingNote(t *testing.T) {
	s := openTemp(t, t.TempDir())
	s.AddFinding(domain.Finding{
		ID: "F-d", WorkspaceID: "L1", Iteration: 1,
		File: "a.go", Line: 1, Severity: domain.SevLow, Message: "bug", Status: string(domain.FindingOpen),
	})
	first, err := s.AddFindingNote("L1", "F-d", "keep", domain.FindingActorHuman)
	if err != nil {
		t.Fatalf("add: %v", err)
	}
	second, err := s.AddFindingNote("L1", "F-d", "drop", domain.FindingActorHuman)
	if err != nil {
		t.Fatalf("add second: %v", err)
	}
	dropID := second.Notes[1].ID
	got, err := s.DeleteFindingNote("L1", "F-d", dropID)
	if err != nil {
		t.Fatalf("delete: %v", err)
	}
	if len(got.Notes) != 1 || got.Notes[0].ID != first.Notes[0].ID || got.Notes[0].Body != "keep" {
		t.Fatalf("after delete: %+v", got.Notes)
	}
	if _, err := s.DeleteFindingNote("L1", "F-d", dropID); err == nil {
		t.Fatal("expected missing note error")
	}
	if _, err := s.DeleteFindingNote("L1", "F-d", ""); err == nil {
		t.Fatal("expected empty note id error")
	}
}

// TestFreshDBIsEmptyByDefault: a brand-new DB gets the default policy/settings
// (+ a zeroed status bar) but NO demo data — a clean first-run app.
func TestFreshDBIsEmptyByDefault(t *testing.T) {
	dir := t.TempDir()
	s, err := open(filepath.Join(dir, "recur.db"))
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	defer s.Close()
	if got := len(s.Projects()); got != 0 {
		t.Errorf("fresh app should have 0 projects, got %d", got)
	}
	if got := len(s.Workspaces()); got != 0 {
		t.Errorf("fresh app should have 0 loops, got %d", got)
	}
	if len(s.Policy().Rails) != 0 {
		t.Errorf("fresh app should seed no rails (retired), got %d", len(s.Policy().Rails))
	}
	if s.Settings().ModelAgent == "" {
		t.Errorf("fresh app should have default settings")
	}
	if s.Settings().BypassPermissions {
		t.Errorf("fresh app should default BypassPermissions OFF — asking is the safe default")
	}
	if r := s.Resource(); r.LoopsRunning != 0 || r.ItersToday != 0 || r.NeedsYou != 0 {
		t.Errorf("fresh status bar should be zeroed, got %+v", r)
	}
}

func TestGitHubBranchRequirePRPersists(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	if len(s.Projects()) == 0 {
		t.Fatal("demo seed has no projects")
	}
	id := s.Projects()[0].ID
	s.SetGitHubBranchRequirePR(id, "acme/app", "main", true)
	s.SetGitHubBranchRequirePR(id, "acme/app", "develop", false)
	s.SetGitHubBranchRequirePR(id, "acme/parent", "main", false)
	got, at, ok := s.GitHubBranchRequirePR(id, "acme/app", "main")
	if !ok || !got || at.IsZero() {
		t.Fatalf("in-memory GitHubBranchRequirePR = require %v at %v ok=%v", got, at, ok)
	}
	s.Close()

	s2 := openTemp(t, dir)
	got, at, ok = s2.GitHubBranchRequirePR(id, "acme/app", "main")
	if !ok || !got {
		t.Errorf("reopened GitHubBranchRequirePR acme/app main = require %v ok=%v, want true true", got, ok)
	}
	if at.IsZero() {
		t.Error("reopened checked_at is zero")
	}
	got, _, ok = s2.GitHubBranchRequirePR(id, "acme/app", "develop")
	if !ok || got {
		t.Errorf("reopened GitHubBranchRequirePR acme/app develop = require %v ok=%v, want false true", got, ok)
	}
	got, _, ok = s2.GitHubBranchRequirePR(id, "acme/parent", "main")
	if !ok || got {
		t.Errorf("reopened GitHubBranchRequirePR acme/parent main = require %v ok=%v, want false true", got, ok)
	}
}

// TestClearAllWipesDataKeepsPolicy: ClearAll empties the demo data + zeroes the
// status bar but keeps policy/settings, and the demo does NOT come back on reopen.
func TestClearAllWipesDataKeepsPolicy(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir) // demo seeded via helper
	if len(s.Projects()) == 0 {
		t.Fatal("precondition: demo projects expected")
	}
	if err := s.ClearAll(); err != nil {
		t.Fatalf("ClearAll: %v", err)
	}
	for _, c := range []struct {
		name string
		n    int
	}{{"projects", len(s.Projects())}, {"workspaces", len(s.Workspaces())}, {"prs", len(s.PRs())}, {"inbox", len(s.Inbox())}} {
		if c.n != 0 {
			t.Errorf("after ClearAll %s: got %d, want 0", c.name, c.n)
		}
	}
	if s.Policy().Autonomy != domain.AutonomyBalanced {
		t.Errorf("ClearAll must keep the policy autonomy, got %q", s.Policy().Autonomy)
	}
	s.Close()
	// Reopen: still empty (DB stays initialized; demo doesn't resurrect), policy kept.
	s2 := openTemp(t, dir)
	if got := len(s2.Projects()); got != 0 {
		t.Errorf("demo came back after ClearAll+reopen: %d projects", got)
	}
	if s2.Policy().Autonomy != domain.AutonomyBalanced {
		t.Errorf("policy lost across reopen after ClearAll, autonomy=%q", s2.Policy().Autonomy)
	}
}

// TestDeleteProjectCascades: deleting a project removes its loops + PRs and persists,
// while OTHER projects/loops survive.
func TestDeleteProjectCascades(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir) // demo: storefront (loops + PR #482) + payments (LOOP-2218) + marketing
	deleted, err := s.DeleteProject("storefront")
	if err != nil {
		t.Fatalf("DeleteProject: %v", err)
	}
	if len(deleted) == 0 {
		t.Error("expected the deleted loop ids")
	}
	if _, ok := s.Project("storefront"); ok {
		t.Error("storefront should be gone")
	}
	if _, ok := s.Project("payments"); !ok {
		t.Error("payments should remain")
	}
	if _, ok := s.Workspace("LOOP-2231"); ok {
		t.Error("storefront loop LOOP-2231 should be gone")
	}
	if _, ok := s.Workspace("LOOP-2218"); !ok {
		t.Error("payments loop LOOP-2218 should remain")
	}
	if _, ok := s.PR("482"); ok {
		t.Error("storefront PR #482 should be gone")
	}
	s.Close()
	s2 := openTemp(t, dir) // persisted across reopen
	if _, ok := s2.Project("storefront"); ok {
		t.Error("storefront resurrected after reopen")
	}
	if _, ok := s2.Workspace("LOOP-2218"); !ok {
		t.Error("payments loop lost after reopen")
	}
}

// TestDeleteWorkspaceCascades mirrors TestDeleteProjectCascades but for the per-loop
// DeleteWorkspace (Phase 3.4): removing LOOP-2231 takes its PR #482 with it, leaves its
// project (storefront) and sibling loops/PRs untouched, and the deletion survives a
// reopen.
func TestDeleteWorkspaceCascades(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir) // demo: storefront (LOOP-2231 + PR #482), payments (LOOP-2218), …

	if err := s.DeleteWorkspace("LOOP-2231"); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	if _, ok := s.Workspace("LOOP-2231"); ok {
		t.Error("LOOP-2231 should be gone")
	}
	if _, ok := s.PR("482"); ok {
		t.Error("PR #482 (owned by LOOP-2231) should be gone")
	}
	if _, ok := s.Project("storefront"); !ok {
		t.Error("storefront project should remain")
	}
	if _, ok := s.Workspace("LOOP-2218"); !ok {
		t.Error("unrelated loop LOOP-2218 should remain")
	}
	for _, it := range s.Inbox() {
		if it.Target == "loop:LOOP-2231" || it.Target == "pr:482" {
			t.Errorf("inbox row %q should have been cascaded away", it.ID)
		}
	}

	s.Close()
	s2 := openTemp(t, dir) // persisted across reopen
	if _, ok := s2.Workspace("LOOP-2231"); ok {
		t.Error("LOOP-2231 resurrected after reopen")
	}
	if _, ok := s2.PR("482"); ok {
		t.Error("PR #482 resurrected after reopen")
	}
	if _, ok := s2.Workspace("LOOP-2218"); !ok {
		t.Error("unrelated loop lost after reopen")
	}
}

// TestDeleteWorkspaceUnknownErrors asserts DeleteWorkspace on a bogus id returns an error
// rather than silently no-op'ing.
func TestDeleteWorkspaceUnknownErrors(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	if err := s.DeleteWorkspace("no-such-loop"); err == nil {
		t.Error("expected an error deleting an unknown loop")
	}
}

// PrependFeed persists incrementally (one INSERT, no rewrite of the loop's
// rows), so the ordering guarantee it relies on — newest first, preserved
// across a reload — needs explicit cover. Also exercises the interaction with
// PatchFeedByToolUseID, which DOES rewrite the rows and renumbers their sort.
func TestFeedPrependOrderSurvivesReload(t *testing.T) {
	dir := t.TempDir()
	s1 := openTemp(t, dir)

	const workspaceID = "LOOP-FEED-ORDER"
	// Seeded feed rows exist for other loops; this loop starts empty.
	if got := len(s1.Feed(workspaceID)); got != 0 {
		t.Fatalf("new loop feed should start empty, got %d", got)
	}
	for i := range 25 {
		s1.PrependFeed(workspaceID, domain.FeedItem{
			ID:        fmt.Sprintf("item-%02d", i),
			AgentCode: "CO",
			Name:      "Coder",
			Text:      fmt.Sprintf("step %d", i),
			ToolUseID: fmt.Sprintf("tu-%02d", i),
		})
	}

	want := make([]string, 0, 25)
	for i := 24; i >= 0; i-- {
		want = append(want, fmt.Sprintf("item-%02d", i))
	}
	assertFeedIDs := func(t *testing.T, s *Store, label string) {
		t.Helper()
		got := make([]string, 0, 25)
		for _, it := range s.Feed(workspaceID) {
			got = append(got, it.ID)
		}
		if len(got) != len(want) {
			t.Fatalf("%s: got %d items, want %d", label, len(got), len(want))
		}
		for i := range want {
			if got[i] != want[i] {
				t.Fatalf("%s: order mismatch at %d: got %q, want %q (%v)", label, i, got[i], want[i], got)
			}
		}
	}
	assertFeedIDs(t, s1, "in memory")

	s1.Close()
	s2 := openTemp(t, dir)
	assertFeedIDs(t, s2, "after reload")

	// A patch rewrites the rows with a fresh 0..N-1 numbering; prepends after it
	// must still land in front of everything, in memory and on disk.
	if got := s2.PatchFeedByToolUseID(workspaceID, "tu-10", func(it *domain.FeedItem) {
		it.Text = "patched"
	}); got == nil || got.Text != "patched" {
		t.Fatalf("PatchFeedByToolUseID = %+v, want patched item", got)
	}
	s2.PrependFeed(workspaceID, domain.FeedItem{ID: "item-25", AgentCode: "CO", Name: "Coder"})
	want = append([]string{"item-25"}, want...)
	assertFeedIDs(t, s2, "after patch + prepend")

	s2.Close()
	s3 := openTemp(t, dir)
	assertFeedIDs(t, s3, "after patch + prepend, reloaded")
	for _, it := range s3.Feed(workspaceID) {
		if it.ID == "item-10" && it.Text != "patched" {
			t.Fatalf("patched text did not persist: %+v", it)
		}
	}
}

func TestInboxTargetsWorkspaceIncludesChat(t *testing.T) {
	if !inboxTargetsWorkspace("chat:demo.ws:PA2", "demo.ws") {
		t.Fatal("chat:demo.ws:PA2 should target demo.ws")
	}
	if inboxTargetsWorkspace("chat:other:PA", "demo.ws") {
		t.Fatal("chat row for another workspace must not match")
	}
	if !inboxTargetsWorkspace("workspace:demo.ws", "demo.ws") {
		t.Fatal("workspace: target should still match")
	}

	s := NewInMemory()
	s.AddInboxItem(domain.InboxItem{ID: "ib-chat-demo.ws-PA2", Kind: "chat", Target: "chat:demo.ws:PA2", Unread: true})
	s.AddInboxItem(domain.InboxItem{ID: "ib-keep", Target: "workspace:other", Unread: true})
	if n := s.ClearInboxForWorkspace("demo.ws"); n != 1 {
		t.Fatalf("ClearInboxForWorkspace = %d, want 1", n)
	}
	if _, ok := inboxItem(s, "ib-chat-demo.ws-PA2"); ok {
		t.Fatal("chat inbox row should have been cleared")
	}
	if _, ok := inboxItem(s, "ib-keep"); !ok {
		t.Fatal("unrelated inbox row should remain")
	}
}

func TestRemoveChatInboxItems(t *testing.T) {
	s := NewInMemory()
	s.AddInboxItem(domain.InboxItem{ID: "ib-chat-ws-PA", Kind: "chat", Unread: true})
	s.AddInboxItem(domain.InboxItem{ID: "ib-keep", Kind: "pr", Unread: true})
	if n := s.RemoveChatInboxItems(); n != 1 {
		t.Fatalf("RemoveChatInboxItems() = %d, want 1", n)
	}
	if _, ok := inboxItem(s, "ib-chat-ws-PA"); ok {
		t.Fatal("chat row should have been removed")
	}
	if _, ok := inboxItem(s, "ib-keep"); !ok {
		t.Fatal("non-chat row should remain")
	}
	if n := s.RemoveChatInboxItems(); n != 0 {
		t.Fatalf("RemoveChatInboxItems() second call = %d, want 0", n)
	}
}

func inboxItem(s *Store, id string) (domain.InboxItem, bool) {
	for _, it := range s.Inbox() {
		if it.ID == id {
			return it, true
		}
	}
	return domain.InboxItem{}, false
}

func TestAddPRStampsAndPreservesCreatedAt(t *testing.T) {
	s := NewInMemory()
	first, err := s.AddPR(domain.PR{ID: "p1", Title: "one", Number: 1})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if first.CreatedAt == "" {
		t.Fatal("AddPR left CreatedAt empty")
	}
	if first.UpdatedAt == "" {
		t.Fatal("AddPR left UpdatedAt empty")
	}
	second, err := s.AddPR(domain.PR{ID: "p1", Title: "rewritten", Number: 1})
	if err != nil {
		t.Fatalf("AddPR rewrite: %v", err)
	}
	if second.CreatedAt != first.CreatedAt {
		t.Fatalf("CreatedAt on rewrite = %q, want preserved %q", second.CreatedAt, first.CreatedAt)
	}
	if second.Title != "rewritten" {
		t.Fatalf("Title = %q, want rewritten", second.Title)
	}
	if second.UpdatedAt == "" {
		t.Fatal("rewrite left UpdatedAt empty")
	}

	if _, ok := s.UpdatePR("p1", func(p *domain.PR) { p.CreatedAt = "" }); !ok {
		t.Fatal("UpdatePR clear CreatedAt")
	}
	third, err := s.AddPR(domain.PR{ID: "p1", Title: "backfill", Number: 1})
	if err != nil {
		t.Fatalf("AddPR backfill: %v", err)
	}
	if third.CreatedAt == "" {
		t.Fatal("AddPR rewrite did not backfill empty CreatedAt")
	}
}

func TestUpdatePRDoesNotInferActivityFromWrites(t *testing.T) {
	s := NewInMemory()
	const frozen = "2026-08-01T12:00:00Z"
	if _, err := s.AddPR(domain.PR{
		ID: "p1", Title: "one", Source: domain.SourceLocal,
		CreatedAt: frozen, UpdatedAt: frozen,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	got, ok := s.UpdatePR("p1", func(p *domain.PR) {
		p.Files = []domain.FileDiff{{Path: "a.go"}}
	})
	if !ok {
		t.Fatal("UpdatePR ok=false")
	}
	if got.UpdatedAt != frozen {
		t.Fatalf("cache-fill UpdatePR bumped UpdatedAt = %q, want frozen %q", got.UpdatedAt, frozen)
	}

	const explicit = "2026-08-20T12:00:00Z"
	got, ok = s.UpdatePR("p1", func(p *domain.PR) { p.UpdatedAt = explicit })
	if !ok {
		t.Fatal("UpdatePR explicit ok=false")
	}
	if got.UpdatedAt != explicit {
		t.Fatalf("explicit UpdatedAt = %q, want %q", got.UpdatedAt, explicit)
	}

	gh, err := s.AddPR(domain.PR{
		ID: "gh1", Title: "gh", Source: domain.SourceGitHub,
		CreatedAt: frozen, UpdatedAt: frozen,
	})
	if err != nil {
		t.Fatalf("AddPR(gh): %v", err)
	}
	if _, ok = s.UpdatePR(gh.ID, func(p *domain.PR) { p.UpdatedAt = "" }); !ok {
		t.Fatal("UpdatePR clear GitHub UpdatedAt")
	}
	got, ok = s.UpdatePR(gh.ID, func(p *domain.PR) { p.BehindBase = 3 })
	if !ok {
		t.Fatal("UpdatePR(gh cache) ok=false")
	}
	if got.UpdatedAt != "" {
		t.Fatalf("empty GitHub UpdatedAt stamped = %q, want empty (frontend falls back to createdAt)", got.UpdatedAt)
	}

	const listed = "2026-08-27T09:00:00Z"
	got, ok = s.UpdatePR(gh.ID, func(p *domain.PR) { p.UpdatedAt = listed })
	if !ok {
		t.Fatal("UpdatePR(gh listed) ok=false")
	}
	if got.UpdatedAt != listed {
		t.Fatalf("GitHub listed UpdatedAt = %q, want %q", got.UpdatedAt, listed)
	}
}

func TestFillMissingPRTimes(t *testing.T) {
	const known = "2026-01-15T00:00:00Z"
	both := domain.PR{}
	if fillMissingPRTimes(&both) || both.CreatedAt != "" || both.UpdatedAt != "" {
		t.Fatalf("both-empty = created %q updated %q changed, want left unknown", both.CreatedAt, both.UpdatedAt)
	}

	createdOnly := domain.PR{CreatedAt: known}
	if !fillMissingPRTimes(&createdOnly) || createdOnly.UpdatedAt != known || createdOnly.CreatedAt != known {
		t.Fatalf("created-only = %+v, want UpdatedAt copied from CreatedAt", createdOnly)
	}

	updatedOnly := domain.PR{UpdatedAt: known}
	if !fillMissingPRTimes(&updatedOnly) || updatedOnly.CreatedAt != known || updatedOnly.UpdatedAt != known {
		t.Fatalf("updated-only = %+v, want CreatedAt copied from UpdatedAt", updatedOnly)
	}

	full := domain.PR{CreatedAt: known, UpdatedAt: "2026-02-01T00:00:00Z"}
	if fillMissingPRTimes(&full) {
		t.Fatal("both-set should not change")
	}
}

func TestLoadBackfillsMissingPRTimes(t *testing.T) {
	dir := t.TempDir()
	s1 := openTemp(t, dir)
	if _, err := s1.AddPR(domain.PR{
		ID: "legacy-local", Title: "old local", Number: 9, Source: domain.SourceLocal,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := s1.UpdatePR("legacy-local", func(p *domain.PR) {
		p.CreatedAt = ""
		p.UpdatedAt = ""
	}); !ok {
		t.Fatal("clear both timestamps")
	}
	if _, err := s1.AddPR(domain.PR{
		ID: "legacy-created", Title: "has created", Number: 10, Source: domain.SourceLocal,
		CreatedAt: "2026-01-15T00:00:00Z",
	}); err != nil {
		t.Fatalf("AddPR created-only: %v", err)
	}
	if _, ok := s1.UpdatePR("legacy-created", func(p *domain.PR) { p.UpdatedAt = "" }); !ok {
		t.Fatal("clear UpdatedAt")
	}
	s1.Close()

	s2 := openTemp(t, dir)
	empty, ok := s2.PR("legacy-local")
	if !ok {
		t.Fatal("legacy-local missing after reload")
	}
	if empty.CreatedAt != "" || empty.UpdatedAt != "" {
		t.Fatalf("both-empty load invented times: created=%q updated=%q", empty.CreatedAt, empty.UpdatedAt)
	}
	createdOnly, ok := s2.PR("legacy-created")
	if !ok {
		t.Fatal("legacy-created missing after reload")
	}
	if createdOnly.CreatedAt != "2026-01-15T00:00:00Z" || createdOnly.UpdatedAt != "2026-01-15T00:00:00Z" {
		t.Fatalf("CreatedAt-only backfill = created %q updated %q, want copy of CreatedAt not now", createdOnly.CreatedAt, createdOnly.UpdatedAt)
	}
	s2.Close()

	s3 := openTemp(t, dir)
	again, _ := s3.PR("legacy-created")
	if again.CreatedAt != "2026-01-15T00:00:00Z" || again.UpdatedAt != "2026-01-15T00:00:00Z" {
		t.Fatalf("created-only backfill not persisted: created %q updated %q", again.CreatedAt, again.UpdatedAt)
	}
	stillEmpty, _ := s3.PR("legacy-local")
	if stillEmpty.CreatedAt != "" || stillEmpty.UpdatedAt != "" {
		t.Fatalf("both-empty stayed invented after reopen: created %q updated %q", stillEmpty.CreatedAt, stillEmpty.UpdatedAt)
	}
}

func TestBackfillPRTimesWriteError(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	if _, err := s.AddPR(domain.PR{
		ID: "p1", Title: "one", Number: 1, Source: domain.SourceLocal,
		CreatedAt: "2026-01-15T00:00:00Z",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := s.UpdatePR("p1", func(p *domain.PR) { p.UpdatedAt = "" }); !ok {
		t.Fatal("clear UpdatedAt")
	}
	if err := s.db.close(); err != nil {
		t.Fatalf("close db: %v", err)
	}
	if err := s.db.backfillPRTimes(s); err == nil {
		t.Fatal("backfillPRTimes after close: want write error")
	}
}

func TestOpenPRByWorkspacePrefersGitHubOverLocal(t *testing.T) {
	s := NewInMemory()
	local, err := s.AddPR(domain.PR{
		ID: "local-4", WorkspaceID: "ws-dup", Number: 4, Source: domain.SourceLocal,
		State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR(local) err=%v", err)
	}
	gh, err := s.AddPR(domain.PR{
		ID: "gh-156", WorkspaceID: "ws-dup", Number: 156, Source: domain.SourceGitHub,
		State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR(gh) err=%v", err)
	}
	got, ok := s.OpenPRByWorkspace("ws-dup")
	if !ok {
		t.Fatal("OpenPRByWorkspace(ws-dup) ok=false, want true")
	}
	if got.ID != gh.ID {
		t.Errorf("OpenPRByWorkspace(ws-dup) = %q, want GitHub id %q", got.ID, gh.ID)
	}
	if newest, _ := s.PRByWorkspace("ws-dup"); newest.ID != gh.ID {
		t.Errorf("PRByWorkspace(ws-dup) = %q, want newest %q", newest.ID, gh.ID)
	}

	if _, ok := s.UpdatePR(gh.ID, func(p *domain.PR) {
		p.State = "Merged"
		p.Group = "merged"
	}); !ok {
		t.Fatal("UpdatePR(gh) ok=false")
	}
	got, ok = s.OpenPRByWorkspace("ws-dup")
	if !ok || got.ID != local.ID {
		t.Errorf("OpenPRByWorkspace after GitHub merge = id %q ok=%v, want leftover Local %q", got.ID, ok, local.ID)
	}
}

func TestDeletePR(t *testing.T) {
	s := NewInMemory()
	pr, err := s.AddPR(domain.PR{ID: "gone", WorkspaceID: "ws-x", Number: 9, Source: domain.SourceLocal, State: "In review"})
	if err != nil {
		t.Fatalf("AddPR err=%v", err)
	}
	if !s.DeletePR(pr.ID) {
		t.Fatalf("DeletePR(%q) = false, want true", pr.ID)
	}
	if _, ok := s.PR(pr.ID); ok {
		t.Errorf("PR(%q) still present after DeletePR", pr.ID)
	}
	if s.DeletePR(pr.ID) {
		t.Error("DeletePR(missing) = true, want false")
	}
}

// Highest Local seed PR is #479 (LOOP-2224). GitHub seed #482 must not set the mark.
const seededPRHighWater = 479

func TestNextPRNumberDoesNotReclaimAfterWorkspaceDelete(t *testing.T) {
	s := NewInMemory()
	if err := s.DeleteWorkspace("LOOP-2224"); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	got := s.NextPRNumber()
	if got <= seededPRHighWater {
		t.Errorf("NextPRNumber() after deleting workspace that owned #%d = %d, want > %d",
			seededPRHighWater, got, seededPRHighWater)
	}
}

func TestNextPRNumberPersistsAfterWorkspaceDelete(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	if err := s.DeleteWorkspace("LOOP-2224"); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	s.Close()
	s2 := openTemp(t, dir)
	got := s2.NextPRNumber()
	if got <= seededPRHighWater {
		t.Errorf("NextPRNumber() after reload with workspace (PR #%d) deleted = %d, want > %d",
			seededPRHighWater, got, seededPRHighWater)
	}
}

func TestPRSeqMigrationBackfillsFromExistingPRs(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	if _, err := s.AddPR(domain.PR{
		ID: "gh-9000", WorkspaceID: "LOOP-2224", Number: 9000, Source: domain.SourceGitHub,
	}); err != nil {
		t.Fatalf("AddPR(gh-9000): %v", err)
	}
	// Promoted Local PR: id stays the issued Local number, source/number become GitHub.
	const promotedID = 500
	if _, err := s.AddPR(domain.PR{
		ID: "500", WorkspaceID: "LOOP-2224", Number: 9001, Source: domain.SourceGitHub,
	}); err != nil {
		t.Fatalf("AddPR(promoted): %v", err)
	}
	s.Close()

	path := filepath.Join(dir, "recur.db")
	legacy, err := openDB(path)
	if err != nil {
		t.Fatalf("openDB: %v", err)
	}
	for _, q := range []string{
		`DROP TABLE pr_seq`,
		`DELETE FROM __migrations WHERE version = 12`,
	} {
		if _, err := legacy.sql.Exec(q); err != nil {
			legacy.close()
			t.Fatalf("simulate pre-0012 (%q): %v", q, err)
		}
	}
	legacy.close()

	s2 := openTemp(t, dir)
	var got int
	if err := s2.db.sql.QueryRow(`SELECT last_number FROM pr_seq WHERE id = 1`).Scan(&got); err != nil {
		t.Fatalf("SELECT pr_seq.last_number: %v", err)
	}
	if got != promotedID {
		t.Errorf("pr_seq.last_number after 0012 backfill = %d, want %d (promoted Local id; not GitHub #9000/#9001)",
			got, promotedID)
	}
}

func TestNextPRNumberAllocationsAreMonotonic(t *testing.T) {
	s := NewInMemory()
	n1 := s.NextPRNumber()
	if _, err := s.AddPR(domain.PR{
		ID: fmt.Sprintf("%d", n1), WorkspaceID: "LOOP-2224", Number: n1, Source: domain.SourceLocal,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if !s.DeletePR(fmt.Sprintf("%d", n1)) {
		t.Fatalf("DeletePR(%d) = false, want true", n1)
	}
	n2 := s.NextPRNumber()
	if n2 <= n1 {
		t.Errorf("NextPRNumber() after deleting #%d = %d, want > %d", n1, n2, n1)
	}
}

func TestNextPRNumberStartsAtOneOnEmptyStore(t *testing.T) {
	dir := t.TempDir()
	s, err := open(filepath.Join(dir, "recur.db"))
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	got := s.NextPRNumber()
	if got != 1 {
		t.Errorf("NextPRNumber() on empty store = %d, want 1", got)
	}
}

func TestClearAllDoesNotReclaimPRNumbers(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	if err := s.ClearAll(); err != nil {
		t.Fatalf("ClearAll: %v", err)
	}
	got := s.NextPRNumber()
	if got <= seededPRHighWater {
		t.Errorf("NextPRNumber() after ClearAll = %d, want > %d", got, seededPRHighWater)
	}
	s.Close()
	s2 := openTemp(t, dir)
	got2 := s2.NextPRNumber()
	if got2 <= got {
		t.Errorf("NextPRNumber() after ClearAll+reopen = %d, want > %d", got2, got)
	}
}

func TestGitHubPRNumberDoesNotRatchetLocalSeq(t *testing.T) {
	s := NewInMemory()
	before := s.NextPRNumber()
	if _, err := s.AddPR(domain.PR{
		ID: "gh-9000", WorkspaceID: "LOOP-2224", Number: 9000, Source: domain.SourceGitHub,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got := s.NextPRNumber()
	if got != before+1 {
		t.Errorf("NextPRNumber() after AddPR GitHub #9000 = %d, want %d", got, before+1)
	}
}

func TestPromoteToGitHubDoesNotRatchetLocalSeq(t *testing.T) {
	s := NewInMemory()
	n := s.NextPRNumber()
	id := fmt.Sprintf("%d", n)
	if _, err := s.AddPR(domain.PR{
		ID: id, WorkspaceID: "LOOP-2224", Number: n, Source: domain.SourceLocal,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := s.UpdatePR(id, func(p *domain.PR) {
		p.Source = domain.SourceGitHub
		p.Number = 9000
	}); !ok {
		t.Fatal("UpdatePR() ok = false, want true")
	}
	got := s.NextPRNumber()
	if got != n+1 {
		t.Errorf("NextPRNumber() after promoting Local #%d to GitHub #9000 = %d, want %d", n, got, n+1)
	}
}

func TestCheckpointFlushesWALIntoMainFile(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	path := filepath.Join(dir, "recur.db")
	if _, err := s.db.sql.Exec("PRAGMA wal_autocheckpoint=0"); err != nil {
		t.Fatalf("disable autocheckpoint: %v", err)
	}
	if _, err := s.AddWorkspace(domain.Workspace{
		ID: "wal-probe", ProjectID: "storefront", Goal: "checkpoint probe",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	stale := filepath.Join(dir, "stale.db")
	copyMainFile(t, path, stale)
	if workspaceInFile(t, stale, "wal-probe") {
		t.Fatal("main-file copy already has wal-probe; test cannot detect a WAL miss")
	}

	s.db.checkpoint()
	copyMainFile(t, path, stale)
	if !workspaceInFile(t, stale, "wal-probe") {
		t.Fatal("Checkpoint did not flush wal-probe into the main file")
	}
}

func copyMainFile(t *testing.T, src, dst string) {
	t.Helper()
	data, err := os.ReadFile(src)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dst, data, 0o644); err != nil {
		t.Fatal(err)
	}
}

func workspaceInFile(t *testing.T, path, id string) bool {
	t.Helper()
	s, err := open(path)
	if err != nil {
		t.Fatalf("open %s: %v", path, err)
	}
	defer s.Close()
	_, ok := s.Workspace(id)
	return ok
}

func TestMoveWorkspaceAgentsMergesPlaceholder(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	s.UpsertAgent("W-host", domain.Agent{
		ID: "h-PA", WorkspaceID: "W-host", Code: "PA", SessionID: "sess-host",
		Unseen: true, Model: "sonnet", Status: "idle", Name: "Agent",
	})
	s.UpsertAgent("W-live", domain.Agent{
		ID: "l-PA", WorkspaceID: "W-live", Code: "PA", Status: "idle", Name: "Agent",
	})
	s.MoveWorkspaceAgents("W-host", "W-live", nil)
	got, ok := s.Agent("W-live", "PA")
	if !ok {
		t.Fatal("Agent(W-live, PA) missing")
	}
	if got.SessionID != "sess-host" {
		t.Errorf("MoveWorkspaceAgents SessionID = %q, want sess-host", got.SessionID)
	}
	if got.ID != "l-PA" {
		t.Errorf("MoveWorkspaceAgents ID = %q, want l-PA", got.ID)
	}
	if got.WorkspaceID != "W-live" {
		t.Errorf("MoveWorkspaceAgents WorkspaceID = %q, want W-live", got.WorkspaceID)
	}
	if !got.Unseen {
		t.Error("MoveWorkspaceAgents Unseen = false, want true")
	}
	if _, still := s.Agent("W-host", "PA"); still {
		t.Error("source workspace still has the agent")
	}
}

func TestMoveWorkspaceAgentsRemapsCollidingCode(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	s.UpsertAgent("W-host", domain.Agent{
		ID: "h-PA", WorkspaceID: "W-host", Code: "PA", SessionID: "sess-host",
	})
	s.UpsertAgent("W-live", domain.Agent{
		ID: "l-PA", WorkspaceID: "W-live", Code: "PA", SessionID: "sess-live",
	})
	s.MoveWorkspaceAgents("W-host", "W-live", map[string]string{"PA": "PA2"})
	live, ok := s.Agent("W-live", "PA")
	if !ok || live.SessionID != "sess-live" {
		t.Fatalf("dest PA = %+v ok=%v, want sess-live", live, ok)
	}
	host, ok := s.Agent("W-live", "PA2")
	if !ok || host.SessionID != "sess-host" {
		t.Fatalf("hoisted PA2 = %+v ok=%v, want sess-host", host, ok)
	}
	if host.ID != "W-live-PA2" || host.WorkspaceID != "W-live" {
		t.Errorf("hoisted identity = %s/%s, want W-live-PA2/W-live", host.ID, host.WorkspaceID)
	}
}

func TestReassignWorkspaceFindings(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	s.AddFinding(domain.Finding{
		ID: "F-host", WorkspaceID: "W-host", File: "a.go", Line: 1,
		Severity: "high", Message: "bug", Status: "open",
	})
	s.AddFinding(domain.Finding{
		ID: "F-live", WorkspaceID: "W-live", File: "b.go", Line: 2,
		Severity: "low", Message: "nit", Status: "open",
	})
	if got := s.ReassignWorkspaceFindings("W-host", "W-live"); got != 1 {
		t.Fatalf("ReassignWorkspaceFindings = %d, want 1", got)
	}
	got := s.Findings("W-live")
	if len(got) != 2 {
		t.Fatalf("Findings(W-live) len = %d, want 2", len(got))
	}
	if got[0].ID != "F-live" || got[1].ID != "F-host" || got[1].WorkspaceID != "W-live" {
		t.Fatalf("Findings(W-live) = %+v, want F-live then moved F-host", got)
	}
	if n := s.OpenFindingsCount("W-host"); n != 0 {
		t.Errorf("OpenFindingsCount(W-host) = %d, want 0", n)
	}
}

func TestPinClaudeDefaultModelsMigrationRewritesLegacyAliases(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	set := s.Settings()
	set.ModelAsk = "claude-code/sonnet"
	set.ModelAgent = "codex/gpt-5"
	set.ModelWorkspaceTitle = "claude-code/haiku"
	s.SetSettings(set)
	s.Close()

	legacy, err := openDB(filepath.Join(dir, "recur.db"))
	if err != nil {
		t.Fatalf("openDB: %v", err)
	}
	if _, err := legacy.sql.Exec(`DELETE FROM __migrations WHERE version = 17`); err != nil {
		legacy.close()
		t.Fatalf("simulate pre-0017: %v", err)
	}
	legacy.close()

	got := openTemp(t, dir).Settings()
	for name, pair := range map[string][2]string{
		"ModelAsk":            {got.ModelAsk, "claude-code/claude-sonnet-5-5"},
		"ModelAgent":          {got.ModelAgent, "codex/gpt-5"},
		"ModelWorkspaceTitle": {got.ModelWorkspaceTitle, "claude-code/claude-haiku-4-5-20251001"},
	} {
		if pair[0] != pair[1] {
			t.Errorf("%s after 0017 = %q, want %q", name, pair[0], pair[1])
		}
	}
}
