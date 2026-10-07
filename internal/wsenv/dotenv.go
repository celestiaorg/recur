package wsenv

import (
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"

	gitpkg "recur/internal/git"
)

// DotenvFiles are repo-relative paths read as expansion lookup for project
// env values. Later entries win. Missing files are skipped.
var DotenvFiles = []string{
	".env",
	".env.local",
	".recur/.env",
	".recur/.env.local",
}

const dotenvMaxBytes = 256 << 10

// ParseDotenv parses KEY=value lines into a map. Blank lines and # comments are
// ignored. Values may be optionally single- or double-quoted; quotes are stripped.
// Later keys win.
func ParseDotenv(text string) map[string]string {
	out := map[string]string{}
	for _, line := range strings.Split(text, "\n") {
		s := strings.TrimSpace(line)
		if s == "" || strings.HasPrefix(s, "#") {
			continue
		}
		// export KEY=value
		s = strings.TrimPrefix(s, "export ")
		s = strings.TrimSpace(s)
		i := strings.IndexByte(s, '=')
		if i <= 0 {
			continue
		}
		k := strings.TrimSpace(s[:i])
		if k == "" {
			continue
		}
		v := strings.TrimSpace(s[i+1:])
		if len(v) >= 2 {
			if (v[0] == '"' && v[len(v)-1] == '"') || (v[0] == '\'' && v[len(v)-1] == '\'') {
				v = v[1 : len(v)-1]
			}
		}
		out[k] = v
	}
	return out
}

// LoadDotenvFiles reads DotenvFiles from each existing directory in dirs
// (typically the project checkout, then the worktree). Later dirs and later
// files win. Unreadable or oversized files are logged and skipped.
func LoadDotenvFiles(dirs ...string) map[string]string {
	out := map[string]string{}
	seen := map[string]bool{}
	for _, dir := range dirs {
		dir = strings.TrimSpace(gitpkg.ExpandHome(dir))
		if dir == "" {
			continue
		}
		abs, err := filepath.Abs(dir)
		if err != nil {
			continue
		}
		abs = filepath.Clean(abs)
		if seen[abs] {
			continue
		}
		st, err := os.Stat(abs)
		if err != nil || !st.IsDir() {
			continue
		}
		seen[abs] = true
		for _, rel := range DotenvFiles {
			mergeDotenvFile(out, filepath.Join(abs, filepath.FromSlash(rel)))
		}
	}
	return out
}

func mergeDotenvFile(into map[string]string, path string) {
	st, err := os.Stat(path)
	if err != nil {
		if !os.IsNotExist(err) {
			log.Printf("recur: dotenv %s: %v", path, err)
		}
		return
	}
	if !st.Mode().IsRegular() {
		return
	}
	if st.Size() > dotenvMaxBytes {
		log.Printf("recur: dotenv %s: skipping (larger than %d bytes)", path, dotenvMaxBytes)
		return
	}
	f, err := os.Open(path)
	if err != nil {
		log.Printf("recur: dotenv %s: %v", path, err)
		return
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, dotenvMaxBytes+1))
	if err != nil {
		log.Printf("recur: dotenv %s: %v", path, err)
		return
	}
	if len(data) > dotenvMaxBytes {
		log.Printf("recur: dotenv %s: skipping (larger than %d bytes)", path, dotenvMaxBytes)
		return
	}
	for k, v := range ParseDotenv(string(data)) {
		into[k] = v
	}
}
