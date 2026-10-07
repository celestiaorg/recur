package store

import (
	"bytes"
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestTeeAppLogWritesAndRestores(t *testing.T) {
	root := t.TempDir()
	var buf bytes.Buffer
	prev := log.Writer()
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(prev) })

	_, cleanup := TeeAppLog(root)
	t.Cleanup(cleanup)

	log.Printf("hello from TeeAppLog")

	path := AppLogPath(root)
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("ReadFile(%s): %v", path, err)
	}
	got := string(body)
	if !strings.Contains(got, "hello from TeeAppLog") {
		t.Errorf("TeeAppLog(%q) file = %q, want it to contain %q", root, got, "hello from TeeAppLog")
	}
	if !strings.Contains(got, "recur: logging to") {
		t.Errorf("TeeAppLog(%q) file = %q, want a boot path line", root, got)
	}
	if !strings.Contains(buf.String(), "hello from TeeAppLog") {
		t.Errorf("TeeAppLog(%q) previous writer = %q, want it to contain %q", root, buf.String(), "hello from TeeAppLog")
	}

	cleanup()
	if log.Writer() != &buf {
		t.Errorf("TeeAppLog cleanup Writer() = %T, want the previous *bytes.Buffer", log.Writer())
	}
}

func TestTeeAppLogPreservesMultilinePrintf(t *testing.T) {
	root := t.TempDir()
	prev := log.Writer()
	wantFlags := log.Flags()
	log.SetOutput(io.Discard)
	t.Cleanup(func() { log.SetOutput(prev) })

	_, cleanup := TeeAppLog(root)
	t.Cleanup(cleanup)
	if got := log.Flags(); got != wantFlags {
		t.Errorf("TeeAppLog Flags() = %d, want %d (stdlib prefix must stay)", got, wantFlags)
	}

	log.Printf("recur: loop %s panicked: %v\n%s", "L1", "boom", "goroutine 1 [running]:")
	body, err := os.ReadFile(AppLogPath(root))
	if err != nil {
		t.Fatalf("ReadFile(%s): %v", AppLogPath(root), err)
	}
	got := string(body)
	if strings.Contains(got, `msg="`) || strings.Contains(got, `\ngoroutine`) {
		t.Errorf("TeeAppLog(%q) reformatted through slog: %q", root, got)
	}
	if !strings.Contains(got, "\ngoroutine 1 [running]:") {
		t.Errorf("TeeAppLog(%q) = %q, want a real newline before the stack", root, got)
	}
}

func TestTeeAppLogRotatesOversizedFile(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "logs"), 0o755); err != nil {
		t.Fatal(err)
	}
	path := AppLogPath(root)
	old := bytes.Repeat([]byte("x"), appLogMaxBytes)
	if err := os.WriteFile(path, old, 0o600); err != nil {
		t.Fatal(err)
	}

	prev := log.Writer()
	log.SetOutput(io.Discard)
	t.Cleanup(func() { log.SetOutput(prev) })

	_, cleanup := TeeAppLog(root)
	t.Cleanup(cleanup)
	log.Printf("after rotate")

	rotated, err := os.ReadFile(path + ".old")
	if err != nil {
		t.Fatalf("ReadFile(%s.old): %v", path, err)
	}
	if !bytes.Equal(rotated, old) {
		t.Errorf("TeeAppLog rotate: %s.old len = %d, want %d (prior contents)", path, len(rotated), len(old))
	}
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("ReadFile(%s): %v", path, err)
	}
	if !strings.Contains(string(body), "after rotate") {
		t.Errorf("TeeAppLog(%q) after rotate = %q, want it to contain %q", root, body, "after rotate")
	}
	if bytes.Contains(body, old[:32]) {
		t.Errorf("TeeAppLog(%q) new log still holds pre-rotate payload", root)
	}
}

func TestRotatingFileRotatesMidSession(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, appLogFile)
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	r := &rotatingFile{f: f, path: path, max: 64}
	t.Cleanup(func() { _ = r.Close() })

	first := []byte("hello\n")
	if n, err := r.Write(first); err != nil || n != len(first) {
		t.Fatalf("Write(%q) = %d, %v, want %d, nil", first, n, err, len(first))
	}
	second := bytes.Repeat([]byte("x"), 64)
	if n, err := r.Write(second); err != nil || n != len(second) {
		t.Fatalf("Write(64 x) = %d, %v, want 64, nil", n, err)
	}

	rotated, err := os.ReadFile(path + ".old")
	if err != nil {
		t.Fatalf("ReadFile(%s.old): %v", path, err)
	}
	if !bytes.Equal(rotated, first) {
		t.Errorf("rotatingFile mid-session .old = %q, want %q", rotated, first)
	}
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("ReadFile(%s): %v", path, err)
	}
	if !bytes.Equal(body, second) {
		t.Errorf("rotatingFile mid-session new file = %q, want 64 x's", body)
	}
}

func TestTeeAppLogEmptyRootIsNoop(t *testing.T) {
	prev := log.Writer()
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(prev) })

	w, cleanup := TeeAppLog("")
	cleanup()
	if w != nil {
		t.Errorf("TeeAppLog(\"\") writer = %T, want nil", w)
	}
	if log.Writer() != &buf {
		t.Errorf("TeeAppLog(\"\") Writer() = %T, want unchanged *bytes.Buffer", log.Writer())
	}
}

func TestTeeAppLogFailOpen(t *testing.T) {
	// Parent is a file, so LogsDir cannot create <root>/logs.
	dir := t.TempDir()
	root := filepath.Join(dir, "blocked")
	if err := os.WriteFile(root, []byte("not a dir"), 0o600); err != nil {
		t.Fatal(err)
	}

	prev := log.Writer()
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(prev) })

	w, cleanup := TeeAppLog(root)
	t.Cleanup(cleanup)
	if w != nil {
		t.Errorf("TeeAppLog(%q) writer = %T, want nil on failure", root, w)
	}
	if log.Writer() != &buf {
		t.Errorf("TeeAppLog(%q) Writer() = %T, want unchanged on failure", root, log.Writer())
	}
	log.Printf("still works")
	if !strings.Contains(buf.String(), "still works") {
		t.Errorf("TeeAppLog(%q) previous writer = %q, want it to contain %q", root, buf.String(), "still works")
	}
}
