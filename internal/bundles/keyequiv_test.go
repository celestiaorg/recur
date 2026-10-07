package bundles_test

import (
	"testing"

	"recur/internal/bundles"
)

func TestParseKeyEquivalent(t *testing.T) {
	cases := []struct {
		in   string
		want string
	}{
		{"cmd+shift+u", "shift+cmd+u"},
		{"Shift+Cmd+U", "shift+cmd+u"},
		{"opt+cmd+f", "opt+cmd+f"},
		{"alt+meta+f", "opt+cmd+f"},
		{"ctrl+shift+`", "ctrl+shift+`"},
		{"f5", "f5"},
		{"escape", "escape"},
		{"esc", "escape"},
		{"ctrl+<", "ctrl+<"},
		{"ctrl+shift+,", "ctrl+shift+,"},
		{"cmd+opt+shift+c", "opt+shift+cmd+c"},
	}
	for _, tc := range cases {
		got, err := bundles.ParseKeyEquivalent(tc.in)
		if err != nil {
			t.Fatalf("%q: %v", tc.in, err)
		}
		if got != tc.want {
			t.Fatalf("%q: got %q want %q", tc.in, got, tc.want)
		}
	}
}

func TestParseKeyEquivalentErrors(t *testing.T) {
	for _, in := range []string{"", "mod+u", "cmd+", "cmd", "cmd+foo", "++u"} {
		if _, err := bundles.ParseKeyEquivalent(in); err == nil {
			t.Fatalf("%q: expected error", in)
		}
	}
}

func TestReservedAppChordsCanonical(t *testing.T) {
	for chord := range bundles.ReservedAppChords {
		got, err := bundles.ParseKeyEquivalent(chord)
		if err != nil {
			t.Fatalf("reserved %q: parse: %v", chord, err)
		}
		if got != chord {
			t.Fatalf("reserved %q is not canonical (parse → %q)", chord, got)
		}
	}
}

func TestFormatKeyEquivalentDisplay(t *testing.T) {
	if got := bundles.FormatKeyEquivalentDisplay("shift+cmd+u"); got != "⇧⌘U" {
		t.Fatalf("got %q", got)
	}
	if got := bundles.FormatKeyEquivalentDisplay("opt+cmd+f"); got != "⌥⌘F" {
		t.Fatalf("got %q", got)
	}
	if got := bundles.FormatKeyEquivalentDisplay("opt+cmd+up"); got != "⌥⌘↑" {
		t.Fatalf("got %q", got)
	}
	if got := bundles.FormatKeyEquivalentDisplay("ctrl+opt+up"); got != "⌃⌥↑" {
		t.Fatalf("got %q", got)
	}
	if got := bundles.FormatKeyEquivalentDisplay("ctrl+<"); got != "⌃<" {
		t.Fatalf("got %q", got)
	}
}
