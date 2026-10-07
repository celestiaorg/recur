package engine

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestWrapComposerDialectSkipsRawRegions(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "README.md"), []byte("hi"), 0o644); err != nil {
		t.Fatal(err)
	}
	in := "please read @README.md\n\n⟦raw⟧\nsee @README.md and /go-code-review in the diff\n⟦/raw⟧\n"
	got := wrapComposerDialect(in, root, nil)
	if strings.Contains(got, "⟦raw⟧") || strings.Contains(got, "⟦/raw⟧") || strings.Contains(got, "⟦rawph:") {
		t.Fatalf("markers should be stripped: %q", got)
	}
	if !strings.Contains(got, "<system_instruction>") {
		t.Fatalf("expected attach wrap for outer @README.md: %q", got)
	}
	if strings.Count(got, " as @README.md") != 1 {
		t.Fatalf("raw-region @README.md must not attach again: %q", got)
	}
	if !strings.Contains(got, "see @README.md and /go-code-review in the diff") {
		t.Fatalf("raw body should remain: %q", got)
	}
	if !strings.Contains(got, "please read @README.md") {
		t.Fatalf("outer body lost: %q", got)
	}
}

func TestWrapComposerDialectNonceIgnoresQuotedBareMarkers(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "app.go"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Diff quotes bare markers (as docs/bundles.md does) plus a real @path later.
	in := "intro @app.go\n⟦raw:a1b2⟧\n" +
		"docs say use `⟦raw⟧`…`⟦/raw⟧` around blobs\n" +
		"and also @app.go /go-code-review inside the diff\n" +
		"⟦/raw:a1b2⟧\n"
	got := wrapComposerDialect(in, root, nil)
	if strings.Contains(got, "⟦raw:a1b2⟧") || strings.Contains(got, "⟦/raw:a1b2⟧") || strings.Contains(got, "⟦rawph:") {
		t.Fatalf("active nonce markers should be stripped: %q", got)
	}
	if strings.Count(got, " as @app.go") != 1 {
		t.Fatalf("inner @app.go must not attach; preamble count=%d in %q", strings.Count(got, " as @app.go"), got)
	}
	// Bare quotes inside the nonced region must survive for the model to read.
	if !strings.Contains(got, "docs say use `⟦raw⟧`…`⟦/raw⟧` around blobs") {
		t.Fatalf("quoted bare markers inside region should remain in body: %q", got)
	}
	if !strings.Contains(got, "and also @app.go /go-code-review inside the diff") {
		t.Fatalf("tail of raw region lost (early close?): %q", got)
	}
}

func TestWrapComposerDialectStripsStrayMarkers(t *testing.T) {
	got := wrapComposerDialect("hello ⟦/raw⟧ world ⟦raw:dead⟧", "", nil)
	if strings.Contains(got, "⟦") {
		t.Fatalf("stray markers remain: %q", got)
	}
	if got != "hello  world " {
		t.Fatalf("got=%q", got)
	}
}

func TestWrapComposerDialectNoRawUnchangedPath(t *testing.T) {
	plain := "hello with no mentions"
	if wrapComposerDialect(plain, t.TempDir(), nil) != plain {
		t.Fatal("plain text should round-trip")
	}
}

func TestWrapComposerDialectDoesNotWrapBareArtifact(t *testing.T) {
	in := "check @artifact please"
	got := wrapComposerDialect(in, t.TempDir(), nil)
	if got != in {
		t.Fatalf("empty workspace must not wrap @artifact: %q", got)
	}
}
