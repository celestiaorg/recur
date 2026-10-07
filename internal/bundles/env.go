package bundles

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"
)

// RunContext is the editor state passed into a script execution.
type RunContext struct {
	SelectedText     string
	CurrentLine      string
	LineNumber       int
	ColumnNumber     int
	CurrentWord      string
	FilePath         string // absolute or workspace-relative path shown to the user
	FileName         string
	Directory        string
	ProjectDirectory string
	Scope            string
	TabSize          int
	SoftTabs         bool
	Clipboard        string
	BundleSupport    string // absolute path to the bundle directory
	Input            string // text piped to stdin (resolved from input/fallback)
	PromptValues     map[string]string
	WorkDir          string // cmd.Dir — worktree / project root
	// BundleSettings is name → value for this command's bundle [[setting]]
	// entries (shebang: RECUR_SETTING_<NAME> + RECUR_BUNDLE_SETTINGS JSON).
	BundleSettings map[string]string
}

// PromptRunContext is workspace + editor state for a shebang [[prompt]] action.
type PromptRunContext struct {
	WorkspaceID string
	ProjectID   string
	Worktree    string
	Port        int

	SelectedText string
	Document     string
	CurrentLine  string
	LineNumber   int
	Column       int
	CurrentWord  string
	FilePath     string
	FileName     string
	Scope        string
	Clipboard    string
	TabSize      int
	SoftTabs     bool

	// BaseBranch is the workspace merge base (main, origin/main, …).
	BaseBranch string
	// UpdateBaseMethod is "merge" or "rebase" (project/settings prefs).
	UpdateBaseMethod string
	// Open (non-merged) PR for this workspace, if any. /open-pr fast-fails
	// when PRID is set so a second invoke does not start an agent turn.
	PRID     string
	PRNumber int
	PRSource string
	// PRDraft is true when /open-draft-pr invoked this shebang.
	PRDraft bool
	// Args is the trailing composer remainder after `/slash` (empty when none).
	// For complete scripts this is the active slot's value.
	Args string
	// CompleteArg is the [[prompt.arg]] name being completed (empty on invoke).
	CompleteArg string
	// SearchQuery is the ⌘K query (RECUR_SEARCH_QUERY); also copied to Args.
	SearchQuery string
	// Params are named [[prompt.arg]] values (same data as Args, structured).
	Params map[string]string
	// BundleSettings is name → value for this prompt's bundle [[setting]]
	// entries. Injected as RECUR_SETTING_<NAME> and RECUR_BUNDLE_SETTINGS JSON.
	// TypeScript Workers never see these values.
	BundleSettings map[string]string
	// PromptSlash is the [[prompt]] slash that launched this run.
	PromptSlash string
	// Surface is the operator's looking-at view: workspace, local_pr, github_pr.
	Surface  string
	PRURL    string
	PRTitle  string
	PRBase   string
	PRBranch string
	// PRHost is true when cwd is a PR chat host (project checkout, not the PR head).
	PRHost bool
	// ReviewSince/ReviewNow are checkpoint commits for a /re-review that
	// continues an earlier review on the same chat: embed `git diff since now`
	// instead of the full branch diff.
	ReviewSince string
	ReviewNow   string

	BundleSupport string
	WorkDir       string // cmd.Dir — usually worktree
}

// Environ maps RunContext to the RECUR_* environment (plus inherited PATH).
func Environ(rc RunContext) []string {
	base := os.Environ()
	// Strip any pre-existing RECUR_* so stale values cannot leak in.
	filtered := make([]string, 0, len(base)+24)
	for _, e := range base {
		if strings.HasPrefix(e, "RECUR_") {
			continue
		}
		filtered = append(filtered, e)
	}
	soft := "0"
	if rc.SoftTabs {
		soft = "1"
	}
	fileName := rc.FileName
	if fileName == "" && rc.FilePath != "" {
		fileName = filepath.Base(rc.FilePath)
	}
	dir := rc.Directory
	if dir == "" && rc.FilePath != "" {
		dir = filepath.Dir(rc.FilePath)
	}
	kv := [][2]string{
		{"RECUR_SELECTED_TEXT", rc.SelectedText},
		{"RECUR_CURRENT_LINE", rc.CurrentLine},
		{"RECUR_LINE_NUMBER", strconv.Itoa(rc.LineNumber)},
		{"RECUR_COLUMN_NUMBER", strconv.Itoa(rc.ColumnNumber)},
		{"RECUR_CURRENT_WORD", rc.CurrentWord},
		{"RECUR_FILEPATH", rc.FilePath},
		{"RECUR_FILENAME", fileName},
		{"RECUR_DIRECTORY", dir},
		{"RECUR_PROJECT_DIRECTORY", rc.ProjectDirectory},
		{"RECUR_SCOPE", rc.Scope},
		{"RECUR_TAB_SIZE", strconv.Itoa(rc.TabSize)},
		{"RECUR_SOFT_TABS", soft},
		{"RECUR_CLIPBOARD", rc.Clipboard},
		{"RECUR_BUNDLE_SUPPORT", rc.BundleSupport},
	}
	for _, pair := range kv {
		filtered = append(filtered, pair[0]+"="+pair[1])
	}
	filtered = appendBundleSettingEnv(filtered, rc.BundleSettings)
	for name, val := range rc.PromptValues {
		key := "RECUR_INPUT_" + sanitizeInputName(name)
		filtered = append(filtered, key+"="+val)
	}
	return filtered
}

// Cap large PromptEnviron blobs so argv+env stays under OS ARG_MAX (~1MB on
// macOS). Authors see a truncation note; full document remains available to
// TypeScript prompts via the Worker JSON context (not this env path).
const promptEnvBlobMax = 64 * 1024

func truncatePromptEnvBlob(s string) string {
	if len(s) <= promptEnvBlobMax {
		return s
	}
	return s[:promptEnvBlobMax] + "\n…(truncated; over PromptEnviron cap)…"
}

// ParamsFromArgs splits the trailing remainder into named [[prompt.arg]] values.
// One arg keeps spaces; multiple args split on whitespace with the remainder on the last.
func ParamsFromArgs(prompt PromptAction, args string) map[string]string {
	specs := prompt.Args
	if len(specs) == 0 {
		return nil
	}
	raw := strings.TrimSpace(args)
	out := make(map[string]string, len(specs))
	if len(specs) == 1 {
		out[specs[0].Name] = raw
		return out
	}
	parts := splitPromptArgTokens(raw)
	for i, spec := range specs {
		if i == len(specs)-1 {
			if i < len(parts) {
				out[spec.Name] = strings.Join(parts[i:], " ")
			} else {
				out[spec.Name] = ""
			}
			continue
		}
		if i < len(parts) {
			out[spec.Name] = parts[i]
		} else {
			out[spec.Name] = ""
		}
	}
	return out
}

func splitPromptArgTokens(s string) []string {
	s = strings.TrimSpace(s)
	var out []string
	i := 0
	for i < len(s) {
		for i < len(s) && (s[i] == ' ' || s[i] == '\t' || s[i] == '\n' || s[i] == '\r') {
			i++
		}
		if i >= len(s) {
			break
		}
		if s[i] == '"' {
			i++
			var b strings.Builder
			for i < len(s) {
				if s[i] == '\\' && i+1 < len(s) {
					b.WriteByte(s[i+1])
					i += 2
					continue
				}
				if s[i] == '"' {
					i++
					break
				}
				b.WriteByte(s[i])
				i++
			}
			out = append(out, b.String())
			continue
		}
		j := i
		for j < len(s) && s[j] != ' ' && s[j] != '\t' && s[j] != '\n' && s[j] != '\r' {
			j++
		}
		out = append(out, s[i:j])
		i = j
	}
	return out
}

// PromptEnviron maps PromptRunContext to RECUR_* env (wsenv-style workspace
// vars plus editor vars matching Environ). Stale RECUR_* from the process are stripped.
func PromptEnviron(rc PromptRunContext) []string {
	base := os.Environ()
	filtered := make([]string, 0, len(base)+32)
	for _, e := range base {
		if strings.HasPrefix(e, "RECUR_") || strings.HasPrefix(e, "PORT=") {
			continue
		}
		filtered = append(filtered, e)
	}
	soft := "0"
	if rc.SoftTabs {
		soft = "1"
	}
	draft := "0"
	if rc.PRDraft {
		draft = "1"
	}
	fileName := rc.FileName
	if fileName == "" && rc.FilePath != "" {
		fileName = filepath.Base(rc.FilePath)
	}
	kv := [][2]string{
		{"RECUR_WORKSPACE_ID", rc.WorkspaceID},
		{"RECUR_PROJECT_ID", rc.ProjectID},
		{"RECUR_WORKTREE", rc.Worktree},
		{"RECUR_SELECTED_TEXT", truncatePromptEnvBlob(rc.SelectedText)},
		{"RECUR_DOCUMENT", truncatePromptEnvBlob(rc.Document)},
		{"RECUR_CURRENT_LINE", truncatePromptEnvBlob(rc.CurrentLine)},
		{"RECUR_LINE_NUMBER", strconv.Itoa(rc.LineNumber)},
		{"RECUR_COLUMN_NUMBER", strconv.Itoa(rc.Column)},
		{"RECUR_CURRENT_WORD", truncatePromptEnvBlob(rc.CurrentWord)},
		{"RECUR_FILEPATH", rc.FilePath},
		{"RECUR_FILENAME", fileName},
		{"RECUR_SCOPE", rc.Scope},
		{"RECUR_TAB_SIZE", strconv.Itoa(rc.TabSize)},
		{"RECUR_SOFT_TABS", soft},
		{"RECUR_CLIPBOARD", truncatePromptEnvBlob(rc.Clipboard)},
		{"RECUR_BUNDLE_SUPPORT", rc.BundleSupport},
		{"RECUR_BASE_BRANCH", rc.BaseBranch},
		{"RECUR_UPDATE_BASE_METHOD", rc.UpdateBaseMethod},
		{"RECUR_PR_ID", rc.PRID},
		{"RECUR_PR_NUMBER", strconv.Itoa(rc.PRNumber)},
		{"RECUR_PR_SOURCE", rc.PRSource},
		{"RECUR_PR_URL", rc.PRURL},
		{"RECUR_PR_TITLE", rc.PRTitle},
		{"RECUR_PR_BASE", rc.PRBase},
		{"RECUR_PR_BRANCH", rc.PRBranch},
		{"RECUR_PR_DRAFT", draft},
		{"RECUR_PROMPT_SLASH", rc.PromptSlash},
		{"RECUR_PROMPT_ARGS", truncatePromptEnvBlob(rc.Args)},
		{"RECUR_PROMPT_COMPLETE_ARG", strings.TrimSpace(rc.CompleteArg)},
		{"RECUR_SEARCH_QUERY", truncatePromptEnvBlob(rc.SearchQuery)},
		{"RECUR_SURFACE", rc.Surface},
	}
	if len(rc.Params) > 0 {
		names := make([]string, 0, len(rc.Params))
		for name := range rc.Params {
			names = append(names, name)
		}
		sort.Strings(names)
		obj := make(map[string]string, len(rc.Params))
		for _, name := range names {
			val := strings.TrimSpace(rc.Params[name])
			obj[name] = val
			envName := "RECUR_PROMPT_ARG_" + strings.ToUpper(name)
			kv = append(kv, [2]string{envName, truncatePromptEnvBlob(val)})
		}
		if b, err := json.Marshal(obj); err == nil {
			kv = append(kv, [2]string{"RECUR_PROMPT_PARAMS", truncatePromptEnvBlob(string(b))})
		}
	}
	if rc.PRHost {
		kv = append(kv, [2]string{"RECUR_PR_HOST", "1"})
	}
	if rc.ReviewSince != "" && rc.ReviewNow != "" {
		kv = append(kv, [2]string{"RECUR_REVIEW_SINCE", rc.ReviewSince}, [2]string{"RECUR_REVIEW_NOW", rc.ReviewNow})
	}
	if rc.Port > 0 {
		p := strconv.Itoa(rc.Port)
		kv = append(kv, [2]string{"RECUR_PORT", p}, [2]string{"PORT", p})
	}
	for _, pair := range kv {
		filtered = append(filtered, pair[0]+"="+pair[1])
	}
	return appendBundleSettingEnv(filtered, rc.BundleSettings)
}

func appendBundleSettingEnv(filtered []string, values map[string]string) []string {
	if len(values) == 0 {
		return filtered
	}
	names := make([]string, 0, len(values))
	for name := range values {
		names = append(names, name)
	}
	sort.Strings(names)
	obj := make(map[string]string, len(names))
	for _, name := range names {
		val := strings.TrimSpace(values[name])
		if val == "" {
			continue
		}
		obj[name] = val
		filtered = append(filtered, SettingEnvName(name)+"="+truncatePromptEnvBlob(val))
	}
	if len(obj) == 0 {
		return filtered
	}
	if b, err := json.Marshal(obj); err == nil {
		filtered = append(filtered, "RECUR_BUNDLE_SETTINGS="+truncatePromptEnvBlob(string(b)))
	}
	return filtered
}

// NormalizePromptDisplay coerces display to a single line ≤ PromptDisplayMaxLen.
// Empty display falls back to fallbackName (typically the prompt action name).
func NormalizePromptDisplay(display, fallbackName string) string {
	d := strings.TrimSpace(display)
	if d == "" {
		d = strings.TrimSpace(fallbackName)
	}
	d = strings.ReplaceAll(d, "\r\n", " ")
	d = strings.ReplaceAll(d, "\n", " ")
	d = strings.ReplaceAll(d, "\r", " ")
	for strings.Contains(d, "  ") {
		d = strings.ReplaceAll(d, "  ", " ")
	}
	d = strings.TrimSpace(d)
	if utf8.RuneCountInString(d) <= PromptDisplayMaxLen {
		return d
	}
	runes := []rune(d)
	if PromptDisplayMaxLen <= 1 {
		return string(runes[:PromptDisplayMaxLen])
	}
	return string(runes[:PromptDisplayMaxLen-1]) + "…"
}

// sanitizeInputName uppercases and replaces non-alnum with underscore.
func sanitizeInputName(name string) string {
	var b strings.Builder
	for _, r := range strings.ToUpper(name) {
		if (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') {
			b.WriteRune(r)
		} else {
			b.WriteByte('_')
		}
	}
	out := b.String()
	if out == "" {
		return "VALUE"
	}
	return out
}

// ResolveInput picks stdin text from the command's input/fallbackInput modes.
// When rc.Input is already set (frontend resolved document/selection), it wins.
func ResolveInput(cmd Command, rc RunContext) string {
	if rc.Input != "" {
		return rc.Input
	}
	pick := func(mode string) string {
		switch mode {
		case InputNone, "":
			return ""
		case InputSelection:
			return rc.SelectedText
		case InputDocument:
			return rc.SelectedText // frontend should set Input for full docs
		case InputLine:
			return rc.CurrentLine
		case InputWord:
			return rc.CurrentWord
		default:
			return rc.SelectedText
		}
	}
	v := pick(cmd.Input)
	if v == "" && cmd.FallbackInput != "" {
		v = pick(cmd.FallbackInput)
	}
	return v
}
