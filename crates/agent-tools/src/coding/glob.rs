use std::fs;
use std::path::Path;

use recur_domain::path::{base, glob_match, rel};

use super::data_dir;
use super::grep::{glob_args, run_rg, search_root};
use crate::json::{Shape, Ty, field, unmarshal};
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

/// Walk order (lexical per directory, symlinks not followed);
/// unreadable entries are skipped.
fn walk(root: &str, p: &str, pattern: &str, out: &mut Vec<String>) {
    let Ok(meta) = fs::symlink_metadata(p) else {
        return;
    };
    if meta.is_dir() {
        let name = base(p);
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
    let Some(r) = rel(root, p) else { return };
    let base = base(&r);
    if glob_match(pattern, &base).unwrap_or(false) || glob_match(pattern, &r).unwrap_or(false) {
        out.push(r);
    }
}
