package app

import (
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/codex"
	"recur/internal/cursor"
	"recur/internal/domain"
)

// Claude Code must always be offered — every install has it, and without a
// group the pickers would be empty on a fresh machine.
func TestModelCatalogAlwaysHasClaudeCode(t *testing.T) {
	groups := (&SettingsService{}).claudeOnlyCatalog()
	if len(groups) == 0 {
		t.Fatal("catalogue is empty")
	}
	if groups[0].Provider != domain.ProviderClaudeCode {
		t.Errorf("first group = %q, want %q", groups[0].Provider, domain.ProviderClaudeCode)
	}
	if groups[0].Label != "Claude Code" {
		t.Errorf("label = %q, want Claude Code", groups[0].Label)
	}
	if len(groups[0].Models) == 0 {
		t.Fatal("Claude Code group has no models")
	}
}

// Every ref must be fully qualified, since the prefix is what routes the turn.
// A bare id would silently run on Claude Code no matter which group it came from.
func TestModelCatalogRefsAreQualified(t *testing.T) {
	for _, g := range (&SettingsService{}).claudeOnlyCatalog() {
		for _, m := range g.Models {
			if !strings.HasPrefix(m.Ref, g.Provider+"/") {
				t.Errorf("%q is not prefixed with %q", m.Ref, g.Provider)
			}
			ref := domain.ParseModelRef(m.Ref)
			if ref.Provider != g.Provider || ref.ID == "" {
				t.Errorf("%q parses to %+v, want provider %q with an id", m.Ref, ref, g.Provider)
			}
			if m.Label == "" {
				t.Errorf("%q has no label", m.Ref)
			}
			if strings.Contains(m.Ref, "[1m]") {
				t.Errorf("%q still encodes context in the ref; want a context flag", m.Ref)
			}
		}
	}
}

func TestClaudeCatalogHasEffortAndContextFlags(t *testing.T) {
	// Only 4.6 still exposes a real 200K↔1M toggle; newer models are always 1M.
	wantContext := map[string]bool{
		"claude-code/claude-opus-4-6":   true,
		"claude-code/claude-sonnet-4-6": true,
	}
	for _, m := range claudeCodeModels {
		if len(m.Flags) == 0 {
			t.Errorf("%s has no flags", m.Ref)
			continue
		}
		if m.Flags[0].ID != "effort" {
			t.Errorf("%s first flag = %q, want effort", m.Ref, m.Flags[0].ID)
		}
		hasContext := false
		for _, f := range m.Flags {
			if f.ID != "context" {
				continue
			}
			hasContext = true
			if len(f.Options) != 2 || f.Options[0].Value != "200k" || f.Options[1].Value != "1m" {
				t.Errorf("%s context options = %+v, want 200k/1m", m.Ref, f.Options)
			}
		}
		if hasContext != wantContext[m.Ref] {
			t.Errorf("%s context flag = %v, want %v", m.Ref, hasContext, wantContext[m.Ref])
		}
		if !hasContext && m.ContextWindow <= 0 {
			t.Errorf("%s has no context picker and no ContextWindow", m.Ref)
		}
	}
}

func TestClaudeCatalogTracksFable51(t *testing.T) {
	var fable, opus []string
	for _, m := range claudeCodeModels {
		row := m.Ref + " " + m.Label
		switch {
		case strings.Contains(m.Ref, "fable"):
			fable = append(fable, row)
		case strings.Contains(m.Ref, "opus") && !strings.Contains(m.Ref, "opus-4"):
			opus = append(opus, row)
		}
	}
	if got, want := strings.Join(fable, ","), "claude-code/claude-fable-5-1 Fable 5.1,claude-code/claude-fable-5 Fable 5"; got != want {
		t.Errorf("fable rows = %s, want %s", got, want)
	}
	if got, want := strings.Join(opus, ","), "claude-code/claude-opus-5-5 Opus 5.5,claude-code/claude-opus-5 Opus 5"; got != want {
		t.Errorf("opus rows = %s, want %s", got, want)
	}
}

// The seeded defaults must exist in the catalogue, or a fresh install opens
// Settings showing a model that is not in its own picker.
func TestSeededDefaultIsInCatalog(t *testing.T) {
	const seeded = "claude-code/claude-sonnet-5-5"
	for _, g := range (&SettingsService{}).claudeOnlyCatalog() {
		for _, m := range g.Models {
			if m.Ref == seeded {
				return
			}
		}
	}
	t.Errorf("seeded default %q is not offered by the catalogue", seeded)
}

func TestCursorCatalogHouseModelsFirst(t *testing.T) {
	out := cursorCatalogModels([]cursor.Model{
		{ID: "gpt-5.6-sol", DisplayName: "GPT-5.6 Sol"},
		{ID: "composer-2.5", DisplayName: "Composer 2.5"},
		{ID: "claude-opus-5", DisplayName: "Opus 5"},
		{ID: "default", DisplayName: "Auto"},
		{ID: "grok-4.6", DisplayName: "Grok 4.6"},
		{ID: "gemini-3.1-pro", DisplayName: "Gemini 3.1 Pro"},
		{ID: "auto-smart", DisplayName: "Auto"},
	})
	got := make([]string, 0, len(out))
	for _, m := range out {
		got = append(got, m.Ref)
	}
	want := []string{
		"cursor/composer-2.5",
		"cursor/grok-4.6",
		"cursor/auto-smart",
		"cursor/claude-opus-5",
		"cursor/gpt-5.6-sol",
		"cursor/gemini-3.1-pro",
	}
	if strings.Join(got, ",") != strings.Join(want, ",") {
		t.Errorf("order = %s, want %s", strings.Join(got, ","), strings.Join(want, ","))
	}
}

func TestCursorCatalogOmitsDefault(t *testing.T) {
	out := cursorCatalogModels([]cursor.Model{
		{ID: "auto-smart", DisplayName: "Auto"},
		{ID: "default", DisplayName: "Auto"},
		{ID: "composer-2.5", DisplayName: "Composer 2.5"},
		{ID: "grok-4.6", DisplayName: "Grok 4.6"},
	})
	if len(out) != 3 {
		t.Fatalf("got %d models, want 3: %+v", len(out), out)
	}
	if out[0].Ref != "cursor/auto-smart" || out[0].Label != "Auto" {
		t.Errorf("first = %+v, want cursor/auto-smart Auto", out[0])
	}
	if out[1].Ref != "cursor/composer-2.5" {
		t.Errorf("second = %+v", out[1])
	}
	if out[1].ContextWindow != 200_000 {
		t.Errorf("composer ContextWindow = %d, want 200000", out[1].ContextWindow)
	}
	if out[2].Ref != "cursor/grok-4.6" || out[2].Label != "Grok 4.6" {
		t.Errorf("third = %+v, want cursor/grok-4.6 Grok 4.6", out[2])
	}
	if out[2].ContextWindow != 500_000 {
		t.Errorf("grok-4.6 ContextWindow = %d, want 500000", out[2].ContextWindow)
	}
}

func TestCursorParametersToFlags(t *testing.T) {
	flags := cursorParametersToFlags(
		[]cursor.ModelParameterDefinition{{
			ID:          "fast",
			DisplayName: "Fast",
			Values: []cursor.ModelParameterChoice{
				{Value: "false"},
				{Value: "true", DisplayName: "Fast"},
			},
		}},
		[]cursor.ModelVariant{{
			DisplayName: "Fast",
			IsDefault:   true,
			Params:      []cursor.ModelParameterValue{{ID: "fast", Value: "true"}},
		}},
	)
	if len(flags) != 1 || flags[0].ID != "fast" {
		t.Fatalf("flags = %+v", flags)
	}
	if len(flags[0].Options) != 2 {
		t.Fatalf("options = %+v", flags[0].Options)
	}
	// Boolean params render as On/Off with On first; wire values stay true/false.
	if flags[0].Options[0].Value != "true" || flags[0].Options[0].Label != "On" {
		t.Errorf("first option = %+v, want On/true", flags[0].Options[0])
	}
	if flags[0].Options[1].Value != "false" || flags[0].Options[1].Label != "Off" {
		t.Errorf("second option = %+v, want Off/false", flags[0].Options[1])
	}
	if !flags[0].Options[0].Default || flags[0].Options[1].Default {
		t.Errorf("default should be On: %+v", flags[0].Options)
	}
}

func TestCursorThinkingBoolLabels(t *testing.T) {
	flags := cursorParametersToFlags(
		[]cursor.ModelParameterDefinition{{
			ID: "thinking",
			Values: []cursor.ModelParameterChoice{
				{Value: "true"},
				{Value: "false"},
			},
		}},
		nil,
	)
	if len(flags) != 1 {
		t.Fatalf("flags = %+v", flags)
	}
	if flags[0].Options[0].Label != "On" || flags[0].Options[0].Value != "true" {
		t.Errorf("want On first: %+v", flags[0].Options)
	}
	if flags[0].Options[1].Label != "Off" || flags[0].Options[1].Value != "false" {
		t.Errorf("want Off second: %+v", flags[0].Options)
	}
	// No variant default → preserve SDK "first value is default" (true).
	if !flags[0].Options[0].Default {
		t.Errorf("On should remain default when SDK lists true first: %+v", flags[0].Options)
	}
}

func TestCursorParametersDefaultToFirstWhenNoVariant(t *testing.T) {
	flags := cursorParametersToFlags(
		[]cursor.ModelParameterDefinition{{
			ID: "effort",
			Values: []cursor.ModelParameterChoice{
				{Value: "low"},
				{Value: "high", DisplayName: "High"},
			},
		}},
		nil,
	)
	if len(flags) != 1 || !flags[0].Options[0].Default || flags[0].Options[1].Default {
		t.Fatalf("want first option default: %+v", flags)
	}
	if flags[0].Options[1].Label != "High" {
		t.Errorf("label = %q", flags[0].Options[1].Label)
	}
}

// claudeOnlyCatalog is the catalogue without the live Cursor fetch, so these
// tests need no bridge, no API key and no network.
func (s *SettingsService) claudeOnlyCatalog() []ModelGroup {
	return buildModelCatalog(domain.Settings{}, nil, nil)
}

func TestModelCatalogOmitsDisabledProviders(t *testing.T) {
	empty := buildModelCatalog(domain.Settings{DisabledProviders: []string{domain.ProviderClaudeCode}}, nil, nil)
	if len(empty) != 0 {
		t.Fatalf("disabled Claude with no other models: got %d groups, want 0", len(empty))
	}

	groups := buildModelCatalog(
		domain.Settings{DisabledProviders: []string{domain.ProviderClaudeCode}},
		[]cursor.Model{{ID: "composer-2.5", DisplayName: "Composer 2.5"}},
		nil,
	)
	if len(groups) != 1 || groups[0].Provider != domain.ProviderCursor {
		t.Fatalf("groups = %+v, want only Cursor", groups)
	}

	groups = buildModelCatalog(
		domain.Settings{DisabledProviders: []string{domain.ProviderCursor, domain.ProviderCodex}},
		[]cursor.Model{{ID: "composer-2.5", DisplayName: "Composer 2.5"}},
		[]codex.Model{{ID: "gpt-5.6-luna", DisplayName: "GPT-5.6-Luna"}},
	)
	if len(groups) != 1 || groups[0].Provider != domain.ProviderClaudeCode {
		t.Fatalf("groups = %+v, want only Claude Code", groups)
	}
}

func TestProviderLabel(t *testing.T) {
	if got := domain.ProviderLabel(domain.ProviderCursor); got != "Cursor" {
		t.Errorf("cursor label = %q", got)
	}
	if got := domain.ProviderLabel(domain.ProviderClaudeCode); got != "Claude Code" {
		t.Errorf("claude label = %q", got)
	}
	if got := domain.ProviderLabel(domain.ProviderCodex); got != "Codex" {
		t.Errorf("codex label = %q", got)
	}
	// An unknown slug renders as itself rather than blank.
	if got := domain.ProviderLabel("openai"); got != "openai" {
		t.Errorf("unknown label = %q, want the slug", got)
	}
}

func TestFriendlyCodexModelLabel(t *testing.T) {
	cases := []struct{ display, id, want string }{
		{"GPT-5.6-Sol", "gpt-5.6-sol", "GPT-5.6 Sol"},
		{"GPT-5.6-Luna", "", "GPT-5.6 Luna"},
		{"", "gpt-5.6-terra", "GPT-5.6 Terra"},
		{"", "gpt-6-astra", "GPT-6 Astra"},
		{"GPT-6-Astra", "", "GPT-6 Astra"},
		{"", "o3", "O3"},
	}
	for _, tc := range cases {
		if got := friendlyCodexModelLabel(tc.display, tc.id); got != tc.want {
			t.Errorf("friendlyCodexModelLabel(%q, %q) = %q, want %q", tc.display, tc.id, got, tc.want)
		}
	}
}

func TestFriendlyCodexEffortLabel(t *testing.T) {
	cases := []struct{ in, want string }{
		{"xhigh", "Extra High"},
		{"ultra", "Ultra"},
		{"max", "Max"},
	}
	for _, tc := range cases {
		if got := friendlyCodexEffortLabel(tc.in); got != tc.want {
			t.Errorf("friendlyCodexEffortLabel(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestCodexCatalogModelsFriendlyLabels(t *testing.T) {
	out := codexCatalogModels([]codex.Model{{
		ID:                     "gpt-5.6-luna",
		DisplayName:            "GPT-5.6-Luna",
		DefaultReasoningEffort: "medium",
		SupportedReasoningEffort: []codex.ReasoningEffort{
			{ReasoningEffort: "low"},
			{ReasoningEffort: "medium"},
			{ReasoningEffort: "xhigh"},
		},
		DefaultServiceTier: "priority",
		ServiceTiers: []codex.ServiceTier{
			{ID: "priority", Name: "Fast", Description: "1.5x speed"},
		},
	}})
	if len(out) != 1 {
		t.Fatalf("got %d models", len(out))
	}
	if out[0].Ref != "codex/gpt-5.6-luna" || out[0].Label != "GPT-5.6 Luna" {
		t.Errorf("model = %+v", out[0])
	}
	if len(out[0].Flags) != 3 {
		t.Fatalf("flags = %+v", out[0].Flags)
	}
	if out[0].Flags[0].ID != "effort" {
		t.Fatalf("first flag = %q, want effort", out[0].Flags[0].ID)
	}
	want := []string{"Low", "Medium", "Extra High"}
	for i, o := range out[0].Flags[0].Options {
		if o.Label != want[i] {
			t.Errorf("option[%d] label = %q, want %q", i, o.Label, want[i])
		}
	}
	if !out[0].Flags[0].Options[1].Default {
		t.Errorf("medium should be default: %+v", out[0].Flags[0].Options)
	}
	ctx := out[0].Flags[1]
	if ctx.ID != "context" || ctx.Label != "Context" {
		t.Fatalf("context flag = %+v", ctx)
	}
	if len(ctx.Options) != 2 || ctx.Options[0].Value != "272k" || !ctx.Options[0].Default || ctx.Options[1].Value != "1m" {
		t.Fatalf("context options = %+v, want 272k default / 1m", ctx.Options)
	}
	fast := out[0].Flags[2]
	if fast.ID != "fast" || fast.Label != "Fast" {
		t.Fatalf("fast flag = %+v", fast)
	}
	if len(fast.Options) != 2 || fast.Options[0].Value != "priority" || fast.Options[1].Value != "false" {
		t.Fatalf("fast options = %+v (On value should be the boost tier id)", fast.Options)
	}
	if fast.Options[0].Label != "On" || fast.Options[1].Label != "Off" {
		t.Fatalf("fast labels = %+v", fast.Options)
	}
	if !fast.Options[0].Default || fast.Options[1].Default {
		t.Errorf("Fast should default On when API defaultServiceTier is set: %+v", fast.Options)
	}
}

func TestCodexCatalogShowsGPT6Astra(t *testing.T) {
	out := codexCatalogModels([]codex.Model{
		{ID: "gpt-5.4-mini", Hidden: true, DisplayName: "GPT-5.4 Mini"},
		{
			ID:                     "gpt-6-astra",
			DisplayName:            "GPT-6-Astra",
			DefaultReasoningEffort: "high",
			SupportedReasoningEffort: []codex.ReasoningEffort{
				{ReasoningEffort: "high"},
			},
		},
	})
	if len(out) != 1 {
		t.Fatalf("got %d models, want only gpt-6-astra: %+v", len(out), out)
	}
	if out[0].Ref != "codex/gpt-6-astra" || out[0].Label != "GPT-6 Astra" {
		t.Errorf("model = %+v", out[0])
	}
	var ctx *FlagDimension
	for i := range out[0].Flags {
		if out[0].Flags[i].ID == "context" {
			ctx = &out[0].Flags[i]
			break
		}
	}
	if ctx == nil {
		t.Fatalf("flags = %+v, want 272k/1m context on gpt-6-astra", out[0].Flags)
	}
	if len(ctx.Options) != 2 || ctx.Options[0].Value != "272k" || !ctx.Options[0].Default || ctx.Options[1].Value != "1m" {
		t.Fatalf("context options = %+v, want 272k default / 1m", ctx.Options)
	}
}

func TestCodexSpeedFlagsOmittedWithoutTiers(t *testing.T) {
	out := codexCatalogModels([]codex.Model{{
		ID:                     "gpt-5.4-mini",
		DisplayName:            "GPT-5.4 Mini",
		DefaultReasoningEffort: "medium",
		SupportedReasoningEffort: []codex.ReasoningEffort{
			{ReasoningEffort: "low"},
			{ReasoningEffort: "medium"},
		},
	}})
	if len(out) != 1 || len(out[0].Flags) != 1 || out[0].Flags[0].ID != "effort" {
		t.Fatalf("want effort-only flags: %+v", out)
	}
}

func TestCursorPassesThroughListModelsContext(t *testing.T) {
	got := cursorModelChoice(cursor.Model{
		ID:          "gpt-5.6-sol",
		DisplayName: "GPT-5.6 Sol",
		Parameters: []cursor.ModelParameterDefinition{{
			ID:          "context",
			DisplayName: "Context",
			Values: []cursor.ModelParameterChoice{
				{Value: "272k", DisplayName: "272K"},
				{Value: "1m", DisplayName: "1M"},
			},
		}},
		Variants: []cursor.ModelVariant{{
			DisplayName: "GPT-5.6 Sol",
			IsDefault:   true,
			Params:      []cursor.ModelParameterValue{{ID: "context", Value: "1m"}},
		}},
	})
	var ctx *FlagDimension
	for i := range got.Flags {
		if got.Flags[i].ID == "context" {
			ctx = &got.Flags[i]
			break
		}
	}
	if ctx == nil {
		t.Fatalf("flags = %+v, want ListModels context param", got.Flags)
	}
	if len(ctx.Options) != 2 || ctx.Options[0].Value != "272k" || ctx.Options[1].Value != "1m" {
		t.Fatalf("context options = %+v, want 272k / 1m from ListModels", ctx.Options)
	}
	if ctx.Options[0].Default || !ctx.Options[1].Default {
		t.Fatalf("should keep Cursor's default variant (1m): %+v", ctx.Options)
	}
}

func TestCursorModelChoiceDoesNotInventGPTContext(t *testing.T) {
	got := cursorModelChoice(cursor.Model{ID: "gpt-5.6-sol", DisplayName: "GPT-5.6 Sol"})
	for _, f := range got.Flags {
		if f.ID == "context" {
			t.Fatalf("should not inject context when ListModels omitted it: %+v", got.Flags)
		}
	}
}

func TestApplyCatalogFallbacksRepointsClaudeWhenOff(t *testing.T) {
	set := domain.Settings{
		DisabledProviders:   []string{domain.ProviderClaudeCode},
		CursorAPIKey:        "sk-x",
		ModelAsk:            "claude-code/sonnet",
		ModelPlan:           "claude-code/opus",
		ModelAgent:          "claude-code/sonnet",
		ModelLoop:           "claude-code/opus",
		ModelReview:         "claude-code/fable",
		ModelWorkspaceTitle: "",
	}
	groups := buildModelCatalog(set, []cursor.Model{
		{ID: "composer-2.5", DisplayName: "Composer 2.5"},
		{ID: "grok-4.6", DisplayName: "Grok 4.6"},
	}, nil)
	got := applyCatalogFallbacks(set, groups)
	want := "cursor/composer-2.5"
	if got.ModelAgent != want || got.ModelAsk != want || got.ModelPlan != want || got.ModelLoop != want {
		t.Errorf("applyCatalogFallbacks() ask/plan/agent/loop = %q %q %q %q, want %q",
			got.ModelAsk, got.ModelPlan, got.ModelAgent, got.ModelLoop, want)
	}
	if got.ModelReview != want {
		t.Errorf("applyCatalogFallbacks() ModelReview = %q, want %q", got.ModelReview, want)
	}
	if got.ModelWorkspaceTitle != want {
		t.Errorf("applyCatalogFallbacks() ModelWorkspaceTitle = %q, want %q (empty title defaults to haiku)", got.ModelWorkspaceTitle, want)
	}
	if !got.ProviderEnabled(domain.ProviderCursor) || got.ProviderEnabled(domain.ProviderClaudeCode) {
		t.Errorf("applyCatalogFallbacks() enablement cursor=%v claude=%v, want cursor on / Claude off",
			got.ProviderEnabled(domain.ProviderCursor), got.ProviderEnabled(domain.ProviderClaudeCode))
	}
}

func TestApplyCatalogFallbacksKeepsEnabledCursorSlot(t *testing.T) {
	set := domain.Settings{
		DisabledProviders: []string{domain.ProviderClaudeCode},
		CursorAPIKey:      "sk-x",
		ModelAgent:        "cursor/grok-4.6",
		ModelAsk:          "claude-code/sonnet",
	}
	groups := buildModelCatalog(set, []cursor.Model{
		{ID: "composer-2.5", DisplayName: "Composer 2.5"},
		{ID: "grok-4.6", DisplayName: "Grok 4.6"},
	}, nil)
	got := applyCatalogFallbacks(set, groups)
	if got.ModelAgent != "cursor/grok-4.6" {
		t.Errorf("applyCatalogFallbacks() ModelAgent = %q, want cursor/grok-4.6 left alone", got.ModelAgent)
	}
	if got.ModelAsk != "cursor/composer-2.5" {
		t.Errorf("applyCatalogFallbacks() ModelAsk = %q, want cursor/composer-2.5", got.ModelAsk)
	}
}

func TestApplyCatalogFallbacksLeavesEmptyReview(t *testing.T) {
	set := domain.Settings{
		DisabledProviders: []string{domain.ProviderClaudeCode},
		CursorAPIKey:      "sk-x",
		ModelAgent:        "claude-code/sonnet",
		ModelLoop:         "claude-code/sonnet",
		ModelReview:       "",
	}
	groups := buildModelCatalog(set, []cursor.Model{{ID: "composer-2.5", DisplayName: "Composer 2.5"}}, nil)
	got := applyCatalogFallbacks(set, groups)
	if got.ModelReview != "" {
		t.Errorf("applyCatalogFallbacks() ModelReview = %q, want empty (inherits Loop)", got.ModelReview)
	}
}

func TestApplyCatalogFallbacksReenablesClaudeWhenCatalogEmpty(t *testing.T) {
	set := domain.Settings{
		DisabledProviders: []string{domain.ProviderClaudeCode},
		CursorAPIKey:      "sk-x",
		ModelAgent:        "claude-code/sonnet",
	}
	got := applyCatalogFallbacks(set, nil)
	if !got.ProviderEnabled(domain.ProviderClaudeCode) {
		t.Fatal("applyCatalogFallbacks() Claude enabled = false, want true when the catalogue is empty")
	}
	if got.ModelAgent != "claude-code/sonnet" {
		t.Errorf("applyCatalogFallbacks() ModelAgent = %q, want claude-code/sonnet", got.ModelAgent)
	}
}

func TestApplyCatalogFallbacksRemovedCompatibleWhenClaudeOff(t *testing.T) {
	beta := domain.CompatibleProvider{
		ID: "beta", Name: "Beta", BaseURL: "https://b.example",
		Models: []domain.CompatibleModel{{ID: "m2", Label: "M2"}},
	}
	set := domain.Settings{
		DisabledProviders:   []string{domain.ProviderClaudeCode, domain.ProviderCursor, domain.ProviderCodex},
		CompatibleProviders: []domain.CompatibleProvider{beta},
		ModelAsk:            "alpha/m1",
		ModelPlan:           "alpha/m1",
		ModelAgent:          "alpha/m1",
		ModelLoop:           "alpha/m1",
		ModelWorkspaceTitle: "alpha/m1",
	}
	if !set.NeedsCatalogFallback() {
		t.Fatal("NeedsCatalogFallback() = false, want true after removing alpha")
	}
	groups := buildModelCatalog(set, nil, nil)
	got := applyCatalogFallbacks(set, groups)
	want := "beta/m2"
	if got.ModelAsk != want || got.ModelPlan != want || got.ModelAgent != want || got.ModelLoop != want {
		t.Errorf("applyCatalogFallbacks() ask/plan/agent/loop = %q %q %q %q, want %q",
			got.ModelAsk, got.ModelPlan, got.ModelAgent, got.ModelLoop, want)
	}
	if got.ModelWorkspaceTitle != want {
		t.Errorf("applyCatalogFallbacks() title = %q, want %q", got.ModelWorkspaceTitle, want)
	}
}

func TestProviderEnablementChanged(t *testing.T) {
	a := domain.Settings{CursorAPIKey: "sk-x"}
	b := a
	if providerEnablementChanged(a, b) {
		t.Fatal("providerEnablementChanged(same) = true, want false")
	}
	b.DisabledProviders = []string{domain.ProviderClaudeCode}
	if !providerEnablementChanged(a, b) {
		t.Fatal("providerEnablementChanged(Claude off) = false, want true")
	}
	b = a
	b.CursorAPIKey = ""
	if !providerEnablementChanged(a, b) {
		t.Fatal("providerEnablementChanged(Cursor key cleared) = false, want true")
	}
	b = a
	b.BypassPermissions = true
	if providerEnablementChanged(a, b) {
		t.Fatal("providerEnablementChanged(unrelated field) = true, want false")
	}
}

func TestSetSettingsUsesCachedCatalog(t *testing.T) {
	svc, _ := newTestServices(t)
	svc.Settings.rememberCatalog([]cursor.Model{{ID: "composer-2.5", DisplayName: "Composer 2.5"}}, nil)

	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	writeOwnedBridge(t, dataDir)

	set := svc.Settings.GetSettings()
	set.CursorAPIKey = "sk-x"
	set.DisabledProviders = []string{domain.ProviderClaudeCode}
	set.ModelAsk = "claude-code/sonnet"
	set.ModelPlan = "claude-code/sonnet"
	set.ModelAgent = "claude-code/sonnet"
	set.ModelLoop = "claude-code/sonnet"
	got := mustSetSettings(t, svc.Settings, set)
	if got.ModelAgent != "cursor/composer-2.5" {
		t.Errorf("SetSettings() ModelAgent = %q, want cursor/composer-2.5 from the cached catalogue", got.ModelAgent)
	}
	if got.ProviderEnabled(domain.ProviderClaudeCode) {
		t.Fatal("SetSettings() left Claude on, want it off when the cached catalogue can remap")
	}
}

func TestCatalogForFallbackUsesCache(t *testing.T) {
	s := &SettingsService{}
	s.rememberCatalog([]cursor.Model{{ID: "composer-2.5", DisplayName: "Composer 2.5"}}, nil)
	set := domain.Settings{
		DisabledProviders: []string{domain.ProviderClaudeCode},
		CursorAPIKey:      "sk-x",
	}
	groups := s.catalogForFallback(set)
	id, _ := firstCatalogChoice(groups)
	if id != "cursor/composer-2.5" {
		t.Errorf("catalogForFallback() first choice = %q, want cursor/composer-2.5 from the cache", id)
	}
}

func TestCatalogForFallbackPrefersCachedCodex(t *testing.T) {
	s := &SettingsService{}
	s.rememberCatalog(nil, []codex.Model{{ID: "gpt-5.6-luna", DisplayName: "GPT-5.6 Luna"}})
	set := domain.Settings{
		DisabledProviders: []string{domain.ProviderClaudeCode},
		CodexAPIKey:       "sk-x",
	}
	groups := s.catalogForFallback(set)
	id, _ := firstCatalogChoice(groups)
	if id != "codex/gpt-5.6-luna" {
		t.Errorf("catalogForFallback() first choice = %q, want codex/gpt-5.6-luna from the cache", id)
	}
}

func TestCatalogForFallbackLiveWhenCacheCold(t *testing.T) {
	t.Setenv(cursor.EnvBridgePath, "")
	s := &SettingsService{}
	set := domain.Settings{
		DisabledProviders: []string{domain.ProviderClaudeCode},
		CursorAPIKey:      "sk-x",
		CursorBridgePath:  filepath.Join(t.TempDir(), "missing-bridge"),
	}
	groups := s.catalogForFallback(set)
	if id, _ := firstCatalogChoice(groups); id != "" {
		t.Errorf("catalogForFallback() first choice = %q, want empty when cache is cold and Locate fails", id)
	}
}

func TestModelCatalogIncludesCompatibleProviders(t *testing.T) {
	set := domain.Settings{
		DisabledProviders: []string{domain.ProviderClaudeCode, domain.ProviderCursor, domain.ProviderCodex},
		CompatibleProviders: []domain.CompatibleProvider{
			{
				ID: "openai", Name: "OpenAI", BaseURL: "https://api.openai.com",
				Models: []domain.CompatibleModel{{
					ID: "gpt-5", Label: "GPT-5",
					Flags: domain.CompatibleFlagPreset("openai"),
				}},
			},
			{
				ID: "groq", Name: "Groq", BaseURL: "https://api.groq.com/openai",
				Models: []domain.CompatibleModel{{ID: "llama-3.1-70b", Label: "Llama"}},
			},
		},
	}
	groups := buildModelCatalog(set, nil, nil)
	if len(groups) != 2 {
		t.Fatalf("buildModelCatalog() groups = %d, want 2 compatible", len(groups))
	}
	if groups[0].Provider != "openai" || groups[0].Label != "OpenAI" {
		t.Errorf("first group = %+v, want openai/OpenAI", groups[0])
	}
	if len(groups[0].Models) != 1 || groups[0].Models[0].Ref != "openai/gpt-5" {
		t.Errorf("openai models = %+v, want openai/gpt-5", groups[0].Models)
	}
	if len(groups[0].Models[0].Flags) != 2 {
		t.Errorf("openai flags = %+v, want effort+verbosity (no Wire)", groups[0].Models[0].Flags)
	}
	if groups[1].Provider != "groq" || groups[1].Models[0].Ref != "groq/llama-3.1-70b" {
		t.Errorf("second group = %+v, want groq", groups[1])
	}

	set.DisabledProviders = append(set.DisabledProviders, "openai")
	groups = buildModelCatalog(set, nil, nil)
	if len(groups) != 1 || groups[0].Provider != "groq" {
		t.Errorf("disabled openai groups = %+v, want only groq", groups)
	}
}

// Models Claude Code reports beyond the pinned list (Mythos on accounts with
// access) join the picker; aliases and pinned ids do not duplicate rows.
func TestProbedClaudeModelsAddAccountOnlyModels(t *testing.T) {
	out := []byte(`{"type":"system","subtype":"init"}
{"type":"control_response","response":{"subtype":"success","request_id":"models","response":{"models":[` +
		`{"value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default (recommended)"},` +
		`{"value":"claude-fable-5","resolvedModel":"claude-fable-5","displayName":"Fable 5"},` +
		`{"value":"claude-mythos-5-1","resolvedModel":"claude-mythos-5-1","displayName":"Mythos 5.1"}]}}}
`)
	models := withProbedClaudeModels(parseClaudeInitModels(out))
	if len(models) != len(claudeCodeModels)+1 {
		t.Fatalf("got %d models, want pinned list + Mythos", len(models))
	}
	last := models[len(models)-1]
	if last.Ref != "claude-code/claude-mythos-5-1" || last.Label != "Mythos 5.1" || len(last.Flags) == 0 || last.ContextWindow != 1_000_000 {
		t.Fatalf("last = %+v, want Mythos 5.1 with effort flags and a 1M window", last)
	}
	if got := withProbedClaudeModels(parseClaudeInitModels([]byte("not json"))); len(got) != len(claudeCodeModels) {
		t.Fatalf("unreadable probe: got %d models, want pinned list", len(got))
	}
}

func TestProbedClaudeModelsStripContextSuffix(t *testing.T) {
	models := withProbedClaudeModels([]claudeModelInfo{
		{Value: "opus[1m]", ResolvedModel: "claude-opus-5-5[1m]", DisplayName: "Opus"},
		{Value: "mythos[1m]", ResolvedModel: "claude-mythos-5-1[1m]", DisplayName: "Mythos 5.1"},
	})
	if len(models) != len(claudeCodeModels)+1 {
		t.Fatalf("got %d models, want pinned list + Mythos", len(models))
	}
	if ref := models[len(models)-1].Ref; ref != "claude-code/claude-mythos-5-1" {
		t.Fatalf("ref = %q, want suffix stripped", ref)
	}
}

func TestClaudeProbeFallsBackToLastGood(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	good := []claudeModelInfo{{ResolvedModel: "claude-mythos-5-1", DisplayName: "Mythos 5.1"}}
	lastClaudeModels.Store(&good)
	t.Cleanup(func() { lastClaudeModels.Store(nil) })
	got := claudeProbe()
	if len(got) != 1 || got[0].ResolvedModel != "claude-mythos-5-1" {
		t.Fatalf("failed probe returned %+v, want last good result", got)
	}
}
