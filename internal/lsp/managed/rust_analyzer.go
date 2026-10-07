package managed

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
)

// RustAnalyzerRelease is the pinned rust-analyzer GitHub release tag.
const RustAnalyzerRelease = "2026-08-10.1"

const rustAnalyzerBase = "https://github.com/rust-lang/rust-analyzer/releases/download/"

// Pinned digests for RustAnalyzerRelease (from GitHub release asset digests).
var rustAnalyzerSHA256 = map[string]string{
	"rust-analyzer-aarch64-apple-darwin.gz":      "954c1dce00ab43d5a35caac07aff12a787b006b50dd3f4471bae4b13512343ef",
	"rust-analyzer-x86_64-apple-darwin.gz":       "b84eba6670c7e55869d191f23cc23e8953ed18141db41de0834d7b9fb47a0090",
	"rust-analyzer-aarch64-unknown-linux-gnu.gz": "a7eb3a97865dd189ad4fff526fbe73559bece876fdbd641e0f50f39271043542",
	"rust-analyzer-x86_64-unknown-linux-gnu.gz":  "d42908a7dc7b89250ae881a0919e477296843665c98574ecc8fe16ba60cecefb",
}

func ensureRustAnalyzer(ctx context.Context, dataDir string) (string, error) {
	return ensurePinnedBinary(ctx, dataDir, "rust-analyzer", RustAnalyzerRelease, EnvRustAnalyzer, "rust-analyzer", nativeBinName("rust-analyzer"), installRustAnalyzer)
}

func rustAnalyzerAsset() (string, error) {
	switch runtime.GOOS + "/" + runtime.GOARCH {
	case "darwin/arm64":
		return "rust-analyzer-aarch64-apple-darwin.gz", nil
	case "darwin/amd64":
		return "rust-analyzer-x86_64-apple-darwin.gz", nil
	case "linux/arm64":
		return "rust-analyzer-aarch64-unknown-linux-gnu.gz", nil
	case "linux/amd64":
		return "rust-analyzer-x86_64-unknown-linux-gnu.gz", nil
	default:
		return "", fmt.Errorf("lsp managed: unsupported platform %s/%s for rust-analyzer", runtime.GOOS, runtime.GOARCH)
	}
}

func installRustAnalyzer(ctx context.Context, dataDir, dest string) error {
	asset, err := rustAnalyzerAsset()
	if err != nil {
		return err
	}
	want, ok := rustAnalyzerSHA256[asset]
	if !ok {
		return fmt.Errorf("lsp managed: no pinned SHA-256 for %s", asset)
	}
	dir := filepath.Dir(dest)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	staging, err := os.MkdirTemp(dir, ".ra-staging-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(staging)

	archive := filepath.Join(staging, asset)
	url := rustAnalyzerBase + RustAnalyzerRelease + "/" + asset
	if err := downloadVerified(ctx, url, want, archive); err != nil {
		return err
	}
	staged := filepath.Join(staging, "rust-analyzer")
	if err := gunzipFile(archive, staged); err != nil {
		return fmt.Errorf("lsp managed: rust-analyzer gunzip: %w", err)
	}
	if err := os.Chmod(staged, 0o755); err != nil {
		return err
	}
	if err := os.Rename(staged, dest); err != nil {
		if err := copyFile(staged, dest); err != nil {
			return err
		}
	}
	return writeManagedVersion(dataDir, "rust-analyzer", RustAnalyzerRelease)
}
