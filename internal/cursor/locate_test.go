package cursor

import (
	"os"
	"path/filepath"
	"testing"
)

func TestLocateOnlyDataDir(t *testing.T) {
	dataDir := t.TempDir()
	external := filepath.Join(t.TempDir(), "cursor-sdk-bridge")
	if err := os.WriteFile(external, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(EnvBridgePath, external)

	if _, err := Locate(dataDir); err == nil {
		t.Fatal("Locate() succeeded with empty dataDir install")
	}

	owned := filepath.Join(dataDir, BridgeDirName, "bin", "cursor-sdk-bridge")
	if err := os.MkdirAll(filepath.Dir(owned), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(owned, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	got, err := Locate(dataDir)
	if err != nil {
		t.Fatalf("Locate() error = %v", err)
	}
	if got != owned {
		t.Fatalf("Locate() = %q, want %q", got, owned)
	}
	if ResolveLauncher(external) != external {
		t.Fatalf("ResolveLauncher(env) = %q, want external launcher", ResolveLauncher(external))
	}
}
