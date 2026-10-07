package code

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"

	"recur/internal/domain"
)

// openInApp is one catalog entry. Only installed apps are listed; OpenIn
// launches the worktree folder in that app.
type openInApp struct {
	id         string
	name       string
	darwin     []string // .app bundle names (e.g. "Cursor.app")
	bins       []string // PATH fallbacks
	darwinOnly bool
	always     bool // skip install check (system apps)
}

func openInCatalog() []openInApp {
	return []openInApp{
		{id: "cursor", name: "Cursor", darwin: []string{"Cursor.app"}, bins: []string{"cursor"}},
		{id: "vscode", name: "Visual Studio Code", darwin: []string{"Visual Studio Code.app", "Code.app"}, bins: []string{"code"}},
		{id: "zed", name: "Zed", darwin: []string{"Zed.app"}, bins: []string{"zed"}},
		{id: "xcode", name: "Xcode", darwin: []string{"Xcode.app"}, darwinOnly: true},
		{id: "ghostty", name: "Ghostty", darwin: []string{"Ghostty.app"}, bins: []string{"ghostty"}},
		{id: "iterm", name: "iTerm", darwin: []string{"iTerm.app", "iTerm2.app"}, darwinOnly: true},
		{id: "terminal", name: "Terminal", darwin: []string{"Terminal.app"}, darwinOnly: true, always: runtime.GOOS == "darwin"},
		{id: "warp", name: "Warp", darwin: []string{"Warp.app"}, bins: []string{"warp"}},
		{id: "kaleidoscope", name: "Kaleidoscope", darwin: []string{"Kaleidoscope.app"}, darwinOnly: true},
		{id: "sublime", name: "Sublime Text", darwin: []string{"Sublime Text.app"}, bins: []string{"subl"}},
	}
}

// Overridable in tests.
var (
	macAppInstalled = macAppExists
	lookPath        = exec.LookPath
	startDetached   = startDetachedCmd
)

func startDetachedCmd(name string, args ...string) error {
	cmd := exec.Command(name, args...)
	if err := cmd.Start(); err != nil {
		return err
	}
	// Wait in the background so the child is reaped; Release() would leave a zombie.
	go func() { _ = cmd.Wait() }()
	return nil
}

func macAppPath(names ...string) string {
	home, _ := os.UserHomeDir()
	dirs := []string{
		"/Applications",
		"/System/Applications",
		"/System/Applications/Utilities",
		"/Applications/Utilities",
		"/System/Library/CoreServices",
	}
	if home != "" {
		dirs = append(dirs, filepath.Join(home, "Applications"))
	}
	for _, name := range names {
		for _, dir := range dirs {
			p := filepath.Join(dir, name)
			st, err := os.Stat(p)
			if err == nil && st.IsDir() {
				return p
			}
		}
	}
	return ""
}

func macAppExists(names ...string) bool {
	return macAppPath(names...) != ""
}

func finderAppPath() string {
	return macAppPath("Finder.app")
}

var iconCache sync.Map // abs app path → data URL (may be empty)

func cachedAppIcon(appPath string) string {
	if appPath == "" {
		return ""
	}
	if v, ok := iconCache.Load(appPath); ok {
		return v.(string)
	}
	url := appIconDataURL(appPath)
	iconCache.Store(appPath, url)
	return url
}

func appPresent(app openInApp) bool {
	if app.always {
		return true
	}
	if app.darwinOnly && runtime.GOOS != "darwin" {
		return false
	}
	if len(app.darwin) > 0 && macAppInstalled(app.darwin...) {
		return true
	}
	if runtime.GOOS == "darwin" {
		for _, bin := range app.bins {
			if _, err := lookPath(bin); err == nil {
				return true
			}
		}
	}
	return false
}

func fileManagerTarget() domain.OpenInTarget {
	return domain.OpenInTarget{
		ID:   "finder",
		Name: "Finder",
		Kind: "app",
		Icon: cachedAppIcon(finderAppPath()),
	}
}

// ListOpenInTargets returns installed apps that can open a workspace folder.
// Finder is first. Copy-path is a frontend-only action. Open In is macOS-only.
func ListOpenInTargets() []domain.OpenInTarget {
	if runtime.GOOS != "darwin" {
		return nil
	}
	out := []domain.OpenInTarget{fileManagerTarget()}
	for _, app := range openInCatalog() {
		if !appPresent(app) {
			continue
		}
		out = append(out, domain.OpenInTarget{
			ID:   app.id,
			Name: app.name,
			Kind: "app",
			Icon: cachedAppIcon(macAppPath(app.darwin...)),
		})
	}
	return out
}

func catalogByID(id string) (openInApp, bool) {
	for _, app := range openInCatalog() {
		if app.id == id {
			return app, true
		}
	}
	return openInApp{}, false
}

// OpenIn opens absPath (a directory) in the named target (finder, cursor, …).
func OpenIn(absPath, targetID string) error {
	st, err := os.Stat(absPath)
	if err != nil {
		return fmt.Errorf("cannot open %s: %w", absPath, err)
	}
	if !st.IsDir() {
		return fmt.Errorf("%s is not a directory", absPath)
	}
	if targetID == "finder" {
		return openFileManager(absPath)
	}
	app, ok := catalogByID(targetID)
	if !ok {
		return fmt.Errorf("unknown open-in target %q", targetID)
	}
	return openWithApp(absPath, app)
}

func openFileManager(path string) error {
	if runtime.GOOS != "darwin" {
		return fmt.Errorf("open folder not supported on %s", runtime.GOOS)
	}
	return startDetached("open", path)
}

func openWithApp(path string, app openInApp) error {
	if runtime.GOOS != "darwin" {
		return fmt.Errorf("open in %s is only supported on macOS", app.name)
	}
	if len(app.darwin) > 0 {
		for _, bundle := range app.darwin {
			if !app.always && !macAppInstalled(bundle) {
				continue
			}
			name := strings.TrimSuffix(bundle, ".app")
			return startDetached("open", "-a", name, path)
		}
	}
	var last error
	for _, bin := range app.bins {
		p, err := lookPath(bin)
		if err != nil {
			last = err
			continue
		}
		if err := startDetached(p, path); err != nil {
			last = err
			continue
		}
		return nil
	}
	if last != nil {
		return fmt.Errorf("open in %s: %w", app.name, last)
	}
	return fmt.Errorf("%s is not installed", app.name)
}
