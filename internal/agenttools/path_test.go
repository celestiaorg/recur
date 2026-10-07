package agenttools

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestResolvePathSandbox(t *testing.T) {
	wt := t.TempDir()
	if err := os.WriteFile(filepath.Join(wt, "in.txt"), []byte("ok"), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := resolvePath(wt, "in.txt")
	if err != nil {
		t.Fatalf("resolvePath(in.txt) error = %v, want nil", err)
	}
	if !strings.HasSuffix(got, "in.txt") {
		t.Errorf("resolvePath(in.txt) = %q, want …/in.txt", got)
	}
	if _, err := resolvePath(wt, "../nope"); err == nil {
		t.Fatal("resolvePath(../nope) error = nil, want escape error")
	}
	outside := t.TempDir()
	if _, err := resolveWritePath(wt, filepath.Join(outside, "x.txt")); err == nil {
		t.Fatal("resolveWritePath(abs-outside) error = nil, want escape error")
	}

	if err := os.Symlink(outside, filepath.Join(wt, "link")); err != nil {
		t.Fatal(err)
	}
	if _, err := resolveWritePath(wt, "link/newdir/file.txt"); err == nil {
		t.Fatal("resolveWritePath through symlink-out-of-tree error = nil, want escape error")
	}
}
