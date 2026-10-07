package cursor

import "strings"

// ContextLimits is the known working-window size for a Cursor model id.
// Base is the default; Max is the expanded window when the dock selects a
// larger context flag / Max Mode (0 means the model does not expand).
type ContextLimits struct {
	Base int64
	Max  int64
}

// contextWindows maps normalized model-id prefixes → token limits.
// Longest prefix wins. IDs are matched after lowercasing and mapping '.' → '-'.
//
// Sources (Cursor docs + SpaceXAI model card, Aug 2026):
//
//	composer-2.5     — fixed; docs omit size, Cursor/community treat as 200k
//	grok-4.7         — 500k (Cursor docs: long context up to 500k; Vercel 500k)
//	grok-4.6         — 500k (SpaceXAI / Vercel; Cursor docs omit size)
//	grok-4.5         — fixed 256k (product; docs omit)
//	claude-sonnet-5  — up to 1M; long-context pricing threshold 200k
//	claude-opus-5    — up to 1M; long-context pricing threshold 300k
//	claude-fable-5   — 1M on API; Max Mode on Cursor → 200k / 1M
//	claude-fable-5-1 — same family window as Fable 5
//	gemini-3.1-pro   — up to 1M
//	gemini-3.6-flash — 1M context window
//	gpt-5.6-*        — ListModels context=272k|1m (Sol/Terra/Luna)
//	gpt-5.5 / 5.4    — same picker. Mini/nano and older GPT have none.
//
// Older / unlisted families keep conservative prefixes below.
var contextWindows = map[string]ContextLimits{
	// Cursor-native (ids are ListModels wire ids; provider prefix already stripped)
	"composer": {Base: 200_000},
	"grok-4-7": {Base: 500_000},
	"grok-4-6": {Base: 500_000},
	"grok":     {Base: 256_000},

	// Anthropic (specific first — longest prefix wins)
	"claude-sonnet-5": {Base: 200_000, Max: 1_000_000},
	"claude-opus-5":   {Base: 300_000, Max: 1_000_000},
	"claude-fable-5":  {Base: 200_000, Max: 1_000_000},
	"claude-sonnet":   {Base: 200_000, Max: 1_000_000},
	"claude-opus":     {Base: 200_000, Max: 1_000_000},
	"claude-fable":    {Base: 200_000, Max: 1_000_000},
	"claude-haiku":    {Base: 200_000},

	// OpenAI GPT. ListModels (Sep 2026): 5.6/5.5/5.4 expose 272k↔1M; others don't.
	"gpt-5-6":      {Base: 272_000, Max: 1_000_000},
	"gpt-5-5":      {Base: 272_000, Max: 1_000_000},
	"gpt-5-4-mini": {Base: 272_000},
	"gpt-5-4-nano": {Base: 272_000},
	"gpt-5-4":      {Base: 272_000, Max: 1_000_000},
	"gpt-5-3":      {Base: 272_000},
	"gpt-5-2":      {Base: 272_000},
	"gpt-5-1":      {Base: 272_000},
	"gpt-5":        {Base: 272_000},

	// Google Gemini
	"gemini-3-6": {Base: 1_000_000},
	"gemini-3-1": {Base: 1_000_000},
	"gemini-3-5": {Base: 1_000_000},
	"gemini-3":   {Base: 1_000_000},
	"gemini-2-5": {Base: 1_000_000},
	"gemini":     {Base: 1_000_000},

	// Others
	"kimi-k3": {Base: 200_000, Max: 1_000_000},
	"kimi":    {Base: 200_000, Max: 1_000_000},
	"glm":     {Base: 200_000},
	"auto":    {Base: 200_000},
}

// defaultContextWindow is used when no prefix matches.
const defaultContextWindow int64 = 200_000

func normalizeModelID(modelID string) string {
	return strings.ReplaceAll(strings.ToLower(strings.TrimSpace(modelID)), ".", "-")
}

// LimitsFor returns the hardcoded context limits for a bare Cursor model id
// (e.g. "composer-2.5", "claude-opus-5", "gpt-5.6-sol"). Unknown ids get 200k.
func LimitsFor(modelID string) ContextLimits {
	m := normalizeModelID(modelID)
	if m == "" {
		return ContextLimits{Base: defaultContextWindow}
	}
	best := ""
	for prefix := range contextWindows {
		if strings.HasPrefix(m, prefix) && len(prefix) > len(best) {
			best = prefix
		}
	}
	if best == "" {
		return ContextLimits{Base: defaultContextWindow}
	}
	return contextWindows[best]
}

// BaseContextWindow is the default (non-expanded) window for a model id.
func BaseContextWindow(modelID string) int64 {
	lim := LimitsFor(modelID)
	if lim.Base > 0 {
		return lim.Base
	}
	return defaultContextWindow
}

// Vendor ranks a Cursor model id for picker order: house models
// (Composer / Auto / Grok) first, then the lab that trained the rest.
func Vendor(modelID string) string {
	n := normalizeModelID(modelID)
	switch {
	case strings.HasPrefix(n, "composer"), strings.HasPrefix(n, "auto"), strings.HasPrefix(n, "grok"):
		return "Cursor"
	case strings.HasPrefix(n, "claude"):
		return "Anthropic"
	case strings.HasPrefix(n, "gpt"), strings.HasPrefix(n, "o1"), strings.HasPrefix(n, "o3"), strings.HasPrefix(n, "o4"):
		return "OpenAI"
	case strings.HasPrefix(n, "gemini"):
		return "Google"
	case strings.HasPrefix(n, "kimi"):
		return "Moonshot"
	case strings.HasPrefix(n, "glm"):
		return "Zhipu"
	default:
		return "Other"
	}
}

// VendorOrder is picker order inside the Cursor group (house models first).
var VendorOrder = []string{"Cursor", "Anthropic", "OpenAI", "Google", "Moonshot", "Zhipu", "Other"}
