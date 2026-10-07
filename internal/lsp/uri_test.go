package lsp

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestCanonPathResolvesSymlink(t *testing.T) {
	dir := t.TempDir()
	real := filepath.Join(dir, "real")
	if err := os.Mkdir(real, 0o755); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(dir, "link")
	if err := os.Symlink(real, link); err != nil {
		t.Fatal(err)
	}
	got := CanonPath(link)
	want, err := filepath.EvalSymlinks(link)
	if err != nil {
		t.Fatal(err)
	}
	if got != want {
		t.Fatalf("got %q want %q", got, want)
	}
}

func TestPathToURIUnix(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("unix URI shape")
	}
	u := PathToURI("/Users/josh/proj")
	if !strings.HasPrefix(u, "file:///") {
		t.Fatalf("uri=%s", u)
	}
	if !strings.Contains(u, "/Users/josh/proj") {
		t.Fatalf("uri=%s", u)
	}
}
