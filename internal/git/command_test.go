package git

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"
)

func TestStatusMapSucceedsWithExistingIndexLock(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	root := initDirtyRepo(t, 0) // no extra untracked files needed
	lock := filepath.Join(root, ".git", "index.lock")
	if err := os.WriteFile(lock, nil, 0o644); err != nil {
		t.Fatal(err)
	}

	st := StatusMap(context.Background(), root)
	if st["a.go"] != "m" {
		t.Fatalf("StatusMap with leftover index.lock = %#v, want a.go=m (git status took an optional lock)", st)
	}
}

func TestStatusMapDoesNotTakeIndexLock(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	// Enough untracked files that git status's optional index refresh is
	// observable — Recur used to SIGKILL that refresh and leave index.lock.
	root := initDirtyRepo(t, 1500)
	lock := filepath.Join(root, ".git", "index.lock")

	var hits atomic.Int32
	stop := make(chan struct{})
	go func() {
		for {
			select {
			case <-stop:
				return
			default:
				if _, err := os.Stat(lock); err == nil {
					hits.Add(1)
				}
				time.Sleep(200 * time.Microsecond)
			}
		}
	}()

	st := StatusMap(context.Background(), root)
	time.Sleep(20 * time.Millisecond)
	close(stop)
	if st["a.go"] != "m" {
		t.Fatalf("StatusMap = %#v, want a.go=m", st)
	}
	if n := hits.Load(); n > 0 {
		t.Fatalf("git status took index.lock %d time(s); Recur status polls must set GIT_OPTIONAL_LOCKS=0", n)
	}
	if _, err := os.Stat(lock); !os.IsNotExist(err) {
		t.Fatalf("git status left %s: %v", lock, err)
	}
}

func TestCommandDisablesOptionalLocks(t *testing.T) {
	t.Setenv("GIT_OPTIONAL_LOCKS", "1")
	cmd := Command(context.Background(), "status")
	var found string
	for _, e := range cmd.Env {
		if strings.HasPrefix(e, "GIT_OPTIONAL_LOCKS=") {
			if found != "" {
				t.Fatalf("GIT_OPTIONAL_LOCKS set twice: %q then %q", found, e)
			}
			found = e
		}
	}
	if found != "GIT_OPTIONAL_LOCKS=0" {
		t.Fatalf("GIT_OPTIONAL_LOCKS = %q, want GIT_OPTIONAL_LOCKS=0", found)
	}
}

func TestCommandSIGTERMsOnCancel(t *testing.T) {
	cmd := Command(context.Background(), "status")
	if cmd.Cancel == nil {
		t.Fatal("Cancel is nil; CommandContext would SIGKILL git and leave lock files")
	}
	if cmd.WaitDelay != gitCancelGrace {
		t.Fatalf("WaitDelay = %s, want %s", cmd.WaitDelay, gitCancelGrace)
	}
	if cmd.SysProcAttr == nil || !cmd.SysProcAttr.Setpgid {
		t.Fatal("Setpgid is false; Cancel would SIGTERM only the git leader, not hook grandchildren")
	}
}

func TestWaitErrSwallowsPipeHoldAfterCleanExit(t *testing.T) {
	cmd := exec.CommandContext(context.Background(), "sh", "-c", "sleep 1 & echo done")
	cmd.WaitDelay = 50 * time.Millisecond
	cmd.Cancel = func() error { return nil }
	out, err := cmd.Output()
	if err == nil {
		t.Skip("grandchild did not hold stdout in this environment")
	}
	if !errors.Is(err, exec.ErrWaitDelay) {
		t.Fatalf("Output error = %v, want ErrWaitDelay", err)
	}
	if err = waitErr(cmd, err); err != nil {
		t.Fatalf("waitErr(ErrWaitDelay after exit 0) = %v, want nil", err)
	}
	if got := strings.TrimSpace(string(out)); got != "done" {
		t.Fatalf("Output = %q, want done", got)
	}
}

func TestWaitErrKeepsNonZeroExit(t *testing.T) {
	cmd := exec.Command("sh", "-c", "echo fail >&2; exit 1")
	_, err := cmd.Output()
	if err == nil {
		t.Fatal("Output() = nil, want exit 1")
	}
	if got := waitErr(cmd, err); got == nil {
		t.Fatal("waitErr swallowed a non-zero exit")
	}
}

func TestWaitErrKeepsWaitDelayWithoutSuccessState(t *testing.T) {
	err := waitErr(&exec.Cmd{}, exec.ErrWaitDelay)
	if !errors.Is(err, exec.ErrWaitDelay) {
		t.Fatalf("waitErr(nil ProcessState, ErrWaitDelay) = %v, want ErrWaitDelay", err)
	}
}

func TestCmdMethodsAbsorbPipeHold(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	t.Run("Output", func(t *testing.T) {
		raw := pipeHoldGit(t)
		_, rawErr := raw.Cmd.Output()
		if rawErr == nil {
			t.Skip("grandchild did not hold stdout in this environment")
		}
		if !errors.Is(rawErr, exec.ErrWaitDelay) {
			t.Fatalf("Cmd.Cmd.Output() error = %v, want ErrWaitDelay", rawErr)
		}
		cmd := pipeHoldGit(t)
		out, err := cmd.Output()
		if err != nil {
			t.Fatalf("Cmd.Output() = %v, want nil", err)
		}
		if got := strings.TrimSpace(string(out)); got != "done" {
			t.Fatalf("Cmd.Output() = %q, want done", got)
		}
	})
	t.Run("Wait", func(t *testing.T) {
		raw := pipeHoldGit(t)
		raw.Stdout = new(bytes.Buffer)
		if err := raw.Start(); err != nil {
			t.Fatalf("Start: %v", err)
		}
		rawErr := raw.Cmd.Wait()
		if rawErr == nil {
			t.Skip("grandchild did not hold stdout in this environment")
		}
		if !errors.Is(rawErr, exec.ErrWaitDelay) {
			t.Fatalf("Cmd.Cmd.Wait() error = %v, want ErrWaitDelay", rawErr)
		}
		cmd := pipeHoldGit(t)
		var stdout bytes.Buffer
		cmd.Stdout = &stdout
		if err := cmd.Start(); err != nil {
			t.Fatalf("Start: %v", err)
		}
		if err := cmd.Wait(); err != nil {
			t.Fatalf("Cmd.Wait() = %v, want nil", err)
		}
		if got := strings.TrimSpace(stdout.String()); got != "done" {
			t.Fatalf("Cmd.Wait() stdout = %q, want done", got)
		}
	})
	t.Run("CombinedOutput", func(t *testing.T) {
		raw := pipeHoldGit(t)
		_, rawErr := raw.Cmd.CombinedOutput()
		if rawErr == nil {
			t.Skip("grandchild did not hold stdout in this environment")
		}
		if !errors.Is(rawErr, exec.ErrWaitDelay) {
			t.Fatalf("Cmd.Cmd.CombinedOutput() error = %v, want ErrWaitDelay", rawErr)
		}
		cmd := pipeHoldGit(t)
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("Cmd.CombinedOutput() = %v, want nil", err)
		}
		if got := strings.TrimSpace(string(out)); got != "done" {
			t.Fatalf("Cmd.CombinedOutput() = %q, want done", got)
		}
	})
	t.Run("Run", func(t *testing.T) {
		raw := pipeHoldGit(t)
		raw.Stdout = new(bytes.Buffer)
		rawErr := raw.Cmd.Run()
		if rawErr == nil {
			t.Skip("grandchild did not hold stdout in this environment")
		}
		if !errors.Is(rawErr, exec.ErrWaitDelay) {
			t.Fatalf("Cmd.Cmd.Run() error = %v, want ErrWaitDelay", rawErr)
		}
		cmd := pipeHoldGit(t)
		var stdout bytes.Buffer
		cmd.Stdout = &stdout
		if err := cmd.Run(); err != nil {
			t.Fatalf("Cmd.Run() = %v, want nil", err)
		}
		if got := strings.TrimSpace(stdout.String()); got != "done" {
			t.Fatalf("Cmd.Run() stdout = %q, want done", got)
		}
	})
}

func pipeHoldGit(t *testing.T) *Cmd {
	t.Helper()
	root := initDirtyRepo(t, 0)
	cmd := Command(context.Background(), "-C", root, "-c", "alias.hold=!sleep 1 & echo done", "hold")
	cmd.WaitDelay = 50 * time.Millisecond
	return cmd
}

func TestCommandStartsInOwnProcessGroup(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	root := initDirtyRepo(t, 0)
	cmd := Command(context.Background(), "-C", root, "status", "--porcelain")
	if err := cmd.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer func() { _ = cmd.Wait() }()
	pgid, err := syscall.Getpgid(cmd.Process.Pid)
	if err != nil {
		t.Fatalf("Getpgid: %v", err)
	}
	if pgid != cmd.Process.Pid {
		t.Fatalf("Getpgid(%d) = %d, want pid (Setpgid so Cancel can SIGTERM the group)", cmd.Process.Pid, pgid)
	}
}

func TestCheckRefFormatViaCommand(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	if err := checkRefFormat("recur/ok"); err != nil {
		t.Fatalf("checkRefFormat(recur/ok) = %v, want nil", err)
	}
	if err := checkRefFormat("has space"); err == nil {
		t.Fatal("checkRefFormat(has space) = nil, want error")
	}
}

func initDirtyRepo(t *testing.T, untracked int) string {
	t.Helper()
	root := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root}, args...)...)
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "core.untrackedCache", "true")
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\nfunc F() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < untracked; i++ {
		name := filepath.Join(root, "u"+strconv.Itoa(i)+".txt")
		if err := os.WriteFile(name, []byte("x\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return root
}
