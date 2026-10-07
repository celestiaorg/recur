package bundles

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	toml "github.com/pelletier/go-toml/v2"
)

// TestCase is one [[test]] entry under a bundle's tests/*.toml.
// Authors declare context + expect; the runner executes the referenced command
// or compares a snippet body (command XOR snippet).
//
// Text payloads may be inline strings or optional *File paths (relative to the
// TOML file's directory). Setting both for the same field is an error.
type TestCase struct {
	Name    string      `toml:"name"`
	Command string      `toml:"command"` // command id (preferred) or name
	Snippet string      `toml:"snippet"` // snippet id; mutually exclusive with command
	Context TestContext `toml:"context"`
	Expect  TestExpect  `toml:"expect"`
}

// TestContext is the editor state fed to the command (MacroContext / RunContext).
type TestContext struct {
	Input            string            `toml:"input"`
	InputFile        string            `toml:"inputFile"`
	SelectedText     string            `toml:"selectedText"`
	SelectedTextFile string            `toml:"selectedTextFile"`
	Document         string            `toml:"document"`
	DocumentFile     string            `toml:"documentFile"`
	CurrentLine      string            `toml:"currentLine"`
	CurrentLineFile  string            `toml:"currentLineFile"`
	CurrentWord      string            `toml:"currentWord"`
	CurrentWordFile  string            `toml:"currentWordFile"`
	LineNumber       int               `toml:"lineNumber"`
	Column           int               `toml:"column"`
	FilePath         string            `toml:"filepath"`
	FileName         string            `toml:"filename"`
	Scope            string            `toml:"scope"`
	Clipboard        string            `toml:"clipboard"`
	ClipboardFile    string            `toml:"clipboardFile"`
	TabSize          int               `toml:"tabSize"`
	SoftTabs         bool              `toml:"softTabs"`
	Prompts          map[string]string `toml:"prompts"`
}

// TestExpect is the assertion block for a case.
type TestExpect struct {
	Stdout     string `toml:"stdout"`
	StdoutFile string `toml:"stdoutFile"`
	Stderr     string `toml:"stderr"`
	StderrFile string `toml:"stderrFile"`
	Output     string `toml:"output"`     // optional routing override check
	ExitCode   *int   `toml:"exitCode"`   // default 0
	SkipStdout bool   `toml:"skipStdout"` // for non-deterministic commands (e.g. shuffle)
	// Language / OpenInPreview assert typescript MacroResult fields (optional).
	Language      string `toml:"language"`
	OpenInPreview *bool  `toml:"openInPreview"`
}

type testFile struct {
	Tests []TestCase `toml:"test"`
}

// LoadedTest is a case plus the TOML file it came from (for reporting).
type LoadedTest struct {
	File string
	Case TestCase
}

// LoadTests reads tests/*.toml under bundleDir. Missing tests/ is not an error.
// File payloads (*File fields) are resolved relative to each TOML file's directory.
func LoadTests(bundleDir string) ([]LoadedTest, error) {
	dir := filepath.Join(bundleDir, "tests")
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, fmt.Errorf("read tests/: %w", err)
	}
	var out []LoadedTest
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".toml") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, fmt.Errorf("read %s: %w", path, err)
		}
		var tf testFile
		if err := toml.Unmarshal(data, &tf); err != nil {
			return nil, fmt.Errorf("parse %s: %w", path, err)
		}
		base := filepath.Dir(path)
		for i, tc := range tf.Tests {
			if strings.TrimSpace(tc.Command) == "" && strings.TrimSpace(tc.Snippet) == "" {
				return nil, fmt.Errorf("%s: test[%d]: command or snippet is required", path, i)
			}
			if strings.TrimSpace(tc.Command) != "" && strings.TrimSpace(tc.Snippet) != "" {
				return nil, fmt.Errorf("%s: test[%d]: set command or snippet, not both", path, i)
			}
			if strings.TrimSpace(tc.Name) == "" {
				tc.Name = fmt.Sprintf("%s#%d", e.Name(), i+1)
			}
			if err := resolveTestFiles(&tc, base); err != nil {
				return nil, fmt.Errorf("%s: test %q: %w", path, tc.Name, err)
			}
			out = append(out, LoadedTest{File: path, Case: tc})
		}
	}
	return out, nil
}

func resolveTestFiles(tc *TestCase, baseDir string) error {
	var err error
	if tc.Context.Input, err = resolvePayload(tc.Context.Input, tc.Context.InputFile, baseDir, "input"); err != nil {
		return err
	}
	tc.Context.InputFile = ""
	if tc.Context.SelectedText, err = resolvePayload(tc.Context.SelectedText, tc.Context.SelectedTextFile, baseDir, "selectedText"); err != nil {
		return err
	}
	tc.Context.SelectedTextFile = ""
	if tc.Context.Document, err = resolvePayload(tc.Context.Document, tc.Context.DocumentFile, baseDir, "document"); err != nil {
		return err
	}
	tc.Context.DocumentFile = ""
	if tc.Context.CurrentLine, err = resolvePayload(tc.Context.CurrentLine, tc.Context.CurrentLineFile, baseDir, "currentLine"); err != nil {
		return err
	}
	tc.Context.CurrentLineFile = ""
	if tc.Context.CurrentWord, err = resolvePayload(tc.Context.CurrentWord, tc.Context.CurrentWordFile, baseDir, "currentWord"); err != nil {
		return err
	}
	tc.Context.CurrentWordFile = ""
	if tc.Context.Clipboard, err = resolvePayload(tc.Context.Clipboard, tc.Context.ClipboardFile, baseDir, "clipboard"); err != nil {
		return err
	}
	tc.Context.ClipboardFile = ""
	if tc.Expect.Stdout, err = resolvePayload(tc.Expect.Stdout, tc.Expect.StdoutFile, baseDir, "stdout"); err != nil {
		return err
	}
	tc.Expect.StdoutFile = ""
	if tc.Expect.Stderr, err = resolvePayload(tc.Expect.Stderr, tc.Expect.StderrFile, baseDir, "stderr"); err != nil {
		return err
	}
	tc.Expect.StderrFile = ""
	return nil
}

// resolvePayload returns inline text, or the contents of relFile when set.
// relFile is relative to baseDir (the test TOML's directory).
func resolvePayload(inline, relFile, baseDir, field string) (string, error) {
	relFile = strings.TrimSpace(relFile)
	if relFile == "" {
		return inline, nil
	}
	if inline != "" {
		return "", fmt.Errorf("%s: set inline value or %sFile, not both", field, field)
	}
	if filepath.IsAbs(relFile) {
		return "", fmt.Errorf("%sFile must be relative (got %q)", field, relFile)
	}
	path := filepath.Clean(filepath.Join(baseDir, relFile))
	baseClean := filepath.Clean(baseDir)
	sep := string(filepath.Separator)
	if path != baseClean && !strings.HasPrefix(path, baseClean+sep) {
		return "", fmt.Errorf("%sFile %q escapes test directory", field, relFile)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("%sFile %q: %w", field, relFile, err)
	}
	return string(data), nil
}

func findCommand(cmds []Command, id string) (Command, bool) {
	for _, c := range cmds {
		if c.ID == id || c.Name == id {
			return c, true
		}
	}
	return Command{}, false
}
