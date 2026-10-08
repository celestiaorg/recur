// Package cliinstall places the bundled Recur CLI on PATH when the desktop
// app launches. The binary lives inside the .app
// (Contents/MacOS/recur-cli); we only write a symlink, so an app update
// is picked up without recopying bytes.
package cliinstall

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// RelBundledCLI is the path of the CLI inside a macOS app bundle, relative
// to the bundle root (the Foo.app directory). Named recur-cli so it does
// not collide with the GUI at Contents/MacOS/recur. Apple wants helper
// executables here (or Contents/Helpers/), not under Resources.
const RelBundledCLI = "Contents/MacOS/recur-cli"

// LinkName is the command users type.
const LinkName = "recur"

// BundledCLI returns the CLI path next to a GUI executable that lives at
// …/Something.app/Contents/MacOS/<name> (macOS) or in a flat package
// directory beside recur-cli (Linux). ok is false when exe is not in a
// bundle or the CLI file is missing.
func BundledCLI(exe string) (string, bool) {
	exe = strings.TrimSpace(exe)
	if exe == "" {
		return "", false
	}
	if runtime.GOOS != "darwin" {
		cli := filepath.Join(filepath.Dir(exe), filepath.Base(RelBundledCLI))
		st, err := os.Stat(cli)
		return cli, err == nil && !st.IsDir()
	}
	// …/App.app/Contents/MacOS/recur → …/App.app
	macos := filepath.Dir(exe)
	contents := filepath.Dir(macos)
	bundle := filepath.Dir(contents)
	if filepath.Base(macos) != "MacOS" || filepath.Base(contents) != "Contents" {
		return "", false
	}
	if !strings.HasSuffix(bundle, ".app") {
		return "", false
	}
	cli := filepath.Join(bundle, RelBundledCLI)
	st, err := os.Stat(cli)
	if err != nil || st.IsDir() {
		return "", false
	}
	return cli, true
}

// IsDevBundle reports a Wails `*.dev.app` or a repo checkout's bin/ (local
// `wails3 task run`), which should not steal the user's production `recur`
// symlink.
func IsDevBundle(exe string) bool {
	if strings.Contains(exe, ".dev.app/") {
		return true
	}
	_, err := os.Stat(filepath.Join(filepath.Dir(filepath.Dir(exe)), "go.mod"))
	return err == nil
}

// IsBundledCLIPath reports whether p looks like a Recur-shipped CLI, so we
// only retarget our own stale symlinks and never clobber Homebrew / go install.
func IsBundledCLIPath(p string) bool {
	if runtime.GOOS != "darwin" && filepath.Base(p) == filepath.Base(RelBundledCLI) {
		return true
	}
	p = filepath.ToSlash(p)
	if strings.Contains(p, ".app/"+filepath.ToSlash(RelBundledCLI)) {
		return true
	}
	// Stale launch-time links from the first Resources/bin layout.
	return strings.Contains(p, ".app/Contents/Resources/bin/"+LinkName)
}

// UserBinDir is ~/.local/bin — writable without sudo. Not on stock macOS PATH;
// /usr/local/bin is (via path_helper) and is tried first when writable.
func UserBinDir() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".local", "bin"), nil
}

// SystemBinDir is /usr/local/bin on Unix — the usual "install shell command"
// location (VS Code, Sublime).
func SystemBinDir() string {
	return "/usr/local/bin"
}

// destFn is the PATH locations InstallFromApp writes. Tests replace it so
// they never touch the real /usr/local/bin.
var destFn = defaultDestinations

// Destinations returns PATH locations to try. /usr/local/bin is first when
// writable (stock macOS PATH via path_helper). ~/.local/bin is always listed
// and created on demand. A permission error on the system dir is skipped.
func Destinations() []string {
	return destFn()
}

func defaultDestinations() []string {
	var out []string
	if dirWritable(SystemBinDir()) {
		out = append(out, filepath.Join(SystemBinDir(), LinkName))
	}
	if user, err := UserBinDir(); err == nil {
		out = append(out, filepath.Join(user, LinkName))
	}
	return out
}

func dirWritable(dir string) bool {
	st, err := os.Stat(dir)
	if err != nil || !st.IsDir() {
		return false
	}
	f, err := os.CreateTemp(dir, ".recur-cli-write-*")
	if err != nil {
		return false
	}
	name := f.Name()
	_ = f.Close()
	_ = os.Remove(name)
	return true
}

// EnsureLink makes dest a symlink to target. No-op when it already points
// there. Refuses to replace a regular file or a symlink that is not ours.
func EnsureLink(dest, target string) error {
	if dest == "" || target == "" {
		return fmt.Errorf("cliinstall: empty dest or target")
	}
	if err := os.MkdirAll(filepath.Dir(dest), 0o755); err != nil {
		return err
	}
	want, err := filepath.Abs(target)
	if err != nil {
		return err
	}

	st, err := os.Lstat(dest)
	if err == nil {
		if st.Mode()&os.ModeSymlink == 0 {
			return fmt.Errorf("cliinstall: %s exists and is not a Recur symlink", dest)
		}
		cur, err := os.Readlink(dest)
		if err != nil {
			return err
		}
		if !filepath.IsAbs(cur) {
			cur = filepath.Join(filepath.Dir(dest), cur)
		}
		curAbs, err := filepath.Abs(cur)
		if err == nil && sameFilePath(curAbs, want) {
			return nil
		}
		if !IsBundledCLIPath(cur) && !IsBundledCLIPath(curAbs) {
			return fmt.Errorf("cliinstall: %s points at %s (not Recur)", dest, cur)
		}
	} else if !os.IsNotExist(err) {
		return err
	}

	tmp := dest + ".tmp"
	_ = os.Remove(tmp)
	if err := os.Symlink(want, tmp); err != nil {
		return err
	}
	if err := os.Rename(tmp, dest); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return nil
}

func sameFilePath(a, b string) bool {
	if a == b {
		return true
	}
	fa, err1 := os.Stat(a)
	fb, err2 := os.Stat(b)
	if err1 != nil || err2 != nil {
		return false
	}
	return os.SameFile(fa, fb)
}

// InstallFromApp finds the CLI next to the running GUI exe and symlinks it
// onto PATH. Missing bundle / dev.app / missing CLI is a no-op (nil).
func InstallFromApp(exe string) error {
	if exe == "" || IsDevBundle(exe) {
		return nil
	}
	cli, ok := BundledCLI(exe)
	if !ok {
		return nil
	}
	var last error
	n := 0
	for _, dest := range Destinations() {
		if err := EnsureLink(dest, cli); err != nil {
			last = err
			continue
		}
		n++
	}
	if n == 0 && last != nil {
		return last
	}
	if runtime.GOOS == "linux" {
		return ensureDesktopEntry(exe)
	}
	return nil
}

// desktopEntryName matches the GTK application id Wails derives from the app
// name ("org.wails.recur"), so Wayland docks pair the window with its icon.
const desktopEntryName = "org.wails.recur.desktop"

// ensureDesktopEntry writes the app-menu launcher for a Linux package
// directory: Exec is exe, Icon the recur.png shipped beside it.
func ensureDesktopEntry(exe string) error {
	dir := os.Getenv("XDG_DATA_HOME")
	if dir == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return err
		}
		dir = filepath.Join(home, ".local", "share")
	}
	dir = filepath.Join(dir, "applications")
	quote := strings.NewReplacer(`\`, `\\\\`, `"`, `\"`, "`", "\\`", "$", `\$`, "%", "%%")
	body := "[Desktop Entry]\nType=Application\nName=Recur\nComment=Run autonomous AI coding loops locally.\n" +
		"Exec=\"" + quote.Replace(exe) + "\"\nIcon=" + filepath.Join(filepath.Dir(exe), "recur.png") +
		"\nCategories=Development;\nTerminal=false\n"
	dest := filepath.Join(dir, desktopEntryName)
	if old, err := os.ReadFile(dest); err == nil && string(old) == body {
		return nil
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	return os.WriteFile(dest, []byte(body), 0o644)
}
