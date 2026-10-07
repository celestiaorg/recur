package domain

import (
	"strings"

	"recur/internal/shortid"
)

// IDType is a registered short-id prefix. Add a const here when a new
// persisted entity gets a public id.
type IDType string

const (
	TypeFinding IDType = "find"
)

// ShortIDLength is the suffix length for NewID (fits shortid.ValidPattern).
const ShortIDLength = 8

// NewID generates a public short id for t, e.g. "find_k7mq2xf9".
func NewID(t IDType) (string, error) {
	return shortid.New(string(t), ShortIDLength)
}

// NewUniqueID generates a public short id that taken reports as free.
func NewUniqueID(t IDType, taken func(string) bool) (string, error) {
	return shortid.NewUnique(string(t), ShortIDLength, shortid.DefaultAttempts, taken)
}

// ValidID reports whether id is a well-formed public id for t.
func ValidID(id string, t IDType) bool {
	prefix, _, ok := strings.Cut(id, "_")
	return ok && prefix == string(t) && shortid.IsValid(id)
}
