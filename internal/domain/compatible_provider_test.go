package domain

import "testing"

func TestParseModelRefCompatibleSlashInID(t *testing.T) {
	got := ParseModelRef("openrouter/openai/gpt-4o")
	if got.Provider != "openrouter" || got.ID != "openai/gpt-4o" {
		t.Errorf("ParseModelRef(openrouter/openai/gpt-4o) = %+v, want provider=openrouter id=openai/gpt-4o", got)
	}
}

func TestNormalizeCompatibleProvider(t *testing.T) {
	p, err := NormalizeCompatibleProvider(CompatibleProvider{
		ID:      " OpenAI ",
		BaseURL: "https://api.openai.com/",
		Models:  []CompatibleModel{{ID: "gpt-5", Label: "GPT-5"}},
	}, nil)
	if err != nil {
		t.Fatalf("NormalizeCompatibleProvider: %v", err)
	}
	if p.ID != "openai" || p.Name != "openai" || p.BaseURL != "https://api.openai.com" {
		t.Errorf("NormalizeCompatibleProvider() = %+v, want id=openai name=openai trimmed URL", p)
	}
}

func TestNormalizeCompatibleProviderRejectsReserved(t *testing.T) {
	_, err := NormalizeCompatibleProvider(CompatibleProvider{
		ID: "claude-code", BaseURL: "https://example.com", Models: []CompatibleModel{{ID: "x"}},
	}, nil)
	if err == nil {
		t.Fatal("NormalizeCompatibleProvider(claude-code) error = nil, want reserved")
	}
}

func TestNormalizeCompatibleProviderRejectsDuplicate(t *testing.T) {
	seen := map[string]bool{"openai": true}
	_, err := NormalizeCompatibleProvider(CompatibleProvider{
		ID: "openai", BaseURL: "https://api.openai.com", Models: []CompatibleModel{{ID: "x"}},
	}, seen)
	if err == nil {
		t.Fatal("NormalizeCompatibleProvider duplicate error = nil, want error")
	}
}

func TestCompatibleProviderOffersModels(t *testing.T) {
	s := Settings{
		CompatibleProviders: []CompatibleProvider{{
			ID: "openai", BaseURL: "https://api.openai.com",
			Models: []CompatibleModel{{ID: "gpt-5"}},
		}},
	}
	if !s.ProviderOffersModels("openai") {
		t.Fatal("ProviderOffersModels(openai) = false, want true when configured")
	}
	s.DisabledProviders = []string{"openai"}
	if s.ProviderOffersModels("openai") {
		t.Fatal("ProviderOffersModels(openai) = true, want false when disabled")
	}
}

func TestNormalizeProvidersReenablesCompatible(t *testing.T) {
	s := Settings{
		DisabledProviders: []string{ProviderClaudeCode, ProviderCursor, ProviderCodex, "openai"},
		CompatibleProviders: []CompatibleProvider{{
			ID: "openai", BaseURL: "https://api.openai.com",
			Models: []CompatibleModel{{ID: "gpt-5"}},
		}},
	}
	got := s.NormalizeProviders()
	if !got.ProviderEnabled("openai") {
		t.Fatal("NormalizeProviders should re-enable the compatible provider when nothing else offers models")
	}
}

func TestCompatibleFlagPreset(t *testing.T) {
	dims := CompatibleFlagPreset("openai")
	if len(dims) != 2 || dims[0].Wire != "reasoning.effort" || dims[1].Wire != "text.verbosity" {
		t.Errorf("CompatibleFlagPreset(openai) = %+v, want effort+verbosity wires", dims)
	}
	if CompatibleFlagPreset("none") != nil {
		t.Errorf("CompatibleFlagPreset(none) = %v, want nil", CompatibleFlagPreset("none"))
	}
}

func TestValidCompatibleWire(t *testing.T) {
	if !ValidCompatibleWire("reasoning.effort") || !ValidCompatibleWire("") {
		t.Fatal("ValidCompatibleWire should allow effort and empty")
	}
	if ValidCompatibleWire("tools.foo") {
		t.Fatal("ValidCompatibleWire(tools.foo) = true, want false")
	}
}

func TestNormalizeProvidersRemapsRemovedCompatible(t *testing.T) {
	s := Settings{
		ModelAgent: "openai/gpt-5",
		CompatibleProviders: []CompatibleProvider{{
			ID: "other", BaseURL: "https://example.com",
			Models: []CompatibleModel{{ID: "m"}},
		}},
	}
	got := s.NormalizeProviders()
	if got.ModelAgent != fallbackClaudeModel {
		t.Errorf("NormalizeProviders() ModelAgent = %q, want remapped off deleted openai slug", got.ModelAgent)
	}
}

func TestValidateCompatibleProvidersRejectsEmptyURL(t *testing.T) {
	err := ValidateCompatibleProviders([]CompatibleProvider{{
		ID: "openai", BaseURL: "", Models: []CompatibleModel{{ID: "gpt-5"}},
	}})
	if err == nil {
		t.Fatal("ValidateCompatibleProviders(empty URL) error = nil, want error")
	}
}

func TestValidateCompatibleProvidersRejectsDuplicateModelIDs(t *testing.T) {
	err := ValidateCompatibleProviders([]CompatibleProvider{{
		ID: "openai", BaseURL: "https://api.openai.com",
		Models: []CompatibleModel{
			{ID: "gpt-5", Label: "GPT-5", ContextWindow: 128000},
			{ID: " gpt-5 ", Label: "Also GPT-5", ContextWindow: 64000},
		},
	}})
	if err == nil {
		t.Fatal("ValidateCompatibleProviders(duplicate model) error = nil, want error")
	}
}

func TestValidateCompatibleProvidersRejectsDuplicateFlagIDs(t *testing.T) {
	err := ValidateCompatibleProviders([]CompatibleProvider{{
		ID: "openai", BaseURL: "https://api.openai.com",
		Models: []CompatibleModel{{
			ID: "gpt-5",
			Flags: []CompatibleFlagDim{
				{ID: "effort", Label: "Effort", Wire: "reasoning.effort", Options: []CompatibleFlagOption{{Value: "low"}}},
				{ID: "effort", Label: "Clobber", Wire: "text.verbosity", Options: []CompatibleFlagOption{{Value: "high", Default: true}}},
			},
		}},
	}})
	if err == nil {
		t.Fatal("ValidateCompatibleProviders(duplicate flag id) error = nil, want error")
	}
}

func TestValidateCompatibleProvidersRejectsDuplicateFlagOptions(t *testing.T) {
	err := ValidateCompatibleProviders([]CompatibleProvider{{
		ID: "openai", BaseURL: "https://api.openai.com",
		Models: []CompatibleModel{{
			ID: "gpt-5",
			Flags: []CompatibleFlagDim{{
				ID: "effort", Label: "Effort", Wire: "reasoning.effort",
				Options: []CompatibleFlagOption{
					{Value: "low", Label: "Low"},
					{Value: "medium", Label: "Medium", Default: true},
					{Value: "low", Label: "Also low"},
				},
			}},
		}},
	}})
	if err == nil {
		t.Fatal("ValidateCompatibleProviders(duplicate flag option) error = nil, want error")
	}
}
