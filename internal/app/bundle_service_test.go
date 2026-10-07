package app

import (
	"os"
	"path/filepath"
	"testing"
)

func TestInvalidateCatalogRereadsScriptBodies(t *testing.T) {
	data := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", data)
	t.Setenv("HOME", t.TempDir())
	dir := filepath.Join(data, "Bundles", "A")
	if err := os.MkdirAll(filepath.Join(dir, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(`
[bundle]
name = "A"
id = "11111111-1111-4111-8111-111111111111"
[[command]]
id = "11111111-1111-4111-8111-111111111201"
name = "Run"
category = "Text"
interpreter = "typescript"
script = "src/x.ts"
`), 0o644); err != nil {
		t.Fatal(err)
	}
	script := filepath.Join(dir, "src", "x.ts")
	s := &BundleService{}
	read := func() string {
		t.Helper()
		cmds, _, err := s.discoverAll()
		if err != nil || len(cmds) != 1 {
			t.Fatalf("discover: %v, %d commands", err, len(cmds))
		}
		return cmds[0].Body
	}
	if err := os.WriteFile(script, []byte("old"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := read(); got != "old" {
		t.Fatalf("body = %q", got)
	}
	if err := os.WriteFile(script, []byte("new"), 0o644); err != nil {
		t.Fatal(err)
	}
	s.InvalidateCatalog()
	if got := read(); got != "new" {
		t.Fatalf("after invalidate, body = %q", got)
	}
}
