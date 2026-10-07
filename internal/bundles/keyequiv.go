package bundles

import (
	"fmt"
	"sort"
	"strings"
)

// Canonical modifier order for chord strings (Apple display order).
var modOrder = []string{"ctrl", "opt", "shift", "cmd", "fn"}

var knownMods = map[string]string{
	"ctrl": "ctrl", "control": "ctrl",
	"opt": "opt", "option": "opt", "alt": "opt",
	"shift": "shift",
	"cmd":   "cmd", "command": "cmd", "meta": "cmd",
	"fn": "fn", "function": "fn",
}

// Aliases normalize key tokens to a single canonical name.
var keyAliases = map[string]string{
	"esc": "escape", "return": "enter", "ret": "enter",
	"bs": "backspace", "del": "delete",
	"pgup": "pageup", "pgdn": "pagedown", "pagedown": "pagedown", "pageup": "pageup",
	"arrowup": "up", "arrowdown": "down", "arrowleft": "left", "arrowright": "right",
	"spacebar": "space", " ": "space",
	"plus": "+", "minus": "-", "period": ".", "comma": ",",
	"slash": "/", "backslash": "\\", "backtick": "`", "grave": "`",
	"bracketleft": "[", "bracketright": "]",
	"semicolon": ";", "quote": "'", "equal": "=",
}

var knownKeys = map[string]bool{
	"a": true, "b": true, "c": true, "d": true, "e": true, "f": true,
	"g": true, "h": true, "i": true, "j": true, "k": true, "l": true,
	"m": true, "n": true, "o": true, "p": true, "q": true, "r": true,
	"s": true, "t": true, "u": true, "v": true, "w": true, "x": true,
	"y": true, "z": true,
	"0": true, "1": true, "2": true, "3": true, "4": true,
	"5": true, "6": true, "7": true, "8": true, "9": true,
	"f1": true, "f2": true, "f3": true, "f4": true, "f5": true, "f6": true,
	"f7": true, "f8": true, "f9": true, "f10": true, "f11": true, "f12": true,
	"enter": true, "escape": true, "tab": true, "space": true,
	"up": true, "down": true, "left": true, "right": true,
	"home": true, "end": true, "pageup": true, "pagedown": true,
	"delete": true, "backspace": true,
	"+": true, "-": true, "=": true, ".": true, ",": true,
	"/": true, "\\": true, "`": true, "[": true, "]": true,
	";": true, "'": true,
	"<": true, ">": true,
}

// ReservedAppChords are macOS app / workbench bindings bundles may not claim.
// Keys must be canonical (ParseKeyEquivalent output). Labels match the shortcuts cheatsheet.
var ReservedAppChords = map[string]string{
	"shift+cmd+/":         "Keyboard shortcuts",
	"cmd+k":               "Go to",
	"shift+cmd+p":         "Run command",
	"cmd+n":               "New workspace",
	"cmd+b":               "Toggle sidebar",
	"cmd+,":               "Settings",
	"escape":              "Back",
	"cmd+p":               "Quick Open",
	"cmd+o":               "Open in",
	"shift+cmd+o":         "Go to symbol",
	"cmd+1":               "Focus agent panel",
	"cmd+2":               "Focus editor",
	"cmd+3":               "Focus terminal",
	"cmd+l":               "Add file / selection to chat",
	"opt+cmd+left":        "Previous tab",
	"opt+cmd+right":       "Next tab",
	"opt+cmd+up":          "Previous workspace",
	"opt+cmd+down":        "Next workspace",
	"opt+cmd+b":           "Toggle left panel",
	"opt+shift+cmd+b":     "Toggle right panel",
	"ctrl+`":              "Toggle terminal",
	"ctrl+shift+`":        "Maximize terminal",
	"cmd+t":               "New tab",
	"cmd+w":               "Close tab",
	"shift+cmd+t":         "Reopen closed editor tab",
	"shift+cmd+backspace": "Stop agent",
	"cmd+.":               "Cycle agent mode",
	"cmd+\\":              "Split editor",
	"cmd+r":               "Run…",
	"shift+cmd+r":         "Stop run",
	"f5":                  "Reload from disk",
	"shift+cmd+v":         "Cycle editor view",
	"ctrl+shift+v":        "Cycle editor view",
	"opt+cmd+[":           "Previous editor theme",
	"opt+cmd+]":           "Next editor theme",
	"cmd+s":               "Save",
	"cmd+f":               "Find",
	"cmd+g":               "Find next",
	"shift+cmd+g":         "Find previous",
	"opt+cmd+g":           "Go to line",
	"cmd+d":               "Select next occurrence",
	"ctrl+shift+up":       "Add cursor above",
	"ctrl+shift+down":     "Add cursor below",
	"cmd+/":               "Toggle comment",
	"f12":                 "Go to definition",
	"cmd+down":            "Open file (Explorer)",
	"opt+cmd+n":           "New file (Explorer)",
	"shift+cmd+n":         "New folder (Explorer)",
	"enter":               "Rename (Explorer)",
	"cmd+backspace":       "Delete (Explorer)",
	"cmd+x":               "Cut",
	"cmd+c":               "Copy",
	"cmd+v":               "Paste",
	"opt+cmd+c":           "Copy path (Explorer)",
	"opt+shift+cmd+c":     "Copy relative path (Explorer)",
	"opt+cmd+r":           "Reveal in Finder",
	"opt+cmd+w":           "Whole word (find)",
	"opt+cmd+z":           "Revert diff hunk",
}

// ParseKeyEquivalent normalizes an authoring chord like "cmd+shift+u" or "Opt+Cmd+F".
// Returns the canonical form (modifiers in Apple order, lowercased key).
func ParseKeyEquivalent(s string) (string, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return "", fmt.Errorf("empty keyEquivalent")
	}
	parts := strings.Split(s, "+")
	mods := map[string]bool{}
	var key string
	for i, p := range parts {
		tok := strings.ToLower(strings.TrimSpace(p))
		if tok == "" {
			return "", fmt.Errorf("empty token in %q", s)
		}
		if m, ok := knownMods[tok]; ok {
			mods[m] = true
			continue
		}
		if i != len(parts)-1 {
			return "", fmt.Errorf("%q: unknown modifier %q", s, p)
		}
		if alias, ok := keyAliases[tok]; ok {
			tok = alias
		}
		if !knownKeys[tok] {
			return "", fmt.Errorf("%q: unknown key %q", s, p)
		}
		key = tok
	}
	if key == "" {
		return "", fmt.Errorf("%q: missing key (modifier alone is not a chord)", s)
	}
	return formatChord(mods, key), nil
}

func formatChord(mods map[string]bool, key string) string {
	var out []string
	for _, m := range modOrder {
		if mods[m] {
			out = append(out, m)
		}
	}
	out = append(out, key)
	return strings.Join(out, "+")
}

// IsReservedAppChord reports whether a canonical chord is owned by the app.
func IsReservedAppChord(canonical string) bool {
	_, ok := ReservedAppChords[canonical]
	return ok
}

// FormatKeyEquivalentDisplay turns a canonical chord into macOS glyph form (⌘⇧U).
func FormatKeyEquivalentDisplay(canonical string) string {
	if canonical == "" {
		return ""
	}
	parts := strings.Split(canonical, "+")
	var b strings.Builder
	for _, p := range parts {
		switch p {
		case "cmd":
			b.WriteString("⌘")
		case "opt":
			b.WriteString("⌥")
		case "shift":
			b.WriteString("⇧")
		case "ctrl":
			b.WriteString("⌃")
		case "fn":
			b.WriteString("fn")
		case "escape":
			b.WriteString("esc")
		case "enter":
			b.WriteString("↵")
		case "backspace":
			b.WriteString("⌫")
		case "delete":
			b.WriteString("⌦")
		case "up":
			b.WriteString("↑")
		case "down":
			b.WriteString("↓")
		case "left":
			b.WriteString("←")
		case "right":
			b.WriteString("→")
		case "tab":
			b.WriteString("⇥")
		case "space":
			b.WriteString("␣")
		default:
			if len(p) == 1 {
				b.WriteString(strings.ToUpper(p))
			} else if strings.HasPrefix(p, "f") && len(p) <= 3 {
				b.WriteString(strings.ToUpper(p))
			} else {
				b.WriteString(p)
			}
		}
	}
	return b.String()
}

// SortedReservedChords returns reserved chords for stable test/docs output.
func SortedReservedChords() []string {
	out := make([]string, 0, len(ReservedAppChords))
	for c := range ReservedAppChords {
		out = append(out, c)
	}
	sort.Strings(out)
	return out
}
