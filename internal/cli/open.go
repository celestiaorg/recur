package cli

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"time"

	"github.com/spf13/cobra"

	"recur/internal/cliinstall"
	gitpkg "recur/internal/git"
	"recur/internal/openipc"
)

const (
	openRetryFor  = 30 * time.Second
	openRetryWait = 250 * time.Millisecond
	recurBundleID = "build.recur.app"
)

func newOpenCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "open [path]",
		Short: "Open a file or folder in Recur",
		Long: `Open a file or folder in the Recur desktop app.

  recur .
  recur ./path/to/folder-or-file
  recur open .

If Recur is already running, the path is handed to that instance. Otherwise
the app is launched (Recur.app) and the path is opened when it is ready.`,
		Args: cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			path := ""
			if len(args) > 0 {
				path = args[0]
			}
			return runOpen(path)
		},
	}
}

func runOpen(arg string) error {
	var abs string
	if arg != "" {
		resolved, err := resolveOpenArg(arg)
		if err != nil {
			return &exitError{code: 2, err: err}
		}
		abs = resolved
	}
	if err := sendOrLaunch(abs); err != nil {
		return &exitError{code: 1, err: err}
	}
	return nil
}

func resolveOpenArg(arg string) (string, error) {
	abs, err := gitpkg.CanonPath(arg)
	if err != nil {
		return "", err
	}
	if _, err := os.Stat(abs); err != nil {
		return "", fmt.Errorf("%s: %w", arg, err)
	}
	return abs, nil
}

func sendOrLaunch(abs string) error {
	sock, err := openipc.SockPath()
	if err != nil {
		return err
	}
	err = callOpen(sock, abs)
	if err == nil {
		return nil
	}
	if !openRetryable(err) {
		return err
	}
	launched := false
	if openipc.IsDialError(err) {
		if err := launchRecur(abs); err != nil {
			return fmt.Errorf("Recur isn't running and could not be launched: %w\nStart Recur (or `wails3 task dev`) and retry", err)
		}
		launched = true
	}
	deadline := time.Now().Add(openRetryFor)
	var last error
	for time.Now().Before(deadline) {
		last = callOpen(sock, abs)
		if last == nil {
			return nil
		}
		if !openRetryable(last) {
			return last
		}
		if !launched && openipc.IsDialError(last) {
			if err := launchRecur(abs); err != nil {
				return fmt.Errorf("Recur isn't running and could not be launched: %w\nStart Recur (or `wails3 task dev`) and retry", err)
			}
			launched = true
		}
		time.Sleep(openRetryWait)
	}
	if last == nil {
		last = fmt.Errorf("timed out waiting for Recur")
	}
	return fmt.Errorf("Recur launched but did not accept the path: %w", last)
}

func callOpen(sock, abs string) error {
	res, err := openipc.Call(sock, openipc.Request{Path: abs})
	if err != nil {
		return err
	}
	if openipc.Retryable(res) {
		return openipc.ErrUnavailable
	}
	if !res.OK {
		if res.Error == "" {
			return fmt.Errorf("recur: open failed")
		}
		return fmt.Errorf("%s", res.Error)
	}
	return nil
}

func openRetryable(err error) bool {
	return openipc.IsDialError(err) || errors.Is(err, openipc.ErrUnavailable)
}

func launchRecur(abs string) error {
	if runtime.GOOS == "windows" {
		return fmt.Errorf("opening from the CLI is not supported on Windows yet")
	}
	var extra []string
	if abs != "" {
		extra = []string{"--open", abs}
	}
	if env := os.Getenv("RECUR_APP"); env != "" {
		return exec.Command(env, extra...).Start()
	}
	if runtime.GOOS == "darwin" {
		openArgs := []string{"-b", recurBundleID}
		if len(extra) > 0 {
			openArgs = append(openArgs, "--args")
			openArgs = append(openArgs, extra...)
		}
		if err := exec.Command("open", openArgs...).Run(); err == nil {
			return nil
		}
		named := []string{"-a", "Recur"}
		if len(extra) > 0 {
			named = append(named, "--args")
			named = append(named, extra...)
		}
		if err := exec.Command("open", named...).Run(); err == nil {
			return nil
		}
	}
	if bin, err := lookDesktopBinary(); err == nil {
		return exec.Command(bin, extra...).Start()
	}
	return fmt.Errorf("Recur.app not found")
}

func lookDesktopBinary() (string, error) {
	// Local `wails3 task build` output, next to the CLI if they share bin/.
	if exe, err := os.Executable(); err == nil {
		cand := filepath.Join(filepath.Dir(exe), "recur")
		if st, err := os.Stat(cand); err == nil && !st.IsDir() && !isCLIBinary(cand) {
			return cand, nil
		}
	}
	if p, err := exec.LookPath("recur"); err == nil && !isCLIBinary(p) {
		return p, nil
	}
	return "", fmt.Errorf("no desktop binary")
}

// isCLIBinary reports this process or the Recur.app-shipped CLI (the file
// launch-time install puts on PATH). Executing it as the desktop app would
// recurse.
func isCLIBinary(p string) bool {
	if self, err := os.Executable(); err == nil {
		if same, _ := sameFile(p, self); same {
			return true
		}
	}
	resolved := p
	if r, err := filepath.EvalSymlinks(p); err == nil {
		resolved = r
	}
	return cliinstall.IsBundledCLIPath(p) || cliinstall.IsBundledCLIPath(resolved)
}

func sameFile(a, b string) (bool, error) {
	fa, err := os.Stat(a)
	if err != nil {
		return false, err
	}
	fb, err := os.Stat(b)
	if err != nil {
		return false, err
	}
	return os.SameFile(fa, fb), nil
}
