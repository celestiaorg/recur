package rg

import (
	"archive/tar"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// ReleaseVersion is the ripgrep release Recur downloads into DataDir()/bin/rg.
// Bumping this causes Ensure to replace a managed install whose rg.version
// sidecar does not match (PATH / $RECUR_RG overrides are left alone).
const ReleaseVersion = "15.2.0"

const releaseBase = "https://github.com/BurntSushi/ripgrep/releases/download/"

// releaseSHA256 pins expected digests for ReleaseVersion tarballs (from the
// upstream .sha256 assets). Linux uses the static musl builds. Bump these together with ReleaseVersion.
var releaseSHA256 = map[string]string{
	"ripgrep-15.2.0-aarch64-apple-darwin.tar.gz":       "3750b2e93f37e0c692657da574d7019a101c0084da05a790c83fd335bad973e4",
	"ripgrep-15.2.0-x86_64-apple-darwin.tar.gz":        "af7825fcc69a2afc7a7aea55fc9af90e26421d8f20fe59df32e233c0b8a231c1",
	"ripgrep-15.2.0-aarch64-unknown-linux-musl.tar.gz": "800b1e7206afe799dfb5a6901f23147cfaabe0e52210538100f61e86e1740915",
	"ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz":  "33e15bcf1624b25cdd2a55813a47a2f95dbe126268203e76aa6a585d1e7b149c",
}

// VersionFileName sits next to the managed binary so we can detect stale installs.
const VersionFileName = "rg.version"

// Ensure returns a usable rg binary path. Managed installs under dataDir/bin
// are upgraded when their sidecar version ≠ ReleaseVersion. $RECUR_RG and a
// PATH-only rg are never auto-replaced.
func Ensure(ctx context.Context, dataDir string) (string, error) {
	if p := os.Getenv(EnvPath); p != "" && isExecutableFile(p) {
		return p, nil
	}
	if dataDir != "" {
		managed := filepath.Join(dataDir, "bin", BinName)
		if isExecutableFile(managed) && managedVersion(dataDir) == ReleaseVersion {
			return managed, nil
		}
		// Missing, unversioned, or stale — (re)install the pinned release.
		if err := install(ctx, dataDir); err != nil {
			// Soft-fail to whatever Locate can still find (stale managed / PATH).
			if p, locErr := Locate(dataDir); locErr == nil {
				return p, nil
			}
			return "", err
		}
		return managed, nil
	}
	if p, err := exec.LookPath(BinName); err == nil && isExecutableFile(p) {
		return p, nil
	}
	return "", fmt.Errorf("rg ensure: no data directory and ripgrep not on PATH")
}

func managedVersion(dataDir string) string {
	b, err := os.ReadFile(filepath.Join(dataDir, "bin", VersionFileName))
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(b))
}

func writeManagedVersion(dataDir string) error {
	dir := filepath.Join(dataDir, "bin")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(dir, VersionFileName), []byte(ReleaseVersion+"\n"), 0o644)
}

func install(ctx context.Context, dataDir string) error {
	asset, err := releaseAsset()
	if err != nil {
		return err
	}
	url := releaseBase + ReleaseVersion + "/" + asset
	ctx, cancel := context.WithTimeout(ctx, 3*time.Minute)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("rg download: %w", err)
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusOK {
		return fmt.Errorf("rg download: HTTP %s", res.Status)
	}

	want, ok := releaseSHA256[asset]
	if !ok {
		return fmt.Errorf("rg download: no pinned SHA-256 for %s", asset)
	}

	binDir := filepath.Join(dataDir, "bin")
	if err := os.MkdirAll(binDir, 0o755); err != nil {
		return err
	}
	staging, err := os.MkdirTemp(binDir, ".rg-staging-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(staging)

	// Fully buffer + hash before extract — TeeReader during extract would miss
	// unread archive tail and fail the digest check.
	archivePath := filepath.Join(staging, asset)
	af, err := os.Create(archivePath)
	if err != nil {
		return err
	}
	hasher := sha256.New()
	if _, err := io.Copy(io.MultiWriter(af, hasher), res.Body); err != nil {
		af.Close()
		return fmt.Errorf("rg download: %w", err)
	}
	if err := af.Close(); err != nil {
		return err
	}
	got := hex.EncodeToString(hasher.Sum(nil))
	if got != want {
		return fmt.Errorf("rg download: SHA-256 mismatch for %s: got %s want %s", asset, got, want)
	}

	staged := filepath.Join(staging, BinName)
	ar, err := os.Open(archivePath)
	if err != nil {
		return err
	}
	extractErr := extractRg(ar, staged)
	_ = ar.Close()
	if extractErr != nil {
		return extractErr
	}
	if err := os.Chmod(staged, 0o755); err != nil {
		return err
	}
	dest := filepath.Join(binDir, BinName)
	if err := os.Rename(staged, dest); err != nil {
		// Cross-device fallback.
		if err := copyFile(staged, dest); err != nil {
			return err
		}
	}
	if err := writeManagedVersion(dataDir); err != nil {
		return fmt.Errorf("rg install: write version: %w", err)
	}
	return nil
}

func releaseAsset() (string, error) {
	var triple string
	switch runtime.GOOS + "/" + runtime.GOARCH {
	case "darwin/arm64":
		triple = "aarch64-apple-darwin"
	case "darwin/amd64":
		triple = "x86_64-apple-darwin"
	case "linux/arm64":
		triple = "aarch64-unknown-linux-musl"
	case "linux/amd64":
		triple = "x86_64-unknown-linux-musl"
	default:
		return "", fmt.Errorf("rg ensure: unsupported platform %s/%s (install ripgrep on PATH)", runtime.GOOS, runtime.GOARCH)
	}
	return fmt.Sprintf("ripgrep-%s-%s.tar.gz", ReleaseVersion, triple), nil
}

// extractRg pulls the `rg` member out of a ripgrep release tarball.
func extractRg(r io.Reader, dest string) error {
	gz, err := gzip.NewReader(r)
	if err != nil {
		return fmt.Errorf("rg download: gzip: %w", err)
	}
	defer gz.Close()
	tr := tar.NewReader(gz)
	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			return fmt.Errorf("rg download: no rg binary in archive")
		}
		if err != nil {
			return err
		}
		name := filepath.Base(hdr.Name)
		if name != BinName || hdr.Typeflag != tar.TypeReg {
			continue
		}
		// Reject path tricks inside the archive member name.
		if strings.Contains(hdr.Name, "..") {
			continue
		}
		f, err := os.OpenFile(dest, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o755)
		if err != nil {
			return err
		}
		_, copyErr := io.Copy(f, tr)
		closeErr := f.Close()
		if copyErr != nil {
			return copyErr
		}
		return closeErr
	}
}

func copyFile(src, dest string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(dest, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o755)
	if err != nil {
		return err
	}
	_, copyErr := io.Copy(out, in)
	closeErr := out.Close()
	if copyErr != nil {
		return copyErr
	}
	return closeErr
}
