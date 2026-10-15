package cursor

import (
	"os"
	"path/filepath"
	"testing"
)

func TestEnsureBridgeShellEnvKeepsGoodShell(t *testing.T) {
	env := []string{"PATH=/usr/bin", "SHELL=/bin/sh"}
	got := ensureBridgeShellEnv(env)
	if envValue(got, "SHELL") != "/bin/sh" {
		t.Fatalf("SHELL=%q, want /bin/sh", envValue(got, "SHELL"))
	}
}

func TestEnsureBridgeShellEnvFillsMissing(t *testing.T) {
	env := []string{"PATH=/usr/bin"}
	got := ensureBridgeShellEnv(env)
	shell := envValue(got, "SHELL")
	if !shellOK(shell) {
		t.Fatalf("SHELL=%q is not a usable absolute shell", shell)
	}
}

func TestEnsureBridgeShellEnvReplacesMissingBinary(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "no-such-shell")
	env := []string{"SHELL=" + missing}
	got := ensureBridgeShellEnv(env)
	shell := envValue(got, "SHELL")
	if shell == missing {
		t.Fatal("did not replace missing SHELL")
	}
	if !shellOK(shell) {
		t.Fatalf("SHELL=%q is not usable", shell)
	}
	if _, err := os.Stat(shell); err != nil {
		t.Fatalf("replacement shell missing: %v", err)
	}
}

func TestDropEnvKeyStripsLinearAPIKey(t *testing.T) {
	in := []string{"PATH=/bin", "LINEAR_API_KEY=secret", "FOO=bar"}
	got := dropEnvKey(in, "LINEAR_API_KEY")
	if envValue(got, "LINEAR_API_KEY") != "" {
		t.Fatalf("dropEnvKey(%v) LINEAR_API_KEY=%q, want empty", in, envValue(got, "LINEAR_API_KEY"))
	}
	if envValue(got, "PATH") != "/bin" || envValue(got, "FOO") != "bar" {
		t.Fatalf("dropEnvKey(%v) = %v, want PATH and FOO kept", in, got)
	}
}
