package osfonts

import (
	"encoding/binary"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"
)

func fromFontDirs() []string {
	var names []string
	for _, dir := range fontDirs() {
		_ = filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
			if err != nil || d.IsDir() {
				return nil
			}
			ext := strings.ToLower(filepath.Ext(path))
			if ext != ".ttf" && ext != ".otf" {
				return nil
			}
			family, mono, err := inspectFont(path)
			if err != nil || family == "" {
				return nil
			}
			if mono || looksMonoName(family) {
				names = append(names, family)
			}
			return nil
		})
	}
	return names
}

func fontDirs() []string {
	var dirs []string
	if home, err := os.UserHomeDir(); err == nil {
		dirs = append(dirs, filepath.Join(home, "Library", "Fonts"))
		dirs = append(dirs, filepath.Join(home, ".local", "share", "fonts"))
		dirs = append(dirs, filepath.Join(home, ".fonts"))
	}
	dirs = append(dirs,
		"/Library/Fonts",
		"/System/Library/Fonts",
		"/System/Library/Fonts/Supplemental",
		"/usr/share/fonts",
		"/usr/local/share/fonts",
		`C:\Windows\Fonts`,
	)
	var out []string
	for _, d := range dirs {
		if st, err := os.Stat(d); err == nil && st.IsDir() {
			out = append(out, d)
		}
	}
	return out
}

func looksMonoName(family string) bool {
	return monoNameRe.MatchString(family)
}

// inspectFont reads the OpenType/TrueType 'name' family and fixed-pitch flag
// from 'post' (isFixedPitch) or OS/2 PANOSE proportion.
func inspectFont(path string) (family string, mono bool, err error) {
	f, err := os.Open(path)
	if err != nil {
		return "", false, err
	}
	defer f.Close()

	hdr := make([]byte, 12)
	if _, err := io.ReadFull(f, hdr); err != nil {
		return "", false, err
	}
	tag := string(hdr[0:4])
	if tag != "\x00\x01\x00\x00" && tag != "OTTO" && tag != "true" && tag != "typ1" {
		return "", false, errors.New("not a single sfnt font")
	}
	numTables := binary.BigEndian.Uint16(hdr[4:6])
	type table struct {
		tag    string
		offset uint32
		length uint32
	}
	tables := make([]table, 0, numTables)
	for i := 0; i < int(numTables); i++ {
		rec := make([]byte, 16)
		if _, err := io.ReadFull(f, rec); err != nil {
			return "", false, err
		}
		tables = append(tables, table{
			tag:    string(rec[0:4]),
			offset: binary.BigEndian.Uint32(rec[8:12]),
			length: binary.BigEndian.Uint32(rec[12:16]),
		})
	}

	readTable := func(tag string) ([]byte, error) {
		for _, t := range tables {
			if t.tag != tag {
				continue
			}
			if t.length > 4<<20 { // 4 MiB sanity
				return nil, errors.New("table too large")
			}
			buf := make([]byte, t.length)
			if _, err := f.ReadAt(buf, int64(t.offset)); err != nil {
				return nil, err
			}
			return buf, nil
		}
		return nil, errors.New("missing table")
	}

	if nameData, err := readTable("name"); err == nil {
		family = familyFromNameTable(nameData)
	}
	// 'post' layout: version(4) + italicAngle(4) + underlinePos(2) +
	// underlineThickness(2) + isFixedPitch(4) → isFixedPitch at offset 12.
	// (An earlier bug read offset 32 and marked nearly every font as mono.)
	if post, err := readTable("post"); err == nil && len(post) >= 16 {
		if binary.BigEndian.Uint32(post[12:16]) != 0 {
			mono = true
		}
	}
	if !mono {
		if os2, err := readTable("OS/2"); err == nil && len(os2) >= 42 {
			// PANOSE proportion byte (index 3 of 10) == 9 ⇒ monospaced.
			// PANOSE starts at offset 32 in the OS/2 table.
			if os2[32+3] == 9 {
				mono = true
			}
		}
	}
	return family, mono, nil
}

func familyFromNameTable(data []byte) string {
	if len(data) < 6 {
		return ""
	}
	count := int(binary.BigEndian.Uint16(data[2:4]))
	stringOffset := int(binary.BigEndian.Uint16(data[4:6]))
	var (
		best     string
		bestRank int // higher is better
	)
	rank := func(platform, encoding, lang, nameID int) int {
		// Prefer Windows English family (name ID 1), then Mac Roman.
		r := 0
		if nameID == 1 {
			r += 100
		} else if nameID == 16 { // Typographic Family
			r += 90
		} else {
			return -1
		}
		if platform == 3 && encoding == 1 && (lang == 0x409 || lang == 0) {
			r += 20
		} else if platform == 1 && encoding == 0 {
			r += 10
		} else if platform == 3 {
			r += 5
		}
		return r
	}
	for i := 0; i < count; i++ {
		off := 6 + i*12
		if off+12 > len(data) {
			break
		}
		platform := int(binary.BigEndian.Uint16(data[off : off+2]))
		encoding := int(binary.BigEndian.Uint16(data[off+2 : off+4]))
		lang := int(binary.BigEndian.Uint16(data[off+4 : off+6]))
		nameID := int(binary.BigEndian.Uint16(data[off+6 : off+8]))
		length := int(binary.BigEndian.Uint16(data[off+8 : off+10]))
		offset := int(binary.BigEndian.Uint16(data[off+10 : off+12]))
		r := rank(platform, encoding, lang, nameID)
		if r < 0 || r < bestRank {
			continue
		}
		start := stringOffset + offset
		end := start + length
		if start < 0 || end > len(data) {
			continue
		}
		raw := data[start:end]
		var s string
		if platform == 3 || (platform == 0) {
			s = decodeUTF16BE(raw)
		} else {
			s = string(raw)
		}
		s = strings.TrimSpace(s)
		if s == "" {
			continue
		}
		best = s
		bestRank = r
	}
	return best
}

func decodeUTF16BE(b []byte) string {
	if len(b)%2 != 0 {
		return ""
	}
	runes := make([]rune, 0, len(b)/2)
	for i := 0; i+1 < len(b); i += 2 {
		runes = append(runes, rune(binary.BigEndian.Uint16(b[i:i+2])))
	}
	return string(runes)
}
