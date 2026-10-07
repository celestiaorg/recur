package lsp

import (
	"fmt"
	"net/url"
	"path/filepath"
	"runtime"
	"strings"
)

// CanonPath returns the symlink-resolved absolute path, or a cleaned path
// when the target does not exist yet.
func CanonPath(absPath string) string {
	abs := filepath.Clean(absPath)
	if real, err := filepath.EvalSymlinks(abs); err == nil {
		return real
	}
	return abs
}

// PathToURI converts an absolute filesystem path to a file:// URI.
func PathToURI(absPath string) string {
	abs := filepath.Clean(absPath)
	if runtime.GOOS == "windows" {
		// file:///C:/path
		p := filepath.ToSlash(abs)
		if !strings.HasPrefix(p, "/") {
			p = "/" + p
		}
		return "file://" + p
	}
	return (&url.URL{Scheme: "file", Path: abs}).String()
}

// URIToPath converts a file:// URI to a filesystem path.
func URIToPath(uri string) (string, error) {
	u, err := url.Parse(uri)
	if err != nil {
		return "", err
	}
	if u.Scheme != "file" {
		return "", fmt.Errorf("not a file URI: %s", uri)
	}
	p := u.Path
	if runtime.GOOS == "windows" && strings.HasPrefix(p, "/") && len(p) >= 3 && p[2] == ':' {
		p = p[1:]
	}
	return filepath.FromSlash(p), nil
}
