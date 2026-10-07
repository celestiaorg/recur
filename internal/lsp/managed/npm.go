package managed

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// npmBinPin installs an npm package (or set) into DataDir and returns a
// node_modules/.bin shim. PrefixID shares one install across multiple bins
// (vscode-langservers-extracted → json/html/css).
type npmBinPin struct {
	ID       string
	Version  string
	EnvKey   string
	PathName string
	BinName  string
	Pkgs     []string
	PrefixID string
}

func ensureNpmBin(ctx context.Context, dataDir string, pin npmBinPin) (string, error) {
	bin := pin.BinName
	if bin == "" {
		bin = pin.PathName
	}
	return ensurePinnedBinary(
		ctx, dataDir, pin.ID, pin.Version, pin.EnvKey, pin.PathName, bin,
		func(ctx context.Context, dataDir, dest string) error {
			return installNpmBin(ctx, dataDir, dest, pin)
		},
	)
}

func installNpmBin(ctx context.Context, dataDir, dest string, pin npmBinPin) error {
	prefixID := pin.PrefixID
	if prefixID == "" {
		prefixID = pin.ID
	}
	prefix := managedBinDir(dataDir, prefixID)
	if err := os.MkdirAll(prefix, 0o755); err != nil {
		return err
	}

	needInstall := managedVersion(dataDir, prefixID) != pin.Version
	if !needInstall {
		if _, err := npmBinPath(prefix, pin.BinName); err != nil {
			needInstall = true
		}
	}
	if needInstall {
		npm, err := exec.LookPath("npm")
		if err != nil {
			return fmt.Errorf("lsp managed: npm not on PATH (needed to install %s): %w", pin.ID, err)
		}
		ctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
		defer cancel()
		args := []string{"install", "--omit=dev", "--no-fund", "--no-audit", "--prefix", prefix}
		args = append(args, pin.Pkgs...)
		cmd := exec.CommandContext(ctx, npm, args...)
		if out, err := cmd.CombinedOutput(); err != nil {
			msg := strings.TrimSpace(string(out))
			if msg == "" {
				msg = err.Error()
			}
			return fmt.Errorf("lsp managed: npm install %s: %s", pin.ID, msg)
		}
		if err := writeManagedVersion(dataDir, prefixID, pin.Version); err != nil {
			return err
		}
	}

	installed, err := npmBinPath(prefix, pin.BinName)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(dest), 0o755); err != nil {
		return err
	}
	if err := os.Remove(dest); err != nil && !os.IsNotExist(err) {
		return err
	}
	if err := os.Symlink(installed, dest); err != nil {
		if err := copyFile(installed, dest); err != nil {
			return fmt.Errorf("lsp managed: link %s: %w", pin.ID, err)
		}
	}
	if err := os.Chmod(dest, 0o755); err != nil && runtime.GOOS != "windows" {
		return err
	}
	if prefixID != pin.ID {
		if err := writeManagedVersion(dataDir, pin.ID, pin.Version); err != nil {
			return err
		}
	}
	return nil
}

func npmBinPath(prefix, binName string) (string, error) {
	candidates := []string{binName}
	if runtime.GOOS == "windows" {
		candidates = []string{binName + ".cmd", binName}
	}
	for _, name := range candidates {
		p := filepath.Join(prefix, "node_modules", ".bin", name)
		if fileExists(p) {
			return p, nil
		}
	}
	return "", fmt.Errorf("lsp managed: npm install succeeded but %s missing under %s", binName, prefix)
}
