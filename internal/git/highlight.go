package git

import (
	"html"
	"path/filepath"
	"strings"
	"sync"

	"github.com/alecthomas/chroma/v2"
	"github.com/alecthomas/chroma/v2/lexers"

	"recur/internal/domain"
)

// Max highlighted lines in a single FileDiff. Must match DiffFile's default
// lineCap (500) so the initial view is uniformly styled; beyond this we
// plain-escape so large full-file adds stay responsive.
const maxHighlightLines = 500

// Cap chroma across a whole patch. A 100-file review × 500 lines would
// otherwise block the RPC for seconds while the webview waits.
const maxHighlightTotalLines = 8 * maxHighlightLines

var lexerCache sync.Map // basename → chroma.Lexer (or false for none)

// HighlightFiles upgrades plain (escaped) CodeHTML on add/del/ctx lines to
// syntax-highlighted markup. Lines that already contain token spans (seed data
// or freshly parsed diffs) are left alone. Returns true if any line changed —
// callers can persist the result so older PRs pick up highlighting once.
func HighlightFiles(files []domain.FileDiff) bool {
	changed := false
	left := maxHighlightTotalLines
	for i := range files {
		if left <= 0 {
			break
		}
		c, n := highlightFileBudget(&files[i], left)
		left -= n
		if c {
			changed = true
		}
	}
	return changed
}

func highlightFileBudget(f *domain.FileDiff, budget int) (changed bool, used int) {
	if budget > maxHighlightLines {
		budget = maxHighlightLines
	}
	n := 0
	for i := range f.Lines {
		l := &f.Lines[i]
		if l.Type == "hunk" || l.CodeHTML == "" {
			continue
		}
		if strings.Contains(l.CodeHTML, `<span class="`) {
			continue
		}
		if n >= budget {
			break
		}
		n++
		plain := html.UnescapeString(l.CodeHTML)
		next := highlightLine(f.Path, plain)
		if next != l.CodeHTML {
			l.CodeHTML = next
			changed = true
		}
	}
	return changed, n
}

func lexerFor(path string) chroma.Lexer {
	key := filepath.Base(path)
	if key == "" {
		key = path
	}
	if v, ok := lexerCache.Load(key); ok {
		if v == false {
			return nil
		}
		return v.(chroma.Lexer)
	}
	lexer := lexers.Match(key)
	if lexer == nil {
		lexerCache.Store(key, false)
		return nil
	}
	lexer = chroma.Coalesce(lexer)
	lexerCache.Store(key, lexer)
	return lexer
}

// LexerName returns the lowercase chroma lexer name for a path ("go", "typescript",
// …), or "" when nothing matches. It exists so callers outside this package
// (internal/code's file reader) share lexerFor's cache instead of paying a fresh
// lexers.Match — a scan over every registered lexer's filename globs — per lookup.
func LexerName(path string) string {
	lexer := lexerFor(path)
	if lexer == nil {
		return ""
	}
	cfg := lexer.Config()
	if cfg == nil {
		return ""
	}
	return strings.ToLower(cfg.Name)
}

// highlightLine returns HTML for one source line: escaped text wrapped in the
// token classes the PR UI already styles (.kw/.str/.fn/.cm/.ty/.nm). Language is
// inferred from the file path; unknown languages fall back to plain escape so we
// never inject raw markup (PRD §12.2).
func highlightLine(path, code string) string {
	if code == "" {
		return ""
	}
	lexer := lexerFor(path)
	if lexer == nil {
		return html.EscapeString(code)
	}
	it, err := lexer.Tokenise(nil, code)
	if err != nil {
		return html.EscapeString(code)
	}
	var b strings.Builder
	for _, tok := range it.Tokens() {
		text := html.EscapeString(tok.Value)
		if text == "" {
			continue
		}
		if class := tokenClass(tok.Type); class != "" {
			b.WriteString(`<span class="`)
			b.WriteString(class)
			b.WriteString(`">`)
			b.WriteString(text)
			b.WriteString(`</span>`)
			continue
		}
		b.WriteString(text)
	}
	return b.String()
}

// tokenClass maps chroma token types onto Recur's small token palette.
func tokenClass(t chroma.TokenType) string {
	switch {
	case t.InCategory(chroma.Comment):
		return "cm"
	case t.InCategory(chroma.String), t.InCategory(chroma.LiteralString):
		return "str"
	case t == chroma.KeywordType, t == chroma.NameClass, t == chroma.NameTag, t == chroma.NameException:
		return "ty"
	case t.InCategory(chroma.Keyword):
		return "kw"
	case t == chroma.NameFunction, t == chroma.NameFunctionMagic, t == chroma.NameDecorator, t == chroma.NameAttribute:
		return "fn"
	case t == chroma.NameConstant, t == chroma.NameBuiltin, t == chroma.NameEntity,
		t.InCategory(chroma.Number), t.InCategory(chroma.LiteralNumber):
		return "nm"
	default:
		return ""
	}
}
