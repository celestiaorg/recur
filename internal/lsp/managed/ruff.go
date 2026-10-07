package managed

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
)

// RuffRelease is the pinned ruff GitHub release tag (native `ruff server` LSP).
const RuffRelease = "0.16.2"

const ruffBase = "https://github.com/astral-sh/ruff/releases/download/"

var ruffSHA256 = map[string]string{
	"ruff-aarch64-apple-darwin.tar.gz":      "fbf6cbc23d254b0bc03a6fb2b1b04efb917fe5ce068d027e735ce7ed65b9bed6",
	"ruff-x86_64-apple-darwin.tar.gz":       "6648fa7a7c95b087c5b9d269d8b9a567fae091bdef3993f77cc7531a01bd7266",
	"ruff-aarch64-unknown-linux-gnu.tar.gz": "b2a2a2573455cc33af98f8a8fb49294c02d4e2e4a7f9e81844411f0a57f30318",
	"ruff-x86_64-unknown-linux-gnu.tar.gz":  "3d2c355e641ceb5b608a158c603768fcc908c5009c56c6e78da7487da033b92a",
}

func ensureRuff(ctx context.Context, dataDir string) (string, error) {
	return ensurePinnedBinary(ctx, dataDir, "ruff", RuffRelease, EnvRuff, "ruff", nativeBinName("ruff"), installRuff)
}

func ruffAsset() (string, error) {
	switch runtime.GOOS + "/" + runtime.GOARCH {
	case "darwin/arm64":
		return "ruff-aarch64-apple-darwin.tar.gz", nil
	case "darwin/amd64":
		return "ruff-x86_64-apple-darwin.tar.gz", nil
	case "linux/arm64":
		return "ruff-aarch64-unknown-linux-gnu.tar.gz", nil
	case "linux/amd64":
		return "ruff-x86_64-unknown-linux-gnu.tar.gz", nil
	default:
		return "", fmt.Errorf("lsp managed: unsupported platform %s/%s for ruff", runtime.GOOS, runtime.GOARCH)
	}
}

func installRuff(ctx context.Context, dataDir, dest string) error {
	asset, err := ruffAsset()
	if err != nil {
		return err
	}
	want, ok := ruffSHA256[asset]
	if !ok {
		return fmt.Errorf("lsp managed: no pinned SHA-256 for %s", asset)
	}
	dir := filepath.Dir(dest)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	staging, err := os.MkdirTemp(dir, ".ruff-staging-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(staging)

	archive := filepath.Join(staging, asset)
	url := ruffBase + RuffRelease + "/" + asset
	if err := downloadVerified(ctx, url, want, archive); err != nil {
		return err
	}
	if err := extractTarGzNamed(archive, dest, filepath.Base(dest)); err != nil {
		return err
	}
	if err := os.Chmod(dest, 0o755); err != nil {
		return err
	}
	return writeManagedVersion(dataDir, "ruff", RuffRelease)
}
