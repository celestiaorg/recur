//go:build darwin

package engine

import (
	"strings"

	"github.com/fsnotify/fsevents"
)

// startFSWatch arms one recursive FSEvents stream on root (already
// symlink-resolved). The returned channel is never closed; stop ends delivery.
func startFSWatch(root string) (<-chan []fsEvent, func(), error) {
	es := &fsevents.EventStream{
		Paths:   []string{root},
		Latency: fsEventLatency,
		Flags:   fsevents.FileEvents | fsevents.WatchRoot | fsevents.NoDefer,
	}
	if dev, err := fsevents.DeviceForPath(root); err == nil {
		es.Device = dev
	}
	if err := es.Start(); err != nil {
		return nil, nil, err
	}
	out := make(chan []fsEvent)
	done := make(chan struct{})
	go func() {
		for {
			select {
			case <-done:
				return
			case batch, ok := <-es.Events:
				if !ok {
					return
				}
				evs := make([]fsEvent, 0, len(batch))
				for _, ev := range batch {
					evs = append(evs, fsEvent{
						Path:   fsPath(ev.Path),
						Rescan: ev.Flags&fsevents.MustScanSubDirs != 0,
					})
				}
				select {
				case out <- evs:
				case <-done:
					return
				}
			}
		}
	}()
	return out, func() { close(done); es.Stop() }, nil
}

// fsPath normalizes an FSEvents path (delivered without the leading slash).
func fsPath(p string) string {
	if !strings.HasPrefix(p, "/") {
		return "/" + p
	}
	return p
}
