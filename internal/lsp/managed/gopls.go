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

// GoplsRelease is the pinned gopls module version installed into DataDir.
const GoplsRelease = "v0.23.0"

const goplsModule = "golang.org/x/tools/gopls@" + GoplsRelease

func ensureGopls(ctx context.Context, dataDir string) (string, error) {
	return ensurePinnedBinary(ctx, dataDir, "gopls", GoplsRelease, EnvGopls, "gopls", nativeBinName("gopls"), installGopls)
}

func installGopls(ctx context.Context, dataDir, dest string) error {
	goBin, err := exec.LookPath("go")
	if err != nil {
		return fmt.Errorf("lsp managed: go not on PATH (needed to install gopls): %w", err)
	}
	dir := filepath.Dir(dest)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()

	cmd := exec.CommandContext(ctx, goBin, "install", goplsModule)
	cmd.Env = append(os.Environ(), "GOBIN="+dir)
	if out, err := cmd.CombinedOutput(); err != nil {
		msg := strings.TrimSpace(string(out))
		if msg == "" {
			msg = err.Error()
		}
		return fmt.Errorf("lsp managed: go install gopls: %s", msg)
	}
	if runtime.GOOS == "windows" {
		// go install may write gopls.exe while dest already ends in .exe.
	}
	if !isExecutableFile(dest) {
		return fmt.Errorf("lsp managed: go install succeeded but %s missing", dest)
	}
	if err := os.Chmod(dest, 0o755); err != nil {
		return err
	}
	return writeManagedVersion(dataDir, "gopls", GoplsRelease)
}
