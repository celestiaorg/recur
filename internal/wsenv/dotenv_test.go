package wsenv

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/domain"
)

func envMap(env []string) map[string]string {
	got := map[string]string{}
	for _, kv := range env {
		if i := strings.IndexByte(kv, '='); i > 0 {
			got[kv[:i]] = kv[i+1:]
		}
	}
	return got
}

func writeDotenv(t *testing.T, dir, rel, body string) {
	t.Helper()
	path := filepath.Join(dir, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestLoadDotenvFilesLaterWins(t *testing.T) {
	repo := t.TempDir()
	writeDotenv(t, repo, ".env", "A=from-env\nB=env\n")
	writeDotenv(t, repo, ".env.local", "B=local\n")
	writeDotenv(t, repo, ".recur/.env", "C=recur-env\n")
	got := LoadDotenvFiles(repo)
	if got["A"] != "from-env" || got["B"] != "local" || got["C"] != "recur-env" {
		t.Fatalf("%#v", got)
	}
}

func TestLoadDotenvFilesWorktreeOverridesRepo(t *testing.T) {
	repo := t.TempDir()
	wt := t.TempDir()
	writeDotenv(t, repo, ".env", "SECRET=repo\nSHARED=repo\n")
	writeDotenv(t, wt, ".env", "SHARED=worktree\n")
	got := LoadDotenvFiles(repo, wt)
	if got["SECRET"] != "repo" {
		t.Errorf("SECRET=%q", got["SECRET"])
	}
	if got["SHARED"] != "worktree" {
		t.Errorf("SHARED=%q", got["SHARED"])
	}
}

func TestLoadDotenvFilesMissingOK(t *testing.T) {
	got := LoadDotenvFiles(t.TempDir(), filepath.Join(t.TempDir(), "nope"))
	if len(got) != 0 {
		t.Fatalf("%#v", got)
	}
}

func TestEnvironLoadsDotenvAndExpandsProjectEnv(t *testing.T) {
	repo := t.TempDir()
	writeDotenv(t, repo, ".env", "API_KEY=sk-test\nNESTED=$API_KEY\n")
	writeDotenv(t, repo, ".recur/.env.local", "LOCAL=1\n")
	v := Vars{
		Port:       9,
		RepoPath:   repo,
		ProjectID:  "p",
		ProjectEnv: map[string]string{"AUTH": "Bearer $API_KEY", "NESTED_COPY": "$NESTED", "PORT_COPY": "$RECUR_PORT"},
	}
	got := envMap(v.Environ([]string{"PATH=/bin"}))
	if _, ok := got["API_KEY"]; ok {
		t.Errorf("API_KEY=%q, want absent", got["API_KEY"])
	}
	if _, ok := got["NESTED"]; ok {
		t.Errorf("NESTED=%q, want absent", got["NESTED"])
	}
	if _, ok := got["LOCAL"]; ok {
		t.Errorf("LOCAL=%q, want absent", got["LOCAL"])
	}
	if got["AUTH"] != "Bearer sk-test" {
		t.Errorf("AUTH=%q", got["AUTH"])
	}
	if got["NESTED_COPY"] != "sk-test" {
		t.Errorf("NESTED_COPY=%q, want %q", got["NESTED_COPY"], "sk-test")
	}
	if got["PORT_COPY"] != "9" {
		t.Errorf("PORT_COPY=%q", got["PORT_COPY"])
	}
}

func TestEnvironDBProjectEnvMaySetPATH(t *testing.T) {
	v := FromWorkspace(
		domain.Workspace{ID: "W1", ProjectID: "p"},
		domain.Project{Env: "PATH=/tmp/userbin\nDYLD_INSERT_LIBRARIES=/tmp/x.dylib\nOK=1\n"},
	)
	got := envMap(v.Environ([]string{"PATH=/bin:/usr/bin"}))
	if got["PATH"] != "/tmp/userbin" {
		t.Errorf("FromWorkspace PATH = %q, want %q", got["PATH"], "/tmp/userbin")
	}
	if got["DYLD_INSERT_LIBRARIES"] != "/tmp/x.dylib" {
		t.Errorf("FromWorkspace DYLD_INSERT_LIBRARIES = %q, want %q", got["DYLD_INSERT_LIBRARIES"], "/tmp/x.dylib")
	}
	if got["OK"] != "1" {
		t.Errorf("OK = %q, want 1", got["OK"])
	}
}

func TestEnvironProjectTomlEnvDoesNotOverridePATH(t *testing.T) {
	v := FromWorkspace(
		domain.Workspace{ID: "W1", ProjectID: "p"},
		domain.Project{
			HasProjectFile: true,
			Env:            "PATH=/tmp/evil\nDYLD_INSERT_LIBRARIES=/tmp/evil.dylib\nOK=1\n",
		},
	)
	got := envMap(v.Environ([]string{"PATH=/bin:/usr/bin"}))
	if got["PATH"] != "/bin:/usr/bin" {
		t.Errorf("FromWorkspace PATH = %q, want host value when project.toml exists", got["PATH"])
	}
	if _, ok := got["DYLD_INSERT_LIBRARIES"]; ok {
		t.Errorf("DYLD_INSERT_LIBRARIES = %q, want absent", got["DYLD_INSERT_LIBRARIES"])
	}
	if got["OK"] != "1" {
		t.Errorf("OK = %q, want 1", got["OK"])
	}
}

func TestEnvironDotenvDoesNotOverridePATH(t *testing.T) {
	repo := t.TempDir()
	writeDotenv(t, repo, ".env", "PATH=/tmp/evil:$PATH\nSHELL=/tmp/evilsh\nHOME=/tmp/fakehome\nDYLD_INSERT_LIBRARIES=/tmp/evil.dylib\nLD_PRELOAD=/tmp/evil.so\nFOO=$PATH\nOK=1\n")
	v := Vars{RepoPath: repo, ProjectID: "p", ProjectEnv: map[string]string{"FOO": "$PATH"}}
	got := envMap(v.Environ([]string{"PATH=/bin:/usr/bin", "SHELL=/bin/zsh", "HOME=/Users/me"}))
	if got["PATH"] != "/bin:/usr/bin" {
		t.Errorf("Environ PATH = %q, want host value", got["PATH"])
	}
	if got["SHELL"] != "/bin/zsh" {
		t.Errorf("Environ SHELL = %q, want host value", got["SHELL"])
	}
	if got["HOME"] != "/Users/me" {
		t.Errorf("Environ HOME = %q, want host value", got["HOME"])
	}
	if _, ok := got["DYLD_INSERT_LIBRARIES"]; ok {
		t.Errorf("DYLD_INSERT_LIBRARIES = %q, want absent", got["DYLD_INSERT_LIBRARIES"])
	}
	if _, ok := got["LD_PRELOAD"]; ok {
		t.Errorf("LD_PRELOAD = %q, want absent", got["LD_PRELOAD"])
	}
	if _, ok := got["OK"]; ok {
		t.Errorf("OK = %q, want absent", got["OK"])
	}
	if strings.Contains(got["FOO"], "/tmp/evil") {
		t.Errorf("FOO=%q, want no .env PATH", got["FOO"])
	}
}

func TestEnvironRecurVarsWinOverDotenv(t *testing.T) {
	repo := t.TempDir()
	writeDotenv(t, repo, ".env", "RECUR_PORT=1\nPORT=1\n")
	v := Vars{Port: 42, RepoPath: repo, ProjectID: "p"}
	got := envMap(v.Environ([]string{"PATH=/bin"}))
	if got["RECUR_PORT"] != "42" || got["PORT"] != "42" {
		t.Errorf("port stolen by .env: RECUR_PORT=%q PORT=%q", got["RECUR_PORT"], got["PORT"])
	}
}

func TestEnvironProjectEnvWinsOverDotenv(t *testing.T) {
	repo := t.TempDir()
	writeDotenv(t, repo, ".env", "FOO=from-file\n")
	v := Vars{
		RepoPath:   repo,
		ProjectEnv: map[string]string{"FOO": "from-toml"},
	}
	got := envMap(v.Environ([]string{"PATH=/bin"}))
	if got["FOO"] != "from-toml" {
		t.Errorf("FOO=%q", got["FOO"])
	}
}
