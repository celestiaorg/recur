package bundles_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"testing/fstest"

	"recur/internal/bundles"
)

func TestSeedDefaultsIdempotent(t *testing.T) {
	src := fstest.MapFS{
		"Demo/bundle.toml": &fstest.MapFile{Data: []byte(`[bundle]
name = "Demo"
id = "d1"
version = "1.0.0"
[[command]]
id = "c1"
name = "Hi"
body = "#!/bin/sh\necho hi\n"
`)},
		"Demo/src/a.ts": &fstest.MapFile{Data: []byte(`export default (ctx) => ctx.input`)},
	}
	dest := t.TempDir()
	if err := bundles.SeedDefaults(dest, src, "."); err != nil {
		t.Fatal(err)
	}
	stamp1, err := os.ReadFile(filepath.Join(dest, "Demo", ".recur-seed"))
	if err != nil {
		t.Fatal(err)
	}
	if err := bundles.SeedDefaults(dest, src, "."); err != nil {
		t.Fatal(err)
	}
	stamp2, _ := os.ReadFile(filepath.Join(dest, "Demo", ".recur-seed"))
	if string(stamp1) != string(stamp2) {
		t.Fatalf("stamp changed on idempotent seed")
	}
}

func TestSeedDefaultsPreservesLocalEditsUntilShipChanges(t *testing.T) {
	src := fstest.MapFS{
		"Demo/bundle.toml": &fstest.MapFile{Data: []byte(`[bundle]
name = "Demo"
id = "d1"
version = "1.0.0"
[[command]]
id = "c1"
name = "Hi"
body = "#!/bin/sh\ntrue\n"
`)},
	}
	dest := t.TempDir()
	if err := bundles.SeedDefaults(dest, src, "."); err != nil {
		t.Fatal(err)
	}
	p := filepath.Join(dest, "Demo", "bundle.toml")
	if err := os.WriteFile(p, []byte("edited locally"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Same shipped tree → leave local edit alone.
	if err := bundles.SeedDefaults(dest, src, "."); err != nil {
		t.Fatal(err)
	}
	got, _ := os.ReadFile(p)
	if string(got) != "edited locally" {
		t.Fatalf("expected local edit preserved, got %q", got)
	}
}

func TestSeedDefaultsRefreshesWhenShippedHashChanges(t *testing.T) {
	src := fstest.MapFS{
		"Demo/bundle.toml": &fstest.MapFile{Data: []byte(`[bundle]
name = "Demo"
id = "d1"
version = "1.0.0"
[[command]]
id = "c1"
name = "Hi"
body = "#!/bin/sh\necho v1\n"
`)},
		"Demo/extra.txt": &fstest.MapFile{Data: []byte("old")},
	}
	dest := t.TempDir()
	if err := bundles.SeedDefaults(dest, src, "."); err != nil {
		t.Fatal(err)
	}
	// Local clutter that should disappear on refresh.
	_ = os.WriteFile(filepath.Join(dest, "Demo", "stale.txt"), []byte("gone"), 0o644)

	src2 := fstest.MapFS{
		"Demo/bundle.toml": &fstest.MapFile{Data: []byte(`[bundle]
name = "Demo"
id = "d1"
version = "1.1.0"
[[command]]
id = "c1"
name = "Hi"
body = "#!/bin/sh\necho v2\n"
`)},
		"Demo/extra.txt": &fstest.MapFile{Data: []byte("new")},
	}
	if err := bundles.SeedDefaults(dest, src2, "."); err != nil {
		t.Fatal(err)
	}
	got, _ := os.ReadFile(filepath.Join(dest, "Demo", "bundle.toml"))
	if !strings.Contains(string(got), "1.1.0") || !strings.Contains(string(got), "echo v2") {
		t.Fatalf("dest not refreshed: %s", got)
	}
	if _, err := os.Stat(filepath.Join(dest, "Demo", "stale.txt")); !os.IsNotExist(err) {
		t.Fatal("stale file should be removed on refresh")
	}
	extra, _ := os.ReadFile(filepath.Join(dest, "Demo", "extra.txt"))
	if string(extra) != "new" {
		t.Fatalf("extra.txt: %q", extra)
	}
}

func TestSeedDefaultsRepoBundles(t *testing.T) {
	srcRoot := filepath.Join("..", "..", "bundles")
	if _, err := os.Stat(filepath.Join(srcRoot, "Text", "bundle.toml")); err != nil {
		t.Fatalf("repo bundles/ missing: %v", err)
	}
	dest := t.TempDir()
	if err := bundles.SeedDefaults(dest, os.DirFS(srcRoot), "."); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dest, "Text", "bundle.toml")); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dest, "Text", ".recur-seed")); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dest, "Diff", "bin", "diff-clipboard.sh")); err != nil {
		t.Fatal(err)
	}
}
