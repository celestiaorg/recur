package bundles

import (
	"fmt"
	"strings"
	"unicode/utf8"
)

// LanguageData is one [[languageData]] entry — comment tokens and indent
// patterns for a TextMate scope (CodeMirror languageData, not Settings prefs).
type LanguageData struct {
	ID                       string `toml:"id" json:"id"`
	ScopeName                string `toml:"scopeName" json:"scopeName"`
	LineComment              string `toml:"lineComment" json:"lineComment,omitempty"`
	BlockCommentStart        string `toml:"blockCommentStart" json:"blockCommentStart,omitempty"`
	BlockCommentEnd          string `toml:"blockCommentEnd" json:"blockCommentEnd,omitempty"`
	IncreaseIndentPattern    string `toml:"increaseIndentPattern" json:"increaseIndentPattern,omitempty"`
	DecreaseIndentPattern    string `toml:"decreaseIndentPattern" json:"decreaseIndentPattern,omitempty"`
	IndentNextLinePattern    string `toml:"indentNextLinePattern" json:"indentNextLinePattern,omitempty"`
	ZeroIndentPattern        string `toml:"zeroIndentPattern" json:"zeroIndentPattern,omitempty"`
	DisableIndentCorrections bool   `toml:"disableIndentCorrections" json:"disableIndentCorrections,omitempty"`
	// CloseBrackets are extra opener characters merged onto CodeMirror defaults
	// (()[]{} '" `). "<" pairs with ">"; "_" / "*" pair with themselves.
	CloseBrackets []string `toml:"closeBrackets" json:"closeBrackets,omitempty"`

	// Filled by Discover — not part of the TOML schema.
	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"` // "user" | "default"
	BundleDir  string `toml:"-" json:"bundleDir"`
}

func parseLanguageData(bundleDir string, meta BundleMeta, raw []LanguageData) ([]LanguageData, error) {
	out := make([]LanguageData, 0, len(raw))
	seenID := map[string]struct{}{}
	seenScope := map[string]struct{}{}
	for i, d := range raw {
		id := strings.TrimSpace(d.ID)
		if id == "" {
			return nil, fmt.Errorf("languageData[%d]: id is required", i)
		}
		if !validBundleID(id) {
			return nil, fmt.Errorf("languageData[%d]: id is not a safe id", i)
		}
		if _, dup := seenID[id]; dup {
			return nil, fmt.Errorf("languageData: duplicate id %q in bundle", id)
		}
		seenID[id] = struct{}{}
		d.ID = id

		scope := strings.TrimSpace(d.ScopeName)
		if scope == "" {
			return nil, fmt.Errorf("languageData %q: scopeName is required", id)
		}
		if _, dup := seenScope[scope]; dup {
			return nil, fmt.Errorf("languageData: duplicate scopeName %q in bundle", scope)
		}
		seenScope[scope] = struct{}{}
		d.ScopeName = scope

		d.LineComment = strings.TrimSpace(d.LineComment)
		d.BlockCommentStart = strings.TrimSpace(d.BlockCommentStart)
		d.BlockCommentEnd = strings.TrimSpace(d.BlockCommentEnd)
		d.IncreaseIndentPattern = strings.TrimSpace(d.IncreaseIndentPattern)
		d.DecreaseIndentPattern = strings.TrimSpace(d.DecreaseIndentPattern)
		d.IndentNextLinePattern = strings.TrimSpace(d.IndentNextLinePattern)
		d.ZeroIndentPattern = strings.TrimSpace(d.ZeroIndentPattern)

		extras := make([]string, 0, len(d.CloseBrackets))
		seenExtra := map[string]struct{}{}
		for _, raw := range d.CloseBrackets {
			ch := strings.TrimSpace(raw)
			if ch == "" {
				continue
			}
			if utf8.RuneCountInString(ch) != 1 {
				return nil, fmt.Errorf("languageData %q: closeBrackets extra %q must be a single character", id, ch)
			}
			if _, dup := seenExtra[ch]; dup {
				continue
			}
			seenExtra[ch] = struct{}{}
			extras = append(extras, ch)
		}
		d.CloseBrackets = extras

		if (d.BlockCommentStart == "") != (d.BlockCommentEnd == "") {
			return nil, fmt.Errorf("languageData %q: blockCommentStart and blockCommentEnd must both be set", id)
		}
		if !languageDataHasPayload(d) {
			return nil, fmt.Errorf("languageData %q: set comment tokens, indent patterns, and/or closeBrackets", id)
		}

		d.BundleName = meta.Name
		d.BundleID = meta.ID
		d.BundleDir = bundleDir
		out = append(out, d)
	}
	return out, nil
}

func languageDataHasPayload(d LanguageData) bool {
	return d.LineComment != "" ||
		d.BlockCommentStart != "" ||
		d.IncreaseIndentPattern != "" ||
		d.DecreaseIndentPattern != "" ||
		d.IndentNextLinePattern != "" ||
		d.ZeroIndentPattern != "" ||
		d.DisableIndentCorrections ||
		len(d.CloseBrackets) > 0
}

// FindLanguageDataByID returns the language-data entry with the given id.
func FindLanguageDataByID(all []LanguageData, id string) (LanguageData, bool) {
	id = strings.TrimSpace(id)
	for _, d := range all {
		if d.ID == id {
			return d, true
		}
	}
	return LanguageData{}, false
}

// FindLanguageDataByScope returns the language-data entry with the given scopeName.
func FindLanguageDataByScope(all []LanguageData, scope string) (LanguageData, bool) {
	scope = strings.TrimSpace(scope)
	for _, d := range all {
		if d.ScopeName == scope {
			return d, true
		}
	}
	return LanguageData{}, false
}

// ResolveLanguageData walks dotted scope prefixes (source.go → source) and
// returns the most specific catalog entry. Empty ok is a miss.
func ResolveLanguageData(all []LanguageData, scope string) (LanguageData, bool) {
	scope = strings.TrimSpace(scope)
	if scope == "" || len(all) == 0 {
		return LanguageData{}, false
	}
	byScope := make(map[string]LanguageData, len(all))
	for _, d := range all {
		byScope[d.ScopeName] = d
	}
	for s := scope; s != ""; {
		if d, ok := byScope[s]; ok {
			return d, true
		}
		i := strings.LastIndex(s, ".")
		if i <= 0 {
			break
		}
		s = s[:i]
	}
	return LanguageData{}, false
}
