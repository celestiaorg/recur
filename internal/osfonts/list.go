// Package osfonts enumerates locally installed fonts for Recur settings.
// The webview cannot reliably see user fonts via canvas probing, so the
// desktop shell lists them natively and the frontend merges the result.
package osfonts

import (
	"sort"
	"strings"
)

// MonospaceFamilies returns distinct CSS font-family names that are monospace.
//
// Sources (union):
//  1. fontconfig `fc-list :spacing=mono` (when available), minus known junk tags
//  2. Scan of common font dirs — keep a face only if OpenType says fixed-pitch
//     (post.isFixedPitch / OS/2 PANOSE) or the family name is a known mono
func MonospaceFamilies() []string {
	seen := map[string]string{} // lower → display
	add := func(name string) {
		name = cleanFamily(name)
		if name == "" || strings.HasPrefix(name, ".") || isJunkMonoName(name) {
			return
		}
		key := strings.ToLower(name)
		if _, ok := seen[key]; ok {
			return
		}
		seen[key] = name
	}

	// fontconfig's spacing=mono is helpful but noisy on macOS (emoji, bitmaps,
	// etc.). Only keep faces whose names clearly look like coding monos; the
	// directory scan below is the OpenType-authoritative path.
	for _, n := range fromFontconfig() {
		if isJunkMonoName(n) || !looksMonoName(n) {
			continue
		}
		add(n)
	}
	for _, n := range fromFontDirs() {
		add(n)
	}

	out := make([]string, 0, len(seen))
	for _, n := range seen {
		out = append(out, n)
	}
	sort.Strings(out)
	return out
}

func cleanFamily(name string) string {
	name = strings.TrimSpace(name)
	// fc-list can return comma-separated localized names; keep the first.
	if i := strings.IndexByte(name, ','); i >= 0 {
		name = strings.TrimSpace(name[:i])
	}
	return name
}
