use std::fs;
use std::path::Path;

use super::data_dir;
use super::grep::{glob_args, run_rg, search_root};
use crate::json::{Shape, Ty, field, unmarshal};
use crate::path::rel;
use crate::tool::{Call, Tool, ToolError};

const SCHEMA: &str = r#"{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}"#;

static IN: Shape = Shape::anon(&[field("pattern", Ty::Str), field("path", Ty::Str)]);

pub struct GlobTool;

#[async_trait::async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "Find files by glob pattern (relative to path, default the worktree root). Respects .gitignore and includes hidden files. Prefer this over shell find."
    }

    fn schema(&self) -> String {
        SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let a =
            unmarshal(&c.args, &IN).map_err(|e| ToolError(format!("invalid Glob args: {e}")))?;
        let pattern = a.str("pattern");
        if pattern.is_empty() {
            return Err("pattern is required".into());
        }
        let (dir, target) = search_root(&c.worktree, a.str("path"))?;
        let Ok(rg_path) = recur_code::rg::locate(&data_dir()) else {
            return walk_glob(&dir, pattern);
        };
        let mut args: Vec<String> = [
            "--files",
            "--no-config",
            "--no-messages",
            "--color",
            "never",
            "--hidden",
            "--glob",
            "!.git",
        ]
        .map(String::from)
        .to_vec();
        args.extend(glob_args(pattern));
        args.push(target);
        let (matches, _) =
            run_rg(&c.cancel, Path::new(&rg_path), &c.worktree, &dir, &args, 0).await?;
        if matches.is_empty() {
            return Ok("No files matched.".into());
        }
        Ok(matches.join("\n"))
    }
}

/// The no-ripgrep fallback: a lexical walk skipping .git and node_modules,
/// matching the pattern against the base name or the relative path.
fn walk_glob(root: &str, pattern: &str) -> Result<String, ToolError> {
    let mut matches = Vec::new();
    walk(root, root, pattern, &mut matches);
    if matches.is_empty() {
        return Ok("No files matched.".into());
    }
    Ok(matches.join("\n"))
}

/// Go `filepath.WalkDir` order (lexical per directory, symlinks not followed);
/// unreadable entries are skipped.
fn walk(root: &str, p: &str, pattern: &str, out: &mut Vec<String>) {
    let Ok(meta) = fs::symlink_metadata(p) else {
        return;
    };
    if meta.is_dir() {
        let name = crate::path::base(p);
        if name == ".git" || name == "node_modules" {
            return;
        }
        let Ok(rd) = fs::read_dir(p) else { return };
        let mut names: Vec<String> = rd
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        names.sort();
        for n in names {
            walk(root, &format!("{p}/{n}"), pattern, out);
        }
        return;
    }
    let Ok(r) = rel(root, p) else { return };
    let base = crate::path::base(&r);
    if glob_match(pattern, &base).unwrap_or(false) || glob_match(pattern, &r).unwrap_or(false) {
        out.push(r);
    }
}

/// Go `filepath.Match` (Unix): `*`, `?`, `[...]` classes and `\` escapes;
/// Err is `ErrBadPattern`.
pub(crate) fn glob_match(pattern: &str, name: &str) -> Result<bool, ()> {
    let mut pattern = pattern.as_bytes();
    let mut name = name.as_bytes();
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            // Trailing * matches the rest unless it has a /.
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
        // Report a malformed remainder before returning false.
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
    let mut in_range = false;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'\\' if i + 1 < pattern.len() => i += 1,
            b'[' => in_range = true,
            b']' => in_range = false,
            b'*' if !in_range => break,
            _ => {}
        }
        i += 1;
    }
    (star, &pattern[..i], &pattern[i..])
}

/// Go `utf8.DecodeRune`: (rune, width), U+FFFD width 1 when invalid.
fn decode_rune(b: &[u8]) -> (u32, usize) {
    for n in 1..=b.len().min(4) {
        if let Ok(s) = std::str::from_utf8(&b[..n])
            && let Some(c) = s.chars().next()
        {
            return (c as u32, n);
        }
    }
    (0xFFFD, 1)
}

fn match_chunk<'a>(mut chunk: &[u8], mut s: &'a [u8]) -> Result<(&'a [u8], bool), ()> {
    let mut failed = false;
    while !chunk.is_empty() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match chunk[0] {
            b'[' => {
                let mut r = 0;
                if !failed {
                    let (rr, n) = decode_rune(s);
                    r = rr;
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
                let c = if c == b'\\' {
                    chunk = &chunk[1..];
                    if chunk.is_empty() {
                        return Err(());
                    }
                    chunk[0]
                } else {
                    c
                };
                if !failed {
                    if c != s[0] {
                        failed = true;
                    }
                    s = &s[1..];
                }
                chunk = &chunk[1..];
            }
        }
    }
    if failed {
        return Ok((&[], false));
    }
    Ok((s, true))
}

fn get_esc(mut chunk: &[u8]) -> Result<(u32, &[u8]), ()> {
    if chunk.is_empty() || chunk[0] == b'-' || chunk[0] == b']' {
        return Err(());
    }
    if chunk[0] == b'\\' {
        chunk = &chunk[1..];
        if chunk.is_empty() {
            return Err(());
        }
    }
    let (r, n) = decode_rune(chunk);
    if r == 0xFFFD && n == 1 && std::str::from_utf8(&chunk[..1]).is_err() {
        return Err(());
    }
    let rest = &chunk[n..];
    if rest.is_empty() {
        return Err(());
    }
    Ok((r, rest))
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn match_matches_go() {
        assert_eq!(glob_match("*.go", "a.go"), Ok(true));
        assert_eq!(glob_match("*.go", "sub/a.go"), Ok(false));
        assert_eq!(glob_match("sub/*.go", "sub/a.go"), Ok(true));
        assert_eq!(glob_match("a?c", "abc"), Ok(true));
        assert_eq!(glob_match("[a-c]x", "bx"), Ok(true));
        assert_eq!(glob_match("[^a-c]x", "bx"), Ok(false));
        assert_eq!(glob_match("[", "a"), Err(()));
        assert_eq!(glob_match("a\\*b", "a*b"), Ok(true));
    }
}
