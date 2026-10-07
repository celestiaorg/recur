package term

import (
	"bytes"
	"net/url"
	"os"
	"strings"
)

const (
	osc7MaxCarry = 8 << 10
	osc7MaxURI   = 4 << 10
)

// osc7Parser watches PTY output for OSC 7 (cwd) reports.
//
// Many shells (fish, oh-my-zsh on Terminal.app, shell-integration snippets)
// emit `ESC ] 7 ; file://host/path BEL` (or ST) after `cd`. We observe those
// sequences without stripping them — xterm.js ignores OSC 7 — so the session's
// recorded cwd tracks the shell instead of staying at the directory it started in.
type osc7Parser struct {
	carry []byte
}

// feed scans in for complete OSC 7 sequences and returns the last valid
// existing directory they named. ok is false when none completed this call.
func (p *osc7Parser) feed(in []byte) (cwd string, ok bool) {
	if len(p.carry) == 0 && len(in) == 0 {
		return "", false
	}
	data := in
	if len(p.carry) > 0 {
		data = append(append([]byte{}, p.carry...), in...)
		p.carry = nil
	}

	for i := 0; i < len(data); {
		esc := bytes.IndexByte(data[i:], 0x1b)
		if esc < 0 {
			return cwd, ok
		}
		i += esc
		n, incomplete := matchOSC7(data[i:])
		if incomplete {
			rest := data[i:]
			if len(rest) > osc7MaxCarry {
				rest = rest[len(rest)-osc7MaxCarry:]
			}
			p.carry = append([]byte{}, rest...)
			return cwd, ok
		}
		if n == 0 {
			i++
			continue
		}
		if path, pathOK := osc7Path(data[i : i+n]); pathOK {
			cwd, ok = path, true
		}
		i += n
	}
	return cwd, ok
}

// matchOSC7 reports the length of an OSC 7 sequence at the start of data.
// incomplete means data is a prefix of a possible OSC 7 and more bytes are needed.
func matchOSC7(data []byte) (n int, incomplete bool) {
	// ESC ] 7 ;
	prefix := []byte{0x1b, ']', '7', ';'}
	if len(data) < len(prefix) {
		if bytes.HasPrefix(prefix, data) {
			return 0, true
		}
		return 0, false
	}
	if !bytes.Equal(data[:len(prefix)], prefix) {
		return 0, false
	}
	for i := len(prefix); i < len(data); i++ {
		if i-len(prefix) > osc7MaxURI {
			return 0, false
		}
		if data[i] == 0x07 { // BEL
			return i + 1, false
		}
		if data[i] == 0x1b {
			if i+1 >= len(data) {
				return 0, true
			}
			if data[i+1] == '\\' { // ST
				return i + 2, false
			}
			return 0, false
		}
	}
	return 0, true
}

func osc7Path(seq []byte) (string, bool) {
	if len(seq) < 5 || seq[0] != 0x1b || seq[1] != ']' || seq[2] != '7' || seq[3] != ';' {
		return "", false
	}
	end := len(seq)
	switch {
	case seq[end-1] == 0x07:
		end--
	case end >= 2 && seq[end-2] == 0x1b && seq[end-1] == '\\':
		end -= 2
	default:
		return "", false
	}
	uri := string(seq[4:end])
	u, err := url.Parse(uri)
	if err != nil || !strings.EqualFold(u.Scheme, "file") {
		return "", false
	}
	if !osc7HostLocal(u.Host) {
		return "", false
	}
	path := u.Path
	if path == "" {
		return "", false
	}
	if decoded, err := url.PathUnescape(path); err == nil {
		path = decoded
	}
	fi, err := os.Stat(path)
	if err != nil || !fi.IsDir() {
		return "", false
	}
	return path, true
}

func osc7HostLocal(host string) bool {
	host = strings.TrimSuffix(strings.TrimPrefix(host, "["), "]")
	switch strings.ToLower(host) {
	case "", "localhost", "127.0.0.1", "::1":
		return true
	}
	name, err := os.Hostname()
	if err != nil {
		return false
	}
	return strings.EqualFold(host, name)
}
