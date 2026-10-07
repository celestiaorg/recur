package responses

import (
	"strconv"
	"strings"

	"recur/internal/domain"
)

// ApplyFlags writes selected flag values into dst at each dimension's Wire path.
// Unknown or empty wires are skipped. Nested objects are created as needed.
func ApplyFlags(dst map[string]any, dims []domain.CompatibleFlagDim, flags map[string]string) {
	if dst == nil {
		return
	}
	for _, d := range dims {
		if d.Wire == "" {
			continue
		}
		val := strings.TrimSpace(flags[d.ID])
		if val == "" {
			continue
		}
		setWire(dst, d.Wire, coerceWireValue(d.Wire, val))
	}
	ensureReasoningSummary(dst)
}

func coerceWireValue(wire, val string) any {
	switch wire {
	case "temperature":
		f, err := strconv.ParseFloat(val, 64)
		if err != nil {
			return val
		}
		return f
	case "max_output_tokens":
		n, err := strconv.Atoi(val)
		if err != nil {
			return val
		}
		return n
	default:
		return val
	}
}

func setWire(dst map[string]any, path string, val any) {
	parts := strings.Split(path, ".")
	cur := dst
	for i, p := range parts {
		if i == len(parts)-1 {
			cur[p] = val
			return
		}
		next, _ := cur[p].(map[string]any)
		if next == nil {
			next = map[string]any{}
			cur[p] = next
		}
		cur = next
	}
}

// ensureReasoningSummary sets reasoning.summary=auto when reasoning is present
// but summary is unset, so streamed reasoning can map to thinking events.
func ensureReasoningSummary(dst map[string]any) {
	raw, ok := dst["reasoning"]
	if !ok {
		return
	}
	m, ok := raw.(map[string]any)
	if !ok {
		return
	}
	if _, ok := m["summary"]; !ok {
		m["summary"] = "auto"
	}
}
