package managed

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
)

// TaploRelease is the pinned taplo CLI GitHub release tag (`taplo lsp stdio`).
const TaploRelease = "0.10.0"

const taploBase = "https://github.com/tamasfe/taplo/releases/download/"

var taploSHA256 = map[string]string{
	"taplo-darwin-aarch64.gz": "713734314c3e71894b9e77513c5349835eefbd52908445a0d73b0c7dc469347d",
	"taplo-darwin-x86_64.gz":  "898122cde3a0b1cd1cbc2d52d3624f23338218c91b5ddb71518236a4c2c10ef2",
	"taplo-linux-aarch64.gz":  "033681d01eec8376c3fd38fa3703c79316f5e14bb013d859943b60a07bccdcc3",
	"taplo-linux-x86_64.gz":   "8fe196b894ccf9072f98d4e1013a180306e17d244830b03986ee5e8eabeb6156",
}

func ensureTaplo(ctx context.Context, dataDir string) (string, error) {
	return ensurePinnedBinary(ctx, dataDir, "taplo", TaploRelease, EnvTaplo, "taplo", nativeBinName("taplo"), installTaplo)
}

func taploAsset() (string, error) {
	switch runtime.GOOS + "/" + runtime.GOARCH {
	case "darwin/arm64":
		return "taplo-darwin-aarch64.gz", nil
	case "darwin/amd64":
		return "taplo-darwin-x86_64.gz", nil
	case "linux/arm64":
		return "taplo-linux-aarch64.gz", nil
	case "linux/amd64":
		return "taplo-linux-x86_64.gz", nil
	default:
		return "", fmt.Errorf("lsp managed: unsupported platform %s/%s for taplo", runtime.GOOS, runtime.GOARCH)
	}
}

func installTaplo(ctx context.Context, dataDir, dest string) error {
	asset, err := taploAsset()
	if err != nil {
		return err
	}
	want, ok := taploSHA256[asset]
	if !ok {
		return fmt.Errorf("lsp managed: no pinned SHA-256 for %s", asset)
	}
	dir := filepath.Dir(dest)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	staging, err := os.MkdirTemp(dir, ".taplo-staging-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(staging)

	archive := filepath.Join(staging, asset)
	url := taploBase + TaploRelease + "/" + asset
	if err := downloadVerified(ctx, url, want, archive); err != nil {
		return err
	}
	if err := gunzipFile(archive, dest); err != nil {
		return fmt.Errorf("lsp managed: taplo gunzip: %w", err)
	}
	if err := os.Chmod(dest, 0o755); err != nil {
		return err
	}
	return writeManagedVersion(dataDir, "taplo", TaploRelease)
}
