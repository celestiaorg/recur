package managed

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
)

// MarksmanRelease is the pinned marksman GitHub release tag.
const MarksmanRelease = "2026-02-08"

const marksmanBase = "https://github.com/artempyanykh/marksman/releases/download/"

// Pinned digests for MarksmanRelease (from GitHub release asset digests).
var marksmanSHA256 = map[string]string{
	"marksman-macos":       "6a801c17b5ac0dba69787c5282b3b3bd416e66c96253fae098d311c6bbd1833b",
	"marksman-linux-x64":   "be5098e8213219269c47fc0d916a66fa31ce0602ec967475c722260aabf26087",
	"marksman-linux-arm64": "db8e124527f7f8048e3e6c91821b9c52ef173d92c01e47d221bf1337afd962fb",
	"marksman.exe":         "a6d05beb08ebe41b0a9f09c98a438540421436fa5531424c22e0bb1d22529705",
}

func ensureMarksman(ctx context.Context, dataDir string) (string, error) {
	return ensurePinnedBinary(ctx, dataDir, "marksman", MarksmanRelease, EnvMarksman, "marksman", nativeBinName("marksman"), installMarksman)
}

func marksmanAsset() (string, error) {
	switch runtime.GOOS + "/" + runtime.GOARCH {
	case "darwin/arm64", "darwin/amd64":
		// Upstream ships a single macOS binary (universal).
		return "marksman-macos", nil
	case "linux/amd64":
		return "marksman-linux-x64", nil
	case "linux/arm64":
		return "marksman-linux-arm64", nil
	case "windows/amd64", "windows/arm64":
		return "marksman.exe", nil
	default:
		return "", fmt.Errorf("lsp managed: unsupported platform %s/%s for marksman", runtime.GOOS, runtime.GOARCH)
	}
}

func installMarksman(ctx context.Context, dataDir, dest string) error {
	asset, err := marksmanAsset()
	if err != nil {
		return err
	}
	want, ok := marksmanSHA256[asset]
	if !ok {
		return fmt.Errorf("lsp managed: no pinned SHA-256 for %s", asset)
	}
	dir := filepath.Dir(dest)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	staging, err := os.MkdirTemp(dir, ".marksman-staging-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(staging)

	downloaded := filepath.Join(staging, asset)
	url := marksmanBase + MarksmanRelease + "/" + asset
	if err := downloadVerified(ctx, url, want, downloaded); err != nil {
		return err
	}
	if err := os.Chmod(downloaded, 0o755); err != nil {
		return err
	}
	if err := os.Rename(downloaded, dest); err != nil {
		if err := copyFile(downloaded, dest); err != nil {
			return err
		}
	}
	if err := os.Chmod(dest, 0o755); err != nil {
		return err
	}
	return writeManagedVersion(dataDir, "marksman", MarksmanRelease)
}
