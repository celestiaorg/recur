package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/store"
)

// resolveBundleTargets expands CLI args into absolute bundle directories.
//
// Each arg may be:
//   - a path to a bundle.toml directory
//   - a directory of bundles (every child with bundle.toml)
//   - a bare name resolved under bundles-dir, ./bundles, then shipped BundlesDir
//
// With no args, uses the current directory (or its bundle children).
func resolveBundleTargets(args []string) ([]string, error) {
	if len(args) == 0 {
		args = []string{"."}
	}
	var out []string
	seen := map[string]bool{}
	add := func(dir string) error {
		abs, err := filepath.Abs(dir)
		if err != nil {
			return err
		}
		if !hasBundleToml(abs) {
			return fmt.Errorf("%s: no bundle.toml", abs)
		}
		if !seen[abs] {
			out = append(out, abs)
			seen[abs] = true
		}
		return nil
	}

	userRoot, err := bundlesDir()
	if err != nil {
		return nil, err
	}
	shippedRoot, _ := store.BundlesDir()

	for _, a := range args {
		if resolved, ok := tryDir(a); ok {
			if hasBundleToml(resolved) {
				if err := add(resolved); err != nil {
					return nil, err
				}
				continue
			}
			children, err := listBundleChildren(resolved)
			if err != nil {
				return nil, err
			}
			if len(children) > 0 {
				for _, c := range children {
					if err := add(c); err != nil {
						return nil, err
					}
				}
				continue
			}
			return nil, fmt.Errorf("%s: no bundle.toml", resolved)
		}

		candidates := []string{
			filepath.Join(userRoot, a),
			filepath.Join("bundles", a),
		}
		if shippedRoot != "" {
			candidates = append(candidates, filepath.Join(shippedRoot, a))
		}
		found := false
		for _, c := range candidates {
			if hasBundleToml(c) {
				if err := add(c); err != nil {
					return nil, err
				}
				found = true
				break
			}
		}
		if !found {
			return nil, fmt.Errorf("bundle %q not found (looked in %s, ./bundles, shipped)", a, userRoot)
		}
	}
	return out, nil
}

func tryDir(a string) (string, bool) {
	looksLikePath := a == "." || a == ".." || strings.ContainsAny(a, `/`) ||
		(len(a) >= 2 && a[1] == ':') // Windows drive
	for i := 0; i < len(a); i++ {
		if os.IsPathSeparator(a[i]) {
			looksLikePath = true
			break
		}
	}
	if !looksLikePath {
		if st, err := os.Stat(a); err == nil && st.IsDir() {
			looksLikePath = true
		}
	}
	if !looksLikePath {
		return "", false
	}
	abs, err := filepath.Abs(a)
	if err != nil {
		return "", false
	}
	st, err := os.Stat(abs)
	if err != nil || !st.IsDir() {
		return "", false
	}
	return abs, true
}

func listBundleChildren(dir string) ([]string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	var out []string
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		child := filepath.Join(dir, e.Name())
		if hasBundleToml(child) {
			out = append(out, child)
		}
	}
	return out, nil
}

func hasBundleToml(dir string) bool {
	st, err := os.Stat(filepath.Join(dir, "bundle.toml"))
	return err == nil && !st.IsDir()
}
