package app

import (
	"os"
	"path/filepath"
	"testing"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

func TestOpenPathNewGitFolder(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	repo := initGitRepo(t)
	svc := newOpenPathSvc(t)

	res, err := svc.Workspaces.OpenPath(repo)
	if err != nil {
		t.Fatalf("OpenPath(%s) error = %v", repo, err)
	}
	if !res.Created || res.WorkspaceID == "" || res.ProjectID == "" {
		t.Fatalf("OpenPath(%s) = %+v, want created project+workspace", repo, res)
	}
	if res.Loose || res.File != "" {
		t.Fatalf("OpenPath folder File/Loose = %q/%v, want empty", res.File, res.Loose)
	}
	if res.Workspace == nil || !res.Workspace.IsMain {
		t.Fatalf("OpenPath workspace IsMain = %v, want true", res.Workspace)
	}

	again, err := svc.Workspaces.OpenPath(repo)
	if err != nil {
		t.Fatalf("OpenPath reuse: %v", err)
	}
	if again.Created {
		t.Fatal("second OpenPath created another project")
	}
	if again.ProjectID != res.ProjectID || again.WorkspaceID != res.WorkspaceID {
		t.Fatalf("OpenPath reuse ids = %s/%s, want %s/%s", again.ProjectID, again.WorkspaceID, res.ProjectID, res.WorkspaceID)
	}
}

func TestOpenPathFileInsideProject(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	repo := initGitRepo(t)
	svc := newOpenPathSvc(t)
	pid := svcAddProject(t, svc.Workspaces.eng, repo)

	file := filepath.Join(repo, "README.md")
	res, err := svc.Workspaces.OpenPath(file)
	if err != nil {
		t.Fatalf("OpenPath(%s) error = %v", file, err)
	}
	if res.Created || res.ProjectID != pid {
		t.Fatalf("OpenPath file = %+v, want existing project %s", res, pid)
	}
	if res.File != "README.md" {
		t.Fatalf("OpenPath file File = %q, want README.md", res.File)
	}
}

func TestOpenPathNestedFolderReusesProject(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	repo := initGitRepo(t)
	sub := filepath.Join(repo, "docs")
	if err := os.Mkdir(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	svc := newOpenPathSvc(t)
	pid := svcAddProject(t, svc.Workspaces.eng, repo)

	res, err := svc.Workspaces.OpenPath(sub)
	if err != nil {
		t.Fatalf("OpenPath(%s) error = %v", sub, err)
	}
	if res.Created || res.ProjectID != pid {
		t.Fatalf("OpenPath nested = %+v, want project %s", res, pid)
	}
	if res.Reveal != "docs" {
		t.Fatalf("OpenPath nested Reveal = %q, want docs", res.Reveal)
	}
}

func TestOpenPathAdoptsGitToplevel(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	repo := initGitRepo(t)
	sub := filepath.Join(repo, "pkg")
	if err := os.Mkdir(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(sub, "x.go"), []byte("package pkg\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	svc := newOpenPathSvc(t)

	res, err := svc.Workspaces.OpenPath(filepath.Join(sub, "x.go"))
	if err != nil {
		t.Fatalf("OpenPath nested file: %v", err)
	}
	if !res.Created {
		t.Fatal("OpenPath should create a project for an unknown git repo")
	}
	root, _ := filepath.EvalSymlinks(repo)
	got, _ := filepath.EvalSymlinks(res.Project.RepoPath)
	if got != root {
		t.Fatalf("OpenPath adopted RepoPath = %q, want git toplevel %q", res.Project.RepoPath, root)
	}
	if res.File != "pkg/x.go" {
		t.Fatalf("OpenPath adopted File = %q, want pkg/x.go", res.File)
	}
}

func TestOpenPathHomelessFile(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	dir := t.TempDir()
	file := filepath.Join(dir, "note.txt")
	if err := os.WriteFile(file, []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	svc := newOpenPathSvc(t)

	res, err := svc.Workspaces.OpenPath(file)
	if err != nil {
		t.Fatalf("OpenPath homeless: %v", err)
	}
	if !res.Loose || res.ProjectID != domain.FilesProjectID {
		t.Fatalf("OpenPath homeless = %+v, want Files project + loose", res)
	}
	if res.File != file && !filepath.IsAbs(res.File) {
		t.Fatalf("OpenPath homeless File = %q, want abs %q", res.File, file)
	}

	fc, err := svc.Code.ReadFile(res.ProjectID, res.WorkspaceID, res.File)
	if err != nil {
		t.Fatalf("ReadFile loose: %v", err)
	}
	if fc.Content != "hi\n" {
		t.Fatalf("ReadFile loose Content = %q, want hi\\n", fc.Content)
	}
	if err := svc.Code.WriteFile(res.ProjectID, res.WorkspaceID, res.File, "bye\n"); err != nil {
		t.Fatalf("WriteFile loose: %v", err)
	}
	raw, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	if string(raw) != "bye\n" {
		t.Fatalf("WriteFile loose disk = %q, want bye\\n", raw)
	}

	_, err = svc.Code.ReadFile(res.ProjectID, res.WorkspaceID, filepath.Join(dir, "other.txt"))
	if err == nil {
		t.Fatal("ReadFile of a non-allowlisted abs path must fail")
	}
}

func TestOpenPathPlainFolder(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	dir := t.TempDir()
	svc := newOpenPathSvc(t)

	res, err := svc.Workspaces.OpenPath(dir)
	if err != nil {
		t.Fatalf("OpenPath plain folder: %v", err)
	}
	if !res.Created || res.Loose || res.ProjectID == domain.FilesProjectID {
		t.Fatalf("OpenPath plain folder = %+v, want a real folder project", res)
	}
	if res.Project != nil && res.Project.IsGit {
		t.Fatal("plain folder project reported IsGit=true")
	}
}

func TestOpenPathLooseFileSurvivesRestart(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	dir := t.TempDir()
	file := filepath.Join(dir, "note.txt")
	if err := os.WriteFile(file, []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	svc := newOpenPathSvc(t)
	res, err := svc.Workspaces.OpenPath(file)
	if err != nil {
		t.Fatalf("OpenPath homeless: %v", err)
	}
	if err := svc.EditorSession.UpdateEditorSession(res.WorkspaceID, EditorSessionPatch{
		Meta: &EditorSessionMeta{OpenFiles: []string{res.File}, SelectedPath: res.File},
	}); err != nil {
		t.Fatalf("UpdateEditorSession: %v", err)
	}

	fresh := NewServices(svc.Workspaces.eng)
	fc, err := fresh.Code.ReadFile(res.ProjectID, res.WorkspaceID, res.File)
	if err != nil {
		t.Fatalf("ReadFile after restart: %v", err)
	}
	if fc.Content != "hi\n" {
		t.Fatalf("ReadFile after restart Content = %q, want hi\\n", fc.Content)
	}
}

func TestAddProjectReservesFilesID(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	svc := newOpenPathSvc(t)
	dir := t.TempDir()
	p, err := svc.Projects.AddProject(domain.Project{Name: "Files", RepoPath: dir})
	if err != nil {
		t.Fatalf("AddProject(Files): %v", err)
	}
	if p.ID == domain.FilesProjectID {
		t.Fatalf("AddProject(Files) ID = %q, want a non-reserved slug", p.ID)
	}
	file := filepath.Join(t.TempDir(), "note.txt")
	if err := os.WriteFile(file, []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res, err := svc.Workspaces.OpenPath(file)
	if err != nil {
		t.Fatalf("OpenPath homeless: %v", err)
	}
	if res.ProjectID != domain.FilesProjectID {
		t.Fatalf("OpenPath homeless ProjectID = %q, want %q", res.ProjectID, domain.FilesProjectID)
	}
}

func TestOpenPathCaseInsensitiveReuse(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	repo := initGitRepo(t)
	svc := newOpenPathSvc(t)
	res, err := svc.Workspaces.OpenPath(repo)
	if err != nil {
		t.Fatalf("OpenPath: %v", err)
	}
	alt := flipPathCase(repo)
	if alt == repo {
		t.Skip("could not flip path case")
	}
	if _, err := os.Stat(alt); err != nil {
		t.Skip("filesystem is case-sensitive")
	}
	again, err := svc.Workspaces.OpenPath(alt)
	if err != nil {
		t.Fatalf("OpenPath(cased): %v", err)
	}
	if again.Created || again.ProjectID != res.ProjectID {
		t.Fatalf("OpenPath(cased) = %+v, want reuse %s", again, res.ProjectID)
	}

	file := filepath.Join(alt, "README.md")
	if _, err := os.Stat(file); err != nil {
		t.Skip("cased README.md not visible")
	}
	got, err := svc.Workspaces.OpenPath(file)
	if err != nil {
		t.Fatalf("OpenPath(cased file): %v", err)
	}
	if got.Loose || got.File != "README.md" {
		t.Fatalf("OpenPath(cased file) File/Loose = %q/%v, want README.md/false", got.File, got.Loose)
	}
}

func flipPathCase(p string) string {
	b := []byte(p)
	for i := len(b) - 1; i >= 0; i-- {
		c := b[i]
		if c >= 'a' && c <= 'z' {
			b[i] = c - 'a' + 'A'
			return string(b)
		}
		if c >= 'A' && c <= 'Z' {
			b[i] = c - 'A' + 'a'
			return string(b)
		}
	}
	return p
}

func TestOpenPathMissing(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	svc := newOpenPathSvc(t)
	_, err := svc.Workspaces.OpenPath(filepath.Join(t.TempDir(), "nope"))
	if err == nil {
		t.Fatal("OpenPath missing path: error = nil, want error")
	}
}

func TestOpenPathManagedWorktree(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	pid := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(pid, "task", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ready := waitWorkspaceReady(t, eng, ws.ID)
	if ready.WorktreePath == "" {
		t.Fatal("workspace has no worktree")
	}
	if err := os.WriteFile(filepath.Join(ready.WorktreePath, "extra.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	res, err := svc.Workspaces.OpenPath(filepath.Join(ready.WorktreePath, "extra.txt"))
	if err != nil {
		t.Fatalf("OpenPath worktree file: %v", err)
	}
	if res.WorkspaceID != ready.ID {
		t.Fatalf("OpenPath worktree WorkspaceID = %q, want %q", res.WorkspaceID, ready.ID)
	}
	if res.File != "extra.txt" {
		t.Fatalf("OpenPath worktree File = %q, want extra.txt", res.File)
	}
}

func newOpenPathSvc(t *testing.T) *Services {
	t.Helper()
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	return NewServices(eng)
}
