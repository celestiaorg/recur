package domain

import "strings"

// Provider slugs live in session.go (the transcript store's provider registry);
// the SAME slug prefixes a ModelRef, so a session's provider and the model that
// produced it are guaranteed to agree.

// ProviderLabel is the human name for a provider slug.
func ProviderLabel(provider string) string {
	switch provider {
	case ProviderCursor:
		return "Cursor"
	case ProviderCodex:
		return "Codex"
	case ProviderChatGPT:
		return "ChatGPT"
	case ProviderClaudeCode:
		return "Claude Code"
	default:
		return provider
	}
}

// BuiltinProvider reports whether slug is a Recur-hosted CLI/SDK harness
// (Claude Code, Cursor, Codex) rather than a user-defined compatible endpoint.
func BuiltinProvider(provider string) bool {
	switch provider {
	case ProviderClaudeCode, ProviderCursor, ProviderCodex, "":
		return true
	default:
		return false
	}
}

// ModelRef is a fully-qualified model selection: which provider runs the turn,
// and which of that provider's models it uses. It is stored as `provider/id`,
// e.g. `claude-code/opus` or `cursor/composer-2.5`.
//
// Qualifying the model this way means there is exactly ONE source of truth for
// which provider runs a given role. Picking a model IS picking a provider.
// Recur's picker lists every enabled, configured provider's models in one
// grouped list.
//
// Model ids MAY contain "/": ParseModelRef splits on the first slash, so
// `openrouter/openai/gpt-4o` is provider `openrouter`, id `openai/gpt-4o`.
type ModelRef struct {
	Provider string
	ID       string
}

// ParseModelRef splits a stored model ref. An unqualified value is Claude Code,
// which is the default provider — every install has it, and Cursor requires
// explicit configuration before it can appear in a picker at all.
//
// An empty value yields an empty ID, which means "let the provider choose its
// own default model".
func ParseModelRef(s string) ModelRef {
	s = strings.TrimSpace(s)
	if s == "" {
		return ModelRef{Provider: ProviderClaudeCode}
	}
	provider, id, ok := strings.Cut(s, "/")
	if !ok {
		return ModelRef{Provider: ProviderClaudeCode, ID: s}
	}
	return ModelRef{Provider: provider, ID: id}
}

// String renders the ref in storage form.
func (m ModelRef) String() string {
	if m.ID == "" {
		return ""
	}
	return m.Provider + "/" + m.ID
}

// IsZero reports whether no model was selected.
func (m ModelRef) IsZero() bool { return m.ID == "" }

// NewModelRef builds a ref, defaulting an empty provider to Claude Code.
func NewModelRef(provider, id string) ModelRef {
	if provider == "" {
		provider = ProviderClaudeCode
	}
	return ModelRef{Provider: provider, ID: id}
}
