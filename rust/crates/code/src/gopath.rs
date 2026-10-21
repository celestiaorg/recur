//! Go `path/filepath` (Unix), `*os.PathError` text and `filepath.Match`, so
//! paths and error strings compare byte for byte with the Go app's.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;

/// Go `filepath.Clean`.
pub(crate) fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let p = path.as_bytes();
    let rooted = p[0] == b'/';
    let n = p.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = (0, 0);
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if p[r] == b'/' || (p[r] == b'.' && (r + 1 == n || p[r + 1] == b'/')) {
            r += 1;
        } else if p[r] == b'.' && p[r + 1] == b'.' && (r + 2 == n || p[r + 2] == b'/') {
            r += 2;
            if out.len() > dotdot {
                let mut w = out.len() - 1;
                while w > dotdot && out[w] != b'/' {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            while r < n && p[r] != b'/' {
                out.push(p[r]);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        out.push(b'.');
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Go `filepath.Join`.
pub(crate) fn join(elems: &[&str]) -> String {
    let parts: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    clean(&parts.join("/"))
}

/// Go `filepath.Dir`.
pub(crate) fn dir(path: &str) -> String {
    let i = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    clean(&path[..i])
}

/// Go `filepath.Base`.
pub(crate) fn base(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".into();
    }
    match trimmed.rfind('/') {
        Some(i) => trimmed[i + 1..].into(),
        None => trimmed.into(),
    }
}

/// Go `filepath.Ext`.
pub(crate) fn ext(path: &str) -> &str {
    for (i, b) in path.bytes().enumerate().rev() {
        match b {
            b'/' => return "",
            b'.' => return &path[i..],
            _ => {}
        }
    }
    ""
}

/// Go `filepath.IsAbs`.
pub(crate) fn is_abs(p: &str) -> bool {
    p.starts_with('/')
}

/// Go `filepath.Rel`.
pub(crate) fn rel(basepath: &str, targpath: &str) -> Result<String, String> {
    let base_c = clean(basepath);
    let targ = clean(targpath);
    if targ == base_c {
        return Ok(".".into());
    }
    let base_s = if base_c == "." { "" } else { base_c.as_str() };
    let cant = || format!("Rel: can't make {targpath} relative to {basepath}");
    if base_s.starts_with('/') != targ.starts_with('/') {
        return Err(cant());
    }
    let (b, t) = (base_s.as_bytes(), targ.as_bytes());
    let (bl, tl) = (b.len(), t.len());
    let (mut b0, mut bi, mut t0, mut ti) = (0, 0, 0, 0);
    loop {
        while bi < bl && b[bi] != b'/' {
            bi += 1;
        }
        while ti < tl && t[ti] != b'/' {
            ti += 1;
        }
        if t[t0..ti] != b[b0..bi] {
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
    if &b[b0..bi] == b".." {
        return Err(cant());
    }
    if b0 != bl {
        let seps = b[b0..bl].iter().filter(|&&c| c == b'/').count();
        let mut out = String::from("..");
        for _ in 0..seps {
            out.push_str("/..");
        }
        if t0 != tl {
            out.push('/');
            out.push_str(&targ[t0..]);
        }
        return Ok(out);
    }
    Ok(targ[t0..].into())
}

/// Go `os.Getwd`: `$PWD` when it names the current directory.
fn getwd() -> io::Result<String> {
    if let Ok(pwd) = std::env::var("PWD")
        && pwd.starts_with('/')
        && let (Ok(a), Ok(b)) = (fs::metadata(&pwd), fs::metadata("."))
        && a.dev() == b.dev()
        && a.ino() == b.ino()
    {
        return Ok(pwd);
    }
    std::env::current_dir().map(|d| d.to_string_lossy().into_owned())
}

/// Go `filepath.Abs`.
pub(crate) fn abs(p: &str) -> io::Result<String> {
    if is_abs(p) {
        return Ok(clean(p));
    }
    Ok(join(&[&getwd()?, p]))
}

/// Go `filepath.EvalSymlinks` for absolute paths; `None` on any error.
pub(crate) fn eval_symlinks(path: &str) -> Option<String> {
    let mut path = path.to_string();
    let mut vol_len = usize::from(path.starts_with('/'));
    let mut vol = path[..vol_len].to_string();
    let mut dest = vol.clone();
    let mut links = 0;
    let mut start = vol_len;
    let mut end;
    while start < path.len() {
        let b = path.as_bytes();
        while start < b.len() && b[start] == b'/' {
            start += 1;
        }
        end = start;
        while end < b.len() && b[end] != b'/' {
            end += 1;
        }
        let comp = &path[start..end];
        if end == start {
            break;
        } else if comp == "." {
            start = end;
            continue;
        } else if comp == ".." {
            match last_slash(&dest, vol_len) {
                Some(r) if &dest[r + 1..] != ".." => dest.truncate(r),
                _ => {
                    if dest.len() > vol_len {
                        dest.push('/');
                    }
                    dest.push_str("..");
                }
            }
            start = end;
            continue;
        }
        if !dest.is_empty() && !dest.ends_with('/') {
            dest.push('/');
        }
        dest.push_str(comp);
        let meta = fs::symlink_metadata(&dest).ok()?;
        if !meta.file_type().is_symlink() {
            if !meta.is_dir() && end < path.len() {
                return None;
            }
            start = end;
            continue;
        }
        links += 1;
        if links > 255 {
            return None;
        }
        let link = fs::read_link(&dest).ok()?.to_string_lossy().into_owned();
        path = format!("{link}{}", &path[end..]);
        if link.starts_with('/') {
            dest = "/".into();
            vol = "/".into();
            vol_len = 1;
            start = 1;
        } else {
            match last_slash(&dest, vol_len) {
                Some(r) => dest.truncate(r),
                None => dest = vol.clone(),
            }
            start = 0;
        }
    }
    Some(clean(&dest))
}

fn last_slash(dest: &str, vol_len: usize) -> Option<usize> {
    dest.get(vol_len..)?.rfind('/').map(|i| vol_len + i)
}

/// Go `syscall.Errno.Error()`: strerror with a lower-case first letter.
pub(crate) fn errno_text(e: &io::Error) -> String {
    let Some(code) = e.raw_os_error() else {
        return e.to_string();
    };
    let s = io::Error::from_raw_os_error(code).to_string();
    let s = s
        .strip_suffix(&format!(" (os error {code})"))
        .unwrap_or(&s)
        .to_string();
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => s,
    }
}

/// Go `os.MkdirAll(path, 0o755)`, reporting the component that failed.
pub(crate) fn mkdir_all(path: &str) -> Result<(), crate::Error> {
    use std::os::unix::fs::DirBuilderExt;
    if let Ok(m) = fs::metadata(path) {
        if m.is_dir() {
            return Ok(());
        }
        return Err(crate::Error::path(
            "mkdir",
            path,
            io::Error::from_raw_os_error(20),
        ));
    }
    let b = path.as_bytes();
    let mut i = b.len();
    while i > 0 && b[i - 1] == b'/' {
        i -= 1;
    }
    let mut j = i;
    while j > 0 && b[j - 1] != b'/' {
        j -= 1;
    }
    if j > 1 {
        mkdir_all(&path[..j - 1])?;
    }
    if let Err(e) = fs::DirBuilder::new().mode(0o755).create(path) {
        if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
            return Ok(());
        }
        return Err(crate::Error::path("mkdir", path, e));
    }
    Ok(())
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

/// Go `filepath.ErrBadPattern`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BadPattern;

/// Go `filepath.Match`, on bytes like Go.
pub(crate) fn glob_match(pattern: &str, name: &str) -> Result<bool, BadPattern> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_like_go() {
        assert_eq!(glob_match("*.go", "a.go"), Ok(true));
        assert_eq!(glob_match("*.go", "dir/a.go"), Ok(false));
        assert_eq!(glob_match("a/*/c", "a/b/c"), Ok(true));
        assert_eq!(glob_match("[a-c]x", "bx"), Ok(true));
        assert_eq!(glob_match("[", "a"), Err(BadPattern));
        assert_eq!(glob_match("a?c", "a/c"), Ok(false));
    }

    #[test]
    fn path_helpers() {
        assert_eq!(clean("a//b/../c/"), "a/c");
        assert_eq!(join(&["/root", "x/../y"]), "/root/y");
        assert_eq!(dir("a/b/c.go"), "a/b");
        assert_eq!(dir("c.go"), ".");
        assert_eq!(base("a/b/"), "b");
        assert_eq!(ext("a.b/c"), "");
        assert_eq!(ext("x.tar.gz"), ".gz");
        assert_eq!(rel("/a/b", "/a/b/c/d").as_deref(), Ok("c/d"));
        assert_eq!(rel("/a/b", "/a/x").as_deref(), Ok("../x"));
        assert!(rel("/a", "b").is_err());
    }
}
