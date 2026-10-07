package osfonts

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

func fromFontconfig() []string {
	bin := findFcList()
	if bin == "" {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, bin, ":spacing=mono", "family").Output()
	if err != nil {
		return nil
	}
	var names []string
	for _, line := range bytes.Split(out, []byte{'\n'}) {
		s := strings.TrimSpace(string(line))
		if s != "" {
			names = append(names, s)
		}
	}
	return names
}

func findFcList() string {
	if p, err := exec.LookPath("fc-list"); err == nil {
		return p
	}
	for _, p := range []string{
		"/opt/homebrew/bin/fc-list",
		"/usr/local/bin/fc-list",
		"/usr/bin/fc-list",
	} {
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	matches, _ := filepath.Glob("/opt/homebrew/Cellar/fontconfig/*/bin/fc-list")
	if len(matches) > 0 {
		return matches[len(matches)-1]
	}
	return ""
}
