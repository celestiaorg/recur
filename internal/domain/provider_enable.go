package domain

import "strings"

// Fallback models used when a mode still points at a provider the user turned
// off. Claude ids are hardcoded in-repo, so they are safe to substitute.
// Cursor/Codex catalogues are live per account — we never invent those ids.
const (
	fallbackClaudeModel      = "claude-code/claude-sonnet-5-5"
	fallbackClaudeTitleModel = "claude-code/claude-haiku-4-5-20251001"
)

// ProviderEnabled reports whether the provider is on in Settings → Providers.
// Empty slug is Claude Code (same default as ParseModelRef). Empty/nil
// DisabledProviders means every provider is on.
func (s Settings) ProviderEnabled(provider string) bool {
	if provider == "" {
		provider = ProviderClaudeCode
	}
	return !skillNameListed(s.DisabledProviders, provider)
}

// ProviderOffersModels reports whether this provider would appear in the model
// catalogue: it is turned on, and (for Cursor/Codex) credentials are present.
// Claude Code needs no Recur-stored key. A non-empty key does not guarantee a
// live catalogue (the bridge/binary may still be missing); SettingsService
// clamps that at save time.
func (s Settings) ProviderOffersModels(provider string) bool {
	if !s.ProviderEnabled(provider) {
		return false
	}
	switch provider {
	case ProviderClaudeCode, "":
		return true
	case ProviderCursor:
		return s.CursorConfigured()
	case ProviderCodex:
		return s.CodexConfigured()
	case ProviderChatGPT:
		return s.ChatGPTConfigured()
	default:
		p := s.CompatibleByID(provider)
		return p != nil && p.Configured()
	}
}

// NormalizeProviders keeps at least one catalogue provider on and remaps mode
// models that pointed at a provider the user turned off. Unconfigured-but-on
// providers are left alone so clearing a key does not rewrite model picks.
func (s Settings) NormalizeProviders() Settings {
	s = s.NormalizeCompatibleProviders()
	s = s.ensureSomeProviderOffersModels()
	return s.remapDisabledModels()
}

func (s Settings) offersAnyModels() bool {
	if s.ProviderOffersModels(ProviderClaudeCode) ||
		s.ProviderOffersModels(ProviderCursor) ||
		s.ProviderOffersModels(ProviderCodex) || s.ProviderOffersModels(ProviderChatGPT) {
		return true
	}
	for _, p := range s.CompatibleProviders {
		if s.ProviderOffersModels(p.ID) {
			return true
		}
	}
	return false
}

func (s Settings) ensureSomeProviderOffersModels() Settings {
	if s.offersAnyModels() {
		return s
	}
	// Prefer a configured compatible endpoint over forcing Claude back on.
	for _, p := range s.CompatibleProviders {
		s.DisabledProviders = dropString(s.DisabledProviders, p.ID)
		if s.offersAnyModels() {
			return s
		}
	}
	for _, p := range []string{ProviderClaudeCode, ProviderCursor, ProviderCodex, ProviderChatGPT} {
		s.DisabledProviders = dropString(s.DisabledProviders, p)
		if s.offersAnyModels() {
			return s
		}
	}
	return s
}

func (s Settings) remapDisabledModels() Settings {
	s.ModelAsk, s.FlagsAsk = s.RemapDisabledModel(s.ModelAsk, s.FlagsAsk, false)
	s.ModelPlan, s.FlagsPlan = s.RemapDisabledModel(s.ModelPlan, s.FlagsPlan, false)
	s.ModelAgent, s.FlagsAgent = s.RemapDisabledModel(s.ModelAgent, s.FlagsAgent, false)
	s.ModelLoop, s.FlagsLoop = s.RemapDisabledModel(s.ModelLoop, s.FlagsLoop, false)
	// Empty Review inherits Loop — do not fill it in just because Claude is off.
	if strings.TrimSpace(s.ModelReview) != "" {
		s.ModelReview, s.FlagsReview = s.RemapDisabledModel(s.ModelReview, s.FlagsReview, false)
	}
	s.ModelWorkspaceTitle, s.FlagsWorkspaceTitle = s.RemapDisabledModel(s.ModelWorkspaceTitle, s.FlagsWorkspaceTitle, true)
	return s
}

// RemapDisabledModel substitutes a Claude default when ref's provider is turned
// off and Claude is still on. Otherwise the ref is unchanged — including when
// the provider is merely unconfigured (so credential edits cannot destroy picks)
// and when Claude itself is off. Cursor/Codex catalogues are live; domain never
// invents those ids. Callers in app remap from the catalogue in that case.
func (s Settings) RemapDisabledModel(ref string, flags map[string]string, title bool) (string, map[string]string) {
	if !s.modelSlotDisabled(ref, title) {
		return ref, flags
	}
	if !s.ProviderEnabled(ProviderClaudeCode) {
		return ref, flags
	}
	if title {
		return fallbackClaudeTitleModel, map[string]string{"effort": "low"}
	}
	return fallbackClaudeModel, map[string]string{"effort": "high"}
}

// NeedsCatalogFallback is true when a Settings slot still names a turned-off
// provider. Empty Ask/Plan/Agent/Loop and the workspace-title slot default to
// Claude, so they count when Claude is off. Empty Review inherits Loop and is
// not filled on its own.
func (s Settings) NeedsCatalogFallback() bool {
	if s.modelSlotDisabled(s.ModelAsk, true) {
		return true
	}
	if s.modelSlotDisabled(s.ModelPlan, true) {
		return true
	}
	if s.modelSlotDisabled(s.ModelAgent, true) {
		return true
	}
	if s.modelSlotDisabled(s.ModelLoop, true) {
		return true
	}
	if strings.TrimSpace(s.ModelReview) != "" && s.modelSlotDisabled(s.ModelReview, false) {
		return true
	}
	return s.modelSlotDisabled(s.ModelWorkspaceTitle, true)
}

// ModelRefUnavailable reports whether ref resolves to a turned-off provider
// or a compatible slug that is no longer in Settings. Empty slots with
// emptyMeansClaude false inherit another slot (not unavailable on their own).
// Empty slots with emptyMeansClaude true default to Claude.
func (s Settings) ModelRefUnavailable(ref string, emptyMeansClaude bool) bool {
	return s.modelSlotDisabled(ref, emptyMeansClaude)
}

// modelSlotDisabled reports whether ref resolves to a turned-off provider.
// Empty slots with emptyMeansClaude false inherit another slot (not disabled
// on their own). Empty slots with emptyMeansClaude true default to Claude.
func (s Settings) modelSlotDisabled(ref string, emptyMeansClaude bool) bool {
	trimmed := strings.TrimSpace(ref)
	if trimmed == "" && !emptyMeansClaude {
		return false
	}
	p := ParseModelRef(trimmed).Provider
	if !s.ProviderEnabled(p) {
		return true
	}
	switch p {
	case ProviderClaudeCode, ProviderCursor, ProviderCodex, ProviderChatGPT, "":
		return false
	default:
		// Removed compatible slugs are unavailable even if not listed in DisabledProviders.
		return s.CompatibleByID(p) == nil
	}
}

func dropString(list []string, name string) []string {
	if !skillNameListed(list, name) {
		return list
	}
	out := make([]string, 0, len(list)-1)
	for _, d := range list {
		if d != name {
			out = append(out, d)
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}
