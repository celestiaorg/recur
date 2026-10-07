package term

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestOSC7ParserExtractsFileURI(t *testing.T) {
	dir := t.TempDir()
	p := &osc7Parser{}
	seq := []byte("\x1b]7;file://" + dir + "\a")
	cwd, ok := p.feed(append([]byte("hello"), append(seq, []byte("world")...)...))
	if !ok {
		t.Fatal("expected a cwd from OSC 7")
	}
	if cwd != dir {
		t.Errorf("cwd = %q, want %q", cwd, dir)
	}
}

func TestOSC7ParserSTTerminatedAndUnescapes(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "has space")
	if err := os.Mkdir(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	p := &osc7Parser{}
	seq := []byte("\x1b]7;file://" + strings.ReplaceAll(dir, " ", "%20") + "\x1b\\")
	cwd, ok := p.feed(seq)
	if !ok || cwd != dir {
		t.Fatalf("cwd = %q ok=%v, want %q", cwd, ok, dir)
	}
}

func TestOSC7ParserCarriesSplitSequence(t *testing.T) {
	dir := t.TempDir()
	p := &osc7Parser{}
	seq := []byte("\x1b]7;file://" + dir + "\a")
	if cwd, ok := p.feed(seq[:5]); ok || cwd != "" {
		t.Fatalf("incomplete prefix should not yield cwd, got %q ok=%v", cwd, ok)
	}
	cwd, ok := p.feed(seq[5:])
	if !ok || cwd != dir {
		t.Fatalf("cwd after split = %q ok=%v, want %q", cwd, ok, dir)
	}
}

func TestOSC7ParserIgnoresMissingDirectory(t *testing.T) {
	p := &osc7Parser{}
	seq := []byte("\x1b]7;file:///definitely/not/a/real/recur/dir\a")
	if cwd, ok := p.feed(seq); ok || cwd != "" {
		t.Fatalf("missing dir should be ignored, got %q ok=%v", cwd, ok)
	}
}

func TestOSC7ParserLastCompleteWins(t *testing.T) {
	a := t.TempDir()
	b := t.TempDir()
	p := &osc7Parser{}
	in := append([]byte("\x1b]7;file://"+a+"\a"), []byte("\x1b]7;file://"+b+"\a")...)
	cwd, ok := p.feed(in)
	if !ok || cwd != b {
		t.Fatalf("cwd = %q ok=%v, want last path %q", cwd, ok, b)
	}
}

func TestOSC7ParserIgnoresNonLocalHost(t *testing.T) {
	dir := t.TempDir()
	p := &osc7Parser{}
	seq := []byte("\x1b]7;file://evil.example" + dir + "\a")
	if cwd, ok := p.feed(seq); ok || cwd != "" {
		t.Fatalf("osc7Path remote host = %q ok=%v, want rejected", cwd, ok)
	}
}

func TestOSC7ParserAcceptsLocalhostAndHostname(t *testing.T) {
	dir := t.TempDir()
	p := &osc7Parser{}
	seq := []byte("\x1b]7;file://localhost" + dir + "\a")
	cwd, ok := p.feed(seq)
	if !ok || cwd != dir {
		t.Fatalf("osc7Path localhost = %q ok=%v, want %q", cwd, ok, dir)
	}
	host, err := os.Hostname()
	if err != nil {
		t.Fatalf("Hostname: %v", err)
	}
	p = &osc7Parser{}
	seq = []byte("\x1b]7;file://" + host + dir + "\a")
	cwd, ok = p.feed(seq)
	if !ok || cwd != dir {
		t.Fatalf("osc7Path hostname %q = %q ok=%v, want %q", host, cwd, ok, dir)
	}
}
