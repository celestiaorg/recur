package term

import (
	"bytes"
	"testing"
)

func TestRingSnapshot(t *testing.T) {
	tests := []struct {
		name   string
		cap    int
		writes []string
		want   string
	}{
		{name: "empty", cap: 8, want: ""},
		{name: "under capacity", cap: 8, writes: []string{"abc"}, want: "abc"},
		{name: "exactly capacity", cap: 4, writes: []string{"abcd"}, want: "abcd"},
		{name: "wraps and drops oldest", cap: 4, writes: []string{"abcd", "ef"}, want: "cdef"},
		{name: "many small writes", cap: 4, writes: []string{"a", "b", "c", "d", "e"}, want: "bcde"},
		{name: "single write larger than capacity", cap: 4, writes: []string{"abcdefgh"}, want: "efgh"},
		{name: "oversized write after wrap", cap: 4, writes: []string{"ab", "cdefghij"}, want: "ghij"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			r := newRing(tt.cap)
			for _, w := range tt.writes {
				r.write([]byte(w))
			}
			if got := string(r.snapshot()); got != tt.want {
				t.Errorf("snapshot = %q, want %q", got, tt.want)
			}
		})
	}
}

func TestRingResetDropsHistory(t *testing.T) {
	r := newRing(8)
	r.write([]byte("abcdefghij")) // force a wrap so full is set
	r.reset()
	if got := r.snapshot(); len(got) != 0 {
		t.Fatalf("snapshot after reset = %q, want empty", got)
	}
	r.write([]byte("xy"))
	if got := string(r.snapshot()); got != "xy" {
		t.Errorf("snapshot after reset+write = %q, want %q", got, "xy")
	}
}

// A zero-capacity ring must not panic — it just remembers nothing.
func TestRingZeroCapacity(t *testing.T) {
	r := newRing(0)
	r.write([]byte("abc"))
	if got := r.snapshot(); len(got) != 0 {
		t.Errorf("snapshot = %q, want empty", got)
	}
}

func TestRingSnapshotIsACopy(t *testing.T) {
	r := newRing(4)
	r.write([]byte("ab"))
	snap := r.snapshot()
	r.write([]byte("cd"))
	if !bytes.Equal(snap, []byte("ab")) {
		t.Errorf("earlier snapshot mutated to %q; snapshot must copy", snap)
	}
}
