//! The `claude -p` argv: permission flags, the MCP config and the append-prompt
//! spill.
//!
//! ## Allowlist translation
//!
//! The engine's allowlist is Claude's own permission-rule syntax, so it passes
//! through unchanged as `--allowedTools a,b,c` (`Bash(git diff:*)`,
//! `mcp__recur__*` and `mcp__<server>__*` globs included). Claude only
//! auto-approves allowlisted tools; it does not hide the rest, so this module
//! also derives `--disallowedTools`, which removes bare-named tools from the
//! model's context:
//!
//! - `Edit`, `Write`, `NotebookEdit` when a non-empty allowlist omits them
//!   (the read-only guard, which `bypassPermissions` cannot override);
//! - `AskUserQuestion` (headless `-p` cannot collect answers; Recur's
//!   `mcp__recur__ask_user` does), `Skill` (Recur owns skill loading) and the
//!   subagent builtins `Agent`, `EnterWorktree`, `ExitWorktree` (delegation
//!   goes through `mcp__recur__create_workspace`) and `ReportFindings` (it
//!   reports to Claude Code's UI; Recur stores `mcp__recur__report_finding`)
//!   whenever builtins are visible, even with no allowlist.
//!
//! `tools = Some("")` disables the whole toolbox, so no deny list is passed.
//! The permission prompt tool is exempt from `--allowedTools`; Claude calls it
//! itself.

use std::collections::BTreeMap;

use recur_agent::TurnRequest;
use recur_agent::permission::PROMPT_TOOL;
use recur_domain::{PERMISSION_MODE_ASK, PERMISSION_MODE_DENY};
use serde_json::{Value, json};

use crate::flags;

/// The file-mutating builtins removed when an allowlist omits them.
pub const EDIT_BUILTINS: [&str; 3] = ["Edit", "Write", "NotebookEdit"];
/// Claude's interactive question tool; always denied when builtins show.
pub const ASK_USER_BUILTIN: &str = "AskUserQuestion";
/// Claude's native skill loader; always denied when builtins show.
pub const SKILL_BUILTIN: &str = "Skill";
/// In-process subagent and worktree tools; always denied when builtins show.
pub const SUBAGENT_BUILTINS: [&str; 3] = ["Agent", "EnterWorktree", "ExitWorktree"];
/// Claude Code's host-UI findings tool; always denied when builtins show.
pub const REPORT_FINDINGS_BUILTIN: &str = "ReportFindings";

/// Keeps `--append-system-prompt` well under Linux's 128 KiB single-argument
/// exec limit (MAX_ARG_STRLEN). Larger prompts go through
/// `--append-system-prompt-file`.
pub const MAX_INLINE_APPEND_PROMPT: usize = 64 << 10;

/// The edit builtins absent from a non-empty allowlist. An empty allowlist
/// means no edit restriction was intended.
pub fn disallowed_edit_tools(allowed: Option<&[String]>) -> Vec<&'static str> {
    let allowed = allowed.unwrap_or_default();
    if allowed.is_empty() {
        return Vec::new();
    }
    EDIT_BUILTINS
        .into_iter()
        .filter(|t| !allowed.iter().any(|a| a == t))
        .collect()
}

/// `--disallowedTools`: omitted edit builtins, then AskUserQuestion, Skill
/// the subagent builtins and ReportFindings.
pub fn disallowed_tools(allowed: Option<&[String]>) -> Vec<&'static str> {
    let mut out = disallowed_edit_tools(allowed);
    out.extend([ASK_USER_BUILTIN, SKILL_BUILTIN]);
    out.extend(SUBAGENT_BUILTINS);
    out.push(REPORT_FINDINGS_BUILTIN);
    out
}

/// The permission flags. The mode is machine-global (Settings), never derived
/// from the role:
///
/// - `bypassPermissions` skips every check; no prompt tool.
/// - `default` + `--permission-prompt-tool` routes anything outside
///   `--allowedTools` to the human through the turn's own Recur MCP endpoint.
///   Without an endpoint there is nowhere to ask, so the turn falls back to
///   `dontAsk`: hanging on an unanswerable prompt is worse than denying.
/// - empty fails closed to `dontAsk`.
pub fn permission_args(mode: &str, mcp_url: &str) -> Vec<String> {
    let mode = if mode.is_empty() {
        PERMISSION_MODE_DENY
    } else {
        mode
    };
    if mode != PERMISSION_MODE_ASK {
        return vec!["--permission-mode".into(), mode.into()];
    }
    if mcp_url.is_empty() {
        return vec!["--permission-mode".into(), PERMISSION_MODE_DENY.into()];
    }
    vec![
        "--permission-mode".into(),
        PERMISSION_MODE_ASK.into(),
        "--permission-prompt-tool".into(),
        PROMPT_TOOL.into(),
    ]
}

/// Joins two prompt parts with a blank line, trimming both; either may be
/// empty.
pub fn join_append_prompt(base: &str, extra: &str) -> String {
    let (base, extra) = (base.trim(), extra.trim());
    match (base.is_empty(), extra.is_empty()) {
        (true, _) => extra.to_owned(),
        (_, true) => base.to_owned(),
        _ => format!("{base}\n\n{extra}"),
    }
}

/// Whether the prompt goes on stream-json stdin instead of `-p`: needed for
/// mid-turn steers, image content blocks, and prompts too large for argv.
pub fn stream_input(req: &TurnRequest, steerable: bool) -> bool {
    steerable || !req.images.is_empty() || req.prompt_on_stdin
}

/// The argv after the binary (headless ref §1, §8):
///
/// ```text
/// -p <prompt> --output-format stream-json --verbose --disable-slash-commands
///   [--include-partial-messages] [--resume id [--fork-session]] [--model m]
///   [--effort e] [--thinking adaptive --thinking-display summarized]
///   [--append-system-prompt p] [--allowedTools …] [--disallowedTools …]
///   [--tools …] [--max-turns n] [--mcp-config json --strict-mcp-config]
///   --permission-mode … [--permission-prompt-tool …]
/// ```
///
/// With stream input the prompt is not on argv; it is the first stdin user
/// message. `--verbose` is mandatory with stream-json. `--bare` is never
/// passed (D8) so the user's Claude Code login works.
pub fn build(req: &TurnRequest, stream_input: bool) -> Vec<String> {
    let mut args: Vec<String> = if stream_input {
        [
            "-p",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
        ]
        .map(String::from)
        .into()
    } else {
        vec![
            "-p".into(),
            req.prompt.clone(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
        ]
    };
    // Recur owns the skill catalog and activation; this drops Claude's
    // native skill slash expansion (independent of --bare).
    args.push("--disable-slash-commands".into());
    if req.include_partials {
        args.push("--include-partial-messages".into());
    }
    if !req.resume_session.is_empty() {
        args.extend(["--resume".into(), req.resume_session.clone()]);
        if req.fork_session {
            args.push("--fork-session".into());
        }
    }
    if !req.model.is_empty() {
        args.extend([
            "--model".into(),
            flags::resolve_model(&req.model, &req.flags),
        ]);
    }
    let effort = flags::effort(&req.flags);
    if !effort.is_empty() {
        args.extend(["--effort".into(), effort.to_owned()]);
    }
    // Without --thinking-display headless claude redacts thinking (empty
    // text + signature). Cheap one-shots opt out of reasoning entirely.
    if !req.skip_thinking {
        args.extend([
            "--thinking".into(),
            "adaptive".into(),
            "--thinking-display".into(),
            "summarized".into(),
        ]);
    }
    let append = join_append_prompt(
        &join_append_prompt(&req.system_prompt, &req.append_system_prompt),
        &req.turn_append_prompt,
    );
    if !append.is_empty() {
        args.extend(["--append-system-prompt".into(), append]);
    }
    let allowed = req.allowed_tools.as_deref();
    if let Some(allowed) = allowed.filter(|a| !a.is_empty()) {
        args.extend(["--allowedTools".into(), allowed.join(",")]);
    }
    if req.tools.as_deref() != Some("") {
        args.extend([
            "--disallowedTools".into(),
            disallowed_tools(allowed).join(","),
        ]);
    }
    if let Some(tools) = &req.tools {
        args.extend(["--tools".into(), tools.clone()]);
    }
    if req.max_turns > 0 {
        args.extend(["--max-turns".into(), req.max_turns.to_string()]);
    }
    if !req.mcp_url.is_empty() || req.playwright || !req.user_mcp.is_empty() {
        args.extend([
            "--mcp-config".into(),
            mcp_config_json(req),
            "--strict-mcp-config".into(),
        ]);
    }
    args.extend(permission_args(&req.permission_mode, &req.mcp_url));
    args
}

/// The inline `--mcp-config` value: the Recur MCP over HTTP, Playwright over
/// stdio, and Settings → MCP servers (HTTP with headers, or stdio). With
/// `--strict-mcp-config` only these load. Empty when there are none.
pub fn mcp_config_json(req: &TurnRequest) -> String {
    let mut servers: BTreeMap<String, Value> = BTreeMap::new();
    if !req.mcp_url.is_empty() {
        servers.insert("recur".into(), json!({"type": "http", "url": req.mcp_url}));
    }
    if req.playwright {
        servers.insert("playwright".into(), playwright_server());
    }
    for u in &req.user_mcp {
        let name = u.name.trim();
        if name.is_empty() {
            continue;
        }
        let mut entry: BTreeMap<&str, Value> = BTreeMap::new();
        if !u.url.is_empty() {
            entry.insert("type", "http".into());
            entry.insert("url", u.url.clone().into());
            if !u.headers.is_empty() {
                entry.insert("headers", json!(u.headers));
            }
        } else if !u.command.is_empty() {
            entry.insert("command", u.command.clone().into());
            if !u.args.is_empty() {
                entry.insert("args", json!(u.args));
            }
            if !u.env.is_empty() {
                entry.insert("env", json!(u.env));
            }
        } else {
            continue;
        }
        servers.insert(name.to_owned(), json!(entry));
    }
    if servers.is_empty() {
        return String::new();
    }
    serde_json::to_string(&BTreeMap::from([("mcpServers", servers)])).unwrap_or_default()
}

/// Microsoft's Playwright MCP server, fetched on demand by npx. Its tools are
/// `mcp__playwright__*`, which the QA role allow-lists.
fn playwright_server() -> Value {
    json!({"args": ["-y", "@playwright/mcp@latest"], "command": "npx"})
}

/// An oversized `--append-system-prompt` moved into a private temp file; the
/// file is removed when this drops (after the process exits).
pub type SpilledPrompt = Option<tempfile::TempPath>;

/// Moves an `--append-system-prompt` value over [`MAX_INLINE_APPEND_PROMPT`]
/// bytes into `--append-system-prompt-file`.
pub fn spill_append_prompt(mut args: Vec<String>) -> std::io::Result<(Vec<String>, SpilledPrompt)> {
    use std::io::Write;
    let Some(i) = (0..args.len().saturating_sub(1)).find(|&i| {
        args[i] == "--append-system-prompt" && args[i + 1].len() > MAX_INLINE_APPEND_PROMPT
    }) else {
        return Ok((args, None));
    };
    let mut f = tempfile::Builder::new()
        .prefix("recur-system-prompt-")
        .suffix(".md")
        .tempfile()?;
    f.write_all(args[i + 1].as_bytes())?;
    let path = f.into_temp_path();
    args[i] = "--append-system-prompt-file".into();
    args[i + 1] = path.to_string_lossy().into_owned();
    Ok((args, Some(path)))
}

#[cfg(test)]
mod tests {
    use recur_agent::McpAttach;

    use super::*;

    fn has_flag(args: &[String], flag: &str) -> bool {
        args.iter().any(|a| a == flag)
    }

    fn has_pair(args: &[String], flag: &str, val: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == val)
    }

    fn req(prompt: &str) -> TurnRequest {
        TurnRequest {
            prompt: prompt.into(),
            ..Default::default()
        }
    }

    fn strs(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn permission_args_cases() {
        let join = |mode: &str, url: &str| permission_args(mode, url).join(" ");
        assert_eq!(
            join("bypassPermissions", "http://x/mcp"),
            "--permission-mode bypassPermissions"
        );
        assert_eq!(
            join("default", "http://x/mcp"),
            "--permission-mode default --permission-prompt-tool mcp__recur__permission_request"
        );
        assert_eq!(join("default", ""), "--permission-mode dontAsk");
        assert_eq!(join("", ""), "--permission-mode dontAsk");
    }

    #[test]
    fn disallowed_edit_tools_cases() {
        assert!(disallowed_edit_tools(None).is_empty());
        assert!(disallowed_edit_tools(Some(&[])).is_empty());
        let ro = strs(&["Read", "Bash", "mcp__recur__report_finding"]).unwrap();
        assert_eq!(disallowed_edit_tools(Some(&ro)), EDIT_BUILTINS);
        let all = strs(&["Read", "Edit", "Write", "Bash", "NotebookEdit"]).unwrap();
        assert!(disallowed_edit_tools(Some(&all)).is_empty());
    }

    #[test]
    fn disallowed_tools_cases() {
        assert_eq!(
            disallowed_tools(None).join(","),
            "AskUserQuestion,Skill,Agent,EnterWorktree,ExitWorktree,ReportFindings"
        );
        let ro = strs(&["Read", "Bash", "mcp__recur__ask_user"]).unwrap();
        assert_eq!(
            disallowed_tools(Some(&ro)).join(","),
            "Edit,Write,NotebookEdit,AskUserQuestion,Skill,Agent,EnterWorktree,ExitWorktree,ReportFindings"
        );
    }

    #[test]
    fn appends_role_not_replace() {
        let args = build(
            &TurnRequest {
                system_prompt: "You are the Maestro.".into(),
                append_system_prompt: "Name the chat.".into(),
                ..req("hi")
            },
            false,
        );
        assert!(!has_flag(&args, "--system-prompt"));
        assert!(has_pair(
            &args,
            "--append-system-prompt",
            "You are the Maestro.\n\nName the chat."
        ));
    }

    #[test]
    fn model_effort_and_context() {
        let mut r = req("hi");
        r.model = "sonnet".into();
        r.flags.insert("effort".into(), "high".into());
        let args = build(&r, false);
        assert!(has_pair(&args, "--model", "sonnet"));
        assert!(has_pair(&args, "--effort", "high"));
        r.model = "claude-sonnet-4-6".into();
        r.flags.insert("context".into(), "1m".into());
        assert!(has_pair(
            &build(&r, false),
            "--model",
            "claude-sonnet-4-6[1m]"
        ));
    }

    #[test]
    fn fork_session() {
        let mut r = req("hi");
        r.resume_session = "sess-1".into();
        r.fork_session = true;
        let args = build(&r, false);
        assert!(has_pair(&args, "--resume", "sess-1") && has_flag(&args, "--fork-session"));
        r.fork_session = false;
        assert!(!has_flag(&build(&r, false), "--fork-session"));
        let mut no_resume = req("hi");
        no_resume.fork_session = true;
        assert!(!has_flag(&build(&no_resume, false), "--fork-session"));
    }

    #[test]
    fn stream_input_keeps_prompt_off_argv() {
        let mut r = req("huge-fork-history");
        r.prompt_on_stdin = true;
        assert!(stream_input(&r, false));
        let args = build(&r, stream_input(&r, false));
        assert!(!args.iter().any(|a| a == "huge-fork-history"));
        assert!(has_pair(&args, "--input-format", "stream-json"));
        let steer = req("do the thing");
        assert!(stream_input(&steer, true));
        let args = build(&steer, true);
        assert!(has_pair(&args, "--input-format", "stream-json"));
        assert!(!has_pair(&args, "-p", "do the thing"));
        assert!(!stream_input(&steer, false));
    }

    #[test]
    fn partials() {
        let mut r = req("hi");
        r.include_partials = true;
        assert!(has_flag(&build(&r, false), "--include-partial-messages"));
        assert!(!has_flag(
            &build(&req("hi"), false),
            "--include-partial-messages"
        ));
    }

    #[test]
    fn permission_mode() {
        assert!(has_pair(
            &build(&req("hi"), false),
            "--permission-mode",
            "dontAsk"
        ));
        let mut r = req("hi");
        r.permission_mode = "bypassPermissions".into();
        assert!(has_pair(
            &build(&r, false),
            "--permission-mode",
            "bypassPermissions"
        ));
        r.mcp_url = "http://127.0.0.1:1/t/mcp".into();
        assert!(!has_flag(&build(&r, false), "--permission-prompt-tool"));
        r.permission_mode = "default".into();
        let ask = build(&r, false);
        assert!(has_pair(&ask, "--permission-mode", "default"));
        assert!(has_pair(
            &ask,
            "--permission-prompt-tool",
            "mcp__recur__permission_request"
        ));
    }

    #[test]
    fn disallowed_edit_tools_in_argv() {
        let mut ro = req("hi");
        ro.allowed_tools = strs(&["Read", "Bash", "mcp__recur__report_finding"]);
        ro.permission_mode = "bypassPermissions".into();
        assert!(has_pair(
            &build(&ro, false),
            "--disallowedTools",
            "Edit,Write,NotebookEdit,AskUserQuestion,Skill,Agent,EnterWorktree,ExitWorktree,ReportFindings"
        ));
        let mut coder = req("hi");
        coder.allowed_tools = strs(&["Read", "Edit", "Write", "Bash", "NotebookEdit"]);
        assert!(has_pair(
            &build(&coder, false),
            "--disallowedTools",
            "AskUserQuestion,Skill,Agent,EnterWorktree,ExitWorktree,ReportFindings"
        ));
        let bare = build(&req("hi"), false);
        assert!(has_pair(
            &bare,
            "--disallowedTools",
            "AskUserQuestion,Skill,Agent,EnterWorktree,ExitWorktree,ReportFindings"
        ));
        assert!(has_flag(&bare, "--disable-slash-commands"));
    }

    #[test]
    fn completion_turn() {
        let mut r = req("hi");
        r.tools = Some(String::new());
        r.max_turns = 1;
        r.skip_thinking = true;
        let args = build(&r, false);
        assert!(has_pair(&args, "--tools", ""));
        assert!(!has_flag(&args, "--disallowedTools"));
        assert!(has_flag(&args, "--disable-slash-commands"));
        assert!(has_pair(&args, "--max-turns", "1"));
        assert!(!has_flag(&args, "--thinking"));
        let normal = build(&req("hi"), false);
        assert!(has_pair(&normal, "--thinking", "adaptive"));
        assert!(!has_flag(&normal, "--tools") && !has_flag(&normal, "--max-turns"));
    }

    #[test]
    fn mcp_config() {
        let stdio = TurnRequest {
            user_mcp: vec![McpAttach {
                name: "fs".into(),
                command: "npx".into(),
                args: vec![
                    "-y".into(),
                    "@modelcontextprotocol/server-filesystem".into(),
                ],
                env: [("FOO".to_owned(), "bar".to_owned())].into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let raw = mcp_config_json(&stdio);
        assert!(
            raw.contains(r#""command":"npx""#) && raw.contains(r#""FOO":"bar""#),
            "{raw}"
        );

        let linear = TurnRequest {
            mcp_url: "http://127.0.0.1:9/tok/mcp".into(),
            user_mcp: vec![McpAttach {
                name: "linear".into(),
                url: "https://mcp.linear.app/mcp".into(),
                headers: [("Authorization".to_owned(), "Bearer lin_api_x".to_owned())].into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            mcp_config_json(&linear),
            r#"{"mcpServers":{"linear":{"headers":{"Authorization":"Bearer lin_api_x"},"type":"http","url":"https://mcp.linear.app/mcp"},"recur":{"type":"http","url":"http://127.0.0.1:9/tok/mcp"}}}"#
        );
        assert_eq!(mcp_config_json(&TurnRequest::default()), "");
        let qa = TurnRequest {
            playwright: true,
            ..Default::default()
        };
        assert_eq!(
            mcp_config_json(&qa),
            r#"{"mcpServers":{"playwright":{"args":["-y","@playwright/mcp@latest"],"command":"npx"}}}"#
        );
    }

    /// A plan-sized append prompt never reaches argv over MAX_ARG_STRLEN.
    #[test]
    fn spill_large_append_prompt() {
        let sys = "x".repeat(256 << 10);
        let r = TurnRequest {
            system_prompt: sys.clone(),
            ..req("go")
        };
        let (args, spilled) = spill_append_prompt(build(&r, false)).unwrap();
        assert!(args.iter().all(|a| a.len() < 128 << 10));
        let i = args
            .iter()
            .position(|a| a == "--append-system-prompt-file")
            .expect("spilled");
        assert_eq!(std::fs::read_to_string(&args[i + 1]).unwrap(), sys);
        let path = args[i + 1].clone();
        drop(spilled);
        assert!(!std::path::Path::new(&path).exists());

        let small = TurnRequest {
            system_prompt: "short".into(),
            ..req("go")
        };
        let (args, spilled) = spill_append_prompt(build(&small, false)).unwrap();
        assert!(spilled.is_none());
        assert!(has_pair(&args, "--append-system-prompt", "short"));
    }
}
