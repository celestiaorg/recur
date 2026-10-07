package bundles_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"recur/internal/bundles"
)

// TestShippedBundleTests runs tests/*.toml for every repo bundles/* pack that
// has cases — keeps the authoring CLI honest against the reference packs.
func TestShippedBundleTests(t *testing.T) {
	if _, err := exec.LookPath("node"); err != nil {
		t.Skip("node not on PATH")
	}
	root := filepath.Join("..", "..", "bundles")
	entries, err := os.ReadDir(root)
	if err != nil {
		t.Fatalf("read bundles/: %v", err)
	}
	cache := t.TempDir()
	ctx := context.Background()
	var any bool
	for _, e := range entries {
		if !e.IsDir() || e.Name() == "sdk" {
			continue
		}
		dir := filepath.Join(root, e.Name())
		if _, err := os.Stat(filepath.Join(dir, "bundle.toml")); err != nil {
			continue
		}
		tests, err := bundles.LoadTests(dir)
		if err != nil {
			t.Fatalf("LoadTests(%s): %v", e.Name(), err)
		}
		if len(tests) == 0 {
			continue
		}
		any = true
		rep, err := bundles.RunBundleTests(ctx, dir, cache)
		if err != nil {
			t.Fatalf("RunBundleTests(%s): %v", e.Name(), err)
		}
		if !rep.OK() {
			t.Errorf("%s:\n%s", e.Name(), bundles.FormatReport(rep, false))
		}
	}
	if !any {
		t.Fatal("expected at least one shipped bundle with tests/")
	}
}

// TestShippedCommandsHaveTests fails if a shipped [[command]] is missing a
// tests/*.toml case (matched by command id or name).
func TestShippedCommandsHaveTests(t *testing.T) {
	root := filepath.Join("..", "..", "bundles")
	entries, err := os.ReadDir(root)
	if err != nil {
		t.Fatalf("read bundles/: %v", err)
	}
	var missing int
	for _, e := range entries {
		if !e.IsDir() || e.Name() == "sdk" {
			continue
		}
		dir := filepath.Join(root, e.Name())
		data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
		if err != nil {
			continue
		}
		_, cmds, _, err := bundles.ParseManifest(dir, data)
		if err != nil {
			t.Fatalf("ParseManifest(%s): %v", e.Name(), err)
		}
		if len(cmds) == 0 {
			continue
		}
		tests, err := bundles.LoadTests(dir)
		if err != nil {
			t.Fatalf("LoadTests(%s): %v", e.Name(), err)
		}
		covered := make(map[string]bool, len(tests)*2)
		for _, lt := range tests {
			covered[lt.Case.Command] = true
		}
		for _, c := range cmds {
			if covered[c.ID] || covered[c.Name] {
				continue
			}
			missing++
			t.Errorf("%s: %s (%s) has no tests/*.toml case", e.Name(), c.Name, c.ID)
		}
	}
	if missing > 0 {
		t.Fatalf("%d shipped command(s) lack tests", missing)
	}
}

// TestShippedSnippetsHaveTests fails if a shipped [[snippet]] is missing a
// tests/*.toml case (matched by snippet id).
func TestShippedSnippetsHaveTests(t *testing.T) {
	root := filepath.Join("..", "..", "bundles")
	entries, err := os.ReadDir(root)
	if err != nil {
		t.Fatalf("read bundles/: %v", err)
	}
	var missing int
	for _, e := range entries {
		if !e.IsDir() || e.Name() == "sdk" {
			continue
		}
		dir := filepath.Join(root, e.Name())
		data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
		if err != nil {
			continue
		}
		parsed, err := bundles.ParseManifestFull(dir, data)
		if err != nil {
			t.Fatalf("ParseManifestFull(%s): %v", e.Name(), err)
		}
		if len(parsed.Snippets) == 0 {
			continue
		}
		tests, err := bundles.LoadTests(dir)
		if err != nil {
			t.Fatalf("LoadTests(%s): %v", e.Name(), err)
		}
		covered := make(map[string]bool, len(tests))
		for _, lt := range tests {
			if lt.Case.Snippet != "" {
				covered[lt.Case.Snippet] = true
			}
		}
		for _, s := range parsed.Snippets {
			if covered[s.ID] {
				continue
			}
			missing++
			t.Errorf("%s: %s (%s) has no tests/*.toml snippet case", e.Name(), s.Name, s.ID)
		}
	}
	if missing > 0 {
		t.Fatalf("%d shipped snippet(s) lack tests", missing)
	}
}
