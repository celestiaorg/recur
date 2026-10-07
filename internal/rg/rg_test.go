package rg

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestFilesArgsIncludesUUAndGlobs(t *testing.T) {
	args := FilesArgs("/tmp/repo")
	joined := filepath.Join(args...)
	_ = joined
	want := []string{"--files", "-uu", "--glob", "!.git", "--glob", "!node_modules"}
	for _, w := range want {
		found := false
		for _, a := range args {
			if a == w {
				found = true
				break
			}
		}
		if !found {
			t.Fatalf("FilesArgs = %v, missing %q", args, w)
		}
	}
}

func TestFilesListsWithSystemRg(t *testing.T) {
	rgPath, err := exec.LookPath("rg")
	if err != nil {
		t.Skip("rg not on PATH")
	}
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "validators"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "validators", "hyperlane_validator2.sh"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "hypothesis.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Hidden file should be included with -uu.
	if err := os.WriteFile(filepath.Join(root, ".env.example"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}

	var got []string
	if err := Files(context.Background(), rgPath, root, nil, func(rel string) error {
		got = append(got, rel)
		return nil
	}); err != nil {
		t.Fatalf("Files: %v", err)
	}
	seen := map[string]bool{}
	for _, p := range got {
		seen[p] = true
	}
	for _, want := range []string{"validators/hyperlane_validator2.sh", "hypothesis.txt", ".env.example"} {
		if !seen[want] {
			t.Fatalf("Files = %v, missing %q", got, want)
		}
	}
}

func TestLocateEnvOverride(t *testing.T) {
	dir := t.TempDir()
	fake := filepath.Join(dir, "fake-rg")
	if err := os.WriteFile(fake, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(EnvPath, fake)
	got, err := Locate("")
	if err != nil {
		t.Fatal(err)
	}
	if got != fake {
		t.Fatalf("Locate = %q, want %q", got, fake)
	}
}
