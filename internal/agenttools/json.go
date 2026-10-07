package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"

	"recur/internal/domain"
)

func requireHost(c Call) (Host, error) {
	if c.Host == nil {
		return nil, fmt.Errorf("no host reporter")
	}
	return c.Host, nil
}

func schemaSummaryOnly(field string) json.RawMessage {
	return json.RawMessage(fmt.Sprintf(`{
  "type": "object",
  "properties": {"%s": {"type": "string", "description": "A short summary"}},
  "required": ["%s"]
}`, field, field))
}

func xmlEscapeCatalog(s string) string {
	s = strings.ReplaceAll(s, "&", "&amp;")
	s = strings.ReplaceAll(s, "<", "&lt;")
	s = strings.ReplaceAll(s, ">", "&gt;")
	return s
}

func parseSeverity(s string) (domain.Severity, bool) {
	switch domain.Severity(strings.ToLower(strings.TrimSpace(s))) {
	case domain.SevInfo:
		return domain.SevInfo, true
	case domain.SevLow:
		return domain.SevLow, true
	case domain.SevMedium:
		return domain.SevMedium, true
	case domain.SevHigh:
		return domain.SevHigh, true
	case domain.SevCritical:
		return domain.SevCritical, true
	default:
		return "", false
	}
}

func strictUnmarshal(args json.RawMessage, dst any) error {
	if len(args) == 0 {
		return nil
	}
	dec := json.NewDecoder(strings.NewReader(string(args)))
	dec.DisallowUnknownFields()
	if err := dec.Decode(dst); err != nil {
		return fmt.Errorf("invalid arguments: %w", err)
	}
	return nil
}
