package domain

import (
	"fmt"
	"net/url"
	"regexp"
	"strings"
)

// Compatible provider slugs that collide with Recur's built-in harnesses or
// reserved session identifiers. "openai" is allowed — users add it as a
// compatible endpoint.
var reservedProviderSlugs = map[string]bool{
	ProviderClaudeCode: true,
	ProviderCursor:     true,
	ProviderCodex:      true,
	ProviderChatGPT:    true,
	ProviderAnthropic:  true,
	ProviderGemini:     true,
	ProviderOllama:     true,
}

var compatibleSlugRE = regexp.MustCompile(`^[a-z][a-z0-9-]{0,47}$`)

// Allowlisted JSON paths a CompatibleFlagDim.Wire may set on the Responses body.
var compatibleFlagWires = map[string]bool{
	"reasoning.effort":  true,
	"reasoning.summary": true,
	"text.verbosity":    true,
	"temperature":       true,
	"max_output_tokens": true,
}

// CompatibleProvider is one user-defined OpenAI-compatible LLM endpoint.
type CompatibleProvider struct {
	ID           string            `json:"id"`
	Name         string            `json:"name"`
	BaseURL      string            `json:"baseUrl"`
	APIKey       string            `json:"apiKey,omitempty"`
	ExtraHeaders map[string]string `json:"extraHeaders,omitempty"`
	Models       []CompatibleModel `json:"models,omitempty"`
}

// CompatibleModel is one model the user declared this endpoint supports.
type CompatibleModel struct {
	ID            string              `json:"id"`
	Label         string              `json:"label"`
	ContextWindow int64               `json:"contextWindow,omitempty"`
	Flags         []CompatibleFlagDim `json:"flags,omitempty"`
}

// CompatibleFlagDim is one picker axis on a compatible model, mapped onto a
// Responses request field via Wire (e.g. "reasoning.effort").
type CompatibleFlagDim struct {
	ID      string                 `json:"id"`
	Label   string                 `json:"label"`
	Wire    string                 `json:"wire,omitempty"`
	Options []CompatibleFlagOption `json:"options,omitempty"`
}

// CompatibleFlagOption is one selectable value inside a CompatibleFlagDim.
type CompatibleFlagOption struct {
	Value   string `json:"value"`
	Label   string `json:"label"`
	Default bool   `json:"default,omitempty"`
}

// ValidCompatibleWire reports whether path is an allowlisted Responses JSON path.
func ValidCompatibleWire(path string) bool {
	path = strings.TrimSpace(path)
	return path == "" || compatibleFlagWires[path]
}

// CompatibleFlagPreset returns stock flag dimensions for the Settings UI.
// Unknown names yield no flags.
func CompatibleFlagPreset(name string) []CompatibleFlagDim {
	effort := CompatibleFlagDim{
		ID:    "effort",
		Label: "Effort",
		Wire:  "reasoning.effort",
		Options: []CompatibleFlagOption{
			{Value: "none", Label: "None"},
			{Value: "low", Label: "Low"},
			{Value: "medium", Label: "Medium", Default: true},
			{Value: "high", Label: "High"},
			{Value: "xhigh", Label: "Extra High"},
		},
	}
	verbosity := CompatibleFlagDim{
		ID:    "verbosity",
		Label: "Verbosity",
		Wire:  "text.verbosity",
		Options: []CompatibleFlagOption{
			{Value: "low", Label: "Low"},
			{Value: "medium", Label: "Medium", Default: true},
			{Value: "high", Label: "High"},
		},
	}
	switch strings.ToLower(strings.TrimSpace(name)) {
	case "openai", "openai-reasoning", "reasoning":
		return []CompatibleFlagDim{effort, verbosity}
	case "effort":
		return []CompatibleFlagDim{effort}
	default:
		return nil
	}
}

// CompatibleByID returns the named provider, or nil.
func (s Settings) CompatibleByID(id string) *CompatibleProvider {
	id = strings.TrimSpace(id)
	if id == "" {
		return nil
	}
	for i := range s.CompatibleProviders {
		if s.CompatibleProviders[i].ID == id {
			p := s.CompatibleProviders[i]
			return &p
		}
	}
	return nil
}

// CompatibleConfigured reports whether the provider has a base URL and at least
// one model. An API key is optional (local servers).
func (p CompatibleProvider) Configured() bool {
	return strings.TrimSpace(p.BaseURL) != "" && len(p.Models) > 0
}

// IsCompatibleSlug reports whether slug names a compatible provider in Settings
// (configured or not). Built-in harness slugs are never compatible.
func (s Settings) IsCompatibleSlug(slug string) bool {
	return s.CompatibleByID(slug) != nil
}

// UsesNativeHarness reports providers whose tools and history Recur owns.
func (s Settings) UsesNativeHarness(slug string) bool {
	return slug == ProviderChatGPT || s.IsCompatibleSlug(slug)
}

func (s Settings) ChatGPTConfigured() bool {
	return s.ChatGPTAccountID != "" && len(s.ChatGPTModels) > 0
}

func (s Settings) ChatGPTProvider() CompatibleProvider {
	return CompatibleProvider{ID: ProviderChatGPT, Name: "ChatGPT", BaseURL: "https://chatgpt.com/backend-api/codex", Models: s.ChatGPTModels}
}

// DisplayLabel returns the human name for a provider slug, preferring a
// compatible provider's Name when present.
func (s Settings) DisplayLabel(provider string) string {
	if p := s.CompatibleByID(provider); p != nil {
		if n := strings.TrimSpace(p.Name); n != "" {
			return n
		}
	}
	return ProviderLabel(provider)
}

// CompatibleModelByID returns the model declared on this provider, or nil.
func (p CompatibleProvider) ModelByID(id string) *CompatibleModel {
	id = strings.TrimSpace(id)
	for i := range p.Models {
		if p.Models[i].ID == id {
			m := p.Models[i]
			return &m
		}
	}
	return nil
}

// NormalizeCompatibleProviders canonicalizes valid providers. Invalid extra-header
// keys are dropped. Empty names become the ID. Base URLs lose a trailing slash.
// Invalid providers are omitted so a corrupt stored row cannot break startup.
func (s Settings) NormalizeCompatibleProviders() Settings {
	if len(s.CompatibleProviders) == 0 {
		return s
	}
	seen := map[string]bool{}
	out := make([]CompatibleProvider, 0, len(s.CompatibleProviders))
	for _, p := range s.CompatibleProviders {
		np, err := NormalizeCompatibleProvider(p, seen)
		if err != nil {
			continue
		}
		seen[np.ID] = true
		out = append(out, np)
	}
	if len(out) == 0 {
		s.CompatibleProviders = nil
	} else {
		s.CompatibleProviders = out
	}
	return s
}

// ValidateCompatibleProviders returns the first validation error without omitting
// any provider. SetSettings uses this so a bad edit cannot delete a saved card.
func ValidateCompatibleProviders(in []CompatibleProvider) error {
	seen := map[string]bool{}
	for _, p := range in {
		np, err := NormalizeCompatibleProvider(p, seen)
		if err != nil {
			return err
		}
		seen[np.ID] = true
	}
	return nil
}

// NormalizeCompatibleProvider canonicalizes one provider. seen holds IDs already
// accepted in this slice (duplicates are rejected).
func NormalizeCompatibleProvider(p CompatibleProvider, seen map[string]bool) (CompatibleProvider, error) {
	p.ID = strings.ToLower(strings.TrimSpace(p.ID))
	if err := validateCompatibleSlug(p.ID); err != nil {
		return CompatibleProvider{}, err
	}
	if seen[p.ID] {
		return CompatibleProvider{}, fmt.Errorf("duplicate compatible provider %q", p.ID)
	}
	p.Name = strings.TrimSpace(p.Name)
	if p.Name == "" {
		p.Name = p.ID
	}
	base, err := normalizeCompatibleBaseURL(p.BaseURL)
	if err != nil {
		return CompatibleProvider{}, err
	}
	p.BaseURL = base
	p.APIKey = strings.TrimSpace(p.APIKey)
	p.ExtraHeaders = cleanExtraHeaders(p.ExtraHeaders)
	if err := uniqueCompatibleModelIDs(p.Models); err != nil {
		return CompatibleProvider{}, fmt.Errorf("compatible provider %q: %w", p.ID, err)
	}
	if err := uniqueCompatibleFlags(p.Models); err != nil {
		return CompatibleProvider{}, fmt.Errorf("compatible provider %q: %w", p.ID, err)
	}
	p.Models = normalizeCompatibleModels(p.Models)
	if len(p.Models) == 0 {
		return CompatibleProvider{}, fmt.Errorf("compatible provider %q has no models", p.ID)
	}
	return p, nil
}

func validateCompatibleSlug(id string) error {
	if !compatibleSlugRE.MatchString(id) {
		return fmt.Errorf("compatible provider id %q must be kebab-case (a-z, digits, hyphen)", id)
	}
	if reservedProviderSlugs[id] {
		return fmt.Errorf("compatible provider id %q is reserved", id)
	}
	return nil
}

func normalizeCompatibleBaseURL(raw string) (string, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return "", fmt.Errorf("base URL is required")
	}
	raw = strings.TrimRight(raw, "/")
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" {
		return "", fmt.Errorf("invalid base URL %q", raw)
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return "", fmt.Errorf("base URL must be http or https")
	}
	return raw, nil
}

func cleanExtraHeaders(in map[string]string) map[string]string {
	if len(in) == 0 {
		return nil
	}
	out := make(map[string]string, len(in))
	for k, v := range in {
		k = strings.TrimSpace(k)
		if k == "" || strings.ContainsAny(k, " \t\r\n:") {
			continue
		}
		out[k] = strings.TrimSpace(v)
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

func uniqueCompatibleModelIDs(in []CompatibleModel) error {
	seen := map[string]bool{}
	for _, m := range in {
		id := strings.TrimSpace(m.ID)
		if id == "" {
			continue
		}
		if seen[id] {
			return fmt.Errorf("duplicate model id %q", id)
		}
		seen[id] = true
	}
	return nil
}

func uniqueCompatibleFlags(in []CompatibleModel) error {
	for _, m := range in {
		seen := map[string]bool{}
		for _, d := range m.Flags {
			id := strings.TrimSpace(d.ID)
			if id == "" {
				continue
			}
			if seen[id] {
				return fmt.Errorf("duplicate flag id %q", id)
			}
			seen[id] = true
			if err := uniqueCompatibleFlagOptions(d.Options); err != nil {
				return fmt.Errorf("flag %q: %w", id, err)
			}
		}
	}
	return nil
}

func uniqueCompatibleFlagOptions(in []CompatibleFlagOption) error {
	seen := map[string]bool{}
	for _, o := range in {
		v := strings.TrimSpace(o.Value)
		if v == "" {
			continue
		}
		if seen[v] {
			return fmt.Errorf("duplicate flag option %q", v)
		}
		seen[v] = true
	}
	return nil
}

func normalizeCompatibleModels(in []CompatibleModel) []CompatibleModel {
	seen := map[string]bool{}
	out := make([]CompatibleModel, 0, len(in))
	for _, m := range in {
		m.ID = strings.TrimSpace(m.ID)
		if m.ID == "" || seen[m.ID] {
			continue
		}
		seen[m.ID] = true
		m.Label = strings.TrimSpace(m.Label)
		if m.Label == "" {
			m.Label = m.ID
		}
		if m.ContextWindow < 0 {
			m.ContextWindow = 0
		}
		m.Flags = normalizeCompatibleFlags(m.Flags)
		out = append(out, m)
	}
	return out
}

func normalizeCompatibleFlags(in []CompatibleFlagDim) []CompatibleFlagDim {
	seen := map[string]bool{}
	out := make([]CompatibleFlagDim, 0, len(in))
	for _, d := range in {
		d.ID = strings.TrimSpace(d.ID)
		if d.ID == "" || seen[d.ID] {
			continue
		}
		seen[d.ID] = true
		d.Label = strings.TrimSpace(d.Label)
		if d.Label == "" {
			d.Label = d.ID
		}
		d.Wire = strings.TrimSpace(d.Wire)
		if !ValidCompatibleWire(d.Wire) {
			d.Wire = ""
		}
		d.Options = normalizeCompatibleFlagOptions(d.Options)
		out = append(out, d)
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

func normalizeCompatibleFlagOptions(in []CompatibleFlagOption) []CompatibleFlagOption {
	seen := map[string]bool{}
	out := make([]CompatibleFlagOption, 0, len(in))
	for _, o := range in {
		o.Value = strings.TrimSpace(o.Value)
		if o.Value == "" || seen[o.Value] {
			continue
		}
		seen[o.Value] = true
		o.Label = strings.TrimSpace(o.Label)
		if o.Label == "" {
			o.Label = o.Value
		}
		out = append(out, o)
	}
	return out
}
