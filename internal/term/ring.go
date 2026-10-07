package term

// ring is a fixed-capacity circular byte buffer holding the tail of a session's
// raw VT output. It backs replay-on-attach: the drawer can be closed (or the user
// can navigate to another workspace) while a shell keeps running, and re-attaching
// repaints the screen instead of showing an empty terminal.
//
// It stores RAW VT bytes, not decoded text, so replay carries cursor moves, colors
// and alt-screen switches — a running vim or htop redraws correctly. The trade-off
// is that a wrapped-around buffer can begin mid-escape-sequence; xterm.js's parser
// resyncs after discarding the partial sequence, which at this capacity costs at
// most a few glyphs off the top of the scrollback.
type ring struct {
	buf  []byte
	w    int  // next write offset
	full bool // w has wrapped at least once
}

func newRing(capacity int) *ring {
	return &ring{buf: make([]byte, capacity)}
}

// write appends p, overwriting the oldest bytes once capacity is reached.
func (r *ring) write(p []byte) {
	n := len(r.buf)
	if n == 0 {
		return
	}
	// A write larger than the whole buffer can only leave its own last n bytes,
	// so skip straight to that tail rather than looping over doomed bytes.
	if len(p) >= n {
		copy(r.buf, p[len(p)-n:])
		r.w = 0
		r.full = true
		return
	}
	c := copy(r.buf[r.w:], p)
	if c < len(p) {
		copy(r.buf, p[c:])
		r.full = true
	}
	r.w += len(p)
	if r.w >= n {
		r.w -= n
		r.full = true
	}
}

// snapshot returns the buffered bytes in write order, oldest first.
func (r *ring) snapshot() []byte {
	if !r.full {
		out := make([]byte, r.w)
		copy(out, r.buf[:r.w])
		return out
	}
	out := make([]byte, len(r.buf))
	n := copy(out, r.buf[r.w:])
	copy(out[n:], r.buf[:r.w])
	return out
}

// reset drops everything buffered. Used when a client clears the terminal so a
// later re-attach doesn't replay the output the user just wiped.
func (r *ring) reset() {
	r.w = 0
	r.full = false
}
