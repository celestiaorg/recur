package shortid

import (
	"errors"
	"strings"
	"testing"
)

func TestNew(t *testing.T) {
	id, err := New("find", 8)
	if err != nil {
		t.Fatalf("New(find, 8) error = %v, want nil", err)
	}
	if !IsValid(id) {
		t.Errorf("New(find, 8) = %q, want a valid short id", id)
	}
	if got, want := id[:5], "find_"; got != want {
		t.Errorf("New(find, 8) prefix = %q, want %q", got, want)
	}
	if got := len(id); got != len("find_")+8 {
		t.Errorf("New(find, 8) len = %d, want %d", got, len("find_")+8)
	}
	if c := suffixStart(id); c < 'a' || c > 'z' {
		t.Errorf("New(find, 8) = %q, want suffix to start with a letter", id)
	}
}

func TestNewRejectsBadInput(t *testing.T) {
	tests := []struct {
		name   string
		prefix string
		length int
	}{
		{name: "empty prefix", prefix: "", length: 8},
		{name: "uppercase prefix", prefix: "Find", length: 8},
		{name: "digit prefix", prefix: "f1", length: 8},
		{name: "zero length", prefix: "find", length: 0},
		{name: "negative length", prefix: "find", length: -1},
		{name: "below min", prefix: "find", length: MinLength - 1},
		{name: "above max", prefix: "find", length: MaxLength + 1},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := New(tt.prefix, tt.length)
			if err == nil {
				t.Fatalf("New(%q, %d) = %q, want error", tt.prefix, tt.length, got)
			}
			if !strings.HasPrefix(err.Error(), "shortid:") {
				t.Errorf("New(%q, %d) error = %v, want shortid: prefix", tt.prefix, tt.length, err)
			}
		})
	}
}

func TestIsValid(t *testing.T) {
	tests := []struct {
		id   string
		want bool
	}{
		{id: "find_k7mq2xf9", want: true},
		{id: "find_abcdef", want: true},
		{id: "usr_a34567", want: true},
		{id: "find_abcdefghjk", want: true}, // 10 suffix
		{id: "usr_234567", want: false},     // suffix must start with a letter
		{id: "find_2abcdef", want: false},
		{id: "find_abcde", want: false}, // 5 suffix
		{id: "find_abcdefghjkm", want: false},
		{id: "find_abcdei", want: false}, // i not in alphabet
		{id: "find_abcde0", want: false},
		{id: "FIND_k7mq2xf9", want: false},
		{id: "find-k7mq2xf9", want: false},
		{id: "k7mq2xf9", want: false},
		{id: "", want: false},
	}
	for _, tt := range tests {
		if got := IsValid(tt.id); got != tt.want {
			t.Errorf("IsValid(%q) = %v, want %v", tt.id, got, tt.want)
		}
	}
}

func TestNewUniqueRetriesWhenTaken(t *testing.T) {
	seen := map[string]bool{}
	rejectFirst := true
	id, err := NewUnique("find", 8, 8, func(s string) bool {
		if rejectFirst {
			rejectFirst = false
			seen[s] = true
			return true
		}
		return seen[s]
	})
	if err != nil {
		t.Fatalf("NewUnique() error = %v, want nil", err)
	}
	if seen[id] {
		t.Errorf("NewUnique() = %q, want an id that was not already taken", id)
	}
	if !IsValid(id) {
		t.Errorf("NewUnique() = %q, want a valid short id", id)
	}
}

func TestNewUniqueExhausted(t *testing.T) {
	_, err := NewUnique("find", 8, 2, func(string) bool { return true })
	if !errors.Is(err, ErrCollision) {
		t.Errorf("NewUnique() error = %v, want ErrCollision", err)
	}
}

func TestNewDistinct(t *testing.T) {
	seen := map[string]bool{}
	for i := 0; i < 64; i++ {
		id, err := New("find", 8)
		if err != nil {
			t.Fatalf("New() error = %v, want nil", err)
		}
		if seen[id] {
			t.Fatalf("New() = %q, already seen", id)
		}
		if c := suffixStart(id); c < 'a' || c > 'z' {
			t.Errorf("New() = %q, want suffix to start with a letter", id)
		}
		seen[id] = true
	}
}

func TestRejectMax(t *testing.T) {
	tests := []struct {
		n, want int
	}{
		{n: 30, want: 240},
		{n: 22, want: 242},
		{n: 32, want: 256},
		{n: 1, want: 256},
		{n: 0, want: 0},
	}
	for _, tt := range tests {
		if got := rejectMax(tt.n); got != tt.want {
			t.Errorf("rejectMax(%d) = %d, want %d", tt.n, got, tt.want)
		}
	}
}

func suffixStart(id string) byte {
	_, suffix, ok := strings.Cut(id, "_")
	if !ok || suffix == "" {
		return 0
	}
	return suffix[0]
}
