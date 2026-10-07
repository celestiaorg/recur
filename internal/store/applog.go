package store

import (
	"io"
	"log"
	"os"
	"path/filepath"
	"runtime/debug"
	"sync"
)

// appLogFile is the Recur process log under LogsDir. Rotated to
// recur.log.old when it grows past appLogMaxBytes, including mid-session.
const appLogFile = "recur.log"

// appLogMaxBytes caps the process log before we rotate, matching
// cursor-bridge.log so a stuck debug loop cannot fill the data dir.
const appLogMaxBytes = 2 << 20 // 2 MiB

// AppLogPath returns <root>/logs/recur.log (the file may not exist yet).
func AppLogPath(root string) string {
	return filepath.Join(root, "logs", appLogFile)
}

// TeeAppLog tees the standard library logger to <LogsDir>/recur.log in
// addition to the previous log writer (stderr at boot). It does not call
// slog.SetDefault — that would reroute log.Printf through a TextHandler
// and collapse multi-line stack traces into one escaped msg= attribute.
// Callers that want slog (Wails) should wrap the returned writer
// themselves: slog.New(slog.NewTextHandler(w, nil)).
//
// Fatal crash traces (unguarded panics, signals) go to the same file via
// debug.SetCrashOutput. The file rotates to recur.log.old when it exceeds
// appLogMaxBytes, including mid-session.
//
// Returns the tee writer and a cleanup that restores the previous log
// writer and crash sink and closes the file.
//
// Fail-open: if root is empty or the file cannot be opened, the writer
// is nil, cleanup is a no-op, and existing sinks are left unchanged.
func TeeAppLog(root string) (w io.Writer, cleanup func()) {
	prevLog := log.Writer()
	rot := openAppLog(root)
	if rot == nil {
		return nil, func() {}
	}
	// Ignore errors on the previous writer (often stderr in a Dock-launched
	// .app) so a broken pipe cannot skip the on-disk write.
	w = io.MultiWriter(ignoreWriteErr{prevLog}, rot)
	log.SetOutput(w)
	rot.enableCrashOutput()
	log.Printf("recur: logging to %s", rot.path)
	return w, func() {
		log.SetOutput(prevLog)
		_ = debug.SetCrashOutput(nil, debug.CrashOptions{})
		_ = rot.Close()
	}
}

func openAppLog(root string) *rotatingFile {
	if root == "" {
		return nil
	}
	if _, err := LogsDir(root); err != nil {
		return nil
	}
	path := AppLogPath(root)
	if fi, err := os.Stat(path); err == nil && fi.Size() >= appLogMaxBytes {
		_ = os.Rename(path, path+".old")
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return nil
	}
	var size int64
	if fi, err := f.Stat(); err == nil {
		size = fi.Size()
	}
	return &rotatingFile{f: f, path: path, size: size, max: appLogMaxBytes}
}

// rotatingFile is an append-only log that rotates to path+".old" when a
// write would cross max. Concurrent writes are serialized. Fail-open: a
// rotate error leaves the current file in place (or drops the handle if
// it cannot be reopened).
type rotatingFile struct {
	mu    sync.Mutex
	f     *os.File
	path  string
	size  int64
	max   int64
	crash bool
}

func (r *rotatingFile) enableCrashOutput() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.crash = true
	r.applyCrashOutputLocked()
}

func (r *rotatingFile) applyCrashOutputLocked() {
	if !r.crash || r.f == nil {
		return
	}
	_ = debug.SetCrashOutput(r.f, debug.CrashOptions{})
}

func (r *rotatingFile) Write(p []byte) (int, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.f == nil {
		return len(p), nil
	}
	if r.size > 0 && r.size+int64(len(p)) >= r.max {
		r.rotateLocked()
		if r.f == nil {
			return len(p), nil
		}
	}
	n, err := r.f.Write(p)
	r.size += int64(n)
	return n, err
}

func (r *rotatingFile) rotateLocked() {
	_ = r.f.Close()
	if err := os.Rename(r.path, r.path+".old"); err != nil {
		f, oerr := os.OpenFile(r.path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
		if oerr != nil {
			r.f = nil
			return
		}
		r.f = f
		if fi, serr := f.Stat(); serr == nil {
			r.size = fi.Size()
		}
		r.applyCrashOutputLocked()
		return
	}
	f, err := os.OpenFile(r.path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		r.f = nil
		return
	}
	r.f = f
	r.size = 0
	r.applyCrashOutputLocked()
}

func (r *rotatingFile) Close() error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.f == nil {
		return nil
	}
	err := r.f.Close()
	r.f = nil
	return err
}

// ignoreWriteErr reports a full write even when the underlying writer
// fails, so io.MultiWriter keeps going to later sinks.
type ignoreWriteErr struct{ w io.Writer }

func (w ignoreWriteErr) Write(p []byte) (int, error) {
	_, _ = w.w.Write(p)
	return len(p), nil
}
