package engine

import (
	"strconv"
	"strings"

	"recur/internal/cursor"
)

// ResolveCursorContextWindow returns the context limit in tokens for a Cursor
// turn. Precedence:
//  1. Explicit dock "context" flag when parseable ("1m", "200k", …) — the
//     picker options come from ListModels, so this value is authoritative
//  2. Max Mode / max flag → model’s Max window when known
//  3. Hardcoded per-model Base from the Cursor catalog table
//  4. 200k fallback
func ResolveCursorContextWindow(model string, flags map[string]string) int64 {
	if n := parseContextFlag(flags["context"]); n > 0 {
		return n
	}
	lim := cursor.LimitsFor(model)
	if maxModeEnabled(flags) && lim.Max > 0 {
		return lim.Max
	}
	if lim.Base > 0 {
		return lim.Base
	}
	return 200_000
}

func maxModeEnabled(flags map[string]string) bool {
	switch strings.ToLower(strings.TrimSpace(flags["maxMode"])) {
	case "true", "on", "1", "max":
		return true
	}
	return strings.EqualFold(strings.TrimSpace(flags["max"]), "true")
}

// parseContextFlag accepts "1m", "200k", "300000", etc.
func parseContextFlag(s string) int64 {
	s = strings.TrimSpace(strings.ToLower(s))
	if s == "" {
		return 0
	}
	mult := int64(1)
	switch {
	case strings.HasSuffix(s, "m"):
		mult = 1_000_000
		s = strings.TrimSuffix(s, "m")
	case strings.HasSuffix(s, "k"):
		mult = 1_000
		s = strings.TrimSuffix(s, "k")
	}
	s = strings.TrimSpace(s)
	if s == "" {
		return 0
	}
	f, err := strconv.ParseFloat(s, 64)
	if err != nil || f <= 0 {
		return 0
	}
	return int64(f * float64(mult))
}
