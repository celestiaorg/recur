package term

import (
	"strings"
	"unicode/utf8"
)

const (
	// DefaultTailLines is how much recent output @terminal wrap and
	// read_terminal include when the caller does not ask for a size.
	DefaultTailLines = 200
	// MaxTailLines caps a read_terminal tail so one dump cannot blow the turn.
	MaxTailLines  = 2000
	maxPlainBytes = 32 << 10
)

// StripVT drops CSI/OSC/DCS and other C0 controls except newline, tab, and CR.
// Use this when CLI stderr (Codex tracing, colored CLIs) is shown outside a PTY.
func StripVT(s string) string {
	if s == "" {
		return ""
	}
	return stripVT([]byte(s))
}

// TailPlain converts raw VT bytes to readable text and keeps the last maxLines
// lines. Cursor-addressed full-screen programs (vim, htop) will not reconstruct
// as a screen — this is a stream tail, which is what shell/test output needs.
func TailPlain(raw []byte, maxLines int) string {
	if maxLines <= 0 {
		maxLines = DefaultTailLines
	}
	if maxLines > MaxTailLines {
		maxLines = MaxTailLines
	}
	s := stripVT(raw)
	s = strings.ReplaceAll(s, "\r\n", "\n")
	// CR overwrites the current line (progress bars, spinners). Keep only the
	// last segment so redraws do not explode into hundreds of tail lines.
	s = collapseCarriageReturns(s)
	lines := strings.Split(s, "\n")
	// Drop a trailing empty from a final newline so "last N" is real lines.
	if n := len(lines); n > 0 && lines[n-1] == "" {
		lines = lines[:n-1]
	}
	if len(lines) > maxLines {
		lines = lines[len(lines)-maxLines:]
	}
	out := strings.Join(lines, "\n")
	if len(out) > maxPlainBytes {
		out = out[len(out)-maxPlainBytes:]
		if i := strings.IndexByte(out, '\n'); i >= 0 {
			out = out[i+1:]
		}
		out = "…\n" + out
	}
	return out
}

func collapseCarriageReturns(s string) string {
	if !strings.Contains(s, "\r") {
		return s
	}
	lines := strings.Split(s, "\n")
	for i, line := range lines {
		if j := strings.LastIndexByte(line, '\r'); j >= 0 {
			lines[i] = line[j+1:]
		}
	}
	return strings.Join(lines, "\n")
}

// stripVT drops CSI/OSC/DCS and other C0 controls except newline and tab so
// a replay ring can be attached as chat context. Incomplete sequences at a
// wrapped-ring start are discarded with the rest of the escape.
func stripVT(p []byte) string {
	var b strings.Builder
	b.Grow(len(p))
	i := 0
	for i < len(p) {
		c := p[i]
		if c == 0x1b {
			i = skipESC(p, i)
			continue
		}
		if c == 0x9b { // 8-bit CSI
			i = skipUntilFinal(p, i+1)
			continue
		}
		if c == 0x9d { // 8-bit OSC
			i = skipOSC(p, i+1)
			continue
		}
		if c == '\n' || c == '\t' || c == '\r' {
			b.WriteByte(c)
			i++
			continue
		}
		if c < 0x20 || c == 0x7f {
			i++
			continue
		}
		if c < 0x80 {
			b.WriteByte(c)
			i++
			continue
		}
		r, size := utf8.DecodeRune(p[i:])
		if r == utf8.RuneError && size == 1 {
			i++
			continue
		}
		b.WriteRune(r)
		i += size
	}
	return b.String()
}

func skipESC(p []byte, i int) int {
	if i+1 >= len(p) {
		return len(p)
	}
	switch p[i+1] {
	case '[': // CSI
		return skipUntilFinal(p, i+2)
	case ']': // OSC
		return skipOSC(p, i+2)
	case 'P', 'X', '^', '_': // DCS / SOS / PM / APC terminated by ST
		return skipST(p, i+2)
	case '(':
		if i+2 < len(p) {
			return i + 3
		}
		return len(p)
	default:
		return i + 2
	}
}

func skipUntilFinal(p []byte, i int) int {
	for i < len(p) {
		if p[i] >= 0x40 && p[i] <= 0x7e {
			return i + 1
		}
		i++
	}
	return i
}

func skipOSC(p []byte, i int) int {
	for i < len(p) {
		if p[i] == 0x07 {
			return i + 1
		}
		if p[i] == 0x1b && i+1 < len(p) && p[i+1] == '\\' {
			return i + 2
		}
		i++
	}
	return i
}

func skipST(p []byte, i int) int {
	for i < len(p) {
		if p[i] == 0x1b && i+1 < len(p) && p[i+1] == '\\' {
			return i + 2
		}
		i++
	}
	return i
}
