use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;

use crate::home::{abs, expand_home, join};

/// Repo-relative files read as expansion lookup for project env values.
/// Later entries win. Missing files are skipped.
pub const DOTENV_FILES: [&str; 4] = [".env", ".env.local", ".recur/.env", ".recur/.env.local"];

const DOTENV_MAX_BYTES: u64 = 256 << 10;

/// Parses `KEY=value` lines. Blank lines and `#` comments are ignored, an
/// `export ` prefix is dropped, and one pair of matching single or double
/// quotes around the value is stripped. Later keys win.
pub fn parse_dotenv(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.split('\n') {
        let s = line.trim();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        let s = s.strip_prefix("export ").unwrap_or(s).trim();
        let Some(i) = s.find('=') else {
            continue;
        };
        if i == 0 {
            continue;
        }
        let k = s[..i].trim();
        if k.is_empty() {
            continue;
        }
        let mut v = s[i + 1..].trim();
        let b = v.as_bytes();
        if b.len() >= 2
            && ((b[0] == b'"' && b[b.len() - 1] == b'"')
                || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
        {
            v = &v[1..v.len() - 1];
        }
        out.insert(k.to_owned(), v.to_owned());
    }
    out
}

/// Reads [`DOTENV_FILES`] from each existing directory in dirs (typically the
/// project checkout, then the worktree). Later dirs and later files win.
/// Unreadable or oversized files are logged and skipped.
pub fn load_dotenv_files(dirs: &[&str]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for dir in dirs {
        let dir = expand_home(dir).trim().to_owned();
        if dir.is_empty() {
            continue;
        }
        let Ok(abs) = abs(&dir) else {
            continue;
        };
        if seen.contains(&abs) {
            continue;
        }
        if !std::fs::metadata(&abs).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        seen.insert(abs.clone());
        for rel in DOTENV_FILES {
            merge_dotenv_file(&mut out, &join(&abs, rel));
        }
    }
    out
}

fn merge_dotenv_file(into: &mut BTreeMap<String, String>, path: &str) {
    let st = match std::fs::metadata(path) {
        Ok(st) => st,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("recur: dotenv {path}: {e}");
            }
            return;
        }
    };
    if !st.is_file() {
        return;
    }
    if st.len() > DOTENV_MAX_BYTES {
        tracing::warn!("recur: dotenv {path}: skipping (larger than {DOTENV_MAX_BYTES} bytes)");
        return;
    }
    let mut data = Vec::new();
    let read =
        std::fs::File::open(path).and_then(|f| f.take(DOTENV_MAX_BYTES + 1).read_to_end(&mut data));
    if let Err(e) = read {
        tracing::warn!("recur: dotenv {path}: {e}");
        return;
    }
    if data.len() as u64 > DOTENV_MAX_BYTES {
        tracing::warn!("recur: dotenv {path}: skipping (larger than {DOTENV_MAX_BYTES} bytes)");
        return;
    }
    into.extend(parse_dotenv(&String::from_utf8_lossy(&data)));
}
