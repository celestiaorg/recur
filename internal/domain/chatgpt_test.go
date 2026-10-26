package domain

import "testing"

func TestChatGPTNativeProviderEnablement(t *testing.T) {
	s := Settings{ChatGPTAccountID: "registration", ChatGPTModels: []CompatibleModel{{ID: "gpt-test"}}, DisabledProviders: []string{ProviderClaudeCode, ProviderCursor, ProviderCodex}}
	s.ModelAgent = "chatgpt/gpt-test"
	s = s.NormalizeProviders()
	if s.ProviderEnabled(ProviderClaudeCode) || !s.ProviderOffersModels(ProviderChatGPT) || s.ModelRefUnavailable(s.ModelAgent, false) {
		t.Fatal("a connected ChatGPT provider was not usable independently")
	}
	if !s.UsesNativeHarness(ProviderChatGPT) || BuiltinProvider(ProviderChatGPT) {
		t.Fatal("ChatGPT should use Recur's tool loop and context")
	}
	if err := ValidateCompatibleProviders([]CompatibleProvider{{ID: ProviderChatGPT, BaseURL: "https://example.com", Models: []CompatibleModel{{ID: "x"}}}}); err == nil {
		t.Fatal("custom endpoint can shadow ChatGPT credentials")
	}
}
