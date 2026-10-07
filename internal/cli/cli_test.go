package cli

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestBundlesCreateAndTest(t *testing.T) {
	parent := t.TempDir()
	root := newRootCmd()
	root.SetArgs([]string{"bundles", "create", "DemoPack", "--dir", parent})
	var out bytes.Buffer
	root.SetOut(&out)
	root.SetErr(&out)
	if err := root.Execute(); err != nil {
		t.Fatalf("create: %v", err)
	}
	dest := filepath.Join(parent, "DemoPack")
	if _, err := os.Stat(filepath.Join(dest, "bundle.toml")); err != nil {
		t.Fatalf("bundle.toml missing: %v", err)
	}
	for _, rel := range []string{
		"README.md",
		"tests/uppercase.toml",
		"tests/prefix-lines.toml",
		"bin/prefix-lines.sh",
		"src/uppercase.ts",
	} {
		if _, err := os.Stat(filepath.Join(dest, rel)); err != nil {
			t.Fatalf("%s missing: %v", rel, err)
		}
	}
	readme, err := os.ReadFile(filepath.Join(dest, "README.md"))
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"# DemoPack", "bundle.toml", "recur bundles test", "typescript", "shebang"} {
		if !strings.Contains(string(readme), want) {
			t.Fatalf("README missing %q", want)
		}
	}

	if _, err := exec.LookPath("node"); err != nil {
		t.Skip("node not on PATH")
	}
	root = newRootCmd()
	root.SetArgs([]string{"bundles", "test", dest})
	out.Reset()
	root.SetOut(&out)
	root.SetErr(&out)
	if err := root.Execute(); err != nil {
		t.Fatalf("test: %v\n%s", err, out.String())
	}
	got := out.String()
	if !strings.Contains(got, "2 passed") {
		t.Fatalf("want 2 passed, output = %q", got)
	}
}

func TestResolveBundleName(t *testing.T) {
	parent := t.TempDir()
	name := "Named"
	if _, err := createBundle(name, parent, false); err != nil {
		t.Fatal(err)
	}
	t.Setenv("RECUR_BUNDLES_DIR", parent)
	if err := initConfig(); err != nil {
		t.Fatal(err)
	}
	dirs, err := resolveBundleTargets([]string{name})
	if err != nil {
		t.Fatal(err)
	}
	if len(dirs) != 1 || filepath.Base(dirs[0]) != name {
		t.Fatalf("dirs = %v", dirs)
	}
}

func TestCreateRejectsBadName(t *testing.T) {
	_, err := createBundle("bad name", t.TempDir(), false)
	if err == nil {
		t.Fatal("expected invalid name error")
	}
}
