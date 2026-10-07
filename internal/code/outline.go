package code

import (
	"bufio"
	"os"
	"regexp"
	"strings"

	"recur/internal/domain"
)

type outlineRule struct {
	re    *regexp.Regexp
	kind  string
	group int // 1-based capture group for the name
}

func rule(re *regexp.Regexp, kind string) outlineRule {
	return outlineRule{re: re, kind: kind, group: 1}
}

var (
	// Go
	reGoFunc  = regexp.MustCompile(`^func\s+(\([^)]+\)\s+)?([A-Za-z_][\w]*)\s*\(`)
	reGoType  = regexp.MustCompile(`^type\s+([A-Za-z_][\w]*)\s+`)
	reGoConst = regexp.MustCompile(`^const\s+([A-Za-z_][\w]*)\s*=`)
	reGoVar   = regexp.MustCompile(`^var\s+([A-Za-z_][\w]*)\s`)

	// JS / TS / Vue script
	reTSFunc   = regexp.MustCompile(`^(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_][\w]*)\s*[<(]`)
	reTSClass  = regexp.MustCompile(`^(?:export\s+)?(?:abstract\s+)?class\s+([A-Za-z_][\w]*)`)
	reTSConst  = regexp.MustCompile(`^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_][\w]*)\s*[:=]`)
	reTSType   = regexp.MustCompile(`^(?:export\s+)?(?:type|interface)\s+([A-Za-z_][\w]*)`)
	reTSMethod = regexp.MustCompile(`^\s+(?:async\s+)?([A-Za-z_][\w]*)\s*\([^)]*\)\s*[{:]`)
	reTSArrow  = regexp.MustCompile(`^(?:export\s+)?(?:const|let)\s+([A-Za-z_][\w]*)\s*=\s*(?:async\s*)?\(`)

	reSvelteExp = regexp.MustCompile(`^(?:export\s+)?(?:let|const)\s+([A-Za-z_][\w]*)`)

	// Python
	rePyDef   = regexp.MustCompile(`^(?:async\s+)?def\s+([A-Za-z_][\w]*)\s*\(`)
	rePyClass = regexp.MustCompile(`^class\s+([A-Za-z_][\w]*)\s*[:(]`)

	// Rust
	reRsFn     = regexp.MustCompile(`^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?fn\s+([A-Za-z_][\w]*)`)
	reRsStruct = regexp.MustCompile(`^(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][\w]*)`)
	reRsEnum   = regexp.MustCompile(`^(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][\w]*)`)
	reRsTrait  = regexp.MustCompile(`^(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][\w]*)`)
	reRsType   = regexp.MustCompile(`^(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_][\w]*)`)
	reRsMod    = regexp.MustCompile(`^(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][\w]*)`)
	reRsConst  = regexp.MustCompile(`^(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Za-z_][\w]*)\s*:`)

	// Java / C# / similar
	reJVMClass  = regexp.MustCompile(`^(?:public\s+|protected\s+|private\s+|internal\s+)?(?:abstract\s+|final\s+|sealed\s+|static\s+|partial\s+)*class\s+([A-Za-z_][\w]*)`)
	reJVMIface  = regexp.MustCompile(`^(?:public\s+|protected\s+|private\s+|internal\s+)?(?:abstract\s+)?interface\s+([A-Za-z_][\w]*)`)
	reJVMEnum   = regexp.MustCompile(`^(?:public\s+|protected\s+|private\s+|internal\s+)?enum\s+([A-Za-z_][\w]*)`)
	reJVMRecord = regexp.MustCompile(`^(?:public\s+|protected\s+|private\s+)?record\s+([A-Za-z_][\w]*)`)
	reCSStruct  = regexp.MustCompile(`^(?:public\s+|protected\s+|private\s+|internal\s+)?(?:readonly\s+|ref\s+)?struct\s+([A-Za-z_][\w]*)`)
	reJVMMethod = regexp.MustCompile(`^(?:public|protected|private|internal|static|final|abstract|override|virtual|async|synchronized|native|sealed|partial|[\s]+)+\s*[\w.<>\[\],?]+\s+([A-Za-z_][\w]*)\s*\(`)

	// Kotlin
	reKtFun   = regexp.MustCompile(`^(?:(?:public|private|internal|protected|override|suspend|inline|open|abstract|final|operator|infix)\s+)*fun\s+(?:[A-Za-z_][\w.]*\.)?([A-Za-z_][\w]*)`)
	reKtClass = regexp.MustCompile(`^(?:(?:public|private|internal|protected|open|abstract|data|sealed|inner|value|annotation)\s+)*(?:class|interface|object)\s+([A-Za-z_][\w]*)`)

	// Swift
	reSwiftFunc = regexp.MustCompile(`^(?:(?:public|private|internal|fileprivate|open|static|class|override|mutating|nonmutating|async|throws|rethrows)\s+)*func\s+([A-Za-z_][\w]*)`)
	reSwiftType = regexp.MustCompile(`^(?:(?:public|private|internal|fileprivate|open)\s+)*(?:class|struct|enum|protocol|actor|extension)\s+([A-Za-z_][\w]*)`)

	// C / C++
	reCppType = regexp.MustCompile(`^(?:class|struct|enum(?:\s+class)?|union|namespace)\s+([A-Za-z_][\w]*)`)
	reCppFunc = regexp.MustCompile(`^(?:(?:static|inline|constexpr|virtual|explicit|extern|unsigned|signed|long|short)\s+)*[\w:<>\*&]+\s+([A-Za-z_][\w]*)\s*\(`)

	// Ruby
	reRbDef    = regexp.MustCompile(`^\s*def\s+(?:self\.)?([A-Za-z_][\w?!]*)`)
	reRbClass  = regexp.MustCompile(`^\s*class\s+([A-Za-z_][\w:]*)`)
	reRbModule = regexp.MustCompile(`^\s*module\s+([A-Za-z_][\w:]*)`)

	// PHP
	rePHPFunc  = regexp.MustCompile(`^(?:public|private|protected|static|final|abstract|\s)*function\s+&?([A-Za-z_][\w]*)`)
	rePHPClass = regexp.MustCompile(`^(?:abstract\s+|final\s+)?(?:class|interface|trait|enum)\s+([A-Za-z_][\w]*)`)

	// Dart
	reDartClass = regexp.MustCompile(`^(?:abstract\s+|base\s+|final\s+|sealed\s+|mixin\s+)?class\s+([A-Za-z_][\w]*)`)
	reDartMixin = regexp.MustCompile(`^mixin\s+([A-Za-z_][\w]*)`)
	reDartFunc  = regexp.MustCompile(`^(?:(?:static|Future|void|int|double|bool|String|dynamic|var|final|const|[A-Z][\w.<>,\s?]+)\s+)+([a-z_][\w]*)\s*\(`)

	// Scala
	reScalaDef  = regexp.MustCompile(`^(?:(?:private|protected|override|final|lazy|implicit)\s+)*(?:def|val|var)\s+([A-Za-z_][\w]*)`)
	reScalaType = regexp.MustCompile(`^(?:(?:private|protected|sealed|abstract|final|case)\s+)*(?:class|object|trait|enum)\s+([A-Za-z_][\w]*)`)

	// Zig
	reZigFn    = regexp.MustCompile(`^(?:pub\s+)?(?:export\s+)?(?:inline\s+)?fn\s+([A-Za-z_][\w]*)`)
	reZigConst = regexp.MustCompile(`^(?:pub\s+)?const\s+([A-Za-z_][\w]*)\s*=`)

	// Elixir
	reExDef = regexp.MustCompile(`^\s*defp?\s+([a-z_][\w?!]*)`)
	reExMod = regexp.MustCompile(`^\s*defmodule\s+([A-Za-z_][\w.]*)`)

	// Haskell
	reHsType = regexp.MustCompile(`^(?:data|newtype|type|class)\s+([A-Za-z_][\w']*)`)
	reHsFunc = regexp.MustCompile(`^([a-z_][\w']*)\s*::`)

	// Lua
	reLuaFunc = regexp.MustCompile(`^(?:local\s+)?function\s+([A-Za-z_][\w.]*)`)
)

var outlineRules = map[string][]outlineRule{
	".py": {
		rule(rePyDef, "func"), rule(rePyClass, "class"),
	},
	".rs": {
		rule(reRsFn, "func"), rule(reRsStruct, "struct"), rule(reRsEnum, "enum"),
		rule(reRsTrait, "trait"), rule(reRsType, "type"), rule(reRsMod, "mod"),
		rule(reRsConst, "const"),
	},
	".java": {
		rule(reJVMClass, "class"), rule(reJVMIface, "interface"),
		rule(reJVMEnum, "enum"), rule(reJVMRecord, "record"), rule(reJVMMethod, "method"),
	},
	".kt": {
		rule(reKtFun, "func"), rule(reKtClass, "type"),
	},
	".kts": {
		rule(reKtFun, "func"), rule(reKtClass, "type"),
	},
	".cs": {
		rule(reJVMClass, "class"), rule(reJVMIface, "interface"),
		rule(reJVMEnum, "enum"), rule(reJVMRecord, "record"),
		rule(reCSStruct, "struct"), rule(reJVMMethod, "method"),
	},
	".swift": {
		rule(reSwiftFunc, "func"), rule(reSwiftType, "type"),
	},
	".c":   {rule(reCppType, "type"), rule(reCppFunc, "func")},
	".h":   {rule(reCppType, "type"), rule(reCppFunc, "func")},
	".cc":  {rule(reCppType, "type"), rule(reCppFunc, "func")},
	".cpp": {rule(reCppType, "type"), rule(reCppFunc, "func")},
	".cxx": {rule(reCppType, "type"), rule(reCppFunc, "func")},
	".hpp": {rule(reCppType, "type"), rule(reCppFunc, "func")},
	".hxx": {rule(reCppType, "type"), rule(reCppFunc, "func")},
	".hh":  {rule(reCppType, "type"), rule(reCppFunc, "func")},
	".m":   {rule(reCppType, "type"), rule(reSwiftFunc, "func")},
	".mm":  {rule(reCppType, "type"), rule(reSwiftFunc, "func"), rule(reCppFunc, "func")},
	".rb": {
		rule(reRbDef, "func"), rule(reRbClass, "class"), rule(reRbModule, "module"),
	},
	".php": {
		rule(rePHPFunc, "func"), rule(rePHPClass, "type"),
	},
	".dart": {
		rule(reDartClass, "class"), rule(reDartMixin, "mixin"), rule(reDartFunc, "func"),
	},
	".scala": {
		rule(reScalaDef, "def"), rule(reScalaType, "type"),
	},
	".zig": {
		rule(reZigFn, "func"), rule(reZigConst, "const"),
	},
	".ex":  {rule(reExDef, "func"), rule(reExMod, "module")},
	".exs": {rule(reExDef, "func"), rule(reExMod, "module")},
	".hs":  {rule(reHsType, "type"), rule(reHsFunc, "func")},
	".lua": {rule(reLuaFunc, "func")},
	".groovy": {
		rule(reJVMClass, "class"), rule(reJVMIface, "interface"), rule(reJVMMethod, "method"),
	},
}

var tsOutlineRules = []outlineRule{
	rule(reTSFunc, "func"), rule(reTSClass, "class"), rule(reTSType, "type"),
	rule(reTSArrow, "func"), rule(reTSConst, "const"),
}

// maxOutlineSymbols caps the symbols reported for a single file.
const maxOutlineSymbols = 200

// Outline returns top-level symbols for a source file under root.
func Outline(root, rel string) ([]domain.OutlineSymbol, error) {
	if ImageMIME(rel) != "" {
		return nil, nil
	}
	full, err := SafeJoin(root, rel)
	if err != nil {
		return nil, err
	}
	st, err := os.Stat(full)
	if err != nil {
		return nil, err
	}
	// Changed-file lists can include git submodule paths (directories). Same
	// quiet empty result as images — callers shouldn't toast EISDIR.
	if st.IsDir() {
		return nil, nil
	}
	f, err := os.Open(full)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	ext := strings.ToLower(filepathExt(rel))
	var out []domain.OutlineSymbol
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 64*1024), 1024*1024)
	lineNo := 0
	for sc.Scan() {
		lineNo++
		sym, ok := outlineLine(ext, sc.Text())
		if !ok {
			continue
		}
		out = append(out, domain.OutlineSymbol{Name: sym.name, Kind: sym.kind, Line: lineNo})
		if len(out) >= maxOutlineSymbols {
			break
		}
	}
	return out, sc.Err()
}

// outlineLine applies one language's definition rules to a single raw source
// line. It is factored out of Outline so the whole-repo symbol walk
// (IndexSymbols) can decide "is this line a definition?" while it is already
// scanning the line for references — instead of re-reading and re-parsing every
// file a second time.
func outlineLine(ext, raw string) (matchedSym, bool) {
	line := strings.TrimRight(raw, "\r")
	trimmed := strings.TrimSpace(line)
	if trimmed == "" || isCommentLine(trimmed) {
		return matchedSym{}, false
	}
	switch ext {
	case ".go":
		if m := reGoFunc.FindStringSubmatch(line); m != nil {
			return matchedSym{name: m[2], kind: "func"}, true
		} else if m := reGoType.FindStringSubmatch(line); m != nil {
			return matchedSym{name: m[1], kind: "type"}, true
		} else if m := reGoConst.FindStringSubmatch(line); m != nil {
			return matchedSym{name: m[1], kind: "const"}, true
		} else if m := reGoVar.FindStringSubmatch(line); m != nil {
			return matchedSym{name: m[1], kind: "var"}, true
		}
	case ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".vue":
		if sym, ok := matchRules(trimmed, tsOutlineRules); ok {
			return sym, true
		} else if m := reTSMethod.FindStringSubmatch(line); m != nil && !isControlKeyword(trimmed) {
			return matchedSym{name: m[1], kind: "method"}, true
		}
	case ".svelte":
		if m := reSvelteExp.FindStringSubmatch(trimmed); m != nil {
			return matchedSym{name: m[1], kind: "export"}, true
		} else if m := reTSFunc.FindStringSubmatch(trimmed); m != nil {
			return matchedSym{name: m[1], kind: "func"}, true
		}
	default:
		rules, ok := outlineRules[ext]
		if !ok {
			return matchedSym{}, false
		}
		// Prefer column-0 matches for C-family funcs to cut noise.
		matchLine := trimmed
		if ext == ".c" || ext == ".h" || ext == ".cc" || ext == ".cpp" || ext == ".cxx" ||
			ext == ".hpp" || ext == ".hxx" || ext == ".hh" {
			if len(line) > 0 && (line[0] == ' ' || line[0] == '\t') {
				matchLine = "" // skip indented; only top-level
			} else {
				matchLine = line
			}
		}
		if matchLine != "" {
			if sym, ok := matchRules(matchLine, rules); ok {
				return sym, true
			}
		}
	}
	return matchedSym{}, false
}

type matchedSym struct {
	name string
	kind string
}

func matchRules(line string, rules []outlineRule) (matchedSym, bool) {
	for _, r := range rules {
		m := r.re.FindStringSubmatch(line)
		if m == nil || r.group >= len(m) {
			continue
		}
		name := strings.Trim(m[r.group], "`")
		if name == "" || isControlKeyword(name) {
			continue
		}
		return matchedSym{name: name, kind: r.kind}, true
	}
	return matchedSym{}, false
}

func isCommentLine(trimmed string) bool {
	return strings.HasPrefix(trimmed, "//") ||
		strings.HasPrefix(trimmed, "#") ||
		strings.HasPrefix(trimmed, "*") ||
		strings.HasPrefix(trimmed, "/*") ||
		strings.HasPrefix(trimmed, "--") ||
		strings.HasPrefix(trimmed, "<!--")
}

func isControlKeyword(s string) bool {
	fields := strings.Fields(s)
	if len(fields) == 0 {
		return false
	}
	switch fields[0] {
	case "if", "for", "while", "switch", "catch", "else", "return", "throw",
		"new", "typeof", "delete", "await", "yield", "case", "default",
		"import", "export", "from", "package", "using", "try", "finally",
		"do", "with", "assert", "elif", "except", "raise", "pass", "break",
		"continue", "goto", "select", "defer", "go", "range", "match", "when":
		return true
	}
	return false
}

func filepathExt(p string) string {
	i := strings.LastIndexByte(p, '.')
	if i < 0 {
		return ""
	}
	return p[i:]
}
