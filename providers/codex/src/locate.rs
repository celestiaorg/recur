//! Finding the `codex` CLI and Recur's `CODEX_HOME`.
//!
//! Recur drives Codex over `codex app-server` JSON-RPC rather than `codex
//! exec` or an SDK: mid-turn `turn/steer`, server→client approvals and
//! thread resume map onto the provider contract. Isolation rules: one
//! Recur-owned `CODEX_HOME` under the data dir (never `~/.codex` for writes,
//! never `.codex/` in a worktree), and the per-agent Recur MCP goes in as
//! `-c` overrides on argv so nothing is written into the worktree.

use std::path::{Path, PathBuf};

use crate::proc::{abs, look_path};

/// Overrides binary resolution (development and tests).
pub const ENV_BIN_PATH: &str = "RECUR_CODEX";

/// The single Recur-owned `CODEX_HOME` directory name under the data dir:
/// auth, sessions and trust live there; worktrees are isolated by `cwd`.
pub const HOME_DIR_NAME: &str = "codex-home";

pub fn home_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(HOME_DIR_NAME)
}

/// Resolves the native `codex` binary, in order: `$RECUR_CODEX`, the
/// configured path (Settings), `codex` on `PATH`, then common Homebrew cask
/// locations on macOS. The broken npm JS shim is rejected.
pub fn locate(configured: &str) -> Result<String, String> {
    locate_with(
        std::env::var(ENV_BIN_PATH).ok().as_deref(),
        configured,
        look_path("codex"),
        &common_bin_candidates(),
    )
}

fn locate_with(
    env: Option<&str>,
    configured: &str,
    on_path: Option<PathBuf>,
    common: &[PathBuf],
) -> Result<String, String> {
    let mut tried: Vec<String> = Vec::new();
    let add = |tried: &mut Vec<String>, p: &str| -> Option<String> {
        let p = p.trim();
        if p.is_empty() {
            return None;
        }
        if let Some(resolved) = resolve_candidate(Path::new(p)) {
            return Some(resolved);
        }
        tried.push(p.to_owned());
        None
    };
    if let Some(p) = add(&mut tried, env.unwrap_or_default()) {
        return Ok(p);
    }
    if let Some(p) = add(&mut tried, configured) {
        return Ok(p);
    }
    if let Some(p) = on_path {
        if let Some(resolved) = resolve_candidate(&p) {
            return Ok(resolved);
        }
        tried.push(format!("{} (unusable)", p.display()));
    }
    for cand in common {
        if let Some(p) = add(&mut tried, &cand.to_string_lossy()) {
            return Ok(p);
        }
    }
    let hint = if cfg!(target_os = "macos") {
        "install the Codex cask (`brew install --cask codex`)"
    } else {
        "install a native codex binary from https://github.com/openai/codex/releases"
    };
    Err(format!(
        "codex binary not found (looked in: {}) — {hint} or set {ENV_BIN_PATH}",
        tried.join(", ")
    ))
}

/// The absolute path when `p` is a native binary; `None` otherwise.
pub fn resolve_candidate(p: &Path) -> Option<String> {
    let abs = abs(p).ok()?;
    let meta = std::fs::metadata(&abs).ok()?;
    if meta.is_dir() || !looks_like_native_codex(&abs) {
        return None;
    }
    Some(abs.to_string_lossy().into_owned())
}

/// Rejects the Node wrapper at `node_modules/@openai/codex/bin/codex.js` (and
/// its PATH symlink), whose vendor binary is often missing: a real CLI is
/// Mach-O (thin or fat) or ELF, not a script.
fn looks_like_native_codex(path: &Path) -> bool {
    use std::io::Read;
    let mut hdr = [0u8; 4];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    if f.read_exact(&mut hdr).is_err() {
        return false;
    }
    matches!(
        hdr,
        [0xCF, 0xFA, 0xED, 0xFE]
            | [0xCE, 0xFA, 0xED, 0xFE]
            | [0xCA, 0xFE, 0xBA, 0xBE]
            | [0x7F, b'E', b'L', b'F']
    )
}

fn common_bin_candidates() -> Vec<PathBuf> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for root in ["/opt/homebrew/Caskroom/codex", "/usr/local/Caskroom/codex"] {
        let root = Path::new(root);
        out.push(root.join("latest/bin/codex"));
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut names: Vec<_> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name() != "latest")
            .map(|e| e.file_name())
            .collect();
        // os.ReadDir returns entries sorted by name.
        names.sort();
        for name in names {
            out.push(root.join(&name).join("codex-package/bin/codex"));
            out.push(root.join(&name).join("bin/codex"));
        }
    }
    out.push("/Applications/Codex.app/Contents/Resources/bin/codex".into());
    out
}

#[cfg(test)]
mod tests {

    use super::*;

    fn write_exec(path: &Path, body: &[u8]) {
        recur_exec::write_executable(path, body, 0o755).unwrap();
    }

    #[test]
    fn env_override() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("codex");
        // Minimal Mach-O thin 64-bit magic.
        write_exec(&bin, &[0xCF, 0xFA, 0xED, 0xFE, 0, 0, 0, 0]);
        let got = locate_with(Some(&bin.to_string_lossy()), "", None, &[]).unwrap();
        assert_eq!(Path::new(&got).file_name().unwrap(), "codex");
        let got = locate_with(None, &bin.to_string_lossy(), None, &[]).unwrap();
        assert_eq!(got, abs(&bin).unwrap().to_string_lossy());
    }

    #[test]
    fn rejects_node_shim() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("codex");
        write_exec(&bin, b"#!/usr/bin/env node\nconsole.log('shim')\n");
        assert_eq!(resolve_candidate(&bin), None);
        assert_eq!(resolve_candidate(dir.path()), None);
    }

    #[test]
    fn not_found_lists_what_was_tried() {
        let shim = tempfile::tempdir().unwrap();
        let on_path = shim.path().join("codex");
        write_exec(&on_path, b"#!/bin/sh\n");
        let err = locate_with(
            Some("/no/such/codex"),
            " ",
            Some(on_path.clone()),
            &[PathBuf::from("/nope/codex")],
        )
        .unwrap_err();
        assert!(
            err.contains(&format!("{} (unusable)", on_path.display())),
            "{err}"
        );
        assert!(
            err.starts_with("codex binary not found (looked in: /no/such/codex, "),
            "{err}"
        );
        assert!(err.contains("/nope/codex"), "{err}");
        assert!(err.ends_with(" or set RECUR_CODEX"), "{err}");
    }

    #[test]
    fn home() {
        assert_eq!(
            home_dir(Path::new("/data")),
            PathBuf::from("/data/codex-home")
        );
    }
}
