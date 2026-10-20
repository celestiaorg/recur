package cursor

import (
	"archive/zip"
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestPinnedSDKVersionInPyPIURL(t *testing.T) {
	if pinnedSDKVersion == "" || pinnedSDKVersion == "latest" {
		t.Fatal("pinnedSDKVersion must be a concrete release")
	}
	if !strings.Contains(pypiPinnedProject, "/cursor-sdk/"+pinnedSDKVersion+"/json") {
		t.Fatalf("pypiPinnedProject = %q, want …/cursor-sdk/%s/json", pypiPinnedProject, pinnedSDKVersion)
	}
	// Attribution anchors were validated against this release — keep the pin
	// and sdkAttributionAnchors in lockstep when bumping.
	if pinnedSDKVersion != "1.0.38" {
		t.Fatalf("pinnedSDKVersion = %q; update this test (and attribution anchors) deliberately when bumping", pinnedSDKVersion)
	}
	if got := PinnedSDKVersion(); got != pinnedSDKVersion {
		t.Fatalf("PinnedSDKVersion() = %q, want %q", got, pinnedSDKVersion)
	}
}

// TestCheckCursorSDKPinDrift fails when PyPI's latest cursor-sdk is ahead of
// pinnedSDKVersion. Opt-in (network); not part of plain go test ./...:
//
//	RECUR_CURSOR_SDK_PIN_CHECK=1 go test ./internal/cursor -run CheckCursorSDKPinDrift -count=1
//
// Or: wails3 task check:cursor-sdk-pin
//
// When this fails, re-pin per .agents/skills/repin-cursor-sdk/SKILL.md.
func TestCheckCursorSDKPinDrift(t *testing.T) {
	if os.Getenv("RECUR_CURSOR_SDK_PIN_CHECK") == "" {
		t.Skip("set RECUR_CURSOR_SDK_PIN_CHECK=1 to compare pinnedSDKVersion to PyPI latest")
	}
	ctx, cancel := context.WithTimeout(t.Context(), 30*time.Second)
	defer cancel()
	latest, err := latestSDKVersion(ctx)
	if err != nil {
		t.Fatalf("latestSDKVersion: %v", err)
	}
	if latest == pinnedSDKVersion {
		t.Logf("cursor-sdk pin current: %s", pinnedSDKVersion)
		return
	}
	t.Fatalf("cursor-sdk pin drift: pinned=%s pypi_latest=%s — re-pin (see .agents/skills/repin-cursor-sdk/SKILL.md)",
		pinnedSDKVersion, latest)
}

func TestPlatformTags(t *testing.T) {
	osTag, archTags, err := platformTags()
	if err != nil {
		t.Skipf("unsupported platform %s/%s", runtime.GOOS, runtime.GOARCH)
	}
	if osTag == "" || len(archTags) == 0 {
		t.Fatalf("empty tags: %q %v", osTag, archTags)
	}

	// The tags must match Cursor's ACTUAL published filenames. Hardcoding a
	// constructed PEP 425 tag is what this replaced: macOS wheels are
	// macosx_11_0_*, and Linux ones carry a compound
	// manylinux2014_x86_64.manylinux_2_17_x86_64.
	samples := map[string][]string{
		"darwin/arm64":  {"cursor_sdk-1.0.27-py3-none-macosx_11_0_arm64.whl"},
		"darwin/amd64":  {"cursor_sdk-1.0.27-py3-none-macosx_11_0_x86_64.whl"},
		"linux/arm64":   {"cursor_sdk-1.0.27-py3-none-manylinux2014_aarch64.manylinux_2_17_aarch64.whl"},
		"linux/amd64":   {"cursor_sdk-1.0.27-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl"},
		"windows/amd64": {"cursor_sdk-1.0.27-py3-none-win_amd64.whl"},
	}
	want := samples[runtime.GOOS+"/"+runtime.GOARCH]
	if len(want) == 0 {
		t.Skip("no sample filename for this platform")
	}
	for _, name := range want {
		lower := strings.ToLower(name)
		if !strings.Contains(lower, osTag) {
			t.Errorf("osTag %q does not match %q", osTag, name)
		}
		matched := false
		for _, a := range archTags {
			if strings.Contains(lower, a) {
				matched = true
			}
		}
		if !matched {
			t.Errorf("archTags %v do not match %q", archTags, name)
		}
	}
}

// TestPlatformTagsRejectOtherPlatforms guards against a tag so loose it would
// download the wrong architecture's Node binary — which would install cleanly
// and then fail with an exec format error.
func TestPlatformTagsRejectOtherPlatforms(t *testing.T) {
	osTag, archTags, err := platformTags()
	if err != nil {
		t.Skip("unsupported platform")
	}
	all := []string{
		"cursor_sdk-1.0.27-py3-none-macosx_11_0_arm64.whl",
		"cursor_sdk-1.0.27-py3-none-macosx_11_0_x86_64.whl",
		"cursor_sdk-1.0.27-py3-none-manylinux2014_aarch64.manylinux_2_17_aarch64.whl",
		"cursor_sdk-1.0.27-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl",
		"cursor_sdk-1.0.27-py3-none-win_amd64.whl",
	}
	matches := 0
	for _, name := range all {
		lower := strings.ToLower(name)
		if !strings.Contains(lower, osTag) {
			continue
		}
		for _, a := range archTags {
			if strings.Contains(lower, a) {
				matches++
				break
			}
		}
	}
	if matches != 1 {
		t.Errorf("platform tags matched %d wheels, want exactly 1", matches)
	}
}

// TestExtractBridgePreservesExecutableBit is the one that matters most: without
// the mode carried across, bin/node and the launcher come out non-executable and
// the bridge cannot start — with a confusing "permission denied" much later.
func TestExtractBridgePreservesExecutableBit(t *testing.T) {
	archive := filepath.Join(t.TempDir(), "wheel.zip")
	writeTestWheel(t, archive, map[string]struct {
		body string
		mode os.FileMode
	}{
		bridgeArchivePrefix + "bin/cursor-sdk-bridge": {"#!/bin/sh\necho hi\n", 0o755},
		bridgeArchivePrefix + "bin/node":              {"binary", 0o755},
		bridgeArchivePrefix + "manifest.json":         {`{"sdkVersion":"1.0.27"}`, 0o644},
		bridgeArchivePrefix + "dist/index.js":         {"//js", 0o644},
		// Everything outside the bridge prefix must be skipped.
		"cursor_sdk/_client.py": {"# python", 0o644},
	})

	dataDir := t.TempDir()
	dst := filepath.Join(dataDir, BridgeDirName)
	if err := extractBridge(archive, dst, nil); err != nil {
		t.Fatalf("extractBridge: %v", err)
	}

	launcher := filepath.Join(dst, launcherRelPath)
	if !isExecutableFile(launcher) {
		t.Errorf("%s is not executable after extraction", launcherRelPath)
	}
	if !isExecutableFile(filepath.Join(dst, "bin", "node")) {
		t.Error("bin/node is not executable after extraction")
	}
	if _, err := os.Stat(filepath.Join(dst, "manifest.json")); err != nil {
		t.Errorf("manifest.json missing: %v", err)
	}
	if _, err := os.Stat(filepath.Join(dst, "dist", "index.js")); err != nil {
		t.Errorf("dist/index.js missing: %v", err)
	}
	// The wheel's Python package must not be unpacked alongside the bridge.
	if _, err := os.Stat(filepath.Join(dst, "_client.py")); err == nil {
		t.Error("extracted a file from outside the bridge prefix")
	}

	// And the result must be resolvable by the normal lookup path.
	if _, err := Locate(dataDir); err != nil {
		t.Errorf("Locate could not find the extracted bridge: %v", err)
	}
}

// A wheel with no bridge inside is an error, not a silently empty install.
func TestExtractBridgeRejectsWheelWithoutBridge(t *testing.T) {
	archive := filepath.Join(t.TempDir(), "wheel.zip")
	writeTestWheel(t, archive, map[string]struct {
		body string
		mode os.FileMode
	}{"cursor_sdk/_client.py": {"# python", 0o644}})

	if err := extractBridge(archive, filepath.Join(t.TempDir(), "out"), nil); err == nil {
		t.Fatal("want an error when the wheel has no bridge")
	}
}

// Zip-slip: a crafted member must not escape the destination.
func TestExtractBridgeRejectsPathTraversal(t *testing.T) {
	archive := filepath.Join(t.TempDir(), "wheel.zip")
	writeTestWheel(t, archive, map[string]struct {
		body string
		mode os.FileMode
	}{
		bridgeArchivePrefix + "../../escaped.sh": {"pwned", 0o755},
		bridgeArchivePrefix + "bin/node":         {"ok", 0o755},
	})

	dst := filepath.Join(t.TempDir(), "out")
	if err := extractBridge(archive, dst, nil); err == nil {
		t.Fatal("want an error for a traversing member")
	}
}

func TestInstallProgressPercent(t *testing.T) {
	if got := (InstallProgress{Current: 50, Total: 200}).Percent(); got != 25 {
		t.Errorf("percent = %d, want 25", got)
	}
	// An unknown total must report -1 (indeterminate), not 0% or a divide panic.
	if got := (InstallProgress{Current: 50}).Percent(); got != -1 {
		t.Errorf("percent with unknown total = %d, want -1", got)
	}
}

func TestInstallRejectsEmptyDataDir(t *testing.T) {
	if _, err := Install(context.Background(), "", nil); err == nil {
		t.Fatal("want an error with no data directory")
	}
}

// TestInstallRealSmoke performs a REAL download+unpack into a temp dir and
// starts the result. Skipped by default: it pulls ~50MB from PyPI.
//
//	RECUR_CURSOR_INSTALL_TEST=1 go test ./internal/cursor -run InstallRealSmoke -v
func TestInstallRealSmoke(t *testing.T) {
	if os.Getenv("RECUR_CURSOR_INSTALL_TEST") == "" {
		t.Skip("set RECUR_CURSOR_INSTALL_TEST=1 to exercise a real download")
	}
	dir := t.TempDir()

	var phases []string
	bin, err := Install(context.Background(), dir, func(p InstallProgress) {
		if len(phases) == 0 || phases[len(phases)-1] != string(p.Phase) {
			phases = append(phases, string(p.Phase))
		}
	})
	if err != nil {
		t.Fatalf("Install: %v", err)
	}
	if want := []string{"resolving", "downloading", "verifying", "extracting", "done"}; !equalStrings(phases, want) {
		t.Errorf("phases = %v, want %v", phases, want)
	}

	// The point of the whole exercise: what we just installed must actually run.
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	b := &Bridge{Bin: bin}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start freshly installed bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	v, err := b.Client().GetVersion(ctx)
	if err != nil {
		t.Fatalf("GetVersion: %v", err)
	}
	if v.ProtocolVersion != "sdk.v1" {
		t.Errorf("protocol = %q, want sdk.v1", v.ProtocolVersion)
	}
	t.Logf("installed %s, bridge %s", bin, v.BridgeVersion)
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// writeTestWheel builds a zip standing in for a cursor-sdk wheel.
func writeTestWheel(t *testing.T, path string, members map[string]struct {
	body string
	mode os.FileMode
}) {
	t.Helper()
	f, err := os.Create(path)
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	defer func() { _ = f.Close() }()

	zw := zip.NewWriter(f)
	for name, m := range members {
		hdr := &zip.FileHeader{Name: name, Method: zip.Deflate}
		hdr.SetMode(m.mode)
		w, err := zw.CreateHeader(hdr)
		if err != nil {
			t.Fatalf("create header %s: %v", name, err)
		}
		if _, err := w.Write([]byte(m.body)); err != nil {
			t.Fatalf("write %s: %v", name, err)
		}
	}
	if err := zw.Close(); err != nil {
		t.Fatalf("close zip: %v", err)
	}
}
