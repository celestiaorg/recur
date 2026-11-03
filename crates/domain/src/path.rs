//! Lexical string paths (Unix): `..` resolves against the text, not the
//! filesystem, which `std::path::Path` does not do on its own.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;

/// Shortest lexically equivalent path.
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

/// Non-empty elements joined by `/`, then cleaned.
pub fn join(elems: &[&str]) -> String {
    let parts: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    clean(&parts.join("/"))
}

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

pub fn dir(path: &str) -> String {
    let cut = path.rfind('/').map_or(0, |i| i + 1);
    clean(&path[..cut])
}

/// Extension including the dot, or `""`.
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

pub fn is_abs(path: &str) -> bool {
    path.starts_with('/')
}

/// The working directory: `$PWD` when it is absolute and names the same
/// directory as `.` (so symlinked paths like macOS `/tmp` are kept), else
/// `getcwd`.
pub fn getwd() -> io::Result<String> {
    let dot = fs::metadata(".")?;
    if let Ok(pwd) = std::env::var("PWD")
        && is_abs(&pwd)
        && let Ok(m) = fs::metadata(&pwd)
        && m.dev() == dot.dev()
        && m.ino() == dot.ino()
    {
        return Ok(pwd);
    }
    Ok(std::env::current_dir()?.to_string_lossy().into_owned())
}

/// `path` joined onto the working directory and cleaned.
pub fn abs(path: &str) -> io::Result<String> {
    if is_abs(path) {
        return Ok(clean(path));
    }
    Ok(join(&[&getwd()?, path]))
}

/// `targpath` relative to `basepath`, both cleaned first. `None` when one is
/// absolute and the other is not, or the result would need to know the
/// working directory.
pub fn rel(basepath: &str, targpath: &str) -> Option<String> {
    let base = clean(basepath);
    let targ = clean(targpath);
    if targ == base {
        return Some(".".to_string());
    }
    let base = if base == "." { String::new() } else { base };
    if is_abs(&base) != is_abs(&targ) {
        return None;
    }
    let (b, t) = (base.as_bytes(), targ.as_bytes());
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
    if &base[b0..bi] == ".." {
        return None;
    }
    if b0 != bl {
        let seps = base[b0..bl].matches('/').count();
        let mut out = String::from("..");
        for _ in 0..seps {
            out.push_str("/..");
        }
        if t0 != tl {
            out.push('/');
            out.push_str(&targ[t0..]);
        }
        return Some(out);
    }
    Some(targ[t0..].to_string())
}

/// Resolves symlinks component by component, applying `..` to the path
/// resolved so far. `None` when a component is missing or links loop.
pub fn eval_symlinks(path: &str) -> Option<String> {
    let mut path = path.to_string();
    let mut vol_len = usize::from(path.starts_with('/'));
    let mut vol = path[..vol_len].to_string();
    let mut dest = vol.clone();
    let mut links = 0;
    let mut start = vol_len;
    while start < path.len() {
        let b = path.as_bytes();
        while start < b.len() && b[start] == b'/' {
            start += 1;
        }
        let mut end = start;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("syntax error in pattern")]
pub struct BadPattern;

/// Shell-style match of the whole `name`: `*` and `?` stop at `/`, `[...]`
/// classes, `\` escapes. Errors on a malformed pattern.
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

/// First char of `b` and its width; invalid input is U+FFFD of width 1.
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
