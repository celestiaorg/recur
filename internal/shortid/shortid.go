// Package shortid generates compact, unambiguous public identifiers.
//
// IDs look like "find_k7mq2xf9": a lowercase prefix, an underscore, and a
// random suffix drawn from a 30-character alphabet that omits i/l/o/0/1 so
// humans (and models) are less likely to misread them.
package shortid

import (
	"crypto/rand"
	"errors"
	"fmt"
	"regexp"
)

const (
	alphabet  = "abcdefghjkmnpqrstvwxyz23456789" // 30 chars, no i/l/o/0/1
	letters   = "abcdefghjkmnpqrstvwxyz"         // suffix always starts with a letter
	MinLength = 6
	MaxLength = 10
)

// DefaultAttempts is the collision-retry budget for NewUnique and insert loops.
const DefaultAttempts = 8

// ErrCollision is returned when NewUnique cannot mint an unused id.
var ErrCollision = errors.New("shortid: collision")

var prefixPattern = regexp.MustCompile(`^[a-z]+$`)

// ValidPattern matches a well-formed public id (prefix + MinLength–MaxLength suffix).
// The first suffix character is always a letter so ids never look like find_2….
var ValidPattern = regexp.MustCompile(fmt.Sprintf(
	`^[a-z]+_[%s][%s]{%d,%d}$`, letters, alphabet, MinLength-1, MaxLength-1,
))

// New generates an ID like "find_k7mq2xf9".
func New(prefix string, length int) (string, error) {
	if !prefixPattern.MatchString(prefix) {
		return "", fmt.Errorf("shortid: prefix %q must be lowercase letters", prefix)
	}
	if length < MinLength || length > MaxLength {
		return "", fmt.Errorf("shortid: length must be %d-%d", MinLength, MaxLength)
	}
	suffix, err := randomString(length)
	if err != nil {
		return "", fmt.Errorf("shortid: %w", err)
	}
	return prefix + "_" + suffix, nil
}

// NewUnique generates IDs until taken reports the candidate is free, or
// attempts is exhausted. A nil taken treats every candidate as free.
// attempts <= 0 uses DefaultAttempts.
func NewUnique(prefix string, length, attempts int, taken func(string) bool) (string, error) {
	if attempts <= 0 {
		attempts = DefaultAttempts
	}
	for range attempts {
		id, err := New(prefix, length)
		if err != nil {
			return "", err
		}
		if taken == nil || !taken(id) {
			return id, nil
		}
	}
	return "", fmt.Errorf("shortid: exhausted %d attempts: %w", attempts, ErrCollision)
}

func randomString(length int) (string, error) {
	out := make([]byte, 0, length)
	letterMax := rejectMax(len(letters))
	alphaMax := rejectMax(len(alphabet))
	buf := make([]byte, length*2) // over-fetch to reduce loop iterations
	for len(out) < length {
		if _, err := rand.Read(buf); err != nil {
			return "", err
		}
		for _, b := range buf {
			alpha, max := alphabet, alphaMax
			if len(out) == 0 {
				alpha, max = letters, letterMax
			}
			if int(b) < max {
				out = append(out, alpha[int(b)%len(alpha)])
				if len(out) == length {
					break
				}
			}
		}
	}
	return string(out), nil
}

// rejectMax is the exclusive upper bound for unbiased rejection sampling over n
// symbols. When n divides 256 every byte maps uniformly, so the bound is 256.
func rejectMax(n int) int {
	if n <= 0 {
		return 0
	}
	rem := 256 % n
	if rem == 0 {
		return 256
	}
	return 256 - rem
}

// IsValid reports whether id matches ValidPattern.
func IsValid(id string) bool {
	return ValidPattern.MatchString(id)
}
