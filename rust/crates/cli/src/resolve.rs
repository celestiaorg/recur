//! Bundle target resolution for `bundles test` / `bundles lint`
//! (`internal/cli/resolve.go`).

use recur_bundles::go;

/// Expands CLI args into absolute bundle directories. Each arg may be a
/// bundle dir, a dir of bundles (every child with `bundle.toml`), or a bare
/// name looked up under the user root, `./bundles`, then the shipped root.
/// No args means the current directory.
pub fn resolve_bundle_targets(
    args: &[String],
    user_root: &str,
    shipped_root: &str,
) -> Result<Vec<String>, String> {
    let default = [".".to_string()];
    let args = if args.is_empty() { &default[..] } else { args };
    let mut out: Vec<String> = Vec::new();
    let add = |dir: &str, out: &mut Vec<String>| -> Result<(), String> {
        let abs = go::abs(dir).map_err(|e| go::errno_text(&e))?;
        if !has_bundle_toml(&abs) {
            return Err(format!("{abs}: no bundle.toml"));
        }
        if !out.contains(&abs) {
            out.push(abs);
        }
        Ok(())
    };
    for a in args {
        if let Some(resolved) = try_dir(a) {
            if has_bundle_toml(&resolved) {
                add(&resolved, &mut out)?;
                continue;
            }
            let children = list_bundle_children(&resolved)?;
            if children.is_empty() {
                return Err(format!("{resolved}: no bundle.toml"));
            }
            for c in children {
                add(&c, &mut out)?;
            }
            continue;
        }
        let mut candidates = vec![go::join(&[user_root, a]), go::join(&["bundles", a])];
        if !shipped_root.is_empty() {
            candidates.push(go::join(&[shipped_root, a]));
        }
        match candidates.iter().find(|c| has_bundle_toml(c)) {
            Some(c) => add(c, &mut out)?,
            None => {
                return Err(format!(
                    "bundle {} not found (looked in {user_root}, ./bundles, shipped)",
                    go::quote(a)
                ));
            }
        }
    }
    Ok(out)
}

fn try_dir(a: &str) -> Option<String> {
    let b = a.as_bytes();
    let mut looks_like_path =
        a == "." || a == ".." || a.contains('/') || (b.len() >= 2 && b[1] == b':');
    if !looks_like_path && std::fs::metadata(a).is_ok_and(|m| m.is_dir()) {
        looks_like_path = true;
    }
    if !looks_like_path {
        return None;
    }
    let abs = go::abs(a).ok()?;
    std::fs::metadata(&abs)
        .is_ok_and(|m| m.is_dir())
        .then_some(abs)
}

fn list_bundle_children(dir: &str) -> Result<Vec<String>, String> {
    let mut entries: Vec<(String, bool)> = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| go::path_err("open", dir, &e))? {
        let e = e.map_err(|e| go::path_err("readdirent", dir, &e))?;
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        entries.push((e.file_name().to_string_lossy().into_owned(), is_dir));
    }
    entries.sort();
    Ok(entries
        .into_iter()
        .filter(|(_, is_dir)| *is_dir)
        .map(|(name, _)| go::join(&[dir, &name]))
        .filter(|c| has_bundle_toml(c))
        .collect())
}

fn has_bundle_toml(dir: &str) -> bool {
    std::fs::metadata(go::join(&[dir, "bundle.toml"])).is_ok_and(|m| !m.is_dir())
}
