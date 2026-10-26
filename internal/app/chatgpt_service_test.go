package app

import (
	"testing"

	"recur/internal/domain"
)

func TestChatGPTCatalogueAndFallbackWithoutCodex(t *testing.T) {
	s := domain.Settings{ChatGPTAccountID: "registration", ChatGPTModels: []domain.CompatibleModel{{ID: "gpt-first", Label: "First"}}, DisabledProviders: []string{domain.ProviderClaudeCode, domain.ProviderCursor, domain.ProviderCodex}}
	groups := buildModelCatalog(s, nil, nil)
	if len(groups) != 1 || groups[0].Provider != domain.ProviderChatGPT || groups[0].Models[0].Ref != "chatgpt/gpt-first" {
		t.Fatalf("catalogue = %+v", groups)
	}
	s = applyCatalogFallbacks(s, groups)
	if s.ModelAsk != "chatgpt/gpt-first" || s.ModelAgent != "chatgpt/gpt-first" {
		t.Fatal("ChatGPT-only setup did not remap model slots")
	}
	if !(&SettingsService{}).providerRunnable(s, domain.ProviderChatGPT) {
		t.Fatal("ChatGPT required a binary")
	}
}
