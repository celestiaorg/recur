//! Worktree sandbox for the coding tools, and the Go `path/filepath` and `os`
//! behavior it is defined in terms of (lexical cleaning, `EvalSymlinks`,
//! `MkdirAll`, `*PathError` text). Paths are UTF-8 strings, as in Go.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;

const ESCAPES: &str = "path escapes worktree";

/// Resolves `p` (relative to the worktree, or absolute) to a canonical path
/// inside the worktree. A missing leaf is resolved through its nearest
/// existing ancestor, so a symlink cannot smuggle a write outside it.
pub fn resolve_path(worktree: &str, p: &str) -> Result<String, String> {
    let wt = abs_eval(worktree.trim()).map_err(|e| format!("worktree: {e}"))?;
    let p = p.trim();
    if p.is_empty() {
        return Err("path is required".into());
    }
    let abs = if is_abs(p) {
        abs_eval_existing(p)?
    } else {
        abs_eval_existing(&safe_join(&wt, p)?)?
    };
    match rel(&wt, &abs) {
        Ok(r) if r != ".." && !r.starts_with("../") => Ok(abs),
        _ => Err(ESCAPES.into()),
    }
}

/// Go `code.SafeJoin`: joins a slash path under root, refusing any `..`.
pub fn safe_join(root: &str, rel_path: &str) -> Result<String, String> {
    let root_abs = abs(root)?;
    let rel_path = rel_path.strip_prefix('/').unwrap_or(rel_path);
    if rel_path == "." || rel_path.is_empty() {
        return Ok(root_abs);
    }
    if rel_path.contains("..") {
        return Err("path escapes root".into());
    }
    let full = abs(&join(&[&root_abs, rel_path]))?;
    if full != root_abs && !full.starts_with(&format!("{root_abs}/")) {
        return Err("path escapes root".into());
    }
    Ok(full)
}

pub(crate) fn abs_eval(p: &str) -> Result<String, String> {
    let a = abs(p)?;
    Ok(eval_symlinks(&a).unwrap_or(a))
}

/// Canonicalizes p, walking up to the nearest existing ancestor so a missing
/// leaf under a symlink cannot fail open to a lexical in-worktree path that
/// `mkdir_all` would follow outside the sandbox.
fn abs_eval_existing(p: &str) -> Result<String, String> {
    let a = abs(p)?;
    if let Some(eval) = eval_symlinks(&a) {
        return Ok(eval);
    }
    let mut rest: Vec<String> = Vec::new();
    let mut cur = a.clone();
    loop {
        if fs::symlink_metadata(&cur).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ESCAPES.into());
        }
        let parent = dir(&cur);
        if parent == cur {
            if rest.is_empty() {
                return Ok(a);
            }
            return Ok(join_owned(&cur, &rest));
        }
        rest.insert(0, base(&cur));
        if let Some(eval) = eval_symlinks(&parent) {
            return Ok(join_owned(&eval, &rest));
        }
        cur = parent;
    }
}

fn join_owned(first: &str, rest: &[String]) -> String {
    let mut parts: Vec<&str> = vec![first];
    parts.extend(rest.iter().map(String::as_str));
    join(&parts)
}

pub(crate) fn is_abs(p: &str) -> bool {
    p.starts_with('/')
}

/// Go `filepath.Clean`.
pub fn clean(path: &str) -> String {
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
            // Empty or "." element.
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
pub fn join(elems: &[&str]) -> String {
    let parts: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    clean(&parts.join("/"))
}

/// Go `filepath.Dir`.
pub fn dir(path: &str) -> String {
    let i = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    clean(&path[..i])
}

/// Go `filepath.Base`.
pub fn base(path: &str) -> String {
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

/// Go `filepath.Rel`.
pub fn rel(basepath: &str, targpath: &str) -> Result<String, String> {
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
fn getwd() -> Result<String, String> {
    if let Ok(pwd) = std::env::var("PWD")
        && pwd.starts_with('/')
        && let (Ok(a), Ok(b)) = (fs::metadata(&pwd), fs::metadata("."))
        && a.dev() == b.dev()
        && a.ino() == b.ino()
    {
        return Ok(pwd);
    }
    std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .map_err(|e| format!("getwd: {}", errno_text(&e)))
}

/// Go `filepath.Abs`.
pub fn abs(p: &str) -> Result<String, String> {
    if is_abs(p) {
        return Ok(clean(p));
    }
    Ok(join(&[&getwd()?, p]))
}

/// Go `filepath.EvalSymlinks` for absolute paths; `None` on any error.
pub fn eval_symlinks(path: &str) -> Option<String> {
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

/// Index of the last '/' in dest at or after vol_len.
fn last_slash(dest: &str, vol_len: usize) -> Option<usize> {
    dest.get(vol_len..)?.rfind('/').map(|i| vol_len + i)
}

/// Go `*os.PathError` text: `op path: errno`.
pub(crate) fn path_err(op: &str, path: &str, e: &io::Error) -> String {
    format!("{op} {path}: {}", errno_text(e))
}

/// Go `syscall.Errno.Error()`: strerror with a lower-case first letter.
pub fn errno_text(e: &io::Error) -> String {
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

/// Go `os.MkdirAll(path, 0o755)`.
pub(crate) fn mkdir_all(path: &str) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    if let Ok(m) = fs::metadata(path) {
        if m.is_dir() {
            return Ok(());
        }
        return Err(path_err("mkdir", path, &io::Error::from_raw_os_error(20)));
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
        return Err(path_err("mkdir", path, &e));
    }
    Ok(())
}

/// Go `os.ReadFile`.
pub(crate) fn read_file(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = fs::File::open(path).map_err(|e| path_err("open", path, &e))?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)
        .map_err(|e| path_err("read", path, &e))?;
    Ok(data)
}

/// Go `os.WriteFile(path, data, 0o644)`.
pub(crate) fn write_file(path: &str, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(path)
        .map_err(|e| path_err("open", path, &e))?;
    f.write_all(data).map_err(|e| path_err("write", path, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filepath_matches_go() {
        assert_eq!(clean("a//b/./c/../d/"), "a/b/d");
        assert_eq!(clean("/../a"), "/a");
        assert_eq!(clean("../../a/.."), "../..");
        assert_eq!(clean(""), ".");
        assert_eq!(dir("/a/b"), "/a");
        assert_eq!(dir("a"), ".");
        assert_eq!(dir("/"), "/");
        assert_eq!(base("/a/b/"), "b");
        assert_eq!(rel("/a/b", "/a/b/c/d").unwrap(), "c/d");
        assert_eq!(rel("/a/b", "/a/x").unwrap(), "../x");
        assert_eq!(rel("/a/b", "/a").unwrap(), "..");
        assert_eq!(rel("/a", "/a").unwrap(), ".");
        assert!(rel("a", "/a").is_err());
    }

    // Port of path_test.go TestResolvePathSandbox.
    #[test]
    fn resolve_path_sandbox() {
        let wt = tempfile::tempdir().unwrap();
        let wt_s = wt.path().to_str().unwrap();
        fs::write(wt.path().join("in.txt"), "ok").unwrap();
        let got = resolve_path(wt_s, "in.txt").unwrap();
        assert!(got.ends_with("in.txt"), "{got}");
        assert!(resolve_path(wt_s, "../nope").is_err());
        let outside = tempfile::tempdir().unwrap();
        let out_file = outside.path().join("x.txt");
        assert!(resolve_path(wt_s, out_file.to_str().unwrap()).is_err());
        std::os::unix::fs::symlink(outside.path(), wt.path().join("link")).unwrap();
        assert!(resolve_path(wt_s, "link/newdir/file.txt").is_err());
    }

    #[test]
    fn eval_symlinks_resolves_relative_links() {
        let wt = tempfile::tempdir().unwrap();
        let root = eval_symlinks(wt.path().to_str().unwrap()).unwrap();
        fs::create_dir_all(format!("{root}/sub/deep")).unwrap();
        std::os::unix::fs::symlink("sub/deep", format!("{root}/l")).unwrap();
        assert_eq!(
            eval_symlinks(&format!("{root}/l/../x")),
            None,
            "missing leaf after .. fails like Go"
        );
        assert_eq!(
            eval_symlinks(&format!("{root}/l/..")),
            Some(format!("{root}/sub"))
        );
        assert_eq!(
            eval_symlinks(&format!("{root}/l")),
            Some(format!("{root}/sub/deep"))
        );
    }
}
