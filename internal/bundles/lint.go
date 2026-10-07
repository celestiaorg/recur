package bundles

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// LintSeverity is error (fails lint) or info (advisory).
type LintSeverity string

const (
	LintError LintSeverity = "error"
	LintInfo  LintSeverity = "info"
)

// LintFinding is one diagnostic from LintBundle / LintBundles.
type LintFinding struct {
	Severity LintSeverity
	Bundle   string
	Command  string // command/prompt name or uuid; empty for bundle-level
	Path     string // bundle dir or file
	Message  string
}

// LintReport aggregates findings across one or more bundles.
type LintReport struct {
	Findings []LintFinding
	Errors   int
	Infos    int
}

// OK is true when there are no error-severity findings.
func (r LintReport) OK() bool { return r.Errors == 0 }

// LintBundles lints each bundle directory and cross-checks shared keyEquivalents.
func LintBundles(dirs []string, cacheDir string) (LintReport, error) {
	var rep LintReport
	type claim struct {
		canonical string
		bundle    string
		cmdName   string
		cmdID     string
		dir       string
	}
	var claims []claim

	type slashClaim struct {
		slash  string
		bundle string
		name   string
		uuid   string
		dir    string
	}
	var slashClaims []slashClaim

	for _, dir := range dirs {
		meta, cmds, prompts, snippets, findings, err := lintOneBundle(dir, cacheDir)
		if err != nil {
			rep.add(LintFinding{
				Severity: LintError,
				Bundle:   filepath.Base(dir),
				Path:     dir,
				Message:  err.Error(),
			})
			continue
		}
		for _, f := range findings {
			rep.add(f)
		}
		addKeyClaims := func(raw, name, id string) {
			raw = strings.TrimSpace(raw)
			if raw == "" {
				return
			}
			can, err := ParseKeyEquivalent(raw)
			if err != nil {
				return
			}
			if IsReservedAppChord(can) {
				return
			}
			claims = append(claims, claim{
				canonical: can,
				bundle:    meta.Name,
				cmdName:   name,
				cmdID:     id,
				dir:       dir,
			})
		}
		for _, c := range cmds {
			addKeyClaims(c.KeyEquivalent, c.Name, c.ID)
		}
		for _, s := range snippets {
			addKeyClaims(s.KeyEquivalent, s.Name, s.ID)
		}
		for _, p := range prompts {
			slashClaims = append(slashClaims, slashClaim{
				slash: p.Slash, bundle: meta.Name, name: p.Name, uuid: p.ID, dir: dir,
			})
		}
	}

	byChord := map[string][]claim{}
	for _, c := range claims {
		byChord[c.canonical] = append(byChord[c.canonical], c)
	}
	chords := make([]string, 0, len(byChord))
	for c := range byChord {
		chords = append(chords, c)
	}
	sort.Strings(chords)
	for _, chord := range chords {
		list := byChord[chord]
		if len(list) < 2 {
			continue
		}
		var names []string
		for _, c := range list {
			names = append(names, fmt.Sprintf("%s/%s", c.bundle, c.cmdName))
		}
		sort.Strings(names)
		rep.add(LintFinding{
			Severity: LintInfo,
			Path:     list[0].dir,
			Message:  fmt.Sprintf("keyEquivalent %q shared by %s (disambiguation menu at runtime)", chord, strings.Join(names, ", ")),
		})
	}

	bySlash := map[string][]slashClaim{}
	for _, c := range slashClaims {
		bySlash[c.slash] = append(bySlash[c.slash], c)
	}
	slashes := make([]string, 0, len(bySlash))
	for s := range bySlash {
		slashes = append(slashes, s)
	}
	sort.Strings(slashes)
	for _, slash := range slashes {
		list := bySlash[slash]
		if len(list) < 2 {
			continue
		}
		// Same-bundle dupes are already reported in lintOneBundle.
		dirs := map[string]bool{}
		for _, c := range list {
			dirs[c.dir] = true
		}
		if len(dirs) < 2 {
			continue
		}
		var names []string
		for _, c := range list {
			names = append(names, fmt.Sprintf("%s/%s", c.bundle, c.name))
		}
		sort.Strings(names)
		rep.add(LintFinding{
			Severity: LintError,
			Bundle:   list[0].bundle,
			Command:  list[0].name,
			Path:     list[0].dir,
			Message:  fmt.Sprintf("slash %q duplicated by %s", slash, strings.Join(names, ", ")),
		})
	}
	return rep, nil
}

func (r *LintReport) add(f LintFinding) {
	r.Findings = append(r.Findings, f)
	switch f.Severity {
	case LintError:
		r.Errors++
	default:
		r.Infos++
	}
}

func lintOneBundle(dir string, cacheDir string) (BundleMeta, []Command, []PromptAction, []Snippet, []LintFinding, error) {
	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		return BundleMeta{}, nil, nil, nil, nil, fmt.Errorf("read bundle.toml: %w", err)
	}
	parsed, err := ParseManifestFull(dir, data)
	if err != nil {
		return BundleMeta{}, nil, nil, nil, nil, err
	}
	meta, cmds, prompts := parsed.Meta, parsed.Commands, parsed.Prompts
	var findings []LintFinding
	for _, g := range parsed.Grammars {
		if !GrammarConventionOK(g.Path) {
			findings = append(findings, LintFinding{
				Severity: LintInfo,
				Bundle:   meta.Name,
				Command:  g.ScopeName,
				Path:     g.AbsPath,
				Message:  fmt.Sprintf("grammar path %q should end in .tmLanguage.json or .tmLanguage", g.Path),
			})
		}
		if st, err := os.Stat(g.AbsPath); err != nil {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  g.ScopeName,
				Path:     dir,
				Message:  fmt.Sprintf("grammar file missing: %v", err),
			})
		} else if st.IsDir() {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  g.ScopeName,
				Path:     g.AbsPath,
				Message:  "grammar path is a directory",
			})
		} else if body, rerr := os.ReadFile(g.AbsPath); rerr == nil && len(body) > 0 {
			// Lightweight JSON check for .json grammars (plist is XML — skip).
			if strings.HasSuffix(strings.ToLower(g.Path), ".json") {
				trim := strings.TrimSpace(string(body))
				if !strings.HasPrefix(trim, "{") && !strings.HasPrefix(trim, "[") {
					findings = append(findings, LintFinding{
						Severity: LintError,
						Bundle:   meta.Name,
						Command:  g.ScopeName,
						Path:     g.AbsPath,
						Message:  "grammar JSON does not look like an object/array",
					})
				}
			}
		}
	}
	for _, t := range parsed.Themes {
		if !ThemeConventionOK(t.Path) {
			findings = append(findings, LintFinding{
				Severity: LintInfo,
				Bundle:   meta.Name,
				Command:  t.Name,
				Path:     t.AbsPath,
				Message:  fmt.Sprintf("theme path %q should end in .tmTheme.json (or .tmTheme)", t.Path),
			})
		}
		if body, rerr := os.ReadFile(t.AbsPath); rerr == nil {
			trim := strings.TrimSpace(string(body))
			if strings.HasSuffix(strings.ToLower(t.Path), ".json") &&
				!strings.HasPrefix(trim, "{") {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  t.Name,
					Path:     t.AbsPath,
					Message:  "theme JSON does not look like an object",
				})
			}
		}
	}
	for _, t := range parsed.TerminalThemes {
		if !TerminalThemeConventionOK(t.Path) {
			findings = append(findings, LintFinding{
				Severity: LintInfo,
				Bundle:   meta.Name,
				Command:  t.Name,
				Path:     t.AbsPath,
				Message:  fmt.Sprintf("terminalTheme path %q should end in .json", t.Path),
			})
		}
	}
	for _, d := range parsed.LanguageData {
		if d.LineComment == "" && d.BlockCommentStart == "" &&
			d.IncreaseIndentPattern == "" && d.DecreaseIndentPattern == "" &&
			d.IndentNextLinePattern == "" && d.ZeroIndentPattern == "" &&
			!d.DisableIndentCorrections &&
			len(d.CloseBrackets) == 0 {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  d.ID,
				Path:     dir,
				Message:  "languageData has no comment tokens, indent patterns, or closeBrackets",
			})
		}
		if (d.BlockCommentStart == "") != (d.BlockCommentEnd == "") {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  d.ID,
				Path:     dir,
				Message:  "blockCommentStart and blockCommentEnd must both be set",
			})
		}
	}
	for _, s := range parsed.Snippets {
		raw := strings.TrimSpace(s.KeyEquivalent)
		if raw != "" {
			can, err := ParseKeyEquivalent(raw)
			if err != nil {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  s.Name,
					Path:     dir,
					Message:  fmt.Sprintf("keyEquivalent: %v", err),
				})
			} else if IsReservedAppChord(can) {
				label := ReservedAppChords[can]
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  s.Name,
					Path:     dir,
					Message:  fmt.Sprintf("keyEquivalent %q collides with app shortcut %q (binding dropped in-app)", can, label),
				})
			}
		}
	}
	for _, sy := range parsed.Symbols {
		if symbolTransformHasOniguruma(sy.Transform) {
			findings = append(findings, LintFinding{
				Severity: LintInfo,
				Bundle:   meta.Name,
				Command:  sy.ID,
				Path:     dir,
				Message:  "symbol transform uses Oniguruma syntax; Recur applies JavaScript regex s/// only",
			})
		}
	}
	roleByID := map[string]Role{}
	for _, r := range parsed.Roles {
		roleByID[r.ID] = r
	}
	for _, lp := range parsed.Loops {
		condID := lp.Roles["conductor"]
		if r, ok := roleByID[condID]; ok && r.Mode != RoleModeLoop {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  lp.ID,
				Path:     dir,
				Message:  fmt.Sprintf("conductor %q must have mode = \"loop\"", condID),
			})
		}
		for _, ph := range lp.Phases {
			if ph.Start == "review" && strings.TrimSpace(ph.Prompts["implement"]) != "" {
				findings = append(findings, LintFinding{
					Severity: LintInfo,
					Bundle:   meta.Name,
					Command:  lp.ID + "/" + ph.ID,
					Path:     dir,
					Message:  "start = \"review\" ignores prompts.implement",
				})
			}
			if ph.Uses == VerbVerify {
				vid := ph.Roles["verifier"]
				if r, ok := roleByID[vid]; ok && !containsStr(r.Tools, "report_result") {
					findings = append(findings, LintFinding{
						Severity: LintError,
						Bundle:   meta.Name,
						Command:  lp.ID + "/" + ph.ID,
						Path:     dir,
						Message:  "verifier role must list report_result",
					})
				}
			}
			for slot, rid := range ph.Roles {
				r, ok := roleByID[rid]
				if !ok {
					continue
				}
				allowed := AllowedRoleModes(ph.Uses, slot)
				if len(allowed) > 0 && !containsStr(allowed, r.Mode) {
					findings = append(findings, LintFinding{
						Severity: LintError,
						Bundle:   meta.Name,
						Command:  lp.ID + "/" + ph.ID,
						Path:     dir,
						Message:  fmt.Sprintf("roles.%s %q has mode %q (want %s)", slot, rid, r.Mode, strings.Join(allowed, "|")),
					})
				}
			}
			if ph.Findings != nil {
				for _, pol := range parsed.Policies {
					if err := SameOrStricter(*ph.Findings, pol.FindingsThreshold); err != nil {
						findings = append(findings, LintFinding{
							Severity: LintError,
							Bundle:   meta.Name,
							Command:  lp.ID + "/" + ph.ID,
							Path:     dir,
							Message:  fmt.Sprintf("findings_threshold vs policy %s: %s", pol.ID, err.Error()),
						})
					}
				}
			}
		}
	}
	for _, c := range cmds {
		raw := strings.TrimSpace(c.KeyEquivalent)
		if raw != "" {
			can, err := ParseKeyEquivalent(raw)
			if err != nil {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  c.Name,
					Path:     dir,
					Message:  fmt.Sprintf("keyEquivalent: %v", err),
				})
			} else if IsReservedAppChord(can) {
				label := ReservedAppChords[can]
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  c.Name,
					Path:     dir,
					Message:  fmt.Sprintf("keyEquivalent %q collides with app shortcut %q (binding dropped in-app)", can, label),
				})
			}
		}

		if c.OpenInPreview {
			if c.Output != OutputNewDocument {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  c.Name,
					Path:     dir,
					Message:  "openInPreview requires output = \"newDocument\"",
				})
			}
			if !PreviewableLanguage(c.Language) {
				lang := strings.TrimSpace(c.Language)
				msg := "openInPreview requires language (html, htm, md, mdx, markdown, svg, mmd, or mermaid)"
				if lang != "" {
					msg = fmt.Sprintf("openInPreview: language %q has no Preview chip (use html, htm, md, mdx, markdown, svg, mmd, or mermaid)", lang)
				}
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  c.Name,
					Path:     dir,
					Message:  msg,
				})
			}
		}

		switch c.Interpreter {
		case InterpreterTypeScript:
			if _, err := CompileMacro(cacheDir, c); err != nil {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  c.Name,
					Path:     scriptPathOrDir(c, dir),
					Message:  err.Error(),
				})
			}
		case InterpreterShebang:
			if err := lintShebang(c); err != nil {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  c.Name,
					Path:     scriptPathOrDir(c, dir),
					Message:  err.Error(),
				})
			}
		}
	}

	seenSlash := map[string]string{} // slash → first prompt name
	for _, p := range prompts {
		if !promptSlashRE.MatchString(p.Slash) {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  p.Name,
				Path:     dir,
				Message:  fmt.Sprintf("slash %q must match %s", p.Slash, promptSlashRE.String()),
			})
		}
		if p.Loop != "" {
			if !validBundleID(p.Loop) {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  p.Name,
					Path:     dir,
					Message:  fmt.Sprintf("loop %q is not a safe id", p.Loop),
				})
			}
			if p.Mode != "" && p.Mode != PromptModeLoop {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  p.Name,
					Path:     dir,
					Message:  `loop requires mode "loop"`,
				})
			}
		}
		if p.Mode != "" && !validPromptMode(p.Mode) {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  p.Name,
				Path:     dir,
				Message:  fmt.Sprintf("unknown mode %q (use ask|plan|agent|review|loop, or omit)", p.Mode),
			})
		}
		if p.FreshSession && p.Mode == PromptModeLoop {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  p.Name,
				Path:     dir,
				Message:  `fresh_session is not valid with mode "loop"`,
			})
		}
		if p.TabGroup != "" && p.Mode == "" {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  p.Name,
				Path:     dir,
				Message:  "tab_group requires mode",
			})
		}
		if p.TabGroup != "" && !promptSlashRE.MatchString(p.TabGroup) {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  p.Name,
				Path:     dir,
				Message:  fmt.Sprintf("tab_group %q must match %s", p.TabGroup, promptSlashRE.String()),
			})
		}
		if p.TimeoutMs < 0 || p.TimeoutMs > PromptTimeoutMaxMs {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  p.Name,
				Path:     dir,
				Message:  fmt.Sprintf("timeoutMs must be 0 or 1..%d", PromptTimeoutMaxMs),
			})
		}
		if prev, ok := seenSlash[p.Slash]; ok {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  p.Name,
				Path:     dir,
				Message:  fmt.Sprintf("slash %q duplicated with prompt %q", p.Slash, prev),
			})
		} else {
			seenSlash[p.Slash] = p.Name
		}

		switch p.Interpreter {
		case InterpreterTypeScript:
			if _, err := CompilePromptMacro(cacheDir, p); err != nil {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  p.Name,
					Path:     promptScriptPathOrDir(p, dir),
					Message:  err.Error(),
				})
			}
		case InterpreterShebang:
			if err := lintPromptShebang(p); err != nil {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  p.Name,
					Path:     promptScriptPathOrDir(p, dir),
					Message:  err.Error(),
				})
			}
		}
		if p.CompleteInterpreter == InterpreterTypeScript {
			if _, err := CompileCompleteMacro(cacheDir, p); err != nil {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  p.Name,
					Path:     p.CompletePath,
					Message:  err.Error(),
				})
			}
		} else if p.CompleteInterpreter == InterpreterShebang {
			body := strings.TrimLeft(p.CompleteBody, "\uFEFF")
			if !strings.HasPrefix(body, "#!") {
				findings = append(findings, LintFinding{
					Severity: LintError,
					Bundle:   meta.Name,
					Command:  p.Name,
					Path:     p.CompletePath,
					Message:  "complete shebang script must start with #!",
				})
			}
		}
	}
	// simplify: no Settings → Bundles toggles for [[search]]/[[expand]] until a
	// pack ships several; disable with the parent prompt pack by removing it.
	for _, se := range parsed.Searches {
		body := strings.TrimLeft(se.Body, "\uFEFF")
		if !strings.HasPrefix(body, "#!") {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  se.Name,
				Path:     se.ScriptPath,
				Message:  "search shebang script must start with #!",
			})
		}
	}
	for _, ex := range parsed.Expands {
		body := strings.TrimLeft(ex.Body, "\uFEFF")
		if !strings.HasPrefix(body, "#!") {
			findings = append(findings, LintFinding{
				Severity: LintError,
				Bundle:   meta.Name,
				Command:  ex.Name,
				Path:     ex.ScriptPath,
				Message:  "expand shebang script must start with #!",
			})
		}
	}
	return meta, cmds, prompts, parsed.Snippets, findings, nil
}

// PreviewableLanguage reports whether a newDocument language/extension gets an
// Observatory Code/Preview chip (mirrors frontend/src/lib/preview.ts).
func PreviewableLanguage(lang string) bool {
	s := strings.TrimSpace(strings.ToLower(lang))
	s = strings.TrimPrefix(s, ".")
	switch s {
	case "html", "htm", "md", "mdx", "markdown", "svg", "mmd", "mermaid":
		return true
	default:
		return false
	}
}

func scriptPathOrDir(c Command, dir string) string {
	if c.ScriptPath != "" {
		return c.ScriptPath
	}
	if c.Script != "" {
		return filepath.Join(dir, filepath.FromSlash(c.Script))
	}
	return filepath.Join(dir, "bundle.toml")
}

func promptScriptPathOrDir(p PromptAction, dir string) string {
	if p.ScriptPath != "" {
		return p.ScriptPath
	}
	if p.Script != "" {
		return filepath.Join(dir, filepath.FromSlash(p.Script))
	}
	return filepath.Join(dir, "bundle.toml")
}

func lintShebang(c Command) error {
	body := strings.TrimLeft(c.Body, "\uFEFF")
	if !strings.HasPrefix(body, "#!") {
		if c.Script != "" {
			return fmt.Errorf("shebang script %q must start with #!", c.Script)
		}
		return fmt.Errorf("shebang body must start with #!")
	}
	return nil
}

func lintPromptShebang(p PromptAction) error {
	body := strings.TrimLeft(p.Body, "\uFEFF")
	if !strings.HasPrefix(body, "#!") {
		if p.Script != "" {
			return fmt.Errorf("shebang script %q must start with #!", p.Script)
		}
		return fmt.Errorf("shebang body must start with #!")
	}
	return nil
}

func symbolTransformHasOniguruma(src string) bool {
	return strings.Contains(src, "(?x)") ||
		strings.Contains(src, "(?i)") ||
		strings.Contains(src, "(?m)") ||
		strings.Contains(src, `\h`) ||
		strings.Contains(src, "(?(")
}

// FormatLintReport prints findings for the CLI.
func FormatLintReport(r LintReport, color bool) string {
	paint := func(code, s string) string {
		if !color {
			return s
		}
		return code + s + "\033[0m"
	}
	const (
		ansiRed    = "\033[31m"
		ansiGreen  = "\033[32m"
		ansiYellow = "\033[33m"
		ansiDim    = "\033[2m"
		ansiBold   = "\033[1m"
	)
	var b strings.Builder
	for _, f := range r.Findings {
		loc := f.Bundle
		if f.Command != "" {
			loc = f.Bundle + "/" + f.Command
		}
		switch f.Severity {
		case LintError:
			fmt.Fprintf(&b, "%s\t%s\t%s\n", paint(ansiBold+ansiRed, "error"), loc, f.Message)
		default:
			fmt.Fprintf(&b, "%s\t%s\t%s\n", paint(ansiYellow, "info"), loc, f.Message)
		}
		if f.Path != "" {
			fmt.Fprintf(&b, "  %s\n", paint(ansiDim, f.Path))
		}
	}
	if r.Errors == 0 && r.Infos == 0 {
		fmt.Fprintf(&b, "%s\n", paint(ansiGreen, "ok\tno issues"))
		return b.String()
	}
	errPart := fmt.Sprintf("%d errors", r.Errors)
	infoPart := fmt.Sprintf("%d info", r.Infos)
	if r.Errors == 0 {
		fmt.Fprintf(&b, "%s, %s\n", paint(ansiGreen, "0 errors"), infoPart)
	} else {
		fmt.Fprintf(&b, "%s, %s\n", paint(ansiBold+ansiRed, errPart), infoPart)
	}
	return b.String()
}
