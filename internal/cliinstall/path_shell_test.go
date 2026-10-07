package cliinstall

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestParsePATHMarks(t *testing.T) {
	junk := "oh-my-zsh loading...\n" + pathMark + "/opt/homebrew/bin:/Users/me/.local/bin" + pathMark + "\n"
	got, ok := parsePATHMarks(junk)
	if !ok {
		t.Fatal("parsePATHMarks(junk) ok = false, want true")
	}
	want := "/opt/homebrew/bin:/Users/me/.local/bin"
	if got != want {
		t.Errorf("parsePATHMarks(junk) = %q, want %q", got, want)
	}
	if _, ok := parsePATHMarks("no markers here /usr/bin"); ok {
		t.Error("parsePATHMarks(no markers) ok = true, want false")
	}
	if _, ok := parsePATHMarks(pathMark + pathMark); ok {
		t.Error("parsePATHMarks(empty PATH) ok = true, want false")
	}
	if _, ok := parsePATHMarks(pathMark + "/bin\n/hack" + pathMark); ok {
		t.Error("parsePATHMarks(newline PATH) ok = true, want false")
	}
}

func TestShellPATHArgs(t *testing.T) {
	zsh := shellPATHArgs("/bin/zsh")
	if len(zsh) != 2 || zsh[0] != "-ilc" {
		t.Errorf("shellPATHArgs(/bin/zsh) = %v, want [-ilc printf...]", zsh)
	}
	fish := shellPATHArgs("/opt/homebrew/bin/fish")
	if len(fish) != 4 || fish[0] != "-l" || fish[1] != "-i" || fish[2] != "-c" {
		t.Errorf("shellPATHArgs(fish) = %v, want [-l -i -c printf...]", fish)
	}
	if !strings.Contains(fish[3], "string join") {
		t.Errorf("shellPATHArgs(fish) cmd = %q, want fish string-join PATH", fish[3])
	}
	login := shellPATHArgs("-zsh")
	if login[0] != "-ilc" {
		t.Errorf("shellPATHArgs(-zsh) = %v, want posix -ilc", login)
	}
}

func TestUserLoginShellUsesSHELL(t *testing.T) {
	t.Setenv("SHELL", "/opt/homebrew/bin/fish")
	got := userLoginShell()
	if got != "/opt/homebrew/bin/fish" {
		t.Errorf("userLoginShell() = %q, want %q", got, "/opt/homebrew/bin/fish")
	}
}

func TestShellFromPasswdFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "passwd")
	raw := "# comment\nnobody:*:-2:-2:Unprivileged User:/var/empty:/usr/bin/false\njosh:*:501:20:Josh:/Users/josh:/bin/zsh\n"
	if err := os.WriteFile(path, []byte(raw), 0o644); err != nil {
		t.Fatalf("write passwd: %v", err)
	}
	got := shellFromPasswdFile(path, 501)
	if got != "/bin/zsh" {
		t.Errorf("shellFromPasswdFile(501) = %q, want %q", got, "/bin/zsh")
	}
	if got := shellFromPasswdFile(path, 999); got != "" {
		t.Errorf("shellFromPasswdFile(999) = %q, want empty", got)
	}
}

func TestReadShellPATHExtractsFromJunk(t *testing.T) {
	shell := writeFakeShell(t, `#!/bin/sh
echo "nvm: loading..."
cmd=""
while [ $# -gt 0 ]; do
  case "$1" in
    -ilc|-c) shift; cmd=$1 ;;
    -l|-i) ;;
    *) if [ -z "$cmd" ]; then cmd=$1; fi ;;
  esac
  shift
done
PATH=/captured/bin:/usr/bin
export PATH
eval "$cmd"
`)
	got := readShellPATH(shell, 30*time.Second)
	want := "/captured/bin:/usr/bin"
	if got != want {
		t.Errorf("readShellPATH(fake) = %q, want %q", got, want)
	}
}

func TestReadShellPATHTimeout(t *testing.T) {
	shell := writeFakeShell(t, "#!/bin/sh\nsleep 5\n")
	start := time.Now()
	got := readShellPATH(shell, 200*time.Millisecond)
	if got != "" {
		t.Errorf("readShellPATH(hang) = %q, want empty", got)
	}
	if d := time.Since(start); d > 3*time.Second {
		t.Errorf("readShellPATH(hang) took %v, want ~timeout", d)
	}
}

func TestReadShellPATHMissingBinary(t *testing.T) {
	got := readShellPATH(filepath.Join(t.TempDir(), "nope"), time.Second)
	if got != "" {
		t.Errorf("readShellPATH(missing) = %q, want empty", got)
	}
}

func writeFakeShell(t *testing.T, body string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), "shell")
	if err := os.WriteFile(p, []byte(body), 0o755); err != nil {
		t.Fatalf("write fake shell: %v", err)
	}
	return p
}
