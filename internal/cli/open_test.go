package cli

import (
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"recur/internal/openipc"
)

func TestLookDesktopBinaryRejectsBundledCLI(t *testing.T) {
	dir := t.TempDir()
	bundled := filepath.Join(dir, "Recur.app", "Contents", "MacOS", "recur-cli")
	if err := os.MkdirAll(filepath.Dir(bundled), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(bundled, []byte("cli"), 0o755); err != nil {
		t.Fatal(err)
	}
	binDir := filepath.Join(dir, "bin")
	if err := os.MkdirAll(binDir, 0o755); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(binDir, "recur")
	if err := os.Symlink(bundled, link); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", binDir)

	_, err := lookDesktopBinary()
	if err == nil {
		t.Fatalf("lookDesktopBinary() error = nil, want reject bundled CLI")
	}
}

func TestIsCLIBinary(t *testing.T) {
	if !isCLIBinary("/Applications/Recur.app/Contents/MacOS/recur-cli") {
		t.Fatal("isCLIBinary(bundled) = false, want true")
	}
	if isCLIBinary("/Applications/Recur.app/Contents/MacOS/recur") {
		t.Fatal("isCLIBinary(GUI) = true, want false")
	}
}

func TestLooksLikeOpenPath(t *testing.T) {
	if !looksLikeOpenPath(".") || !looksLikeOpenPath("..") {
		t.Fatal("looksLikeOpenPath(.|..) = false, want true")
	}
	if !looksLikeOpenPath("./foo") {
		t.Fatal("looksLikeOpenPath(./foo) = false, want true")
	}
	if looksLikeOpenPath("bundlez") {
		t.Fatal("looksLikeOpenPath(bundlez) = true, want false")
	}
	dir := t.TempDir()
	if !looksLikeOpenPath(dir) {
		t.Fatalf("looksLikeOpenPath(%s) = false, want true", dir)
	}
}

func TestRootUnknownCommand(t *testing.T) {
	code := execute([]string{"bundlez"})
	if code == 0 {
		t.Fatal("execute([bundlez]) exit = 0, want nonzero")
	}
}

func TestResolveOpenArg(t *testing.T) {
	dir := t.TempDir()
	file := filepath.Join(dir, "a.txt")
	if err := os.WriteFile(file, []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := resolveOpenArg(file)
	if err != nil {
		t.Fatalf("resolveOpenArg(%s) error = %v", file, err)
	}
	if got != file && !filepath.IsAbs(got) {
		t.Fatalf("resolveOpenArg(%s) = %q, want an absolute path", file, got)
	}
	_, err = resolveOpenArg(filepath.Join(dir, "missing"))
	if err == nil {
		t.Fatal("resolveOpenArg(missing) error = nil, want error")
	}
}

func TestSendOrLaunchUsesSocket(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	sock, err := openipc.SockPath()
	if err != nil {
		t.Fatal(err)
	}
	stop := make(chan struct{})
	defer close(stop)
	got := make(chan openipc.Request, 1)
	go func() {
		_ = openipc.Serve(stop, sock, func(req openipc.Request) openipc.Response {
			got <- req
			return openipc.Response{OK: true, Path: req.Path, WorkspaceID: "ws-1"}
		})
	}()
	// Wait until the socket exists so Call doesn't race listen.
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(sock); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("socket was not created")
		}
		time.Sleep(10 * time.Millisecond)
	}

	dir := t.TempDir()
	if err := sendOrLaunch(dir); err != nil {
		t.Fatalf("sendOrLaunch: %v", err)
	}
	select {
	case req := <-got:
		if req.Path != dir {
			t.Fatalf("sendOrLaunch Path = %q, want %q", req.Path, dir)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("server did not receive the open request")
	}
}

func TestSendOrLaunchRetriesShutdown(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	sock, err := openipc.SockPath()
	if err != nil {
		t.Fatal(err)
	}
	stop := make(chan struct{})
	defer close(stop)
	var n atomic.Int32
	got := make(chan openipc.Request, 1)
	go func() {
		_ = openipc.Serve(stop, sock, func(req openipc.Request) openipc.Response {
			if n.Add(1) < 3 {
				return openipc.Response{OK: false, Error: openipc.ErrShuttingDown}
			}
			got <- req
			return openipc.Response{OK: true, Path: req.Path}
		})
	}()
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(sock); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("socket was not created")
		}
		time.Sleep(10 * time.Millisecond)
	}

	dir := t.TempDir()
	if err := sendOrLaunch(dir); err != nil {
		t.Fatalf("sendOrLaunch: %v", err)
	}
	select {
	case req := <-got:
		if req.Path != dir {
			t.Fatalf("sendOrLaunch Path = %q, want %q", req.Path, dir)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("server did not receive the open request after shutdown retries")
	}
}

func TestSendOrLaunchPermanentError(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	sock, err := openipc.SockPath()
	if err != nil {
		t.Fatal(err)
	}
	stop := make(chan struct{})
	defer close(stop)
	go func() {
		_ = openipc.Serve(stop, sock, func(openipc.Request) openipc.Response {
			return openipc.Response{OK: false, Error: "not a git repo"}
		})
	}()
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(sock); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("socket was not created")
		}
		time.Sleep(10 * time.Millisecond)
	}

	dir := t.TempDir()
	err = sendOrLaunch(dir)
	if err == nil || err.Error() != "not a git repo" {
		t.Fatalf("sendOrLaunch error = %v, want not a git repo", err)
	}
}

func TestRootOpenArg(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	sock, err := openipc.SockPath()
	if err != nil {
		t.Fatal(err)
	}
	stop := make(chan struct{})
	defer close(stop)
	got := make(chan string, 1)
	go func() {
		_ = openipc.Serve(stop, sock, func(req openipc.Request) openipc.Response {
			got <- req.Path
			return openipc.Response{OK: true}
		})
	}()
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(sock); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("socket was not created")
		}
		time.Sleep(10 * time.Millisecond)
	}

	dir := t.TempDir()
	want, err := resolveOpenArg(dir)
	if err != nil {
		t.Fatal(err)
	}
	if code := execute([]string{dir}); code != 0 {
		t.Fatalf("execute([%s]) exit = %d, want 0", dir, code)
	}
	select {
	case p := <-got:
		if p != want {
			t.Fatalf("root open Path = %q, want %q", p, want)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("root command did not reach the socket")
	}
}
