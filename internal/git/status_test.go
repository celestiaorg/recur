package git

import (
	"strings"
	"testing"
)

func TestParseStatusPorcelain(t *testing.T) {
	in := strings.Join([]string{
		` M src/main.go`,
		`A  new.txt`,
		` D gone.md`,
		`?? scratch.go`,
		`R  old.go -> renamed.go`,
		`!! ignored.bin`,
		`MM both.go`,
		`AD staged-del.go`,
	}, "\n")
	got := ParseStatusPorcelain(in)
	want := map[string]string{
		"src/main.go":   "m",
		"new.txt":       "a",
		"gone.md":       "d",
		"scratch.go":    "u",
		"old.go":        "d",
		"renamed.go":    "a",
		"both.go":       "m",
		"staged-del.go": "d", // D beats A
	}
	if len(got) != len(want) {
		t.Fatalf("len=%d want %d: %#v", len(got), len(want), got)
	}
	for k, v := range want {
		if got[k] != v {
			t.Errorf("%s: got %q want %q", k, got[k], v)
		}
	}
}

func TestParseStatusPorcelainQuoted(t *testing.T) {
	in := `?? "weird name.txt"`
	got := ParseStatusPorcelain(in)
	if got[`weird name.txt`] != "u" {
		t.Fatalf("got %#v", got)
	}
}

func TestParseStatusPorcelainEmpty(t *testing.T) {
	if ParseStatusPorcelain("") != nil {
		t.Fatal("expected nil")
	}
}
