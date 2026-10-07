package domain

import "testing"

func TestProviderEnabledDefaultOn(t *testing.T) {
	var s Settings
	for _, p := range []string{ProviderClaudeCode, ProviderCursor, ProviderCodex, ""} {
		if !s.ProviderEnabled(p) {
			t.Errorf("ProviderEnabled(%q) = false, want true on empty settings", p)
		}
	}
}

func TestProviderEnabledDisabledList(t *testing.T) {
	s := Settings{DisabledProviders: []string{ProviderCursor, ProviderCodex}}
	if !s.ProviderEnabled(ProviderClaudeCode) {
		t.Fatal("Claude should stay on")
	}
	if s.ProviderEnabled(ProviderCursor) || s.ProviderEnabled(ProviderCodex) {
		t.Fatal("listed providers should be off")
	}
}

func TestProviderOffersModels(t *testing.T) {
	s := Settings{CursorAPIKey: "sk-x", CodexChatGPTAuth: true}
	if !s.ProviderOffersModels(ProviderClaudeCode) {
		t.Fatal("Claude should offer models with no Recur key")
	}
	if !s.ProviderOffersModels(ProviderCursor) || !s.ProviderOffersModels(ProviderCodex) {
		t.Fatal("configured Cursor/Codex should offer models")
	}

	s.DisabledProviders = []string{ProviderCursor}
	if s.ProviderOffersModels(ProviderCursor) {
		t.Fatal("disabled Cursor must not offer models even with a key")
	}
	if (Settings{}).ProviderOffersModels(ProviderCursor) {
		t.Fatal("Cursor without a key must not offer models")
	}
	if (Settings{}).ProviderOffersModels(ProviderCodex) {
		t.Fatal("Codex without credentials must not offer models")
	}
}

func TestNormalizeProvidersReenablesLastCatalogue(t *testing.T) {
	s := Settings{
		DisabledProviders: []string{ProviderClaudeCode, ProviderCursor, ProviderCodex},
		ModelAgent:        "cursor/composer-2.5",
	}
	got := s.NormalizeProviders()
	if !got.ProviderEnabled(ProviderClaudeCode) {
		t.Fatal("NormalizeProviders should turn Claude back on")
	}
	if got.ModelAgent != fallbackClaudeModel {
		t.Errorf("NormalizeProviders() ModelAgent = %q, want %q", got.ModelAgent, fallbackClaudeModel)
	}
}

func TestNormalizeProvidersRemapsDisabledProvider(t *testing.T) {
	s := Settings{
		DisabledProviders:   []string{ProviderCursor},
		CursorAPIKey:        "sk-x",
		ModelAsk:            "cursor/composer-2.5",
		ModelPlan:           "cursor/grok-4.6",
		ModelAgent:          "cursor/composer-2.5",
		ModelReview:         "cursor/composer-2.5",
		ModelLoop:           "claude-code/sonnet",
		ModelWorkspaceTitle: "cursor/composer-2.5",
		FlagsAsk:            map[string]string{"fast": "true"},
		FlagsLoop:           map[string]string{"effort": "high"},
	}
	got := s.NormalizeProviders()
	if got.ModelAsk != fallbackClaudeModel || got.ModelPlan != fallbackClaudeModel {
		t.Errorf("NormalizeProviders() ask/plan = %q %q, want %q", got.ModelAsk, got.ModelPlan, fallbackClaudeModel)
	}
	if got.ModelLoop != "claude-code/sonnet" {
		t.Errorf("NormalizeProviders() ModelLoop = %q, want claude-code/sonnet", got.ModelLoop)
	}
	if got.FlagsAsk["effort"] != "high" {
		t.Errorf("NormalizeProviders() remapped ask flags = %v, want effort=high", got.FlagsAsk)
	}
	if got.ModelWorkspaceTitle != fallbackClaudeTitleModel {
		t.Errorf("NormalizeProviders() title = %q, want %q", got.ModelWorkspaceTitle, fallbackClaudeTitleModel)
	}
	if got.FlagsWorkspaceTitle["effort"] != "low" {
		t.Errorf("NormalizeProviders() title flags = %v, want effort=low", got.FlagsWorkspaceTitle)
	}
	if got.CursorAPIKey != "sk-x" {
		t.Fatal("turning Cursor off must keep the API key")
	}
}

func TestNormalizeProvidersLeavesUnconfiguredPick(t *testing.T) {
	s := Settings{
		ModelAgent:   "codex/gpt-5.6-luna",
		FlagsAgent:   map[string]string{"effort": "high"},
		CodexAPIKey:  "",
		ModelAsk:     "cursor/composer-2.5",
		CursorAPIKey: "",
	}
	got := s.NormalizeProviders()
	if got.ModelAgent != "codex/gpt-5.6-luna" {
		t.Errorf("NormalizeProviders() ModelAgent = %q, want unchanged when Codex is on but unconfigured", got.ModelAgent)
	}
	if got.ModelAsk != "cursor/composer-2.5" {
		t.Errorf("NormalizeProviders() ModelAsk = %q, want unchanged when Cursor is on but unconfigured", got.ModelAsk)
	}
}

func TestNormalizeProvidersDoesNotInventCursorFallback(t *testing.T) {
	s := Settings{
		DisabledProviders: []string{ProviderClaudeCode},
		CursorAPIKey:      "sk-x",
		ModelAgent:        "claude-code/sonnet",
	}
	got := s.NormalizeProviders()
	if got.ModelAgent != "claude-code/sonnet" {
		t.Errorf("NormalizeProviders() ModelAgent = %q, want the Claude ref left in place", got.ModelAgent)
	}
	if !got.NeedsCatalogFallback() {
		t.Fatal("NeedsCatalogFallback() = false, want true when Claude-off slots still name Claude")
	}
}

func TestNeedsCatalogFallbackEmptyTitleWhenClaudeOff(t *testing.T) {
	s := Settings{DisabledProviders: []string{ProviderClaudeCode}, CursorAPIKey: "sk-x"}
	if !s.NeedsCatalogFallback() {
		t.Fatal("NeedsCatalogFallback() = false, want true: empty title defaults to haiku")
	}
	s.ModelAsk, s.ModelPlan, s.ModelAgent, s.ModelLoop = "cursor/composer-2", "cursor/composer-2", "cursor/composer-2", "cursor/composer-2"
	s.ModelWorkspaceTitle = "cursor/composer-2"
	if s.NeedsCatalogFallback() {
		t.Fatal("NeedsCatalogFallback() = true, want false when every slot is on an enabled provider")
	}
}

func TestModelRefUnavailableRemovedCompatible(t *testing.T) {
	s := Settings{
		DisabledProviders: []string{ProviderClaudeCode},
		CompatibleProviders: []CompatibleProvider{{
			ID: "beta", BaseURL: "https://b.example",
			Models: []CompatibleModel{{ID: "m2"}},
		}},
		ModelAsk:            "alpha/m1",
		ModelPlan:           "alpha/m1",
		ModelAgent:          "alpha/m1",
		ModelLoop:           "alpha/m1",
		ModelWorkspaceTitle: "alpha/m1",
	}
	if !s.ModelRefUnavailable("alpha/m1", false) {
		t.Fatal("ModelRefUnavailable(removed alpha) = false, want true")
	}
	if s.ModelRefUnavailable("beta/m2", false) {
		t.Fatal("ModelRefUnavailable(beta) = true, want false")
	}
	if !s.NeedsCatalogFallback() {
		t.Fatal("NeedsCatalogFallback() = false, want true when slots still name removed alpha")
	}
}
