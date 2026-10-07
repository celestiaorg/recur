package managed

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestEnsureGoplsUsesEnvOverride(t *testing.T) {
	dir := t.TempDir()
	fake := filepath.Join(dir, "gopls")
	if err := os.WriteFile(fake, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(EnvGopls, fake)
	got, err := Ensure(context.Background(), t.TempDir(), "gopls")
	if err != nil {
		t.Fatal(err)
	}
	if got != fake {
		t.Fatalf("got %q want %q", got, fake)
	}
}

func TestEnsureGoplsReusesPinnedInstall(t *testing.T) {
	data := t.TempDir()
	binDir := filepath.Join(data, "bin", "lsp", "gopls")
	if err := os.MkdirAll(binDir, 0o755); err != nil {
		t.Fatal(err)
	}
	bin := filepath.Join(binDir, "gopls")
	if err := os.WriteFile(bin, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := writeManagedVersion(data, "gopls", GoplsRelease); err != nil {
		t.Fatal(err)
	}
	// Point PATH away from a real gopls and clear override.
	t.Setenv(EnvGopls, "")
	t.Setenv("PATH", filepath.Join(data, "empty"))
	got, err := Ensure(context.Background(), data, "gopls")
	if err != nil {
		t.Fatal(err)
	}
	if got != bin {
		t.Fatalf("got %q want %q", got, bin)
	}
}

func TestInstallNpmBinCreatesDestDirForSharedPrefix(t *testing.T) {
	data := t.TempDir()
	prefix := managedBinDir(data, "vscode-langservers-extracted")
	binDir := filepath.Join(prefix, "node_modules", ".bin")
	if err := os.MkdirAll(binDir, 0o755); err != nil {
		t.Fatal(err)
	}
	shim := filepath.Join(binDir, "vscode-json-language-server")
	if err := os.WriteFile(shim, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := writeManagedVersion(data, "vscode-langservers-extracted", "1"); err != nil {
		t.Fatal(err)
	}
	dest := filepath.Join(managedBinDir(data, "vscode-json-language-server"), "vscode-json-language-server")
	err := installNpmBin(context.Background(), data, dest, npmBinPin{
		ID:       "vscode-json-language-server",
		Version:  "1",
		BinName:  "vscode-json-language-server",
		PrefixID: "vscode-langservers-extracted",
	})
	if err != nil {
		t.Fatal(err)
	}
	st, err := os.Lstat(dest)
	if err != nil {
		t.Fatal(err)
	}
	if st.Mode()&os.ModeSymlink == 0 {
		t.Fatalf("dest is not a symlink: %v", st.Mode())
	}
	target, err := os.Readlink(dest)
	if err != nil {
		t.Fatal(err)
	}
	if target != shim {
		t.Fatalf("symlink %q want %q", target, shim)
	}
}

func TestEnsureRuffUsesEnvOverride(t *testing.T) {
	dir := t.TempDir()
	fake := filepath.Join(dir, "ruff")
	if err := os.WriteFile(fake, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(EnvRuff, fake)
	got, err := Ensure(context.Background(), t.TempDir(), "ruff")
	if err != nil {
		t.Fatal(err)
	}
	if got != fake {
		t.Fatalf("got %q want %q", got, fake)
	}
}

func TestEnsureGoplsInstallsWhenGoPresent(t *testing.T) {
	if testing.Short() {
		t.Skip("downloads gopls via go install")
	}
	if _, err := exec.LookPath("go"); err != nil {
		t.Skip("go not on PATH")
	}
	data := t.TempDir()
	t.Setenv(EnvGopls, "")
	got, err := Ensure(context.Background(), data, "gopls")
	if err != nil {
		t.Fatal(err)
	}
	if !isExecutableFile(got) {
		t.Fatalf("not executable: %s", got)
	}
	if managedVersion(data, "gopls") != GoplsRelease {
		t.Fatalf("version sidecar=%q", managedVersion(data, "gopls"))
	}
}

func TestVuePluginPath(t *testing.T) {
	if VuePluginPath("") != "" || VuePluginPath(t.TempDir()) != "" {
		t.Fatal("missing prefix should be empty")
	}
	data := t.TempDir()
	dir := filepath.Join(data, "bin", "lsp", "vue-language-server", "node_modules", "@vue", "language-server")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if got := VuePluginPath(data); got != dir {
		t.Fatalf("got %q want %q", got, dir)
	}
}
