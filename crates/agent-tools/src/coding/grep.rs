use std::path::Path;
use std::process::Stdio;

use recur_domain::SearchOpts;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::{MAX_GREP_HITS, MAX_LINE_CHARS, data_dir};
use crate::json::{Shape, Ty, field, unmarshal};
use crate::path::{abs_eval, resolve_path};
use crate::tool::{Call, Tool, ToolError};
use recur_domain::path::{base, dir, rel};

const SCHEMA: &str = concat!(
    r#"{"type":"object","properties":{"#,
    r#""pattern":{"type":"string","description":"Regex (ripgrep syntax)"},"#,
    r#""path":{"type":"string","description":"File or directory to search (default: worktree root)"},"#,
    r#""glob":{"type":"string","description":"Filter files by glob relative to path, e.g. *.go or **/*.{ts,tsx}"},"#,
    r#""type":{"type":"string","description":"ripgrep file type, e.g. go, ts, py"},"#,
    r#""output_mode":{"type":"string","enum":["content","files_with_matches","count"],"description":"content (default): matching lines; files_with_matches: paths only; count: matches per file"},"#,
    r#""-i":{"type":"boolean","description":"Case insensitive"},"#,
    r#""-A":{"type":"number","description":"Lines of context after each match (content mode)"},"#,
    r#""-B":{"type":"number","description":"Lines of context before each match (content mode)"},"#,
    r#""-C":{"type":"number","description":"Lines of context around each match (content mode)"},"#,
    r#""multiline":{"type":"boolean","description":"Let patterns span lines; . matches newlines"}"#,
    r#"},"required":["pattern"]}"#
);

static IN: Shape = Shape::anon(&[
    field("pattern", Ty::Str),
    field("path", Ty::Str),
    field("glob", Ty::Str),
    field("type", Ty::Str),
    field("output_mode", Ty::Str),
    field("-i", Ty::Bool),
    field("-A", Ty::Float),
    field("-B", Ty::Float),
    field("-C", Ty::Float),
    field("multiline", Ty::Bool),
]);

pub struct GrepTool;

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Search file contents with ripgrep. Case sensitive unless -i. Respects .gitignore and includes hidden files. Prefer this over shell grep/rg."
    }

    fn schema(&self) -> String {
        SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let a =
            unmarshal(&c.args, &IN).map_err(|e| ToolError(format!("invalid Grep args: {e}")))?;
        let pattern = a.str("pattern");
        if pattern.is_empty() {
            return Err("pattern is required".into());
        }
        let (dir, target) = search_root(&c.worktree, a.str("path"))?;
        let Ok(rg_path) = recur_code::rg::locate(&data_dir()) else {
            return grep_fallback(&c, &dir, &target, pattern, a.str("glob"), a.bool("-i"));
        };
        let mut args: Vec<String> = vec![
            "--no-config".into(),
            "--no-messages".into(),
            "--color".into(),
            "never".into(),
            "--hidden".into(),
            "--glob".into(),
            "!.git".into(),
            "--max-columns".into(),
            MAX_LINE_CHARS.to_string(),
            "--max-columns-preview".into(),
        ];
        match a.str("output_mode") {
            "" | "content" => {
                args.extend(["--line-number", "--with-filename", "--no-heading"].map(String::from));
                for (flag, n) in [
                    ("-A", a.float("-A")),
                    ("-B", a.float("-B")),
                    ("-C", a.float("-C")),
                ] {
                    if n > 0.0 {
                        args.push(flag.into());
                        args.push((n as i64).to_string());
                    }
                }
            }
            "files_with_matches" => args.push("--files-with-matches".into()),
            "count" => args.extend(["--count", "--with-filename"].map(String::from)),
            _ => return Err("output_mode must be content, files_with_matches, or count".into()),
        }
        if a.bool("-i") {
            args.push("--ignore-case".into());
        }
        if a.bool("multiline") {
            args.extend(["--multiline", "--multiline-dotall"].map(String::from));
        }
        args.extend(glob_args(a.str("glob")));
        if !a.str("type").is_empty() {
            args.push("--type".into());
            args.push(a.str("type").into());
        }
        args.extend(["-e".to_string(), pattern.to_string(), target]);
        let (lines, truncated) = run_rg(
            &c.cancel,
            Path::new(&rg_path),
            &c.worktree,
            &dir,
            &args,
            MAX_GREP_HITS,
        )
        .await?;
        if lines.is_empty() {
            return Ok("No matches.".into());
        }
        let mut out = lines.join("\n") + "\n";
        if truncated {
            out.push_str(&format!("\n<truncated at {MAX_GREP_HITS} lines>\n"));
        }
        Ok(out)
    }
}

/// Filters rg by glob without un-ignoring files: a positive `--glob`
/// overrides .gitignore, a `--type` filter does not. Type globs only match
/// file names, so path globs still use `--glob`.
// simplify: path globs like src/*.go also match gitignored files; filter rg's
// output if that starts to matter.
pub(crate) fn glob_args(g: &str) -> Vec<String> {
    let g = g.trim();
    if g.is_empty() {
        return Vec::new();
    }
    let base = g.strip_prefix("**/").unwrap_or(g);
    if !g.starts_with('!') && !base.contains('/') {
        return vec![
            "--type-add".into(),
            format!("recurglob:{base}"),
            "--type".into(),
            "recurglob".into(),
        ];
    }
    vec!["--glob".into(), g.into()]
}

/// The directory rg runs in and the target it searches there ("." or
/// "./<file>"), so globs are relative to path.
pub(crate) fn search_root(worktree: &str, path: &str) -> Result<(String, String), ToolError> {
    if path.trim().is_empty() {
        return Ok((abs_eval(worktree)?, ".".into()));
    }
    let abs = resolve_path(worktree, path)?;
    let meta = std::fs::metadata(&abs).map_err(|e| format!("{abs}: {e}"))?;
    if meta.is_dir() {
        return Ok((abs, ".".into()));
    }
    Ok((dir(&abs), format!("./{}", base(&abs))))
}

/// dir relative to the worktree with a trailing slash ("" at the root).
fn worktree_prefix(worktree: &str, dir: &str) -> String {
    let Ok(wt) = abs_eval(worktree) else {
        return String::new();
    };
    match rel(&wt, dir) {
        Some(r) if r != "." => format!("{r}/"),
        _ => String::new(),
    }
}

/// The maximum line size when scanning rg output.
const MAX_SCAN_LINE: usize = 1024 * 1024;

/// Runs ripgrep in dir and returns up to `limit` (0 = all) output lines with
/// leading "./" paths rewritten to worktree-relative ones.
pub(crate) async fn run_rg(
    cancel: &CancellationToken,
    rg: &Path,
    worktree: &str,
    dir: &str,
    args: &[String],
    limit: usize,
) -> Result<(Vec<String>, bool), ToolError> {
    let prefix = worktree_prefix(worktree, dir);
    let mut child = tokio::process::Command::new(rg)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ToolError(format!("rg start: {}: {e}", rg.to_string_lossy())))?;
    let mut stderr_pipe = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(p) = stderr_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf).await;
        }
        buf
    });
    let mut lines = Vec::new();
    let mut truncated = false;
    let scan = async {
        let Some(stdout) = child.stdout.take() else {
            return;
        };
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if buf.last() == Some(&b'\n') {
                buf.pop();
            }
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            // bufio.Scanner stops at a token over its buffer size.
            if buf.len() > MAX_SCAN_LINE {
                return;
            }
            if limit > 0 && lines.len() == limit {
                truncated = true;
                return;
            }
            let line = String::from_utf8_lossy(&buf).into_owned();
            lines.push(match line.strip_prefix("./") {
                Some(rest) => format!("{prefix}{rest}"),
                None => line,
            });
        }
    };
    tokio::select! {
        _ = scan => {}
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err("context canceled".into());
        }
    }
    if truncated {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Ok((lines, true));
    }
    let status = tokio::select! {
        s = child.wait() => s,
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err("context canceled".into());
        }
    };
    let stderr = stderr_task.await.unwrap_or_default();
    match status {
        Ok(s) if s.success() => Ok((lines, false)),
        // Exit 1 is "no matches"; exit 2 with output is a partial error.
        Ok(s) if s.code().is_some() && (s.code() == Some(1) || !lines.is_empty()) => {
            Ok((lines, false))
        }
        Ok(s) => {
            let msg = String::from_utf8_lossy(&stderr)
                .into_owned()
                .trim()
                .to_string();
            if !msg.is_empty() {
                return Err(msg.into());
            }
            Err(super::bash::exit_status_text(&s).into())
        }
        Err(e) => Err(e.to_string().into()),
    }
}

/// Searches without ripgrep: the editor's walk, which skips dot-dirs and
/// build dirs and ignores output_mode, type, context and multiline.
fn grep_fallback(
    c: &Call,
    dir: &str,
    target: &str,
    pattern: &str,
    glob: &str,
    ignore_case: bool,
) -> Result<String, ToolError> {
    let glob = if target != "." && glob.is_empty() {
        target.strip_prefix("./").unwrap_or(target)
    } else {
        glob
    };
    let res = recur_code::search_opts(
        &c.cancel,
        Path::new(dir),
        pattern,
        &SearchOpts {
            regex: true,
            case_sensitive: !ignore_case,
            glob: glob.into(),
            limit: MAX_GREP_HITS as i64,
            offset: 0,
        },
    )
    .map_err(|e| ToolError(e.to_string()))?;
    let hits = res.hits.unwrap_or_default();
    if hits.is_empty() {
        let mut msg = "No matches.".to_string();
        if res.skipped_large > 0 {
            msg.push_str(&format!(" ({} large files skipped)", res.skipped_large));
        }
        return Ok(msg);
    }
    let prefix = worktree_prefix(&c.worktree, dir);
    let mut b = String::new();
    for h in &hits {
        b.push_str(&format!(
            "{prefix}{}:{}:{}\n",
            h.path,
            h.line,
            h.preview.trim_end_matches('\n')
        ));
    }
    if res.truncated {
        b.push_str(&format!("\n<truncated at {MAX_GREP_HITS} hits>\n"));
    }
    Ok(b)
}
