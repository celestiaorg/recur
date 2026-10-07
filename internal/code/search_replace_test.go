package code

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestReplaceInFilesCaseInsensitive(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "a.go")
	if err := os.WriteFile(path, []byte("Foo foo FOO\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res, err := ReplaceInFiles(t.Context(), root, "foo", "bar", domain.SearchOpts{})
	if err != nil {
		t.Fatal(err)
	}
	if res.FilesChanged != 1 || res.Replacements != 3 {
		t.Fatalf("got %+v", res)
	}
	data, _ := os.ReadFile(path)
	if string(data) != "bar bar bar\n" {
		t.Fatalf("content=%q", data)
	}
}

func TestSearchOptsRegexAndGlob(t *testing.T) {
	root := t.TempDir()
	_ = os.WriteFile(filepath.Join(root, "a.go"), []byte("func Alpha()\n"), 0o644)
	_ = os.WriteFile(filepath.Join(root, "b.txt"), []byte("func Alpha()\n"), 0o644)
	res, err := SearchOpts(t.Context(), root, `func\s+A\w+`, domain.SearchOpts{Regex: true, Glob: "*.go"})
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Hits) != 1 || res.Hits[0].Path != "a.go" {
		t.Fatalf("hits=%+v", res)
	}
	if res.Truncated || res.SkippedLarge != 0 {
		t.Fatalf("unexpected truncation/skips: %+v", res)
	}
}

// Files over the per-file size cap are skipped; the result must say so rather
// than reporting a silent "no matches".
func TestSearchOptsReportsSkippedLargeFiles(t *testing.T) {
	root := t.TempDir()
	big := strings.Repeat("needle\n", (maxSearchFileBytes/7)+16)
	if err := os.WriteFile(filepath.Join(root, "big.go"), []byte(big), 0o644); err != nil {
		t.Fatal(err)
	}
	res, err := SearchOpts(t.Context(), root, "needle", domain.SearchOpts{})
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Hits) != 0 {
		t.Fatalf("expected the oversized file to be skipped, got %+v", res.Hits)
	}
	if res.SkippedLarge != 1 {
		t.Fatalf("SkippedLarge = %d, want 1", res.SkippedLarge)
	}
}

// A cancelled context must abort the walk instead of scanning the whole repo.
func TestSearchOptsHonoursCancellation(t *testing.T) {
	root := t.TempDir()
	_ = os.WriteFile(filepath.Join(root, "a.go"), []byte("needle\n"), 0o644)
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	if _, err := SearchOpts(ctx, root, "needle", domain.SearchOpts{}); !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want context.Canceled", err)
	}
}

func TestListFilesAndFuzzy(t *testing.T) {
	root := t.TempDir()
	_ = os.MkdirAll(filepath.Join(root, "pkg"), 0o755)
	_ = os.WriteFile(filepath.Join(root, "pkg", "x.go"), []byte("package pkg\n"), 0o644)
	_ = os.WriteFile(filepath.Join(root, "README.md"), []byte("#\n"), 0o644)
	files, err := ListFiles(t.Context(), root)
	if err != nil {
		t.Fatal(err)
	}
	if len(files) < 2 {
		t.Fatalf("files=%v", files)
	}
	m := MatchFiles(files, "pkg/x", 10)
	if len(m) != 1 || m[0] != "pkg/x.go" {
		t.Fatalf("match=%v", m)
	}
}
