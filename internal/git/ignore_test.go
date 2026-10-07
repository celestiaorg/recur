package git

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestParseCheckIgnore(t *testing.T) {
	got := ParseCheckIgnore([]byte("node_modules\x00.env\x00weird name.txt\x00"))
	for _, p := range []string{"node_modules", ".env", "weird name.txt"} {
		if !got[p] {
			t.Errorf("ParseCheckIgnore(...)[%q] = false, want true (got %#v)", p, got)
		}
	}
	if ParseCheckIgnore(nil) != nil {
		t.Fatal("ParseCheckIgnore(nil) = non-nil, want nil")
	}
}

func TestIgnoredSet(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	root := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root}, args...)...)
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	if err := os.WriteFile(filepath.Join(root, ".gitignore"), []byte("scratch.bin\nbuild/\n*.log\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "tracked.go"), []byte("package t\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(root, "build"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "build", "out.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "scratch.bin"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "café.log"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "  spaced.log"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	nlName := "we\nird.log"
	if err := os.WriteFile(filepath.Join(root, nlName), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".gitignore", "tracked.go")
	run("commit", "-m", "init")

	got := IgnoredSet(t.Context(), root, []string{
		".gitignore", "tracked.go", "scratch.bin", "build", "build/out.txt",
		"café.log", "  spaced.log", nlName, "", ".",
	})
	want := map[string]bool{
		"scratch.bin":   true,
		"build":         true,
		"build/out.txt": true,
		"café.log":      true,
		"  spaced.log":  true,
		nlName:          true,
	}
	for p, ignored := range want {
		if got[p] != ignored {
			t.Errorf("IgnoredSet(...)[%q] = %v, want %v (got %#v)", p, got[p], ignored, got)
		}
	}
	if got[".gitignore"] {
		t.Errorf("IgnoredSet(...)[.gitignore] = true, want false (tracked)")
	}
	if got["tracked.go"] {
		t.Errorf("IgnoredSet(...)[tracked.go] = true, want false")
	}
	if got["ird.log"] {
		t.Errorf("IgnoredSet(...)[ird.log] = true, want false (newline-name must not split)")
	}

	if IgnoredSet(t.Context(), root, nil) != nil {
		t.Fatal("IgnoredSet(empty paths) = non-nil, want nil")
	}
	if IgnoredSet(t.Context(), t.TempDir(), []string{"foo"}) != nil {
		t.Fatal("IgnoredSet(non-git dir) = non-nil, want nil")
	}
}
