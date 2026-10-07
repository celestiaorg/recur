package term

import (
	"bytes"
	"fmt"
)

// Default charcoal theme colours, as 16-bit xterm rgb: reports. Matching the
// frontend's --bg / --text / --amber keeps termenv/pterm dark-mode detection
// consistent with what the user sees even though replies never round-trip the
// webview. (App themes are all dark today; a light theme would need a live push.)
const (
	defaultFGReport     = "rgb:e6e6/e4e4/e1e1" // #e6e4e1
	defaultBGReport     = "rgb:1414/1414/1414" // #141414
	defaultCursorReport = "rgb:e0e0/7a7a/5f5f" // #e07a5f
)

// queryFilter answers terminal status probes on the Go side of the PTY and strips
// them from the stream forwarded to xterm.js.
//
// Tools built on muesli/termenv (wails3 → pterm, charmbracelet, …) probe OSC
// 10/11/12 and CSI 6n to learn fg/bg colours. A real terminal replies on the
// same fd within microseconds. Our replies used to take a JS↔Go bridge hop, so
// they often landed after termenv had moved on — and then showed up as typed
// garbage (`11;rgb:1414/1414/1414`, `1R`) on the shell prompt. Answering here
// keeps the probe synchronous with the reader.
type queryFilter struct {
	carry  []byte
	fg     string
	bg     string
	cursor string
}

func newQueryFilter() *queryFilter {
	return &queryFilter{
		fg:     defaultFGReport,
		bg:     defaultBGReport,
		cursor: defaultCursorReport,
	}
}

// process consumes PTY output, returning the bytes to forward to the frontend
// and any immediate replies to write back to the PTY master.
func (f *queryFilter) process(in []byte) (forward, replies []byte) {
	if len(f.carry) == 0 && len(in) == 0 {
		return nil, nil
	}
	data := in
	if len(f.carry) > 0 {
		data = append(append([]byte{}, f.carry...), in...)
		f.carry = nil
	}

	for i := 0; i < len(data); {
		if data[i] != 0x1b {
			j := i + 1
			for j < len(data) && data[j] != 0x1b {
				j++
			}
			forward = append(forward, data[i:j]...)
			i = j
			continue
		}

		n, incomplete := matchTerminalQuery(data[i:])
		if incomplete {
			f.carry = append([]byte{}, data[i:]...)
			break
		}
		if n > 0 {
			replies = append(replies, f.replyFor(data[i:i+n])...)
			i += n
			continue
		}
		// ESC that isn't one of our probes — forward it and keep scanning.
		forward = append(forward, data[i])
		i++
	}
	return forward, replies
}

func (f *queryFilter) replyFor(query []byte) []byte {
	// CSI 6n — cursor position report. termenv only uses this as a fence after
	// the OSC probe; the coordinates don't matter as long as the shape is valid.
	if bytes.Equal(query, []byte("\x1b[6n")) {
		return []byte("\x1b[1;1R")
	}
	// OSC 10/11/12 colour queries: ESC ] 1 X ; ? BEL/ST
	if len(query) < 6 || query[0] != 0x1b || query[1] != ']' || query[2] != '1' {
		return nil
	}
	var colour string
	switch query[3] {
	case '0':
		colour = f.fg
	case '1':
		colour = f.bg
	case '2':
		colour = f.cursor
	default:
		return nil
	}
	// BEL termination; termenv accepts BEL or ST.
	return []byte(fmt.Sprintf("\x1b]1%c;%s\a", query[3], colour))
}

// knownQueries are the exact probes we intercept. Anything else (OSC titles,
// other CSI) passes through untouched.
var knownQueries = [][]byte{
	[]byte("\x1b]10;?\x1b\\"),
	[]byte("\x1b]10;?\a"),
	[]byte("\x1b]11;?\x1b\\"),
	[]byte("\x1b]11;?\a"),
	[]byte("\x1b]12;?\x1b\\"),
	[]byte("\x1b]12;?\a"),
	[]byte("\x1b[6n"),
}

// matchTerminalQuery reports whether data begins with a colour/CPR probe.
// incomplete means data is a proper prefix of a known probe and more bytes are needed.
func matchTerminalQuery(data []byte) (n int, incomplete bool) {
	couldBePrefix := false
	for _, q := range knownQueries {
		if len(data) >= len(q) && bytes.Equal(data[:len(q)], q) {
			return len(q), false
		}
		if len(data) < len(q) && bytes.HasPrefix(q, data) {
			couldBePrefix = true
		}
	}
	return 0, couldBePrefix
}
