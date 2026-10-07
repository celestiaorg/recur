package mcp

import (
	"crypto/rand"
	"encoding/hex"
)

// newToken mints an unguessable MCP URL token. It is used as the path segment
// of the agent's MCP URL so a tool call is attributed to the right loop+agent
// (and a leaked token 404s after Unregister / Drop). 16 random bytes → 32 hex chars.
func newToken() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		// crypto/rand failure is fatal-class; fall back to a fixed-length zero token
		// rather than panic in a server goroutine. Practically never happens.
		return "0000000000000000000000000000000000000000000000000000000000000000"[:32]
	}
	return hex.EncodeToString(b[:])
}
