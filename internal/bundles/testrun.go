package bundles

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// CaseResult is one executed [[test]] outcome.
type CaseResult struct {
	Bundle string
	File   string
	Name   string
	OK     bool
	Detail string // failure explanation when !OK
}

// Report aggregates case results for a run.
type Report struct {
	Results []CaseResult
	Passed  int
	Failed  int
}

// OK reports whether every case passed.
func (r Report) OK() bool { return r.Failed == 0 }

// RunBundleTests loads bundleDir's manifest + tests/*.toml and executes each case.
// cacheDir holds CompileMacro output (use a temp dir for one-off CLI runs).
func RunBundleTests(ctx context.Context, bundleDir, cacheDir string) (Report, error) {
	abs, err := filepath.Abs(bundleDir)
	if err != nil {
		return Report{}, err
	}
	bundleDir = abs
	data, err := os.ReadFile(filepath.Join(bundleDir, "bundle.toml"))
	if err != nil {
		return Report{}, fmt.Errorf("read bundle.toml: %w", err)
	}
	parsed, err := ParseManifestFull(bundleDir, data)
	if err != nil {
		return Report{}, err
	}
	meta, cmds := parsed.Meta, parsed.Commands
	tests, err := LoadTests(bundleDir)
	if err != nil {
		return Report{}, err
	}
	var report Report
	for _, lt := range tests {
		var cr CaseResult
		if strings.TrimSpace(lt.Case.Snippet) != "" {
			cr = runOneSnippetTest(meta.Name, parsed.Snippets, lt)
		} else {
			cr = runOneTest(ctx, meta.Name, cacheDir, cmds, lt)
		}
		report.Results = append(report.Results, cr)
		if cr.OK {
			report.Passed++
		} else {
			report.Failed++
		}
	}
	return report, nil
}

func runOneTest(ctx context.Context, bundleName, cacheDir string, cmds []Command, lt LoadedTest) CaseResult {
	cr := CaseResult{
		Bundle: bundleName,
		File:   lt.File,
		Name:   lt.Case.Name,
	}
	cmd, ok := findCommand(cmds, lt.Case.Command)
	if !ok {
		cr.Detail = fmt.Sprintf("unknown command %q", lt.Case.Command)
		return cr
	}
	res, err := executeTestCommand(ctx, cacheDir, cmd, lt.Case.Context)
	if err != nil {
		cr.Detail = err.Error()
		return cr
	}
	if detail := checkExpect(lt.Case.Expect, res); detail != "" {
		cr.Detail = detail
		return cr
	}
	cr.OK = true
	return cr
}

func runOneSnippetTest(bundleName string, snippets []Snippet, lt LoadedTest) CaseResult {
	cr := CaseResult{
		Bundle: bundleName,
		File:   lt.File,
		Name:   lt.Case.Name,
	}
	sn, ok := FindSnippetByID(snippets, lt.Case.Snippet)
	if !ok {
		cr.Detail = fmt.Sprintf("unknown snippet %q", lt.Case.Snippet)
		return cr
	}
	got := strings.TrimSpace(sn.Body)
	want := strings.TrimSpace(lt.Case.Expect.Stdout)
	if want == "" && !lt.Case.Expect.SkipStdout {
		cr.Detail = "snippet tests must set expect.stdout to the expansion body"
		return cr
	}
	if !lt.Case.Expect.SkipStdout && got != want {
		cr.Detail = fmt.Sprintf("body mismatch:\n got %q\nwant %q", got, want)
		return cr
	}
	cr.OK = true
	return cr
}

func executeTestCommand(ctx context.Context, cacheDir string, cmd Command, tc TestContext) (RunResult, error) {
	switch {
	case IsScriptInterpreter(cmd.Interpreter):
		return runTypeScriptMacro(ctx, cacheDir, cmd, tc)
	case cmd.Interpreter == InterpreterShebang:
		rc := runContextFromTest(tc, cmd.BundleDir)
		return Run(ctx, cmd, rc)
	default:
		return RunResult{}, fmt.Errorf("interpreter %q cannot be tested", cmd.Interpreter)
	}
}

func runContextFromTest(tc TestContext, bundleDir string) RunContext {
	input := tc.Input
	selected := tc.SelectedText
	if selected == "" {
		selected = input
	}
	word := tc.CurrentWord
	if word == "" {
		word = input
	}
	tab := tc.TabSize
	if tab == 0 {
		tab = 4
	}
	return RunContext{
		SelectedText:     selected,
		CurrentLine:      tc.CurrentLine,
		LineNumber:       tc.LineNumber,
		ColumnNumber:     tc.Column,
		CurrentWord:      word,
		FilePath:         tc.FilePath,
		FileName:         tc.FileName,
		Scope:            tc.Scope,
		TabSize:          tab,
		SoftTabs:         tc.SoftTabs,
		Clipboard:        tc.Clipboard,
		BundleSupport:    bundleDir,
		Input:            input,
		PromptValues:     tc.Prompts,
		ProjectDirectory: filepath.Dir(bundleDir),
		WorkDir:          filepath.Dir(bundleDir),
	}
}

func checkExpect(exp TestExpect, res RunResult) string {
	wantExit := 0
	if exp.ExitCode != nil {
		wantExit = *exp.ExitCode
	}
	if res.ExitCode != wantExit {
		return fmt.Sprintf("exitCode = %d, want %d (stderr=%q)", res.ExitCode, wantExit, res.Stderr)
	}
	if !exp.SkipStdout {
		if exp.Stdout != "" || res.Stdout != "" {
			if res.Stdout != exp.Stdout {
				return fmt.Sprintf("stdout mismatch:\n got %q\nwant %q", res.Stdout, exp.Stdout)
			}
		}
	}
	if exp.Output != "" && res.Output != exp.Output {
		return fmt.Sprintf("output = %q, want %q", res.Output, exp.Output)
	}
	if exp.Stderr != "" && res.Stderr != exp.Stderr {
		return fmt.Sprintf("stderr = %q, want %q", res.Stderr, exp.Stderr)
	}
	if exp.Language != "" && res.Language != exp.Language {
		return fmt.Sprintf("language = %q, want %q", res.Language, exp.Language)
	}
	if exp.OpenInPreview != nil {
		got := false
		if res.OpenInPreview != nil {
			got = *res.OpenInPreview
		}
		if got != *exp.OpenInPreview {
			return fmt.Sprintf("openInPreview = %v, want %v", got, *exp.OpenInPreview)
		}
	}
	return ""
}

// ANSI colors for FormatReport (disabled when color=false).
const (
	ansiReset = "\033[0m"
	ansiGreen = "\033[32m"
	ansiRed   = "\033[31m"
	ansiBold  = "\033[1m"
	ansiDim   = "\033[2m"
)

// FormatReport renders a human-readable summary. When color is true, status
// tokens use ANSI (green ok, red FAIL, tinted summary).
func FormatReport(r Report, color bool) string {
	paint := func(code, s string) string {
		if !color || code == "" {
			return s
		}
		return code + s + ansiReset
	}
	var b strings.Builder
	for _, c := range r.Results {
		rel := c.File
		if c.OK {
			fmt.Fprintf(&b, "%s\t%s\t%s\n", paint(ansiGreen, "ok"), c.Bundle, c.Name)
			continue
		}
		fmt.Fprintf(&b, "%s\t%s\t%s\n", paint(ansiBold+ansiRed, "FAIL"), c.Bundle, c.Name)
		fmt.Fprintf(&b, "  %s\n", paint(ansiRed, c.Detail))
		fmt.Fprintf(&b, "  %s\n", paint(ansiDim, "("+rel+")"))
	}
	pass := fmt.Sprintf("%d passed", r.Passed)
	fail := fmt.Sprintf("%d failed", r.Failed)
	if r.Failed == 0 {
		fmt.Fprintf(&b, "%s, %s\n", paint(ansiGreen, pass), fail)
	} else {
		fmt.Fprintf(&b, "%s, %s\n", pass, paint(ansiBold+ansiRed, fail))
	}
	return b.String()
}
