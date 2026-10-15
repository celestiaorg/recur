//go:build !darwin

package engine

import (
	"errors"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/fsnotify/fsnotify"
)

// startFSWatch watches root with fsnotify. inotify (and fsnotify's Windows
// backend) is per-directory, so every directory is added, skipping .git
// internals (only .git itself, for HEAD) and node_modules. Events are batched
// like FSEvents: the first after idle is sent at once, a burst coalesces into
// one batch per fsEventLatency. The returned channel is never closed.
func startFSWatch(root string) (<-chan []fsEvent, func(), error) {
	w, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, nil, err
	}
	addWatchTree(w, root, root, nil)
	latency := fsEventLatency // read once, like FSEvents' stream Latency
	out := make(chan []fsEvent)
	done := make(chan struct{})
	go func() {
		defer w.Close()
		var (
			pending []fsEvent
			hold    <-chan time.Time
			sent    time.Time
		)
		for {
			var send chan []fsEvent
			if len(pending) > 0 && hold == nil {
				send = out
			}
			select {
			case <-done:
				return
			case ev, ok := <-w.Events:
				if !ok {
					return
				}
				if ev.Has(fsnotify.Rename) || ev.Has(fsnotify.Remove) {
					unwatchTree(w, ev.Name)
				}
				if ev.Has(fsnotify.Create) {
					if st, err := os.Stat(ev.Name); err == nil && st.IsDir() {
						// Files written before the new watch existed raise no event.
						addWatchTree(w, root, ev.Name, func(p string) {
							pending = append(pending, fsEvent{Path: p})
						})
					}
				}
				pending = append(pending, fsEvent{Path: ev.Name})
				if wait := latency - time.Since(sent); hold == nil && wait > 0 {
					hold = time.After(wait)
				}
			case _, ok := <-w.Errors:
				if !ok {
					return
				}
				// Queue overflow (or a dropped watch): re-list everything.
				pending = append(pending, fsEvent{Path: root, Rescan: true})
			case <-hold:
				hold = nil
			case send <- pending:
				pending = nil
				sent = time.Now()
			}
		}
	}()
	return out, func() { close(done) }, nil
}

// addWatchTree adds dir and its subdirectories under root, passing each file it
// walks past to onFile when onFile is non-nil.
// simplify: stops at the inotify max_user_watches limit and leaves the rest of
// the tree to the fingerprint backstop; raise fs.inotify.max_user_watches or
// move to fanotify if huge non-node_modules trees need live refresh.
func addWatchTree(w *fsnotify.Watcher, root, dir string, onFile func(string)) {
	_ = filepath.WalkDir(dir, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if !d.IsDir() {
			if onFile != nil {
				onFile(p)
			}
			return nil
		}
		gitDir := p == filepath.Join(root, ".git")
		if !gitDir && fsIgnored(root, p) {
			return filepath.SkipDir
		}
		if err := w.Add(p); err != nil {
			if errors.Is(err, syscall.ENOSPC) {
				log.Printf("recur: fs watch on %s hit the inotify watch limit; rest of the tree relies on the fingerprint poll", root)
				return filepath.SkipAll
			}
			return filepath.SkipDir
		}
		if gitDir {
			return filepath.SkipDir
		}
		return nil
	})
}

// unwatchTree drops the watches on dir and everything under it. fsnotify keeps
// a moved directory's watches under their old paths, so without this the
// paired Create's Add is a no-op and the new name's events go missing.
func unwatchTree(w *fsnotify.Watcher, dir string) {
	prefix := dir + string(filepath.Separator)
	for _, p := range w.WatchList() {
		if p == dir || strings.HasPrefix(p, prefix) {
			_ = w.Remove(p)
		}
	}
}
