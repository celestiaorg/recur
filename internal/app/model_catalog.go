package app

import (
	"strings"
	"unicode"
	"unicode/utf8"

	"recur/internal/codex"
	"recur/internal/cursor"
	"recur/internal/domain"
)

// The model catalogue backs every model picker in the app. It lists all
// enabled, configured providers in one grouped list, so choosing a model is
// what selects the provider for that role. Settings → Providers can turn a
// provider off without clearing credentials; disabled providers are omitted
// here so pickers cannot point a role at them.
//
// Each ModelChoice may advertise FlagDimensions (effort, context, fast, …). The
// Settings UI shows those as a second T3-style picker; each provider resolves
// selected flags into whatever it needs at run time.

// FlagOption is one selectable value inside a FlagDimension.
type FlagOption struct {
	Value   string `json:"value"`
	Label   string `json:"label"`
	Default bool   `json:"default,omitempty"`
}

// FlagDimension is one configurable axis on a model (Effort, Context, Fast, …).
type FlagDimension struct {
	ID      string       `json:"id"`
	Label   string       `json:"label"`
	Options []FlagOption `json:"options"`
}

// ModelChoice is one selectable model.
type ModelChoice struct {
	// Ref is the fully-qualified value to store, e.g. "cursor/composer-2.5".
	Ref string `json:"ref"`
	// Label is the display name, e.g. "Composer 2.5".
	Label string `json:"label"`
	// Flags are the per-model dimensions the second picker shows. Empty → hide it.
	Flags []FlagDimension `json:"flags,omitempty"`
	// ContextWindow is the model's default context limit in tokens (Cursor:
	// hardcoded catalogue; Claude: omitted — use the context flag instead).
	ContextWindow int64 `json:"contextWindow,omitempty"`
}

// ModelGroup is one provider's section of the picker.
type ModelGroup struct {
	// Provider is the slug, e.g. "cursor".
	Provider string `json:"provider"`
	// Label is the provider's display name, e.g. "Cursor".
	Label string `json:"label"`
	// Models are the provider's selectable models, in display order.
	Models []ModelChoice `json:"models"`
}

// Claude Code flag dimensions — hardcoded because Claude Code has no catalogue API.
var (
	claudeEffort = FlagDimension{
		ID:    "effort",
		Label: "Effort",
		Options: []FlagOption{
			{Value: "low", Label: "Low"},
			{Value: "medium", Label: "Medium"},
			{Value: "high", Label: "High", Default: true},
			{Value: "xhigh", Label: "Extra High"},
			{Value: "max", Label: "Max"},
		},
	}
	// claudeContext is only for models that still expose a real 200K↔1M toggle
	// via the [1m] model suffix (Sonnet/Opus 4.6). Always-1M models omit it.
	claudeContext = FlagDimension{
		ID:    "context",
		Label: "Context",
		Options: []FlagOption{
			{Value: "200k", Label: "200K", Default: true},
			{Value: "1m", Label: "1M"},
		},
	}
	// gpt56Context is opt-in long context for GPT-5.6 / GPT-6. Default stays
	// the 272K window; prompts above that are billed at 2× input / 1.5× output.
	// simplify: Astra's API ceiling is 1.05M; Recur's 1M flag matches GPT-5.6.
	gpt56Context = FlagDimension{
		ID:    "context",
		Label: "Context",
		Options: []FlagOption{
			{Value: "272k", Label: "272K", Default: true},
			{Value: "1m", Label: "1M"},
		},
	}
)

func gpt6Family(id string) bool {
	n := strings.ReplaceAll(strings.ToLower(strings.TrimSpace(id)), ".", "-")
	return strings.HasPrefix(n, "gpt-6")
}

func gpt56Family(id string) bool {
	n := strings.ReplaceAll(strings.ToLower(strings.TrimSpace(id)), ".", "-")
	return strings.Contains(n, "gpt-5-6") || gpt6Family(id)
}

func showCodexModel(m codex.Model) bool {
	return strings.TrimSpace(m.ID) != "" && !m.Hidden
}

func claudeFlags(withContext bool) []FlagDimension {
	if withContext {
		return []FlagDimension{claudeEffort, claudeContext}
	}
	return []FlagDimension{claudeEffort}
}

// claudeCodeModels is the Claude Code picker list. Every row is a pinned id,
// never an alias, so a saved choice keeps meaning the same model. Extended context is a flag (200k / 1m) only where
// Claude Code still distinguishes them — ResolveClaude applies [1m] at run time.
// Sonnet 5 / Fable 5.1 / Opus 4.7+ always run at 1M on the Anthropic API.
var claudeCodeModels = []ModelChoice{
	{Ref: "claude-code/claude-fable-5-1", Label: "Fable 5.1", Flags: claudeFlags(false), ContextWindow: 1_000_000},
	{Ref: "claude-code/claude-fable-5", Label: "Fable 5", Flags: claudeFlags(false), ContextWindow: 1_000_000},
	{Ref: "claude-code/claude-opus-5-5", Label: "Opus 5.5", Flags: claudeFlags(false), ContextWindow: 1_000_000},
	{Ref: "claude-code/claude-opus-5", Label: "Opus 5", Flags: claudeFlags(false), ContextWindow: 1_000_000},
	{Ref: "claude-code/claude-opus-4-8", Label: "Opus 4.8", Flags: claudeFlags(false), ContextWindow: 1_000_000},
	{Ref: "claude-code/claude-opus-4-7", Label: "Opus 4.7", Flags: claudeFlags(false), ContextWindow: 1_000_000},
	{Ref: "claude-code/claude-opus-4-6", Label: "Opus 4.6", Flags: claudeFlags(true)},
	{Ref: "claude-code/claude-sonnet-5-5", Label: "Sonnet 5.5", Flags: claudeFlags(false), ContextWindow: 1_000_000},
	{Ref: "claude-code/claude-sonnet-5", Label: "Sonnet 5", Flags: claudeFlags(false), ContextWindow: 1_000_000},
	{Ref: "claude-code/claude-sonnet-4-6", Label: "Sonnet 4.6", Flags: claudeFlags(true)},
	{Ref: "claude-code/claude-haiku-4-5-20251001", Label: "Haiku 4.5", Flags: claudeFlags(false), ContextWindow: 200_000},
}

// ListModelCatalog returns the grouped model list for the pickers.
//
// Each provider appears only when it is on in Settings → Providers. Claude Code
// needs no Recur-stored key. Cursor appears once Connect has minted a key, and
// its models (+ parameters) are fetched live. Codex appears once an API key or
// ChatGPT auth is set.
//
// Fetching Cursor's catalogue starts a bridge, which is slow on a cold start, so
// callers should load this once per pane rather than per picker. The result is
// remembered so SetSettings can remap Claude-off slots without starting another
// throwaway bridge (boot prefetchModelCatalog warms this).
func (s *SettingsService) ListModelCatalog() []ModelGroup {
	set := s.st().Settings()
	cur := s.listCursorModelsFor(set)
	cod := s.listCodexModelsFor(set)
	s.rememberCatalog(cur, cod)
	return buildModelCatalog(set, cur, cod)
}

func (s *SettingsService) rememberCatalog(cur []cursor.Model, cod []codex.Model) {
	if s == nil {
		return
	}
	s.catalogMu.Lock()
	defer s.catalogMu.Unlock()
	if cur != nil {
		s.lastCursor = cur
	}
	if cod != nil {
		s.lastCodex = cod
	}
}

func (s *SettingsService) cachedCatalogModels() ([]cursor.Model, []codex.Model) {
	if s == nil {
		return nil, nil
	}
	s.catalogMu.Lock()
	defer s.catalogMu.Unlock()
	return s.lastCursor, s.lastCodex
}

// catalogForFallback returns the grouped catalogue used to remap slots off a
// turned-off provider. Prefers the last ListModelCatalog result so a Settings
// toggle does not start a throwaway bridge. When that cache has nothing the
// incoming settings can use (first-time Cursor/Codex setup, Models pane never
// visited), it falls back to a live probe and remembers the result.
func (s *SettingsService) catalogForFallback(set domain.Settings) []ModelGroup {
	cur, cod := s.cachedCatalogModels()
	groups := buildModelCatalog(set, cur, cod)
	if id, _ := firstCatalogChoice(groups); id != "" {
		return groups
	}
	// Probe one provider at a time so a hung Cursor bridge cannot also burn
	// the Codex timeout before SetSettings returns.
	cur = s.listCursorModelsFor(set)
	s.rememberCatalog(cur, nil)
	groups = buildModelCatalog(set, cur, cod)
	if id, _ := firstCatalogChoice(groups); id != "" {
		return groups
	}
	cod = s.listCodexModelsFor(set)
	s.rememberCatalog(nil, cod)
	return buildModelCatalog(set, cur, cod)
}

// applyCatalogFallbacks repoints Settings slots that still name a turned-off
// provider to the first catalogue model (with that model's default flags).
// Domain cannot invent Cursor/Codex IDs; this is the app-layer remap used when
// the user turns Claude Code off with Cursor or Codex still on.
//
// If the catalogue has nothing to pick and a slot still points at a disabled
// provider, Claude is turned back on so a turn can still run.
func applyCatalogFallbacks(set domain.Settings, groups []ModelGroup) domain.Settings {
	if !set.NeedsCatalogFallback() {
		return set
	}
	id, flags := firstCatalogChoice(groups)
	if id == "" {
		set.DisabledProviders = stripDisabledProvider(set.DisabledProviders, domain.ProviderClaudeCode)
		return set.NormalizeProviders()
	}
	apply := func(ref string, fl map[string]string, emptyMeansClaude bool) (string, map[string]string) {
		if !slotNeedsFallback(set, ref, emptyMeansClaude) {
			return ref, fl
		}
		return id, cloneStringMap(flags)
	}
	set.ModelAsk, set.FlagsAsk = apply(set.ModelAsk, set.FlagsAsk, true)
	set.ModelPlan, set.FlagsPlan = apply(set.ModelPlan, set.FlagsPlan, true)
	set.ModelAgent, set.FlagsAgent = apply(set.ModelAgent, set.FlagsAgent, true)
	set.ModelLoop, set.FlagsLoop = apply(set.ModelLoop, set.FlagsLoop, true)
	if strings.TrimSpace(set.ModelReview) != "" {
		set.ModelReview, set.FlagsReview = apply(set.ModelReview, set.FlagsReview, false)
	}
	set.ModelWorkspaceTitle, set.FlagsWorkspaceTitle = apply(set.ModelWorkspaceTitle, set.FlagsWorkspaceTitle, true)
	return set
}

func slotNeedsFallback(set domain.Settings, ref string, emptyMeansClaude bool) bool {
	return set.ModelRefUnavailable(ref, emptyMeansClaude)
}

func firstCatalogChoice(groups []ModelGroup) (string, map[string]string) {
	for _, g := range groups {
		for _, m := range g.Models {
			if strings.TrimSpace(m.Ref) == "" {
				continue
			}
			return m.Ref, defaultFlagsFromDims(m.Flags)
		}
	}
	return "", nil
}

func defaultFlagsFromDims(dims []FlagDimension) map[string]string {
	if len(dims) == 0 {
		return nil
	}
	out := make(map[string]string, len(dims))
	for _, d := range dims {
		def := ""
		for _, o := range d.Options {
			if o.Default {
				def = o.Value
				break
			}
		}
		if def == "" && len(d.Options) > 0 {
			def = d.Options[0].Value
		}
		if def != "" {
			out[d.ID] = def
		}
	}
	return out
}

func stripDisabledProvider(list []string, p string) []string {
	var out []string
	for _, x := range list {
		if x == p {
			continue
		}
		out = append(out, x)
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

func buildModelCatalog(set domain.Settings, cursorModels []cursor.Model, codexModels []codex.Model) []ModelGroup {
	var groups []ModelGroup
	if set.ProviderEnabled(domain.ProviderClaudeCode) {
		groups = append(groups, ModelGroup{
			Provider: domain.ProviderClaudeCode,
			Label:    domain.ProviderLabel(domain.ProviderClaudeCode),
			Models:   claudeCodeModels,
		})
	}
	if set.ProviderEnabled(domain.ProviderCursor) {
		if models := cursorCatalogModels(cursorModels); len(models) > 0 {
			groups = append(groups, ModelGroup{
				Provider: domain.ProviderCursor,
				Label:    domain.ProviderLabel(domain.ProviderCursor),
				Models:   models,
			})
		}
	}
	if set.ProviderEnabled(domain.ProviderCodex) {
		if models := codexCatalogModels(codexModels); len(models) > 0 {
			groups = append(groups, ModelGroup{
				Provider: domain.ProviderCodex,
				Label:    domain.ProviderLabel(domain.ProviderCodex),
				Models:   models,
			})
		}
	}
	for _, p := range set.CompatibleProviders {
		if !set.ProviderEnabled(p.ID) || !p.Configured() {
			continue
		}
		groups = append(groups, ModelGroup{
			Provider: p.ID,
			Label:    set.DisplayLabel(p.ID),
			Models:   compatibleCatalogModels(p),
		})
	}
	if set.ProviderOffersModels(domain.ProviderChatGPT) {
		p := set.ChatGPTProvider()
		groups = append(groups, ModelGroup{Provider: p.ID, Label: p.Name, Models: compatibleCatalogModels(p)})
	}
	return groups
}

func compatibleCatalogModels(p domain.CompatibleProvider) []ModelChoice {
	out := make([]ModelChoice, 0, len(p.Models))
	for _, m := range p.Models {
		if m.ID == "" {
			continue
		}
		label := m.Label
		if label == "" {
			label = m.ID
		}
		out = append(out, ModelChoice{
			Ref:           domain.NewModelRef(p.ID, m.ID).String(),
			Label:         label,
			Flags:         compatibleFlagDims(m.Flags),
			ContextWindow: m.ContextWindow,
		})
	}
	return out
}

func compatibleFlagDims(in []domain.CompatibleFlagDim) []FlagDimension {
	out := make([]FlagDimension, 0, len(in))
	for _, d := range in {
		opts := make([]FlagOption, 0, len(d.Options))
		for _, o := range d.Options {
			opts = append(opts, FlagOption{Value: o.Value, Label: o.Label, Default: o.Default})
		}
		out = append(out, FlagDimension{ID: d.ID, Label: d.Label, Options: opts})
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

func codexCatalogModels(in []codex.Model) []ModelChoice {
	out := make([]ModelChoice, 0, len(in))
	for _, m := range in {
		if !showCodexModel(m) {
			continue
		}
		out = append(out, ModelChoice{
			Ref:   domain.NewModelRef(domain.ProviderCodex, m.ID).String(),
			Label: friendlyCodexModelLabel(m.DisplayName, m.ID),
			Flags: codexModelFlags(m),
		})
	}
	return out
}

// codexModelFlags builds effort + optional context / speed-tier dimensions.
func codexModelFlags(m codex.Model) []FlagDimension {
	var out []FlagDimension
	out = append(out, codexEffortFlags(m)...)
	if gpt56Family(m.ID) {
		out = append(out, gpt56Context)
	}
	if speed := codexSpeedFlags(m); speed != nil {
		out = append(out, *speed)
	}
	return out
}

// friendlyCodexModelLabel turns API display names / slug ids into picker labels
// in the same register as Claude's ("Sonnet 5"): "GPT-5.6-Sol" / "gpt-5.6-sol"
// → "GPT-5.6 Sol".
func friendlyCodexModelLabel(displayName, id string) string {
	if s := strings.TrimSpace(displayName); s != "" {
		return spaceCodexCodename(s)
	}
	return spaceCodexCodename(prettyCodexSlug(id))
}

// spaceCodexCodename swaps the final hyphen before a letter-only codename for a
// space ("GPT-5.6-Sol" → "GPT-5.6 Sol"). Version hyphens stay put.
func spaceCodexCodename(s string) string {
	i := strings.LastIndex(s, "-")
	if i <= 0 {
		return s
	}
	tail := s[i+1:]
	if tail == "" || strings.ContainsAny(tail, "0123456789.") {
		return s
	}
	return s[:i] + " " + upperFirst(tail)
}

func prettyCodexSlug(id string) string {
	id = strings.TrimSpace(id)
	if id == "" {
		return ""
	}
	if len(id) >= 4 && strings.EqualFold(id[:4], "gpt-") {
		return "GPT-" + id[4:]
	}
	return upperFirst(id)
}

func upperFirst(s string) string {
	if s == "" {
		return ""
	}
	r, size := utf8.DecodeRuneInString(s)
	return string(unicode.ToUpper(r)) + s[size:]
}

func codexEffortFlags(m codex.Model) []FlagDimension {
	if len(m.SupportedReasoningEffort) == 0 {
		return nil
	}
	opts := make([]FlagOption, 0, len(m.SupportedReasoningEffort))
	for _, e := range m.SupportedReasoningEffort {
		opts = append(opts, FlagOption{
			Value:   e.ReasoningEffort,
			Label:   friendlyCodexEffortLabel(e.ReasoningEffort),
			Default: e.ReasoningEffort == m.DefaultReasoningEffort,
		})
	}
	return []FlagDimension{{ID: "effort", Label: "Effort", Options: opts}}
}

// friendlyCodexEffortLabel mirrors Claude's effort chips (Low / Medium / …).
func friendlyCodexEffortLabel(effort string) string {
	switch strings.ToLower(strings.TrimSpace(effort)) {
	case "low":
		return "Low"
	case "medium":
		return "Medium"
	case "high":
		return "High"
	case "xhigh":
		return "Extra High"
	case "max":
		return "Max"
	case "ultra":
		return "Ultra"
	case "":
		return ""
	default:
		return upperFirst(effort)
	}
}

// codexSpeedFlags exposes model/list serviceTiers as a Fast
// On/Off flag. Models with no boost tier omit it. The On value is the real
// tier id from model/list so turn/start never invents an unsupported id.
func codexSpeedFlags(m codex.Model) *FlagDimension {
	boost := codexBoostTierID(m)
	if boost == "" {
		return nil
	}
	def := strings.TrimSpace(m.DefaultServiceTier)
	onByDefault := def != "" && strings.EqualFold(def, boost)
	return &FlagDimension{
		ID:    "fast",
		Label: "Fast",
		Options: []FlagOption{
			{Value: boost, Label: "On", Default: onByDefault},
			{Value: "false", Label: "Off", Default: !onByDefault},
		},
	}
}

// codexBoostTierID picks the Fast/priority-style tier id from model/list.
// Empty when the model only advertises standard/default (or nothing).
func codexBoostTierID(m codex.Model) string {
	var fallback string
	for _, t := range m.ServiceTiers {
		id := strings.TrimSpace(t.ID)
		if id == "" {
			continue
		}
		switch strings.ToLower(id) {
		case "standard", "default":
			continue
		case "priority", "fast":
			return id
		default:
			if fallback == "" {
				fallback = id
			}
		}
	}
	return fallback
}

// cursorCatalogModels maps live ListModels entries into catalog rows.
// Drops id "default" — Cursor advertises it as a second "Auto" (account/team
// default); Recur always picks an explicit model. House models (Composer, Auto,
// Grok) sort first; the rest follow by lab. One Cursor group in the picker.
func cursorCatalogModels(in []cursor.Model) []ModelChoice {
	out := make([]ModelChoice, 0, len(in))
	for _, m := range in {
		if m.ID == "default" {
			continue
		}
		out = append(out, cursorModelChoice(m))
	}
	return orderCursorModels(out)
}

func orderCursorModels(in []ModelChoice) []ModelChoice {
	if len(in) < 2 {
		return in
	}
	buckets := make(map[string][]ModelChoice, len(cursor.VendorOrder))
	for _, m := range in {
		v := cursor.Vendor(domain.ParseModelRef(m.Ref).ID)
		buckets[v] = append(buckets[v], m)
	}
	out := make([]ModelChoice, 0, len(in))
	for _, v := range cursor.VendorOrder {
		out = append(out, buckets[v]...)
	}
	return out
}

// cursorModelChoice maps a live ListModels entry into a catalog row. Parameters
// become FlagDimensions; the default variant (if any) marks Default options.
// ContextWindow comes from Recur's hardcoded Cursor table (ListModels does not
// advertise limits).
func cursorModelChoice(m cursor.Model) ModelChoice {
	label := m.DisplayName
	if label == "" {
		label = m.ID
	}
	return ModelChoice{
		Ref:           domain.NewModelRef(domain.ProviderCursor, m.ID).String(),
		Label:         label,
		Flags:         cursorParametersToFlags(m.Parameters, m.Variants),
		ContextWindow: cursor.BaseContextWindow(m.ID),
	}
}

func cursorParametersToFlags(params []cursor.ModelParameterDefinition, variants []cursor.ModelVariant) []FlagDimension {
	if len(params) == 0 {
		return nil
	}
	defaults := map[string]string{}
	for _, v := range variants {
		if !v.IsDefault {
			continue
		}
		for _, p := range v.Params {
			defaults[p.ID] = p.Value
		}
		break
	}

	out := make([]FlagDimension, 0, len(params))
	for _, p := range params {
		if len(p.Values) == 0 {
			continue
		}
		label := p.DisplayName
		if label == "" {
			label = p.ID
		}
		defVal, hasDef := defaults[p.ID]
		opts := make([]FlagOption, 0, len(p.Values))
		for i, v := range p.Values {
			ol := v.DisplayName
			if ol == "" {
				ol = v.Value
			}
			isDef := false
			if hasDef {
				isDef = v.Value == defVal
			} else if i == 0 {
				// SDK: omitted params use the first allowed value.
				isDef = true
			}
			opts = append(opts, FlagOption{Value: v.Value, Label: ol, Default: isDef})
		}
		opts = normalizeBoolFlagOptions(opts, hasDef, defVal)
		out = append(out, FlagDimension{ID: p.ID, Label: label, Options: opts})
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// normalizeBoolFlagOptions rewrites true/false Cursor params (fast, thinking, …)
// as On/Off with On first. Wire values stay "true"/"false".
func normalizeBoolFlagOptions(opts []FlagOption, hasDef bool, defVal string) []FlagOption {
	if len(opts) != 2 {
		return opts
	}
	var on, off *FlagOption
	for i := range opts {
		switch opts[i].Value {
		case "true":
			on = &opts[i]
		case "false":
			off = &opts[i]
		}
	}
	if on == nil || off == nil {
		return opts
	}
	onOpt := FlagOption{Value: "true", Label: "On"}
	offOpt := FlagOption{Value: "false", Label: "Off"}
	if hasDef {
		onOpt.Default = defVal == "true"
		offOpt.Default = defVal == "false"
	} else {
		// Prefer On first; keep SDK-first-value default semantics on the
		// original first option's value.
		first := opts[0].Value
		onOpt.Default = first == "true"
		offOpt.Default = first == "false"
	}
	return []FlagOption{onOpt, offOpt}
}
