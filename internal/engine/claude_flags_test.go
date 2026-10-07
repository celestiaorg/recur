package engine

import (
	"testing"

	"recur/internal/domain"
)

func TestResolveClaudeContextWindow(t *testing.T) {
	// Sonnet 5 always 1M — ignore missing/stale context flags.
	if got := ResolveClaudeContextWindow("sonnet", nil); got != 1_000_000 {
		t.Errorf("sonnet nil flags = %d, want 1000000", got)
	}
	if got := ResolveClaudeContextWindow("sonnet", map[string]string{"context": "300k"}); got != 1_000_000 {
		t.Errorf("sonnet stale 300k = %d, want 1000000", got)
	}
	if got := ResolveClaudeContextWindow("claude-code/opus", nil); got != 1_000_000 {
		t.Errorf("opus = %d, want 1000000", got)
	}
	if got := ResolveClaudeContextWindow("claude-code/claude-opus-5", nil); got != 1_000_000 {
		t.Errorf("opus 5 = %d, want 1000000", got)
	}
	if got := ResolveClaudeContextWindow("claude-fable-5-1", nil); got != 1_000_000 {
		t.Errorf("fable 5.1 = %d, want 1000000", got)
	}

	// 4.6 models: 200K default, 1M when selected.
	if got := ResolveClaudeContextWindow("claude-sonnet-4-6", map[string]string{"context": "200k"}); got != 200_000 {
		t.Errorf("sonnet 4.6 200k = %d, want 200000", got)
	}
	if got := ResolveClaudeContextWindow("claude-opus-4-6", nil); got != 200_000 {
		t.Errorf("opus 4.6 default = %d, want 200000", got)
	}
	if got := ResolveClaudeContextWindow("claude-sonnet-4-6", map[string]string{"context": "1m"}); got != 1_000_000 {
		t.Errorf("sonnet 4.6 1m = %d, want 1000000", got)
	}

	// Haiku / unknown without 1m flag.
	if got := ResolveClaudeContextWindow("haiku", nil); got != 200_000 {
		t.Errorf("haiku = %d, want 200000", got)
	}
}

func TestContextWindowForMessageClaudeDockFlag(t *testing.T) {
	m := domain.Message{
		Provider:      domain.ProviderClaudeCode,
		ModelID:       "claude-sonnet-4-6",
		ContextWindow: 1_000_000, // provider ceiling — must not win over 200k
		InputTokens:   32_000,
	}
	got := contextWindowForMessage(m, "", map[string]string{"context": "200k"})
	if got != 200_000 {
		t.Fatalf("200k flag = %d, want 200000", got)
	}
	got = contextWindowForMessage(m, "", map[string]string{"context": "1m"})
	if got != 1_000_000 {
		t.Fatalf("1m flag = %d, want 1000000", got)
	}

	// Always-1M Sonnet 5 via session model when message ModelID empty.
	sonnet := domain.Message{
		Provider:      domain.ProviderClaudeCode,
		ContextWindow: 200_000,
		InputTokens:   10_000,
	}
	got = contextWindowForMessage(sonnet, "claude-code/sonnet", map[string]string{"context": "300k"})
	if got != 1_000_000 {
		t.Fatalf("sonnet session model = %d, want 1000000", got)
	}
}

func TestContextWindowForMessageCursorSessionModel(t *testing.T) {
	// Legacy Cursor turn ending on a tool_result: no ModelID, no stamped window.
	m := domain.Message{
		Provider:    domain.ProviderCursor,
		InputTokens: 50_000,
	}
	got := contextWindowForMessage(m, "cursor/gemini-3.1-pro", nil)
	if got != 1_000_000 {
		t.Fatalf("session model fallback = %d, want 1000000", got)
	}
}

func TestClaudeModelLabel(t *testing.T) {
	cases := []struct {
		in, want string
	}{
		{"claude-fable-5-1", "Fable 5.1"},
		{"claude-fable-5", "Fable 5"},
		{"claude-opus-4-8", "Opus 4.8"},
		{"claude-code/fable", "Fable"},
		{"opus", "Opus"},
		{"", ""},
	}
	for _, c := range cases {
		if got := claudeModelLabel(c.in); got != c.want {
			t.Errorf("claudeModelLabel(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}
