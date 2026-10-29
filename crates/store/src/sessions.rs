//! Sessions: the v2 transcript store. Unlike the v1 aggregates they are not
//! mirrored in memory (a transcript can be large and long-lived); they live
//! only in SQLite and are queried on demand. `session_mu` serializes the
//! count-then-insert idx allocations so two writers to one session cannot
//! collide on an idx; reads take neither lock.

use std::collections::BTreeMap;

use recur_domain::{
    BLOCK_FALLBACK, BLOCK_IMAGE, BLOCK_TEXT, BLOCK_THINKING, BLOCK_TOOL_RESULT, BLOCK_TOOL_USE,
    CONTEXT_MODE_PROVIDER, ContentBlock, FallbackPayload, Message, STREAM_COMPLETE,
    STREAM_STREAMING, Session, SessionContext, TextPayload, ToolUsePayload,
};
use rusqlite::{Connection, OptionalExtension as _, Row, params};
use serde::Deserialize;
use serde_json::to_string as marshal;

use crate::db::{Db, upsert_doc_tx};
use crate::error::{Error, Result, SqlContext as _, json_err};
use crate::store::Store;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Empty and zero become SQL NULL so optional columns stay NULL.
fn null_str(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

fn null_int(n: i64) -> Option<i64> {
    (n != 0).then_some(n)
}

fn requires_db() -> Error {
    Error::msg("session store requires a database")
}

/// The subset of session config_json used to match a chat and the
/// specialist sessions it owns.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ChatCfg {
    #[serde(deserialize_with = "nullable")]
    code: String,
    #[serde(rename = "ownerCode", deserialize_with = "nullable")]
    owner_code: String,
}

fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

fn chat_cfg(config_json: &str) -> ChatCfg {
    serde_json::from_str(config_json).unwrap_or_default()
}

const SESSION_COLS: &str = "id, workspace_id, kind, parent_session_id, fork_message_id,
	context_mode, provider, provider_session_ref, config_json, active_context_id,
	status, created_at, updated_at";

fn scan_session(r: &Row<'_>) -> rusqlite::Result<Session> {
    let opt = |i: usize| -> rusqlite::Result<String> {
        Ok(r.get::<_, Option<String>>(i)?.unwrap_or_default())
    };
    Ok(Session {
        id: r.get(0)?,
        workspace_id: opt(1)?,
        kind: r.get(2)?,
        parent_session_id: opt(3)?,
        fork_message_id: opt(4)?,
        context_mode: r.get(5)?,
        provider: r.get(6)?,
        provider_session_ref: opt(7)?,
        config_json: r.get(8)?,
        active_context_id: opt(9)?,
        status: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

fn session_row(c: &Connection, q: &str, p: impl rusqlite::Params) -> Result<Option<Session>> {
    c.query_row(q, p, scan_session).optional().bare()
}

fn session_list(
    c: &Connection,
    q: &str,
    p: impl rusqlite::Params,
    ctx: impl Fn() -> String,
) -> Result<Vec<Session>> {
    let mut stmt = c.prepare(q).ctx(&ctx)?;
    let rows = stmt.query_map(p, scan_session).ctx(&ctx)?;
    let mut out = Vec::new();
    for s in rows {
        out.push(s.bare()?);
    }
    Ok(out)
}

fn sessions_for_loop_query(c: &Connection, workspace_id: &str) -> Result<Vec<Session>> {
    session_list(
        c,
        &format!(
            "SELECT {SESSION_COLS} FROM session WHERE workspace_id = ? ORDER BY created_at, id"
        ),
        [workspace_id],
        || format!("query sessions for loop {workspace_id:?}"),
    )
}

pub(crate) fn sessions_for_app_kind(db: &Db, kind: &str) -> Result<Vec<Session>> {
    db.with(|c| {
        session_list(
            c,
            &format!(
                "SELECT {SESSION_COLS} FROM session
		WHERE workspace_id IS NULL AND kind = ? ORDER BY created_at ASC, id ASC"
            ),
            [kind],
            || "query app sessions".into(),
        )
    })
}

fn insert_session_tx(c: &Connection, s: &Session) -> Result<()> {
    c.execute(
        "INSERT INTO session
		(id, workspace_id, kind, parent_session_id, fork_message_id,
		 context_mode, provider, provider_session_ref, config_json,
		 active_context_id, status, created_at, updated_at)
		VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            s.id,
            null_str(&s.workspace_id),
            s.kind,
            null_str(&s.parent_session_id),
            null_str(&s.fork_message_id),
            s.context_mode,
            s.provider,
            null_str(&s.provider_session_ref),
            s.config_json,
            null_str(&s.active_context_id),
            s.status,
            s.created_at,
            s.updated_at
        ],
    )
    .ctx(|| format!("insert session {:?}", s.id))?;
    Ok(())
}

/// Messages only this session still joins (message_content and
/// session_message rows cascade), then the session (leftover join rows for
/// forks and session_context cascade).
fn delete_session_tx(c: &Connection, session_id: &str) -> Result<()> {
    c.execute(
        "
		DELETE FROM message
		 WHERE id IN (SELECT message_id FROM session_message WHERE session_id = ?)
		   AND NOT EXISTS (
		     SELECT 1 FROM session_message sm
		      WHERE sm.message_id = message.id AND sm.session_id != ?
		   )",
        params![session_id, session_id],
    )
    .ctx(|| format!("delete session messages {session_id}"))?;
    c.execute("DELETE FROM session WHERE id = ?", [session_id])
        .ctx(|| format!("delete session {session_id}"))?;
    Ok(())
}

fn session_has_messages_query(c: &Connection, session_id: &str) -> bool {
    c.query_row(
        "SELECT COUNT(*) FROM session_message WHERE session_id = ?",
        [session_id],
        |r| r.get::<_, i64>(0),
    )
    .is_ok_and(|n| n > 0)
}

fn next_session_idx_tx(c: &Connection, session_id: &str) -> Result<i64> {
    let idx: Option<i64> = c
        .query_row(
            "SELECT MAX(idx) FROM session_message WHERE session_id = ?",
            [session_id],
            |r| r.get(0),
        )
        .ctx(|| "next session idx".into())?;
    Ok(idx.map_or(0, |i| i + 1))
}

fn insert_message_tx(c: &Connection, m: &Message) -> Result<()> {
    c.execute(
        "INSERT INTO message
		(id, origin_session_id, role, model_id, provider, stream_status,
		 started_at, completed_at, input_tokens, output_tokens,
		 cache_creation_tokens, cache_read_tokens, created_at)
		VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            m.id,
            m.origin_session_id,
            m.role,
            null_str(&m.model_id),
            null_str(&m.provider),
            m.stream_status,
            null_int(m.started_at),
            null_int(m.completed_at),
            null_int(m.input_tokens),
            null_int(m.output_tokens),
            null_int(m.cache_creation_tokens),
            null_int(m.cache_read_tokens),
            m.created_at
        ],
    )
    .ctx(|| format!("insert message {:?}", m.id))?;
    Ok(())
}

fn insert_session_message_tx(
    c: &Connection,
    session_id: &str,
    message_id: &str,
    idx: i64,
) -> Result<()> {
    c.execute(
        "INSERT INTO session_message (session_id, message_id, idx) VALUES (?,?,?)",
        params![session_id, message_id, idx],
    )
    .ctx(|| format!("insert session_message {session_id}/{message_id}"))?;
    Ok(())
}

fn insert_content_block_tx(
    c: &Connection,
    message_id: &str,
    idx: i64,
    b: &ContentBlock,
) -> Result<()> {
    let id = if b.id.is_empty() {
        new_id()
    } else {
        b.id.clone()
    };
    let content = if b.content_json.is_empty() {
        "{}"
    } else {
        &b.content_json
    };
    c.execute(
        "INSERT INTO message_content (id, message_id, idx, type, content_json, created_at)
		VALUES (?,?,?,?,?,?)",
        params![id, message_id, idx, b.r#type, content, now_ms()],
    )
    .ctx(|| format!("insert content block {id:?}"))?;
    Ok(())
}

const MESSAGE_COLS: &str = "m.id, m.origin_session_id, m.role, m.model_id, m.provider,
	m.stream_status, m.started_at, m.completed_at, m.input_tokens, m.output_tokens,
	m.cache_creation_tokens, m.cache_read_tokens, m.duration_ms,
	m.context_window, m.created_at";

fn scan_message(r: &Row<'_>) -> rusqlite::Result<Message> {
    let s = |i: usize| -> rusqlite::Result<String> {
        Ok(r.get::<_, Option<String>>(i)?.unwrap_or_default())
    };
    let n =
        |i: usize| -> rusqlite::Result<i64> { Ok(r.get::<_, Option<i64>>(i)?.unwrap_or_default()) };
    Ok(Message {
        id: r.get(0)?,
        origin_session_id: r.get(1)?,
        role: r.get(2)?,
        model_id: s(3)?,
        provider: s(4)?,
        stream_status: r.get(5)?,
        started_at: n(6)?,
        completed_at: n(7)?,
        input_tokens: n(8)?,
        output_tokens: n(9)?,
        cache_creation_tokens: n(10)?,
        cache_read_tokens: n(11)?,
        duration_ms: n(12)?,
        context_window: n(13)?,
        created_at: r.get(14)?,
        content: None,
    })
}

impl Store {
    fn sdb(&self) -> Result<&Db> {
        self.db.as_ref().ok_or_else(requires_db)
    }

    /// Inserts a new session, filling id (UUIDv7), timestamps, and default
    /// status (`idle`), context mode (`provider`) and config (`{}`).
    pub fn create_session(&self, mut sess: Session) -> Result<Session> {
        let db = self.sdb()?;
        if sess.id.is_empty() {
            sess.id = new_id();
        }
        let now = now_ms();
        if sess.created_at == 0 {
            sess.created_at = now;
        }
        sess.updated_at = now;
        if sess.status.is_empty() {
            sess.status = "idle".into();
        }
        if sess.context_mode.is_empty() {
            sess.context_mode = CONTEXT_MODE_PROVIDER.into();
        }
        if sess.config_json.is_empty() {
            sess.config_json = "{}".into();
        }
        db.tx(|tx| insert_session_tx(tx, &sess))?;
        Ok(sess)
    }

    /// A session header (no transcript).
    pub fn get_session(&self, id: &str) -> Option<Session> {
        let db = self.db.as_ref()?;
        db.with(|c| {
            session_row(
                c,
                &format!("SELECT {SESSION_COLS} FROM session WHERE id = ?"),
                [id],
            )
        })
        .ok()
        .flatten()
    }

    /// The newest session of a kind bound to a workspace.
    pub fn session_for_workspace_kind(&self, workspace_id: &str, kind: &str) -> Option<Session> {
        let db = self.db.as_ref()?;
        db.with(|c| {
            session_row(
                c,
                &format!(
                    "SELECT {SESSION_COLS} FROM session
		WHERE workspace_id = ? AND kind = ? ORDER BY created_at DESC LIMIT 1"
                ),
                [workspace_id, kind],
            )
        })
        .ok()
        .flatten()
    }

    /// The newest session of a kind with a NULL workspace (the persistent
    /// Ask Recur coordinator).
    pub fn session_for_app_kind(&self, kind: &str) -> Option<Session> {
        let db = self.db.as_ref().filter(|_| !kind.is_empty())?;
        db.with(|c| {
            session_row(
                c,
                &format!(
                    "SELECT {SESSION_COLS} FROM session
		WHERE workspace_id IS NULL AND kind = ? ORDER BY created_at DESC LIMIT 1"
                ),
                [kind],
            )
        })
        .ok()
        .flatten()
    }

    /// Every app-scoped session of a kind, oldest first (the Ask Recur tab
    /// strip). `None` when there are none (Go's nil).
    pub fn sessions_for_app_kind(&self, kind: &str) -> Option<Vec<Session>> {
        let db = self.db.as_ref().filter(|_| !kind.is_empty())?;
        sessions_for_app_kind(db, kind)
            .ok()
            .filter(|l| !l.is_empty())
    }

    /// Session headers grouped by workspace, newest-updated first, at most 8
    /// per workspace. `None` without a database or on error.
    pub fn session_headers_by_workspace(&self) -> Option<BTreeMap<String, Vec<Session>>> {
        let db = self.db.as_ref()?;
        let cap = 8;
        let list = db
            .with(|c| {
                session_list(
                    c,
                    &format!(
                        "SELECT {SESSION_COLS} FROM session
		WHERE workspace_id IS NOT NULL AND workspace_id != ''
		ORDER BY workspace_id, updated_at DESC, id DESC"
                    ),
                    [],
                    || "query session headers".into(),
                )
            })
            .ok()?;
        let mut out: BTreeMap<String, Vec<Session>> = BTreeMap::new();
        for s in list {
            let l = out.entry(s.workspace_id.clone()).or_default();
            if l.len() < cap {
                l.push(s);
            }
        }
        Some(out)
    }

    /// MAX(events.ts) per workspace.
    pub fn latest_event_ts_by_workspace(&self) -> Option<BTreeMap<String, i64>> {
        let db = self.db.as_ref()?;
        db.with(|c| {
            let ctx = || "query latest event ts".to_owned();
            let mut stmt = c
                .prepare(
                    "SELECT workspace_id, MAX(ts) FROM events
		WHERE workspace_id IS NOT NULL AND workspace_id != '' GROUP BY workspace_id",
                )
                .ctx(ctx)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .ctx(ctx)?;
            let mut out = BTreeMap::new();
            for row in rows {
                let (k, v) = row.bare()?;
                out.insert(k, v);
            }
            Ok(out)
        })
        .ok()
    }

    /// App-scoped sessions left `streaming` after a crash (Recover idles
    /// them), newest first. `None` when none.
    pub fn streaming_app_sessions(&self) -> Option<Vec<Session>> {
        let db = self.db.as_ref()?;
        db.with(|c| {
            session_list(
                c,
                &format!(
                    "SELECT {SESSION_COLS} FROM session
		WHERE workspace_id IS NULL AND status = 'streaming' ORDER BY updated_at DESC"
                ),
                [],
                || "query streaming app sessions".into(),
            )
        })
        .ok()
        .filter(|l| !l.is_empty())
    }

    /// Moves every session from one workspace onto another so PR chat-host
    /// transcripts survive linking a real worktree. remap rewrites
    /// config_json.code (and matching ownerCode) so two transcripts that
    /// shared a tab code stay distinct. Empty dest sessions whose code is
    /// kept are dropped so the hoisted row wins.
    pub fn reassign_workspace_sessions(
        &self,
        from_id: &str,
        to_id: &str,
        remap: &BTreeMap<String, String>,
    ) -> Result<()> {
        let from_id = from_id.trim();
        let to_id = to_id.trim();
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return Ok(());
        }
        let Some(db) = &self.db else {
            return Ok(());
        };
        let _g = self.session_mu.lock();
        let ctx = || format!("reassign sessions {from_id:?} → {to_id:?}");
        db.with(|c| {
            let tx = c.transaction().ctx(ctx)?;
            let from = sessions_for_loop_query(&tx, from_id)?;
            let dest = sessions_for_loop_query(&tx, to_id)?;
            let from_codes: std::collections::BTreeSet<String> = from
                .iter()
                .map(|s| chat_cfg(&s.config_json).code)
                .filter(|c| !c.is_empty())
                .collect();
            let remapped = |code: &str| remap.get(code).filter(|n| !n.is_empty()).cloned();
            for s in &dest {
                let cfg = chat_cfg(&s.config_json);
                if cfg.code.is_empty()
                    || !from_codes.contains(&cfg.code)
                    || remapped(&cfg.code).is_some()
                {
                    continue;
                }
                if session_has_messages_query(&tx, &s.id) {
                    continue;
                }
                delete_session_tx(&tx, &s.id)?;
            }
            for s in &from {
                let cfg = chat_cfg(&s.config_json);
                let new_code = remapped(&cfg.code).unwrap_or_else(|| cfg.code.clone());
                let new_owner = remapped(&cfg.owner_code).unwrap_or_else(|| cfg.owner_code.clone());
                let mut q = String::from("UPDATE session SET workspace_id = ?");
                let mut args: Vec<String> = vec![to_id.to_owned()];
                if new_code != cfg.code || new_owner != cfg.owner_code {
                    let mut expr = String::from("json_set(COALESCE(NULLIF(config_json, ''), '{}')");
                    if new_code != cfg.code {
                        expr.push_str(", '$.code', ?");
                        args.push(new_code.clone());
                    }
                    if new_owner != cfg.owner_code {
                        expr.push_str(", '$.ownerCode', ?");
                        args.push(new_owner.clone());
                    }
                    expr.push(')');
                    q.push_str(", config_json = ");
                    q.push_str(&expr);
                }
                q.push_str(" WHERE id = ?");
                args.push(s.id.clone());
                tx.execute(&q, rusqlite::params_from_iter(args.iter()))
                    .ctx(ctx)?;
            }
            tx.commit().ctx(ctx)
        })
    }

    /// The session for a (workspace, agent code); the code lives in
    /// config_json.code.
    pub fn session_for_loop_agent(&self, workspace_id: &str, code: &str) -> Option<Session> {
        let db = self.db.as_ref().filter(|_| !code.is_empty())?;
        db.with(|c| {
            session_row(
                c,
                &format!(
                    "SELECT {SESSION_COLS} FROM session
		WHERE workspace_id = ? AND json_extract(config_json, '$.code') = ?
		ORDER BY created_at DESC LIMIT 1"
                ),
                [workspace_id, code],
            )
        })
        .ok()
        .flatten()
    }

    /// The specialist session for this conductor tab.
    pub fn session_for_chat_agent(
        &self,
        workspace_id: &str,
        owner: &str,
        code: &str,
    ) -> Option<Session> {
        let db = self.db.as_ref().filter(|_| !code.is_empty())?;
        db.with(|c| {
            session_row(
                c,
                &format!(
                    "SELECT {SESSION_COLS} FROM session
		WHERE workspace_id = ? AND json_extract(config_json, '$.code') = ?
		AND ifnull(json_extract(config_json, '$.ownerCode'), '') = ?
		ORDER BY created_at DESC LIMIT 1"
                ),
                [workspace_id, code, owner],
            )
        })
        .ok()
        .flatten()
    }

    /// Every session bound to a workspace, oldest created first. `None`
    /// when none.
    pub fn sessions_for_loop(&self, workspace_id: &str) -> Option<Vec<Session>> {
        let db = self.db.as_ref()?;
        db.with(|c| sessions_for_loop_query(c, workspace_id))
            .ok()
            .filter(|l| !l.is_empty())
    }

    fn exec_update(
        &self,
        q: &str,
        p: impl rusqlite::Params,
        ctx: impl FnOnce() -> String,
    ) -> Result<()> {
        match &self.db {
            None => Ok(()),
            Some(db) => db.with(|c| {
                c.execute(q, p).ctx(ctx)?;
                Ok(())
            }),
        }
    }

    /// Records the provider session ref and touches updated_at. No-op for
    /// an empty ref.
    pub fn update_session_ref(&self, id: &str, r#ref: &str) -> Result<()> {
        if r#ref.is_empty() {
            return Ok(());
        }
        self.exec_update(
            "UPDATE session SET provider_session_ref = ?, updated_at = ? WHERE id = ?",
            params![r#ref, now_ms(), id],
            || format!("update session ref {id:?}"),
        )
    }

    /// Records which provider last ran (or is selected for) the session.
    pub fn update_session_provider(&self, id: &str, provider: &str) -> Result<()> {
        if id.is_empty() || provider.trim().is_empty() {
            return Ok(());
        }
        self.exec_update(
            "UPDATE session SET provider = ?, updated_at = ? WHERE id = ?",
            params![provider, now_ms(), id],
            || format!("update session provider {id:?}"),
        )
    }

    /// idle | streaming | error.
    pub fn set_session_status(&self, id: &str, status: &str) -> Result<()> {
        self.exec_update(
            "UPDATE session SET status = ?, updated_at = ? WHERE id = ?",
            params![status, now_ms(), id],
            || format!("set session status {id:?}"),
        )
    }

    /// Rewrites config_json (empty becomes `{}`) and touches updated_at.
    pub fn update_session_config_json(&self, id: &str, config_json: &str) -> Result<()> {
        if id.is_empty() {
            return Ok(());
        }
        let cfg = if config_json.is_empty() {
            "{}"
        } else {
            config_json
        };
        self.exec_update(
            "UPDATE session SET config_json = ?, updated_at = ? WHERE id = ?",
            params![cfg, now_ms(), id],
            || format!("update session config {id:?}"),
        )
    }

    /// Inserts a compaction watermark and points active_context_id at it.
    /// Full history is untouched.
    pub fn write_session_context(
        &self,
        session_id: &str,
        summary: &str,
        last_compacted_message_id: &str,
    ) -> Result<SessionContext> {
        let db = self.sdb()?;
        let summary = summary.trim();
        if session_id.is_empty() || summary.is_empty() || last_compacted_message_id.is_empty() {
            return Err(Error::msg(
                "session context: session, summary, and watermark are required",
            ));
        }
        let _g = self.session_mu.lock();
        let now = now_ms();
        let ctx = SessionContext {
            id: new_id(),
            session_id: session_id.to_owned(),
            summary: summary.to_owned(),
            last_compacted_message_id: last_compacted_message_id.to_owned(),
            created_at: now,
        };
        db.tx(|tx| {
            tx.execute(
                "INSERT INTO session_context (id, session_id, summary, last_compacted_message_id, created_at)
		 VALUES (?,?,?,?,?)",
                params![ctx.id, ctx.session_id, ctx.summary, ctx.last_compacted_message_id, ctx.created_at],
            )
            .ctx(|| "insert session_context".into())?;
            tx.execute(
                "UPDATE session SET active_context_id = ?, updated_at = ? WHERE id = ?",
                params![ctx.id, now, session_id],
            )
            .ctx(|| "set active_context_id".into())?;
            Ok(())
        })?;
        Ok(ctx)
    }

    /// The compaction row active_context_id points at.
    pub fn active_session_context(&self, session_id: &str) -> Option<SessionContext> {
        let db = self.db.as_ref().filter(|_| !session_id.is_empty())?;
        db.with(|c| {
            c.query_row(
                "SELECT c.id, c.session_id, c.summary, c.last_compacted_message_id, c.created_at
		 FROM session s
		 JOIN session_context c ON c.id = s.active_context_id
		 WHERE s.id = ?",
                [session_id],
                |r| {
                    Ok(SessionContext {
                        id: r.get(0)?,
                        session_id: r.get(1)?,
                        summary: r.get(2)?,
                        last_compacted_message_id: r.get(3)?,
                        created_at: r.get(4)?,
                    })
                },
            )
            .optional()
            .bare()
        })
        .ok()
        .flatten()
    }

    /// Sets config_json.title without replacing the rest of the object.
    pub fn patch_session_config_title(&self, id: &str, title: &str) -> Result<()> {
        if id.is_empty() {
            return Ok(());
        }
        self.exec_update(
            "UPDATE session SET config_json = json_set(COALESCE(NULLIF(config_json, ''), '{}'), '$.title', ?), updated_at = ? WHERE id = ?",
            params![title, now_ms(), id],
            || format!("patch session title {id:?}"),
        )
    }

    /// Sets config_json.simplify without replacing the rest of the object.
    pub fn patch_session_config_simplify(&self, id: &str, on: bool) -> Result<()> {
        if id.is_empty() {
            return Ok(());
        }
        self.exec_update(
            "UPDATE session SET config_json = json_set(COALESCE(NULLIF(config_json, ''), '{}'), '$.simplify', iif(?, json('true'), json('false'))), updated_at = ? WHERE id = ?",
            params![on, now_ms(), id],
            || format!("patch session simplify {id:?}"),
        )
    }

    /// Freezes simplify only while the key is still missing, so a concurrent
    /// explicit choice wins.
    pub fn patch_session_config_simplify_if_unset(&self, id: &str, on: bool) -> Result<()> {
        if id.is_empty() {
            return Ok(());
        }
        self.exec_update(
            "UPDATE session SET config_json = json_set(COALESCE(NULLIF(config_json, ''), '{}'), '$.simplify', iif(?, json('true'), json('false'))), updated_at = ?
		 WHERE id = ? AND json_extract(COALESCE(NULLIF(config_json, ''), '{}'), '$.simplify') IS NULL",
            params![on, now_ms(), id],
            || format!("patch session simplify if unset {id:?}"),
        )
    }

    /// Merges a JSON object into config_json (RFC 7396, via json_patch)
    /// without deleting keys the patch omits.
    pub fn patch_session_config_merge(&self, id: &str, patch_json: &str) -> Result<()> {
        if id.is_empty() || patch_json.trim().is_empty() {
            return Ok(());
        }
        self.patch_merge(id, patch_json)
    }

    fn patch_merge(&self, id: &str, patch_json: &str) -> Result<()> {
        self.exec_update(
            "UPDATE session SET config_json = json_patch(COALESCE(NULLIF(config_json, ''), '{}'), json(?)), updated_at = ? WHERE id = ?",
            params![patch_json, now_ms(), id],
            || format!("patch session config merge {id:?}"),
        )
    }

    /// json_patch the dock fields, then replace flags (json_set) in the same
    /// UPDATE. Empty flags_json leaves flags untouched.
    pub fn patch_session_config_dock(
        &self,
        id: &str,
        patch_json: &str,
        flags_json: &str,
    ) -> Result<()> {
        if id.is_empty() || self.db.is_none() {
            return Ok(());
        }
        let (patch, flags) = (patch_json.trim(), flags_json.trim());
        if patch.is_empty() && flags.is_empty() {
            return Ok(());
        }
        let patch = if patch.is_empty() { "{}" } else { patch };
        if flags.is_empty() {
            return self.patch_merge(id, patch);
        }
        self.exec_update(
            "UPDATE session SET config_json = json_set(json_patch(COALESCE(NULLIF(config_json, ''), '{}'), json(?)), '$.flags', json(?)), updated_at = ? WHERE id = ?",
            params![patch, flags, now_ms(), id],
            || format!("patch session dock {id:?}"),
        )
    }

    /// Replaces config_json.flags (json_set, not a merge) so a smaller map
    /// does not keep stale keys. Empty becomes `{}`.
    pub fn patch_session_config_flags(&self, id: &str, flags_json: &str) -> Result<()> {
        if id.is_empty() {
            return Ok(());
        }
        let flags = if flags_json.trim().is_empty() {
            "{}"
        } else {
            flags_json
        };
        self.exec_update(
            "UPDATE session SET config_json = json_set(COALESCE(NULLIF(config_json, ''), '{}'), '$.flags', json(?)), updated_at = ? WHERE id = ?",
            params![flags, now_ms(), id],
            || format!("patch session flags {id:?}"),
        )
    }

    /// Inserts a full message (with blocks) at the end of a session in one
    /// transaction, assigning ids and the session-relative idx. Returns the
    /// stored message.
    pub fn append_message(&self, session_id: &str, mut m: Message) -> Result<Message> {
        let db = self.sdb()?;
        let _g = self.session_mu.lock();
        if m.id.is_empty() {
            m.id = new_id();
        }
        if m.origin_session_id.is_empty() {
            m.origin_session_id = session_id.to_owned();
        }
        if m.created_at == 0 {
            m.created_at = now_ms();
        }
        if m.stream_status.is_empty() {
            m.stream_status = STREAM_COMPLETE.into();
        }
        if let Some(blocks) = &mut m.content {
            for b in blocks {
                if b.id.is_empty() {
                    b.id = new_id();
                }
            }
        }
        db.tx(|tx| {
            let idx = next_session_idx_tx(tx, session_id)?;
            insert_message_tx(tx, &m)?;
            insert_session_message_tx(tx, session_id, &m.id, idx)?;
            for (bi, b) in m.blocks().iter().enumerate() {
                insert_content_block_tx(tx, &m.id, bi as i64, b)?;
            }
            Ok(())
        })?;
        Ok(m)
    }

    /// Inserts an empty message at the end of a session and returns its id:
    /// the streaming entry point. Blocks follow with
    /// [`Store::append_content_block`], then [`Store::complete_message`].
    pub fn create_message(&self, session_id: &str, mut m: Message) -> Result<String> {
        let db = self.sdb()?;
        let _g = self.session_mu.lock();
        if m.id.is_empty() {
            m.id = new_id();
        }
        if m.origin_session_id.is_empty() {
            m.origin_session_id = session_id.to_owned();
        }
        if m.created_at == 0 {
            m.created_at = now_ms();
        }
        if m.stream_status.is_empty() {
            m.stream_status = STREAM_STREAMING.into();
        }
        db.tx(|tx| {
            let idx = next_session_idx_tx(tx, session_id)?;
            insert_message_tx(tx, &m)?;
            insert_session_message_tx(tx, session_id, &m.id, idx)
        })?;
        Ok(m.id)
    }

    /// Unlinks and deletes messages from a session (refusal fallback
    /// retraction). Idx gaps are left as-is.
    pub fn drop_session_messages(&self, session_id: &str, message_ids: &[String]) -> Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        if session_id.is_empty() || message_ids.is_empty() {
            return Ok(());
        }
        let _g = self.session_mu.lock();
        db.tx(|tx| {
            for id in message_ids {
                let id = id.trim();
                if id.is_empty() {
                    continue;
                }
                tx.execute(
                    "DELETE FROM session_message WHERE session_id = ? AND message_id = ?",
                    [session_id, id],
                )
                .ctx(|| format!("drop session_message {session_id}/{id}"))?;
                tx.execute("DELETE FROM message_content WHERE message_id = ?", [id])
                    .ctx(|| format!("drop message_content {id}"))?;
                tx.execute("DELETE FROM message WHERE id = ?", [id])
                    .ctx(|| format!("drop message {id}"))?;
            }
            Ok(())
        })
    }

    /// Drops a session and the messages no other session (a fork) still
    /// references; session_context rows cascade.
    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        if session_id.is_empty() {
            return Ok(());
        }
        let _g = self.session_mu.lock();
        db.tx(|tx| delete_session_tx(tx, session_id))
    }

    /// Permanently drops the pair/MA session whose config code matches, plus
    /// the specialist sessions that name it as ownerCode, and strips the code
    /// from ChatTabOrder in the same transaction. Messages still referenced
    /// by a fork are kept.
    pub fn delete_closed_chat(&self, workspace_id: &str, code: &str) -> Result<()> {
        let db = self.sdb()?;
        if workspace_id.is_empty() || code.is_empty() {
            return Err(Error::msg(
                "delete closed chat: workspace and code are required",
            ));
        }
        let _g = self.session_mu.lock();
        let mut s = self.state.write();
        let Some(sort_idx) = s.workspaces.iter().position(|w| w.id == workspace_id) else {
            return Err(Error::msg(format!("workspace {workspace_id:?} not found")));
        };
        if s.agents
            .get(workspace_id)
            .is_some_and(|l| l.iter().any(|a| a.code == code))
        {
            return Err(Error::msg("chat is still open"));
        }
        let next_order: Vec<String> = s.workspaces[sort_idx]
            .chat_tab_order
            .iter()
            .filter(|c| c.as_str() != code)
            .cloned()
            .collect();
        let mut persisted = s.workspaces[sort_idx].clone();
        persisted.chat_tab_order = next_order.clone();
        let wrap = |e: Error| match e {
            Error::Sql { context, source } if context.is_empty() => Error::Sql {
                context: "delete closed chat".into(),
                source,
            },
            e => e,
        };
        db.tx(|tx| {
            let sessions = sessions_for_loop_query(tx, workspace_id)?;
            let mut owned = Vec::new();
            let mut own = Vec::new();
            for sess in &sessions {
                let cfg = chat_cfg(&sess.config_json);
                if cfg.owner_code == code {
                    owned.push(sess.id.clone());
                } else if cfg.code == code {
                    own.push(sess.id.clone());
                }
            }
            for id in owned.iter().chain(&own) {
                delete_session_tx(tx, id)?;
            }
            upsert_doc_tx(tx, "workspaces", workspace_id, &persisted, sort_idx)
        })
        .map_err(wrap)?;
        s.workspaces[sort_idx].chat_tab_order = next_order;
        Ok(())
    }

    /// Unlinks every message after message_id (chat "reset to this point");
    /// message rows are deleted only when no fork still references them.
    /// Returns the unlinked ids (`None` when none).
    pub fn truncate_session_after(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<Vec<String>>> {
        let Some(db) = &self.db else {
            return Ok(None);
        };
        if session_id.is_empty() || message_id.is_empty() {
            return Ok(None);
        }
        let _g = self.session_mu.lock();
        db.tx(|tx| {
            let ids: Vec<String> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT message_id FROM session_message WHERE session_id = ?
		   AND idx > (SELECT idx FROM session_message WHERE session_id = ? AND message_id = ?)",
                    )
                    .bare()?;
                let rows = stmt
                    .query_map([session_id, session_id, message_id], |r| {
                        r.get::<_, String>(0)
                    })
                    .bare()?;
                rows.collect::<rusqlite::Result<_>>().bare()?
            };
            for id in &ids {
                tx.execute(
                    "DELETE FROM session_message WHERE session_id = ? AND message_id = ?",
                    [session_id, id],
                )
                .bare()?;
                let refs: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM session_message WHERE message_id = ?",
                        [id],
                        |r| r.get(0),
                    )
                    .bare()?;
                if refs > 0 {
                    continue;
                }
                tx.execute("DELETE FROM message_content WHERE message_id = ?", [id])
                    .bare()?;
                tx.execute("DELETE FROM message WHERE id = ?", [id])
                    .bare()?;
            }
            Ok((!ids.is_empty()).then_some(ids))
        })
    }

    /// Appends one block at the message's next block idx (assigning an id).
    /// Safe to call repeatedly while a message streams.
    pub fn append_content_block(&self, message_id: &str, mut b: ContentBlock) -> Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        let _g = self.session_mu.lock();
        if b.id.is_empty() {
            b.id = new_id();
        }
        db.with(|c| {
            let idx: Option<i64> = c
                .query_row(
                    "SELECT MAX(idx) FROM message_content WHERE message_id = ?",
                    [message_id],
                    |r| r.get(0),
                )
                .ctx(|| "next content idx".into())?;
            insert_content_block_tx(c, message_id, idx.map_or(0, |i| i + 1), &b)
        })
    }

    /// Replaces the arguments on a tool_use block found by its internal id
    /// (providers that attach full args only on completion). `input` is raw
    /// JSON; empty is a no-op.
    pub fn patch_tool_use_input(&self, internal_tool_use_id: &str, input: &str) -> Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        if internal_tool_use_id.is_empty() || input.is_empty() {
            return Ok(());
        }
        let _g = self.session_mu.lock();
        db.with(|c| {
            let row: Option<(String, String)> = c
                .query_row(
                    "SELECT id, content_json FROM message_content
		 WHERE type = ? AND json_extract(content_json, '$.id') = ?
		 ORDER BY created_at DESC LIMIT 1",
                    [BLOCK_TOOL_USE, internal_tool_use_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .ctx(|| format!("lookup tool_use {internal_tool_use_id:?}"))?;
            let Some((id, payload)) = row else {
                return Ok(());
            };
            let mut tu: ToolUsePayload = serde_json::from_str(&payload).map_err(json_err(
                format!("decode tool_use {internal_tool_use_id:?}"),
            ))?;
            tu.input = input.to_owned();
            let b = marshal(&tu).map_err(json_err(""))?;
            c.execute(
                "UPDATE message_content SET content_json = ? WHERE id = ?",
                [&b, &id],
            )
            .ctx(|| format!("patch tool_use {internal_tool_use_id:?}"))?;
            Ok(())
        })
    }

    /// Fills a missing `from` on the latest fallback block of a message.
    pub fn patch_fallback_from(&self, message_id: &str, from: &str) -> Result<()> {
        let from = from.trim();
        let Some(db) = &self.db else {
            return Ok(());
        };
        if message_id.is_empty() || from.is_empty() {
            return Ok(());
        }
        let _g = self.session_mu.lock();
        db.with(|c| {
            let row: Option<(String, String)> = c
                .query_row(
                    "SELECT id, content_json FROM message_content
		 WHERE message_id = ? AND type = ?
		 ORDER BY idx DESC LIMIT 1",
                    [message_id, BLOCK_FALLBACK],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .ctx(|| format!("lookup fallback {message_id}"))?;
            let Some((id, payload)) = row else {
                return Ok(());
            };
            let mut p: FallbackPayload = serde_json::from_str(&payload)
                .map_err(json_err(format!("decode fallback {message_id}")))?;
            if !p.from.trim().is_empty() {
                return Ok(());
            }
            p.from = from.to_owned();
            let b = marshal(&p).map_err(json_err(""))?;
            c.execute(
                "UPDATE message_content SET content_json = ? WHERE id = ?",
                [&b, &id],
            )
            .ctx(|| format!("patch fallback {message_id}"))?;
            Ok(())
        })
    }

    /// The newest tool_use in a session whose name ends with suffix (e.g.
    /// `__ask_user`), compared exactly (LIKE would treat `_` as a wildcard).
    pub fn latest_tool_use_by_name_suffix(
        &self,
        session_id: &str,
        suffix: &str,
    ) -> Option<ToolUsePayload> {
        let db = self.db.as_ref()?;
        if session_id.is_empty() || suffix.is_empty() {
            return None;
        }
        let payload: String = db
            .with(|c| {
                c.query_row(
                    "SELECT mc.content_json FROM message_content mc
		 JOIN session_message sm ON sm.message_id = mc.message_id
		 WHERE sm.session_id = ? AND mc.type = ?
		   AND substr(json_extract(mc.content_json, '$.name'), -?) = ?
		 ORDER BY mc.created_at DESC LIMIT 1",
                    params![session_id, BLOCK_TOOL_USE, suffix.len() as i64, suffix],
                    |r| r.get(0),
                )
                .bare()
            })
            .ok()?;
        let tu: ToolUsePayload = serde_json::from_str(&payload).ok()?;
        (!tu.id.is_empty()).then_some(tu)
    }

    /// Marks a message finished: status, completion time, usage, duration
    /// and context window. Pass `STREAM_ERROR` for a failed turn.
    pub fn complete_message(&self, message_id: &str, status: &str, usage: &Message) -> Result<()> {
        self.exec_update(
            "UPDATE message SET
		stream_status = ?, completed_at = ?,
		input_tokens = ?, output_tokens = ?,
		cache_creation_tokens = ?, cache_read_tokens = ?,
		duration_ms = ?, context_window = ?
		WHERE id = ?",
            params![
                status,
                now_ms(),
                null_int(usage.input_tokens),
                null_int(usage.output_tokens),
                null_int(usage.cache_creation_tokens),
                null_int(usage.cache_read_tokens),
                null_int(usage.duration_ms),
                null_int(usage.context_window),
                message_id
            ],
            || format!("complete message {message_id:?}"),
        )
    }

    /// A session's messages in order, each with its blocks: the single load
    /// path the renderer replays. `None` when the session has no messages.
    pub fn session_transcript(&self, session_id: &str) -> Result<Option<Vec<Message>>> {
        let Some(db) = &self.db else {
            return Ok(None);
        };
        db.with(|c| {
            // One JOIN instead of 1+N: a quiet tab can still have hundreds of
            // tool blocks in a very large message_content table.
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {MESSAGE_COLS},
			mc.id, mc.type, mc.content_json, mc.created_at
		FROM message m
		JOIN session_message sm ON sm.message_id = m.id
		LEFT JOIN message_content mc ON mc.message_id = m.id
		WHERE sm.session_id = ?
		ORDER BY sm.idx, mc.idx"
                ))
                .ctx(|| "query session messages".into())?;
            let rows = stmt
                .query_map([session_id], |r| {
                    let m = scan_message(r)?;
                    let block = match r.get::<_, Option<String>>(15)? {
                        Some(id) => Some(ContentBlock {
                            id,
                            r#type: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
                            content_json: r.get::<_, Option<String>>(17)?.unwrap_or_default(),
                            created_at: r.get::<_, Option<i64>>(18)?.unwrap_or_default(),
                        }),
                        None => None,
                    };
                    Ok((m, block))
                })
                .ctx(|| "query session messages".into())?;
            let mut out: Vec<Message> = Vec::new();
            for row in rows {
                let (m, block) = row.ctx(|| "scan message".into())?;
                if out.last().is_none_or(|last| last.id != m.id) {
                    out.push(m);
                }
                if let (Some(b), Some(last)) = (block, out.last_mut()) {
                    last.content.get_or_insert_with(Vec::new).push(b);
                }
            }
            Ok((!out.is_empty()).then_some(out))
        })
    }

    /// A cheap emptiness check (COUNT on session_message).
    pub fn session_has_messages(&self, session_id: &str) -> bool {
        match &self.db {
            Some(db) if !session_id.is_empty() => db
                .with(|c| Ok(session_has_messages_query(c, session_id)))
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Creates a child session sharing the parent's messages up to and
    /// including fork_message_id (a copy of the join rows, not the
    /// messages). kind and config_json override the parent's (empty keeps
    /// them). The child starts idle with no provider resume ref.
    pub fn fork_session(
        &self,
        parent_id: &str,
        fork_message_id: &str,
        kind: &str,
        config_json: &str,
    ) -> Result<Session> {
        self.fork_session_to(parent_id, fork_message_id, "", kind, config_json)
    }

    /// [`Store::fork_session`] with the child attached to another workspace.
    pub fn fork_session_to_workspace(
        &self,
        parent_id: &str,
        fork_message_id: &str,
        dest_workspace_id: &str,
        kind: &str,
        config_json: &str,
    ) -> Result<Session> {
        if dest_workspace_id.trim().is_empty() {
            return Err(Error::msg(
                "fork session: destination workspace is required",
            ));
        }
        self.fork_session_to(
            parent_id,
            fork_message_id,
            dest_workspace_id,
            kind,
            config_json,
        )
    }

    fn fork_session_to(
        &self,
        parent_id: &str,
        fork_message_id: &str,
        dest_workspace_id: &str,
        kind: &str,
        config_json: &str,
    ) -> Result<Session> {
        let db = self.sdb()?;
        if parent_id.is_empty() || fork_message_id.is_empty() {
            return Err(Error::msg(
                "fork session: parent and fork message are required",
            ));
        }
        let _g = self.session_mu.lock();
        let parent = db
            .with(|c| {
                session_row(
                    c,
                    &format!("SELECT {SESSION_COLS} FROM session WHERE id = ?"),
                    [parent_id],
                )
            })
            .map_err(|e| Error::msg(format!("fork session: load parent: {e}")))?
            .ok_or_else(|| Error::msg("fork session: load parent: sql: no rows in result set"))?;
        let fork_idx: Option<i64> = db
            .with(|c| {
                c.query_row(
                    "SELECT idx FROM session_message WHERE session_id = ? AND message_id = ?",
                    [parent_id, fork_message_id],
                    |r| r.get(0),
                )
                .optional()
                .ctx(|| "fork session: lookup fork point".into())
            })?
            .flatten();
        let Some(fork_idx) = fork_idx else {
            return Err(Error::msg(format!(
                "fork session: message {fork_message_id:?} is not in session {parent_id:?}"
            )));
        };
        let kind = if kind.trim().is_empty() {
            parent.kind.clone()
        } else {
            kind.to_owned()
        };
        let mut config_json = config_json.to_owned();
        if config_json.is_empty() {
            config_json = parent.config_json.clone();
        }
        if config_json.is_empty() {
            config_json = "{}".into();
        }
        let workspace_id = match dest_workspace_id.trim() {
            "" => parent.workspace_id.clone(),
            dest => dest.to_owned(),
        };
        let now = now_ms();
        let mut child = Session {
            id: new_id(),
            workspace_id,
            kind,
            parent_session_id: parent.id.clone(),
            fork_message_id: fork_message_id.to_owned(),
            context_mode: parent.context_mode.clone(),
            provider: parent.provider.clone(),
            config_json,
            status: "idle".into(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        child.active_context_id = db.tx(|tx| {
            insert_session_tx(tx, &child)?;
            tx.execute(
                "INSERT INTO session_message (session_id, message_id, idx)
		 SELECT ?, message_id, idx FROM session_message
		 WHERE session_id = ? AND idx <= ?",
                params![child.id, parent_id, fork_idx],
            )
            .ctx(|| "fork session: copy messages".into())?;
            if parent.active_context_id.is_empty() {
                return Ok(String::new());
            }
            copy_session_context_tx(tx, &parent.active_context_id, &child.id, now)
        })?;
        Ok(child)
    }

    /// Whether message_id is the session's last message.
    pub fn session_message_is_tip(&self, session_id: &str, message_id: &str) -> bool {
        let Some(db) = &self.db else {
            return false;
        };
        if session_id.is_empty() || message_id.is_empty() {
            return false;
        }
        db.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM session_message
		 WHERE session_id = ? AND message_id = ?
		   AND idx = (SELECT MAX(idx) FROM session_message WHERE session_id = ?)",
                [session_id, message_id, session_id],
                |r| r.get::<_, i64>(0),
            )
            .bare()
        })
        .is_ok_and(|n| n == 1)
    }

    /// Whether message_id is a join row of session_id.
    pub fn session_contains_message(&self, session_id: &str, message_id: &str) -> bool {
        let Some(db) = &self.db else {
            return false;
        };
        if session_id.is_empty() || message_id.is_empty() {
            return false;
        }
        db.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM session_message WHERE session_id = ? AND message_id = ?",
                [session_id, message_id],
                |r| r.get::<_, i64>(0),
            )
            .bare()
        })
        .is_ok_and(|n| n > 0)
    }

    /// The first human chat-turn text (display_text when set, else wire
    /// text) without loading tool blocks. Empty until there is a user
    /// prompt (tool_result carrier rows are skipped).
    pub fn session_first_user_prompt(&self, session_id: &str) -> String {
        let Some(db) = &self.db else {
            return String::new();
        };
        if session_id.is_empty() {
            return String::new();
        }
        db.with(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT mc.content_json
		FROM message_content mc
		WHERE mc.message_id = (
			SELECT m.id
			FROM message m
			JOIN session_message sm ON sm.message_id = m.id
			WHERE sm.session_id = ?
			  AND m.role = 'user'
			  AND NOT EXISTS (
				SELECT 1 FROM message_content x
				WHERE x.message_id = m.id
				  AND x.type IN (?, ?, ?)
			  )
			  AND EXISTS (
				SELECT 1 FROM message_content y
				WHERE y.message_id = m.id
				  AND y.type IN (?, ?)
			  )
			ORDER BY sm.idx
			LIMIT 1
		)
		AND mc.type = ?
		ORDER BY mc.idx",
                )
                .bare()?;
            let rows = stmt
                .query_map(
                    params![
                        session_id,
                        BLOCK_TOOL_RESULT,
                        BLOCK_TOOL_USE,
                        BLOCK_THINKING,
                        BLOCK_TEXT,
                        BLOCK_IMAGE,
                        BLOCK_TEXT
                    ],
                    |r| r.get::<_, String>(0),
                )
                .bare()?;
            let mut text = String::new();
            for payload in rows {
                let payload = payload.bare()?;
                let Ok(p) = serde_json::from_str::<TextPayload>(&payload) else {
                    continue;
                };
                let shown = if p.display_text.is_empty() {
                    p.text
                } else {
                    p.display_text
                };
                if shown.trim().is_empty() {
                    continue;
                }
                text.push_str(&shown);
            }
            Ok(text)
        })
        .unwrap_or_default()
    }

    /// The last assistant text in the workspace's latest session (newest by
    /// updated_at, then newest message, then newest block), preferring
    /// display_text. Empty when there is none.
    pub fn last_assistant_text(&self, workspace_id: &str) -> String {
        let Some(db) = &self.db else {
            return String::new();
        };
        if workspace_id.trim().is_empty() {
            return String::new();
        }
        let payload: Option<String> = db
            .with(|c| {
                c.query_row(
                    "SELECT mc.content_json
		FROM message_content mc
		JOIN message m ON m.id = mc.message_id
		JOIN session_message sm ON sm.message_id = m.id
		WHERE sm.session_id = (
			SELECT id FROM session
			WHERE workspace_id = ?
			ORDER BY updated_at DESC, created_at DESC, id DESC
			LIMIT 1
		)
		AND m.role = 'assistant'
		AND mc.type = ?
		AND (
			length(trim(coalesce(json_extract(mc.content_json, '$.text'), ''))) > 0
			OR length(trim(coalesce(json_extract(mc.content_json, '$.display_text'), ''))) > 0
		)
		ORDER BY sm.idx DESC, mc.idx DESC
		LIMIT 1",
                    [workspace_id, BLOCK_TEXT],
                    |r| r.get(0),
                )
                .bare()
            })
            .ok();
        let Some(p) = payload.and_then(|p| serde_json::from_str::<TextPayload>(&p).ok()) else {
            return String::new();
        };
        let text = p.display_text.trim();
        if text.is_empty() {
            p.text.trim().to_owned()
        } else {
            text.to_owned()
        }
    }
}

/// Clones the parent's active compaction row onto the child when the
/// watermark message was copied. Empty means the fork point is before the
/// compact, so the child has no active context.
fn copy_session_context_tx(
    c: &Connection,
    parent_context_id: &str,
    child_id: &str,
    now: i64,
) -> Result<String> {
    let row: Option<(String, String)> = c
        .query_row(
            "SELECT summary, last_compacted_message_id FROM session_context WHERE id = ?",
            [parent_context_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ctx(|| "fork session: load parent context".into())?;
    let Some((summary, watermark)) = row else {
        return Ok(String::new());
    };
    let n: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM session_message WHERE session_id = ? AND message_id = ?",
            [child_id, &watermark],
            |r| r.get(0),
        )
        .ctx(|| "fork session: check context watermark".into())?;
    if n == 0 {
        return Ok(String::new());
    }
    let id = new_id();
    c.execute(
        "INSERT INTO session_context (id, session_id, summary, last_compacted_message_id, created_at)
		 VALUES (?,?,?,?,?)",
        params![id, child_id, summary, watermark, now],
    )
    .ctx(|| "fork session: copy context".into())?;
    c.execute(
        "UPDATE session SET active_context_id = ? WHERE id = ?",
        [&id, child_id],
    )
    .ctx(|| "fork session: set child context".into())?;
    Ok(id)
}
