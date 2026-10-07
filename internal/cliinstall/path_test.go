package cliinstall

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestMergePATHPrependsExisting(t *testing.T) {
	extra := t.TempDir()
	missing := filepath.Join(t.TempDir(), "nope")
	got := mergePATH("/usr/bin:/bin", []string{extra, missing})
	want := extra + ":/usr/bin:/bin"
	if got != want {
		t.Errorf("mergePATH(/usr/bin:/bin, [%s, missing]) = %q, want %q", extra, got, want)
	}
}

func TestMergePATHSkipsAlreadyPresent(t *testing.T) {
	extra := t.TempDir()
	path := extra + ":/usr/bin"
	got := mergePATH(path, []string{extra})
	if got != path {
		t.Errorf("mergePATH(%q, [%s]) = %q, want unchanged", path, extra, got)
	}
}

func TestMergePATHDedupsExtra(t *testing.T) {
	extra := t.TempDir()
	got := mergePATH("/bin", []string{extra, extra})
	want := extra + ":/bin"
	if got != want {
		t.Errorf("mergePATH(/bin, [%s, %s]) = %q, want %q", extra, extra, got, want)
	}
}

func TestMergePATHEmptyBase(t *testing.T) {
	extra := t.TempDir()
	got := mergePATH("", []string{extra})
	if got != extra {
		t.Errorf("mergePATH(\"\", [%s]) = %q, want %q", extra, got, extra)
	}
}

func TestExtraPATHDirsIncludesCommon(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	got := extraPATHDirs()
	want := []string{
		filepath.Join(home, ".local", "bin"),
		filepath.Join(home, "bin"),
		filepath.Join(home, ".cargo", "bin"),
		filepath.Join(home, "go", "bin"),
		SystemBinDir(),
		"/opt/homebrew/bin",
		"/opt/homebrew/sbin",
		"/usr/local/sbin",
		"/opt/local/bin",
	}
	for _, dir := range want {
		if !contains(got, dir) {
			t.Errorf("extraPATHDirs() = %v, want to contain %q", got, dir)
		}
	}
	local := filepath.Join(home, ".local", "bin")
	if indexOf(got, local) > indexOf(got, SystemBinDir()) {
		t.Errorf("extraPATHDirs() = %v, %q should precede %q", got, local, SystemBinDir())
	}
}

func TestReadPathHelperDirs(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "etc", "paths.d"), 0o755); err != nil {
		t.Fatalf("mkdir paths.d: %v", err)
	}
	if err := os.WriteFile(filepath.Join(root, "etc", "paths"), []byte("/usr/local/bin\n/usr/bin\n# comment\n\n"), 0o644); err != nil {
		t.Fatalf("write /etc/paths: %v", err)
	}
	if err := os.WriteFile(filepath.Join(root, "etc", "paths.d", "homebrew"), []byte("/opt/homebrew/bin\n"), 0o644); err != nil {
		t.Fatalf("write paths.d/homebrew: %v", err)
	}
	if err := os.WriteFile(filepath.Join(root, "etc", "paths.d", "zzz-extra"), []byte("/opt/extra/bin\n"), 0o644); err != nil {
		t.Fatalf("write paths.d/zzz-extra: %v", err)
	}
	got := readPathHelperDirs(root)
	want := []string{"/usr/local/bin", "/usr/bin", "/opt/homebrew/bin", "/opt/extra/bin"}
	if len(got) != len(want) {
		t.Fatalf("readPathHelperDirs(%s) = %v, want %v", root, got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("readPathHelperDirs(%s)[%d] = %q, want %q", root, i, got[i], want[i])
		}
	}
}

func TestEnsurePATHAddsLocalBin(t *testing.T) {
	stubLoginPATH(t, "")
	home := t.TempDir()
	t.Setenv("HOME", home)
	local := filepath.Join(home, ".local", "bin")
	if err := os.MkdirAll(local, 0o755); err != nil {
		t.Fatalf("mkdir %s: %v", local, err)
	}
	t.Setenv("PATH", "/usr/bin:/bin")
	EnsurePATH()
	got := filepath.SplitList(os.Getenv("PATH"))
	idx := indexOf(got, local)
	if idx < 0 {
		t.Errorf("EnsurePATH() PATH = %q, want to contain %q", os.Getenv("PATH"), local)
	}
	usr := indexOf(got, "/usr/bin")
	if idx >= 0 && usr >= 0 && idx > usr {
		t.Errorf("EnsurePATH() PATH = %q, %q should precede /usr/bin", os.Getenv("PATH"), local)
	}
}

func TestEnsurePATHMakesLookPathFindClaude(t *testing.T) {
	stubLoginPATH(t, "")
	home := t.TempDir()
	t.Setenv("HOME", home)
	local := filepath.Join(home, ".local", "bin")
	if err := os.MkdirAll(local, 0o755); err != nil {
		t.Fatalf("mkdir %s: %v", local, err)
	}
	bin := filepath.Join(local, "claude")
	if err := os.WriteFile(bin, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatalf("write %s: %v", bin, err)
	}
	t.Setenv("PATH", "/usr/bin:/bin")
	if _, err := exec.LookPath("claude"); err == nil {
		t.Fatal("LookPath(claude) succeeded before EnsurePATH, want not found")
	}
	EnsurePATH()
	got, err := exec.LookPath("claude")
	if err != nil {
		t.Fatalf("LookPath(claude) after EnsurePATH: %v", err)
	}
	if got != bin {
		t.Errorf("LookPath(claude) = %q, want %q", got, bin)
	}
}

func TestEnsurePATHSkipsMissingLocalBin(t *testing.T) {
	stubLoginPATH(t, "")
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("PATH", "/usr/bin:/bin")
	EnsurePATH()
	local := filepath.Join(home, ".local", "bin")
	if contains(filepath.SplitList(os.Getenv("PATH")), local) {
		t.Errorf("EnsurePATH() PATH = %q, should omit missing %q", os.Getenv("PATH"), local)
	}
}

func TestEnsurePATHUsesLoginShellPATH(t *testing.T) {
	stubLoginPATH(t, "/from/shell:/usr/bin")
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("PATH", "/usr/bin:/bin")
	EnsurePATH()
	if !contains(filepath.SplitList(os.Getenv("PATH")), "/from/shell") {
		t.Errorf("EnsurePATH() PATH = %q, want to contain /from/shell from the login probe", os.Getenv("PATH"))
	}
}

func stubLoginPATH(t *testing.T, path string) {
	t.Helper()
	orig := readLoginPATH
	readLoginPATH = func() string { return path }
	t.Cleanup(func() { readLoginPATH = orig })
}

func contains(parts []string, want string) bool {
	return indexOf(parts, want) >= 0
}

func indexOf(parts []string, want string) int {
	for i, p := range parts {
		if p == want {
			return i
		}
	}
	return -1
}
