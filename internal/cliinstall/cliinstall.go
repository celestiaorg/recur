// Package cliinstall places the bundled Recur CLI on PATH when the desktop
// app launches. The binary lives inside the .app
// (Contents/MacOS/recur-cli); we only write a symlink, so an app update
// is picked up without recopying bytes.
package cliinstall

import (
	"fmt"
	"os"
	"path/filepath"
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
// …/Something.app/Contents/MacOS/<name>. ok is false when exe is not in a
// bundle or the CLI file is missing.
func BundledCLI(exe string) (string, bool) {
	exe = strings.TrimSpace(exe)
	if exe == "" {
		return "", false
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

// IsDevBundle reports a Wails `*.dev.app` (local `wails3 task run`), which
// should not steal the user's production `recur` symlink.
func IsDevBundle(exe string) bool {
	return strings.Contains(exe, ".dev.app/")
}

// IsBundledCLIPath reports whether p looks like a Recur.app-shipped CLI, so
// we only retarget our own stale symlinks and never clobber Homebrew / go install.
func IsBundledCLIPath(p string) bool {
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
	return nil
}
