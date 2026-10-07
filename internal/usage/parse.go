package usage

import (
	"encoding/base64"
	"encoding/json"
	"strconv"
	"strings"
	"time"
)

func asFloat(v any) float64 {
	switch t := v.(type) {
	case float64:
		return t
	case float32:
		return float64(t)
	case int:
		return float64(t)
	case int64:
		return float64(t)
	case json.Number:
		n, _ := t.Float64()
		return n
	case string:
		n, _ := strconv.ParseFloat(strings.TrimSpace(t), 64)
		return n
	}
	return 0
}

func asInt64(v any) int64 {
	switch t := v.(type) {
	case int64:
		return t
	case int:
		return int64(t)
	case float64:
		return int64(t)
	case json.Number:
		n, _ := t.Int64()
		return n
	case string:
		s := strings.TrimSpace(t)
		if s == "" {
			return 0
		}
		n, err := strconv.ParseInt(s, 10, 64)
		if err != nil {
			f, ferr := strconv.ParseFloat(s, 64)
			if ferr != nil {
				return 0
			}
			return int64(f)
		}
		return n
	}
	return 0
}

func asString(v any) string {
	switch t := v.(type) {
	case string:
		return strings.TrimSpace(t)
	}
	return ""
}

func mapOf(v any) map[string]any {
	m, _ := v.(map[string]any)
	return m
}

func sliceOf(v any) []any {
	s, _ := v.([]any)
	return s
}

// parseResetsAt accepts unix seconds, unix milliseconds, or RFC3339.
func parseResetsAt(v any) int64 {
	switch t := v.(type) {
	case string:
		s := strings.TrimSpace(t)
		if s == "" {
			return 0
		}
		if tm, err := time.Parse(time.RFC3339, s); err == nil {
			return tm.UTC().UnixMilli()
		}
		return parseResetsAt(asFloat(s))
	default:
		n := asFloat(t)
		if n <= 0 {
			return 0
		}
		if n > 1e12 {
			return int64(n)
		}
		return int64(n) * 1000
	}
}

func decodeMap(raw []byte) (map[string]any, error) {
	var m map[string]any
	if err := json.Unmarshal(raw, &m); err != nil {
		return nil, err
	}
	return m, nil
}

func emailFromCursorJSON(m map[string]any) string {
	if m == nil {
		return ""
	}
	for _, k := range []string{"email", "userEmail", "user_email", "cachedEmail", "cached_email", "emailAddress", "email_address"} {
		if s := asString(m[k]); looksLikeEmail(s) {
			return s
		}
	}
	for _, v := range m {
		if e := emailFromCursorJSON(mapOf(v)); e != "" {
			return e
		}
	}
	return ""
}

func jwtPayload(tok string) map[string]any {
	parts := strings.Split(tok, ".")
	if len(parts) < 2 {
		return nil
	}
	m, err := decodeJWTPayload(parts[1])
	if err != nil {
		return nil
	}
	return m
}

func jwtClaim(tok, key string) string {
	m := jwtPayload(tok)
	if m == nil {
		return ""
	}
	return strings.TrimSpace(asString(m[key]))
}

func emailFromJWT(tok string) string {
	m := jwtPayload(tok)
	if m == nil {
		return ""
	}
	for _, k := range []string{"email", "email_address", "emailAddress", "preferred_username", "upn", "sub"} {
		if s := asString(m[k]); looksLikeEmail(s) {
			return s
		}
	}
	return ""
}

func looksLikeEmail(s string) bool {
	s = strings.TrimSpace(s)
	at := strings.IndexByte(s, '@')
	return at > 0 && at < len(s)-1 && !strings.ContainsAny(s, " \t")
}

// cursorRefreshLeeway is how soon before expiry Recur rotates Connect
// credentials (session JWT and minted user API key). A week covers a long
// weekend if Recur is closed over the last days of a 60/90-day token.
const cursorRefreshLeeway = 7 * 24 * time.Hour

func jwtNeedsRefresh(access string) bool {
	parts := strings.Split(access, ".")
	if len(parts) < 2 {
		return false
	}
	payload, err := decodeJWTPayload(parts[1])
	if err != nil {
		return false
	}
	exp := asFloat(payload["exp"])
	if exp <= 0 {
		return false
	}
	return time.Now().Add(cursorRefreshLeeway).Unix() >= int64(exp)
}

func decodeJWTPayload(seg string) (map[string]any, error) {
	raw, err := base64.RawURLEncoding.DecodeString(strings.TrimRight(seg, "="))
	if err != nil {
		raw, err = base64.URLEncoding.DecodeString(seg)
		if err != nil {
			return nil, err
		}
	}
	return decodeMap(raw)
}

func usedPctFrom(m map[string]any) (float64, bool) {
	if m == nil {
		return 0, false
	}
	for _, k := range []string{"used_percentage", "used_percent", "utilization", "usedPercent", "percent"} {
		if v, ok := m[k]; ok && v != nil {
			return clampPct(asFloat(v)), true
		}
	}
	return 0, false
}
