package cliinstall

import (
	"bytes"
	"context"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

// pathMark wraps $PATH so rc-file greetings (oh-my-zsh, nvm) cannot be
// mistaken for the value. Same idea VS Code / GitHub Desktop use.
const pathMark = "__RECUR_PATH_MARK__"

// shellPATHTimeout bounds the login-shell probe. A blocking read in .zshrc
// must not stall Recur.app launch.
const shellPATHTimeout = 5 * time.Second

// readLoginPATH is the production probe. Tests replace it so EnsurePATH does
// not spawn the real user shell.
var readLoginPATH = captureLoginPATH

var captureOnce struct {
	once sync.Once
	path string
}

func captureLoginPATH() string {
	captureOnce.once.Do(func() {
		captureOnce.path = readShellPATH(userLoginShell(), shellPATHTimeout)
	})
	return captureOnce.path
}

func userLoginShell() string {
	if sh := strings.TrimSpace(os.Getenv("SHELL")); sh != "" {
		return sh
	}
	if sh := strings.TrimSpace(passwdShell()); sh != "" {
		return sh
	}
	if runtime.GOOS == "darwin" {
		return "/bin/zsh"
	}
	return "/bin/sh"
}

func passwdShell() string {
	if runtime.GOOS == "darwin" {
		if sh := darwinIDShell(); sh != "" {
			return sh
		}
	}
	return shellFromPasswdFile("/etc/passwd", os.Getuid())
}

// darwinIDShell is getpwuid for macOS without cgo. `id -P` prints the
// Directory Services passwd line; the login shell is the last field.
func darwinIDShell() string {
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, "id", "-P").Output()
	if err != nil {
		return ""
	}
	fields := strings.Split(strings.TrimSpace(string(out)), ":")
	if len(fields) < 10 {
		return ""
	}
	return fields[len(fields)-1]
}

func shellFromPasswdFile(path string, uid int) string {
	raw, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	want := strconv.Itoa(uid)
	for _, line := range strings.Split(string(raw), "\n") {
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		fields := strings.Split(line, ":")
		if len(fields) < 7 {
			continue
		}
		if fields[2] != want {
			continue
		}
		return fields[6]
	}
	return ""
}

func shellPATHArgs(shell string) []string {
	name := strings.ToLower(filepath.Base(shell))
	name = strings.TrimPrefix(name, "-")
	switch name {
	case "fish":
		return []string{"-l", "-i", "-c", `printf '` + pathMark + `%s` + pathMark + `' (string join : $PATH)`}
	default:
		return []string{"-ilc", `printf '` + pathMark + `%s` + pathMark + `' "$PATH"`}
	}
}

func readShellPATH(shell string, timeout time.Duration) string {
	if strings.TrimSpace(shell) == "" {
		return ""
	}
	if timeout <= 0 {
		timeout = shellPATHTimeout
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()

	cmd := exec.Command(shell, shellPATHArgs(shell)...) //nolint:gosec // shell is $SHELL / passwd
	cmd.Stdin = nil                                     // rc files must not block on a prompt
	cmd.Stderr = io.Discard
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	var stdout bytes.Buffer
	cmd.Stdout = &stdout
	if err := cmd.Start(); err != nil {
		return ""
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	select {
	case err := <-done:
		if err != nil {
			return ""
		}
	case <-ctx.Done():
		if cmd.Process != nil {
			_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
		}
		<-done
		return ""
	}
	p, ok := parsePATHMarks(stdout.String())
	if !ok {
		return ""
	}
	return p
}

func parsePATHMarks(out string) (string, bool) {
	parts := strings.Split(out, pathMark)
	if len(parts) < 3 {
		return "", false
	}
	p := strings.TrimSpace(parts[1])
	if p == "" || strings.ContainsAny(p, "\n\r") {
		return "", false
	}
	return p, true
}
