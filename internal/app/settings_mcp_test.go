package app

import (
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestSetSettingsRejectsDuplicateMCP(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.GetSettings()
	set.MCPServers = []domain.MCPServer{
		{ID: "acme", Name: "Acme", Enabled: true, URL: "https://keep.example/mcp"},
	}
	mustSetSettings(t, svc.Settings, set)

	set = svc.Settings.GetSettings()
	set.MCPServers = []domain.MCPServer{
		{ID: "acme", Name: "Acme", Enabled: true, URL: "https://keep.example/mcp"},
		{ID: "acme", Name: "Acme 2", Enabled: true, URL: "https://clobber.example/mcp"},
	}
	got, err := svc.Settings.SetSettings(set)
	if err == nil {
		t.Fatal("SetSettings(duplicate acme) err = nil, want validation error")
	}
	if !strings.Contains(err.Error(), "duplicate") {
		t.Fatalf("SetSettings(duplicate acme) err = %v, want duplicate", err)
	}
	stored := svc.Settings.GetSettings()
	if len(stored.MCPServers) != 1 || stored.MCPServers[0].URL != "https://keep.example/mcp" {
		t.Fatalf("stored MCPServers = %+v, want previous catalog intact", stored.MCPServers)
	}
	if len(got.MCPServers) != 1 || got.MCPServers[0].URL != "https://keep.example/mcp" {
		t.Fatalf("SetSettings() = %+v, want previous catalog", got.MCPServers)
	}
}

func TestSetSettingsRejectsReservedMCP(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.GetSettings()
	set.MCPServers = []domain.MCPServer{
		{ID: "acme", Name: "Acme", Enabled: true, URL: "https://keep.example/mcp"},
	}
	mustSetSettings(t, svc.Settings, set)

	set = svc.Settings.GetSettings()
	set.MCPServers = append(set.MCPServers, domain.MCPServer{ID: "recur", Enabled: true})
	if _, err := svc.Settings.SetSettings(set); err == nil {
		t.Fatal("SetSettings(recur) err = nil, want reserved error")
	}
	stored := svc.Settings.GetSettings()
	if len(stored.MCPServers) != 1 || stored.MCPServers[0].ID != "acme" {
		t.Fatalf("stored MCPServers = %+v, want previous catalog intact", stored.MCPServers)
	}
}

func TestSetSettingsRejectsInvalidCompatibleProvider(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.GetSettings()
	good := domain.CompatibleProvider{
		ID: "openai", Name: "OpenAI", BaseURL: "https://api.openai.com",
		APIKey: "sk-keep", Models: []domain.CompatibleModel{{ID: "gpt-5", Label: "GPT-5"}},
	}
	set.CompatibleProviders = []domain.CompatibleProvider{good}
	mustSetSettings(t, svc.Settings, set)

	set = svc.Settings.GetSettings()
	set.CompatibleProviders = []domain.CompatibleProvider{{
		ID: "openai", Name: "OpenAI", BaseURL: "not-a-url",
		APIKey: "sk-keep", Models: []domain.CompatibleModel{{ID: "gpt-5"}},
	}}
	got, err := svc.Settings.SetSettings(set)
	if err == nil {
		t.Fatal("SetSettings(bad URL) err = nil, want validation error")
	}
	if len(got.CompatibleProviders) != 1 || got.CompatibleProviders[0].BaseURL != "https://api.openai.com" {
		t.Fatalf("SetSettings() returned %+v, want previous provider kept", got.CompatibleProviders)
	}
	stored := svc.Settings.GetSettings()
	if len(stored.CompatibleProviders) != 1 || stored.CompatibleProviders[0].APIKey != "sk-keep" {
		t.Fatalf("stored compatible provider = %+v, want previous API key intact", stored.CompatibleProviders)
	}
}

func TestSetSettingsRejectsDuplicateCompatibleModel(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.GetSettings()
	good := domain.CompatibleProvider{
		ID: "openai", Name: "OpenAI", BaseURL: "https://api.openai.com",
		APIKey: "sk-keep",
		Models: []domain.CompatibleModel{
			{ID: "gpt-5", Label: "GPT-5", ContextWindow: 128000},
			{ID: "gpt-4o", Label: "GPT-4o", ContextWindow: 128000},
		},
	}
	set.CompatibleProviders = []domain.CompatibleProvider{good}
	mustSetSettings(t, svc.Settings, set)

	set = svc.Settings.GetSettings()
	set.CompatibleProviders = []domain.CompatibleProvider{{
		ID: "openai", Name: "OpenAI", BaseURL: "https://api.openai.com",
		APIKey: "sk-keep",
		Models: []domain.CompatibleModel{
			{ID: "gpt-5", Label: "GPT-5", ContextWindow: 128000},
			{ID: "gpt-5", Label: "Clobber", ContextWindow: 1},
		},
	}}
	got, err := svc.Settings.SetSettings(set)
	if err == nil {
		t.Fatal("SetSettings(duplicate model id) err = nil, want validation error")
	}
	if !strings.Contains(err.Error(), "duplicate model id") {
		t.Fatalf("SetSettings(duplicate model id) err = %v, want duplicate model id", err)
	}
	if len(got.CompatibleProviders) != 1 || len(got.CompatibleProviders[0].Models) != 2 {
		t.Fatalf("SetSettings() returned %+v, want previous models kept", got.CompatibleProviders)
	}
	if got.CompatibleProviders[0].Models[1].ID != "gpt-4o" {
		t.Fatalf("SetSettings() models = %+v, want gpt-4o preserved", got.CompatibleProviders[0].Models)
	}
}

func TestSetSettingsRejectsDuplicateCompatibleFlagID(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.GetSettings()
	good := domain.CompatibleProvider{
		ID: "openai", Name: "OpenAI", BaseURL: "https://api.openai.com",
		APIKey: "sk-keep",
		Models: []domain.CompatibleModel{{
			ID: "gpt-5", Label: "GPT-5",
			Flags: []domain.CompatibleFlagDim{
				{
					ID: "effort", Label: "Effort", Wire: "reasoning.effort",
					Options: []domain.CompatibleFlagOption{{Value: "low"}, {Value: "medium", Default: true}},
				},
				{
					ID: "verbosity", Label: "Verbosity", Wire: "text.verbosity",
					Options: []domain.CompatibleFlagOption{{Value: "low"}, {Value: "medium", Default: true}},
				},
			},
		}},
	}
	set.CompatibleProviders = []domain.CompatibleProvider{good}
	mustSetSettings(t, svc.Settings, set)

	set = svc.Settings.GetSettings()
	set.CompatibleProviders = []domain.CompatibleProvider{{
		ID: "openai", Name: "OpenAI", BaseURL: "https://api.openai.com",
		APIKey: "sk-keep",
		Models: []domain.CompatibleModel{{
			ID: "gpt-5", Label: "GPT-5",
			Flags: []domain.CompatibleFlagDim{
				{
					ID: "effort", Label: "Effort", Wire: "reasoning.effort",
					Options: []domain.CompatibleFlagOption{{Value: "low"}, {Value: "medium", Default: true}},
				},
				{
					ID: "effort", Label: "Verbosity", Wire: "text.verbosity",
					Options: []domain.CompatibleFlagOption{{Value: "low"}, {Value: "medium", Default: true}},
				},
			},
		}},
	}}
	got, err := svc.Settings.SetSettings(set)
	if err == nil {
		t.Fatal("SetSettings(duplicate flag id) err = nil, want validation error")
	}
	if !strings.Contains(err.Error(), "duplicate flag id") {
		t.Fatalf("SetSettings(duplicate flag id) err = %v, want duplicate flag id", err)
	}
	if len(got.CompatibleProviders) != 1 || len(got.CompatibleProviders[0].Models) != 1 {
		t.Fatalf("SetSettings() returned %+v, want previous provider kept", got.CompatibleProviders)
	}
	flags := got.CompatibleProviders[0].Models[0].Flags
	if len(flags) != 2 || flags[0].ID != "effort" || flags[1].ID != "verbosity" {
		t.Fatalf("SetSettings() flags = %+v, want effort+verbosity preserved", flags)
	}
}

func TestSetSettingsRejectsDuplicateCompatibleFlagOption(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.GetSettings()
	good := domain.CompatibleProvider{
		ID: "openai", Name: "OpenAI", BaseURL: "https://api.openai.com",
		APIKey: "sk-keep",
		Models: []domain.CompatibleModel{{
			ID: "gpt-5", Label: "GPT-5",
			Flags: []domain.CompatibleFlagDim{{
				ID: "effort", Label: "Effort", Wire: "reasoning.effort",
				Options: []domain.CompatibleFlagOption{
					{Value: "low", Label: "Low"},
					{Value: "medium", Label: "Medium", Default: true},
					{Value: "high", Label: "High"},
				},
			}},
		}},
	}
	set.CompatibleProviders = []domain.CompatibleProvider{good}
	mustSetSettings(t, svc.Settings, set)

	set = svc.Settings.GetSettings()
	set.CompatibleProviders = []domain.CompatibleProvider{{
		ID: "openai", Name: "OpenAI", BaseURL: "https://api.openai.com",
		APIKey: "sk-keep",
		Models: []domain.CompatibleModel{{
			ID: "gpt-5", Label: "GPT-5",
			Flags: []domain.CompatibleFlagDim{{
				ID: "effort", Label: "Effort", Wire: "reasoning.effort",
				Options: []domain.CompatibleFlagOption{
					{Value: "low", Label: "Low"},
					{Value: "low", Label: "Medium", Default: true},
					{Value: "high", Label: "High"},
				},
			}},
		}},
	}}
	got, err := svc.Settings.SetSettings(set)
	if err == nil {
		t.Fatal("SetSettings(duplicate flag option) err = nil, want validation error")
	}
	if !strings.Contains(err.Error(), "duplicate flag option") {
		t.Fatalf("SetSettings(duplicate flag option) err = %v, want duplicate flag option", err)
	}
	opts := got.CompatibleProviders[0].Models[0].Flags[0].Options
	if len(opts) != 3 || opts[1].Value != "medium" || !opts[1].Default {
		t.Fatalf("SetSettings() options = %+v, want medium default preserved", opts)
	}
}
