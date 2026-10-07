package codex

import (
	"os"
	"path/filepath"
	"testing"
)

func TestLocateEnvOverride(t *testing.T) {
	dir := t.TempDir()
	bin := filepath.Join(dir, "codex")
	// Minimal Mach-O thin 64-bit magic so looksLikeNativeCodex accepts it.
	if err := os.WriteFile(bin, []byte{0xCF, 0xFA, 0xED, 0xFE, 0, 0, 0, 0}, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(EnvBinPath, bin)
	got, err := Locate("")
	if err != nil {
		t.Fatal(err)
	}
	if got != bin {
		// Locate Abs-normalizes; compare base.
		if filepath.Base(got) != "codex" {
			t.Fatalf("got %q", got)
		}
	}
}

func TestResolveCandidateRejectsNodeShim(t *testing.T) {
	dir := t.TempDir()
	bin := filepath.Join(dir, "codex")
	script := "#!/usr/bin/env node\nconsole.log('shim')\n"
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	if got := resolveCandidate(bin); got != "" {
		t.Fatalf("node shim should be rejected, got %q", got)
	}
}

func TestHomeDir(t *testing.T) {
	got := HomeDir("/data")
	if got != filepath.Join("/data", "codex-home") {
		t.Fatalf("got %q", got)
	}
}
