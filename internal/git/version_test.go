package git

import (
	"fmt"
	"os/exec"
	"strings"
	"testing"
)

func TestParseVersion(t *testing.T) {
	tests := []struct {
		in   string
		want Version
		ok   bool
	}{
		{in: "git version 2.31.0", want: Version{2, 31, 0}, ok: true},
		{in: "git version 2.50.1 (Apple Git-155)", want: Version{2, 50, 1}, ok: true},
		{in: "git version 2.39.2.windows.1", want: Version{2, 39, 2}, ok: true},
		{in: "git version 2.31", want: Version{2, 31, 0}, ok: true},
		{in: "2.24.3", want: Version{2, 24, 3}, ok: true},
		{in: "git version 2.45.GIT", want: Version{2, 45, 0}, ok: true},
		{in: "git version 2.31.0-rc2", want: Version{2, 31, 0}, ok: true},
		{in: "git version 1", ok: false},
		{in: "", ok: false},
		{in: "not a version", ok: false},
	}
	for _, tt := range tests {
		got, ok := ParseVersion(tt.in)
		if ok != tt.ok {
			t.Errorf("ParseVersion(%q) ok = %v, want %v", tt.in, ok, tt.ok)
			continue
		}
		if !tt.ok {
			continue
		}
		if got != tt.want {
			t.Errorf("ParseVersion(%q) = %+v, want %+v", tt.in, got, tt.want)
		}
	}
}

func TestVersionLess(t *testing.T) {
	if !((Version{2, 30, 2}).Less(MinVersion)) {
		t.Errorf("Version{2, 30, 2}.Less(%s) = false, want true", MinVersion)
	}
	if (Version{2, 31, 0}).Less(MinVersion) {
		t.Errorf("Version{2, 31, 0}.Less(%s) = true, want false", MinVersion)
	}
	if (Version{2, 50, 1}).Less(MinVersion) {
		t.Errorf("Version{2, 50, 1}.Less(%s) = true, want false", MinVersion)
	}
}

func TestMessageIfUnsupported(t *testing.T) {
	if got := MessageIfUnsupported(Version{2, 50, 1}, nil); got != "" {
		t.Errorf("MessageIfUnsupported(2.50.1, nil) = %q, want empty", got)
	}
	got := MessageIfUnsupported(Version{2, 24, 3}, nil)
	if got == "" || !strings.Contains(got, "2.31") || !strings.Contains(got, "2.24.3") {
		t.Errorf("MessageIfUnsupported(2.24.3, nil) = %q, want mention of 2.31 and 2.24.3", got)
	}
	got = MessageIfUnsupported(Version{}, exec.ErrNotFound)
	if got == "" || !strings.Contains(got, "2.31") || !strings.Contains(got, "not found") {
		t.Errorf("MessageIfUnsupported(err) = %q, want missing-git alert", got)
	}
	got = MessageIfUnsupported(Version{}, fmt.Errorf("%w: %q", ErrUnparsed, "bogus"))
	if got == "" || strings.Contains(got, "not found") || !strings.Contains(got, "could not read") {
		t.Errorf("MessageIfUnsupported(ErrUnparsed) = %q, want unparsed alert, not missing-git", got)
	}
}

func TestLogNumstatFlags(t *testing.T) {
	got := logNumstatFlags(Version{2, 31, 0}, nil)
	if len(got) != 1 || got[0] != "--diff-merges=first-parent" {
		t.Errorf("logNumstatFlags(2.31, nil) = %v, want [--diff-merges=first-parent]", got)
	}
	if got := logNumstatFlags(Version{2, 30, 0}, nil); got != nil {
		t.Errorf("logNumstatFlags(2.30, nil) = %v, want nil", got)
	}
	if got := logNumstatFlags(Version{}, exec.ErrNotFound); got != nil {
		t.Errorf("logNumstatFlags(err) = %v, want nil", got)
	}
	v, ok := ParseVersion("git version 2.45.GIT")
	if !ok {
		t.Fatal("ParseVersion(2.45.GIT) ok = false, want true")
	}
	got = logNumstatFlags(v, nil)
	if len(got) != 1 || got[0] != "--diff-merges=first-parent" {
		t.Errorf("logNumstatFlags(2.45.GIT, nil) = %v, want [--diff-merges=first-parent]", got)
	}
}

func TestUnsupportedAlert(t *testing.T) {
	title, body := UnsupportedAlert(Version{2, 50, 1}, nil)
	if title != "" || body != "" {
		t.Errorf("UnsupportedAlert(2.50.1, nil) = %q, %q, want empty", title, body)
	}
	title, body = UnsupportedAlert(Version{2, 24, 0}, nil)
	if title != "Git is too old" || body == "" {
		t.Errorf("UnsupportedAlert(2.24, nil) = %q, %q, want title Git is too old", title, body)
	}
	title, body = UnsupportedAlert(Version{}, exec.ErrNotFound)
	if title != "Git was not found" || !strings.Contains(body, "not found") {
		t.Errorf("UnsupportedAlert(missing) = %q, %q, want title Git was not found", title, body)
	}
	title, body = UnsupportedAlert(Version{}, fmt.Errorf("%w: %q", ErrUnparsed, "nope"))
	if title != "Git version unknown" || strings.Contains(body, "not found") {
		t.Errorf("UnsupportedAlert(unparsed) = %q, %q, want title Git version unknown", title, body)
	}
}

func TestProbe(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	v, err := Probe()
	if err != nil {
		t.Fatalf("Probe() error = %v, want nil", err)
	}
	if v.Major < 2 {
		t.Errorf("Probe() = %s, want major ≥ 2", v)
	}
}
