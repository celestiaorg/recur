package osfonts

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestMonospaceFamiliesIncludesInconsolataWhenPresent(t *testing.T) {
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatal(err)
	}
	incon := filepath.Join(home, "Library", "Fonts", "Inconsolata-Regular.ttf")
	if runtime.GOOS != "darwin" {
		t.Skip("Inconsolata fixture path is macOS-specific")
	}
	if _, err := os.Stat(incon); err != nil {
		t.Skip("Inconsolata not installed in ~/Library/Fonts")
	}

	fams := MonospaceFamilies()
	found := false
	for _, f := range fams {
		if strings.EqualFold(f, "Inconsolata") {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("expected Inconsolata in monospace list, got %v", fams)
	}
}

func TestInspectInconsolata(t *testing.T) {
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatal(err)
	}
	incon := filepath.Join(home, "Library", "Fonts", "Inconsolata-Regular.ttf")
	if _, err := os.Stat(incon); err != nil {
		t.Skip("Inconsolata not installed")
	}
	family, mono, err := inspectFont(incon)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.EqualFold(family, "Inconsolata") {
		t.Fatalf("family=%q", family)
	}
	if !mono {
		t.Fatalf("expected post.isFixedPitch for Inconsolata (mono=%v)", mono)
	}
}

func TestInspectArialNotMono(t *testing.T) {
	candidates := []string{
		"/Library/Fonts/Arial.ttf",
		"/System/Library/Fonts/Supplemental/Arial.ttf",
		"/Library/Fonts/Arial Unicode.ttf",
	}
	var path string
	for _, c := range candidates {
		if _, err := os.Stat(c); err == nil {
			path = c
			break
		}
	}
	if path == "" {
		t.Skip("Arial not found")
	}
	_, mono, err := inspectFont(path)
	if err != nil {
		t.Fatal(err)
	}
	if mono {
		t.Fatalf("Arial should not be fixed-pitch")
	}
}

func TestMonospaceFamiliesExcludesArial(t *testing.T) {
	for _, f := range MonospaceFamilies() {
		if strings.EqualFold(f, "Arial") || strings.EqualFold(f, "Comic Sans MS") {
			t.Fatalf("proportional face %q leaked into monospace list", f)
		}
	}
}
