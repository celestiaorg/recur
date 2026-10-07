package app

import (
	"os"
	"path/filepath"
	"testing"

	"recur/internal/cursor"
)

func TestApplyCursorPin(t *testing.T) {
	dataDir := t.TempDir()
	owned := writeOwnedBridge(t, dataDir)

	out := CursorStatus{BridgeVersion: "1.0.27", BridgePath: owned}
	applyCursorPin(&out, dataDir, owned)
	if out.PinnedVersion != cursor.PinnedSDKVersion() {
		t.Errorf("applyCursorPin PinnedVersion = %q, want %q", out.PinnedVersion, cursor.PinnedSDKVersion())
	}
	if !out.UpdateAvailable {
		t.Errorf("applyCursorPin(owned, BridgeVersion=%q) UpdateAvailable = false, want true", out.BridgeVersion)
	}

	out = CursorStatus{BridgeVersion: cursor.PinnedSDKVersion(), BridgePath: owned}
	applyCursorPin(&out, dataDir, owned)
	if out.UpdateAvailable {
		t.Errorf("applyCursorPin(owned, BridgeVersion=%q) UpdateAvailable = true, want false", out.BridgeVersion)
	}

	out = CursorStatus{}
	applyCursorPin(&out, dataDir, owned)
	if out.UpdateAvailable {
		t.Errorf("applyCursorPin(owned, empty BridgeVersion) UpdateAvailable = true, want false")
	}

	external := filepath.Join(t.TempDir(), "cursor-bridge", "bin", "cursor-sdk-bridge")
	if err := os.MkdirAll(filepath.Dir(external), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(external, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	out = CursorStatus{BridgeVersion: "1.0.27", BridgePath: external}
	applyCursorPin(&out, dataDir, external)
	if out.UpdateAvailable {
		t.Errorf("applyCursorPin(external, BridgeVersion=%q) UpdateAvailable = true, want false", out.BridgeVersion)
	}
	if out.PinnedVersion != cursor.PinnedSDKVersion() {
		t.Errorf("applyCursorPin(external) PinnedVersion = %q, want %q", out.PinnedVersion, cursor.PinnedSDKVersion())
	}
}

func writeOwnedBridge(t *testing.T, dataDir string) string {
	t.Helper()
	launcher := filepath.Join(dataDir, cursor.BridgeDirName, "bin", "cursor-sdk-bridge")
	if err := os.MkdirAll(filepath.Dir(launcher), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(launcher, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	return launcher
}
