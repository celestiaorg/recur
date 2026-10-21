#!/usr/bin/env bash
# Regenerates recur-git's vendored chroma data and Go-derived tables.
#
# Needs Go and python3. Outputs:
#   chroma/lexers/*.xml        chroma's embedded lexers, verbatim
#   chroma/generated/*.xml     lexers chroma builds in Go, dumped to the same
#                              XML (closures become named <func>/<raku…> elements)
#   chroma/remap_*.tsv         TypeRemappingLexer word tables (Common Lisp, EmacsLisp)
#   chroma/raku_brackets.tsv   the Raku lexer's bracket pairs
#   tests/fixtures/*.json      Go highlightLine output and lexer names to compare against
#   src/chroma/regexp2/unicode_tables.rs, src/html_entities.rs
#
# Usage: chroma/regenerate.sh [chroma version, default v2.27.0]
set -euo pipefail

crate="$(cd "$(dirname "$0")/.." && pwd)"
version="${1:-v2.27.0}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

src="$(go env GOMODCACHE)/github.com/alecthomas/chroma/v2@$version"
[ -d "$src" ] || (cd "$work" && go mod download "github.com/alecthomas/chroma/v2@$version")
cp -r "$src" "$work/chroma" && chmod -R u+w "$work/chroma"
cd "$work/chroma"

# Name the closures chroma's Go-coded lexers use so the dump can describe them.
patch -s -p1 <<'PATCH'
--- a/emitters.go
+++ b/emitters.go
@@ -68,8 +68,15 @@
 }
 
 // ByGroupNames emits a token for each named matching group in the rule's regex.
+type ByGroupNamesEmitter struct {
+	Emitters map[string]Emitter
+	Fn       EmitterFunc
+}
+
+func (b *ByGroupNamesEmitter) Emit(groups []string, state *LexerState) Iterator { return b.Fn(groups, state) }
+
 func ByGroupNames(emitters map[string]Emitter) Emitter {
-	return EmitterFunc(func(groups []string, state *LexerState) Iterator {
+	return &ByGroupNamesEmitter{Emitters: emitters, Fn: EmitterFunc(func(groups []string, state *LexerState) Iterator {
 		iterators := make([]Iterator, 0, len(state.NamedGroups)-1)
 		if len(state.NamedGroups)-1 == 0 {
 			if emitter, ok := emitters[`0`]; ok {
@@ -92,7 +99,7 @@
 			}
 		}
 		return Concaterator(iterators...)
-	})
+	})}
 }
 
 // UsingByGroup emits tokens for the matched groups in the regex using a
--- a/lexers/raku.go
+++ b/lexers/raku.go
@@ -334,12 +334,13 @@
 		'\uff5b': '\uff5d', '\uff5f': '\uff60', '\uff62': '\uff63',
 	}
 
+	RakuBrackets = brackets
 	bracketsPattern := `[` + regexp.QuoteMeta(joinRuneMap(brackets)) + `]`
 
 	// Finds opening brackets and their closing counterparts (including pod and heredoc)
 	// and modifies state groups and position accordingly
-	findBrackets := func(tokenClass RakuToken) MutatorFunc {
-		return func(state *LexerState) error {
+	findBrackets := func(tokenClass RakuToken) Mutator {
+		return RakuFindBrackets{Token: int(tokenClass), Fn: func(state *LexerState) error {
 			var openingChars []rune
 			var adverbs []rune
 			switch tokenClass {
@@ -386,7 +387,7 @@
 					rulePosition: bottomRule,
 				}
 
-				err := replaceRule(formatterRule)(state)
+				err := replaceRule(formatterRule).Fn(state)
 				if err != nil {
 					panic(err)
 				}
@@ -398,7 +399,7 @@
 					pushState:              true,
 					numberOfDelimiterChars: nChars,
 					appendMutator:          popRule(formatterRule),
-				})(state)
+				}).Fn(state)
 				if err != nil {
 					panic(err)
 				}
@@ -418,7 +419,7 @@
 					stateName: `regex`,
 					popState:  true,
 					pushState: true,
-				})(state)
+				}).Fn(state)
 				if err != nil {
 					panic(err)
 				}
@@ -434,7 +435,7 @@
 					stateName:    `regex`,
 					popState:     true,
 					pushState:    true,
-				})(state)
+				}).Fn(state)
 				if err != nil {
 					panic(err)
 				}
@@ -566,7 +567,7 @@
 			state.Pos = endPos + nChars
 
 			return nil
-		}
+		}}
 	}
 
 	// Raku rules
@@ -1348,8 +1349,24 @@
 }
 
 // Pops rule from state-stack and replaces the rule with the previous rule
-func popRule(rule ruleReplacingConfig) MutatorFunc {
-	return func(state *LexerState) error {
+var RakuBrackets map[rune]rune
+
+type RakuPopRule struct {
+	Config ruleReplacingConfig
+	Fn     MutatorFunc
+}
+
+func (r RakuPopRule) Mutate(state *LexerState) error { return r.Fn(state) }
+
+type RakuReplaceRule struct {
+	Config ruleReplacingConfig
+	Fn     MutatorFunc
+}
+
+func (r RakuReplaceRule) Mutate(state *LexerState) error { return r.Fn(state) }
+
+func popRule(rule ruleReplacingConfig) Mutator {
+	return RakuPopRule{Config: rule, Fn: func(state *LexerState) error {
 		stackName := genStackName(rule.stateName, rule.rulePosition)
 
 		stack, ok := state.Get(stackName).([]ruleReplacingConfig)
@@ -1364,19 +1381,19 @@
 			state.Set(stackName, stack)
 
 			// Call replaceRule to use the last rule
-			err := replaceRule(lastRule)(state)
+			err := replaceRule(lastRule).Fn(state)
 			if err != nil {
 				panic(err)
 			}
 		}
 
 		return nil
-	}
+	}}
 }
 
 // Replaces a state's rule based on the rule config and position
-func replaceRule(rule ruleReplacingConfig) MutatorFunc {
-	return func(state *LexerState) error {
+func replaceRule(rule ruleReplacingConfig) RakuReplaceRule {
+	return RakuReplaceRule{Config: rule, Fn: func(state *LexerState) error {
 		stateName := rule.stateName
 		stackName := genStackName(rule.stateName, rule.rulePosition)
 
@@ -1462,7 +1479,7 @@
 		}
 
 		return nil
-	}
+	}}
 }
 
 // Generates rule replacing stack using state name and rule position
@@ -1503,8 +1520,22 @@
 }
 
 // Emitter for colon pairs, changes token state based on key and brackets
+type RakuFindBrackets struct {
+	Token int
+	Fn    MutatorFunc
+}
+
+func (r RakuFindBrackets) Mutate(state *LexerState) error { return r.Fn(state) }
+
+type RakuColonPair struct {
+	Token TokenType
+	Fn    EmitterFunc
+}
+
+func (r RakuColonPair) Emit(groups []string, state *LexerState) Iterator { return r.Fn(groups, state) }
+
 func colonPair(tokenClass TokenType) Emitter {
-	return EmitterFunc(func(groups []string, state *LexerState) Iterator {
+	return RakuColonPair{Token: tokenClass, Fn: EmitterFunc(func(groups []string, state *LexerState) Iterator {
 		iterators := []Iterator{}
 		tokens := []Token{
 			{Punctuation, state.NamedGroups[`colon`]},
@@ -1576,7 +1607,7 @@
 		iterators = append(iterators, Literator(tokens[2]))
 
 		return Concaterator(iterators...)
-	})
+	})}
 }
 
 // Emitter for quoting constructs, changes token state based on quote name and adverbs
PATCH
# Go map order would make the Raku bracket class differ per run; sort it.
python3 - lexers/raku.go <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
s = s.replace("\treturn string(runes)\n}", "\tslices.Sort(runes)\n\treturn string(runes)\n}", 1)
open(p, "w").write(s)
PY

cat > lexers/zz_dump_test.go <<'GO'
package lexers

import (
	"bytes"
	"encoding/json"
	"encoding/xml"
	"fmt"
	"hash/fnv"
	"html"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"testing"
	"unsafe"

	. "github.com/alecthomas/chroma/v2"
)

func out() string { return os.Getenv("DUMP_DIR") }

func esc(s string) string {
	var b bytes.Buffer
	_ = xml.EscapeText(&b, []byte(s))
	return b.String()
}

func funcName(f any) string {
	return runtime.FuncForPC(reflect.ValueOf(f).Pointer()).Name()
}

func field(v reflect.Value, name string) reflect.Value {
	f := v.FieldByName(name)
	return reflect.NewAt(f.Type(), unsafe.Pointer(f.UnsafeAddr())).Elem()
}

func emitterXML(b *strings.Builder, e Emitter, ind string) {
	if e == nil {
		fmt.Fprintf(b, "%s<nil/>\n", ind)
		return
	}
	switch t := e.(type) {
	case TokenType:
		fmt.Fprintf(b, "%s<token type=\"%s\"/>\n", ind, t.String())
		return
	case *ByGroupNamesEmitter:
		fmt.Fprintf(b, "%s<bygroupnames>\n", ind)
		keys := make([]string, 0, len(t.Emitters))
		for k := range t.Emitters {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			fmt.Fprintf(b, "%s  <group name=\"%s\">\n", ind, esc(k))
			emitterXML(b, t.Emitters[k], ind+"    ")
			fmt.Fprintf(b, "%s  </group>\n", ind)
		}
		fmt.Fprintf(b, "%s</bygroupnames>\n", ind)
		return
	case RakuColonPair:
		fmt.Fprintf(b, "%s<rakucolonpair token=\"%s\"/>\n", ind, t.Token.String())
		return
	case EmitterFunc:
		fmt.Fprintf(b, "%s<func name=\"%s\"/>\n", ind, funcName(t))
		return
	}
	v := reflect.ValueOf(e).Elem()
	switch e.(SerialisableEmitter).EmitterKind() {
	case "bygroups":
		fmt.Fprintf(b, "%s<bygroups>\n", ind)
		for _, c := range field(v, "Emitters").Interface().(Emitters) {
			emitterXML(b, c, ind+"  ")
		}
		fmt.Fprintf(b, "%s</bygroups>\n", ind)
	case "usingbygroup":
		fmt.Fprintf(b, "%s<usingbygroup>\n%s  <sublexer_name_group>%d</sublexer_name_group>\n%s  <code_group>%d</code_group>\n%s  <emitters>\n", ind, ind, field(v, "SublexerNameGroup").Int(), ind, field(v, "CodeGroup").Int(), ind)
		for _, c := range field(v, "Emitters").Interface().(Emitters) {
			emitterXML(b, c, ind+"    ")
		}
		fmt.Fprintf(b, "%s  </emitters>\n%s</usingbygroup>\n", ind, ind)
	case "using":
		fmt.Fprintf(b, "%s<using lexer=\"%s\"/>\n", ind, esc(field(v, "Lexer").String()))
	case "usingself":
		fmt.Fprintf(b, "%s<usingself state=\"%s\"/>\n", ind, esc(field(v, "State").String()))
	default:
		panic(fmt.Sprintf("emitter %T", e))
	}
}

func mutatorXML(b *strings.Builder, m Mutator, ind string) {
	switch t := m.(type) {
	case RakuFindBrackets:
		fmt.Fprintf(b, "%s<rakufindbrackets token=\"%d\"/>\n", ind, t.Token)
		return
	case MutatorFunc:
		fmt.Fprintf(b, "%s<func name=\"%s\"/>\n", ind, funcName(t))
		return
	case RakuReplaceRule:
		rakuRuleXML(b, "rakureplacerule", t.Config, ind)
		return
	case RakuPopRule:
		rakuRuleXML(b, "rakupoprule", t.Config, ind)
		return
	}
	v := reflect.ValueOf(m).Elem()
	switch m.(SerialisableMutator).MutatorKind() {
	case "include":
		fmt.Fprintf(b, "%s<include state=\"%s\"/>\n", ind, esc(field(v, "State").String()))
	case "combined":
		fmt.Fprintf(b, "%s<combined", ind)
		for _, s := range field(v, "States").Interface().([]string) {
			fmt.Fprintf(b, " state=\"%s\"", esc(s))
		}
		b.WriteString("/>\n")
	case "push":
		fmt.Fprintf(b, "%s<push", ind)
		for _, s := range field(v, "States").Interface().([]string) {
			fmt.Fprintf(b, " state=\"%s\"", esc(s))
		}
		b.WriteString("/>\n")
	case "pop":
		fmt.Fprintf(b, "%s<pop depth=\"%d\"/>\n", ind, field(v, "Depth").Int())
	case "mutators":
		fmt.Fprintf(b, "%s<mutators>\n", ind)
		for _, c := range field(v, "Mutators").Interface().([]Mutator) {
			mutatorXML(b, c, ind+"  ")
		}
		fmt.Fprintf(b, "%s</mutators>\n", ind)
	default:
		panic(fmt.Sprintf("mutator %T", m))
	}
}

func rakuRuleXML(b *strings.Builder, tag string, c ruleReplacingConfig, ind string) {
	fmt.Fprintf(b, "%s<%s delimiter=\"%s\" pattern=\"%s\" ndelim=\"%d\" position=\"%d\" state=\"%s\" pop=\"%v\" popstate=\"%v\" pushstate=\"%v\">\n",
		ind, tag, esc(string(c.delimiter)), esc(c.pattern), c.numberOfDelimiterChars, int(c.rulePosition), esc(c.stateName), c.pop, c.popState, c.pushState)
	if c.tokenType != nil {
		fmt.Fprintf(b, "%s  <emitter>\n", ind)
		emitterXML(b, c.tokenType, ind+"    ")
		fmt.Fprintf(b, "%s  </emitter>\n", ind)
	}
	if c.mutator != nil {
		fmt.Fprintf(b, "%s  <mutator>\n", ind)
		mutatorXML(b, c.mutator, ind+"    ")
		fmt.Fprintf(b, "%s  </mutator>\n", ind)
	}
	if c.appendMutator != nil {
		fmt.Fprintf(b, "%s  <append>\n", ind)
		mutatorXML(b, c.appendMutator, ind+"    ")
		fmt.Fprintf(b, "%s  </append>\n", ind)
	}
	fmt.Fprintf(b, "%s</%s>\n", ind, tag)
}

func configXML(b *strings.Builder, c *Config) {
	b.WriteString("  <config>\n")
	fmt.Fprintf(b, "    <name>%s</name>\n", esc(c.Name))
	for _, a := range c.Aliases {
		fmt.Fprintf(b, "    <alias>%s</alias>\n", esc(a))
	}
	for _, a := range c.Filenames {
		fmt.Fprintf(b, "    <filename>%s</filename>\n", esc(a))
	}
	for _, a := range c.AliasFilenames {
		fmt.Fprintf(b, "    <alias_filename>%s</alias_filename>\n", esc(a))
	}
	for _, a := range c.MimeTypes {
		fmt.Fprintf(b, "    <mime_type>%s</mime_type>\n", esc(a))
	}
	if c.CaseInsensitive {
		b.WriteString("    <case_insensitive>true</case_insensitive>\n")
	}
	if c.DotAll {
		b.WriteString("    <dot_all>true</dot_all>\n")
	}
	if c.NotMultiline {
		b.WriteString("    <not_multiline>true</not_multiline>\n")
	}
	if c.EnsureNL {
		b.WriteString("    <ensure_nl>true</ensure_nl>\n")
	}
	if c.Priority != 0 {
		fmt.Fprintf(b, "    <priority>%s</priority>\n", strconv.FormatFloat(float64(c.Priority), 'g', -1, 32))
	}
	b.WriteString("  </config>\n")
}

func rulesXML(b *strings.Builder, rules Rules) {
	b.WriteString("  <rules>\n")
	states := make([]string, 0, len(rules))
	for k := range rules {
		states = append(states, k)
	}
	sort.Strings(states)
	for _, st := range states {
		fmt.Fprintf(b, "    <state name=\"%s\">\n", esc(st))
		for _, r := range rules[st] {
			if r.Pattern != "" {
				fmt.Fprintf(b, "      <rule pattern=\"%s\">\n", esc(r.Pattern))
			} else {
				b.WriteString("      <rule>\n")
			}
			if r.Type != nil {
				emitterXML(b, r.Type, "        ")
			}
			if r.Mutator != nil {
				mutatorXML(b, r.Mutator, "        ")
			}
			b.WriteString("      </rule>\n")
		}
		b.WriteString("    </state>\n")
	}
	b.WriteString("  </rules>\n")
}

func lexerXML(l *RegexLexer) string {
	var b strings.Builder
	b.WriteString("<lexer>\n")
	configXML(&b, l.Config())
	rulesXML(&b, l.MustRules())
	b.WriteString("</lexer>\n")
	return b.String()
}

func slug(name string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(name) {
		if r >= 'a' && r <= 'z' || r >= '0' && r <= '9' {
			b.WriteRune(r)
		} else {
			b.WriteRune('_')
		}
	}
	return b.String()
}

func inner(l Lexer) Lexer {
	v := reflect.ValueOf(l).Elem()
	return field(v, "Lexer").Interface().(Lexer)
}

func TestDump(t *testing.T) {
	dir := out()
	_ = os.MkdirAll(filepath.Join(dir, "generated"), 0o755)
	embeddedNames := map[string]string{}
	paths, _ := filepath.Glob("embedded/*.xml")
	for _, p := range paths {
		l := MustNewXMLLexer(embedded, p)
		embeddedNames[l.Config().Name] = filepath.Base(p)
	}
	var reg strings.Builder
	write := func(name string, l *RegexLexer) string {
		f := slug(name) + ".xml"
		if err := os.WriteFile(filepath.Join(dir, "generated", f), []byte(lexerXML(l)), 0o644); err != nil {
			t.Fatal(err)
		}
		return f
	}
	for i, l := range GlobalLexerRegistry.Lexers {
		cfg := l.Config()
		kind := fmt.Sprintf("%T", l)
		desc := ""
		switch x := l.(type) {
		case *RegexLexer:
			f, ok := embeddedNames[cfg.Name]
			same := false
			if ok {
				fresh := MustNewXMLLexer(embedded, "embedded/"+f)
				same = lexerXML(fresh) == lexerXML(x)
			}
			if ok && same {
				desc = "xml " + f
			} else {
				desc = "gen " + write(cfg.Name, x)
			}
		default:
			v := reflect.ValueOf(l).Elem()
			switch kind {
			case "*chroma.delegatingLexer":
				root := field(v, "root").Interface().(Lexer)
				lang := field(v, "language").Interface().(Lexer)
				rd := "xml " + embeddedNames[root.Config().Name]
				ld := "gen " + write(cfg.Name+" language", lang.(*RegexLexer))
				desc = "delegating " + rd + " | " + ld
			case "*chroma.remappingLexer":
				in := field(v, "lexer").Interface().(Lexer)
				desc = "remapping " + embeddedNames[in.Config().Name]
			case "*lexers.markdownLexer", "*lexers.httpBodyContentTyper":
				desc = "wrapper gen " + write(cfg.Name, inner(l).(*RegexLexer))
			default:
				t.Fatalf("unknown %s", kind)
			}
		}

		fmt.Fprintf(&reg, "%d\t%s\t%s\n", i, cfg.Name, desc)
	}
	if err := os.WriteFile(filepath.Join(dir, "registry.tsv"), []byte(reg.String()), 0o644); err != nil {
		t.Fatal(err)
	}

	// Remapping tables (word → type) for the two TypeRemappingLexers.
	remap := func(mapping TypeMapping) string {
		lut := map[string]TokenType{}
		for _, rt := range mapping {
			for _, k := range rt.Words {
				lut[k] = rt.To
			}
		}
		keys := make([]string, 0, len(lut))
		for k := range lut {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		var b strings.Builder
		for _, k := range keys {
			fmt.Fprintf(&b, "%s\t%s\n", k, lut[k].String())
		}
		return b.String()
	}
	_ = os.WriteFile(filepath.Join(dir, "remap_common_lisp.tsv"), []byte(remap(TypeMapping{
		{NameVariable, NameFunction, clBuiltinFunctions},
		{NameVariable, Keyword, clSpecialForms},
		{NameVariable, NameBuiltin, clMacros},
		{NameVariable, Keyword, clLambdaListKeywords},
		{NameVariable, Keyword, clDeclarations},
		{NameVariable, KeywordType, clBuiltinTypes},
		{NameVariable, NameClass, clBuiltinClasses},
	})), 0o644)
	_ = os.WriteFile(filepath.Join(dir, "remap_emacslisp.tsv"), []byte(remap(TypeMapping{
		{NameVariable, NameFunction, emacsBuiltinFunction},
		{NameVariable, NameBuiltin, emacsSpecialForms},
		{NameVariable, NameException, emacsErrorKeywords},
		{NameVariable, NameBuiltin, append(emacsBuiltinFunctionHighlighted, emacsMacros...)},
		{NameVariable, KeywordPseudo, emacsLambdaListKeywords},
	})), 0o644)

	Raku.(*RegexLexer).MustRules()
	keys := make([]int, 0, len(RakuBrackets))
	for k := range RakuBrackets {
		keys = append(keys, int(k))
	}
	sort.Ints(keys)
	var rb strings.Builder
	for _, k := range keys {
		fmt.Fprintf(&rb, "%X\t%X\n", k, int(RakuBrackets[rune(k)]))
	}
	_ = os.WriteFile(filepath.Join(dir, "raku_brackets.tsv"), []byte(rb.String()), 0o644)

	// Token type names.
	var tt strings.Builder
	for _, v := range TokenTypeValues() {
		fmt.Fprintf(&tt, "%d\t%s\n", int(v), v.String())
	}
	_ = os.WriteFile(filepath.Join(dir, "token_types.tsv"), []byte(tt.String()), 0o644)

	// Per-lexer rule fingerprints (raw rules, before include expansion).
	var fp strings.Builder
	for _, l := range GlobalLexerRegistry.Lexers {
		var rl *RegexLexer
		switch x := l.(type) {
		case *RegexLexer:
			rl = x
		default:
			continue
		}
		var b strings.Builder
		rulesXML(&b, rl.MustRules())
		h := fnv.New64a()
		h.Write([]byte(b.String()))
		fmt.Fprintf(&fp, "%s\t%016x\n", l.Config().Name, h.Sum64())
	}
	_ = os.WriteFile(filepath.Join(dir, "rule_fingerprints.tsv"), []byte(fp.String()), 0o644)
	_ = json.Marshal
	_ = html.EscapeString
}
GO

cat > lexers/zz_fixture_test.go <<'GO'
package lexers

import (
	"bufio"
	"encoding/json"
	"html"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/alecthomas/chroma/v2"
)

// Mirrors recur/internal/git/highlight.go exactly.
func recurHighlightLine(path, code string) string {
	if code == "" {
		return ""
	}
	key := filepath.Base(path)
	if key == "" {
		key = path
	}
	lexer := Match(key)
	if lexer == nil {
		return html.EscapeString(code)
	}
	lexer = chroma.Coalesce(lexer)
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
		if class := recurTokenClass(tok.Type); class != "" {
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

func recurTokenClass(t chroma.TokenType) string {
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

func samplePath(glob string) string {
	if strings.ContainsAny(glob, "[?\\") {
		return ""
	}
	return strings.ReplaceAll(glob, "*", "sample")
}

type hlCase struct {
	Path string `json:"path"`
	Code string `json:"code"`
	HTML string `json:"html"`
}

func TestHighlightFixture(t *testing.T) {
	dir := os.Getenv("DUMP_DIR")
	var cases []hlCase
	seenPath := map[string]bool{}
	files, _ := filepath.Glob("testdata/*.actual")
	sub, _ := filepath.Glob("testdata/*/*.actual")
	files = append(files, sub...)
	sort.Strings(files)
	for _, f := range files {
		if strings.Contains(f, "analysis") {
			continue
		}
		name := strings.Split(filepath.Base(f), ".")[0]
		if d := filepath.Base(filepath.Dir(f)); d != "testdata" {
			name = d
		}
		lexer := Get(name)
		if lexer == nil {
			continue
		}
		path := ""
		for _, g := range lexer.Config().Filenames {
			if p := samplePath(g); p != "" {
				path = "src/" + p
				break
			}
		}
		if path == "" || seenPath[path+f] {
			continue
		}
		seenPath[path+f] = true
		fh, err := os.Open(f)
		if err != nil {
			t.Fatal(err)
		}
		sc := bufio.NewScanner(fh)
		sc.Buffer(make([]byte, 1<<20), 1<<20)
		n := 0
		for sc.Scan() && n < 8 {
			line := sc.Text()
			if strings.TrimSpace(line) == "" || len(line) > 240 {
				continue
			}
			done := make(chan string, 1)
			go func() { done <- recurHighlightLine(path, line) }()
			select {
			case h := <-done:
				cases = append(cases, hlCase{path, line, h})
			case <-time.After(5 * time.Second):
				println("HANG", f, path, line)
			}
			n++
		}
		fh.Close()
	}
	extra := []hlCase{
		{Path: "src/auth/oauth.ts", Code: "const secret = await secrets.get('oauth/client_secret')"},
		{Path: "NOTES", Code: "<b>hi</b>"},
		{Path: "a.go", Code: ""},
		{Path: "main.go", Code: "\tfmt.Println(\"héllo\", 42, 0x1F) // done"},
		{Path: "main.go", Code: "x := `raw {{ .Name }} string`"},
		{Path: "App.svelte", Code: "<button on:click|preventDefault={() => count++}>{count}</button>"},
		{Path: "lib.rs", Code: "pub fn f<'a>(x: &'a str) -> Option<&'a str> { Some(x) }"},
		{Path: "x.py", Code: "def f(a, *args, **kw): return f\"{a!r}\"  # comment"},
		{Path: "x.sql", Code: "SELECT id, name FROM users WHERE id = 1; -- note"},
		{Path: "x.css", Code: ".a > b:hover { color: #fff; margin: 0 auto !important; }"},
		{Path: "README.md", Code: "## Title with `code` and **bold** [link](http://x)"},
		{Path: "x.yaml", Code: "key: [1, two, \"three\"]  # c"},
		{Path: "x.json", Code: "{\"a\": [true, null, 1.5e3]}"},
		{Path: "x.sh", Code: "for f in *.go; do echo \"$f\"; done"},
		{Path: "x.php", Code: "<?php echo $x->y(); ?>"},
		{Path: "Dockerfile", Code: "RUN apt-get install -y curl && rm -rf /var/lib/apt"},
		{Path: "Makefile", Code: "build: deps\n\tgo build ./..."},
		{Path: "x.raku", Code: "my $x = q:to/END/;"},
		{Path: "x.go", Code: "\x00weird\xffbytes"},
		{Path: "x.ts", Code: "let s = \"tab\there\" + 'quote\\'s' /* c */;"},
	}
	for _, c := range extra {
		c.HTML = recurHighlightLine(c.Path, c.Code)
		cases = append(cases, c)
	}
	data, _ := json.MarshalIndent(cases, "", " ")
	_ = os.WriteFile(filepath.Join(dir, "highlight_fixture.json"), data, 0o644)

	// Filename → lowercase lexer name, for every glob that names a file.
	names := map[string]string{}
	for _, l := range GlobalLexerRegistry.Lexers {
		for _, g := range append(append([]string{}, l.Config().Filenames...), l.Config().AliasFilenames...) {
			p := samplePath(g)
			if p == "" {
				continue
			}
			for _, cand := range []string{p, "dir/" + p, p + ".bak", p + "~", p + ".in"} {
				m := Match(filepath.Base(cand))
				if m == nil {
					names[cand] = ""
				} else {
					names[cand] = strings.ToLower(m.Config().Name)
				}
			}
		}
	}
	names["NOTES"] = ""
	data, _ = json.MarshalIndent(names, "", " ")
	_ = os.WriteFile(filepath.Join(dir, "lexer_names.json"), data, 0o644)
}
GO

out="$work/out"
mkdir -p "$out"
DUMP_DIR="$out" go test ./lexers -run 'TestDump|TestHighlightFixture' -count=1 >/dev/null

rm -rf "$crate/chroma/lexers" "$crate/chroma/generated"
mkdir -p "$crate/chroma/lexers" "$crate/chroma/generated" "$crate/tests/fixtures"
rm -f "$crate"/chroma/*.tsv "$crate/chroma/COPYING"
cp "$src"/lexers/embedded/*.xml "$crate/chroma/lexers/"
cp "$out"/generated/*.xml "$crate/chroma/generated/"
cp "$out"/remap_*.tsv "$out"/raku_brackets.tsv "$src/COPYING" "$crate/chroma/"
cp "$out"/highlight_fixture.json "$out"/lexer_names.json "$crate/tests/fixtures/"
chmod -R u+w "$crate/chroma" "$crate/tests/fixtures"

# Unicode tables (Go's unicode package) for the regexp2 port.
mkdir -p "$work/unicode" && cd "$work/unicode"
cat > main.go <<'GO'
// Generates rust/crates/git/src/regex/unicode_tables.rs from Go's unicode package.
package main

import (
	"fmt"
	"os"
	"sort"
	"strings"
	"unicode"
)

func ranges(t *unicode.RangeTable) [][2]rune {
	var out [][2]rune
	for _, r := range t.R16 {
		for c := rune(r.Lo); c <= rune(r.Hi); c += rune(r.Stride) {
			out = appendRune(out, c)
		}
	}
	for _, r := range t.R32 {
		for c := rune(r.Lo); c <= rune(r.Hi); c += rune(r.Stride) {
			out = appendRune(out, c)
		}
	}
	return out
}

func appendRune(out [][2]rune, c rune) [][2]rune {
	if c >= 0xD800 && c <= 0xDFFF {
		return out // Rust chars are never surrogates.
	}
	if n := len(out); n > 0 && out[n-1][1]+1 == c {
		out[n-1][1] = c
		return out
	}
	return append(out, [2]rune{c, c})
}

func lit(r rune) string { return fmt.Sprintf("'\\u{%X}'", r) }

func main() {
	var b strings.Builder
	fmt.Fprintf(&b, "//! Unicode %s tables from Go's `unicode` package (the tables regexp2 and\n", unicode.Version)
	b.WriteString("//! Go's `strings`/`unicode` functions use), generated by a small Go program\n")
	b.WriteString("//! walking `unicode.Categories`, `unicode.CategoryAliases`, `unicode.ToLower`,\n")
	b.WriteString("//! `unicode.ToUpper` and `unicode.SimpleFold`.\n\n")
	names := make([]string, 0, len(unicode.Categories))
	for k := range unicode.Categories {
		names = append(names, k)
	}
	sort.Strings(names)
	for _, k := range names {
		fmt.Fprintf(&b, "static CAT_%s: &[(char, char)] = &[", k)
		for i, r := range ranges(unicode.Categories[k]) {
			if i%4 == 0 {
				b.WriteString("\n   ")
			}
			fmt.Fprintf(&b, " (%s, %s),", lit(r[0]), lit(r[1]))
		}
		b.WriteString("\n];\n\n")
	}
	b.WriteString("/// General categories by name, sorted.\npub(crate) static CATEGORIES: &[(&str, &[(char, char)])] = &[\n")
	for _, k := range names {
		fmt.Fprintf(&b, "    (%q, CAT_%s),\n", k, k)
	}
	b.WriteString("];\n\n")
	aliases := make([]string, 0, len(unicode.CategoryAliases))
	for k := range unicode.CategoryAliases {
		aliases = append(aliases, k)
	}
	sort.Strings(aliases)
	b.WriteString("/// Long category names to their short names, sorted.\npub(crate) static CATEGORY_ALIASES: &[(&str, &str)] = &[\n")
	for _, k := range aliases {
		fmt.Fprintf(&b, "    (%q, %q),\n", k, unicode.CategoryAliases[k])
	}
	b.WriteString("];\n\n")
	var lower, upper, fold [][2]rune
	for c := rune(0); c <= unicode.MaxRune; c++ {
		if c >= 0xD800 && c <= 0xDFFF {
			continue
		}
		if l := unicode.ToLower(c); l != c {
			lower = append(lower, [2]rune{c, l})
		}
		if u := unicode.ToUpper(c); u != c {
			upper = append(upper, [2]rune{c, u})
		}
		if f := unicode.SimpleFold(c); f != c {
			fold = append(fold, [2]rune{c, f})
		}
	}
	for _, t := range []struct {
		name, doc string
		rows      [][2]rune
	}{
		{"TO_LOWER", "`unicode.ToLower` for every rune it changes, sorted.", lower},
		{"TO_UPPER", "`unicode.ToUpper` for every rune it changes, sorted.", upper},
		{"SIMPLE_FOLD", "`unicode.SimpleFold` for every rune it changes, sorted.", fold},
	} {
		fmt.Fprintf(&b, "/// %s\npub(crate) static %s: &[(char, char)] = &[", t.doc, t.name)
		for i, r := range t.rows {
			if i%4 == 0 {
				b.WriteString("\n   ")
			}
			fmt.Fprintf(&b, " (%s, %s),", lit(r[0]), lit(r[1]))
		}
		b.WriteString("\n];\n\n")
	}
	os.Stdout.WriteString(strings.TrimRight(b.String(), "\n") + "\n")
}
GO
{ echo '#![allow(non_upper_case_globals)]'; echo; go run main.go; } > "$crate/src/chroma/regexp2/unicode_tables.rs"

# HTML named character references (Go's html package).
python3 - "$(go env GOROOT)/src/html/entity.go" "$crate/src/html_entities.rs" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
e1 = src[src.index("entity = map[string]rune{"):src.index("entity2 = map")]
e2 = src[src.index("entity2 = map"):]
R = r"'\\(?:U([0-9A-Fa-f]{8})|u([0-9A-Fa-f]{4}))'"
cp = lambda a, b: int(a or b, 16)
one = [(k, cp(a, b)) for k, a, b in re.findall(r'^\s*"([^"]+)":\s*' + R, e1, re.M)]
two = [(k, cp(a, b), cp(c, d)) for k, a, b, c, d in re.findall(r'^\s*"([^"]+)":\s*\{' + R + r',\s*' + R + r'\}', e2, re.M)]
out = ["//! Go `html` named character references, generated from Go's `src/html/entity.go`.", "",
       "/// Sorted by name for binary search.", "pub(crate) static ENTITY: &[(&str, char)] = &["]
out += [f'    ("{k}", \'\\u{{{v:X}}}\'),' for k, v in sorted(one)]
out += ["];", "", "pub(crate) static ENTITY2: &[(&str, [char; 2])] = &["]
out += [f'    ("{k}", [\'\\u{{{a:X}}}\', \'\\u{{{b:X}}}\']),' for k, a, b in sorted(two)]
out += ["];"]
open(sys.argv[2], "w").write("\n".join(out) + "\n")
PY

(cd "$crate" && cargo fmt -p recur-git)
echo "regenerated from chroma $version"
