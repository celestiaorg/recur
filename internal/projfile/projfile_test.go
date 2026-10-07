package projfile

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestLoadMissingIsZero(t *testing.T) {
	dir := t.TempDir()
	f, err := load(dir)
	if err != nil {
		t.Fatalf("load(%q) error = %v, want nil", dir, err)
	}
	if f.SetupCmd != "" || f.Env != "" || f.Runs != nil {
		t.Fatalf("load(missing) = %+v, want zero File", f)
	}
	if Exists(dir) {
		t.Fatalf("Exists(%q) = true, want false", dir)
	}
}

func TestWriteMissingRepoPathFails(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "no-such-repo")
	err := Write(domain.Project{RepoPath: missing, SetupCmd: "make setup"})
	if err == nil {
		t.Fatalf("Write(%q) error = nil, want missing-repo error", missing)
	}
	if _, statErr := os.Stat(filepath.Join(missing, RelPath)); !os.IsNotExist(statErr) {
		t.Errorf("Write(%q) created %s, want no files", missing, RelPath)
	}
}

func TestWriteRepoPathIsFileFails(t *testing.T) {
	f := filepath.Join(t.TempDir(), "not-a-dir")
	if err := os.WriteFile(f, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	err := Write(domain.Project{RepoPath: f, SetupCmd: "make setup"})
	if err == nil {
		t.Fatalf("Write(%q) error = nil, want not-a-directory error", f)
	}
}

func TestWriteRoundTrip(t *testing.T) {
	dir := t.TempDir()
	p := domain.Project{
		RepoPath:    dir,
		SetupCmd:    "pnpm install",
		TeardownCmd: "rm -rf node_modules",
		RunCommands: []domain.RunCommand{{Name: "dev", Cmd: "pnpm dev --port $RECUR_PORT"}},
		AppURL:      "http://localhost:$RECUR_PORT",
		Env:         "FOO=bar\n# comment\nBAZ=$HOME",
		CopyFiles:   ".env*\napps/**/.env.local",
	}
	if err := Write(p); err != nil {
		t.Fatalf("Write: %v", err)
	}
	path := filepath.Join(dir, RelPath)
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	text := string(data)
	if !strings.Contains(text, "setup_cmd") || !strings.Contains(text, "pnpm install") {
		t.Errorf("file missing setup: %s", text)
	}
	if strings.Contains(text, "dev_cmd") {
		t.Errorf("file should not write leftover dev_cmd: %s", text)
	}
	if strings.Contains(text, `\n`) || !strings.Contains(text, "env = \"\"\"") {
		t.Errorf("Write env should be multiline, got:\n%s", text)
	}
	if !strings.Contains(text, "copy_files") || !strings.Contains(text, ".env*") {
		t.Errorf("file missing copy_files: %s", text)
	}
	if !strings.HasPrefix(text, "# Recur project settings") {
		t.Errorf("missing header comment: %s", text[:min(80, len(text))])
	}
	if !Exists(dir) {
		t.Fatalf("Exists(%q) = false, want true", dir)
	}

	got := Overlay(domain.Project{RepoPath: dir, Name: "keep"})
	if got.Name != "keep" {
		t.Errorf("non-file fields should stay: %+v", got)
	}
	if !got.HasProjectFile {
		t.Errorf("HasProjectFile = false, want true")
	}
	if got.SetupCmd != p.SetupCmd || got.TeardownCmd != p.TeardownCmd || domain.VerifyDevCmd(got) != domain.VerifyDevCmd(p) {
		t.Errorf("cmds: %+v", got)
	}
	if got.AppURL != p.AppURL || got.Env != p.Env || got.CopyFiles != p.CopyFiles {
		t.Errorf("url/env/copy: %+v", got)
	}
}

func TestOverlayOmittedKeysAreEmpty(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	body := "[[run]]\nname = \"dev\"\ncmd = \"make dev\"\n"
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	got := Overlay(domain.Project{
		RepoPath:  dir,
		SetupCmd:  "make setup",
		Env:       "A=1",
		CopyFiles: ".env*",
	})
	if domain.VerifyDevCmd(got) != "make dev" {
		t.Errorf("Overlay VerifyDevCmd = %q, want %q", domain.VerifyDevCmd(got), "make dev")
	}
	if got.SetupCmd != "" {
		t.Errorf("Overlay omitted setup_cmd = %q, want empty (file is source of truth)", got.SetupCmd)
	}
	if got.Env != "" {
		t.Errorf("Overlay omitted env = %q, want empty (file is source of truth)", got.Env)
	}
	if got.CopyFiles != "" {
		t.Errorf("Overlay omitted copy_files = %q, want empty (file is source of truth)", got.CopyFiles)
	}
}

func TestOverlayIgnoresLegacyDevCmd(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte("dev_cmd = \"legacy\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if domain.VerifyDevCmd(got) != "" {
		t.Errorf("Overlay VerifyDevCmd = %q, want empty (legacy toml key ignored)", domain.VerifyDevCmd(got))
	}
	if len(got.RunCommands) != 0 {
		t.Errorf("RunCommands = %+v, want empty", got.RunCommands)
	}
}

func TestOverlayExplicitEmptyClears(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte("setup_cmd = \"\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	got := Overlay(domain.Project{RepoPath: dir, SetupCmd: "make setup"})
	if got.SetupCmd != "" {
		t.Errorf("explicit empty should clear, got %q", got.SetupCmd)
	}
}

func TestOverlayRecurProjectToml(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	src, err := os.ReadFile(filepath.Join("testdata", "project.toml"))
	if err != nil {
		t.Fatalf("read testdata/project.toml: %v", err)
	}
	if err := os.WriteFile(filepath.Join(dir, RelPath), src, 0o644); err != nil {
		t.Fatal(err)
	}
	got := Overlay(domain.Project{RepoPath: dir, Env: "OLD=1"})
	if !got.HasProjectFile {
		t.Fatal("HasProjectFile = false, want true")
	}
	if !strings.Contains(got.SetupCmd, "go mod download") {
		t.Errorf("setup_cmd missing go mod download: %q", got.SetupCmd)
	}
	if got.Env == "OLD=1" || !strings.Contains(got.Env, "RECUR_DATA_DIR") {
		t.Errorf("env not overlaid: %q", got.Env)
	}
	if len(got.RunCommands) < 2 || got.RunCommands[0].Cmd != "wails3 task dev" {
		t.Fatalf("runs: %+v", got.RunCommands)
	}
}

func TestOverlayMalformedYieldsEmptyRunConfig(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte("[[[not toml"), 0o644); err != nil {
		t.Fatal(err)
	}
	got := Overlay(domain.Project{RepoPath: dir, SetupCmd: "keep"})
	if got.SetupCmd != "" {
		t.Errorf("Overlay SetupCmd = %q, want empty when file is malformed", got.SetupCmd)
	}
	if !got.HasProjectFile {
		t.Errorf("HasProjectFile = false, want true for malformed file")
	}
	if got.ProjectFileError == "" {
		t.Errorf("Overlay ProjectFileError = %q, want a parse error", got.ProjectFileError)
	}
}

func TestOverlayMissingFileClearsDBRunConfig(t *testing.T) {
	dir := t.TempDir()
	got := Overlay(domain.Project{
		RepoPath:    dir,
		SetupCmd:    "make setup",
		TeardownCmd: "make clean",
		AppURL:      "http://localhost:3000",
		Env:         "A=1",
		CopyFiles:   ".env*",
		RunCommands: []domain.RunCommand{{Name: "dev", Cmd: "make dev"}},
	})
	if got.SetupCmd != "" || got.TeardownCmd != "" || got.AppURL != "" || got.Env != "" || got.CopyFiles != "" {
		t.Errorf("Overlay(missing) run-config = setup=%q teardown=%q url=%q env=%q copy=%q, want empty",
			got.SetupCmd, got.TeardownCmd, got.AppURL, got.Env, got.CopyFiles)
	}
	if len(got.RunCommands) != 0 {
		t.Errorf("Overlay(missing) RunCommands = %+v, want empty", got.RunCommands)
	}
	if got.HasProjectFile {
		t.Errorf("Overlay(missing) HasProjectFile = true, want false")
	}
	if got.ProjectFileError != "" {
		t.Errorf("Overlay(missing) ProjectFileError = %q, want empty", got.ProjectFileError)
	}
}

func TestLoadTildePath(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	repo := filepath.Join(home, "code", "app")
	if err := os.MkdirAll(repo, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := Write(domain.Project{RepoPath: repo, RunCommands: []domain.RunCommand{{Name: "dev", Cmd: "go run ."}}}); err != nil {
		t.Fatal(err)
	}
	got := Overlay(domain.Project{RepoPath: "~/code/app"})
	if domain.VerifyDevCmd(got) != "go run ." {
		t.Errorf("tilde overlay VerifyDevCmd = %q, want %q", domain.VerifyDevCmd(got), "go run .")
	}
}

func TestOverlayNamedRuns(t *testing.T) {
	dir := t.TempDir()
	body := `
dev_cmd = "legacy"
[[run]]
name = "dev"
cmd = "wails3 task dev"
[[run]]
name = "test"
cmd = "go test ./..."
`
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	got := Overlay(domain.Project{RepoPath: dir, PinnedRun: "test"})
	if got.PinnedRun != "test" {
		t.Errorf("pinned should stay machine-local: %q", got.PinnedRun)
	}
	if len(got.RunCommands) != 2 || got.RunCommands[1].Name != "test" {
		t.Fatalf("runs: %+v", got.RunCommands)
	}
	if domain.VerifyDevCmd(got) != "wails3 task dev" {
		t.Errorf("VerifyDevCmd should follow named dev: %q", domain.VerifyDevCmd(got))
	}
}

func TestWriteNamedRuns(t *testing.T) {
	dir := t.TempDir()
	p := domain.Project{
		RepoPath: dir,
		RunCommands: []domain.RunCommand{
			{Name: "dev", Cmd: "wails3 task dev"},
			{Name: "test", Cmd: "go test ./..."},
		},
	}
	if err := Write(p); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(filepath.Join(dir, RelPath))
	if err != nil {
		t.Fatal(err)
	}
	text := string(data)
	if !strings.Contains(text, "[[run]]") || !strings.Contains(text, "go test") {
		t.Fatalf("missing [[run]]:\n%s", text)
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if len(got.RunCommands) != 2 || got.RunCommands[1].Cmd != "go test ./..." {
		t.Fatalf("%+v", got.RunCommands)
	}
}

func TestWriteMultilineEnv(t *testing.T) {
	dir := t.TempDir()
	env := "RECUR_DATA_DIR=$HOME/tmp\nWAILS_VITE_PORT=$RECUR_PORT\n"
	if err := Write(domain.Project{RepoPath: dir, Env: env}); err != nil {
		t.Fatalf("Write: %v", err)
	}
	text := readProjectToml(t, dir)
	if strings.Contains(text, `\n`) {
		t.Errorf("Write env used escaped newlines, want multiline:\n%s", text)
	}
	if !strings.Contains(text, "env = \"\"\"") {
		t.Errorf("Write env missing multiline quotes:\n%s", text)
	}
	if !strings.Contains(text, "WAILS_VITE_PORT=$RECUR_PORT") {
		t.Errorf("Write env missing value:\n%s", text)
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if got.Env != env {
		t.Errorf("Overlay Env = %q, want %q", got.Env, env)
	}
}

func TestWriteMultilineRunCmd(t *testing.T) {
	dir := t.TempDir()
	cmd := "go test ./...\ngo vet ./...\n"
	if err := Write(domain.Project{
		RepoPath: dir,
		RunCommands: []domain.RunCommand{
			{Name: "check", Cmd: cmd},
		},
	}); err != nil {
		t.Fatalf("Write: %v", err)
	}
	text := readProjectToml(t, dir)
	if strings.Contains(text, `\n`) {
		t.Errorf("Write run cmd used escaped newlines, want multiline:\n%s", text)
	}
	if !strings.Contains(text, "cmd = \"\"\"") {
		t.Errorf("Write run cmd missing multiline quotes:\n%s", text)
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if len(got.RunCommands) != 1 || got.RunCommands[0].Cmd != strings.TrimSpace(cmd) {
		t.Errorf("Overlay RunCommands = %+v, want check with multiline cmd", got.RunCommands)
	}
}

func TestWriteMergesMultilineRunCmd(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	hand := `# keep
[[run]]
name = "dev"
cmd = "make dev"
`
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte(hand), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Write(domain.Project{
		RepoPath: dir,
		RunCommands: []domain.RunCommand{
			{Name: "dev", Cmd: "make setup\nmake dev\n"},
		},
	}); err != nil {
		t.Fatalf("Write: %v", err)
	}
	text := readProjectToml(t, dir)
	if !strings.Contains(text, "# keep") {
		t.Errorf("Write dropped comment:\n%s", text)
	}
	if !strings.Contains(text, "cmd = \"\"\"") || strings.Contains(text, `\n`) {
		t.Errorf("merged run cmd should be multiline:\n%s", text)
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if len(got.RunCommands) != 1 || got.RunCommands[0].Cmd != "make setup\nmake dev" {
		t.Errorf("Overlay RunCommands = %+v, want multiline dev cmd", got.RunCommands)
	}
}

func TestWriteMergesCommentsAndUnknownKeys(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	hand := `# team notes

# secrets stay in .env
env = """
FOO=old
"""

extra = "keep-me"

[[run]]
# local override
name = "dev"
cmd = "make dev"
`
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte(hand), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Write(domain.Project{
		RepoPath: dir,
		Env:      "FOO=new\nBAR=2\n",
		RunCommands: []domain.RunCommand{
			{Name: "dev", Cmd: "make dev"},
			{Name: "test", Cmd: "go test"},
		},
	}); err != nil {
		t.Fatalf("Write: %v", err)
	}
	text := readProjectToml(t, dir)
	for _, want := range []string{"# team notes", "# secrets stay in .env", `extra = "keep-me"`, "# local override"} {
		if !strings.Contains(text, want) {
			t.Errorf("Write dropped %q:\n%s", want, text)
		}
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if got.Env != "FOO=new\nBAR=2\n" {
		t.Errorf("Overlay Env = %q, want updated multiline", got.Env)
	}
	if len(got.RunCommands) != 2 || got.RunCommands[1].Cmd != "go test" {
		t.Errorf("Overlay RunCommands = %+v, want appended test run", got.RunCommands)
	}
}

func TestWriteAddsRunsWhenFileHasNone(t *testing.T) {
	dir := t.TempDir()
	if err := Write(domain.Project{RepoPath: dir, SetupCmd: "make setup", Env: "A=1"}); err != nil {
		t.Fatalf("Write(no runs): %v", err)
	}
	text := readProjectToml(t, dir)
	if strings.Contains(text, "[[run]]") || strings.Contains(text, "[run.") {
		t.Fatalf("Write(no runs) should omit run tables:\n%s", text)
	}
	if err := Write(domain.Project{
		RepoPath: dir,
		SetupCmd: "make setup",
		Env:      "A=1",
		RunCommands: []domain.RunCommand{
			{Name: "dev", Cmd: "make dev"},
		},
	}); err != nil {
		t.Fatalf("Write(add run): %v", err)
	}
	text = readProjectToml(t, dir)
	if strings.Contains(text, "[run.0]") {
		t.Errorf("Write added [run.0] tables, want [[run]]:\n%s", text)
	}
	if !strings.Contains(text, "[[run]]") || !strings.Contains(text, "make dev") {
		t.Errorf("Write missing [[run]] after adding a run:\n%s", text)
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if len(got.RunCommands) != 1 || got.RunCommands[0].Name != "dev" || got.RunCommands[0].Cmd != "make dev" {
		t.Errorf("Overlay RunCommands = %+v, want [{dev make dev}]", got.RunCommands)
	}
}

func TestWriteHealsIndexedRunTables(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	broken := "setup_cmd = 'make setup'\n\n[run.0]\nname = 'dev'\ncmd = 'old'\n"
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte(broken), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Write(domain.Project{
		RepoPath: dir,
		SetupCmd: "make setup",
		RunCommands: []domain.RunCommand{
			{Name: "dev", Cmd: "make dev"},
		},
	}); err != nil {
		t.Fatalf("Write(heal [run.0]): %v", err)
	}
	text := readProjectToml(t, dir)
	if strings.Contains(text, "[run.0]") {
		t.Errorf("Write left [run.0] tables, want [[run]]:\n%s", text)
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if len(got.RunCommands) != 1 || got.RunCommands[0].Cmd != "make dev" {
		t.Errorf("Overlay RunCommands = %+v, want healed [{dev make dev}]", got.RunCommands)
	}
}

func TestWriteMalformedRewrites(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, ".recur"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, RelPath), []byte("[[[not toml"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Write(domain.Project{RepoPath: dir, Env: "A=1"}); err != nil {
		t.Fatalf("Write: %v", err)
	}
	text := readProjectToml(t, dir)
	if !strings.HasPrefix(text, "# Recur project settings") {
		t.Errorf("malformed rewrite missing header: %s", text[:min(80, len(text))])
	}
	got := Overlay(domain.Project{RepoPath: dir})
	if got.Env != "A=1" {
		t.Errorf("Overlay Env = %q, want %q", got.Env, "A=1")
	}
}

func readProjectToml(t *testing.T, repo string) string {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(repo, RelPath))
	if err != nil {
		t.Fatalf("read %s: %v", RelPath, err)
	}
	return string(data)
}
