package cursor

import (
	"fmt"
	"os"
	"path/filepath"
)

// BridgeDirName is the directory Recur installs the bridge into, under the app's
// data dir. Laid out exactly like the vendored copy inside Cursor's Python
// wheel, so the same launcher script works:
//
//	<dataDir>/cursor-bridge/
//	  bin/cursor-sdk-bridge   (sh launcher)
//	  bin/node                (~117MB bundled runtime)
//	  dist/…                  (the SDK JS)
//	  manifest.json
const BridgeDirName = "cursor-bridge"

// EnvBridgePath is the opt-in smoke-test seam: point it at an unpacked
// cursor-sdk wheel and tests that call ResolveLauncher will drive that copy.
// Production Locate ignores it — the only valid install is Recur's data dir.
const EnvBridgePath = "RECUR_CURSOR_BRIDGE"

// launcherRelPath is where the executable sits inside a bridge install.
var launcherRelPath = filepath.Join("bin", "cursor-sdk-bridge")

// Locate resolves Recur's own <dataDir>/cursor-bridge launcher. Env overrides,
// Settings.CursorBridgePath, and pip wheels are not consulted — those copies
// skip EnsureAttributionPatch and would keep native getAgentSkills.
func Locate(dataDir string) (string, error) {
	if dataDir == "" {
		return "", fmt.Errorf("cursor bridge not found — install it from Settings")
	}
	p := filepath.Join(dataDir, BridgeDirName)
	if resolved := resolveCandidate(p); resolved != "" {
		return resolved, nil
	}
	return "", fmt.Errorf("cursor bridge not found at %s — install it from Settings", p)
}

// ResolveLauncher accepts a launcher path or bridge root. Smoke tests use this
// with $RECUR_CURSOR_BRIDGE; agent runs must use Locate.
func ResolveLauncher(p string) string {
	return resolveCandidate(p)
}

// resolveCandidate accepts either a launcher path or a bridge root and returns
// the launcher if it exists and is executable.
func resolveCandidate(p string) string {
	if p == "" {
		return ""
	}
	if isExecutableFile(p) {
		return p
	}
	launcher := filepath.Join(p, launcherRelPath)
	if isExecutableFile(launcher) {
		return launcher
	}
	return ""
}

func isExecutableFile(p string) bool {
	fi, err := os.Stat(p)
	if err != nil || fi.IsDir() {
		return false
	}
	return fi.Mode()&0o111 != 0
}
