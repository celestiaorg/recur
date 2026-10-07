package cursor

import (
	"os"
	"path/filepath"
	"testing"
)

func TestSDKVersionFromManifest(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "bin"), 0o755); err != nil {
		t.Fatal(err)
	}
	launcher := filepath.Join(root, "bin", "cursor-sdk-bridge")
	if err := os.WriteFile(launcher, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "manifest.json"), []byte(`{
		"bridgeVersion": "1.0.0",
		"sdkVersion": "1.0.27"
	}`), 0o644); err != nil {
		t.Fatal(err)
	}

	if got := SDKVersion(launcher); got != "1.0.27" {
		t.Fatalf("SDKVersion(launcher) = %q, want 1.0.27", got)
	}
	if got := SDKVersion(root); got != "1.0.27" {
		t.Fatalf("SDKVersion(root) = %q, want 1.0.27", got)
	}
	if got := SDKVersion(""); got != "" {
		t.Fatalf("SDKVersion(empty) = %q", got)
	}
}
