package cli

import (
	"embed"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"unicode"
)

// Scaffold tree for `recur bundles create`. Edit these files in-repo; they are
// embedded at build time (not seeded into the app DataDir).
//
//go:embed all:scaffold
var scaffoldFS embed.FS

var safeBundleName = regexp.MustCompile(`^[A-Za-z][A-Za-z0-9_-]*$`)

// createBundle scaffolds a new user bundle under destParent/<name> by copying
// the embedded scaffold/ tree and substituting {{BUNDLE_NAME}}, {{BUNDLE_ID}},
// {{TS_CMD_ID}}, {{SH_CMD_ID}}.
func createBundle(name, destParent string, force bool) (string, error) {
	if !safeBundleName.MatchString(name) {
		return "", fmt.Errorf("invalid bundle name %q (use letters, numbers, _ or -; start with a letter)", name)
	}
	if destParent == "" {
		var err error
		destParent, err = bundlesDir()
		if err != nil {
			return "", err
		}
	}
	dest := filepath.Join(destParent, name)
	if st, err := os.Stat(dest); err == nil {
		if !force {
			return "", fmt.Errorf("%s already exists (pass --force to replace)", dest)
		}
		if !st.IsDir() {
			return "", fmt.Errorf("%s exists and is not a directory", dest)
		}
		if err := os.RemoveAll(dest); err != nil {
			return "", err
		}
	}
	if err := os.MkdirAll(dest, 0o755); err != nil {
		return "", err
	}

	bundleID := scaffoldBundleID(name)
	repl := strings.NewReplacer(
		"{{BUNDLE_NAME}}", name,
		"{{BUNDLE_ID}}", bundleID,
		"{{TS_CMD_ID}}", bundleID+".uppercase",
		"{{SH_CMD_ID}}", bundleID+".prefix-lines",
	)

	root := "scaffold"
	err := fs.WalkDir(scaffoldFS, root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		if rel == "." {
			return nil
		}
		target := filepath.Join(dest, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		data, err := scaffoldFS.ReadFile(path)
		if err != nil {
			return err
		}
		body := repl.Replace(string(data))
		mode := os.FileMode(0o644)
		if strings.HasPrefix(filepath.ToSlash(rel), "bin/") || strings.HasSuffix(rel, ".sh") {
			mode = 0o755
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return err
		}
		return os.WriteFile(target, []byte(body), mode)
	})
	if err != nil {
		return "", err
	}
	return dest, nil
}

func normalizeBundleName(name string) string {
	return strings.TrimSpace(name)
}

// scaffoldBundleID turns a display name into a dotted-safe id (e.g. MyBundle → mybundle).
func scaffoldBundleID(name string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(strings.TrimSpace(name)) {
		switch {
		case unicode.IsLetter(r) || unicode.IsDigit(r):
			b.WriteRune(r)
		case r == '-' || r == '_':
			b.WriteRune(r)
		}
	}
	id := b.String()
	if id == "" {
		return "bundle"
	}
	return id
}
