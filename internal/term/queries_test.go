package term

import (
	"bytes"
	"testing"
)

func TestQueryFilterAnswersOSC11AndCPR(t *testing.T) {
	f := newQueryFilter()
	// termenv sends OSC 11 then CSI 6n back-to-back.
	in := []byte("\x1b]11;?\x1b\\\x1b[6n")
	forward, replies := f.process(in)
	if len(forward) != 0 {
		t.Fatalf("forward = %q, want empty (probes stripped)", forward)
	}
	if !bytes.Contains(replies, []byte("\x1b]11;"+defaultBGReport+"\a")) {
		t.Fatalf("replies missing OSC 11 bg report: %q", replies)
	}
	if !bytes.HasSuffix(replies, []byte("\x1b[1;1R")) {
		t.Fatalf("replies missing CPR fence: %q", replies)
	}
}

func TestQueryFilterAnswersBELTerminatedQuery(t *testing.T) {
	f := newQueryFilter()
	forward, replies := f.process([]byte("\x1b]10;?\a"))
	if len(forward) != 0 {
		t.Fatalf("forward = %q, want empty", forward)
	}
	want := []byte("\x1b]10;" + defaultFGReport + "\a")
	if !bytes.Equal(replies, want) {
		t.Fatalf("replies = %q, want %q", replies, want)
	}
}

func TestQueryFilterPassesThroughOSCTitles(t *testing.T) {
	f := newQueryFilter()
	title := []byte("\x1b]2;josh@Mac:~/recur\a")
	forward, replies := f.process(title)
	if len(replies) != 0 {
		t.Fatalf("unexpected replies for title OSC: %q", replies)
	}
	if !bytes.Equal(forward, title) {
		t.Fatalf("forward = %q, want title untouched", forward)
	}
}

func TestQueryFilterCarriesSplitProbes(t *testing.T) {
	f := newQueryFilter()
	probe := []byte("\x1b]11;?\x1b\\")
	// Split mid-sequence.
	fwd1, rep1 := f.process(probe[:3]) // ESC ] 1
	if len(fwd1) != 0 || len(rep1) != 0 {
		t.Fatalf("incomplete prefix should carry, got forward=%q replies=%q", fwd1, rep1)
	}
	fwd2, rep2 := f.process(probe[3:])
	if len(fwd2) != 0 {
		t.Fatalf("forward after completing probe = %q, want empty", fwd2)
	}
	if !bytes.Contains(rep2, []byte("\x1b]11;"+defaultBGReport+"\a")) {
		t.Fatalf("replies after split missing bg report: %q", rep2)
	}
}

func TestQueryFilterPreservesSurroundingOutput(t *testing.T) {
	f := newQueryFilter()
	in := append([]byte("hello"), []byte("\x1b]11;?\a")...)
	in = append(in, []byte("world")...)
	forward, replies := f.process(in)
	if string(forward) != "helloworld" {
		t.Fatalf("forward = %q, want helloworld", forward)
	}
	if !bytes.Contains(replies, []byte(defaultBGReport)) {
		t.Fatalf("replies missing bg: %q", replies)
	}
}

func TestQueryFilterCursorColour(t *testing.T) {
	f := newQueryFilter()
	_, replies := f.process([]byte("\x1b]12;?\a"))
	want := []byte("\x1b]12;" + defaultCursorReport + "\a")
	if !bytes.Equal(replies, want) {
		t.Fatalf("replies = %q, want %q", replies, want)
	}
}
