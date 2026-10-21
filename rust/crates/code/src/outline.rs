//! Heuristic per-language outline: top-level definitions found by line
//! regexes (no parser).

use std::fs::{self, File};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::LazyLock;

use recur_domain::OutlineSymbol;
use regex::Regex;

use crate::error::{Error, Result};
use crate::file::image_mime;
use crate::gore::must;
use crate::root::{path_str, safe_join_str};
use crate::scan::{ScanError, scan_lines};

/// Caps the symbols reported for one file.
const MAX_OUTLINE_SYMBOLS: usize = 200;

struct Rules {
    go_func: Regex,
    go_type: Regex,
    go_const: Regex,
    go_var: Regex,
    ts_func: Regex,
    ts_method: Regex,
    svelte_exp: Regex,
    ts: Vec<(Regex, &'static str)>,
}

static RULES: LazyLock<Rules> = LazyLock::new(|| Rules {
    go_func: must(r"^func\s+(\([^)]+\)\s+)?([A-Za-z_][\w]*)\s*\("),
    go_type: must(r"^type\s+([A-Za-z_][\w]*)\s+"),
    go_const: must(r"^const\s+([A-Za-z_][\w]*)\s*="),
    go_var: must(r"^var\s+([A-Za-z_][\w]*)\s"),
    ts_func: must(TS_FUNC),
    ts_method: must(r"^\s+(?:async\s+)?([A-Za-z_][\w]*)\s*\([^)]*\)\s*[{:]"),
    svelte_exp: must(r"^(?:export\s+)?(?:let|const)\s+([A-Za-z_][\w]*)"),
    ts: vec![
        (must(TS_FUNC), "func"),
        (
            must(r"^(?:export\s+)?(?:abstract\s+)?class\s+([A-Za-z_][\w]*)"),
            "class",
        ),
        (
            must(r"^(?:export\s+)?(?:type|interface)\s+([A-Za-z_][\w]*)"),
            "type",
        ),
        (
            must(r"^(?:export\s+)?(?:const|let)\s+([A-Za-z_][\w]*)\s*=\s*(?:async\s*)?\("),
            "func",
        ),
        (
            must(r"^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_][\w]*)\s*[:=]"),
            "const",
        ),
    ],
});

const TS_FUNC: &str = r"^(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_][\w]*)\s*[<(]";

const PY_DEF: &str = r"^(?:async\s+)?def\s+([A-Za-z_][\w]*)\s*\(";
const PY_CLASS: &str = r"^class\s+([A-Za-z_][\w]*)\s*[:(]";
const RS_FN: &str =
    r"^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?fn\s+([A-Za-z_][\w]*)";
const RS_STRUCT: &str = r"^(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][\w]*)";
const RS_ENUM: &str = r"^(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][\w]*)";
const RS_TRAIT: &str = r"^(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][\w]*)";
const RS_TYPE: &str = r"^(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_][\w]*)";
const RS_MOD: &str = r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][\w]*)";
const RS_CONST: &str = r"^(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Za-z_][\w]*)\s*:";
const JVM_CLASS: &str = r"^(?:public\s+|protected\s+|private\s+|internal\s+)?(?:abstract\s+|final\s+|sealed\s+|static\s+|partial\s+)*class\s+([A-Za-z_][\w]*)";
const JVM_IFACE: &str = r"^(?:public\s+|protected\s+|private\s+|internal\s+)?(?:abstract\s+)?interface\s+([A-Za-z_][\w]*)";
const JVM_ENUM: &str =
    r"^(?:public\s+|protected\s+|private\s+|internal\s+)?enum\s+([A-Za-z_][\w]*)";
const JVM_RECORD: &str = r"^(?:public\s+|protected\s+|private\s+)?record\s+([A-Za-z_][\w]*)";
const CS_STRUCT: &str = r"^(?:public\s+|protected\s+|private\s+|internal\s+)?(?:readonly\s+|ref\s+)?struct\s+([A-Za-z_][\w]*)";
const JVM_METHOD: &str = r"^(?:public|protected|private|internal|static|final|abstract|override|virtual|async|synchronized|native|sealed|partial|[\s]+)+\s*[\w.<>\[\],?]+\s+([A-Za-z_][\w]*)\s*\(";
const KT_FUN: &str = r"^(?:(?:public|private|internal|protected|override|suspend|inline|open|abstract|final|operator|infix)\s+)*fun\s+(?:[A-Za-z_][\w.]*\.)?([A-Za-z_][\w]*)";
const KT_CLASS: &str = r"^(?:(?:public|private|internal|protected|open|abstract|data|sealed|inner|value|annotation)\s+)*(?:class|interface|object)\s+([A-Za-z_][\w]*)";
const SWIFT_FUNC: &str = r"^(?:(?:public|private|internal|fileprivate|open|static|class|override|mutating|nonmutating|async|throws|rethrows)\s+)*func\s+([A-Za-z_][\w]*)";
const SWIFT_TYPE: &str = r"^(?:(?:public|private|internal|fileprivate|open)\s+)*(?:class|struct|enum|protocol|actor|extension)\s+([A-Za-z_][\w]*)";
const CPP_TYPE: &str = r"^(?:class|struct|enum(?:\s+class)?|union|namespace)\s+([A-Za-z_][\w]*)";
const CPP_FUNC: &str = r"^(?:(?:static|inline|constexpr|virtual|explicit|extern|unsigned|signed|long|short)\s+)*[\w:<>\*&]+\s+([A-Za-z_][\w]*)\s*\(";
const RB_DEF: &str = r"^\s*def\s+(?:self\.)?([A-Za-z_][\w?!]*)";
const RB_CLASS: &str = r"^\s*class\s+([A-Za-z_][\w:]*)";
const RB_MODULE: &str = r"^\s*module\s+([A-Za-z_][\w:]*)";
const PHP_FUNC: &str =
    r"^(?:public|private|protected|static|final|abstract|\s)*function\s+&?([A-Za-z_][\w]*)";
const PHP_CLASS: &str =
    r"^(?:abstract\s+|final\s+)?(?:class|interface|trait|enum)\s+([A-Za-z_][\w]*)";
const DART_CLASS: &str =
    r"^(?:abstract\s+|base\s+|final\s+|sealed\s+|mixin\s+)?class\s+([A-Za-z_][\w]*)";
const DART_MIXIN: &str = r"^mixin\s+([A-Za-z_][\w]*)";
const DART_FUNC: &str = r"^(?:(?:static|Future|void|int|double|bool|String|dynamic|var|final|const|[A-Z][\w.<>,\s?]+)\s+)+([a-z_][\w]*)\s*\(";
const SCALA_DEF: &str = r"^(?:(?:private|protected|override|final|lazy|implicit)\s+)*(?:def|val|var)\s+([A-Za-z_][\w]*)";
const SCALA_TYPE: &str = r"^(?:(?:private|protected|sealed|abstract|final|case)\s+)*(?:class|object|trait|enum)\s+([A-Za-z_][\w]*)";
const ZIG_FN: &str = r"^(?:pub\s+)?(?:export\s+)?(?:inline\s+)?fn\s+([A-Za-z_][\w]*)";
const ZIG_CONST: &str = r"^(?:pub\s+)?const\s+([A-Za-z_][\w]*)\s*=";
const EX_DEF: &str = r"^\s*defp?\s+([a-z_][\w?!]*)";
const EX_MOD: &str = r"^\s*defmodule\s+([A-Za-z_][\w.]*)";
const HS_TYPE: &str = r"^(?:data|newtype|type|class)\s+([A-Za-z_][\w']*)";
const HS_FUNC: &str = r"^([a-z_][\w']*)\s*::";
const LUA_FUNC: &str = r"^(?:local\s+)?function\s+([A-Za-z_][\w.]*)";

/// Per-extension rules for everything but Go, JS/TS/Vue and Svelte.
type ExtRules = Vec<(&'static [&'static str], Vec<(Regex, &'static str)>)>;

static EXT_RULES: LazyLock<ExtRules> = LazyLock::new(|| {
    let r = |p: &str, kind: &'static str| (must(p), kind);
    vec![
        (&[".py"][..], vec![r(PY_DEF, "func"), r(PY_CLASS, "class")]),
        (
            &[".rs"],
            vec![
                r(RS_FN, "func"),
                r(RS_STRUCT, "struct"),
                r(RS_ENUM, "enum"),
                r(RS_TRAIT, "trait"),
                r(RS_TYPE, "type"),
                r(RS_MOD, "mod"),
                r(RS_CONST, "const"),
            ],
        ),
        (
            &[".java"],
            vec![
                r(JVM_CLASS, "class"),
                r(JVM_IFACE, "interface"),
                r(JVM_ENUM, "enum"),
                r(JVM_RECORD, "record"),
                r(JVM_METHOD, "method"),
            ],
        ),
        (
            &[".kt", ".kts"],
            vec![r(KT_FUN, "func"), r(KT_CLASS, "type")],
        ),
        (
            &[".cs"],
            vec![
                r(JVM_CLASS, "class"),
                r(JVM_IFACE, "interface"),
                r(JVM_ENUM, "enum"),
                r(JVM_RECORD, "record"),
                r(CS_STRUCT, "struct"),
                r(JVM_METHOD, "method"),
            ],
        ),
        (
            &[".swift"],
            vec![r(SWIFT_FUNC, "func"), r(SWIFT_TYPE, "type")],
        ),
        (
            &[".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hxx", ".hh"],
            vec![r(CPP_TYPE, "type"), r(CPP_FUNC, "func")],
        ),
        (&[".m"], vec![r(CPP_TYPE, "type"), r(SWIFT_FUNC, "func")]),
        (
            &[".mm"],
            vec![
                r(CPP_TYPE, "type"),
                r(SWIFT_FUNC, "func"),
                r(CPP_FUNC, "func"),
            ],
        ),
        (
            &[".rb"],
            vec![
                r(RB_DEF, "func"),
                r(RB_CLASS, "class"),
                r(RB_MODULE, "module"),
            ],
        ),
        (&[".php"], vec![r(PHP_FUNC, "func"), r(PHP_CLASS, "type")]),
        (
            &[".dart"],
            vec![
                r(DART_CLASS, "class"),
                r(DART_MIXIN, "mixin"),
                r(DART_FUNC, "func"),
            ],
        ),
        (
            &[".scala"],
            vec![r(SCALA_DEF, "def"), r(SCALA_TYPE, "type")],
        ),
        (&[".zig"], vec![r(ZIG_FN, "func"), r(ZIG_CONST, "const")]),
        (
            &[".ex", ".exs"],
            vec![r(EX_DEF, "func"), r(EX_MOD, "module")],
        ),
        (&[".hs"], vec![r(HS_TYPE, "type"), r(HS_FUNC, "func")]),
        (&[".lua"], vec![r(LUA_FUNC, "func")]),
        (
            &[".groovy"],
            vec![
                r(JVM_CLASS, "class"),
                r(JVM_IFACE, "interface"),
                r(JVM_METHOD, "method"),
            ],
        ),
    ]
});

fn ext_rules(ext: &str) -> Option<&'static [(Regex, &'static str)]> {
    EXT_RULES
        .iter()
        .find(|(exts, _)| exts.contains(&ext))
        .map(|(_, rules)| rules.as_slice())
}

/// Top-level symbols of a source file under `root`. Images and directories
/// (submodule paths in changed-file lists) are a quiet empty result.
pub fn outline(root: &Path, rel: &str) -> Result<Vec<OutlineSymbol>> {
    if !image_mime(rel).is_empty() {
        return Ok(Vec::new());
    }
    let full = safe_join_str(&path_str(root), rel)?;
    let st = fs::metadata(&full).map_err(|e| Error::path("stat", &full, e))?;
    if st.is_dir() {
        return Ok(Vec::new());
    }
    let f = File::open(&full).map_err(|e| Error::path("open", &full, e))?;
    let ext = filepath_ext(rel).to_lowercase();
    let mut out = Vec::new();
    let res = scan_lines(f, |line_no, raw| {
        if let Some(sym) = outline_line(&ext, &String::from_utf8_lossy(raw)) {
            out.push(OutlineSymbol {
                name: sym.name,
                kind: sym.kind.into(),
                line: line_no as i64,
                ..Default::default()
            });
            if out.len() >= MAX_OUTLINE_SYMBOLS {
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    });
    match res {
        Ok(_) => Ok(out),
        Err(ScanError::Io(e)) => Err(Error::path("read", &full, e)),
        Err(e @ ScanError::TooLong) => Err(Error::msg(e.to_string())),
    }
}

pub(crate) struct MatchedSym {
    pub(crate) name: String,
    pub(crate) kind: &'static str,
}

/// One language's definition rules applied to one raw source line, shared by
/// [`outline`] and the whole-repo symbol walk so a line is parsed once.
pub(crate) fn outline_line(ext: &str, raw: &str) -> Option<MatchedSym> {
    let line = raw.trim_end_matches('\r');
    let trimmed = line.trim();
    if trimmed.is_empty() || is_comment_line(trimmed) {
        return None;
    }
    let r = &*RULES;
    let cap = |re: &Regex, s: &str, g: usize| {
        re.captures(s)
            .map(|c| c.get(g).map_or("", |m| m.as_str()).to_string())
    };
    match ext {
        ".go" => {
            let found = cap(&r.go_func, line, 2)
                .map(|n| (n, "func"))
                .or_else(|| cap(&r.go_type, line, 1).map(|n| (n, "type")))
                .or_else(|| cap(&r.go_const, line, 1).map(|n| (n, "const")))
                .or_else(|| cap(&r.go_var, line, 1).map(|n| (n, "var")));
            found.map(|(name, kind)| MatchedSym { name, kind })
        }
        ".ts" | ".tsx" | ".js" | ".jsx" | ".mjs" | ".cjs" | ".vue" => {
            if let Some(sym) = match_rules(trimmed, &r.ts) {
                return Some(sym);
            }
            match cap(&r.ts_method, line, 1) {
                Some(name) if !is_control_keyword(trimmed) => Some(MatchedSym {
                    name,
                    kind: "method",
                }),
                _ => None,
            }
        }
        ".svelte" => cap(&r.svelte_exp, trimmed, 1)
            .map(|name| MatchedSym {
                name,
                kind: "export",
            })
            .or_else(|| cap(&r.ts_func, trimmed, 1).map(|name| MatchedSym { name, kind: "func" })),
        _ => {
            let rules = ext_rules(ext)?;
            // Only column-0 C-family functions, to cut noise.
            let c_family = matches!(
                ext,
                ".c" | ".h" | ".cc" | ".cpp" | ".cxx" | ".hpp" | ".hxx" | ".hh"
            );
            let target = if c_family {
                if line.starts_with([' ', '\t']) {
                    return None;
                }
                line
            } else {
                trimmed
            };
            if target.is_empty() {
                return None;
            }
            match_rules(target, rules)
        }
    }
}

fn match_rules(line: &str, rules: &[(Regex, &'static str)]) -> Option<MatchedSym> {
    for (re, kind) in rules {
        let Some(c) = re.captures(line) else {
            continue;
        };
        let name = c.get(1).map_or("", |m| m.as_str()).trim_matches('`');
        if name.is_empty() || is_control_keyword(name) {
            continue;
        }
        return Some(MatchedSym {
            name: name.to_string(),
            kind,
        });
    }
    None
}

fn is_comment_line(trimmed: &str) -> bool {
    ["//", "#", "*", "/*", "--", "<!--"]
        .iter()
        .any(|p| trimmed.starts_with(p))
}

fn is_control_keyword(s: &str) -> bool {
    matches!(
        s.split_whitespace().next(),
        Some(
            "if" | "for"
                | "while"
                | "switch"
                | "catch"
                | "else"
                | "return"
                | "throw"
                | "new"
                | "typeof"
                | "delete"
                | "await"
                | "yield"
                | "case"
                | "default"
                | "import"
                | "export"
                | "from"
                | "package"
                | "using"
                | "try"
                | "finally"
                | "do"
                | "with"
                | "assert"
                | "elif"
                | "except"
                | "raise"
                | "pass"
                | "break"
                | "continue"
                | "goto"
                | "select"
                | "defer"
                | "go"
                | "range"
                | "match"
                | "when"
        )
    )
}

/// Everything from the last `.` in the path (even inside a directory name),
/// as the Go outline did.
pub(crate) fn filepath_ext(p: &str) -> &str {
    p.rfind('.').map_or("", |i| &p[i..])
}
