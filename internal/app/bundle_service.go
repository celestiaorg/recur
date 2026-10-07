package app

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"recur/internal/bundles"
	"recur/internal/domain"
	"recur/internal/store"
)

// BundleService discovers Recur Bundles, runs shebang commands, and compiles
// typescript macros for the frontend Worker (CompiledModule).
type BundleService struct {
	base
	catMu    sync.Mutex
	catalog  bundles.Catalog
	catStamp string
	catUser  string
	catDef   string
}

// CommandPromptDTO is a prompt field for the UI.
type CommandPromptDTO struct {
	Name    string   `json:"name"`
	Label   string   `json:"label"`
	Type    string   `json:"type"`
	Default string   `json:"default"`
	Choices []string `json:"choices,omitempty"`
}

// CommandInfo is the plain-struct view of a bundle command for the UI.
// No TOML crosses the Wails bridge.
type CommandInfo struct {
	ID            string             `json:"id"`
	Name          string             `json:"name"`
	Category      string             `json:"category"`
	Order         int                `json:"order"`
	Scope         string             `json:"scope"`
	FileGlob      string             `json:"fileGlob"`
	Input         string             `json:"input"`
	FallbackInput string             `json:"fallbackInput"`
	Output        string             `json:"output"`
	Triggers      []string           `json:"triggers"`
	Prompts       []CommandPromptDTO `json:"prompts,omitempty"`
	KeyEquivalent string             `json:"keyEquivalent"`
	Language      string             `json:"language"`
	OpenInPreview bool               `json:"openInPreview"`
	Interpreter   string             `json:"interpreter"`
	BundleName    string             `json:"bundleName"`
	SourceRoot    string             `json:"sourceRoot"`
}

// SnippetInfo is a bundle [[snippet]] for the UI.
type SnippetInfo struct {
	ID            string `json:"id"`
	Name          string `json:"name"`
	TabTrigger    string `json:"tabTrigger"`
	Scope         string `json:"scope"`
	FileGlob      string `json:"fileGlob"`
	Body          string `json:"body"`
	KeyEquivalent string `json:"keyEquivalent"`
	Order         int    `json:"order"`
	BundleName    string `json:"bundleName"`
	SourceRoot    string `json:"sourceRoot"`
}

// SymbolInfo is a bundle [[symbol]] rule for the open-file outline.
type SymbolInfo struct {
	ID         string `json:"id"`
	Scope      string `json:"scope"`
	Transform  string `json:"transform,omitempty"`
	Kind       string `json:"kind,omitempty"`
	Show       bool   `json:"show"`
	BundleName string `json:"bundleName"`
	SourceRoot string `json:"sourceRoot"`
}

// RunContextDTO is the editor state the frontend sends for RunCommand.
type RunContextDTO struct {
	SelectedText     string            `json:"selectedText"`
	CurrentLine      string            `json:"currentLine"`
	LineNumber       int               `json:"lineNumber"`
	ColumnNumber     int               `json:"columnNumber"`
	CurrentWord      string            `json:"currentWord"`
	FilePath         string            `json:"filePath"`
	FileName         string            `json:"fileName"`
	Directory        string            `json:"directory"`
	ProjectDirectory string            `json:"projectDirectory"`
	Scope            string            `json:"scope"`
	TabSize          int               `json:"tabSize"`
	SoftTabs         bool              `json:"softTabs"`
	Clipboard        string            `json:"clipboard"`
	Input            string            `json:"input"`
	PromptValues     map[string]string `json:"promptValues,omitempty"`
	WorkDir          string            `json:"workDir"`
	// TimeoutMs overrides the default 30s shebang deadline when > 0 (e.g. willSave).
	TimeoutMs int `json:"timeoutMs,omitempty"`
}

// RunResultDTO is the outcome of RunCommand.
type RunResultDTO struct {
	Stdout   string `json:"stdout"`
	Stderr   string `json:"stderr"`
	ExitCode int    `json:"exitCode"`
	Output   string `json:"output"`
}

// BundleSkipInfo is a bundle directory Discover skipped (Settings / diagnostics).
type BundleSkipInfo struct {
	Dir    string `json:"dir"`
	Name   string `json:"name"`
	Source string `json:"source"`
	Reason string `json:"reason"`
}

// GrammarInfo is a TextMate grammar contributed by a bundle (metadata only).
type GrammarInfo struct {
	ScopeName         string            `json:"scopeName"`
	ID                string            `json:"id"`
	Path              string            `json:"path"`
	Language          string            `json:"language,omitempty"`
	FileTypes         []string          `json:"fileTypes,omitempty"`
	FirstLineMatch    string            `json:"firstLineMatch,omitempty"`
	InjectTo          []string          `json:"injectTo,omitempty"`
	EmbeddedLanguages map[string]string `json:"embeddedLanguages,omitempty"`
	BundleName        string            `json:"bundleName"`
	SourceRoot        string            `json:"sourceRoot"`
	// Optional [[languageData]] merged by scopeName (and dotted prefixes).
	LineComment              string   `json:"lineComment,omitempty"`
	BlockCommentStart        string   `json:"blockCommentStart,omitempty"`
	BlockCommentEnd          string   `json:"blockCommentEnd,omitempty"`
	IncreaseIndentPattern    string   `json:"increaseIndentPattern,omitempty"`
	DecreaseIndentPattern    string   `json:"decreaseIndentPattern,omitempty"`
	IndentNextLinePattern    string   `json:"indentNextLinePattern,omitempty"`
	ZeroIndentPattern        string   `json:"zeroIndentPattern,omitempty"`
	DisableIndentCorrections bool     `json:"disableIndentCorrections,omitempty"`
	CloseBrackets            []string `json:"closeBrackets,omitempty"`
}

// ThemePreviewInfo is Settings picker swatches from [[theme]].preview.
type ThemePreviewInfo struct {
	Bg     string `json:"bg,omitempty"`
	Fg     string `json:"fg,omitempty"`
	Fn     string `json:"fn,omitempty"`
	Name   string `json:"name,omitempty"`
	String string `json:"string,omitempty"`
}

// ThemeInfo is a TextMate / VS Code theme contributed by a bundle (metadata only).
type ThemeInfo struct {
	ID         string           `json:"id"`
	Name       string           `json:"name"`
	Path       string           `json:"path"`
	UITheme    string           `json:"uiTheme"`
	Order      int              `json:"order,omitempty"`
	Preview    ThemePreviewInfo `json:"preview"`
	BundleName string           `json:"bundleName"`
	SourceRoot string           `json:"sourceRoot"`
}

// TerminalPaletteInfo is an xterm.js ITheme from [[terminalTheme]].
type TerminalPaletteInfo struct {
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

// TerminalThemeInfo is an xterm.js ITheme contributed by a bundle.
type TerminalThemeInfo struct {
	ID         string              `json:"id"`
	Name       string              `json:"name"`
	Path       string              `json:"path"`
	UITheme    string              `json:"uiTheme"`
	Order      int                 `json:"order,omitempty"`
	Palette    TerminalPaletteInfo `json:"palette"`
	BundleName string              `json:"bundleName"`
	SourceRoot string              `json:"sourceRoot"`
}

// ListCommands discovers all bundles and returns commands matching scope/path.
// When both scope and path are empty, every command is returned (callers that
// need file affinity — e.g. the Run Command palette — re-filter client-side).
func (s *BundleService) ListCommands(scope, path string) ([]CommandInfo, error) {
	cmds, err := s.discover()
	if err != nil {
		return nil, err
	}
	filter := scope != "" || path != ""
	out := make([]CommandInfo, 0, len(cmds))
	for _, c := range cmds {
		if filter && !bundles.Matches(c, scope, path) {
			continue
		}
		out = append(out, toCommandInfo(c))
	}
	return out, nil
}

// ListSnippets discovers all [[snippet]] entries matching scope/path.
// When both scope and path are empty, every snippet is returned.
func (s *BundleService) ListSnippets(scope, path string) ([]SnippetInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	filter := scope != "" || path != ""
	out := make([]SnippetInfo, 0, len(cat.Snippets))
	for _, sn := range cat.Snippets {
		if filter && !bundles.SnippetMatches(sn, scope, path) {
			continue
		}
		out = append(out, toSnippetInfo(sn))
	}
	return out, nil
}

// ListSymbols discovers all [[symbol]] showInSymbolList rules.
func (s *BundleService) ListSymbols() ([]SymbolInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	out := make([]SymbolInfo, 0, len(cat.Symbols))
	for _, sy := range cat.Symbols {
		out = append(out, toSymbolInfo(sy))
	}
	return out, nil
}

// BundleSettingInfo is one [[setting]] for Settings → Integrations.
type BundleSettingInfo struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Label       string `json:"label"`
	Description string `json:"description"`
	Secret      bool   `json:"secret"`
	Placeholder string `json:"placeholder"`
	URL         string `json:"url"`
	URLLabel    string `json:"urlLabel"`
	BundleID    string `json:"bundleId"`
	BundleName  string `json:"bundleName"`
	SourceRoot  string `json:"sourceRoot"`
}

// ListBundleSettings discovers all [[setting]] entries (grouped by bundle in the UI).
func (s *BundleService) ListBundleSettings() ([]BundleSettingInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	out := make([]BundleSettingInfo, 0, len(cat.Settings))
	for _, st := range cat.Settings {
		out = append(out, BundleSettingInfo{
			ID:          st.ID,
			Name:        st.Name,
			Label:       st.Label,
			Description: st.Description,
			Secret:      st.Secret,
			Placeholder: st.Placeholder,
			URL:         st.URL,
			URLLabel:    st.URLLabel,
			BundleID:    st.BundleID,
			BundleName:  st.BundleName,
			SourceRoot:  st.SourceRoot,
		})
	}
	return out, nil
}

// ListSkippedBundles returns directories under the scan roots that were not loaded
// (bad id, parse error, missing manifest, …). Used by Settings → Bundles.
func (s *BundleService) ListSkippedBundles() ([]BundleSkipInfo, error) {
	_, _, skips, err := s.discoverReport()
	if err != nil {
		return nil, err
	}
	out := make([]BundleSkipInfo, len(skips))
	for i, sk := range skips {
		out[i] = BundleSkipInfo{
			Dir: sk.Dir, Name: sk.Name, Source: sk.Source, Reason: sk.Reason,
		}
	}
	return out, nil
}

// PromptActionInfo is the plain-struct view of a [[prompt]] for the UI.
type PromptActionInfo struct {
	ID                  string          `json:"id"`
	Name                string          `json:"name"`
	Description         string          `json:"description"`
	Slash               string          `json:"slash"`
	Mode                string          `json:"mode,omitempty"`
	Loop                string          `json:"loop,omitempty"`
	TimeoutMs           int             `json:"timeoutMs,omitempty"`
	FreshSession        bool            `json:"freshSession,omitempty"`
	TabGroup            string          `json:"tabGroup,omitempty"`
	Interpreter         string          `json:"interpreter"`
	TakesArgs           bool            `json:"takesArgs,omitempty"`
	HasComplete         bool            `json:"hasComplete,omitempty"`
	ArgPlaceholder      string          `json:"argPlaceholder,omitempty"`
	CompleteInterpreter string          `json:"completeInterpreter,omitempty"`
	IconSVG             string          `json:"iconSvg,omitempty"`
	Args                []PromptArgInfo `json:"args,omitempty"`
	BundleName          string          `json:"bundleName"`
	SourceRoot          string          `json:"sourceRoot"`
}

// PromptArgInfo is one named [[prompt.arg]] slot for the composer.
type PromptArgInfo struct {
	Name        string `json:"name"`
	Placeholder string `json:"placeholder,omitempty"`
	Optional    bool   `json:"optional,omitempty"`
}

// PromptContextDTO is workspace + editor state the frontend sends for RunPromptAction.
// Workspace fields may be marshaled by the frontend (no store lookup required).
type PromptContextDTO struct {
	WorkspaceID string `json:"workspaceId"`
	ProjectID   string `json:"projectId"`
	Worktree    string `json:"worktree"`
	Port        int    `json:"port"`

	SelectedText string `json:"selectedText"`
	Document     string `json:"document"`
	CurrentLine  string `json:"currentLine"`
	LineNumber   int    `json:"lineNumber"`
	Column       int    `json:"column"`
	CurrentWord  string `json:"currentWord"`
	FilePath     string `json:"filepath"`
	FileName     string `json:"filename"`
	Scope        string `json:"scope"`
	Clipboard    string `json:"clipboard"`
	TabSize      int    `json:"tabSize"`
	SoftTabs     bool   `json:"softTabs"`
	WorkDir      string `json:"workDir"`
	// TimeoutMs overrides the default / manifest shebang deadline when > 0.
	TimeoutMs int `json:"timeoutMs,omitempty"`
	// Surface is the operator's looking-at view: workspace, local_pr, github_pr.
	Surface string `json:"surface,omitempty"`
	// ChatCode is the chat tab the prompt will be sent to (PA, PA2, …).
	ChatCode string `json:"chatCode,omitempty"`
	// Args is the trailing composer remainder after `/slash`.
	Args string `json:"args,omitempty"`
	// Params are named [[prompt.arg]] values.
	Params map[string]string `json:"params,omitempty"`
}

// PromptResultDTO is the outcome of RunPromptAction.
type PromptResultDTO struct {
	Prompt  string `json:"prompt"`
	Display string `json:"display"`
	// FreshSession, when set, overrides the manifest fresh_session flag for
	// this send (a /re-review continuing the same chat's review resumes it).
	FreshSession *bool `json:"freshSession,omitempty"`
}

// ListPromptActions discovers all [[prompt]] entries (alphabetical by slash).
func (s *BundleService) ListPromptActions() ([]PromptActionInfo, error) {
	_, prompts, err := s.discoverAll()
	if err != nil {
		return nil, err
	}
	out := make([]PromptActionInfo, 0, len(prompts))
	for _, p := range prompts {
		out = append(out, toPromptActionInfo(p))
	}
	return out, nil
}

// CompiledPromptModule returns esbuild-compiled CommonJS for a typescript prompt.
// Shebang prompts must use RunPromptAction instead.
func (s *BundleService) CompiledPromptModule(id string) (string, error) {
	_, prompts, err := s.discoverAll()
	if err != nil {
		return "", err
	}
	prompt, ok := bundles.FindPromptByID(prompts, id)
	if !ok {
		return "", fmt.Errorf("prompt %q not found", id)
	}
	if !bundles.IsScriptInterpreter(prompt.Interpreter) {
		return "", fmt.Errorf("prompt %q is not typescript", id)
	}
	cache, err := store.BundlesCacheDir()
	if err != nil {
		return "", err
	}
	res, err := bundles.CompilePromptMacro(cache, prompt)
	if err != nil {
		return "", err
	}
	return res.JS, nil
}

// CompiledCompleteModule returns esbuild-compiled CommonJS for a typescript
// [[prompt]] complete script. Shebang complete must use RunPromptComplete.
func (s *BundleService) CompiledCompleteModule(id string) (string, error) {
	_, prompts, err := s.discoverAll()
	if err != nil {
		return "", err
	}
	prompt, ok := bundles.FindPromptByID(prompts, id)
	if !ok {
		return "", fmt.Errorf("prompt %q not found", id)
	}
	if prompt.CompleteInterpreter != bundles.InterpreterTypeScript {
		return "", fmt.Errorf("prompt %q complete is not typescript", id)
	}
	cache, err := store.BundlesCacheDir()
	if err != nil {
		return "", err
	}
	res, err := bundles.CompileCompleteMacro(cache, prompt)
	if err != nil {
		return "", err
	}
	return res.JS, nil
}

// RunPromptAction looks up a shebang prompt by id and executes it.
// TypeScript prompts must use CompiledPromptModule + the frontend Worker.
func (s *BundleService) RunPromptAction(id string, rc PromptContextDTO) (PromptResultDTO, error) {
	_, prompts, err := s.discoverAll()
	if err != nil {
		return PromptResultDTO{}, err
	}
	prompt, ok := bundles.FindPromptByID(prompts, id)
	if !ok {
		return PromptResultDTO{}, fmt.Errorf("prompt %q not found", id)
	}
	if bundles.IsScriptInterpreter(prompt.Interpreter) {
		return PromptResultDTO{}, fmt.Errorf("prompt %q is %s — use CompiledPromptModule", id, prompt.Interpreter)
	}
	timeout := bundles.PromptTimeout
	if prompt.TimeoutMs > 0 {
		timeout = time.Duration(prompt.TimeoutMs) * time.Millisecond
	}
	if rc.TimeoutMs > 0 {
		timeout = time.Duration(rc.TimeoutMs) * time.Millisecond
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	runCtx := toPromptRunContext(rc, prompt)
	s.fillPromptGitEnv(&runCtx, rc.WorkspaceID)
	s.fillBundleSettingEnv(&runCtx.BundleSettings, prompt.BundleID)
	out := PromptResultDTO{}
	if rc.ChatCode != "" && runCtx.Surface == "workspace" {
		runCtx.ReviewSince, runCtx.ReviewNow = s.eng.ReviewCheckpoint(rc.WorkspaceID, rc.ChatCode, prompt.Slash)
		if runCtx.ReviewSince != "" {
			fresh := false
			out.FreshSession = &fresh
		}
	}
	res, err := bundles.RunPromptShebang(ctx, prompt, runCtx)
	if err != nil {
		return PromptResultDTO{}, err
	}
	out.Prompt, out.Display = res.Prompt, res.Display
	return out, nil
}

// PromptCompleteRequestDTO is the composer slot state for RunPromptComplete.
type PromptCompleteRequestDTO struct {
	// Args is the active slot's current value (also ctx.args / RECUR_PROMPT_ARGS).
	Args string `json:"args"`
	// Arg is the [[prompt.arg]] name being completed (ctx.arg / RECUR_PROMPT_COMPLETE_ARG).
	Arg string `json:"arg,omitempty"`
	// Params are all named slot values so far.
	Params map[string]string `json:"params,omitempty"`
	// WorkspaceID sets cwd to the workspace worktree so tools like `gh` see the right repo.
	WorkspaceID string `json:"workspaceId,omitempty"`
}

// PromptCompleteHitDTO is one trailing-arg suggestion from a complete script.
type PromptCompleteHitDTO struct {
	Insert string `json:"insert"`
	Label  string `json:"label"`
	Psub   string `json:"psub,omitempty"`
	Detail string `json:"detail,omitempty"`
	Badge  string `json:"badge,omitempty"`
	Action string `json:"action,omitempty"`
}

// PromptCompleteResultDTO is the outcome of RunPromptComplete.
type PromptCompleteResultDTO struct {
	Hits    []PromptCompleteHitDTO `json:"hits"`
	Message string                 `json:"message,omitempty"`
}

// RunPromptComplete runs a shebang [[prompt]] complete script.
// TypeScript complete scripts must use CompiledCompleteModule + the frontend Worker.
func (s *BundleService) RunPromptComplete(id string, req PromptCompleteRequestDTO) (PromptCompleteResultDTO, error) {
	_, prompts, err := s.discoverAll()
	if err != nil {
		return PromptCompleteResultDTO{}, err
	}
	prompt, ok := bundles.FindPromptByID(prompts, id)
	if !ok {
		return PromptCompleteResultDTO{}, fmt.Errorf("prompt %q not found", id)
	}
	if strings.TrimSpace(prompt.CompleteBody) == "" {
		return PromptCompleteResultDTO{Hits: []PromptCompleteHitDTO{}}, nil
	}
	if bundles.IsScriptInterpreter(prompt.CompleteInterpreter) {
		return PromptCompleteResultDTO{}, fmt.Errorf("prompt %q complete is %s — use CompiledCompleteModule", id, prompt.CompleteInterpreter)
	}
	ctx, cancel := context.WithTimeout(context.Background(), bundles.PromptCompleteTimeout)
	defer cancel()
	args := req.Args
	arg := strings.TrimSpace(req.Arg)
	params := req.Params
	if len(params) == 0 {
		params = bundles.ParamsFromArgs(prompt, args)
	}
	if arg == "" && len(prompt.Args) > 0 {
		arg = prompt.Args[0].Name
	}
	runCtx := bundles.PromptRunContext{
		Args:          args,
		CompleteArg:   arg,
		Params:        params,
		PromptSlash:   prompt.Slash,
		BundleSupport: prompt.BundleDir,
		WorkDir:       prompt.BundleDir,
		WorkspaceID:   req.WorkspaceID,
	}
	if req.WorkspaceID != "" {
		if ws, ok := s.st().Workspace(req.WorkspaceID); ok {
			runCtx.ProjectID = ws.ProjectID
			runCtx.Worktree = ws.WorktreePath
			runCtx.Port = ws.Port
			if wt := strings.TrimSpace(ws.WorktreePath); wt != "" {
				runCtx.WorkDir = wt
			}
		}
	}
	s.fillPromptGitEnv(&runCtx, req.WorkspaceID)
	s.fillBundleSettingEnv(&runCtx.BundleSettings, prompt.BundleID)
	res, err := bundles.RunPromptCompleteShebang(ctx, prompt, runCtx)
	if err != nil {
		return PromptCompleteResultDTO{}, err
	}
	out := make([]PromptCompleteHitDTO, 0, len(res.Hits))
	for _, h := range res.Hits {
		out = append(out, toCompleteHitDTO(h))
	}
	return PromptCompleteResultDTO{Hits: out, Message: res.Message}, nil
}

func toCompleteHitDTO(h bundles.PromptCompleteHit) PromptCompleteHitDTO {
	return PromptCompleteHitDTO{
		Insert: h.Insert, Label: h.Label, Psub: h.Psub,
		Detail: h.Detail, Badge: h.Badge, Action: h.Action,
	}
}

// PaletteSearchGroupDTO is one ⌘K section from bundle [[search]] items.
type PaletteSearchGroupDTO struct {
	Name    string                 `json:"name"`
	IconSVG string                 `json:"iconSvg,omitempty"`
	Hits    []PromptCompleteHitDTO `json:"hits"`
}

// PaletteSearchDTO is the outcome of SearchPalette.
type PaletteSearchDTO struct {
	Groups []PaletteSearchGroupDTO `json:"groups"`
}

// SearchPalette runs every [[search]] concurrently for the ⌘K query.
// Queries shorter than 2 runes, or catalogs with no search items, return empty.
func (s *BundleService) SearchPalette(query string) (PaletteSearchDTO, error) {
	q := strings.TrimSpace(query)
	empty := PaletteSearchDTO{Groups: []PaletteSearchGroupDTO{}}
	if utf8.RuneCountInString(q) < 2 {
		return empty, nil
	}
	cat, err := s.discoverCatalog()
	if err != nil {
		return empty, err
	}
	if len(cat.Searches) == 0 {
		return empty, nil
	}
	groups := bundles.CollectSearchGroups(context.Background(), cat.Searches, func(se bundles.SearchAction) bundles.PromptRunContext {
		rc := bundles.PromptRunContext{
			SearchQuery:   q,
			Args:          q,
			CompleteArg:   "issue",
			Params:        map[string]string{"issue": q},
			BundleSupport: se.BundleDir,
			WorkDir:       se.BundleDir,
		}
		s.fillBundleSettingEnv(&rc.BundleSettings, se.BundleID)
		return rc
	})
	out := make([]PaletteSearchGroupDTO, 0, len(groups))
	for _, g := range groups {
		hits := make([]PromptCompleteHitDTO, 0, len(g.Hits))
		for _, h := range g.Hits {
			hits = append(hits, toCompleteHitDTO(h))
		}
		out = append(out, PaletteSearchGroupDTO{Name: g.Name, IconSVG: g.IconSVG, Hits: hits})
	}
	return PaletteSearchDTO{Groups: out}, nil
}

// ExpandGoal runs matching [[expand]] shebangs. Unchanged text if none match.
func (s *BundleService) ExpandGoal(text string) (PromptResultDTO, error) {
	trimmed := strings.TrimSpace(text)
	if trimmed == "" {
		return PromptResultDTO{Prompt: ""}, nil
	}
	cat, err := s.discoverCatalog()
	if err != nil {
		return PromptResultDTO{}, err
	}
	res, err := bundles.ExpandMatching(trimmed, cat.Expands, func(ex bundles.ExpandAction) bundles.PromptRunContext {
		rc := bundles.PromptRunContext{
			Args:          trimmed,
			Params:        map[string]string{"issue": trimmed},
			BundleSupport: ex.BundleDir,
			WorkDir:       ex.BundleDir,
		}
		s.fillBundleSettingEnv(&rc.BundleSettings, ex.BundleID)
		return rc
	})
	if err != nil {
		return PromptResultDTO{}, err
	}
	return PromptResultDTO{Prompt: res.Prompt, Display: res.Display}, nil
}

// RunCommand looks up a shebang command by id and executes it as a subprocess.
// TypeScript macros must use CompiledModule + the frontend Worker instead.
func (s *BundleService) RunCommand(id string, rc RunContextDTO) (RunResultDTO, error) {
	cmds, err := s.discover()
	if err != nil {
		return RunResultDTO{}, err
	}
	cmd, ok := bundles.FindByID(cmds, id)
	if !ok {
		return RunResultDTO{}, fmt.Errorf("command %q not found", id)
	}
	if bundles.IsScriptInterpreter(cmd.Interpreter) {
		return RunResultDTO{}, fmt.Errorf("command %q is %s — use CompiledModule", id, cmd.Interpreter)
	}
	timeout := bundles.DefaultTimeout
	if rc.TimeoutMs > 0 {
		timeout = time.Duration(rc.TimeoutMs) * time.Millisecond
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	runCtx := toRunContext(rc, cmd)
	s.fillBundleSettingEnv(&runCtx.BundleSettings, cmd.BundleID)
	res, err := bundles.Run(ctx, cmd, runCtx)
	if err != nil {
		return RunResultDTO{
			Stdout:   res.Stdout,
			Stderr:   res.Stderr,
			ExitCode: res.ExitCode,
			Output:   cmd.Output,
		}, err
	}
	return RunResultDTO{
		Stdout:   res.Stdout,
		Stderr:   res.Stderr,
		ExitCode: res.ExitCode,
		Output:   res.Output,
	}, nil
}

// CompiledModule returns esbuild-compiled CommonJS for a typescript command,
// using the on-disk BundleCache (rebuilt when source changes). The frontend
// runs it in a sandboxed Worker.
func (s *BundleService) CompiledModule(id string) (string, error) {
	cmds, err := s.discover()
	if err != nil {
		return "", err
	}
	cmd, ok := bundles.FindByID(cmds, id)
	if !ok {
		return "", fmt.Errorf("command %q not found", id)
	}
	if !bundles.IsScriptInterpreter(cmd.Interpreter) {
		return "", fmt.Errorf("command %q is not typescript", id)
	}
	cache, err := store.BundlesCacheDir()
	if err != nil {
		return "", err
	}
	res, err := bundles.CompileMacro(cache, cmd)
	if err != nil {
		return "", err
	}
	return res.JS, nil
}

// ListGrammars returns TextMate grammars from shipped + user bundles.
func (s *BundleService) ListGrammars() ([]GrammarInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	out := make([]GrammarInfo, len(cat.Grammars))
	for i, g := range cat.Grammars {
		out[i] = toGrammarInfo(g, cat.LanguageData)
	}
	return out, nil
}

// ReadGrammar returns the raw grammar file contents for scopeName.
func (s *BundleService) ReadGrammar(scopeName string) (string, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return "", err
	}
	g, ok := bundles.FindGrammarByScope(cat.Grammars, scopeName)
	if !ok {
		return "", fmt.Errorf("grammar %q not found", scopeName)
	}
	body, err := os.ReadFile(g.AbsPath)
	if err != nil {
		return "", fmt.Errorf("read grammar %q: %w", scopeName, err)
	}
	return string(body), nil
}

// RoleInfo is a bundle [[role]] for pickers / Settings.
type RoleInfo struct {
	ID         string   `json:"id"`
	Name       string   `json:"name"`
	Hint       string   `json:"hint,omitempty"`
	Code       string   `json:"code"`
	Mode       string   `json:"mode"`
	Tools      []string `json:"tools,omitempty"`
	ModelSlot  string   `json:"modelSlot,omitempty"`
	BundleName string   `json:"bundleName"`
	SourceRoot string   `json:"sourceRoot"`
}

// LoopInfo is a bundle [[loop]] for the dock / New Task picker.
type LoopInfo struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	Hint       string `json:"hint,omitempty"`
	Entry      string `json:"entry"`
	BundleName string `json:"bundleName"`
	SourceRoot string `json:"sourceRoot"`
}

// PolicyInfo is a bundle [[policy]] for the dock / New Task picker.
type PolicyInfo struct {
	ID           string `json:"id"`
	Name         string `json:"name"`
	Hint         string `json:"hint,omitempty"`
	MaxRounds    int    `json:"maxRounds"`
	OnMaxRounds  string `json:"onMaxRounds"`
	PlanApproval string `json:"planApproval"`
	ExtendRounds int    `json:"extendRounds"`
	BundleName   string `json:"bundleName"`
	SourceRoot   string `json:"sourceRoot"`
}

// ListRoles returns discovered [[role]] items (name order).
func (s *BundleService) ListRoles() ([]RoleInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	out := make([]RoleInfo, 0, len(cat.Roles))
	for _, r := range cat.Roles {
		out = append(out, RoleInfo{
			ID: r.ID, Name: r.Name, Hint: r.Hint, Code: r.Code, Mode: r.Mode,
			Tools: r.Tools, ModelSlot: r.ModelSlot,
			BundleName: r.BundleName, SourceRoot: r.SourceRoot,
		})
	}
	return out, nil
}

// ListLoops returns discovered [[loop]] items (name order).
func (s *BundleService) ListLoops() ([]LoopInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	out := make([]LoopInfo, 0, len(cat.Loops))
	for _, l := range cat.Loops {
		out = append(out, LoopInfo{
			ID: l.ID, Name: l.Name, Hint: l.Hint, Entry: l.Entry,
			BundleName: l.BundleName, SourceRoot: l.SourceRoot,
		})
	}
	return out, nil
}

// ListPolicies returns discovered [[policy]] items (name order).
func (s *BundleService) ListPolicies() ([]PolicyInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	out := make([]PolicyInfo, 0, len(cat.Policies))
	for _, p := range cat.Policies {
		out = append(out, PolicyInfo{
			ID: p.ID, Name: p.Name, Hint: p.Hint,
			MaxRounds: p.MaxRounds, OnMaxRounds: p.OnMaxRounds,
			PlanApproval: p.PlanApproval, ExtendRounds: p.ExtendRounds,
			BundleName: p.BundleName, SourceRoot: p.SourceRoot,
		})
	}
	return out, nil
}

// ListThemes returns TextMate themes from shipped + user bundles.
func (s *BundleService) ListThemes() ([]ThemeInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	out := make([]ThemeInfo, len(cat.Themes))
	for i, t := range cat.Themes {
		out[i] = toThemeInfo(t)
	}
	return out, nil
}

// ListTerminalThemes returns xterm.js ITheme palettes from shipped + user bundles.
func (s *BundleService) ListTerminalThemes() ([]TerminalThemeInfo, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, err
	}
	out := make([]TerminalThemeInfo, len(cat.TerminalThemes))
	for i, t := range cat.TerminalThemes {
		out[i] = toTerminalThemeInfo(t)
	}
	return out, nil
}

// ReadTheme returns the raw theme file contents for theme id.
func (s *BundleService) ReadTheme(id string) (string, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return "", err
	}
	t, ok := bundles.FindThemeByID(cat.Themes, id)
	if !ok {
		return "", fmt.Errorf("theme %q not found", id)
	}
	body, err := os.ReadFile(t.AbsPath)
	if err != nil {
		return "", fmt.Errorf("read theme %q: %w", id, err)
	}
	return string(body), nil
}

func (s *BundleService) discover() ([]bundles.Command, error) {
	cmds, _, err := s.discoverAll()
	return cmds, err
}

func (s *BundleService) discoverAll() ([]bundles.Command, []bundles.PromptAction, error) {
	cmds, prompts, _, err := s.discoverReport()
	return cmds, prompts, err
}

func (s *BundleService) discoverReport() ([]bundles.Command, []bundles.PromptAction, []bundles.DiscoverSkip, error) {
	cat, err := s.discoverCatalog()
	if err != nil {
		return nil, nil, nil, err
	}
	return cat.Commands, cat.Prompts, cat.Skips, nil
}

func catalogStamp(user, def string) string {
	var b strings.Builder
	b.WriteString(user)
	b.WriteByte(0)
	b.WriteString(def)
	for _, dir := range []string{def, user} {
		entries, err := os.ReadDir(dir)
		if err != nil {
			b.WriteByte(1)
			b.WriteString(err.Error())
			continue
		}
		for _, e := range entries {
			if !e.IsDir() {
				continue
			}
			p := filepath.Join(dir, e.Name(), "bundle.toml")
			fi, err := os.Stat(p)
			if err != nil {
				b.WriteByte(2)
				b.WriteString(p)
				continue
			}
			fmt.Fprintf(&b, "%s:%d:%d;", p, fi.ModTime().UnixNano(), fi.Size())
		}
	}
	return b.String()
}

// InvalidateCatalog forces the next listing to re-read every bundle, including
// script and palette files the bundle.toml stamp does not cover (Reload Bundles).
func (s *BundleService) InvalidateCatalog() {
	s.catMu.Lock()
	s.catStamp = ""
	s.catMu.Unlock()
}

func (s *BundleService) discoverCatalog() (bundles.Catalog, error) {
	def, err := store.BundlesDir()
	if err != nil {
		return bundles.Catalog{}, err
	}
	user, err := store.UserBundlesDir()
	if err != nil {
		return bundles.Catalog{}, err
	}
	stamp := catalogStamp(user, def)
	s.catMu.Lock()
	defer s.catMu.Unlock()
	if s.catStamp == stamp && s.catUser == user && s.catDef == def {
		return s.catalog, nil
	}
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		return bundles.Catalog{}, err
	}
	s.catalog = cat
	s.catStamp = stamp
	s.catUser = user
	s.catDef = def
	return cat, nil
}

func toGrammarInfo(g bundles.Grammar, lang []bundles.LanguageData) GrammarInfo {
	info := GrammarInfo{
		ScopeName: g.ScopeName, ID: g.ID, Path: g.Path, Language: g.Language,
		FileTypes: g.FileTypes, FirstLineMatch: g.FirstLineMatch, InjectTo: g.InjectTo,
		EmbeddedLanguages: g.EmbeddedLanguages, BundleName: g.BundleName, SourceRoot: g.SourceRoot,
	}
	if d, ok := bundles.ResolveLanguageData(lang, g.ScopeName); ok {
		info.LineComment = d.LineComment
		info.BlockCommentStart = d.BlockCommentStart
		info.BlockCommentEnd = d.BlockCommentEnd
		info.IncreaseIndentPattern = d.IncreaseIndentPattern
		info.DecreaseIndentPattern = d.DecreaseIndentPattern
		info.IndentNextLinePattern = d.IndentNextLinePattern
		info.ZeroIndentPattern = d.ZeroIndentPattern
		info.DisableIndentCorrections = d.DisableIndentCorrections
		info.CloseBrackets = d.CloseBrackets
	}
	return info
}

func toSnippetInfo(s bundles.Snippet) SnippetInfo {
	return SnippetInfo{
		ID: s.ID, Name: s.Name, TabTrigger: s.TabTrigger, Scope: s.Scope,
		FileGlob: s.FileGlob, Body: s.Body, KeyEquivalent: s.KeyEquivalent,
		Order: s.Order, BundleName: s.BundleName, SourceRoot: s.SourceRoot,
	}
}

func toSymbolInfo(s bundles.Symbol) SymbolInfo {
	return SymbolInfo{
		ID: s.ID, Scope: s.Scope, Transform: s.Transform, Kind: s.Kind,
		Show: s.Show, BundleName: s.BundleName, SourceRoot: s.SourceRoot,
	}
}

func toThemeInfo(t bundles.Theme) ThemeInfo {
	return ThemeInfo{
		ID: t.ID, Name: t.Name, Path: t.Path, UITheme: t.UITheme,
		Order: t.Order,
		Preview: ThemePreviewInfo{
			Bg: t.Preview.Bg, Fg: t.Preview.Fg, Fn: t.Preview.Fn,
			Name: t.Preview.Name, String: t.Preview.String,
		},
		BundleName: t.BundleName, SourceRoot: t.SourceRoot,
	}
}

func toTerminalThemeInfo(t bundles.TerminalTheme) TerminalThemeInfo {
	p := t.Palette
	return TerminalThemeInfo{
		ID: t.ID, Name: t.Name, Path: t.Path, UITheme: t.UITheme,
		Order: t.Order,
		Palette: TerminalPaletteInfo{
			Background: p.Background, Foreground: p.Foreground,
			Cursor: p.Cursor, CursorAccent: p.CursorAccent,
			SelectionBackground:         p.SelectionBackground,
			SelectionForeground:         p.SelectionForeground,
			SelectionInactiveBackground: p.SelectionInactiveBackground,
			Black:                       p.Black, Red: p.Red, Green: p.Green, Yellow: p.Yellow,
			Blue: p.Blue, Magenta: p.Magenta, Cyan: p.Cyan, White: p.White,
			BrightBlack: p.BrightBlack, BrightRed: p.BrightRed,
			BrightGreen: p.BrightGreen, BrightYellow: p.BrightYellow,
			BrightBlue: p.BrightBlue, BrightMagenta: p.BrightMagenta,
			BrightCyan: p.BrightCyan, BrightWhite: p.BrightWhite,
		},
		BundleName: t.BundleName, SourceRoot: t.SourceRoot,
	}
}

func toPromptActionInfo(p bundles.PromptAction) PromptActionInfo {
	args := make([]PromptArgInfo, 0, len(p.Args))
	for _, a := range p.Args {
		args = append(args, PromptArgInfo{Name: a.Name, Placeholder: a.Placeholder, Optional: a.Optional})
	}
	return PromptActionInfo{
		ID: p.ID, Name: p.Name, Description: p.Description, Slash: p.Slash,
		Mode: p.Mode, Loop: p.Loop, TimeoutMs: p.TimeoutMs, FreshSession: p.FreshSession,
		TabGroup:            p.TabGroup,
		Interpreter:         p.Interpreter,
		TakesArgs:           p.TakesArgs,
		HasComplete:         strings.TrimSpace(p.CompleteBody) != "",
		ArgPlaceholder:      p.ArgPlaceholder,
		CompleteInterpreter: p.CompleteInterpreter,
		IconSVG:             p.IconSVG,
		Args:                args,
		BundleName:          p.BundleName, SourceRoot: p.SourceRoot,
	}
}

// fillPromptGitEnv stamps the workspace merge base, update-with-base method,
// looking-at surface, and any open PR so shebang prompts (/update, /review,
// /open-pr) don't have to guess main vs origin/main or review the wrong subject.
func (s *BundleService) fillPromptGitEnv(rc *bundles.PromptRunContext, workspaceID string) {
	if rc == nil || workspaceID == "" {
		return
	}
	ws, ok := s.st().Workspace(workspaceID)
	if !ok {
		return
	}
	base := strings.TrimSpace(ws.BaseBranch)
	proj, pok := s.st().Project(ws.ProjectID)
	if base == "" && pok {
		base = strings.TrimSpace(proj.BaseBranch)
	}
	rc.BaseBranch = base
	if pok {
		rc.UpdateBaseMethod = string(domain.EffectiveUpdateBaseMethod(proj, s.st().Settings()))
	}
	switch strings.TrimSpace(strings.ToLower(rc.Surface)) {
	case "local_pr", "local-pr", "local":
		rc.Surface = "local_pr"
	case "github_pr", "github-pr", "gh", "github":
		rc.Surface = "github_pr"
	case "workspace":
		rc.Surface = "workspace"
	default:
		rc.Surface = ""
	}

	var pr domain.PR
	var hasPR bool
	if ws.IsPRChatHost() {
		rc.PRHost = true
		rc.Surface = "github_pr"
		pr, hasPR = s.st().PR(strings.TrimSpace(ws.HostPRID))
	} else if p, ok := s.st().PRByWorkspace(workspaceID); ok && p.State != "Merged" && p.Group != "merged" {
		pr, hasPR = p, true
	}
	if hasPR {
		rc.PRSource = string(pr.Source)
		rc.PRURL = pr.URL
		rc.PRTitle = pr.Title
		rc.PRBase = strings.TrimSpace(pr.Base)
		rc.PRBranch = strings.TrimSpace(pr.Branch)
		// Shebang fast-fail is GitHub-only. A Local fallback (gh create failed)
		// must leave /open-pr runnable so the agent can retry.
		if pr.Source == domain.SourceGitHub {
			rc.PRID = pr.ID
			rc.PRNumber = pr.Number
		} else {
			rc.PRNumber = pr.Number
		}
	}
	if rc.Surface == "" {
		rc.Surface = "workspace"
	}
}

func (s *BundleService) fillBundleSettingEnv(dst *map[string]string, bundleID string) {
	if dst == nil || strings.TrimSpace(bundleID) == "" {
		return
	}
	cat, err := s.discoverCatalog()
	if err != nil {
		return
	}
	set := s.st().Settings().MigrateBundleValues()
	out := make(map[string]string)
	for _, st := range bundles.SettingsForBundle(cat.Settings, bundleID) {
		val := set.BundleValue(st.ID)
		if val == "" {
			val = s.bundleSettingProcessFallback(st.ID)
		}
		if val != "" {
			out[st.Name] = val
		}
	}
	if len(out) == 0 {
		return
	}
	*dst = out
}

func (s *BundleService) bundleSettingProcessFallback(id string) string {
	switch id {
	case domain.SettingLinearAPIKey:
		return strings.TrimSpace(os.Getenv("LINEAR_API_KEY"))
	case domain.SettingLinearTeamKey:
		return strings.TrimSpace(os.Getenv("LINEAR_TEAM_KEY"))
	default:
		return ""
	}
}

func toPromptRunContext(rc PromptContextDTO, prompt bundles.PromptAction) bundles.PromptRunContext {
	workDir := rc.WorkDir
	if workDir == "" {
		workDir = rc.Worktree
	}
	params := rc.Params
	if len(params) == 0 {
		params = bundles.ParamsFromArgs(prompt, rc.Args)
	}
	return bundles.PromptRunContext{
		WorkspaceID:   rc.WorkspaceID,
		ProjectID:     rc.ProjectID,
		Worktree:      rc.Worktree,
		Port:          rc.Port,
		SelectedText:  rc.SelectedText,
		Document:      rc.Document,
		CurrentLine:   rc.CurrentLine,
		LineNumber:    rc.LineNumber,
		Column:        rc.Column,
		CurrentWord:   rc.CurrentWord,
		FilePath:      rc.FilePath,
		FileName:      rc.FileName,
		Scope:         rc.Scope,
		Clipboard:     rc.Clipboard,
		TabSize:       rc.TabSize,
		SoftTabs:      rc.SoftTabs,
		PromptSlash:   prompt.Slash,
		Args:          rc.Args,
		Params:        params,
		PRDraft:       prompt.Slash == "open-draft-pr",
		Surface:       rc.Surface,
		BundleSupport: prompt.BundleDir,
		WorkDir:       workDir,
	}
}

func toCommandInfo(c bundles.Command) CommandInfo {
	prompts := make([]CommandPromptDTO, len(c.Prompts))
	for i, p := range c.Prompts {
		prompts[i] = CommandPromptDTO{
			Name: p.Name, Label: p.Label, Type: p.Type,
			Default: p.Default, Choices: p.Choices,
		}
	}
	return CommandInfo{
		ID: c.ID, Name: c.Name, Category: c.Category, Order: c.Order,
		Scope: c.Scope, FileGlob: c.FileGlob, Input: c.Input,
		FallbackInput: c.FallbackInput, Output: c.Output, Triggers: c.Triggers,
		Prompts: prompts, KeyEquivalent: c.KeyEquivalent, Language: c.Language,
		OpenInPreview: c.OpenInPreview,
		Interpreter:   c.Interpreter, BundleName: c.BundleName, SourceRoot: c.SourceRoot,
	}
}

func toRunContext(rc RunContextDTO, cmd bundles.Command) bundles.RunContext {
	return bundles.RunContext{
		SelectedText:     rc.SelectedText,
		CurrentLine:      rc.CurrentLine,
		LineNumber:       rc.LineNumber,
		ColumnNumber:     rc.ColumnNumber,
		CurrentWord:      rc.CurrentWord,
		FilePath:         rc.FilePath,
		FileName:         rc.FileName,
		Directory:        rc.Directory,
		ProjectDirectory: rc.ProjectDirectory,
		Scope:            rc.Scope,
		TabSize:          rc.TabSize,
		SoftTabs:         rc.SoftTabs,
		Clipboard:        rc.Clipboard,
		BundleSupport:    cmd.BundleDir,
		Input:            rc.Input,
		PromptValues:     rc.PromptValues,
		WorkDir:          rc.WorkDir,
	}
}
