// Package bundles implements Recur Bundles: TOML-manifested command packs that
// extend the editor with TextMate-style macros (typescript or shebang).
package bundles

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	toml "github.com/pelletier/go-toml/v2"
)

// Prompt action runtime defaults / ceilings.
const (
	PromptTimeout         = 5 * time.Second
	PromptTimeoutMaxMs    = 30000
	PromptDisplayMaxLen   = 120
	PromptCompleteTimeout = 4 * time.Second
	PromptCompleteMaxHits = 25
	ExpandTimeout         = 15 * time.Second
	promptIconMaxBytes    = 16 << 10
)

// Prompt modes (optional manifest field — omit to stay on the current chat).
const (
	PromptModeAsk    = "ask"
	PromptModePlan   = "plan"
	PromptModeAgent  = "agent"
	PromptModeReview = "review"
	PromptModeLoop   = "loop"
)

var promptSlashRE = regexp.MustCompile(`^[a-z][a-z0-9-]*$`)
var promptArgNameRE = regexp.MustCompile(`^[a-z][a-z0-9_]{0,39}$`)

const PromptArgMax = 8

// validBundleID rejects empty / path-like ids so BundleCache filenames cannot escape
// the cache directory (e.g. id = "../../../../tmp/evil").
func validBundleID(s string) bool {
	s = strings.TrimSpace(s)
	if s == "" || len(s) > 80 {
		return false
	}
	if strings.Contains(s, "..") || strings.ContainsAny(s, `/\`) {
		return false
	}
	if filepath.Base(s) != s {
		return false
	}
	for _, r := range s {
		if unicode.IsLetter(r) || unicode.IsDigit(r) || r == '-' || r == '_' || r == '.' {
			continue
		}
		return false
	}
	return true
}

// Input kinds declare what text a command receives on stdin / as selected text.
const (
	InputNone      = "none"
	InputSelection = "selection"
	InputDocument  = "document"
	InputLine      = "line"
	InputWord      = "word"
)

// Output routing tells the frontend how to apply a command's stdout.
const (
	OutputDiscard          = "discard"
	OutputReplaceSelection = "replaceSelection"
	OutputReplaceDocument  = "replaceDocument"
	OutputReplaceInput     = "replaceInput"
	OutputInsertText       = "insertText"
	OutputInsertSnippet    = "insertSnippet"
	OutputNewDocument      = "newDocument"
	OutputShowAsTooltip    = "showAsTooltip"
	OutputCopyToClipboard  = "copyToClipboard"
	OutputShowAsToast      = "showAsToast"
)

// Interpreter kinds.
const (
	InterpreterShebang    = "shebang"
	InterpreterNative     = "native" // rejected — use typescript
	InterpreterTypeScript = "typescript"
)

// TriggerPalette / TriggerWillSave are the v1 trigger names.
const (
	TriggerPalette  = "palette"
	TriggerWillSave = "willSave"
)

// BundleMeta is the [bundle] table.
type BundleMeta struct {
	Name    string `toml:"name" json:"name"`
	ID      string `toml:"id" json:"id"`
	Version string `toml:"version" json:"version"`
	Author  string `toml:"author" json:"author"`
}

// Prompt is a user-input field collected before a command runs.
type Prompt struct {
	Name    string   `toml:"name" json:"name"`
	Label   string   `toml:"label" json:"label"`
	Type    string   `toml:"type" json:"type"` // text|choice|password
	Default string   `toml:"default" json:"default"`
	Choices []string `toml:"choices" json:"choices,omitempty"`
}

// Command is one [[command]] entry after load (body resolved if script= was set).
type Command struct {
	ID            string   `toml:"id" json:"id"`
	Name          string   `toml:"name" json:"name"`
	Category      string   `toml:"category" json:"category"`
	Order         int      `toml:"order" json:"order"`
	Scope         string   `toml:"scope" json:"scope"`
	FileGlob      string   `toml:"fileGlob" json:"fileGlob"`
	Input         string   `toml:"input" json:"input"`
	FallbackInput string   `toml:"fallbackInput" json:"fallbackInput"`
	Output        string   `toml:"output" json:"output"`
	Triggers      []string `toml:"triggers" json:"triggers"`
	Prompts       []Prompt `toml:"prompts" json:"prompts,omitempty"`
	KeyEquivalent string   `toml:"keyEquivalent" json:"keyEquivalent"`
	// Language is a scratch-tab file extension for newDocument output (e.g. "diff", "json").
	Language string `toml:"language" json:"language"`
	// OpenInPreview opens a newDocument scratch on the Preview chip when the
	// language supports it (html, markdown, svg, …). Ignored for other outputs.
	OpenInPreview bool   `toml:"openInPreview" json:"openInPreview"`
	Interpreter   string `toml:"interpreter" json:"interpreter"`
	Body          string `toml:"body" json:"-"`
	Script        string `toml:"script" json:"-"`

	// Filled by Discover — not part of the TOML schema.
	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"` // "user" | "default"
	BundleDir  string `toml:"-" json:"bundleDir"`
	// ScriptPath is the absolute path when Script was a pointer; empty for inline body.
	ScriptPath string `toml:"-" json:"-"`
}

// PromptAction is one [[prompt]] entry — a chat prompt action (not a command
// Prompt input field). Body is resolved when script= was set.
type PromptAction struct {
	ID           string `toml:"id" json:"id"`
	Name         string `toml:"name" json:"name"`
	Description  string `toml:"description" json:"description"`
	Slash        string `toml:"slash" json:"slash"`
	Mode         string `toml:"mode" json:"mode,omitempty"`
	Loop         string `toml:"loop" json:"loop,omitempty"`
	TimeoutMs    int    `toml:"timeoutMs" json:"timeoutMs,omitempty"`
	FreshSession bool   `toml:"fresh_session" json:"freshSession,omitempty"`
	// TabGroup is a shared chat-tab slot. Empty: nearest idle tab in Mode
	// (or create). Set: nearest idle tab already in this group (or create).
	TabGroup    string `toml:"tab_group" json:"tabGroup,omitempty"`
	Interpreter string `toml:"interpreter" json:"interpreter"`
	// TakesArgs: picking from the / menu inserts a command + arg slots
	// (Discord-style) instead of invoking. Implied when [[prompt.arg]] is set.
	TakesArgs bool `toml:"takesArgs" json:"takesArgs,omitempty"`
	// ArgPlaceholder is the empty-args hint (also the first arg's placeholder
	// when [[prompt.arg]] omits placeholder).
	ArgPlaceholder string `toml:"argPlaceholder" json:"argPlaceholder,omitempty"`
	// Args are named trailing parameters (`[[prompt.arg]]`). The composer
	// renders one labeled slot per entry; scripts see ctx.params / RECUR_PROMPT_ARG_*.
	Args []PromptArgSpec `toml:"arg" json:"args,omitempty"`
	// Complete is an optional script (relative) that suggests trailing-arg hits
	// as the user types. Interpreter is inferred from the extension (.ts →
	// typescript Worker, anything else → shebang). Stdout / return: JSON
	// {"hits":[{"insert","label","detail?","badge?","action?"}],"message?"}.
	Complete string `toml:"complete" json:"-"`
	// Icon is a pack-relative .svg used on complete (and ⌘K) rows. Empty → hash.
	Icon   string `toml:"icon" json:"-"`
	Body   string `toml:"body" json:"-"`
	Script string `toml:"script" json:"-"`

	// Filled by Discover — not part of the TOML schema.
	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"` // "user" | "default"
	BundleDir  string `toml:"-" json:"bundleDir"`
	// ScriptPath is the absolute path when Script was a pointer; empty for inline body.
	ScriptPath string `toml:"-" json:"-"`
	// CompleteBody / CompletePath / CompleteInterpreter are resolved from Complete.
	CompleteBody        string `toml:"-" json:"-"`
	CompletePath        string `toml:"-" json:"-"`
	CompleteInterpreter string `toml:"-" json:"completeInterpreter,omitempty"`
	// IconSVG is the themed markup from Icon (fill=#… → currentColor).
	IconSVG string `toml:"-" json:"iconSvg,omitempty"`
}

// SearchAction is one [[search]] entry — ⌘K rows. Shebang only (secrets).
type SearchAction struct {
	ID          string `toml:"id" json:"id"`
	Name        string `toml:"name" json:"name"`
	TimeoutMs   int    `toml:"timeoutMs" json:"timeoutMs,omitempty"`
	Interpreter string `toml:"interpreter" json:"interpreter"`
	Script      string `toml:"script" json:"-"`
	Icon        string `toml:"icon" json:"-"`
	Body        string `toml:"body" json:"-"`

	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
	ScriptPath string `toml:"-" json:"-"`
	IconSVG    string `toml:"-" json:"iconSvg,omitempty"`
}

// ExpandAction is one [[expand]] entry — New Task / ⌘K pick → work prompt.
// Shebang only. Empty Match means the host never auto-runs this expander.
type ExpandAction struct {
	ID          string `toml:"id" json:"id"`
	Name        string `toml:"name" json:"name"`
	Match       string `toml:"match" json:"match,omitempty"`
	TimeoutMs   int    `toml:"timeoutMs" json:"timeoutMs,omitempty"`
	Interpreter string `toml:"interpreter" json:"interpreter"`
	Script      string `toml:"script" json:"-"`
	Body        string `toml:"body" json:"-"`

	BundleName string         `toml:"-" json:"bundleName"`
	BundleID   string         `toml:"-" json:"bundleID"`
	SourceRoot string         `toml:"-" json:"sourceRoot"`
	BundleDir  string         `toml:"-" json:"bundleDir"`
	ScriptPath string         `toml:"-" json:"-"`
	MatchRE    *regexp.Regexp `toml:"-" json:"-"`
}

// Matches is true when Match is set and text hits it.
func (e ExpandAction) Matches(text string) bool {
	if e.MatchRE == nil {
		return false
	}
	return e.MatchRE.MatchString(strings.TrimSpace(text))
}

func (s SearchAction) timeout() time.Duration {
	if s.TimeoutMs > 0 {
		return time.Duration(s.TimeoutMs) * time.Millisecond
	}
	return PromptCompleteTimeout
}

func (e ExpandAction) timeout() time.Duration {
	if e.TimeoutMs > 0 {
		return time.Duration(e.TimeoutMs) * time.Millisecond
	}
	return ExpandTimeout
}

// PromptArgSpec is one named [[prompt.arg]] slot (Discord-style composer params).
type PromptArgSpec struct {
	Name        string `toml:"name" json:"name"`
	Placeholder string `toml:"placeholder,omitempty" json:"placeholder,omitempty"`
	// Optional: empty slot + Enter invokes (scripts treat empty as unset).
	// Default false — Linear's issue slot stays required.
	Optional bool `toml:"optional,omitempty" json:"optional,omitempty"`
}

// LanguageServer is one [[languageServer]] entry — a long-lived LSP process
// declaration (not a palette/willSave macro). Binaries are resolved via PATH
// and/or a managed download keyed by Managed (see internal/lsp/managed).
type LanguageServer struct {
	ID        string   `toml:"id" json:"id"`
	Languages []string `toml:"languages" json:"languages"`
	FileGlob  []string `toml:"fileGlob" json:"fileGlob,omitempty"`
	Command   string   `toml:"command" json:"command"`
	Args      []string `toml:"args" json:"args,omitempty"`
	Managed   string   `toml:"managed" json:"managed,omitempty"` // pin registry key, e.g. "gopls"
	Env       []string `toml:"env" json:"env,omitempty"`         // KEY=VAL extras

	// Filled by Discover — not part of the TOML schema.
	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"` // "user" | "default"
	BundleDir  string `toml:"-" json:"bundleDir"`
}

// fileManifest is the on-disk shape of bundle.toml.
type fileManifest struct {
	Bundle          BundleMeta       `toml:"bundle"`
	Commands        []Command        `toml:"command"`
	Prompt          []PromptAction   `toml:"prompt"`
	Grammar         []Grammar        `toml:"grammar"`
	Theme           []Theme          `toml:"theme"`
	TerminalTheme   []TerminalTheme  `toml:"terminalTheme"`
	LanguageData    []LanguageData   `toml:"languageData"`
	Snippet         []Snippet        `toml:"snippet"`
	Symbol          []symbolTOML     `toml:"symbol"`
	LanguageServers []LanguageServer `toml:"languageServer"`
	Setting         []Setting        `toml:"setting"`
	Search          []SearchAction   `toml:"search"`
	Expand          []ExpandAction   `toml:"expand"`
	Role            []Role           `toml:"role"`
	Loop            []LoopDef        `toml:"loop"`
	Policy          []Policy         `toml:"policy"`
}

// ParseManifest reads and validates a bundle.toml, resolving script pointers
// relative to bundleDir. Inline body and script are mutually exclusive per
// command and prompt. Returns commands and [[prompt]] actions.
func ParseManifest(bundleDir string, data []byte) (BundleMeta, []Command, []PromptAction, error) {
	parsed, err := ParseManifestFull(bundleDir, data)
	if err != nil {
		return BundleMeta{}, nil, nil, err
	}
	return parsed.Meta, parsed.Commands, parsed.Prompts, nil
}

// ParseManifestFull is ParseManifest plus [[grammar]], [[theme]],
// [[terminalTheme]], [[languageData]], [[snippet]], [[symbol]],
// [[languageServer]], and [[setting]] entries. Grammar-only, theme-only,
// terminal-theme-only, language-data-only, snippet-only, symbol-only,
// language-server-only, or setting-only bundles (no commands) are valid.
func ParseManifestFull(bundleDir string, data []byte) (ParsedBundle, error) {
	var fm fileManifest
	if err := toml.Unmarshal(data, &fm); err != nil {
		return ParsedBundle{}, fmt.Errorf("parse bundle.toml: %w", err)
	}
	fm.Bundle.ID = strings.TrimSpace(fm.Bundle.ID)
	if fm.Bundle.ID == "" {
		return ParsedBundle{}, fmt.Errorf("bundle.toml: [bundle].id is required")
	}
	if !validBundleID(fm.Bundle.ID) {
		return ParsedBundle{}, fmt.Errorf("bundle.toml: [bundle].id is not a safe id")
	}
	if strings.TrimSpace(fm.Bundle.Name) == "" {
		fm.Bundle.Name = filepath.Base(bundleDir)
	}
	out := make([]Command, 0, len(fm.Commands))
	for i, c := range fm.Commands {
		c.ID = strings.TrimSpace(c.ID)
		if c.ID == "" {
			return ParsedBundle{}, fmt.Errorf("command[%d]: id is required", i)
		}
		if !validBundleID(c.ID) {
			return ParsedBundle{}, fmt.Errorf("command[%d]: id is not a safe id", i)
		}
		if strings.TrimSpace(c.Name) == "" {
			return ParsedBundle{}, fmt.Errorf("command %q: name is required", c.ID)
		}
		if c.Category == "" {
			c.Category = fm.Bundle.Name
		}
		if c.Input == "" {
			c.Input = InputSelection
		}
		if !validInputKind(c.Input) {
			return ParsedBundle{}, fmt.Errorf(
				"command %q: unknown input %q (use none|selection|document|line|word)",
				c.ID, c.Input,
			)
		}
		if c.FallbackInput != "" && !validInputKind(c.FallbackInput) {
			return ParsedBundle{}, fmt.Errorf(
				"command %q: unknown fallbackInput %q (use none|selection|document|line|word)",
				c.ID, c.FallbackInput,
			)
		}
		if c.Output == "" {
			c.Output = OutputReplaceSelection
		}
		if err := ValidateOutput(c.Output); err != nil {
			return ParsedBundle{}, fmt.Errorf("command %q: %w", c.ID, err)
		}
		if c.Interpreter == "" {
			c.Interpreter = InterpreterShebang
		}
		switch c.Interpreter {
		case InterpreterNative, "js":
			return ParsedBundle{}, fmt.Errorf(
				"command %q: interpreter %q is not valid (use typescript or shebang)",
				c.ID, c.Interpreter,
			)
		case InterpreterShebang, InterpreterTypeScript:
			// ok
		default:
			return ParsedBundle{}, fmt.Errorf("command %q: unknown interpreter %q", c.ID, c.Interpreter)
		}
		if len(c.Triggers) == 0 {
			c.Triggers = []string{TriggerPalette}
		}
		for _, tr := range c.Triggers {
			if !validTrigger(tr) {
				return ParsedBundle{}, fmt.Errorf(
					"command %q: unknown trigger %q (use palette|willSave)",
					c.ID, tr,
				)
			}
		}
		body := strings.TrimSpace(c.Body)
		script := strings.TrimSpace(c.Script)
		if body != "" && script != "" {
			return ParsedBundle{}, fmt.Errorf("command %q: set body or script, not both", c.ID)
		}
		if script != "" {
			p := filepath.Join(bundleDir, filepath.FromSlash(script))
			b, err := os.ReadFile(p)
			if err != nil {
				return ParsedBundle{}, fmt.Errorf("command %q: read script %q: %w", c.ID, script, err)
			}
			c.Body = string(b)
			c.ScriptPath = p
			c.Script = script
		}
		if strings.TrimSpace(c.Body) == "" {
			return ParsedBundle{}, fmt.Errorf("command %q: body or script is required", c.ID)
		}
		c.BundleName = fm.Bundle.Name
		c.BundleID = fm.Bundle.ID
		c.BundleDir = bundleDir
		out = append(out, c)
	}
	prompts, err := parsePromptActions(bundleDir, fm.Bundle, fm.Prompt)
	if err != nil {
		return ParsedBundle{}, err
	}
	grammars, err := parseGrammars(bundleDir, fm.Bundle, fm.Grammar)
	if err != nil {
		return ParsedBundle{}, err
	}
	themes, err := parseThemes(bundleDir, fm.Bundle, fm.Theme)
	if err != nil {
		return ParsedBundle{}, err
	}
	termThemes, err := parseTerminalThemes(bundleDir, fm.Bundle, fm.TerminalTheme)
	if err != nil {
		return ParsedBundle{}, err
	}
	langData, err := parseLanguageData(bundleDir, fm.Bundle, fm.LanguageData)
	if err != nil {
		return ParsedBundle{}, err
	}
	snippets, err := parseSnippets(bundleDir, fm.Bundle, fm.Snippet)
	if err != nil {
		return ParsedBundle{}, err
	}
	symbols, err := parseSymbols(bundleDir, fm.Bundle, fm.Symbol)
	if err != nil {
		return ParsedBundle{}, err
	}
	servers, err := parseLanguageServers(bundleDir, fm.Bundle, fm.LanguageServers)
	if err != nil {
		return ParsedBundle{}, err
	}
	settings, err := parseSettings(bundleDir, fm.Bundle, fm.Setting)
	if err != nil {
		return ParsedBundle{}, err
	}
	searches, err := parseSearchActions(bundleDir, fm.Bundle, fm.Search)
	if err != nil {
		return ParsedBundle{}, err
	}
	expands, err := parseExpandActions(bundleDir, fm.Bundle, fm.Expand)
	if err != nil {
		return ParsedBundle{}, err
	}
	roles, err := parseRoles(bundleDir, fm.Bundle, fm.Role)
	if err != nil {
		return ParsedBundle{}, err
	}
	policies, err := parsePolicies(bundleDir, fm.Bundle, fm.Policy)
	if err != nil {
		return ParsedBundle{}, err
	}
	loops, err := parseLoops(bundleDir, fm.Bundle, fm.Loop)
	if err != nil {
		return ParsedBundle{}, err
	}
	return ParsedBundle{
		Meta:            fm.Bundle,
		Commands:        out,
		Prompts:         prompts,
		Grammars:        grammars,
		Themes:          themes,
		TerminalThemes:  termThemes,
		LanguageData:    langData,
		Snippets:        snippets,
		Symbols:         symbols,
		LanguageServers: servers,
		Settings:        settings,
		Searches:        searches,
		Expands:         expands,
		Roles:           roles,
		Loops:           loops,
		Policies:        policies,
	}, nil
}

func parseLanguageServers(bundleDir string, meta BundleMeta, raw []LanguageServer) ([]LanguageServer, error) {
	out := make([]LanguageServer, 0, len(raw))
	for i, s := range raw {
		id := strings.TrimSpace(s.ID)
		if id == "" {
			return nil, fmt.Errorf("languageServer[%d]: id is required", i)
		}
		if !validBundleID(id) {
			return nil, fmt.Errorf("languageServer[%d]: id %q is not a safe id", i, id)
		}
		s.ID = id
		if strings.TrimSpace(s.Command) == "" {
			return nil, fmt.Errorf("languageServer %q: command is required", id)
		}
		langs := make([]string, 0, len(s.Languages))
		for _, l := range s.Languages {
			l = strings.ToLower(strings.TrimSpace(l))
			if l == "" {
				continue
			}
			langs = append(langs, l)
		}
		if len(langs) == 0 {
			return nil, fmt.Errorf("languageServer %q: languages must list at least one language id", id)
		}
		s.Languages = langs
		s.Managed = strings.TrimSpace(s.Managed)
		s.BundleName = meta.Name
		s.BundleID = meta.ID
		s.BundleDir = bundleDir
		out = append(out, s)
	}
	return out, nil
}

func parsePromptActions(bundleDir string, meta BundleMeta, raw []PromptAction) ([]PromptAction, error) {
	out := make([]PromptAction, 0, len(raw))
	for i, p := range raw {
		p.ID = strings.TrimSpace(p.ID)
		if p.ID == "" {
			return nil, fmt.Errorf("prompt[%d]: id is required", i)
		}
		if !validBundleID(p.ID) {
			return nil, fmt.Errorf("prompt[%d]: id is not a safe id", i)
		}
		if strings.TrimSpace(p.Name) == "" {
			return nil, fmt.Errorf("prompt %q: name is required", p.ID)
		}
		slash := strings.TrimSpace(p.Slash)
		if slash == "" {
			slash = kebabFromName(p.Name)
		}
		if !promptSlashRE.MatchString(slash) {
			return nil, fmt.Errorf(
				"prompt %q: slash %q must match %s",
				p.ID, slash, promptSlashRE.String(),
			)
		}
		p.Slash = slash
		mode := strings.TrimSpace(p.Mode)
		loopID := strings.TrimSpace(p.Loop)
		if loopID != "" {
			if !validBundleID(loopID) {
				return nil, fmt.Errorf("prompt %q: loop %q is not a safe id", p.ID, loopID)
			}
			if mode == "" {
				mode = PromptModeLoop
			} else if mode != PromptModeLoop {
				return nil, fmt.Errorf("prompt %q: loop requires mode %q", p.ID, PromptModeLoop)
			}
		}
		if mode != "" && !validPromptMode(mode) {
			return nil, fmt.Errorf(
				"prompt %q: unknown mode %q (use ask|plan|agent|review|loop, or omit)",
				p.ID, mode,
			)
		}
		p.Mode = mode
		p.Loop = loopID
		if p.FreshSession && p.Mode == PromptModeLoop {
			return nil, fmt.Errorf(
				`prompt %q: fresh_session is not valid with mode %q`,
				p.ID, PromptModeLoop,
			)
		}
		tabGroup := strings.TrimSpace(p.TabGroup)
		if tabGroup != "" {
			if p.Mode == "" {
				return nil, fmt.Errorf("prompt %q: tab_group requires mode", p.ID)
			}
			if !promptSlashRE.MatchString(tabGroup) {
				return nil, fmt.Errorf(
					"prompt %q: tab_group %q must match %s",
					p.ID, tabGroup, promptSlashRE.String(),
				)
			}
		}
		p.TabGroup = tabGroup
		if p.TimeoutMs < 0 || p.TimeoutMs > PromptTimeoutMaxMs {
			return nil, fmt.Errorf(
				"prompt %q: timeoutMs must be 0 or 1..%d",
				p.ID, PromptTimeoutMaxMs,
			)
		}
		if p.Interpreter == "" {
			p.Interpreter = InterpreterShebang
		}
		switch p.Interpreter {
		case InterpreterNative, "js":
			return nil, fmt.Errorf(
				"prompt %q: interpreter %q is not valid (use typescript or shebang)",
				p.ID, p.Interpreter,
			)
		case InterpreterShebang, InterpreterTypeScript:
			// ok
		default:
			return nil, fmt.Errorf("prompt %q: unknown interpreter %q", p.ID, p.Interpreter)
		}
		body := strings.TrimSpace(p.Body)
		script := strings.TrimSpace(p.Script)
		if body != "" && script != "" {
			return nil, fmt.Errorf("prompt %q: set body or script, not both", p.ID)
		}
		if body == "" && script == "" {
			return nil, fmt.Errorf("prompt %q: body or script is required", p.ID)
		}
		if script != "" {
			path := filepath.Join(bundleDir, filepath.FromSlash(script))
			b, err := os.ReadFile(path)
			if err != nil {
				return nil, fmt.Errorf("prompt %q: read script %q: %w", p.ID, script, err)
			}
			p.Body = string(b)
			p.ScriptPath = path
			p.Script = script
		}
		if strings.TrimSpace(p.Body) == "" {
			return nil, fmt.Errorf("prompt %q: body or script is required", p.ID)
		}
		if err := normalizePromptArgs(&p); err != nil {
			return nil, err
		}
		if err := resolvePromptComplete(&p, bundleDir); err != nil {
			return nil, err
		}
		if err := resolvePromptIcon(&p, bundleDir); err != nil {
			return nil, err
		}
		p.ArgPlaceholder = strings.TrimSpace(p.ArgPlaceholder)
		if utf8.RuneCountInString(p.ArgPlaceholder) > 200 {
			return nil, fmt.Errorf("prompt %q: argPlaceholder is too long", p.ID)
		}
		p.BundleName = meta.Name
		p.BundleID = meta.ID
		p.BundleDir = bundleDir
		out = append(out, p)
	}
	return out, nil
}

func parseSearchActions(bundleDir string, meta BundleMeta, raw []SearchAction) ([]SearchAction, error) {
	out := make([]SearchAction, 0, len(raw))
	for i, s := range raw {
		s.ID = strings.TrimSpace(s.ID)
		if s.ID == "" {
			return nil, fmt.Errorf("search[%d]: id is required", i)
		}
		if !validBundleID(s.ID) {
			return nil, fmt.Errorf("search[%d]: id is not a safe id", i)
		}
		if strings.TrimSpace(s.Name) == "" {
			return nil, fmt.Errorf("search %q: name is required", s.ID)
		}
		if s.TimeoutMs < 0 || s.TimeoutMs > PromptTimeoutMaxMs {
			return nil, fmt.Errorf("search %q: timeoutMs must be 0 or 1..%d", s.ID, PromptTimeoutMaxMs)
		}
		if err := requireShebangScript("search", s.ID, &s.Interpreter, &s.Body, &s.Script, &s.ScriptPath, bundleDir); err != nil {
			return nil, err
		}
		svg, err := resolvePackIcon("search", s.ID, s.Icon, bundleDir)
		if err != nil {
			return nil, err
		}
		s.Icon = strings.TrimSpace(s.Icon)
		s.IconSVG = svg
		s.BundleName = meta.Name
		s.BundleID = meta.ID
		s.BundleDir = bundleDir
		out = append(out, s)
	}
	return out, nil
}

func parseExpandActions(bundleDir string, meta BundleMeta, raw []ExpandAction) ([]ExpandAction, error) {
	out := make([]ExpandAction, 0, len(raw))
	for i, e := range raw {
		e.ID = strings.TrimSpace(e.ID)
		if e.ID == "" {
			return nil, fmt.Errorf("expand[%d]: id is required", i)
		}
		if !validBundleID(e.ID) {
			return nil, fmt.Errorf("expand[%d]: id is not a safe id", i)
		}
		if strings.TrimSpace(e.Name) == "" {
			return nil, fmt.Errorf("expand %q: name is required", e.ID)
		}
		if e.TimeoutMs < 0 || e.TimeoutMs > PromptTimeoutMaxMs {
			return nil, fmt.Errorf("expand %q: timeoutMs must be 0 or 1..%d", e.ID, PromptTimeoutMaxMs)
		}
		if err := requireShebangScript("expand", e.ID, &e.Interpreter, &e.Body, &e.Script, &e.ScriptPath, bundleDir); err != nil {
			return nil, err
		}
		e.Match = strings.TrimSpace(e.Match)
		if e.Match != "" {
			re, err := regexp.Compile(e.Match)
			if err != nil {
				return nil, fmt.Errorf("expand %q: match: %w", e.ID, err)
			}
			e.MatchRE = re
		}
		e.BundleName = meta.Name
		e.BundleID = meta.ID
		e.BundleDir = bundleDir
		out = append(out, e)
	}
	return out, nil
}

func requireShebangScript(kind, id string, interpreter, body, script, scriptPath *string, bundleDir string) error {
	interp := strings.TrimSpace(*interpreter)
	if interp == "" {
		interp = InterpreterShebang
	}
	if interp != InterpreterShebang {
		return fmt.Errorf("%s %q: interpreter must be shebang", kind, id)
	}
	*interpreter = interp
	b := strings.TrimSpace(*body)
	sc := strings.TrimSpace(*script)
	if b != "" && sc != "" {
		return fmt.Errorf("%s %q: set body or script, not both", kind, id)
	}
	if b == "" && sc == "" {
		return fmt.Errorf("%s %q: body or script is required", kind, id)
	}
	if sc != "" {
		path := filepath.Join(bundleDir, filepath.FromSlash(sc))
		raw, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("%s %q: read script %q: %w", kind, id, sc, err)
		}
		*body = string(raw)
		*scriptPath = path
		*script = sc
	}
	if strings.TrimSpace(*body) == "" {
		return fmt.Errorf("%s %q: body or script is required", kind, id)
	}
	return nil
}

// kebabFromName lowercases name and replaces non-alphanumeric runs with '-'.
func kebabFromName(name string) string {
	var b strings.Builder
	prevHyphen := false
	for _, r := range strings.ToLower(strings.TrimSpace(name)) {
		if (r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') {
			b.WriteRune(r)
			prevHyphen = false
			continue
		}
		if !prevHyphen && b.Len() > 0 {
			b.WriteByte('-')
			prevHyphen = true
		}
	}
	return strings.Trim(b.String(), "-")
}

func normalizePromptArgs(p *PromptAction) error {
	raw := p.Args
	out := make([]PromptArgSpec, 0, len(raw))
	seen := make(map[string]struct{}, len(raw))
	for i, a := range raw {
		name := strings.ToLower(strings.TrimSpace(a.Name))
		if name == "" {
			return fmt.Errorf("prompt %q: arg[%d]: name is required", p.ID, i)
		}
		if !promptArgNameRE.MatchString(name) {
			return fmt.Errorf("prompt %q: arg %q must match %s", p.ID, a.Name, promptArgNameRE.String())
		}
		if _, dup := seen[name]; dup {
			return fmt.Errorf("prompt %q: duplicate arg %q", p.ID, name)
		}
		seen[name] = struct{}{}
		ph := strings.TrimSpace(a.Placeholder)
		if utf8.RuneCountInString(ph) > 200 {
			return fmt.Errorf("prompt %q: arg %q placeholder is too long", p.ID, name)
		}
		out = append(out, PromptArgSpec{Name: name, Placeholder: ph, Optional: a.Optional})
	}
	if len(out) > PromptArgMax {
		return fmt.Errorf("prompt %q: at most %d args", p.ID, PromptArgMax)
	}
	if len(out) > 0 {
		p.TakesArgs = true
		if p.ArgPlaceholder == "" {
			p.ArgPlaceholder = out[0].Placeholder
		}
	} else if p.TakesArgs {
		out = []PromptArgSpec{{Name: "args", Placeholder: p.ArgPlaceholder}}
	}
	p.Args = out
	return nil
}

func resolvePromptComplete(p *PromptAction, bundleDir string) error {
	rel := strings.TrimSpace(p.Complete)
	p.Complete = rel
	p.CompleteBody = ""
	p.CompletePath = ""
	p.CompleteInterpreter = ""
	if rel == "" {
		return nil
	}
	if !p.TakesArgs {
		return fmt.Errorf("prompt %q: complete requires takesArgs", p.ID)
	}
	path := filepath.Join(bundleDir, filepath.FromSlash(rel))
	b, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("prompt %q: read complete %q: %w", p.ID, rel, err)
	}
	if strings.TrimSpace(string(b)) == "" {
		return fmt.Errorf("prompt %q: complete %q is empty", p.ID, rel)
	}
	p.CompleteBody = string(b)
	p.CompletePath = path
	p.CompleteInterpreter = completeInterpreterFor(rel)
	return nil
}

var (
	promptIconFillRE = regexp.MustCompile(`fill\s*=\s*["']#[0-9A-Fa-f]{3,8}["']`)
	promptIconBad    = []string{"<script", "javascript:", "onload=", "onerror=", "<foreignobject", "<iframe", "<embed"}
)

func resolvePromptIcon(p *PromptAction, bundleDir string) error {
	svg, err := resolvePackIcon("prompt", p.ID, p.Icon, bundleDir)
	if err != nil {
		return err
	}
	p.Icon = strings.TrimSpace(p.Icon)
	p.IconSVG = svg
	return nil
}

func resolvePackIcon(kind, id, rel, bundleDir string) (string, error) {
	rel = strings.TrimSpace(rel)
	if rel == "" {
		return "", nil
	}
	if strings.ToLower(filepath.Ext(rel)) != ".svg" {
		return "", fmt.Errorf("%s %q: icon must be a .svg", kind, id)
	}
	path := filepath.Join(bundleDir, filepath.FromSlash(rel))
	b, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("%s %q: read icon %q: %w", kind, id, rel, err)
	}
	if len(b) > promptIconMaxBytes {
		return "", fmt.Errorf("%s %q: icon is larger than %d bytes", kind, id, promptIconMaxBytes)
	}
	svg, err := sanitizePromptIconSVG(string(b))
	if err != nil {
		return "", fmt.Errorf("%s %q: icon %q: %w", kind, id, rel, err)
	}
	return svg, nil
}

func sanitizePromptIconSVG(raw string) (string, error) {
	s := strings.TrimSpace(raw)
	low := strings.ToLower(s)
	if !strings.Contains(low, "<svg") {
		return "", fmt.Errorf("not an svg")
	}
	for _, bad := range promptIconBad {
		if strings.Contains(low, bad) {
			return "", fmt.Errorf("svg is not a static icon")
		}
	}
	// simplify: hardcoded fills → currentColor so dark/light chrome can theme.
	return promptIconFillRE.ReplaceAllString(s, `fill="currentColor"`), nil
}

func completeInterpreterFor(rel string) string {
	switch strings.ToLower(filepath.Ext(rel)) {
	case ".ts", ".mts", ".js", ".mjs":
		return InterpreterTypeScript
	default:
		return InterpreterShebang
	}
}

func validPromptMode(s string) bool {
	switch s {
	case PromptModeAsk, PromptModePlan, PromptModeAgent, PromptModeReview, PromptModeLoop:
		return true
	default:
		return false
	}
}

func validInputKind(s string) bool {
	switch s {
	case InputNone, InputSelection, InputDocument, InputLine, InputWord:
		return true
	default:
		return false
	}
}

func validTrigger(s string) bool {
	switch s {
	case TriggerPalette, TriggerWillSave:
		return true
	default:
		return false
	}
}

// ValidateOutput reports whether s is a known Output* routing value.
// showAsHTML is rejected with a migration hint.
func ValidateOutput(s string) error {
	switch s {
	case OutputDiscard, OutputReplaceSelection, OutputReplaceDocument, OutputReplaceInput,
		OutputInsertText, OutputInsertSnippet, OutputNewDocument, OutputShowAsTooltip,
		OutputCopyToClipboard, OutputShowAsToast:
		return nil
	case "showAsHTML":
		return fmt.Errorf("output showAsHTML was removed — use newDocument with language + openInPreview")
	default:
		return fmt.Errorf("unknown output %q", s)
	}
}
