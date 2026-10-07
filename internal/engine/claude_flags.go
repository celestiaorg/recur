package engine

import "strings"

// ResolveClaudeModel applies Claude Code context flags to a bare model id.
// Models with a 200K↔1M toggle get [1m] only when context=1m. Pinned
// always-1M ids always get it: a Claude Code build that predates the id
// assumes a 200K window and auto-compacts at ~167K (claude-fable-5-1 on
// 2.1.252); [1m] makes it use 1M and is a no-op on recognized models.
// Aliases (sonnet/fable/opus) are always recognized and stay bare.
func ResolveClaudeModel(base string, flags map[string]string) string {
	if base == "" {
		return base
	}
	if flags["context"] == "1m" && claudeSelectableContext(base) {
		return base + "[1m]"
	}
	if claudeAlways1M(base) && strings.HasPrefix(claudeBareModelID(base), "claude-") && !strings.HasSuffix(base, "]") {
		return base + "[1m]"
	}
	return base
}

// ResolveClaudeContextWindow returns the active context limit in tokens for the
// meter. Claude's modelUsage.contextWindow is the model ceiling and is not a
// reliable signal of the dock selection for 4.6's 200K↔1M toggle.
//
// Always-1M models (Sonnet 5, Fable 5.1, Opus 4.7+) ignore stale "300k" flags.
// Selectable 4.6 models use 200K unless context=1m.
func ResolveClaudeContextWindow(model string, flags map[string]string) int64 {
	if flags["context"] == "1m" {
		return 1_000_000
	}
	if claudeAlways1M(model) {
		return 1_000_000
	}
	if claudeSelectableContext(model) {
		return 200_000
	}
	// Haiku / unknown — 200K is the standard non-extended window.
	return 200_000
}

// hasClaudeContextFlag is true when a dock context dimension was set (including
// legacy "300k"). Empty means the model has no context picker.
func hasClaudeContextFlag(flags map[string]string) bool {
	_, ok := flags["context"]
	return ok
}

// claudeAlways1M reports models that always run with a 1M window on the
// Anthropic API (no real 200K/300K variant).
func claudeAlways1M(model string) bool {
	id := claudeBareModelID(model)
	switch id {
	case "sonnet", "fable", "opus",
		"claude-sonnet-5", "claude-fable-5-1", "claude-fable-5", "claude-opus-5",
		"claude-opus-4-8", "claude-opus-4-7":
		return true
	}
	// Future pinned Sonnet 5 / Opus 5.x ids.
	if strings.HasPrefix(id, "claude-sonnet-5") ||
		strings.HasPrefix(id, "claude-fable-5") ||
		strings.HasPrefix(id, "claude-opus-5") {
		return true
	}
	return false
}

// claudeSelectableContext is true for models that still expose a 200K↔1M
// toggle via the [1m] suffix.
func claudeSelectableContext(model string) bool {
	id := claudeBareModelID(model)
	return id == "claude-sonnet-4-6" || id == "claude-opus-4-6"
}

func claudeBareModelID(model string) string {
	id := strings.TrimSpace(model)
	if i := strings.IndexByte(id, '/'); i >= 0 {
		id = id[i+1:]
	}
	if i := strings.Index(id, "["); i >= 0 {
		id = id[:i]
	}
	return id
}

// claudeModelLabel turns a Claude model id into a short picker-style name:
// claude-fable-5-1 → Fable 5.1, claude-opus-4-8 → Opus 4.8, fable → Fable.
func claudeModelLabel(model string) string {
	id := strings.TrimPrefix(claudeBareModelID(model), "claude-")
	id = strings.TrimSpace(id)
	if id == "" {
		return ""
	}
	parts := strings.Split(id, "-")
	family := parts[0]
	if family == "" {
		return id
	}
	family = strings.ToUpper(family[:1]) + family[1:]
	if len(parts) == 1 {
		return family
	}
	return family + " " + strings.Join(parts[1:], ".")
}

// ResolveClaudeEffort returns the --effort value from flags. Empty when unset
// so the runner can omit the flag entirely.
func ResolveClaudeEffort(flags map[string]string) string {
	return flags["effort"]
}
