package engine

import "testing"

func TestResolveCursorContextWindow(t *testing.T) {
	cases := []struct {
		model string
		flags map[string]string
		want  int64
	}{
		{"composer-2.5", nil, 200_000},
		{"grok-4.7", nil, 500_000},
		{"grok-4.6", nil, 500_000},
		{"grok-4.5", nil, 256_000},
		{"claude-sonnet-5", nil, 200_000},
		{"claude-sonnet-5", map[string]string{"context": "1m"}, 1_000_000},
		{"claude-opus-5", nil, 300_000},
		{"claude-opus-5", map[string]string{"maxMode": "true"}, 1_000_000},
		{"gpt-5.6-sol", nil, 272_000},
		{"gpt-5.6-sol", map[string]string{"context": "1m"}, 1_000_000},
		{"gpt-5.6-terra", nil, 272_000},
		{"gpt-5.6-terra", map[string]string{"maxMode": "true"}, 1_000_000},
		{"gpt-5.6-luna", map[string]string{"maxMode": "true"}, 1_000_000},
		{"gpt-5.4-mini", map[string]string{"context": "1m"}, 1_000_000},
		{"gemini-3.1-pro", nil, 1_000_000},
		{"gemini-3.6-flash", nil, 1_000_000},
		{"composer-2.5", map[string]string{"maxMode": "true"}, 200_000},
		{"grok-4.7", map[string]string{"maxMode": "true"}, 500_000},
		{"grok-4.6", map[string]string{"maxMode": "true"}, 500_000},
		{"grok-4.5", map[string]string{"maxMode": "true"}, 256_000},
		{"brand-new-model-9", nil, 200_000},
		{"brand-new-model-9", map[string]string{"context": "1m"}, 1_000_000},
	}
	for _, tc := range cases {
		if got := ResolveCursorContextWindow(tc.model, tc.flags); got != tc.want {
			t.Errorf("ResolveCursorContextWindow(%q, %v) = %d, want %d", tc.model, tc.flags, got, tc.want)
		}
	}
}

func TestParseContextFlag(t *testing.T) {
	cases := map[string]int64{
		"":        0,
		"272k":    272_000,
		"1m":      1_000_000,
		"1.0M":    1_000_000,
		"200k":    200_000,
		"300K":    300_000,
		"200000":  200_000,
		"garbage": 0,
	}
	for in, want := range cases {
		if got := parseContextFlag(in); got != want {
			t.Errorf("parseContextFlag(%q) = %d, want %d", in, got, want)
		}
	}
}
