package cursor

import "testing"

func TestLimitsForDocumentedModels(t *testing.T) {
	cases := []struct {
		id       string
		wantBase int64
		wantMax  int64
	}{
		{"composer-2.5", 200_000, 0},
		{"grok-4.7", 500_000, 0},
		{"grok-4.6", 500_000, 0},
		{"grok-4.5", 256_000, 0},
		{"claude-sonnet-5", 200_000, 1_000_000},
		{"claude-opus-5", 300_000, 1_000_000},
		{"claude-opus-5-fast", 300_000, 1_000_000},
		{"claude-fable-5", 200_000, 1_000_000},
		{"claude-fable-5-1", 200_000, 1_000_000},
		{"gemini-3.1-pro", 1_000_000, 0},
		{"gemini-3-1-pro", 1_000_000, 0},
		{"gemini-3.6-flash", 1_000_000, 0},
		{"gpt-5.6-sol", 272_000, 1_000_000},
		{"gpt-5.6-sol-fast", 272_000, 1_000_000},
		{"gpt-5.6-terra", 272_000, 1_000_000},
		{"gpt-5.6-luna", 272_000, 1_000_000},
		{"gpt-5.5", 272_000, 1_000_000},
		{"gpt-5.4", 272_000, 1_000_000},
		{"gpt-5.4-mini", 272_000, 0},
		{"gpt-5.2", 272_000, 0},
		// Older Opus must not steal Opus 5's 300k default.
		{"claude-opus-4-8", 200_000, 1_000_000},
	}
	for _, tc := range cases {
		got := LimitsFor(tc.id)
		if got.Base != tc.wantBase || got.Max != tc.wantMax {
			t.Errorf("LimitsFor(%q) = %+v, want Base=%d Max=%d", tc.id, got, tc.wantBase, tc.wantMax)
		}
	}
}

func TestNormalizeModelID(t *testing.T) {
	if got := normalizeModelID("GPT-5.6-Sol"); got != "gpt-5-6-sol" {
		t.Errorf("normalize = %q", got)
	}
}

func TestVendor(t *testing.T) {
	cases := []struct{ id, want string }{
		{"composer-2.5", "Cursor"},
		{"auto-smart", "Cursor"},
		{"grok-4.6", "Cursor"},
		{"claude-opus-5", "Anthropic"},
		{"gpt-5.6-sol", "OpenAI"},
		{"o3", "OpenAI"},
		{"gemini-3.1-pro", "Google"},
		{"kimi-k2", "Moonshot"},
		{"glm-4.6", "Zhipu"},
		{"mystery-9", "Other"},
	}
	for _, tc := range cases {
		if got := Vendor(tc.id); got != tc.want {
			t.Errorf("Vendor(%q) = %q, want %q", tc.id, got, tc.want)
		}
	}
}
