package copyfiles

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestParsePatterns(t *testing.T) {
	got := ParsePatterns("\n.env*\n# comment\n  apps/**/.env.local  \n\\foo\\bar\n")
	want := []string{".env*", "apps/**/.env.local", "foo/bar"}
	if len(got) != len(want) {
		t.Fatalf("ParsePatterns = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("ParsePatterns[%d] = %q, want %q", i, got[i], want[i])
		}
	}
}

func TestMatchDotenvAnyDepth(t *testing.T) {
	pats := []string{".env*"}
	for _, rel := range []string{".env", ".env.local", "apps/web/.env", "deploy/.env.mainnet"} {
		if !Match(rel, pats) {
			t.Errorf("Match(%q, .env*) = false, want true", rel)
		}
	}
	for _, rel := range []string{"README.md", "src/env.ts", "apps/web/config.json"} {
		if Match(rel, pats) {
			t.Errorf("Match(%q, .env*) = true, want false", rel)
		}
	}
}

func TestMatchRootRelative(t *testing.T) {
	pats := []string{"apps/.env"}
	if !Match("apps/.env", pats) {
		t.Fatal("Match(apps/.env, apps/.env) = false")
	}
	if Match("apps/web/.env", pats) {
		t.Fatal("Match(apps/web/.env, apps/.env) = true, want false (no **)")
	}
	if Match(".env", pats) {
		t.Fatal("Match(.env, apps/.env) = true, want false")
	}
}

func TestMatchDoubleStar(t *testing.T) {
	pats := []string{"apps/**/.env.local"}
	if !Match("apps/web/.env.local", pats) {
		t.Fatal("expected apps/web/.env.local to match apps/**/.env.local")
	}
	if !Match("apps/.env.local", pats) {
		t.Fatal("expected apps/.env.local to match apps/**/.env.local")
	}
	if Match("other/.env.local", pats) {
		t.Fatal("other/.env.local should not match apps/**/.env.local")
	}
}

func TestCopyAndPreview(t *testing.T) {
	src := t.TempDir()
	dest := t.TempDir()
	write := func(rel, body string) {
		t.Helper()
		p := filepath.Join(src, filepath.FromSlash(rel))
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write(".env", "A=1\n")
	write("apps/web/.env.local", "B=2\n")
	write("deploy/.env.mainnet", "C=3\n")
	write("README.md", "hello\n")
	write(filepath.Join("node_modules", ".env"), "SKIP=1\n")
	if err := os.MkdirAll(filepath.Join(src, ".git", "objects"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(src, ".git", ".env"), []byte("nope\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	prev := Preview(context.Background(), src, ".env*\n")
	if prev.Error != "" {
		t.Fatalf("Preview error: %s", prev.Error)
	}
	if prev.Count != 3 {
		t.Fatalf("Preview count = %d files=%v, want 3", prev.Count, prev.Files)
	}

	n, err := Copy(context.Background(), src, dest, ".env*")
	if err != nil {
		t.Fatalf("Copy: %v", err)
	}
	if n != 3 {
		t.Fatalf("Copy n = %d, want 3", n)
	}
	for _, rel := range []string{".env", "apps/web/.env.local", "deploy/.env.mainnet"} {
		got, err := os.ReadFile(filepath.Join(dest, filepath.FromSlash(rel)))
		if err != nil {
			t.Fatalf("read dest %s: %v", rel, err)
		}
		want, _ := os.ReadFile(filepath.Join(src, filepath.FromSlash(rel)))
		if string(got) != string(want) {
			t.Errorf("dest %s = %q, want %q", rel, got, want)
		}
	}
	if _, err := os.Stat(filepath.Join(dest, "README.md")); !os.IsNotExist(err) {
		t.Fatal("README.md should not be copied")
	}
	if _, err := os.Stat(filepath.Join(dest, "node_modules", ".env")); !os.IsNotExist(err) {
		t.Fatal("node_modules/.env should be skipped unless the glob names it")
	}
	if _, err := os.Stat(filepath.Join(dest, ".git", ".env")); !os.IsNotExist(err) {
		t.Fatal(".git/.env must never be copied")
	}
}

func TestCopyEmptyPatternsIsNoop(t *testing.T) {
	n, err := Copy(context.Background(), t.TempDir(), t.TempDir(), "\n# none\n")
	if err != nil || n != 0 {
		t.Fatalf("Copy empty = %d, %v", n, err)
	}
}

func TestPreviewMissingRoot(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "missing")
	prev := Preview(context.Background(), missing, ".env*")
	want := "Checkout not found at " + missing
	if prev.Error != want {
		t.Fatalf("Preview Error = %q, want %q", prev.Error, want)
	}
	if prev.Count != 0 {
		t.Fatalf("Preview missing count = %d", prev.Count)
	}
}

func TestMatchDirAnyDepth(t *testing.T) {
	pats := []string{"secrets/"}
	if !Match("secrets/a.txt", pats) {
		t.Fatal("Match(secrets/a.txt, secrets/) = false, want true")
	}
	if !Match("apps/secrets/a.txt", pats) {
		t.Fatal("Match(apps/secrets/a.txt, secrets/) = false, want true (any depth)")
	}
	if Match("secret/a.txt", pats) {
		t.Fatal("Match(secret/a.txt, secrets/) = true, want false")
	}
}

func TestNegationIsRejected(t *testing.T) {
	text := ".env*\n!.env.example\n"
	if got := ParsePatterns(text); len(got) != 1 || got[0] != ".env*" {
		t.Fatalf("ParsePatterns(%q) = %v, want [.env*]", text, got)
	}
	prev := Preview(context.Background(), t.TempDir(), text)
	if prev.Error != errNegation.Error() {
		t.Fatalf("Preview Error = %q, want %q", prev.Error, errNegation.Error())
	}
	n, err := Copy(context.Background(), t.TempDir(), t.TempDir(), text)
	if n != 0 || !errors.Is(err, errNegation) {
		t.Fatalf("Copy negation = %d, %v, want 0, %v", n, err, errNegation)
	}
}

func TestCopyDotDotInFilename(t *testing.T) {
	src := t.TempDir()
	dest := t.TempDir()
	if err := os.WriteFile(filepath.Join(src, ".env"), []byte("A=1\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(src, ".env..bak"), []byte("B=2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	n, err := Copy(context.Background(), src, dest, ".env*")
	if err != nil {
		t.Fatalf("Copy: %v", err)
	}
	if n != 2 {
		t.Fatalf("Copy n = %d, want 2 (including .env..bak)", n)
	}
	got, err := os.ReadFile(filepath.Join(dest, ".env..bak"))
	if err != nil {
		t.Fatalf("read .env..bak: %v", err)
	}
	if string(got) != "B=2\n" {
		t.Errorf(".env..bak = %q, want B=2", got)
	}
}

func TestCopyLeavesExistingDest(t *testing.T) {
	src := t.TempDir()
	dest := t.TempDir()
	if err := os.WriteFile(filepath.Join(src, ".env"), []byte("from-src\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dest, ".env"), []byte("from-dest\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	n, err := Copy(context.Background(), src, dest, ".env")
	if err != nil {
		t.Fatalf("Copy: %v", err)
	}
	if n != 0 {
		t.Fatalf("Copy n = %d, want 0 (dest already had .env)", n)
	}
	got, err := os.ReadFile(filepath.Join(dest, ".env"))
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != "from-dest\n" {
		t.Errorf("dest .env = %q, want from-dest (not overwritten)", got)
	}
}

func TestCopyRespectsCancel(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	n, err := Copy(ctx, t.TempDir(), t.TempDir(), ".env*")
	if n != 0 || !errors.Is(err, context.Canceled) {
		t.Fatalf("Copy cancelled = %d, %v, want 0, context.Canceled", n, err)
	}
}

func TestPreviewAndCopyShareScanCap(t *testing.T) {
	old := scanLimit
	scanLimit = 2
	t.Cleanup(func() { scanLimit = old })

	src := t.TempDir()
	for _, name := range []string{".env", ".env.local", ".env.prod"} {
		if err := os.WriteFile(filepath.Join(src, name), []byte("x\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	prev := Preview(context.Background(), src, ".env*")
	if prev.Error == "" || !strings.Contains(prev.Error, "more than 2 matching files") {
		t.Fatalf("Preview Error = %q, want more-than-cap", prev.Error)
	}
	n, err := Copy(context.Background(), src, t.TempDir(), ".env*")
	if n != 0 || err == nil || !strings.Contains(err.Error(), "more than 2 matching files") {
		t.Fatalf("Copy = %d, %v, want cap error", n, err)
	}
}

func chmodUnreadable(t *testing.T, path string) {
	t.Helper()
	if err := os.Chmod(path, 0); err != nil {
		t.Fatalf("Chmod(%s, 0): %v", path, err)
	}
	t.Cleanup(func() { _ = os.Chmod(path, 0o644) })
	f, err := os.Open(path)
	if err == nil {
		f.Close()
		t.Skip("process can read mode-0000 files")
	}
}

func TestCopySkipsUnreadableWithoutFailing(t *testing.T) {
	src := t.TempDir()
	dest := t.TempDir()
	path := filepath.Join(src, ".env")
	if err := os.WriteFile(path, []byte("secret\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	chmodUnreadable(t, path)
	n, err := Copy(context.Background(), src, dest, ".env*")
	if err != nil {
		t.Fatalf("Copy unreadable = %d, %v, want 0, nil", n, err)
	}
	if n != 0 {
		t.Fatalf("Copy unreadable n = %d, want 0", n)
	}
	if _, err := os.Stat(filepath.Join(dest, ".env")); !os.IsNotExist(err) {
		t.Fatal("unreadable .env should not appear in dest")
	}
}

func TestCopyExistingPlusUnreadableIsOK(t *testing.T) {
	src := t.TempDir()
	dest := t.TempDir()
	if err := os.WriteFile(filepath.Join(src, ".env"), []byte("from-src\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dest, ".env"), []byte("from-dest\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	bad := filepath.Join(src, ".env.bad")
	if err := os.WriteFile(bad, []byte("nope\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	chmodUnreadable(t, bad)
	n, err := Copy(context.Background(), src, dest, ".env*")
	if err != nil {
		t.Fatalf("Copy existing+unreadable = %d, %v, want 0, nil", n, err)
	}
	if n != 0 {
		t.Fatalf("Copy existing+unreadable n = %d, want 0", n)
	}
	got, err := os.ReadFile(filepath.Join(dest, ".env"))
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != "from-dest\n" {
		t.Errorf("dest .env = %q, want from-dest", got)
	}
}

func TestPreviewCancel(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	prev := Preview(ctx, t.TempDir(), ".env*")
	if prev.Error != "Scan timed out — narrow the globs" {
		t.Fatalf("Preview Error = %q, want timed-out copy", prev.Error)
	}
}
