package bundles

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// UI theme kinds for [[theme]].uiTheme.
const (
	UIThemeDark  = "dark"
	UIThemeLight = "light"
)

// Grammar is one [[grammar]] entry — a TextMate language grammar contributed
// by a bundle (JSON .tmLanguage.json preferred; plist .tmLanguage accepted).
type Grammar struct {
	ID                string            `toml:"id" json:"id"`
	ScopeName         string            `toml:"scopeName" json:"scopeName"`
	Path              string            `toml:"path" json:"path"`
	Language          string            `toml:"language" json:"language"`
	FileTypes         []string          `toml:"fileTypes" json:"fileTypes"`
	FirstLineMatch    string            `toml:"firstLineMatch" json:"firstLineMatch"`
	InjectTo          []string          `toml:"injectTo" json:"injectTo"`
	EmbeddedLanguages map[string]string `toml:"embeddedLanguages" json:"embeddedLanguages,omitempty"`

	// Filled by Discover — not part of the TOML schema.
	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"` // "user" | "default"
	BundleDir  string `toml:"-" json:"bundleDir"`
	AbsPath    string `toml:"-" json:"-"`
}

// ThemePreview is optional Settings picker swatches for a [[theme]].
// Roles match the built-in editor theme cards: bg/fg plus sample
// `fn` / `main` / `"hi"` colours.
type ThemePreview struct {
	Bg     string `toml:"bg" json:"bg,omitempty"`
	Fg     string `toml:"fg" json:"fg,omitempty"`
	Fn     string `toml:"fn" json:"fn,omitempty"`
	Name   string `toml:"name" json:"name,omitempty"`
	String string `toml:"string" json:"string,omitempty"`
}

// Theme is one [[theme]] entry — a TextMate / VS Code colour theme JSON
// (conventional path: themes/<name>.tmTheme.json).
// ID is the Settings preference value (e.g. "cool-glow", "recur").
type Theme struct {
	ID      string `toml:"id" json:"id"`
	Name    string `toml:"name" json:"name"`
	Path    string `toml:"path" json:"path"`
	UITheme string `toml:"uiTheme" json:"uiTheme"` // light | dark
	// Optional Settings display order (lower first). Zero sorts after explicit orders.
	Order int `toml:"order" json:"order,omitempty"`

	// Optional: preview = { bg, fg, fn, name, string }
	Preview ThemePreview `toml:"preview" json:"preview"`

	// Filled by Discover — not part of the TOML schema.
	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
	AbsPath    string `toml:"-" json:"-"`
}

// TerminalPalette is an xterm.js ITheme JSON object.
type TerminalPalette struct {
	Background                  string `json:"background,omitempty"`
	Foreground                  string `json:"foreground,omitempty"`
	Cursor                      string `json:"cursor,omitempty"`
	CursorAccent                string `json:"cursorAccent,omitempty"`
	SelectionBackground         string `json:"selectionBackground,omitempty"`
	SelectionForeground         string `json:"selectionForeground,omitempty"`
	SelectionInactiveBackground string `json:"selectionInactiveBackground,omitempty"`
	Black                       string `json:"black,omitempty"`
	Red                         string `json:"red,omitempty"`
	Green                       string `json:"green,omitempty"`
	Yellow                      string `json:"yellow,omitempty"`
	Blue                        string `json:"blue,omitempty"`
	Magenta                     string `json:"magenta,omitempty"`
	Cyan                        string `json:"cyan,omitempty"`
	White                       string `json:"white,omitempty"`
	BrightBlack                 string `json:"brightBlack,omitempty"`
	BrightRed                   string `json:"brightRed,omitempty"`
	BrightGreen                 string `json:"brightGreen,omitempty"`
	BrightYellow                string `json:"brightYellow,omitempty"`
	BrightBlue                  string `json:"brightBlue,omitempty"`
	BrightMagenta               string `json:"brightMagenta,omitempty"`
	BrightCyan                  string `json:"brightCyan,omitempty"`
	BrightWhite                 string `json:"brightWhite,omitempty"`
}

func (p *TerminalPalette) colorFields() []struct {
	name string
	dst  *string
} {
	return []struct {
		name string
		dst  *string
	}{
		{"background", &p.Background},
		{"foreground", &p.Foreground},
		{"cursor", &p.Cursor},
		{"cursorAccent", &p.CursorAccent},
		{"selectionBackground", &p.SelectionBackground},
		{"selectionForeground", &p.SelectionForeground},
		{"selectionInactiveBackground", &p.SelectionInactiveBackground},
		{"black", &p.Black},
		{"red", &p.Red},
		{"green", &p.Green},
		{"yellow", &p.Yellow},
		{"blue", &p.Blue},
		{"magenta", &p.Magenta},
		{"cyan", &p.Cyan},
		{"white", &p.White},
		{"brightBlack", &p.BrightBlack},
		{"brightRed", &p.BrightRed},
		{"brightGreen", &p.BrightGreen},
		{"brightYellow", &p.BrightYellow},
		{"brightBlue", &p.BrightBlue},
		{"brightMagenta", &p.BrightMagenta},
		{"brightCyan", &p.BrightCyan},
		{"brightWhite", &p.BrightWhite},
	}
}

// TerminalTheme is one [[terminalTheme]] entry — an xterm.js ITheme JSON file.
type TerminalTheme struct {
	ID      string `toml:"id" json:"id"`
	Name    string `toml:"name" json:"name"`
	Path    string `toml:"path" json:"path"`
	UITheme string `toml:"uiTheme" json:"uiTheme"`
	Order   int    `toml:"order" json:"order,omitempty"`

	Palette TerminalPalette `toml:"-" json:"palette"`

	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
	AbsPath    string `toml:"-" json:"-"`
}

// ParsedBundle is the full result of parsing one bundle.toml.
type ParsedBundle struct {
	Meta            BundleMeta
	Commands        []Command
	Prompts         []PromptAction
	Grammars        []Grammar
	Themes          []Theme
	TerminalThemes  []TerminalTheme
	LanguageData    []LanguageData
	Snippets        []Snippet
	Symbols         []Symbol
	LanguageServers []LanguageServer
	Settings        []Setting
	Searches        []SearchAction
	Expands         []ExpandAction
	Roles           []Role
	Loops           []LoopDef
	Policies        []Policy
}

// Catalog is the deduped discover result across all scan roots.
type Catalog struct {
	Commands        []Command
	Prompts         []PromptAction
	Grammars        []Grammar
	Themes          []Theme
	TerminalThemes  []TerminalTheme
	LanguageData    []LanguageData
	Snippets        []Snippet
	Symbols         []Symbol
	LanguageServers []LanguageServer
	Settings        []Setting
	Searches        []SearchAction
	Expands         []ExpandAction
	Roles           []Role
	Loops           []LoopDef
	Policies        []Policy
	Skips           []DiscoverSkip
}

// resolveBundleRelPath joins rel to bundleDir and rejects escapes / abs paths.
func resolveBundleRelPath(bundleDir, rel string) (string, error) {
	rel = strings.TrimSpace(rel)
	if rel == "" {
		return "", fmt.Errorf("path is required")
	}
	if filepath.IsAbs(rel) || strings.Contains(rel, "..") {
		return "", fmt.Errorf("path %q must be relative and must not contain ..", rel)
	}
	abs := filepath.Clean(filepath.Join(bundleDir, filepath.FromSlash(rel)))
	base := filepath.Clean(bundleDir)
	relOut, err := filepath.Rel(base, abs)
	if err != nil || relOut == ".." || strings.HasPrefix(relOut, ".."+string(filepath.Separator)) {
		return "", fmt.Errorf("path %q escapes bundle directory", rel)
	}
	return abs, nil
}

func parseGrammars(bundleDir string, meta BundleMeta, raw []Grammar) ([]Grammar, error) {
	out := make([]Grammar, 0, len(raw))
	seenScope := map[string]struct{}{}
	for i, g := range raw {
		scope := strings.TrimSpace(g.ScopeName)
		if scope == "" {
			return nil, fmt.Errorf("grammar[%d]: scopeName is required", i)
		}
		if _, dup := seenScope[scope]; dup {
			return nil, fmt.Errorf("grammar: duplicate scopeName %q in bundle", scope)
		}
		seenScope[scope] = struct{}{}
		g.ScopeName = scope

		id := strings.TrimSpace(g.ID)
		if id == "" {
			return nil, fmt.Errorf("grammar %q: id is required", scope)
		}
		if !validBundleID(id) {
			return nil, fmt.Errorf("grammar %q: id is not a safe id", scope)
		}
		g.ID = id

		abs, err := resolveBundleRelPath(bundleDir, g.Path)
		if err != nil {
			return nil, fmt.Errorf("grammar %q: %w", scope, err)
		}
		if _, err := os.Stat(abs); err != nil {
			return nil, fmt.Errorf("grammar %q: read path %q: %w", scope, g.Path, err)
		}
		g.AbsPath = abs
		g.Path = strings.TrimSpace(g.Path)

		for j, ft := range g.FileTypes {
			g.FileTypes[j] = strings.TrimSpace(ft)
		}
		for j, inj := range g.InjectTo {
			g.InjectTo[j] = strings.TrimSpace(inj)
		}
		g.Language = strings.TrimSpace(g.Language)
		g.FirstLineMatch = strings.TrimSpace(g.FirstLineMatch)
		g.BundleName = meta.Name
		g.BundleID = meta.ID
		g.BundleDir = bundleDir
		out = append(out, g)
	}
	return out, nil
}

func parseThemes(bundleDir string, meta BundleMeta, raw []Theme) ([]Theme, error) {
	out := make([]Theme, 0, len(raw))
	seenID := map[string]struct{}{}
	for i, t := range raw {
		id := strings.TrimSpace(t.ID)
		if id == "" {
			return nil, fmt.Errorf("theme[%d]: id is required", i)
		}
		if !validBundleID(id) {
			return nil, fmt.Errorf("theme[%d]: id is not a safe id", i)
		}
		if _, dup := seenID[id]; dup {
			return nil, fmt.Errorf("theme: duplicate id %q in bundle", id)
		}
		seenID[id] = struct{}{}
		t.ID = id

		name := strings.TrimSpace(t.Name)
		if name == "" {
			return nil, fmt.Errorf("theme %q: name is required", id)
		}
		t.Name = name

		ui := strings.TrimSpace(t.UITheme)
		if ui == "" {
			ui = UIThemeDark
		}
		if ui != UIThemeDark && ui != UIThemeLight {
			return nil, fmt.Errorf("theme %q: uiTheme must be light or dark", id)
		}
		t.UITheme = ui

		for _, pc := range []struct {
			name string
			dst  *string
		}{
			{"preview.bg", &t.Preview.Bg},
			{"preview.fg", &t.Preview.Fg},
			{"preview.fn", &t.Preview.Fn},
			{"preview.name", &t.Preview.Name},
			{"preview.string", &t.Preview.String},
		} {
			norm, err := normalizePreviewColor(*pc.dst)
			if err != nil {
				return nil, fmt.Errorf("theme %q: %s: %w", id, pc.name, err)
			}
			*pc.dst = norm
		}

		abs, err := resolveBundleRelPath(bundleDir, t.Path)
		if err != nil {
			return nil, fmt.Errorf("theme %q: %w", id, err)
		}
		if _, err := os.Stat(abs); err != nil {
			return nil, fmt.Errorf("theme %q: read path %q: %w", id, t.Path, err)
		}
		t.AbsPath = abs
		t.Path = strings.TrimSpace(t.Path)
		t.BundleName = meta.Name
		t.BundleID = meta.ID
		t.BundleDir = bundleDir
		out = append(out, t)
	}
	return out, nil
}

func parseTerminalThemes(bundleDir string, meta BundleMeta, raw []TerminalTheme) ([]TerminalTheme, error) {
	out := make([]TerminalTheme, 0, len(raw))
	seenID := map[string]struct{}{}
	for i, t := range raw {
		id := strings.TrimSpace(t.ID)
		if id == "" {
			return nil, fmt.Errorf("terminalTheme[%d]: id is required", i)
		}
		if !validBundleID(id) {
			return nil, fmt.Errorf("terminalTheme[%d]: id is not a safe id", i)
		}
		if _, dup := seenID[id]; dup {
			return nil, fmt.Errorf("terminalTheme: duplicate id %q in bundle", id)
		}
		seenID[id] = struct{}{}
		t.ID = id

		name := strings.TrimSpace(t.Name)
		if name == "" {
			return nil, fmt.Errorf("terminalTheme %q: name is required", id)
		}
		t.Name = name

		ui := strings.TrimSpace(t.UITheme)
		if ui == "" {
			ui = UIThemeDark
		}
		if ui != UIThemeDark && ui != UIThemeLight {
			return nil, fmt.Errorf("terminalTheme %q: uiTheme must be light or dark", id)
		}
		t.UITheme = ui

		abs, err := resolveBundleRelPath(bundleDir, t.Path)
		if err != nil {
			return nil, fmt.Errorf("terminalTheme %q: %w", id, err)
		}
		body, err := os.ReadFile(abs)
		if err != nil {
			return nil, fmt.Errorf("terminalTheme %q: read path %q: %w", id, t.Path, err)
		}
		var pal TerminalPalette
		if err := json.Unmarshal(body, &pal); err != nil {
			return nil, fmt.Errorf("terminalTheme %q: parse %q: %w", id, t.Path, err)
		}
		for _, pc := range pal.colorFields() {
			norm, err := normalizeThemeColor(*pc.dst)
			if err != nil {
				return nil, fmt.Errorf("terminalTheme %q: %s: %w", id, pc.name, err)
			}
			*pc.dst = norm
		}
		if pal.Background == "" || pal.Foreground == "" {
			return nil, fmt.Errorf("terminalTheme %q: background and foreground are required", id)
		}
		t.Palette = pal
		t.AbsPath = abs
		t.Path = strings.TrimSpace(t.Path)
		t.BundleName = meta.Name
		t.BundleID = meta.ID
		t.BundleDir = bundleDir
		out = append(out, t)
	}
	return out, nil
}

// normalizePreviewColor accepts empty or #rgb / #rrggbb / #rrggbbaa.
func normalizePreviewColor(s string) (string, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return "", nil
	}
	if !strings.HasPrefix(s, "#") {
		return "", fmt.Errorf("must be a #hex colour")
	}
	n := len(s) - 1
	if n != 3 && n != 6 && n != 8 {
		return "", fmt.Errorf("must be #rgb, #rrggbb, or #rrggbbaa")
	}
	for _, r := range s[1:] {
		if (r >= '0' && r <= '9') || (r >= 'a' && r <= 'f') || (r >= 'A' && r <= 'F') {
			continue
		}
		return "", fmt.Errorf("invalid hex digit")
	}
	return s, nil
}

// normalizeThemeColor accepts empty, #hex, or rgb/hsl CSS colours.
func normalizeThemeColor(s string) (string, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return "", nil
	}
	if strings.HasPrefix(s, "#") {
		return normalizePreviewColor(s)
	}
	lower := strings.ToLower(s)
	if strings.HasPrefix(lower, "rgb(") || strings.HasPrefix(lower, "rgba(") ||
		strings.HasPrefix(lower, "hsl(") || strings.HasPrefix(lower, "hsla(") {
		return s, nil
	}
	return "", fmt.Errorf("must be a #hex or rgb/hsl colour")
}

// FindGrammarByScope returns the grammar with the given scopeName.
func FindGrammarByScope(grammars []Grammar, scope string) (Grammar, bool) {
	scope = strings.TrimSpace(scope)
	for _, g := range grammars {
		if g.ScopeName == scope {
			return g, true
		}
	}
	return Grammar{}, false
}

// FindGrammarByID returns the grammar with the given id.
func FindGrammarByID(grammars []Grammar, id string) (Grammar, bool) {
	id = strings.TrimSpace(id)
	for _, g := range grammars {
		if g.ID == id {
			return g, true
		}
	}
	return Grammar{}, false
}

// FindThemeByID returns the theme with the given id.
func FindThemeByID(themes []Theme, id string) (Theme, bool) {
	id = strings.TrimSpace(id)
	for _, t := range themes {
		if t.ID == id {
			return t, true
		}
	}
	return Theme{}, false
}

// FindTerminalThemeByID returns the terminal theme with the given id.
func FindTerminalThemeByID(themes []TerminalTheme, id string) (TerminalTheme, bool) {
	id = strings.TrimSpace(id)
	for _, t := range themes {
		if t.ID == id {
			return t, true
		}
	}
	return TerminalTheme{}, false
}

// GrammarConventionOK reports whether path looks like a TextMate grammar file.
func GrammarConventionOK(path string) bool {
	lower := strings.ToLower(path)
	return strings.HasSuffix(lower, ".tmlanguage.json") ||
		strings.HasSuffix(lower, ".tmlanguage") ||
		strings.HasSuffix(lower, ".json-tmlanguage")
}

// ThemeConventionOK reports whether path looks like a TextMate theme JSON file.
func ThemeConventionOK(path string) bool {
	lower := strings.ToLower(path)
	return strings.HasSuffix(lower, ".tmtheme.json") ||
		strings.HasSuffix(lower, ".tmtheme")
}

// TerminalThemeConventionOK reports whether path looks like an xterm ITheme JSON file.
func TerminalThemeConventionOK(path string) bool {
	lower := strings.ToLower(path)
	return strings.HasSuffix(lower, ".json") && !strings.HasSuffix(lower, ".tmtheme.json")
}
