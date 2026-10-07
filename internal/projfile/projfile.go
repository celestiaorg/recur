// Package projfile reads and writes portable Recur project settings from
// <repo>/.recur/project.toml so setup/teardown/runs/env/copy-files can be
// committed and shared across machines. That file is the only source for
// run-config; the settings page writes it (creating it if needed). Machine-local
// fields (path, sidebar, pinned run, skills) stay in Recur's database.
package projfile

import (
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	toml "github.com/pelletier/go-toml/v2"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// RelPath is the repo-relative location of the committed project file.
const RelPath = ".recur/project.toml"

const fileHeader = `# Recur project settings: setup, teardown, named runs, app URL, env, and files to copy.
# Safe to commit and share. Keep secrets in gitignored dotenv for the app to load;
# Recur copies them via copy_files and does not export them.
`

// File is the TOML shape for read and write. multiline:"true" emits """…"""
// when a string contains a newline; Unmarshal ignores the tag.
type File struct {
	SetupCmd    string    `toml:"setup_cmd" multiline:"true"`
	TeardownCmd string    `toml:"teardown_cmd" multiline:"true"`
	AppURL      string    `toml:"app_url"`
	Env         string    `toml:"env" multiline:"true"`
	CopyFiles   string    `toml:"copy_files" multiline:"true"`
	Runs        []fileRun `toml:"run,omitempty"`
}

type fileRun struct {
	Name string `toml:"name"`
	Cmd  string `toml:"cmd" multiline:"true"`
}

type cacheEnt struct {
	mtime time.Time
	size  int64
	file  File
	err   error
}

var (
	cacheMu sync.Mutex
	cache   = map[string]cacheEnt{}
	logged  = map[string]time.Time{} // path → mtime of last parse-error log
)

// Path returns the absolute project.toml path for a repo, or "" if repoPath
// is empty.
func Path(repoPath string) string {
	root := repoRoot(repoPath)
	if root == "" {
		return ""
	}
	return filepath.Join(root, RelPath)
}

// Exists reports whether <repo>/.recur/project.toml is present as a file.
func Exists(repoPath string) bool {
	path := Path(repoPath)
	if path == "" {
		return false
	}
	st, err := os.Stat(path)
	return err == nil && !st.IsDir()
}

// Overlay replaces p's run-config with .recur/project.toml. A missing file
// yields empty run-config. A malformed file is logged once per change, sets
// ProjectFileError, and also yields empty run-config so the project stays
// visible and settings can rewrite the file.
func Overlay(p domain.Project) domain.Project {
	p.HasProjectFile = Exists(p.RepoPath)
	p.ProjectFileError = ""
	clearRunConfig(&p)
	if !p.HasProjectFile {
		return p
	}
	f, err := load(p.RepoPath)
	if err != nil {
		logParseErr(p.RepoPath, err)
		p.ProjectFileError = err.Error()
		return p
	}
	Apply(&p, f)
	return p
}

func clearRunConfig(p *domain.Project) {
	if p == nil {
		return
	}
	p.SetupCmd = ""
	p.TeardownCmd = ""
	p.AppURL = ""
	p.Env = ""
	p.CopyFiles = ""
	p.RunCommands = nil
	p.ProjectFileError = ""
}

func load(repoPath string) (File, error) {
	path := Path(repoPath)
	if path == "" {
		return File{}, nil
	}
	st, err := os.Stat(path)
	if err != nil {
		if os.IsNotExist(err) {
			invalidate(path)
			return File{}, nil
		}
		return File{}, err
	}
	if st.IsDir() {
		return File{}, fmt.Errorf("parse %s: is a directory", RelPath)
	}
	mtime, size := st.ModTime(), st.Size()
	cacheMu.Lock()
	if e, ok := cache[path]; ok && e.mtime.Equal(mtime) && e.size == size {
		f, loadErr := e.file, e.err
		cacheMu.Unlock()
		return f, loadErr
	}
	cacheMu.Unlock()

	data, err := os.ReadFile(path)
	if err != nil {
		return File{}, err
	}
	var f File
	parseErr := toml.Unmarshal(data, &f)
	if parseErr != nil {
		parseErr = fmt.Errorf("parse %s: %w", RelPath, parseErr)
	}
	cacheMu.Lock()
	cache[path] = cacheEnt{mtime: mtime, size: size, file: f, err: parseErr}
	cacheMu.Unlock()
	return f, parseErr
}

func logParseErr(repoPath string, err error) {
	path := Path(repoPath)
	var mtime time.Time
	if st, statErr := os.Stat(path); statErr == nil {
		mtime = st.ModTime()
	}
	cacheMu.Lock()
	prev, ok := logged[path]
	if ok && prev.Equal(mtime) {
		cacheMu.Unlock()
		return
	}
	logged[path] = mtime
	cacheMu.Unlock()
	log.Printf("recur: %s: %v", RelPath, err)
}

func invalidate(path string) {
	if path == "" {
		return
	}
	cacheMu.Lock()
	delete(cache, path)
	delete(logged, path)
	cacheMu.Unlock()
}

// Apply copies file fields onto p. Overlay clears run-config first so
// omitted keys stay empty.
func Apply(p *domain.Project, f File) {
	if p == nil {
		return
	}
	p.SetupCmd = f.SetupCmd
	p.TeardownCmd = f.TeardownCmd
	p.AppURL = f.AppURL
	p.Env = f.Env
	p.CopyFiles = f.CopyFiles
	p.RunCommands = domainRuns(f.Runs)
}

func repoRoot(repoPath string) string {
	return strings.TrimSpace(gitpkg.ExpandHome(repoPath))
}

func domainRuns(in []fileRun) []domain.RunCommand {
	out := make([]domain.RunCommand, 0, len(in))
	for _, r := range in {
		out = append(out, domain.RunCommand{Name: r.Name, Cmd: r.Cmd})
	}
	return domain.EffectiveRuns(domain.Project{RunCommands: out})
}
