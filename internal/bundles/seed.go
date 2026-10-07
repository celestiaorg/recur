package bundles

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

// seedStampName is written inside each seeded bundle directory. Its contents are
// the hex SHA-256 of the shipped tree. When the shipped hash changes, the whole
// destination directory is replaced so upgrades pick up new defaults.
//
// Customize via ~/.recur/bundles (id wins) — edits under <DataDir>/Bundles are
// overwritten on the next shipped change.
const seedStampName = ".recur-seed"

// SeedDefaults copies shipped default bundles from src (rooted at root, e.g.
// "bundles") into destDir. Each top-level directory is content-hashed; when the
// hash differs from dest's .recur-seed stamp (or the stamp is missing), that
// directory is replaced entirely. Unchanged bundles are left alone so local
// tweaks survive until the next shipped update. Top-level loose files are
// overwritten when their bytes differ.
//
// User bundles in ~/.recur/bundles still win on id collision via Discover.
//
// The canonical source in the repo is top-level bundles/. main.go embeds that
// tree (go:embed cannot reach outside its package, so the embed lives in main,
// not here) and passes it in. Tests may pass os.DirFS("…/bundles") with root ".".
func SeedDefaults(destDir string, src fs.FS, root string) error {
	if src == nil {
		return fmt.Errorf("seed defaults: nil source FS")
	}
	if root == "" {
		root = "."
	}
	if err := os.MkdirAll(destDir, 0o755); err != nil {
		return err
	}
	entries, err := fs.ReadDir(src, root)
	if err != nil {
		return fmt.Errorf("seed defaults: read %s: %w", root, err)
	}
	for _, e := range entries {
		name := e.Name()
		if name == seedStampName || strings.HasPrefix(name, ".") {
			continue
		}
		srcPath := pathJoin(root, name)
		if e.IsDir() {
			if err := seedBundleDir(destDir, src, srcPath, name); err != nil {
				return err
			}
			continue
		}
		if err := seedRootFile(destDir, src, srcPath, name); err != nil {
			return err
		}
	}
	return nil
}

func seedBundleDir(destDir string, src fs.FS, srcPath, name string) error {
	hash, err := hashTree(src, srcPath)
	if err != nil {
		return fmt.Errorf("seed hash %s: %w", name, err)
	}
	destBundle := filepath.Join(destDir, name)
	stampPath := filepath.Join(destBundle, seedStampName)
	if cur, err := os.ReadFile(stampPath); err == nil && strings.TrimSpace(string(cur)) == hash {
		return nil
	}
	// Replace the whole directory so removed files disappear.
	if err := os.RemoveAll(destBundle); err != nil {
		return fmt.Errorf("seed replace %s: %w", name, err)
	}
	if err := copyTree(src, srcPath, destBundle); err != nil {
		return err
	}
	if err := os.WriteFile(stampPath, []byte(hash+"\n"), 0o644); err != nil {
		return fmt.Errorf("seed stamp %s: %w", name, err)
	}
	return nil
}

func seedRootFile(destDir string, src fs.FS, srcPath, name string) error {
	data, err := fs.ReadFile(src, srcPath)
	if err != nil {
		return fmt.Errorf("seed read %s: %w", name, err)
	}
	target := filepath.Join(destDir, name)
	if cur, err := os.ReadFile(target); err == nil && string(cur) == string(data) {
		return nil
	}
	mode := os.FileMode(0o644)
	if strings.HasSuffix(name, ".sh") {
		mode = 0o755
	}
	if err := os.WriteFile(target, data, mode); err != nil {
		return fmt.Errorf("seed %s: %w", name, err)
	}
	return nil
}

func hashTree(src fs.FS, dir string) (string, error) {
	h := sha256.New()
	err := fs.WalkDir(src, dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if skipSeedPath(dir, path, d) {
			if d.IsDir() {
				return fs.SkipDir
			}
			return nil
		}
		if d.IsDir() {
			return nil
		}
		rel, err := filepath.Rel(dir, path)
		if err != nil {
			return err
		}
		rel = filepath.ToSlash(rel)
		data, err := fs.ReadFile(src, path)
		if err != nil {
			return err
		}
		// path\0len\0bytes — stable, collision-resistant for tree identity
		fmt.Fprintf(h, "%s\x00%d\x00", rel, len(data))
		_, _ = h.Write(data)
		_, _ = h.Write([]byte{0})
		return nil
	})
	if err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

func copyTree(src fs.FS, srcDir, destDir string) error {
	return fs.WalkDir(src, srcDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(srcDir, path)
		if err != nil {
			return err
		}
		if rel == "." {
			return os.MkdirAll(destDir, 0o755)
		}
		if skipSeedPath(srcDir, path, d) {
			if d.IsDir() {
				return fs.SkipDir
			}
			return nil
		}
		target := filepath.Join(destDir, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		data, err := fs.ReadFile(src, path)
		if err != nil {
			return err
		}
		mode := os.FileMode(0o644)
		slash := filepath.ToSlash(path)
		if strings.HasSuffix(path, ".sh") || strings.Contains(slash, "/bin/") {
			mode = 0o755
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return err
		}
		if err := os.WriteFile(target, data, mode); err != nil {
			return fmt.Errorf("seed %s: %w", rel, err)
		}
		return nil
	})
}

func pathJoin(root, name string) string {
	if root == "." {
		return name
	}
	return root + "/" + name
}

// skipSeedPath omits authoring-only paths from seeded runtime copies:
// .recur-seed, dotfiles, and the bundle's top-level tests/ directory.
func skipSeedPath(bundleRoot, path string, d fs.DirEntry) bool {
	base := filepath.Base(path)
	if base == seedStampName || strings.HasPrefix(base, ".") {
		return true
	}
	rel, err := filepath.Rel(bundleRoot, path)
	if err != nil {
		return false
	}
	rel = filepath.ToSlash(rel)
	return rel == "tests" || strings.HasPrefix(rel, "tests/")
}
