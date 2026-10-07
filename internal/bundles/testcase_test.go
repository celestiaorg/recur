package bundles_test

import (
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestLoadTestsFilePayloads(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "tests", "fixtures", "in.txt"), "hello\n")
	mustWrite(t, filepath.Join(dir, "tests", "fixtures", "out.txt"), "HELLO\n")
	mustWrite(t, filepath.Join(dir, "tests", "case.toml"), `
[[test]]
name = "from files"
command = "upper-ts"
[test.context]
inputFile = "fixtures/in.txt"
selectedTextFile = "fixtures/in.txt"
[test.expect]
stdoutFile = "fixtures/out.txt"
output = "replaceSelection"
`)
	tests, err := bundles.LoadTests(dir)
	if err != nil {
		t.Fatalf("LoadTests: %v", err)
	}
	if len(tests) != 1 {
		t.Fatalf("len = %d, want 1", len(tests))
	}
	tc := tests[0].Case
	if tc.Context.Input != "hello\n" {
		t.Fatalf("Input = %q, want %q", tc.Context.Input, "hello\n")
	}
	if tc.Expect.Stdout != "HELLO\n" {
		t.Fatalf("Stdout = %q, want %q", tc.Expect.Stdout, "HELLO\n")
	}
	if tc.Context.InputFile != "" || tc.Expect.StdoutFile != "" {
		t.Fatalf("file fields should be cleared after resolve")
	}
}

func TestLoadTestsRejectsInlineAndFile(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "tests", "in.txt"), "x")
	mustWrite(t, filepath.Join(dir, "tests", "bad.toml"), `
[[test]]
name = "both"
command = "c"
[test.context]
input = "inline"
inputFile = "in.txt"
[test.expect]
stdout = "x"
`)
	_, err := bundles.LoadTests(dir)
	if err == nil {
		t.Fatal("expected error for inline+file")
	}
	if !strings.Contains(err.Error(), "not both") {
		t.Fatalf("err = %v, want not both", err)
	}
}
