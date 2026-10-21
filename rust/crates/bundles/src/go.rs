//! Go standard library semantics that leak into user-visible text.
//!
//! Lint findings, skip reasons and test details embed Go's `%q` quoting,
//! `path/filepath` results and `*fs.PathError` messages, so the CLI output
//! stays byte-identical only if these behave exactly like Go (Unix only).

use std::io;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

static PRINTABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}\p{M}\p{N}\p{P}\p{S} ]$").expect("printable class"));

/// Go `strconv.Quote` (what `%q` prints for a string).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        push_escaped(&mut out, c);
    }
    out.push('"');
    out
}

/// Go `strconv.Quote` over raw bytes: invalid UTF-8 bytes print as `\xNN`.
pub fn quote_bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() + 2);
    out.push('"');
    for chunk in b.utf8_chunks() {
        for c in chunk.valid().chars() {
            push_escaped(&mut out, c);
        }
        for byte in chunk.invalid() {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
    out.push('"');
    out
}

fn push_escaped(out: &mut String, c: char) {
    if c == '"' || c == '\\' {
        out.push('\\');
        out.push(c);
        return;
    }
    let mut buf = [0u8; 4];
    if PRINTABLE.is_match(c.encode_utf8(&mut buf)) {
        out.push(c);
        return;
    }
    match c {
        '\u{07}' => out.push_str("\\a"),
        '\u{08}' => out.push_str("\\b"),
        '\u{0C}' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{0B}' => out.push_str("\\v"),
        c if (c as u32) < 0x20 || c as u32 == 0x7f => {
            out.push_str(&format!("\\x{:02x}", c as u32));
        }
        c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
        c => out.push_str(&format!("\\U{:08x}", c as u32)),
    }
}

/// Go `strings.TrimSpace` (Unicode `White_Space`, same set as `str::trim`).
pub fn trim(s: &str) -> &str {
    s.trim()
}

/// Go `strings.ToLower`: per-rune simple case mapping.
pub fn to_lower(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Go `strings.ToUpper`: per-rune simple case mapping (a rune whose full
/// uppercase is several runes, like `ß`, stays as is).
pub fn to_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        })
        .collect()
}

/// Go `strconv.Atoi` (decimal, optional sign) with its error text.
pub fn atoi(s: &str) -> Result<i64, String> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "strconv.Atoi: parsing {}: invalid syntax",
            quote(s)
        ));
    }
    s.parse::<i64>()
        .map_err(|_| format!("strconv.Atoi: parsing {}: value out of range", quote(s)))
}

/// Message text of an OS error as Go's `syscall.Errno.Error()` prints it
/// (`strerror` with a lowercase first letter, no `(os error N)` suffix).
pub fn errno_text(err: &io::Error) -> String {
    if err.raw_os_error().is_none() {
        return err.to_string();
    }
    let text = err.to_string();
    let text = match text.rfind(" (os error ") {
        Some(i) => &text[..i],
        None => &text,
    };
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Go `*fs.PathError` text: `op path: errno`.
pub fn path_err(op: &str, path: &str, err: &io::Error) -> String {
    format!("{op} {path}: {}", errno_text(err))
}

/// Go `os.ReadFile`: open errors report `open`, read errors report `read`.
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| path_err("open", path, &e))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .map_err(|e| path_err("read", path, &e))?;
    Ok(buf)
}

/// [`read_file`] decoded as text. Go strings carry arbitrary bytes; invalid
/// UTF-8 is replaced here because Rust strings cannot hold it.
pub fn read_file_string(path: &str) -> Result<String, String> {
    read_file(path).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Go `os.Stat` error text (`stat path: errno`).
pub fn stat(path: &str) -> Result<std::fs::Metadata, String> {
    std::fs::metadata(path).map_err(|e| path_err("stat", path, &e))
}

/// Go `filepath.Clean` (Unix).
pub fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let rooted = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// Go `filepath.Join`: non-empty elements joined by `/`, then cleaned.
pub fn join(elems: &[&str]) -> String {
    let parts: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    clean(&parts.join("/"))
}

/// Go `filepath.Base`.
pub fn base(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        Some(i) => trimmed[i + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

/// Go `filepath.Dir`.
pub fn dir(path: &str) -> String {
    let cut = path.rfind('/').map_or(0, |i| i + 1);
    clean(&path[..cut])
}

/// Go `filepath.Ext`.
pub fn ext(path: &str) -> &str {
    for (i, b) in path.bytes().enumerate().rev() {
        match b {
            b'/' => return "",
            b'.' => return &path[i..],
            _ => {}
        }
    }
    ""
}

/// Go `filepath.IsAbs` (Unix).
pub fn is_abs(path: &str) -> bool {
    path.starts_with('/')
}

/// Go `os.Getwd`: `$PWD` when it is absolute and names the same directory
/// as `.` (so symlinked paths like macOS `/tmp` are kept), else `getcwd`.
pub fn getwd() -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let dot = std::fs::metadata(".")?;
    if let Ok(pwd) = std::env::var("PWD")
        && is_abs(&pwd)
        && let Ok(m) = std::fs::metadata(&pwd)
        && m.dev() == dot.dev()
        && m.ino() == dot.ino()
    {
        return Ok(pwd);
    }
    Ok(path_str(&std::env::current_dir()?))
}

/// Go `filepath.Abs`: joins the working directory and cleans.
pub fn abs(path: &str) -> io::Result<String> {
    if is_abs(path) {
        return Ok(clean(path));
    }
    Ok(join(&[&getwd()?, path]))
}

/// Lossy `Path` → `String` for Go-style string path handling.
pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Go `filepath.Rel` (Unix). Errors when `targ` cannot be made relative.
pub fn rel(basepath: &str, targpath: &str) -> Result<String, String> {
    let base = clean(basepath);
    let targ = clean(targpath);
    if targ == base {
        return Ok(".".to_string());
    }
    let base = if base == "." { String::new() } else { base };
    let targ_s = if targ == "." {
        String::new()
    } else {
        targ.clone()
    };
    let base_slashed = base.starts_with('/');
    let targ_slashed = targ_s.starts_with('/');
    if base_slashed != targ_slashed {
        return Err(format!("Rel: can't make {targpath} relative to {basepath}"));
    }
    let b = base.as_bytes();
    let t = targ_s.as_bytes();
    let (bl, tl) = (b.len(), t.len());
    let (mut b0, mut bi, mut t0, mut ti) = (0, 0, 0, 0);
    loop {
        while bi < bl && b[bi] != b'/' {
            bi += 1;
        }
        while ti < tl && t[ti] != b'/' {
            ti += 1;
        }
        if targ_s[t0..ti] != base[b0..bi] {
            break;
        }
        if bi < bl {
            bi += 1;
        }
        if ti < tl {
            ti += 1;
        }
        b0 = bi;
        t0 = ti;
    }
    if &base[b0..bi] == ".." {
        return Err(format!("Rel: can't make {targpath} relative to {basepath}"));
    }
    if b0 != bl {
        let seps = base[b0..bl].matches('/').count();
        let mut out = String::from("..");
        for _ in 0..seps {
            out.push_str("/..");
        }
        if t0 != tl {
            out.push('/');
            out.push_str(&targ_s[t0..]);
        }
        return Ok(out);
    }
    Ok(targ_s[t0..].to_string())
}

/// Go `filepath.ErrBadPattern`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("syntax error in pattern")]
pub struct BadPattern;

/// Go `filepath.Match`, on bytes like Go.
pub fn glob_match(pattern: &str, name: &str) -> Result<bool, BadPattern> {
    let mut pattern = pattern.as_bytes();
    let mut name = name.as_bytes();
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            return Ok(!name.contains(&b'/'));
        }
        let (t, ok) = match_chunk(chunk, name)?;
        if ok && (t.is_empty() || !pattern.is_empty()) {
            name = t;
            continue;
        }
        if star {
            let mut i = 0;
            while i < name.len() && name[i] != b'/' {
                let (t, ok) = match_chunk(chunk, &name[i + 1..])?;
                if ok {
                    if pattern.is_empty() && !t.is_empty() {
                        i += 1;
                        continue;
                    }
                    name = t;
                    continue 'pattern;
                }
                i += 1;
            }
        }
        while !pattern.is_empty() {
            let (_, chunk, rest) = scan_chunk(pattern);
            pattern = rest;
            match_chunk(chunk, b"")?;
        }
        return Ok(false);
    }
    Ok(name.is_empty())
}

fn scan_chunk(mut pattern: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut star = false;
    while pattern.first() == Some(&b'*') {
        pattern = &pattern[1..];
        star = true;
    }
    let mut inrange = false;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'\\' if i + 1 < pattern.len() => i += 1,
            b'[' => inrange = true,
            b']' => inrange = false,
            b'*' if !inrange => break,
            _ => {}
        }
        i += 1;
    }
    (star, &pattern[..i], &pattern[i..])
}

/// Go `utf8.DecodeRune`: invalid input decodes as U+FFFD of width 1.
fn decode_rune(b: &[u8]) -> (char, usize) {
    if b.is_empty() {
        return ('\u{FFFD}', 0);
    }
    let n = match b[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => return ('\u{FFFD}', 1),
    };
    match b.get(..n).and_then(|s| std::str::from_utf8(s).ok()) {
        Some(s) => (s.chars().next().unwrap_or('\u{FFFD}'), n),
        None => ('\u{FFFD}', 1),
    }
}

fn match_chunk<'a>(mut chunk: &[u8], mut s: &'a [u8]) -> Result<(&'a [u8], bool), BadPattern> {
    let mut failed = false;
    while !chunk.is_empty() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match chunk[0] {
            b'[' => {
                let mut r = '\0';
                if !failed {
                    let (c, n) = decode_rune(s);
                    r = c;
                    s = &s[n..];
                }
                chunk = &chunk[1..];
                let mut negated = false;
                if chunk.first() == Some(&b'^') {
                    negated = true;
                    chunk = &chunk[1..];
                }
                let mut matched = false;
                let mut nrange = 0;
                loop {
                    if chunk.first() == Some(&b']') && nrange > 0 {
                        chunk = &chunk[1..];
                        break;
                    }
                    let (lo, rest) = get_esc(chunk)?;
                    chunk = rest;
                    let mut hi = lo;
                    if chunk[0] == b'-' {
                        let (h, rest) = get_esc(&chunk[1..])?;
                        hi = h;
                        chunk = rest;
                    }
                    if lo <= r && r <= hi {
                        matched = true;
                    }
                    nrange += 1;
                }
                if matched == negated {
                    failed = true;
                }
            }
            b'?' => {
                if !failed {
                    if s[0] == b'/' {
                        failed = true;
                    }
                    let (_, n) = decode_rune(s);
                    s = &s[n..];
                }
                chunk = &chunk[1..];
            }
            c => {
                if c == b'\\' {
                    chunk = &chunk[1..];
                    if chunk.is_empty() {
                        return Err(BadPattern);
                    }
                }
                if !failed {
                    if chunk[0] != s[0] {
                        failed = true;
                    }
                    s = &s[1..];
                }
                chunk = &chunk[1..];
            }
        }
    }
    if failed {
        return Ok((b"", false));
    }
    Ok((s, true))
}

fn get_esc(chunk: &[u8]) -> Result<(char, &[u8]), BadPattern> {
    if matches!(chunk.first(), None | Some(b'-') | Some(b']')) {
        return Err(BadPattern);
    }
    let mut chunk = chunk;
    if chunk[0] == b'\\' {
        chunk = &chunk[1..];
        if chunk.is_empty() {
            return Err(BadPattern);
        }
    }
    let (r, n) = decode_rune(chunk);
    if r == '\u{FFFD}' && n == 1 {
        return Err(BadPattern);
    }
    chunk = &chunk[n..];
    if chunk.is_empty() {
        return Err(BadPattern);
    }
    Ok((r, chunk))
}

/// Compiles a Go RE2 pattern with Go semantics: `\d \s \w \b` and their
/// negations are ASCII-only in Go but Unicode in Rust, `\Q…\E` quotes, and
/// `[`, `&`, `~` are literal inside a Go character class.
pub fn compile_regexp(pattern: &str) -> Result<Regex, String> {
    Regex::new(&translate_regexp(pattern)).map_err(|e| {
        let text = e.to_string();
        let msg = text
            .lines()
            .rev()
            .find_map(|l| l.trim().strip_prefix("error: "))
            .unwrap_or(text.trim());
        format!("error parsing regexp: {msg}: `{pattern}`")
    })
}

fn translate_regexp(pattern: &str) -> String {
    const SPACE: &str = r"\t\n\x0C\r ";
    const WORD: &str = "0-9A-Za-z_";
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut in_class = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let d = chars[i + 1];
            i += 2;
            match (d, in_class) {
                ('d', true) => out.push_str("0-9"),
                ('d', false) => out.push_str("[0-9]"),
                ('D', _) => out.push_str("[^0-9]"),
                ('s', true) => out.push_str(SPACE),
                ('s', false) => out.push_str(&format!("[{SPACE}]")),
                ('S', _) => out.push_str(&format!("[^{SPACE}]")),
                ('w', true) => out.push_str(WORD),
                ('w', false) => out.push_str(&format!("[{WORD}]")),
                ('W', _) => out.push_str(&format!("[^{WORD}]")),
                ('b', false) => out.push_str(r"(?-u:\b)"),
                ('B', false) => out.push_str(r"(?-u:\B)"),
                ('Q', _) => {
                    let mut lit = String::new();
                    while i < chars.len() {
                        if chars[i] == '\\' && chars.get(i + 1) == Some(&'E') {
                            i += 2;
                            break;
                        }
                        lit.push(chars[i]);
                        i += 1;
                    }
                    if in_class {
                        for ch in lit.chars() {
                            out.push_str(&regex::escape(&ch.to_string()));
                        }
                    } else {
                        out.push_str(&regex::escape(&lit));
                    }
                }
                _ => {
                    out.push('\\');
                    out.push(d);
                }
            }
            continue;
        }
        if in_class {
            match c {
                ']' => {
                    in_class = false;
                    out.push(']');
                }
                '[' if chars.get(i + 1) == Some(&':') => {
                    // POSIX class `[:alpha:]`: copy through the closing `:]`.
                    let end = (i + 2..chars.len().saturating_sub(1))
                        .find(|&j| chars[j] == ':' && chars[j + 1] == ']');
                    match end {
                        Some(j) => {
                            out.extend(&chars[i..j + 2]);
                            i = j + 2;
                            continue;
                        }
                        None => out.push_str(r"\["),
                    }
                }
                '[' | '&' | '~' => {
                    out.push('\\');
                    out.push(c);
                }
                _ => out.push(c),
            }
            i += 1;
            continue;
        }
        if c == '[' {
            in_class = true;
            out.push('[');
            i += 1;
            if chars.get(i) == Some(&'^') {
                out.push('^');
                i += 1;
            }
            if chars.get(i) == Some(&']') {
                out.push_str(r"\]");
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_matches_go() {
        assert_eq!(quote("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(quote("x\ny\t\u{7}"), r#""x\ny\t\a""#);
        assert_eq!(quote("\u{1}\u{7f}"), r#""\x01\x7f""#);
        assert_eq!(quote("héllo ✓"), "\"héllo ✓\"");
        assert_eq!(quote("\u{a0}"), "\"\\u00a0\"");
        assert_eq!(quote("\u{feff}"), "\"\\ufeff\"");
        assert_eq!(quote_bytes(b"a\xffb"), r#""a\xffb""#);
    }

    #[test]
    fn clean_matches_go() {
        for (input, want) in [
            ("", "."),
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            ("a/b/c", "a/b/c"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
            ("abc/", "abc"),
            ("abc/def/", "abc/def"),
            ("//abc", "/abc"),
            ("///abc", "/abc"),
            ("abc//def//ghi", "abc/def/ghi"),
            ("abc/./def", "abc/def"),
            ("/./abc/def", "/abc/def"),
            ("abc/.", "abc"),
            ("abc/def/ghi/../jkl", "abc/def/jkl"),
            ("abc/def/../ghi/../jkl", "abc/jkl"),
            ("abc/def/..", "abc"),
            ("abc/def/../..", "."),
            ("/abc/def/../..", "/"),
            ("abc/def/../../..", ".."),
            ("/abc/def/../../..", "/"),
            ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
            ("/../abc", "/abc"),
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
        ] {
            assert_eq!(clean(input), want, "clean({input:?})");
        }
    }

    #[test]
    fn rel_matches_go() {
        for (b, t, want) in [
            ("a/b", "a/b", Ok(".")),
            ("a/b/.", "a/b", Ok(".")),
            ("a/b", "a/b/.", Ok(".")),
            ("./a/b", "a/b", Ok(".")),
            ("a/b", "./a/b", Ok(".")),
            ("ab/cd", "ab/cde", Ok("../cde")),
            ("ab/cd", "ab/c", Ok("../c")),
            ("a/b", "a/b/c/d", Ok("c/d")),
            ("a/b", "a/b/../c", Ok("../c")),
            ("a/b/../c", "a/b", Ok("../b")),
            ("a/b/c", "a/c/d", Ok("../../c/d")),
            ("a/b", "c/d", Ok("../../c/d")),
            ("a/b/c/d", "a/b", Ok("../..")),
            ("a/b/c/d", "a/b/", Ok("../..")),
            ("a/b/c/d/", "a/b", Ok("../..")),
            ("a/b/c/d/", "a/b/", Ok("../..")),
            ("../../a/b", "../../a/b/c/d", Ok("c/d")),
            ("/a/b", "/a/b", Ok(".")),
            ("/a/b/.", "/a/b", Ok(".")),
            ("/a/b", "/a/b/.", Ok(".")),
            ("/ab/cd", "/ab/cde", Ok("../cde")),
            ("/ab/cd", "/ab/c", Ok("../c")),
            ("/a/b", "/a/b/c/d", Ok("c/d")),
            ("/a/b", "/a/b/../c", Ok("../c")),
            ("/a/b/../c", "/a/b", Ok("../b")),
            ("/a/b/c", "/a/c/d", Ok("../../c/d")),
            ("/a/b", "/c/d", Ok("../../c/d")),
            ("/a/b/c/d", "/a/b", Ok("../..")),
            ("/a/b/c/d", "/a/b/", Ok("../..")),
            ("/a/b/c/d/", "/a/b", Ok("../..")),
            ("/a/b/c/d/", "/a/b/", Ok("../..")),
            ("/../../a/b", "/../../a/b/c/d", Ok("c/d")),
            (".", "a/b", Ok("a/b")),
            (".", "..", Ok("..")),
            ("..", ".", Err(())),
            ("..", "a", Err(())),
            ("../..", "..", Err(())),
            ("a", "/a", Err(())),
            ("/a", "a", Err(())),
        ] {
            let got = rel(b, t).map_err(|_| ());
            assert_eq!(got, want.map(str::to_string), "rel({b:?}, {t:?})");
        }
    }

    #[test]
    fn match_matches_go() {
        for (pattern, s, want) in [
            ("abc", "abc", Ok(true)),
            ("*", "abc", Ok(true)),
            ("*c", "abc", Ok(true)),
            ("a*", "a", Ok(true)),
            ("a*", "abc", Ok(true)),
            ("a*", "ab/c", Ok(false)),
            ("a*/b", "abc/b", Ok(true)),
            ("a*/b", "a/c/b", Ok(false)),
            ("a*b*c*d*e*/f", "axbxcxdxe/f", Ok(true)),
            ("a*b*c*d*e*/f", "axbxcxdxexxx/f", Ok(true)),
            ("a*b*c*d*e*/f", "axbxcxdxe/xxx/f", Ok(false)),
            ("a*b*c*d*e*/f", "axbxcxdxexxx/fff", Ok(false)),
            ("a*b?c*x", "abxbbxdbxebxczzx", Ok(true)),
            ("a*b?c*x", "abxbbxdbxebxczzy", Ok(false)),
            ("ab[c]", "abc", Ok(true)),
            ("ab[b-d]", "abc", Ok(true)),
            ("ab[e-g]", "abc", Ok(false)),
            ("ab[^c]", "abc", Ok(false)),
            ("ab[^b-d]", "abc", Ok(false)),
            ("ab[^e-g]", "abc", Ok(true)),
            ("a\\*b", "a*b", Ok(true)),
            ("a\\*b", "ab", Ok(false)),
            ("a?b", "a☺b", Ok(true)),
            ("a[^a]b", "a☺b", Ok(true)),
            ("a???b", "a☺b", Ok(false)),
            ("a[^a][^a][^a]b", "a☺b", Ok(false)),
            ("[a-ζ]*", "α", Ok(true)),
            ("*[a-ζ]", "A", Ok(false)),
            ("a?b", "a/b", Ok(false)),
            ("a*b", "a/b", Ok(false)),
            ("[\\]a]", "]", Ok(true)),
            ("[\\-]", "-", Ok(true)),
            ("[x\\-]", "x", Ok(true)),
            ("[x\\-]", "-", Ok(true)),
            ("[x\\-]", "z", Ok(false)),
            ("[\\-x]", "x", Ok(true)),
            ("[\\-x]", "-", Ok(true)),
            ("[\\-x]", "a", Ok(false)),
            ("[]a]", "]", Err(())),
            ("[-]", "-", Err(())),
            ("[x-]", "x", Err(())),
            ("[x-]", "-", Err(())),
            ("[x-]", "z", Err(())),
            ("[-x]", "x", Err(())),
            ("[-x]", "-", Err(())),
            ("[-x]", "a", Err(())),
            ("\\", "a", Err(())),
            ("[a-b-c]", "a", Err(())),
            ("[", "a", Err(())),
            ("[^", "a", Err(())),
            ("[^bc", "a", Err(())),
            ("a[", "a", Err(())),
            ("a[", "ab", Err(())),
            ("a[", "x", Err(())),
            ("a/b[", "x", Err(())),
            ("*x", "xxx", Ok(true)),
        ] {
            assert_eq!(
                glob_match(pattern, s).map_err(|_| ()),
                want,
                "match({pattern:?}, {s:?})"
            );
        }
    }

    #[test]
    fn base_dir_ext() {
        assert_eq!(base("/a/b/"), "b");
        assert_eq!(base("/"), "/");
        assert_eq!(base(""), ".");
        assert_eq!(dir("/a/b"), "/a");
        assert_eq!(dir("a"), ".");
        assert_eq!(dir("/"), "/");
        assert_eq!(ext("x/y.tmLanguage.json"), ".json");
        assert_eq!(ext("x.d/y"), "");
        assert_eq!(join(&["a", "", "../b"]), "b");
        assert_eq!(join(&["", ""]), "");
    }

    #[test]
    fn regexp_uses_go_classes() {
        let re = compile_regexp(r"^\d+\s\w$").unwrap();
        assert!(re.is_match("12 a"));
        assert!(!re.is_match("\u{0663} a"));
        assert!(!re.is_match("1\u{a0}a"));
        assert!(compile_regexp(r"[\d.]+").unwrap().is_match("1.2"));
        assert!(compile_regexp(r"a\Q.*\E").unwrap().is_match("a.*"));
        assert!(compile_regexp(r"[]a]").unwrap().is_match("]"));
        assert!(compile_regexp(r"[a&&b]").unwrap().is_match("&"));
        assert!(compile_regexp(r"(?i)^eng-\d+").unwrap().is_match("ENG-12"));
        let err = compile_regexp("(").unwrap_err();
        assert!(err.starts_with("error parsing regexp: "), "{err}");
    }

    #[test]
    fn errno_text_is_go_style() {
        let e = io::Error::from_raw_os_error(2);
        assert_eq!(errno_text(&e), "no such file or directory");
        assert_eq!(
            read_file("/definitely/missing").unwrap_err(),
            "open /definitely/missing: no such file or directory"
        );
    }
}
