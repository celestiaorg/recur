package code

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestListOpenInTargetsIncludesFileManager(t *testing.T) {
	targets := ListOpenInTargets()
	if runtime.GOOS != "darwin" {
		if len(targets) == 0 || targets[0].ID != "finder" || targets[0].Name != "Files" {
			t.Fatalf("ListOpenInTargets() = %v, want finder/Files first", targets)
		}
		return
	}
	if len(targets) == 0 {
		t.Fatal("ListOpenInTargets() empty, want Finder")
	}
	if targets[0].ID != "finder" || targets[0].Name != "Finder" {
		t.Fatalf("first target: got %+v, want finder/Finder", targets[0])
	}
	foundTerm := false
	for _, tgt := range targets {
		if tgt.ID == "terminal" {
			foundTerm = true
			break
		}
	}
	if !foundTerm {
		t.Fatal("darwin catalog should include Terminal")
	}
}

func TestListOpenInTargetsFiltersInstalled(t *testing.T) {
	origMac := macAppInstalled
	origLP := lookPath
	t.Cleanup(func() {
		macAppInstalled = origMac
		lookPath = origLP
	})
	macAppInstalled = func(names ...string) bool {
		for _, n := range names {
			if n == "Cursor.app" {
				return true
			}
		}
		return false
	}
	lookPath = func(string) (string, error) { return "", exec.ErrNotFound }

	ids := map[string]bool{}
	for _, tgt := range ListOpenInTargets() {
		ids[tgt.ID] = true
	}
	if runtime.GOOS != "darwin" {
		// .app bundles only count on macOS; nothing is on PATH here.
		if len(ids) != 1 || !ids["finder"] {
			t.Fatalf("ListOpenInTargets() = %v, want only finder", ids)
		}
		return
	}
	if !ids["finder"] {
		t.Fatal("ListOpenInTargets() missing finder")
	}
	if !ids["cursor"] {
		t.Fatal("cursor should be listed when Cursor.app is present")
	}
	if ids["vscode"] || ids["zed"] || ids["ghostty"] {
		t.Fatalf("uninstalled apps leaked: %v", ids)
	}
	if runtime.GOOS == "darwin" && !ids["terminal"] {
		t.Fatal("terminal is always-on on darwin")
	}
}

func TestListOpenInTargetsIncludesOSAppIcons(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("NSWorkspace icons are darwin")
	}
	targets := ListOpenInTargets()
	if len(targets) == 0 || targets[0].ID != "finder" {
		t.Fatal("expected Finder")
	}
	if !strings.HasPrefix(targets[0].Icon, "data:image/png;base64,") {
		t.Fatalf("Finder icon: %q", trunc(targets[0].Icon, 40))
	}
	// Cached second call should match.
	again := ListOpenInTargets()
	if again[0].Icon != targets[0].Icon {
		t.Fatal("icon cache should be stable")
	}
}

func trunc(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}

func TestOpenInFinder(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("open folder is macOS-only")
	}
	dir := t.TempDir()
	var gotName string
	var gotArgs []string
	orig := startDetached
	startDetached = func(name string, args ...string) error {
		gotName = name
		gotArgs = append([]string{}, args...)
		return nil
	}
	t.Cleanup(func() { startDetached = orig })

	if err := OpenIn(dir, "finder"); err != nil {
		t.Fatalf("OpenIn finder: %v", err)
	}
	if gotName != "open" || len(gotArgs) != 1 || gotArgs[0] != dir {
		t.Fatalf("OpenIn(finder) = %s %v, want open [%s]", gotName, gotArgs, dir)
	}
}

func TestOpenInRequiresDarwin(t *testing.T) {
	if runtime.GOOS == "darwin" {
		t.Skip("macOS is supported")
	}
	dir := t.TempDir()
	if err := OpenIn(dir, "finder"); err == nil {
		t.Fatal("OpenIn(finder) error = nil, want error off macOS")
	}
}

func TestOpenInCursorUsesOpenA(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("open -a is darwin")
	}
	dir := t.TempDir()
	origMac := macAppInstalled
	origStart := startDetached
	t.Cleanup(func() {
		macAppInstalled = origMac
		startDetached = origStart
	})
	macAppInstalled = func(names ...string) bool {
		for _, n := range names {
			if n == "Cursor.app" {
				return true
			}
		}
		return false
	}
	var gotName string
	var gotArgs []string
	startDetached = func(name string, args ...string) error {
		gotName = name
		gotArgs = append([]string{}, args...)
		return nil
	}

	if err := OpenIn(dir, "cursor"); err != nil {
		t.Fatalf("OpenIn cursor: %v", err)
	}
	if gotName != "open" || len(gotArgs) != 3 || gotArgs[0] != "-a" || gotArgs[1] != "Cursor" || gotArgs[2] != dir {
		t.Fatalf("got %s %v", gotName, gotArgs)
	}
}

func TestOpenInUnknownAndMissing(t *testing.T) {
	dir := t.TempDir()
	if err := OpenIn(dir, "nope"); err == nil {
		t.Fatal("unknown target should fail")
	}
	if err := OpenIn(filepath.Join(dir, "missing"), "finder"); err == nil {
		t.Fatal("missing path should fail")
	}
	file := filepath.Join(dir, "file.txt")
	if err := os.WriteFile(file, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := OpenIn(file, "finder"); err == nil {
		t.Fatal("file (not dir) should fail")
	}
}

func TestOpenInPropagatesStartError(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("open is macOS-only")
	}
	dir := t.TempDir()
	orig := startDetached
	startDetached = func(string, ...string) error { return errors.New("boom") }
	t.Cleanup(func() { startDetached = orig })
	if err := OpenIn(dir, "finder"); err == nil {
		t.Fatal("expected start error")
	}
}

func TestOpenInGhosttyBinPassesWorkingDirectory(t *testing.T) {
	dir := t.TempDir()
	origMac, origLook, origStart := macAppInstalled, lookPath, startDetached
	t.Cleanup(func() { macAppInstalled, lookPath, startDetached = origMac, origLook, origStart })
	macAppInstalled = func(...string) bool { return false }
	lookPath = func(bin string) (string, error) { return "/usr/bin/" + bin, nil }
	var gotName string
	var gotArgs []string
	startDetached = func(name string, args ...string) error {
		gotName, gotArgs = name, append([]string{}, args...)
		return nil
	}

	if err := OpenIn(dir, "ghostty"); err != nil {
		t.Fatalf("OpenIn ghostty: %v", err)
	}
	if gotName != "/usr/bin/ghostty" || len(gotArgs) != 1 || gotArgs[0] != "--working-directory="+dir {
		t.Fatalf("got %s %v", gotName, gotArgs)
	}
}
