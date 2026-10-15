package code

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"time"
)

// OpenInEditor opens path:line in an external editor. Prefers $EDITOR / $VISUAL,
// then Cursor, then VS Code, then the OS default opener.
func OpenInEditor(absPath string, line int) error {
	if st, err := os.Stat(absPath); err != nil || st.IsDir() {
		return fmt.Errorf("cannot open %s", absPath)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	target := absPath
	if line > 0 {
		target = absPath + ":" + strconv.Itoa(line)
	}

	candidates := [][]string{}
	if ed := os.Getenv("RECUR_EDITOR"); ed != "" {
		candidates = append(candidates, []string{ed, target})
	}
	// Cursor / VS Code understand path:line.
	candidates = append(candidates,
		[]string{"cursor", "-g", target},
		[]string{"code", "-g", target},
	)
	if ed := firstNonEmpty(os.Getenv("VISUAL"), os.Getenv("EDITOR")); ed != "" {
		if line > 0 {
			candidates = append(candidates, []string{ed, "+" + strconv.Itoa(line), absPath})
		} else {
			candidates = append(candidates, []string{ed, absPath})
		}
	}
	switch runtime.GOOS {
	case "darwin":
		candidates = append(candidates, []string{"open", absPath})
	case "linux":
		candidates = append(candidates, []string{"xdg-open", absPath})
	}

	var last error
	for _, c := range candidates {
		cmd := exec.CommandContext(ctx, c[0], c[1:]...)
		if err := cmd.Start(); err != nil {
			last = err
			continue
		}
		_ = cmd.Process.Release()
		return nil
	}
	if last != nil {
		return last
	}
	return fmt.Errorf("no editor available")
}

// RevealInFinder shows the file/folder in the OS file manager.
// Do not use CommandContext here: cancelling on return kills `open` before
// Finder activates (same Release pattern as OpenInEditor).
func RevealInFinder(absPath string) error {
	target := absPath
	if _, err := os.Stat(absPath); err != nil {
		// Deleted / missing — reveal the parent folder if possible.
		parent := filepath.Dir(absPath)
		if st, e2 := os.Stat(parent); e2 != nil || !st.IsDir() {
			return err
		}
		target = parent
	}
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", "-R", target)
	case "linux":
		// xdg-open has no "select"; open the containing folder instead of the file.
		if st, err := os.Stat(target); err == nil && !st.IsDir() {
			target = filepath.Dir(target)
		}
		cmd = exec.Command("xdg-open", target)
	default:
		return fmt.Errorf("reveal not supported on %s", runtime.GOOS)
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	_ = cmd.Process.Release()
	return nil
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}
