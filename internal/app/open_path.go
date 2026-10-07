package app

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/code"
	"recur/internal/domain"
	"recur/internal/engine"
	gitpkg "recur/internal/git"
	"recur/internal/store"
)

// OpenPath opens a file or folder in Recur: find-or-create a project, focus
// its working copy (or the matching workspace), and optionally jump to a file.
// Homeless files (not inside any project or git repo) become absolute-path
// tabs on the singleton Files project — they do not add ~/Downloads as a project.
func (s *WorkspacesService) OpenPath(path string) (domain.OpenPathResult, error) {
	s.openMu.Lock()
	defer s.openMu.Unlock()

	res, err := s.openPathLocked(path)
	if err != nil {
		return res, err
	}
	s.eng.Bus.Publish(engine.Event{Name: "app.open", Data: res})
	return res, nil
}

func (s *WorkspacesService) openPathLocked(path string) (domain.OpenPathResult, error) {
	abs, err := gitpkg.CanonPath(path)
	if err != nil {
		return domain.OpenPathResult{}, err
	}
	st, err := os.Stat(abs)
	if err != nil {
		return domain.OpenPathResult{}, fmt.Errorf("open %s: %w", abs, err)
	}
	isFile := !st.IsDir()
	dir := abs
	if isFile {
		dir = filepath.Dir(abs)
	}

	res := domain.OpenPathResult{Path: abs}

	// 1. Longest matching workspace worktree (managed loop or main checkout).
	if ws, ok := s.bestWorkspaceFor(abs); ok {
		return s.finishInWorkspace(res, ws, abs, isFile)
	}

	// 2. Longest matching existing project root.
	if p, ok := s.bestProjectFor(dir); ok {
		return s.finishInProject(res, p, abs, isFile, false)
	}

	// 3. Enclosing git checkout that Recur doesn't know yet — adopt the toplevel.
	if top, ok := gitpkg.RepoRoot(dir); ok {
		if p, found := s.bestProjectFor(top); found {
			return s.finishInProject(res, p, abs, isFile, false)
		}
		p, created, err := s.ensureProjectAt(top)
		if err != nil {
			return res, err
		}
		return s.finishInProject(res, p, abs, isFile, created)
	}

	// 4. A folder the user asked to open — becomes a (possibly non-git) project.
	if !isFile {
		p, created, err := s.ensureProjectAt(abs)
		if err != nil {
			return res, err
		}
		return s.finishInProject(res, p, abs, false, created)
	}

	// 5. Homeless file — Files project + absolute-path tab.
	return s.finishLooseFile(res, abs)
}

func (s *WorkspacesService) finishInWorkspace(res domain.OpenPathResult, ws domain.Workspace, abs string, isFile bool) (domain.OpenPathResult, error) {
	root, err := gitpkg.CanonPath(ws.WorktreePath)
	if err != nil {
		root = filepath.Clean(ws.WorktreePath)
	}
	res.ProjectID = ws.ProjectID
	res.WorkspaceID = ws.ID
	cp := ws
	res.Workspace = &cp
	if p, ok := s.st().Project(ws.ProjectID); ok {
		pp := withIsGit(p)
		res.Project = &pp
	}
	if isFile {
		res.File = relOrAbs(root, abs)
		if filepath.IsAbs(res.File) {
			s.loose.allow(abs)
			res.Loose = true
		}
	} else if !gitpkg.SamePath(abs, root) {
		res.Reveal = relOrAbs(root, abs)
	}
	return res, nil
}

func (s *WorkspacesService) finishInProject(res domain.OpenPathResult, p domain.Project, abs string, isFile, created bool) (domain.OpenPathResult, error) {
	ws, err := s.eng.OpenMainWorkspace(p.ID)
	if err != nil {
		return res, err
	}
	p = withIsGit(p)
	res.Created = created
	res.ProjectID = p.ID
	res.WorkspaceID = ws.ID
	res.Project = &p
	res.Workspace = &ws
	root, err := gitpkg.CanonPath(p.RepoPath)
	if err != nil {
		root = filepath.Clean(gitpkg.ExpandHome(p.RepoPath))
	}
	if isFile {
		res.File = relOrAbs(root, abs)
	} else if !gitpkg.SamePath(abs, root) {
		res.Reveal = relOrAbs(root, abs)
	}
	return res, nil
}

func (s *WorkspacesService) finishLooseFile(res domain.OpenPathResult, abs string) (domain.OpenPathResult, error) {
	p, created, err := s.ensureFilesProject()
	if err != nil {
		return res, err
	}
	ws, err := s.eng.OpenMainWorkspace(p.ID)
	if err != nil {
		return res, err
	}
	s.loose.allow(abs)
	p = withIsGit(p)
	res.Created = created
	res.Loose = true
	res.ProjectID = p.ID
	res.WorkspaceID = ws.ID
	res.File = abs
	res.Project = &p
	res.Workspace = &ws
	return res, nil
}

func (s *WorkspacesService) bestWorkspaceFor(abs string) (domain.Workspace, bool) {
	var best domain.Workspace
	bestLen := -1
	for _, w := range s.st().Workspaces() {
		if w.Archived || w.IsPRChatHost() || strings.TrimSpace(w.WorktreePath) == "" {
			continue
		}
		root, err := gitpkg.CanonPath(w.WorktreePath)
		if err != nil || root == "" {
			continue
		}
		if !gitpkg.PathHasPrefix(abs, root) {
			continue
		}
		if len(root) > bestLen {
			best = w
			bestLen = len(root)
		}
	}
	return best, bestLen >= 0
}

func (s *WorkspacesService) bestProjectFor(abs string) (domain.Project, bool) {
	var best domain.Project
	bestLen := -1
	for _, p := range s.st().Projects() {
		if strings.TrimSpace(p.RepoPath) == "" {
			continue
		}
		root, err := gitpkg.CanonPath(p.RepoPath)
		if err != nil || root == "" {
			continue
		}
		if !gitpkg.PathHasPrefix(abs, root) {
			continue
		}
		if len(root) > bestLen {
			best = p
			bestLen = len(root)
		}
	}
	return best, bestLen >= 0
}

func (s *WorkspacesService) ensureProjectAt(abs string) (domain.Project, bool, error) {
	if p, ok := s.bestProjectFor(abs); ok {
		root, err := gitpkg.CanonPath(p.RepoPath)
		if err == nil && gitpkg.SamePath(root, abs) {
			return p, false, nil
		}
	}
	name := filepath.Base(abs)
	if name == "" || name == string(os.PathSeparator) || name == "." {
		name = "folder"
	}
	ps := ProjectsService{s.base}
	saved, err := ps.AddProject(domain.Project{Name: name, RepoPath: abs})
	if err != nil {
		return domain.Project{}, false, err
	}
	return saved, true, nil
}

func (s *WorkspacesService) ensureFilesProject() (domain.Project, bool, error) {
	if p, ok := s.st().Project(domain.FilesProjectID); ok {
		return p, false, nil
	}
	dir, err := store.OpenFilesDir()
	if err != nil {
		return domain.Project{}, false, err
	}
	ps := ProjectsService{s.base}
	saved, err := ps.AddProject(domain.Project{
		ID:       domain.FilesProjectID,
		Name:     "Files",
		Pic:      "F",
		Color:    "var(--indigo)",
		RepoPath: dir,
	})
	if err != nil {
		return domain.Project{}, false, err
	}
	return saved, true, nil
}

func relOrAbs(root, abs string) string {
	abs = gitpkg.AlignUnderRoot(abs, root)
	if gitpkg.SamePath(root, abs) {
		return ""
	}
	rel := code.RelSlash(root, abs)
	if rel == "." {
		return ""
	}
	if strings.HasPrefix(rel, "..") {
		return abs
	}
	return rel
}
