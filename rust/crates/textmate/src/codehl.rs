//! Lightweight per-line highlighting and line diffs for chat tool cards and
//! LSP hover code blocks (`codehl.ts`). Not a grammar: strings, comments,
//! numbers and keywords, tuned for card-sized snippets.

use crate::jsregex::JsRegex;

/// Token classes (`sh-*` in the web UI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShKind {
    Comment,
    String,
    Number,
    Keyword,
}

impl ShKind {
    pub fn class(self) -> &'static str {
        match self {
            ShKind::Comment => "sh-com",
            ShKind::String => "sh-str",
            ShKind::Number => "sh-num",
            ShKind::Keyword => "sh-kw",
        }
    }
}

struct Lang {
    kw: &'static [&'static str],
    line_comment: &'static [&'static str],
}

const KW_JS: &[&str] = &[
    "const",
    "let",
    "var",
    "function",
    "return",
    "if",
    "else",
    "for",
    "while",
    "switch",
    "case",
    "break",
    "continue",
    "new",
    "class",
    "extends",
    "import",
    "export",
    "from",
    "default",
    "async",
    "await",
    "try",
    "catch",
    "finally",
    "throw",
    "typeof",
    "instanceof",
    "in",
    "of",
    "delete",
    "void",
    "yield",
    "this",
    "super",
    "null",
    "undefined",
    "true",
    "false",
    "interface",
    "type",
    "enum",
    "implements",
    "readonly",
    "public",
    "private",
    "protected",
    "static",
    "as",
    "satisfies",
    "keyof",
    "namespace",
    "declare",
    "abstract",
    "unknown",
];
const KW_GO: &[&str] = &[
    "func",
    "return",
    "if",
    "else",
    "for",
    "range",
    "switch",
    "case",
    "default",
    "break",
    "continue",
    "go",
    "defer",
    "select",
    "chan",
    "map",
    "struct",
    "interface",
    "type",
    "var",
    "const",
    "package",
    "import",
    "goto",
    "fallthrough",
    "nil",
    "true",
    "false",
    "make",
    "new",
    "len",
    "cap",
    "append",
    "copy",
    "delete",
    "panic",
    "recover",
    "error",
    "string",
    "int",
    "int64",
    "int32",
    "uint",
    "byte",
    "rune",
    "bool",
    "float64",
    "float32",
    "any",
];
const KW_PY: &[&str] = &[
    "def", "return", "if", "elif", "else", "for", "while", "break", "continue", "import", "from",
    "as", "class", "try", "except", "finally", "raise", "with", "lambda", "pass", "yield",
    "global", "nonlocal", "assert", "del", "in", "is", "not", "and", "or", "None", "True", "False",
    "async", "await", "match", "case", "self",
];
const KW_RS: &[&str] = &[
    "fn", "let", "mut", "return", "if", "else", "for", "while", "loop", "match", "break",
    "continue", "impl", "trait", "struct", "enum", "use", "mod", "pub", "crate", "self", "super",
    "const", "static", "ref", "move", "async", "await", "dyn", "where", "type", "unsafe", "extern",
    "true", "false", "Some", "None", "Ok", "Err",
];
const KW_C: &[&str] = &[
    "int",
    "char",
    "float",
    "double",
    "void",
    "long",
    "short",
    "unsigned",
    "signed",
    "struct",
    "union",
    "enum",
    "typedef",
    "const",
    "static",
    "extern",
    "return",
    "if",
    "else",
    "for",
    "while",
    "do",
    "switch",
    "case",
    "break",
    "continue",
    "goto",
    "sizeof",
    "include",
    "define",
    "class",
    "public",
    "private",
    "protected",
    "virtual",
    "template",
    "typename",
    "namespace",
    "using",
    "new",
    "delete",
    "nullptr",
    "true",
    "false",
    "auto",
];
const KW_JAVA: &[&str] = &[
    "public",
    "private",
    "protected",
    "class",
    "interface",
    "extends",
    "implements",
    "static",
    "final",
    "void",
    "int",
    "long",
    "double",
    "float",
    "boolean",
    "char",
    "byte",
    "short",
    "new",
    "return",
    "if",
    "else",
    "for",
    "while",
    "switch",
    "case",
    "break",
    "continue",
    "try",
    "catch",
    "finally",
    "throw",
    "throws",
    "import",
    "package",
    "this",
    "super",
    "null",
    "true",
    "false",
    "abstract",
    "synchronized",
    "instanceof",
    "enum",
    "record",
    "var",
];
const KW_SQL: &[&str] = &[
    "select",
    "from",
    "where",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "index",
    "drop",
    "alter",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "on",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "as",
    "and",
    "or",
    "not",
    "null",
    "primary",
    "key",
    "foreign",
    "references",
    "unique",
    "default",
    "cascade",
    "if",
    "exists",
    "distinct",
    "union",
    "all",
    "desc",
    "asc",
    "count",
    "sum",
    "avg",
    "min",
    "max",
];
const KW_RB: &[&str] = &[
    "begin",
    "end",
    "def",
    "class",
    "module",
    "require",
    "require_relative",
    "include",
    "extend",
    "if",
    "elsif",
    "else",
    "unless",
    "case",
    "when",
    "while",
    "until",
    "for",
    "do",
    "return",
    "yield",
    "break",
    "next",
    "redo",
    "retry",
    "rescue",
    "ensure",
    "raise",
    "super",
    "self",
    "nil",
    "true",
    "false",
    "and",
    "or",
    "not",
    "alias",
    "undef",
    "attr_reader",
    "attr_writer",
    "attr_accessor",
    "private",
    "protected",
    "public",
    "lambda",
    "proc",
    "puts",
    "print",
];
const KW_PHP: &[&str] = &[
    "echo",
    "print",
    "function",
    "return",
    "if",
    "else",
    "elseif",
    "for",
    "foreach",
    "while",
    "do",
    "switch",
    "case",
    "break",
    "continue",
    "class",
    "interface",
    "trait",
    "extends",
    "implements",
    "public",
    "private",
    "protected",
    "static",
    "new",
    "try",
    "catch",
    "finally",
    "throw",
    "namespace",
    "use",
    "as",
    "const",
    "var",
    "array",
    "true",
    "false",
    "null",
    "instanceof",
    "clone",
];
const KW_LITERALS: &[&str] = &["true", "false", "null"];

fn lang(id: &str) -> Lang {
    let (kw, line_comment): (&'static [&'static str], &'static [&'static str]) = match id {
        "js" => (KW_JS, &["//"]),
        "go" => (KW_GO, &["//"]),
        "py" => (KW_PY, &["#"]),
        "rs" => (KW_RS, &["//"]),
        "c" => (KW_C, &["//"]),
        "java" => (KW_JAVA, &["//"]),
        "sql" => (KW_SQL, &["--"]),
        "rb" => (KW_RB, &["#"]),
        "php" => (KW_PHP, &["//", "#"]),
        "css" => (&[], &[]),
        "sh" => (&[], &["#"]),
        "json" => (KW_LITERALS, &[]),
        "yaml" => (KW_LITERALS, &["#"]),
        _ => (&[], &[]),
    };
    Lang { kw, line_comment }
}

const EXT: &[(&str, &str)] = &[
    ("js", "js"),
    ("jsx", "js"),
    ("ts", "js"),
    ("tsx", "js"),
    ("mjs", "js"),
    ("cjs", "js"),
    ("svelte", "js"),
    ("vue", "js"),
    ("go", "go"),
    ("py", "py"),
    ("rs", "rs"),
    ("c", "c"),
    ("h", "c"),
    ("cc", "c"),
    ("cpp", "c"),
    ("hpp", "c"),
    ("m", "c"),
    ("java", "java"),
    ("kt", "java"),
    ("swift", "java"),
    ("scala", "java"),
    ("sql", "sql"),
    ("rb", "rb"),
    ("php", "php"),
    ("css", "css"),
    ("scss", "css"),
    ("less", "css"),
    ("sh", "sh"),
    ("bash", "sh"),
    ("zsh", "sh"),
    ("json", "json"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("toml", "yaml"),
];

const INFO: &[(&str, &str)] = &[
    ("js", "js"),
    ("javascript", "js"),
    ("jsx", "js"),
    ("mjs", "js"),
    ("cjs", "js"),
    ("ts", "js"),
    ("typescript", "js"),
    ("tsx", "js"),
    ("svelte", "js"),
    ("vue", "js"),
    ("go", "go"),
    ("golang", "go"),
    ("py", "py"),
    ("python", "py"),
    ("rs", "rs"),
    ("rust", "rs"),
    ("c", "c"),
    ("h", "c"),
    ("cpp", "c"),
    ("c++", "c"),
    ("cc", "c"),
    ("cxx", "c"),
    ("hpp", "c"),
    ("java", "java"),
    ("kotlin", "java"),
    ("kt", "java"),
    ("swift", "java"),
    ("scala", "java"),
    ("sql", "sql"),
    ("rb", "rb"),
    ("ruby", "rb"),
    ("php", "php"),
    ("css", "css"),
    ("scss", "css"),
    ("less", "css"),
    ("sh", "sh"),
    ("bash", "sh"),
    ("zsh", "sh"),
    ("shell", "sh"),
    ("json", "json"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("toml", "yaml"),
    ("text", "text"),
    ("plaintext", "text"),
    ("txt", "text"),
    ("html", "text"),
    ("xml", "text"),
    ("md", "text"),
    ("markdown", "text"),
];

fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// File path -> highlighter id (`text` = plain).
pub fn lang_from_path(path: &str) -> &'static str {
    let base = path.rsplit('/').next().unwrap_or(path);
    let ext = base.rsplit('.').next().unwrap_or(base).to_lowercase();
    lookup(EXT, &ext).unwrap_or("text")
}

/// Markdown fence info string -> highlighter id.
pub fn lang_from_info(info: &str) -> &'static str {
    let tag = info.split_whitespace().next().unwrap_or("").to_lowercase();
    if tag.is_empty() {
        return "text";
    }
    lookup(INFO, &tag).unwrap_or("text")
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// Spans of one source line: `(byte range, kind)`, uncoloured text has `None`.
pub fn highlight_line_spans(
    line: &str,
    lang_id: &str,
) -> Vec<(std::ops::Range<usize>, Option<ShKind>)> {
    let lang = lang(lang_id);
    let b = line.as_bytes();
    let n = b.len();
    let mut out: Vec<(std::ops::Range<usize>, Option<ShKind>)> = Vec::new();
    let mut push = |r: std::ops::Range<usize>, k: Option<ShKind>| {
        if let Some(last) = out.last_mut()
            && last.1.is_none()
            && k.is_none()
            && last.0.end == r.start
        {
            last.0.end = r.end;
            return;
        }
        out.push((r, k));
    };
    let mut i = 0;
    while i < n {
        if lang.line_comment.iter().any(|p| line[i..].starts_with(p)) {
            push(i..n, Some(ShKind::Comment));
            break;
        }
        let c = b[i];
        if c == b'\'' || c == b'"' || c == b'`' {
            let mut j = i + 1;
            while j < n && b[j] != c {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let mut j = (j + 1).min(n);
            while !line.is_char_boundary(j) {
                j += 1;
            }
            push(i..j, Some(ShKind::String));
            i = j;
            continue;
        }
        if c.is_ascii_digit() && !(i > 0 && is_word_byte(b[i - 1])) {
            let mut j = i + 1;
            while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'.') {
                j += 1;
            }
            push(i..j, Some(ShKind::Number));
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let mut j = i + 1;
            while j < n && is_word_byte(b[j]) {
                j += 1;
            }
            let word = &line[i..j];
            let key = if lang_id == "sql" {
                word.to_lowercase()
            } else {
                word.to_string()
            };
            let kind = lang.kw.contains(&key.as_str()).then_some(ShKind::Keyword);
            push(i..j, kind);
            i = j;
            continue;
        }
        let len = line[i..].chars().next().map_or(1, char::len_utf8);
        push(i..i + len, None);
        i += len;
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One source line as safe HTML with `sh-*` spans.
pub fn highlight_line(line: &str, lang_id: &str) -> String {
    let mut out = String::new();
    for (r, kind) in highlight_line_spans(line, lang_id) {
        let text = esc(&line[r]);
        match kind {
            Some(k) => out.push_str(&format!("<span class=\"{}\">{text}</span>", k.class())),
            None => out.push_str(&text),
        }
    }
    out
}

/// A multi-line snippet as HTML lines.
pub fn highlight_code(src: &str, lang_id: &str) -> String {
    let body = src.strip_suffix('\n').unwrap_or(src);
    if body.is_empty() {
        return String::new();
    }
    body.split('\n')
        .map(|l| highlight_line(l, lang_id))
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_html_text(s: &str) -> String {
    let br =
        JsRegex::new("<br\\s*\\/?>", "gi").map_or_else(|| s.to_string(), |r| r.replace(s, "\n"));
    br.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

pub fn lang_from_code_class(cls: &str) -> &'static str {
    for c in cls.split_whitespace() {
        let tag = c
            .strip_prefix("language-")
            .or_else(|| c.strip_prefix("lang-"))
            .unwrap_or(c);
        let id = lang_from_info(tag);
        if id != "text" {
            return id;
        }
    }
    "text"
}

/// Tint fenced code blocks in LSP hover / signature HTML.
pub fn highlight_html_code_blocks(html: &str) -> String {
    if !html.contains("<pre") {
        return html.to_string();
    }
    let (Some(block), Some(class_attr), Some(tag), Some(br)) = (
        JsRegex::new(
            "<pre([^>]*)><code([^>]*)>([\\s\\S]*?)<\\/code><\\/pre>",
            "gi",
        ),
        JsRegex::new("class=\"([^\"]*)\"", ""),
        JsRegex::new("<[a-z]", "i"),
        JsRegex::new("<br\\s*\\/?>", "gi"),
    ) else {
        return html.to_string();
    };
    let mut out = String::with_capacity(html.len());
    let mut last = 0;
    let mut pos = 0;
    while let Some(g) = block.exec_at(html, pos) {
        let Some((ms, me)) = g[0] else { break };
        let part = |i: usize| g.get(i).copied().flatten().map_or("", |(s, e)| &html[s..e]);
        let (pre, code, inner) = (part(1), part(2), part(3));
        out.push_str(&html[last..ms]);
        if tag.test(&br.replace(inner, "")) {
            out.push_str(&html[ms..me]);
        } else {
            let cls_of = |attrs: &str| {
                class_attr
                    .exec_at(attrs, 0)
                    .and_then(|c| c.get(1).copied().flatten())
                    .map(|(s, e)| attrs[s..e].to_string())
            };
            let cls = cls_of(code).or_else(|| cls_of(pre)).unwrap_or_default();
            let raw = decode_html_text(inner);
            out.push_str(&format!(
                "<pre{pre}><code{code}>{}</code></pre>",
                highlight_code(&raw, lang_from_code_class(&cls))
            ));
        }
        last = me;
        pos = if me == ms { me + 1 } else { me };
    }
    out.push_str(&html[last..]);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Ctx,
    Del,
    Add,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: DiffKind,
    pub text: String,
}

fn row(kind: DiffKind, text: &str) -> DiffRow {
    DiffRow {
        kind,
        text: text.to_string(),
    }
}

/// Minimal line diff via LCS; huge inputs fall back to all-del + all-add.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffRow> {
    let a: Vec<&str> = old.strip_suffix('\n').unwrap_or(old).split('\n').collect();
    let b: Vec<&str> = new.strip_suffix('\n').unwrap_or(new).split('\n').collect();
    if a.len() * b.len() > 250_000 {
        return a
            .iter()
            .map(|t| row(DiffKind::Del, t))
            .chain(b.iter().map(|t| row(DiffKind::Add, t)))
            .collect();
    }
    let (m, n) = (a.len(), b.len());
    let w = n + 1;
    let mut dp = vec![0u32; (m + 1) * w];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i * w + j] = if a[i] == b[j] {
                dp[(i + 1) * w + j + 1] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + j + 1])
            };
        }
    }
    let mut rows = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m && j < n {
        if a[i] == b[j] {
            rows.push(row(DiffKind::Ctx, a[i]));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
            rows.push(row(DiffKind::Del, a[i]));
            i += 1;
        } else {
            rows.push(row(DiffKind::Add, b[j]));
            j += 1;
        }
    }
    rows.extend(a[i..].iter().map(|t| row(DiffKind::Del, t)));
    rows.extend(b[j..].iter().map(|t| row(DiffKind::Add, t)));
    rows
}

/// A unified diff (full or hunk-only) as rows; `[]` when not a diff.
pub fn parse_unified_diff(src: &str) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut in_hunk = false;
    for line in src.split('\n') {
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk || line.starts_with('\\') {
            continue;
        }
        if let Some(t) = line.strip_prefix('+') {
            rows.push(row(DiffKind::Add, t));
        } else if let Some(t) = line.strip_prefix('-') {
            rows.push(row(DiffKind::Del, t));
        } else if let Some(t) = line.strip_prefix(' ') {
            rows.push(row(DiffKind::Ctx, t));
        } else if line.is_empty() {
            rows.push(row(DiffKind::Ctx, ""));
        }
    }
    rows
}

pub fn looks_like_unified_diff(src: &str) -> bool {
    if !src.contains("@@") {
        return false;
    }
    if JsRegex::new("^--- .*\\n\\+\\+\\+ ", "m").is_some_and(|r| r.test(src)) {
        return true;
    }
    src.split('\n')
        .find(|l| !l.trim().is_empty())
        .is_some_and(|f| f.starts_with("@@"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tints_typescript_fences_from_hover_markdown() {
        let html = "<pre><code class=\"language-typescript\">(alias) function toErrorResponse(error: unknown): Record&lt;string, unknown&gt;</code></pre>";
        let out = highlight_html_code_blocks(html);
        assert!(out.contains("class=\"sh-kw\">function</span>"), "{out}");
        assert!(out.contains("class=\"sh-kw\">unknown</span>"));
        assert!(out.contains("Record"));
    }

    #[test]
    fn leaves_highlighted_fences_alone() {
        let html = "<pre><code class=\"language-ts\"><span class=\"sh-kw\">function</span> f()</code></pre>";
        assert_eq!(highlight_html_code_blocks(html), html);
    }

    #[test]
    fn detects_unified_diffs() {
        assert!(looks_like_unified_diff(
            "--- a/hello.txt\n+++ b/hello.txt\n@@ -1,2 +1,2 @@\n hello\n-world\n+universe\n"
        ));
        assert!(looks_like_unified_diff(
            "@@ -1,2 +1,2 @@\n hello\n-world\n+universe\n"
        ));
        assert!(!looks_like_unified_diff("export const hi = 1;\n"));
        assert!(!looks_like_unified_diff("{\"file_path\":\"/tmp/a.ts\"}"));
    }

    #[test]
    fn parses_hunk_only_bodies() {
        let rows: Vec<DiffRow> = parse_unified_diff("@@ -1,2 +1,2 @@\n hello\n-world\n+universe\n")
            .into_iter()
            .filter(|r| !r.text.is_empty() || r.kind != DiffKind::Ctx)
            .collect();
        assert_eq!(
            rows,
            vec![
                row(DiffKind::Ctx, "hello"),
                row(DiffKind::Del, "world"),
                row(DiffKind::Add, "universe")
            ]
        );
    }

    #[test]
    fn highlights_lines() {
        assert_eq!(
            highlight_line("const x = \"a<b\" // hi", "js"),
            "<span class=\"sh-kw\">const</span> x = <span class=\"sh-str\">\"a&lt;b\"</span> <span class=\"sh-com\">// hi</span>"
        );
        assert_eq!(
            highlight_line("x1 = 42", "py"),
            "x1 = <span class=\"sh-num\">42</span>"
        );
        assert_eq!(
            highlight_line("SELECT a", "sql"),
            "<span class=\"sh-kw\">SELECT</span> a"
        );
        assert_eq!(lang_from_path("src/a.tsx"), "js");
        assert_eq!(lang_from_info("ruby title=x"), "rb");
        assert_eq!(lang_from_info(""), "text");
    }

    #[test]
    fn diffs_lines_with_lcs() {
        let rows = diff_lines("a\nb\nc\n", "a\nx\nc\n");
        assert_eq!(
            rows,
            vec![
                row(DiffKind::Ctx, "a"),
                row(DiffKind::Del, "b"),
                row(DiffKind::Add, "x"),
                row(DiffKind::Ctx, "c")
            ]
        );
    }
}
