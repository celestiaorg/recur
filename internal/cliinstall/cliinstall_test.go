package cliinstall

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestBundledCLI(t *testing.T) {
	if _, ok := BundledCLI(""); ok {
		t.Fatal("BundledCLI(\"\") ok = true, want false")
	}
	if _, ok := BundledCLI("/usr/local/bin/recur"); ok {
		t.Fatal("BundledCLI(plain path) ok = true, want false")
	}

	root := t.TempDir()
	exe := filepath.Join(root, "Recur.app", "Contents", "MacOS", "recur")
	cli := filepath.Join(root, "Recur.app", RelBundledCLI)
	if err := os.MkdirAll(filepath.Dir(exe), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(cli), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(exe, []byte("gui"), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, ok := BundledCLI(exe); ok {
		t.Fatal("BundledCLI ok = true before the CLI file exists, want false")
	}
	if err := os.WriteFile(cli, []byte("cli"), 0o755); err != nil {
		t.Fatal(err)
	}
	got, ok := BundledCLI(exe)
	if !ok {
		t.Fatal("BundledCLI ok = false, want true")
	}
	if got != cli {
		t.Fatalf("BundledCLI(%s) = %q, want %q", exe, got, cli)
	}
}

func TestIsDevBundle(t *testing.T) {
	if !IsDevBundle("/tmp/bin/recur.dev.app/Contents/MacOS/recur") {
		t.Fatal("IsDevBundle(dev.app) = false, want true")
	}
	if IsDevBundle("/Applications/Recur.app/Contents/MacOS/recur") {
		t.Fatal("IsDevBundle(Recur.app) = true, want false")
	}
}

func TestIsBundledCLIPath(t *testing.T) {
	if !IsBundledCLIPath("/Applications/Recur.app/Contents/MacOS/recur-cli") {
		t.Fatal("IsBundledCLIPath(app cli) = false, want true")
	}
	if !IsBundledCLIPath("/Applications/Recur.app/Contents/Resources/bin/recur") {
		t.Fatal("IsBundledCLIPath(legacy Resources/bin) = false, want true")
	}
	if IsBundledCLIPath("/Applications/Recur.app/Contents/MacOS/recur") {
		t.Fatal("IsBundledCLIPath(GUI) = true, want false")
	}
	if IsBundledCLIPath("/usr/local/bin/recur") {
		t.Fatal("IsBundledCLIPath(/usr/local/bin/recur) = true, want false")
	}
	if IsBundledCLIPath("/opt/homebrew/bin/recur") {
		t.Fatal("IsBundledCLIPath(homebrew) = true, want false")
	}
}

func TestEnsureLinkCreateAndUpdate(t *testing.T) {
	dir := t.TempDir()
	bundle := filepath.Join(dir, "Recur.app", RelBundledCLI)
	if err := os.MkdirAll(filepath.Dir(bundle), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(bundle, []byte("v1"), 0o755); err != nil {
		t.Fatal(err)
	}
	dest := filepath.Join(dir, "bin", "recur")

	if err := EnsureLink(dest, bundle); err != nil {
		t.Fatalf("EnsureLink create: %v", err)
	}
	got, err := os.Readlink(dest)
	if err != nil {
		t.Fatalf("Readlink: %v", err)
	}
	want, _ := filepath.Abs(bundle)
	if got != want {
		t.Fatalf("EnsureLink dest → %q, want %q", got, want)
	}
	if err := EnsureLink(dest, bundle); err != nil {
		t.Fatalf("EnsureLink idempotent: %v", err)
	}

	bundle2 := filepath.Join(dir, "Recur2.app", RelBundledCLI)
	if err := os.MkdirAll(filepath.Dir(bundle2), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(bundle2, []byte("v2"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := EnsureLink(dest, bundle2); err != nil {
		t.Fatalf("EnsureLink retarget: %v", err)
	}
	got, err = os.Readlink(dest)
	if err != nil {
		t.Fatal(err)
	}
	want2, _ := filepath.Abs(bundle2)
	if got != want2 {
		t.Fatalf("EnsureLink retarget → %q, want %q", got, want2)
	}
}

func TestEnsureLinkRefusesForeign(t *testing.T) {
	dir := t.TempDir()
	dest := filepath.Join(dir, "recur")
	if err := os.WriteFile(dest, []byte("go-install"), 0o755); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(dir, "Recur.app", RelBundledCLI)
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(target, []byte("cli"), 0o755); err != nil {
		t.Fatal(err)
	}
	err := EnsureLink(dest, target)
	if err == nil {
		t.Fatal("EnsureLink(regular file) error = nil, want refuse")
	}
	if !strings.Contains(err.Error(), "not a Recur symlink") {
		t.Fatalf("EnsureLink foreign file error = %v, want not a Recur symlink", err)
	}

	_ = os.Remove(dest)
	if err := os.Symlink("/opt/homebrew/bin/other", dest); err != nil {
		t.Fatal(err)
	}
	err = EnsureLink(dest, target)
	if err == nil {
		t.Fatal("EnsureLink(foreign symlink) error = nil, want refuse")
	}
}

func TestInstallFromApp(t *testing.T) {
	root := t.TempDir()
	t.Setenv("HOME", root)
	exe := filepath.Join(root, "Recur.app", "Contents", "MacOS", "recur")
	cli := filepath.Join(root, "Recur.app", RelBundledCLI)
	if err := os.MkdirAll(filepath.Dir(exe), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(cli), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(exe, []byte("gui"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(cli, []byte("cli"), 0o755); err != nil {
		t.Fatal(err)
	}

	old := destFn
	destFn = func() []string {
		return []string{filepath.Join(root, ".local", "bin", "recur")}
	}
	t.Cleanup(func() { destFn = old })

	if err := InstallFromApp(exe); err != nil {
		t.Fatalf("InstallFromApp: %v", err)
	}
	link := filepath.Join(root, ".local", "bin", "recur")
	got, err := os.Readlink(link)
	if err != nil {
		t.Fatalf("Readlink(%s): %v", link, err)
	}
	want, _ := filepath.Abs(cli)
	if got != want {
		t.Fatalf("InstallFromApp link → %q, want %q", got, want)
	}

	if err := InstallFromApp(strings.Replace(exe, "Recur.app", "recur.dev.app", 1)); err != nil {
		t.Fatalf("InstallFromApp(dev) = %v, want nil", err)
	}
	if err := InstallFromApp("/tmp/not-a-bundle/recur"); err != nil {
		t.Fatalf("InstallFromApp(unbundled) = %v, want nil", err)
	}
}

func TestEnsureDesktopEntry(t *testing.T) {
	data := t.TempDir()
	t.Setenv("XDG_DATA_HOME", data)
	exe := "/opt/my apps/recur-linux-amd64/recur"
	if err := ensureDesktopEntry(exe); err != nil {
		t.Fatalf("ensureDesktopEntry: %v", err)
	}
	b, err := os.ReadFile(filepath.Join(data, "applications", desktopEntryName))
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{
		"Exec=\"/opt/my apps/recur-linux-amd64/recur\"\n",
		"Icon=/opt/my apps/recur-linux-amd64/recur.png\n",
	} {
		if !strings.Contains(string(b), want) {
			t.Fatalf("desktop entry missing %q:\n%s", want, b)
		}
	}
}
