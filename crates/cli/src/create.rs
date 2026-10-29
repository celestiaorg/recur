//! `recur bundles create` (`internal/cli/bundles_create.go`).

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::LazyLock;

use include_dir::{Dir, DirEntry, include_dir};
use recur_domain::path;
use regex::Regex;

/// The `recur bundles create` template tree, embedded from its Go home.
static SCAFFOLD: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/scaffold");

static SAFE_BUNDLE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^[A-Za-z][A-Za-z0-9_-]*$").expect("name pattern"));

static ID_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}\p{Nd}_-]$").expect("id chars"));

pub fn normalize_bundle_name(name: &str) -> String {
    name.trim().to_string()
}

/// A dotted-safe id from a display name (`MyBundle` → `mybundle`).
pub fn scaffold_bundle_id(name: &str) -> String {
    let id: String = name
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| ID_CHARS.is_match(c.encode_utf8(&mut [0u8; 4])))
        .collect();
    if id.is_empty() {
        "bundle".to_string()
    } else {
        id
    }
}

/// Scaffolds `dest_parent/<name>` from the embedded tree, substituting
/// `{{BUNDLE_NAME}}`, `{{BUNDLE_ID}}`, `{{TS_CMD_ID}}`, `{{SH_CMD_ID}}`.
pub fn create_bundle(name: &str, dest_parent: &str, force: bool) -> Result<String, String> {
    if !SAFE_BUNDLE_NAME.is_match(name) {
        return Err(format!(
            "invalid bundle name {:?} (use letters, numbers, _ or -; start with a letter)",
            name
        ));
    }
    let dest = path::join(&[dest_parent, name]);
    if let Ok(st) = std::fs::metadata(&dest) {
        if !force {
            return Err(format!("{dest} already exists (pass --force to replace)"));
        }
        if !st.is_dir() {
            return Err(format!("{dest} exists and is not a directory"));
        }
        std::fs::remove_dir_all(&dest).map_err(|e| format!("{dest}: {e}"))?;
    }
    mkdir_all(&dest)?;
    let id = scaffold_bundle_id(name);
    let pairs = [
        ("{{BUNDLE_NAME}}", name.to_string()),
        ("{{BUNDLE_ID}}", id.clone()),
        ("{{TS_CMD_ID}}", format!("{id}.uppercase")),
        ("{{SH_CMD_ID}}", format!("{id}.prefix-lines")),
    ];
    write_tree(&SCAFFOLD, &dest, &pairs)?;
    Ok(dest)
}

/// Walks the embedded tree in lexical order (Go `fs.WalkDir`).
fn write_tree(dir: &Dir<'static>, dest: &str, pairs: &[(&str, String)]) -> Result<(), String> {
    let mut entries: Vec<&DirEntry<'static>> = dir.entries().iter().collect();
    entries.sort_by_key(|e| e.path().file_name().map(|n| n.to_os_string()));
    for e in entries {
        let rel = e.path().to_string_lossy().into_owned();
        let target = path::join(&[dest, &rel]);
        match e {
            DirEntry::Dir(d) => {
                mkdir_all(&target)?;
                write_tree(d, dest, pairs)?;
            }
            DirEntry::File(f) => {
                let body = replace_all(&String::from_utf8_lossy(f.contents()), pairs);
                let mode = if rel.starts_with("bin/") || rel.ends_with(".sh") {
                    0o755
                } else {
                    0o644
                };
                mkdir_all(&path::dir(&target))?;
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(mode)
                    .open(&target)
                    .map_err(|e| format!("{target}: {e}"))?;
                out.write_all(body.as_bytes())
                    .map_err(|e| format!("{target}: {e}"))?;
            }
        }
    }
    Ok(())
}

/// Go `strings.NewReplacer`: one left-to-right pass; at each position the
/// first listed pattern that matches wins.
fn replace_all(s: &str, pairs: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    'scan: while !rest.is_empty() {
        for (old, new) in pairs {
            if let Some(after) = rest.strip_prefix(old) {
                out.push_str(new);
                rest = after;
                continue 'scan;
            }
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn mkdir_all(dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_replacer() {
        assert_eq!(scaffold_bundle_id("MyBundle"), "mybundle");
        assert_eq!(scaffold_bundle_id("My-Pack_2"), "my-pack_2");
        assert_eq!(scaffold_bundle_id("!!"), "bundle");
        let pairs = [("{{A}}", "x".to_string()), ("{{AB}}", "y".to_string())];
        assert_eq!(replace_all("{{A}}-{{AB}}-é", &pairs), "x-y-é");
    }

    #[test]
    fn rejects_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_string_lossy().into_owned();
        assert_eq!(
            create_bundle("bad name", &d, false).unwrap_err(),
            "invalid bundle name \"bad name\" (use letters, numbers, _ or -; start with a letter)"
        );
    }
}
