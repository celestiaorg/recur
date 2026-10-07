package term

import (
	"fmt"
	"strings"
	"testing"
)

func TestTailPlainStripsCSIAndKeepsText(t *testing.T) {
	raw := []byte("hello \x1b[31mred\x1b[0m world")
	got := TailPlain(raw, 50)
	if got != "hello red world" {
		t.Fatalf("got %q", got)
	}
	if StripVT("\x1b[2mdim\x1b[0m \x1b[31mERROR\x1b[0m boom") != "dim ERROR boom" {
		t.Fatalf("StripVT = %q", StripVT("\x1b[2mdim\x1b[0m \x1b[31mERROR\x1b[0m boom"))
	}
}

func TestTailPlainLastLines(t *testing.T) {
	var b strings.Builder
	for i := 1; i <= 10; i++ {
		b.WriteString(strings.Repeat("x", i))
		b.WriteByte('\n')
	}
	got := TailPlain([]byte(b.String()), 3)
	if got != "xxxxxxxx\nxxxxxxxxx\nxxxxxxxxxx" {
		t.Fatalf("got %q", got)
	}
}

func TestTailPlainCollapsesCROverwrite(t *testing.T) {
	got := TailPlain([]byte("a\rb\r\nc"), 10)
	if got != "b\nc" {
		t.Fatalf("got %q", got)
	}
}

func TestTailPlainSpinnerDoesNotEvictError(t *testing.T) {
	var b strings.Builder
	for i := 0; i < 500; i++ {
		fmt.Fprintf(&b, "\rBuilding %d%%", i)
	}
	b.WriteString("\nFATAL: boom\n")
	got := TailPlain([]byte(b.String()), 200)
	if !strings.Contains(got, "FATAL: boom") {
		t.Fatalf("spinner evicted the error: %q", got)
	}
	if strings.Count(got, "Building") > 1 {
		t.Fatalf("spinner frames were not collapsed: %q", got)
	}
}

func TestTailPlainDropsOSC(t *testing.T) {
	raw := []byte("pre\x1b]0;title\x07post")
	got := TailPlain(raw, 10)
	if got != "prepost" {
		t.Fatalf("got %q", got)
	}
}

func TestCollectAndWrapNeedsManager(t *testing.T) {
	// Engine wrap is tested in engine; this just keeps TailPlain defaulting.
	if TailPlain(nil, 0) != "" {
		t.Fatal("empty input")
	}
}
