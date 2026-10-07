package git

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestWorkingDiffModified(t *testing.T) {
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
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\nfunc X() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	fd, err := WorkingDiff(context.Background(), root, "main", "a.go")
	if err != nil {
		t.Fatal(err)
	}
	if fd.Status != "m" {
		t.Errorf("status=%q want m", fd.Status)
	}
	if fd.Diff.Add < 1 {
		t.Errorf("expected additions, got %+v", fd.Diff)
	}
}

func TestWorkingDiffCleanPathNoCachedOutsideRepo(t *testing.T) {
	// Outside a git dir, `git diff --cached` errors with
	// "unknown option `cached'" / usage: git diff --no-index. WorkingDiff must
	// not invoke that path when porcelain status is empty.
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	_, err := WorkingDiff(context.Background(), root, "", "a.txt")
	if err == nil {
		t.Fatal("expected error for non-git dir")
	}
	if strings.Contains(err.Error(), "cached") || strings.Contains(err.Error(), "no-index") {
		t.Fatalf("error should not be the --cached/--no-index failure: %v", err)
	}
}

func TestWorkingDiffUntracked(t *testing.T) {
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
	if err := os.WriteFile(filepath.Join(root, "seed.go"), []byte("package s\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")
	if err := os.WriteFile(filepath.Join(root, "new.go"), []byte("package n\nfunc Y() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	fd, err := WorkingDiff(context.Background(), root, "main", "new.go")
	if err != nil {
		t.Fatal(err)
	}
	if fd.Status != "a" {
		t.Errorf("status=%q want a", fd.Status)
	}
	if fd.Diff.Add < 1 || len(fd.Lines) < 2 {
		t.Fatalf("expected add lines, got %+v lines=%d", fd.Diff, len(fd.Lines))
	}
}

func TestFileAtBase(t *testing.T) {
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
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")
	run("checkout", "-b", "recur/feat")
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\nfunc X() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "feat")

	base, err := FileAtBase(context.Background(), root, "main", "a.go")
	if err != nil {
		t.Fatal(err)
	}
	if base != "package a\n" {
		t.Fatalf("base=%q want package a", base)
	}
	missing, err := FileAtBase(context.Background(), root, "main", "nope.go")
	if err != nil || missing != "" {
		t.Fatalf("missing: %q %v", missing, err)
	}

	against := MergeBase(context.Background(), root, "main")
	size, ok, err := FileSizeAtRev(context.Background(), root, against, "a.go")
	if err != nil || !ok {
		t.Fatalf("FileSizeAtRev: ok=%v err=%v", ok, err)
	}
	if size != int64(len("package a\n")) {
		t.Fatalf("FileSizeAtRev=%d want %d", size, len("package a\n"))
	}
	var buf bytes.Buffer
	copied, err := CopyFileAtRev(context.Background(), root, against, "a.go", &buf, 1<<20)
	if err != nil || !copied {
		t.Fatalf("CopyFileAtRev: ok=%v err=%v", copied, err)
	}
	if buf.String() != "package a\n" {
		t.Fatalf("CopyFileAtRev=%q", buf.String())
	}
	_, ok, err = FileSizeAtRev(context.Background(), root, against, "nope.go")
	if err != nil || ok {
		t.Fatalf("FileSizeAtRev missing: ok=%v err=%v", ok, err)
	}
}

func TestChangeStatusVersusBaseIncludesCommitted(t *testing.T) {
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
	if err := os.WriteFile(filepath.Join(root, "base.go"), []byte("package b\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "base")
	run("checkout", "-b", "recur/feat")
	if err := os.WriteFile(filepath.Join(root, "feat.go"), []byte("package f\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "feat") // committed on loop branch; porcelain clean

	st := ChangeStatus(context.Background(), root, "main")
	if st["feat.go"] != "a" {
		t.Fatalf("expected feat.go=a versus main, got %#v", st)
	}
	if porcelain := StatusMap(context.Background(), root); len(porcelain) != 0 {
		t.Fatalf("expected clean porcelain, got %#v", porcelain)
	}

	fd, err := WorkingDiff(context.Background(), root, "main", "feat.go")
	if err != nil {
		t.Fatal(err)
	}
	if fd.Status != "a" || fd.Diff.Add < 1 {
		t.Fatalf("diff versus base: %+v", fd)
	}
}

func TestSyntheticAddDiffSkipsBinary(t *testing.T) {
	bin := syntheticAddDiff("x.png", "\x00PNG", false)
	if bin.Status != "a" {
		t.Errorf("status=%q want a", bin.Status)
	}
	if len(bin.Lines) != 0 {
		t.Errorf("binary synthetic add should have no lines, got %d", len(bin.Lines))
	}
	text := syntheticAddDiff("a.txt", "hello\n", false)
	if len(text.Lines) < 2 {
		t.Fatalf("text synthetic add should have hunk+line, got %d", len(text.Lines))
	}
}
