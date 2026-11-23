//! `tool_call` rows: one per tool_use block, closed by its tool_result, so
//! Insights can aggregate a time range without reading transcript JSON. The
//! label groups calls by program ("cargo test") or tool family.

use std::time::Instant;

use recur_domain::{ToolResultPayload, ToolUsePayload};
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde_json::Value;

use crate::db::Db;
use crate::error::{Error, Result, SqlContext as _};
use crate::store::Store;

pub(crate) const FILES: &str = "Read / Edit files";
pub(crate) const SEARCH: &str = "Search (rg, grep)";
const GIT: &str = "git, gh";

/// Tools that wait on the human rather than run.
pub(crate) fn is_ask(name: &str) -> bool {
    name == "AskUserQuestion" || name.ends_with("__ask_user")
}

/// The program or tool group a call counts under. Claude (`Bash`, `Read`),
/// Codex and Cursor (`shell`, `read`, `mcp`) map to the same labels.
pub(crate) fn tool_label(name: &str, input: &str) -> String {
    let arg = |k: &str| -> Option<Value> {
        let v: Value = serde_json::from_str(input).ok()?;
        v.get(k).cloned()
    };
    match name {
        "Bash" | "shell" => match arg("command") {
            Some(Value::String(s)) => command_label(&s),
            Some(Value::Array(a)) => command_label(
                &a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => "shell".into(),
        },
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "read" | "edit" | "write"
        | "delete" => FILES.into(),
        "Grep" | "Glob" | "grep" | "glob" => SEARCH.into(),
        "mcp" => {
            let s = |k: &str| match arg(k) {
                Some(Value::String(s)) => s,
                _ => String::new(),
            };
            match (s("providerIdentifier"), s("toolName")) {
                (_, t) if t.is_empty() => name.into(),
                (p, t) if p.is_empty() => format!("mcp__{t}"),
                (p, t) => format!("mcp__{p}__{t}"),
            }
        }
        _ => name.into(),
    }
}

/// `cd x && FOO=1 cargo test -p y 2>&1 | tail` is "cargo test": the first
/// real program of the first line, with a subcommand for multiplexers.
fn command_label(cmd: &str) -> String {
    let cmd = unwrap_shell(cmd.trim());
    let line = cmd.lines().next().unwrap_or_default();
    for seg in line.split([';', '|', '&']) {
        let mut words = seg
            .split_whitespace()
            .map(|w| w.trim_matches(['"', '\'', '(', ')']))
            .skip_while(|w| {
                matches!(
                    *w,
                    "time"
                        | "env"
                        | "nohup"
                        | "sudo"
                        | "command"
                        | "exec"
                        | "timeout"
                        | "do"
                        | "then"
                        | "else"
                        | "if"
                        | "!"
                ) || w.starts_with(|c: char| c.is_ascii_digit())
                    || w.split_once('=').is_some_and(|(k, _)| {
                        !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    })
            });
        let Some(prog) = words.next() else { continue };
        let prog = prog.rsplit('/').next().unwrap_or(prog);
        if matches!(
            prog,
            "cd" | "pushd" | "popd" | "export" | "set" | "source" | "." | "true" | ":" | "for"
        ) {
            continue;
        }
        let mut args = words.filter(|w| !w.starts_with(['-', '+']));
        return match prog {
            "git" | "gh" => GIT.into(),
            "until" | "while" => "Wait loops (until, while)".into(),
            "rg" | "grep" | "egrep" | "fgrep" | "ag" | "fd" | "find" => SEARCH.into(),
            "cat" | "head" | "tail" | "sed" | "nl" | "wc" | "ls" => FILES.into(),
            "cargo" => match args.next() {
                Some("test" | "nextest") => "cargo test".into(),
                Some("build" | "check") => "cargo build / check".into(),
                Some(sub) => format!("cargo {sub}"),
                None => "cargo".into(),
            },
            "mise" | "pnpm" | "npm" | "yarn" | "bun" | "go" | "make" | "just" | "npx" | "uv"
            | "docker" | "deno" => match (args.next(), args.next()) {
                (Some("run"), Some(task)) => format!("{prog} run {task}"),
                (Some(sub), _) => format!("{prog} {sub}"),
                (None, _) => prog.into(),
            },
            _ => prog.into(),
        };
    }
    "shell".into()
}

/// `/bin/zsh -lc 'cargo test'` → `cargo test`.
fn unwrap_shell(cmd: &str) -> &str {
    let mut parts = cmd.splitn(3, ' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(sh), Some("-c" | "-lc" | "-ic"), Some(rest))
            if matches!(sh.rsplit('/').next(), Some("sh" | "bash" | "zsh")) =>
        {
            let rest = rest.trim();
            match rest.chars().next() {
                Some(q @ ('\'' | '"')) if rest.len() > 1 && rest.ends_with(q) => {
                    &rest[1..rest.len() - 1]
                }
                _ => rest,
            }
        }
        _ => cmd,
    }
}

/// A search that ran fine but matched nothing (not a failure). Covers
/// Claude's Grep/Glob text, rg's bare exit 1, and Cursor's empty glob.
// simplify: Cursor's grep payload has no reliable empty marker, so its
// no-match searches count as successes until one is found.
pub(crate) fn no_match(label: &str, result: &str, is_error: bool) -> bool {
    if label != SEARCH {
        return false;
    }
    let r = result.trim();
    (is_error && (r.is_empty() || r == "Exit code 1"))
        || r.starts_with("No matches found")
        || r.starts_with("No files found")
        || r.contains("\"totalFiles\":0")
}

pub(crate) fn build_lock(result: &str) -> bool {
    result.contains("Blocking waiting for file lock")
}

/// A UUIDv7 stamped at `ms`, for rows backfilled from older data.
fn uuid7_at(ms: i64) -> String {
    let ms = ms.max(0) as u64;
    let ts =
        uuid::Timestamp::from_unix(uuid::NoContext, ms / 1000, ((ms % 1000) * 1_000_000) as u32);
    uuid::Uuid::new_v7(ts).to_string()
}

/// SQL functions the migrations backfill with, so labels have one source.
pub(crate) fn register_functions(c: &Connection) -> rusqlite::Result<()> {
    let det = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    let text = |ctx: &rusqlite::functions::Context<'_>, i: usize| -> String {
        ctx.get_raw(i)
            .as_str_or_null()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_owned()
    };
    c.create_scalar_function("recur_tool_label", 2, det, move |ctx| {
        Ok(tool_label(&text(ctx, 0), &text(ctx, 1)))
    })?;
    c.create_scalar_function("recur_tool_no_match", 3, det, move |ctx| {
        let is_error = ctx.get::<Option<i64>>(2)?.unwrap_or(0) != 0;
        Ok(no_match(&text(ctx, 0), &text(ctx, 1), is_error))
    })?;
    c.create_scalar_function("recur_uuid7", 1, FunctionFlags::SQLITE_UTF8, |ctx| {
        Ok(uuid7_at(ctx.get::<Option<i64>>(0)?.unwrap_or(0)))
    })
}

/// Opens a call for a stored tool_use block. Undecodable payloads are skipped:
/// the transcript write must not fail on them.
pub(crate) fn open_tool_call_tx(
    c: &Connection,
    message_id: &str,
    content: &str,
    at: i64,
) -> Result<()> {
    let Ok(tu) = serde_json::from_str::<ToolUsePayload>(content) else {
        return Ok(());
    };
    if tu.id.is_empty() {
        return Ok(());
    }
    c.execute(
        "INSERT OR IGNORE INTO tool_call
		(id, tool_use_id, session_id, message_id, name, label, started_at)
		SELECT ?, ?, origin_session_id, id, ?, ?, ? FROM message WHERE id = ?",
        params![
            uuid::Uuid::now_v7().to_string(),
            tu.id,
            tu.name,
            tool_label(&tu.name, &tu.input),
            at,
            message_id
        ],
    )
    .ctx(|| format!("open tool call {:?}", tu.id))?;
    Ok(())
}

/// Closes the call a provider's tool_result answers (first result wins).
/// Startup recovery's provider-less carrier leaves it open: the call's end
/// is unknown, not the relaunch.
pub(crate) fn close_tool_call_tx(
    c: &Connection,
    message_id: &str,
    content: &str,
    at: i64,
) -> Result<()> {
    let Ok(r) = serde_json::from_str::<ToolResultPayload>(content) else {
        return Ok(());
    };
    let label: Option<String> = c
        .query_row(
            "SELECT label FROM tool_call WHERE tool_use_id = ? AND ended_at IS NULL
				AND EXISTS (SELECT 1 FROM message WHERE id = ? AND provider IS NOT NULL)",
            [&r.tool_use_id, message_id],
            |row| row.get(0),
        )
        .optional()
        .ctx(|| format!("lookup tool call {:?}", r.tool_use_id))?;
    let Some(label) = label else {
        return Ok(());
    };
    let nm = no_match(&label, &r.result, r.is_error);
    c.execute(
        "UPDATE tool_call SET ended_at = ?, is_error = ?, no_match = ?, build_lock = ?
		WHERE tool_use_id = ?",
        params![
            at,
            r.is_error && !nm,
            nm,
            build_lock(&r.result),
            r.tool_use_id
        ],
    )
    .ctx(|| format!("close tool call {:?}", r.tool_use_id))?;
    Ok(())
}

/// Relabels a call whose arguments arrived late.
pub(crate) fn relabel_tool_call_tx(c: &Connection, tu: &ToolUsePayload) -> Result<()> {
    c.execute(
        "UPDATE tool_call SET label = ? WHERE tool_use_id = ?",
        params![tool_label(&tu.name, &tu.input), tu.id],
    )
    .ctx(|| format!("relabel tool call {:?}", tu.id))?;
    Ok(())
}

/// message_content rows per backfill transaction.
const BACKFILL_WINDOW: i64 = 1_000;

impl Store {
    /// Fills `tool_call` from transcript rows written before migration 0019,
    /// one window per hold of the store's connection, so app writes interleave
    /// with it. Each window commits with the cursor, so the job resumes after a
    /// restart; it returns once the cursor row is gone or the store closes.
    /// Blocks: run it on its own thread.
    pub fn backfill_tool_calls(&self) -> Result<()> {
        match &self.db {
            Some(db) => backfill_tool_calls(db, BACKFILL_WINDOW),
            None => Ok(()),
        }
    }
}

/// How far the backfill is (0 to 1), or `None` once it is done.
pub(crate) fn backfill_progress(c: &Connection) -> Result<Option<f32>> {
    c.query_row(
        "SELECT next_rowid, end_rowid FROM tool_call_backfill",
        [],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )
    .optional()
    .map(|p| p.map(|(next, end)| (next as f32 / end.max(1) as f32).min(1.)))
    .ctx(|| "read tool call backfill".into())
}

pub(crate) fn backfill_tool_calls(db: &Db, window: i64) -> Result<()> {
    loop {
        let t = Instant::now();
        match db.with(|c| backfill_window(c, window)) {
            Ok(true) => {}
            Ok(false) | Err(Error::Closed) => return Ok(()),
            Err(e) => return Err(e),
        }
        // As long off the connection as on it, so app queries are not starved.
        std::thread::sleep(t.elapsed());
    }
}

/// Backfills one window; false once there is nothing left.
fn backfill_window(c: &mut Connection, window: i64) -> Result<bool> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .ctx(|| "begin backfill".into())?;
    let Some((lo, end)) = tx
        .query_row(
            "SELECT next_rowid, end_rowid FROM tool_call_backfill",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()
        .ctx(|| "read tool call backfill".into())?
    else {
        return Ok(false);
    };
    let hi = lo + window;
    tx.execute(
        "INSERT OR IGNORE INTO tool_call (id, tool_use_id, session_id, message_id, name, label, started_at)
			SELECT recur_uuid7(u.created_at), u.tool_use_id, m.origin_session_id, u.message_id, u.name,
				recur_tool_label(u.name, u.input), u.created_at
			FROM (
				SELECT message_id, created_at,
					json_extract(content_json, '$.id') AS tool_use_id,
					COALESCE(json_extract(content_json, '$.name'), '') AS name,
					json_extract(content_json, '$.input') AS input
				FROM message_content WHERE rowid >= ?1 AND rowid < ?2 AND type = 'tool_use'
			) u
			JOIN message m ON m.id = u.message_id
			WHERE COALESCE(u.tool_use_id, '') != ''
			ORDER BY u.created_at",
        params![lo, hi],
    )
    .ctx(|| format!("backfill tool calls from rowid {lo}"))?;
    // The earliest result in the window closes a call no earlier window
    // closed (MIN's bare columns come from its row).
    tx.execute(
        "UPDATE tool_call SET
				ended_at = r.created_at,
				is_error = r.is_error AND NOT r.no_match,
				no_match = r.no_match,
				build_lock = r.build_lock
			FROM (
				SELECT r.tool_use_id, r.created_at, r.is_error,
					recur_tool_no_match(t.label, r.result, r.is_error) AS no_match,
					instr(r.result, 'Blocking waiting for file lock') > 0 AS build_lock
				FROM (
					SELECT json_extract(mc.content_json, '$.tool_use_id') AS tool_use_id,
						MIN(mc.created_at) AS created_at,
						COALESCE(json_extract(mc.content_json, '$.is_error'), 0) != 0 AS is_error,
						COALESCE(json_extract(mc.content_json, '$.result'), '') AS result
					FROM message_content mc JOIN message m ON m.id = mc.message_id
					WHERE mc.rowid >= ?1 AND mc.rowid < ?2 AND mc.type = 'tool_result'
						AND m.provider IS NOT NULL
					GROUP BY 1
				) r
				JOIN tool_call t ON t.tool_use_id = r.tool_use_id AND t.ended_at IS NULL
			) r
			WHERE tool_call.tool_use_id = r.tool_use_id",
        params![lo, hi],
    )
    .ctx(|| format!("backfill tool results from rowid {lo}"))?;
    if hi > end {
        tx.execute("DELETE FROM tool_call_backfill", [])
    } else {
        tx.execute("UPDATE tool_call_backfill SET next_rowid = ?", [hi])
    }
    .ctx(|| "advance tool call backfill".into())?;
    tx.commit().ctx(|| "commit backfill".into())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        let bash = |c: &str| tool_label("Bash", &serde_json::json!({ "command": c }).to_string());
        assert_eq!(bash("cargo test -p x"), "cargo test");
        assert_eq!(bash("cd x && cargo build"), "cargo build / check");
        assert_eq!(
            bash("cd /a; RUST_LOG=1 cargo nextest run 2>&1 | tail"),
            "cargo test"
        );
        assert_eq!(bash("cargo +nightly -q clippy"), "cargo clippy");
        assert_eq!(bash("mise run test:bundles"), "mise run test:bundles");
        assert_eq!(bash("pnpm install"), "pnpm install");
        assert_eq!(bash("/bin/zsh -lc 'git status --short'"), "git, gh");
        assert_eq!(bash("gh pr view 3"), "git, gh");
        assert_eq!(bash("rg -n foo src | head"), SEARCH);
        assert_eq!(bash("sed -n 1,20p a.rs"), FILES);
        assert_eq!(bash("python3 - <<'EOF'\nprint(1)\nEOF"), "python3");
        assert_eq!(bash("./scripts/check.sh > /tmp/x 2>&1"), "check.sh");
        assert_eq!(bash(""), "shell");
        assert_eq!(bash("for f in a b; do sed -n 1p $f; done"), FILES);
        assert_eq!(bash("timeout 600 cargo test 2>&1 | tail"), "cargo test");
        assert_eq!(
            bash("until grep -q x log; do sleep 20; done"),
            "Wait loops (until, while)"
        );
        assert_eq!(
            tool_label("shell", r#"{"command":["bash","-lc","cargo test"]}"#),
            "cargo test"
        );
        assert_eq!(
            tool_label("shell", r#"{"command":"go test ./..."}"#),
            "go test"
        );
        assert_eq!(tool_label("Read", r#"{"file_path":"a"}"#), FILES);
        assert_eq!(tool_label("edit", "{}"), FILES);
        assert_eq!(tool_label("grep", "{}"), SEARCH);
        assert_eq!(tool_label("Glob", "{}"), SEARCH);
        assert_eq!(
            tool_label(
                "mcp",
                r#"{"providerIdentifier":"recur","toolName":"ask_user"}"#
            ),
            "mcp__recur__ask_user"
        );
        assert_eq!(tool_label("WebFetch", "{}"), "WebFetch");
    }

    #[test]
    fn outcomes() {
        assert!(no_match(SEARCH, "No matches found", false));
        assert!(no_match(SEARCH, "", true));
        assert!(no_match(SEARCH, "Exit code 1", true));
        assert!(!no_match(SEARCH, "a.rs:1: hit", false));
        assert!(!no_match("cargo test", "", true));
        assert!(build_lock(
            "   Blocking waiting for file lock on build directory"
        ));
        assert!(uuid7_at(1_700_000_000_000).starts_with("018bcfe5-6800-7"));
    }
}
