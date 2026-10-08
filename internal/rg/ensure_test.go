package rg

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

func TestManagedVersionSidecar(t *testing.T) {
	data := t.TempDir()
	if got := managedVersion(data); got != "" {
		t.Fatalf("managedVersion(empty) = %q, want \"\"", got)
	}
	if err := writeManagedVersion(data); err != nil {
		t.Fatal(err)
	}
	if got := managedVersion(data); got != ReleaseVersion {
		t.Fatalf("managedVersion = %q, want %q", got, ReleaseVersion)
	}
}

func TestEnsureReusesCurrentManagedWithoutNetwork(t *testing.T) {
	t.Setenv(EnvPath, "")
	data := t.TempDir()
	bin := filepath.Join(data, "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	managed := filepath.Join(bin, BinName)
	if err := os.WriteFile(managed, []byte("#!/bin/sh\necho fake\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := writeManagedVersion(data); err != nil {
		t.Fatal(err)
	}

	got, err := Ensure(context.Background(), data)
	if err != nil {
		t.Fatal(err)
	}
	if got != managed {
		t.Fatalf("Ensure = %q, want managed %q", got, managed)
	}
}

func TestEnsurePrefersEnvOverStaleManaged(t *testing.T) {
	data := t.TempDir()
	bin := filepath.Join(data, "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	managed := filepath.Join(bin, BinName)
	if err := os.WriteFile(managed, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	// Stale / missing sidecar — would trigger reinstall if Ensure reached it.
	if err := os.WriteFile(filepath.Join(bin, VersionFileName), []byte("0.0.1\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	override := filepath.Join(t.TempDir(), "override-rg")
	if err := os.WriteFile(override, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(EnvPath, override)

	got, err := Ensure(context.Background(), data)
	if err != nil {
		t.Fatal(err)
	}
	if got != override {
		t.Fatalf("Ensure = %q, want env override %q", got, override)
	}
	// Stale managed must not have been replaced when env wins.
	if managedVersion(data) != "0.0.1" {
		t.Fatalf("managed version changed to %q; env override should skip install", managedVersion(data))
	}
}

func TestReleaseSHA256CoversAssets(t *testing.T) {
	for _, arch := range []string{"aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-musl", "x86_64-unknown-linux-musl"} {
		asset := "ripgrep-" + ReleaseVersion + "-" + arch + ".tar.gz"
		sum, ok := releaseSHA256[asset]
		if !ok || len(sum) != 64 {
			t.Fatalf("missing/invalid SHA-256 pin for %s: %q", asset, sum)
		}
	}
}

func TestStaleManagedNeedsUpgrade(t *testing.T) {
	data := t.TempDir()
	bin := filepath.Join(data, "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bin, BinName), []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bin, VersionFileName), []byte("0.0.1\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if managedVersion(data) == ReleaseVersion {
		t.Fatal("test setup: version should be stale")
	}
	if !isExecutableFile(filepath.Join(bin, BinName)) {
		t.Fatal("managed binary missing")
	}
	// Ensure would call install() here; we only assert the gate condition.
	if managedVersion(data) == ReleaseVersion {
		t.Fatal("stale managed should not look current")
	}
}
