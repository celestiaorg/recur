use std::collections::BTreeMap;

use recur_domain::{
    BLOCK_TEXT, BLOCK_TOOL_USE, CONTEXT_MODE_PROVIDER, ContentBlock, Message, PROVIDER_CLAUDE_CODE,
    PROVIDER_CURSOR, SESSION_KIND_APP, STREAM_COMPLETE, Session, StoredEvent, Workspace,
};

use super::*;
use crate::migrations::{latest_schema_version, schema_version};

fn sess(ws: &str, kind: &str, cfg: &str) -> Session {
    Session {
        workspace_id: ws.into(),
        kind: kind.into(),
        provider: PROVIDER_CLAUDE_CODE.into(),
        config_json: cfg.into(),
        ..Default::default()
    }
}

fn msg(role: &str, blocks: Vec<ContentBlock>) -> Message {
    Message {
        role: role.into(),
        content: Some(blocks),
        ..Default::default()
    }
}

fn text(role: &str, t: &str) -> Message {
    msg(role, vec![ContentBlock::text(t)])
}

fn ids_of(msgs: &[Message]) -> Vec<String> {
    msgs.iter().map(|m| m.id.clone()).collect()
}

fn last_text(msgs: &[Message]) -> String {
    msgs.last()
        .and_then(|m| m.blocks().first())
        .and_then(ContentBlock::as_text)
        .map(|p| p.text)
        .unwrap_or_default()
}

fn transcript(st: &Store, id: &str) -> Vec<Message> {
    st.session_transcript(id).unwrap().unwrap_or_default()
}

fn version(st: &Store) -> i64 {
    db(st).with(|c| schema_version(c)).unwrap()
}

/// A fresh DB reports the latest schema version, and a pre-rename
/// (loop-era) database migrates in place without losing data.
#[test]
fn migration_runner_baseline_and_sessions() {
    let dir = tmp();
    let path = db_file(dir.path());
    let st = open_temp(dir.path());
    assert_eq!(version(&st), latest_schema_version());
    exec(&st, "SELECT 1 FROM session LIMIT 1").unwrap();
    let seeded = st.workspaces().len();
    st.close().unwrap();

    raw(
        &path,
        &[
            "ALTER TABLE workspaces RENAME TO loops",
            "ALTER TABLE agents RENAME COLUMN workspace_id TO loop_id",
            "ALTER TABLE feed RENAME COLUMN workspace_id TO loop_id",
            "ALTER TABLE escalations RENAME COLUMN workspace_id TO loop_id",
            // maestro_chat was dropped by 0006, but a real pre-4 DB had it
            // and 0004 renames its column.
            "CREATE TABLE maestro_chat (loop_id TEXT PRIMARY KEY, doc_json TEXT NOT NULL)",
            "ALTER TABLE events RENAME COLUMN workspace_id TO loop_id",
            "ALTER TABLE findings RENAME COLUMN workspace_id TO loop_id",
            "ALTER TABLE analysis RENAME COLUMN workspace_id TO loop_id",
            "DELETE FROM __migrations WHERE version = 4",
        ],
    );

    let s2 = Store::open(&path).unwrap();
    assert_eq!(version(&s2), latest_schema_version());
    assert_eq!(s2.workspaces().len(), seeded, "in-place rename lost data");
    exec(&s2, "SELECT 1 FROM workspaces LIMIT 1").unwrap();
    exec(&s2, "SELECT workspace_id FROM feed LIMIT 1").unwrap();
}

#[test]
fn drop_message_cost_usd() {
    let dir = tmp();
    open_temp(dir.path()).close().unwrap();
    raw(
        &db_file(dir.path()),
        &[
            "ALTER TABLE message ADD COLUMN cost_usd REAL",
            "DELETE FROM __migrations WHERE version = 13",
        ],
    );
    let s2 = open_temp(dir.path());
    assert_eq!(
        query_i64(
            &s2,
            "SELECT COUNT(*) FROM pragma_table_info('message') WHERE name = 'cost_usd'"
        ),
        0
    );
}

/// Messages and typed blocks persist and reload in order across a reopen.
#[test]
fn session_transcript_round_trip() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st.create_session(sess("LOOP-1", "maestro", "")).unwrap();
    assert!(!s.id.is_empty() && s.context_mode == CONTEXT_MODE_PROVIDER && s.status == "idle");
    assert_eq!(s.config_json, "{}");
    assert!(!st.session_has_messages(&s.id));

    st.append_message(&s.id, text("user", "fix the flaky test"))
        .unwrap();
    assert!(st.session_has_messages(&s.id));
    let msg_id = st
        .create_message(
            &s.id,
            Message {
                role: "assistant".into(),
                model_id: "claude-opus-4-8".into(),
                provider: PROVIDER_CLAUDE_CODE.into(),
                ..Default::default()
            },
        )
        .unwrap();
    st.append_content_block(&msg_id, ContentBlock::text("Looking at it."))
        .unwrap();
    st.append_content_block(
        &msg_id,
        ContentBlock::tool_use(
            "tu-int-1",
            "toolu_abc",
            "Read",
            r#"{"file_path":"x_test.go"}"#,
        ),
    )
    .unwrap();
    st.complete_message(
        &msg_id,
        STREAM_COMPLETE,
        &Message {
            input_tokens: 1200,
            output_tokens: 340,
            ..Default::default()
        },
    )
    .unwrap();
    st.append_message(
        &s.id,
        msg(
            "user",
            vec![ContentBlock::tool_result("tu-int-1", "package x", false)],
        ),
    )
    .unwrap();

    let check = |st: &Store, label: &str| {
        let msgs = transcript(st, &s.id);
        assert_eq!(msgs.len(), 3, "{label}");
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "user"], "{label}");
        let asst = &msgs[1];
        assert_eq!(asst.model_id, "claude-opus-4-8");
        assert_eq!(asst.stream_status, STREAM_COMPLETE);
        assert!(asst.completed_at > 0);
        assert_eq!(
            (asst.input_tokens, asst.output_tokens),
            (1200, 340),
            "{label}"
        );
        let blocks = asst.blocks();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].r#type, BLOCK_TEXT);
        assert_eq!(blocks[1].r#type, BLOCK_TOOL_USE);
        let tu = blocks[1].as_tool_use().unwrap();
        assert!(tu.id == "tu-int-1" && tu.provider_tool_use_id == "toolu_abc" && tu.name == "Read");
        let tr = msgs[2].blocks()[0].as_tool_result().unwrap();
        assert!(tr.tool_use_id == "tu-int-1" && tr.result == "package x");
    };
    check(&st, "in memory");
    st.close().unwrap();
    check(&open_temp(dir.path()), "after reopen");
}

#[test]
fn session_for_workspace_kind() {
    let dir = tmp();
    let st = open_temp(dir.path());
    assert!(st.session_for_workspace_kind("LOOP-9", "maestro").is_none());
    let created = st.create_session(sess("LOOP-9", "maestro", "")).unwrap();
    assert_eq!(
        st.session_for_workspace_kind("LOOP-9", "maestro")
            .unwrap()
            .id,
        created.id
    );
    st.update_session_ref(&created.id, "claude-sess-xyz")
        .unwrap();
    st.update_session_ref(&created.id, "").unwrap();
    st.set_session_status(&created.id, "streaming").unwrap();
    st.update_session_provider(&created.id, PROVIDER_CURSOR)
        .unwrap();
    st.update_session_provider(&created.id, " ").unwrap();
    st.close().unwrap();
    let s2 = open_temp(dir.path());
    let got = s2.get_session(&created.id).unwrap();
    assert_eq!(got.provider_session_ref, "claude-sess-xyz");
    assert_eq!(got.status, "streaming");
    assert_eq!(got.provider, PROVIDER_CURSOR);
    assert!(s2.get_session("nope").is_none());
}

#[test]
fn session_for_app_kind_and_streaming_recovery() {
    let dir = tmp();
    let st = open_temp(dir.path());
    assert!(st.session_for_app_kind(SESSION_KIND_APP).is_none());
    assert!(st.session_for_app_kind("").is_none());
    let created = st
        .create_session(Session {
            status: "streaming".into(),
            ..sess("", SESSION_KIND_APP, r#"{"code":"APP","name":"Ask Recur"}"#)
        })
        .unwrap();
    assert_eq!(created.workspace_id, "");
    assert_eq!(
        st.session_for_app_kind(SESSION_KIND_APP).unwrap().id,
        created.id
    );
    assert_eq!(st.sessions_for_app_kind(SESSION_KIND_APP).unwrap().len(), 1);
    let live = st.streaming_app_sessions().unwrap();
    assert_eq!(ids(&live), vec![created.id.clone()]);
    st.set_session_status(&created.id, "idle").unwrap();
    assert!(st.streaming_app_sessions().is_none());
}

fn ids(list: &[Session]) -> Vec<String> {
    list.iter().map(|s| s.id.clone()).collect()
}

#[test]
fn attention_counts_app_unseen() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let before = st.attention_counts();
    let created = st
        .create_session(sess(
            "",
            SESSION_KIND_APP,
            r#"{"code":"APP","unseen":true}"#,
        ))
        .unwrap();
    st.create_session(sess(
        "",
        SESSION_KIND_APP,
        r#"{"code":"APP2","unseen":true,"closed":true}"#,
    ))
    .unwrap();
    let got = st.attention_counts();
    assert_eq!(got.app_unseen, 1);
    assert_eq!(got.dock, before.dock + 1);
    st.update_session_config_json(&created.id, r#"{"code":"APP"}"#)
        .unwrap();
    assert_eq!(st.attention_counts().app_unseen, 0);
    st.update_session_config_json(&created.id, "").unwrap();
    assert_eq!(st.get_session(&created.id).unwrap().config_json, "{}");
}

#[test]
fn session_headers_and_latest_event_ts() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let a = st
        .create_session(sess("W-A", "pair", r#"{"code":"PA"}"#))
        .unwrap();
    st.create_session(sess("W-B", "pair", r#"{"code":"PA"}"#))
        .unwrap();
    for _ in 0..9 {
        st.create_session(sess("W-C", "pair", "")).unwrap();
    }
    let headers = st.session_headers_by_workspace().unwrap();
    assert_eq!(headers["W-A"][0].id, a.id);
    assert!(headers.contains_key("W-B"));
    assert_eq!(headers["W-C"].len(), 8, "capped per workspace");
    st.append_event(&StoredEvent {
        workspace_id: "W-A".into(),
        r#type: "note".into(),
        ts: 99,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(st.latest_event_ts_by_workspace().unwrap()["W-A"], 99);
}

#[test]
fn reassign_workspace_sessions() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st
        .create_session(sess("W-host", "pair", r#"{"code":"PA"}"#))
        .unwrap();
    st.reassign_workspace_sessions("W-host", "W-live", &BTreeMap::new())
        .unwrap();
    assert_eq!(st.session_for_loop_agent("W-live", "PA").unwrap().id, s.id);
    assert!(st.session_for_loop_agent("W-host", "PA").is_none());
    assert_eq!(st.sessions_for_loop("W-live").unwrap().len(), 1);
    assert!(st.sessions_for_loop("W-host").is_none());
}

#[test]
fn reassign_workspace_sessions_remaps_colliding_codes() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let host = st
        .create_session(sess("W-host", "pair", r#"{"code":"PA"}"#))
        .unwrap();
    st.append_message(&host.id, text("user", "host chat"))
        .unwrap();
    let owned = st
        .create_session(sess("W-host", "coder", r#"{"code":"CO","ownerCode":"PA"}"#))
        .unwrap();
    let live = st
        .create_session(sess("W-live", "pair", r#"{"code":"PA"}"#))
        .unwrap();
    st.append_message(&live.id, text("user", "live chat"))
        .unwrap();
    st.reassign_workspace_sessions(
        "W-host",
        "W-live",
        &BTreeMap::from([("PA".to_owned(), "PA2".to_owned())]),
    )
    .unwrap();
    assert_eq!(
        st.session_for_loop_agent("W-live", "PA").unwrap().id,
        live.id
    );
    let got = st.session_for_loop_agent("W-live", "PA2").unwrap();
    assert_eq!(got.id, host.id);
    assert!(
        got.config_json.contains(r#""code":"PA2""#),
        "{}",
        got.config_json
    );
    let spec = st.session_for_chat_agent("W-live", "PA2", "CO").unwrap();
    assert_eq!(spec.id, owned.id);
    assert!(
        spec.config_json.contains(r#""ownerCode":"PA2""#),
        "{}",
        spec.config_json
    );
}

#[test]
fn reassign_workspace_sessions_drops_empty_dest_collision() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let host = st
        .create_session(sess("W-host", "pair", r#"{"code":"PA"}"#))
        .unwrap();
    st.append_message(&host.id, text("user", "host chat"))
        .unwrap();
    st.create_session(sess("W-live", "pair", r#"{"code":"PA"}"#))
        .unwrap();
    st.reassign_workspace_sessions("W-host", "W-live", &BTreeMap::new())
        .unwrap();
    assert_eq!(
        st.session_for_loop_agent("W-live", "PA").unwrap().id,
        host.id
    );
    assert_eq!(st.sessions_for_loop("W-live").unwrap().len(), 1);
}

/// A second tool_result for the same internal tool_use id in one message is
/// rejected by the unique partial index.
#[test]
fn tool_result_unique_index() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st.create_session(sess("", "task", "")).unwrap();
    let m = st
        .create_message(
            &s.id,
            Message {
                role: "user".into(),
                ..Default::default()
            },
        )
        .unwrap();
    st.append_content_block(&m, ContentBlock::tool_result("tu-1", "first", false))
        .unwrap();
    assert!(
        st.append_content_block(&m, ContentBlock::tool_result("tu-1", "duplicate", false))
            .is_err()
    );
}

#[test]
fn patch_tool_use_input() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st.create_session(sess("", "task", "")).unwrap();
    let m = st
        .create_message(
            &s.id,
            Message {
                role: "assistant".into(),
                ..Default::default()
            },
        )
        .unwrap();
    st.append_content_block(
        &m,
        ContentBlock::tool_use("tu-ask", "call-1", "mcp__recur__ask_user", "{}"),
    )
    .unwrap();
    let want = r#"{"question":"Ship it?"}"#;
    st.patch_tool_use_input("tu-ask", want).unwrap();
    st.patch_tool_use_input("missing", want).unwrap();
    let msgs = transcript(&st, &s.id);
    assert_eq!(msgs.len(), 1);
    let tu = msgs[0].blocks()[0].as_tool_use().unwrap();
    assert_eq!(tu.input, want);
    let got = st
        .latest_tool_use_by_name_suffix(&s.id, "__ask_user")
        .unwrap();
    assert_eq!(got.id, "tu-ask");
    assert_eq!(got.input, want);
    assert!(
        st.latest_tool_use_by_name_suffix(&s.id, "_xask_user")
            .is_none()
    );
}

#[test]
fn patch_fallback_from() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st.create_session(sess("", "task", "")).unwrap();
    let m = st
        .create_message(
            &s.id,
            Message {
                role: "assistant".into(),
                ..Default::default()
            },
        )
        .unwrap();
    st.append_content_block(&m, ContentBlock::fallback("", "sonnet"))
        .unwrap();
    st.patch_fallback_from(&m, " opus ").unwrap();
    st.patch_fallback_from(&m, "other").unwrap();
    let fb = transcript(&st, &s.id)[0].blocks()[0].as_fallback().unwrap();
    assert_eq!(fb.from, "opus");
    assert_eq!(fb.to, "sonnet");
}

fn coder(cfg: &str) -> Session {
    Session {
        context_mode: CONTEXT_MODE_PROVIDER.into(),
        ..sess("LOOP-1", "coder", cfg)
    }
}

fn cfg_of(st: &Store, code: &str) -> String {
    st.session_for_loop_agent("LOOP-1", code)
        .unwrap()
        .config_json
}

#[test]
fn patch_session_config_title_keeps_mode_model_flags() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st
        .create_session(coder(
            r#"{"code":"PA2","name":"Agent","model":"sonnet","flags":{"effort":"high"},"mode":"review"}"#,
        ))
        .unwrap();
    st.patch_session_config_title(&s.id, "Review the diff")
        .unwrap();
    let got = cfg_of(&st, "PA2");
    for want in [
        r#""mode":"review""#,
        r#""model":"sonnet""#,
        r#""effort":"high""#,
        r#""title":"Review the diff""#,
    ] {
        assert!(got.contains(want), "{want} missing: {got}");
    }
}

#[test]
fn patch_session_config_simplify_keeps_mode_model_flags() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st
        .create_session(coder(
            r#"{"code":"PA2","name":"Agent","model":"sonnet","flags":{"effort":"high"},"mode":"review","title":"Review the diff"}"#,
        ))
        .unwrap();
    st.patch_session_config_simplify(&s.id, false).unwrap();
    let got = cfg_of(&st, "PA2");
    for want in [
        r#""mode":"review""#,
        r#""model":"sonnet""#,
        r#""effort":"high""#,
        r#""title":"Review the diff""#,
        r#""simplify":false"#,
    ] {
        assert!(got.contains(want), "{want} missing: {got}");
    }
    st.patch_session_config_title(&s.id, "Still reviewing")
        .unwrap();
    let got = cfg_of(&st, "PA2");
    assert!(
        got.contains(r#""simplify":false"#) && got.contains(r#""title":"Still reviewing""#),
        "{got}"
    );
    st.patch_session_config_simplify(&s.id, true).unwrap();
    assert!(cfg_of(&st, "PA2").contains(r#""simplify":true"#));
}

#[test]
fn patch_session_config_merge_keeps_simplify() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st
        .create_session(coder(
            r#"{"code":"PA","name":"Agent","model":"sonnet","mode":"agent","simplify":false}"#,
        ))
        .unwrap();
    st.patch_session_config_merge(
        &s.id,
        r#"{"model":"opus","mode":"review","flags":{"effort":"high"}}"#,
    )
    .unwrap();
    let got = cfg_of(&st, "PA");
    for want in [
        r#""simplify":false"#,
        r#""mode":"review""#,
        r#""model":"opus""#,
    ] {
        assert!(got.contains(want), "{want} missing: {got}");
    }
    st.patch_session_config_simplify_if_unset(&s.id, true)
        .unwrap();
    assert!(
        cfg_of(&st, "PA").contains(r#""simplify":false"#),
        "if-unset overwrote explicit false"
    );

    let fresh = st.create_session(coder(r#"{"code":"PB"}"#)).unwrap();
    st.patch_session_config_simplify_if_unset(&fresh.id, true)
        .unwrap();
    assert!(cfg_of(&st, "PB").contains(r#""simplify":true"#));
}

#[test]
fn patch_session_config_flags_replaces_object() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st
        .create_session(coder(r#"{"code":"PA","model":"sonnet","flags":{"effort":"high","thinking":"on"},"simplify":false}"#))
        .unwrap();
    st.patch_session_config_flags(&s.id, r#"{"effort":"low"}"#)
        .unwrap();
    let got = cfg_of(&st, "PA");
    assert!(
        got.contains(r#""simplify":false"#) && got.contains(r#""effort":"low""#),
        "{got}"
    );
    assert!(!got.contains(r#""thinking""#), "{got}");
    st.patch_session_config_flags(&s.id, " ").unwrap();
    assert!(cfg_of(&st, "PA").contains(r#""flags":{}"#));
}

#[test]
fn patch_session_config_dock_replaces_flags_keeps_simplify() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st
        .create_session(coder(
            r#"{"code":"PA","model":"sonnet","mode":"agent","flags":{"effort":"high","thinking":"on"},"simplify":false}"#,
        ))
        .unwrap();
    st.patch_session_config_dock(
        &s.id,
        r#"{"model":"opus","mode":"review"}"#,
        r#"{"effort":"low"}"#,
    )
    .unwrap();
    let got = cfg_of(&st, "PA");
    for want in [
        r#""simplify":false"#,
        r#""model":"opus""#,
        r#""mode":"review""#,
        r#""effort":"low""#,
    ] {
        assert!(got.contains(want), "{want} missing: {got}");
    }
    assert!(!got.contains(r#""thinking""#), "{got}");
    st.patch_session_config_dock(&s.id, r#"{"mode":"ask"}"#, "")
        .unwrap();
    let got = cfg_of(&st, "PA");
    assert!(
        got.contains(r#""mode":"ask""#) && got.contains(r#""effort":"low""#),
        "{got}"
    );
    st.patch_session_config_dock(&s.id, "", "").unwrap();
}

#[test]
fn patch_session_config_merge_removes_fork_inject_keeps_simplify() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st
        .create_session(coder(
            r#"{"code":"PA","name":"Agent","forkInject":true,"simplify":false}"#,
        ))
        .unwrap();
    st.patch_session_config_merge(&s.id, r#"{"forkInject":null}"#)
        .unwrap();
    let got = cfg_of(&st, "PA");
    assert!(!got.contains("forkInject"), "{got}");
    assert!(got.contains(r#""simplify":false"#), "{got}");
}

#[test]
fn message_duration_round_trip() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st.create_session(coder("")).unwrap();
    let id = st
        .create_message(
            &s.id,
            Message {
                role: "assistant".into(),
                ..Default::default()
            },
        )
        .unwrap();
    st.complete_message(
        &id,
        STREAM_COMPLETE,
        &Message {
            duration_ms: 8460,
            ..Default::default()
        },
    )
    .unwrap();
    let msgs = transcript(&st, &s.id);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].duration_ms, 8460);
    assert!(msgs[0].content.is_none(), "no blocks is Go's nil");
}

#[test]
fn message_context_window_round_trip() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st.create_session(coder("")).unwrap();
    let id = st
        .create_message(
            &s.id,
            Message {
                role: "assistant".into(),
                ..Default::default()
            },
        )
        .unwrap();
    st.complete_message(
        &id,
        STREAM_COMPLETE,
        &Message {
            input_tokens: 120,
            cache_read_tokens: 18000,
            cache_creation_tokens: 500,
            duration_ms: 100,
            context_window: 200_000,
            ..Default::default()
        },
    )
    .unwrap();
    let m = &transcript(&st, &s.id)[0];
    assert_eq!(
        (m.input_tokens, m.cache_read_tokens, m.cache_creation_tokens),
        (120, 18000, 500)
    );
    assert_eq!(m.context_window, 200_000);
}

/// Skips tool_result carriers and prefers display_text.
#[test]
fn session_first_user_prompt() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st.create_session(coder("")).unwrap();
    assert_eq!(st.session_first_user_prompt(&s.id), "");
    st.append_message(
        &s.id,
        msg(
            "user",
            vec![ContentBlock::tool_result(
                "tu-1",
                &"diff ".repeat(200),
                false,
            )],
        ),
    )
    .unwrap();
    assert_eq!(st.session_first_user_prompt(&s.id), "");
    st.append_message(
        &s.id,
        msg(
            "user",
            vec![ContentBlock::text_display(
                "wire prompt with @files",
                "review the flaky suite",
            )],
        ),
    )
    .unwrap();
    st.append_message(&s.id, text("assistant", "ok")).unwrap();
    assert_eq!(
        st.session_first_user_prompt(&s.id),
        "review the flaky suite"
    );
}

/// Walks assistant text newest first in the newest session.
#[test]
fn last_assistant_text_picks_last_text_in_latest_session() {
    let dir = tmp();
    let st = open_temp(dir.path());
    assert_eq!(st.last_assistant_text("WS-snippet"), "");
    let old = st
        .create_session(Session {
            created_at: 1,
            ..sess("WS-snippet", "coder", "")
        })
        .unwrap();
    st.append_message(&old.id, text("assistant", "old session last line"))
        .unwrap();
    let s = st
        .create_session(Session {
            created_at: 2,
            ..sess("WS-snippet", "coder", "")
        })
        .unwrap();
    st.append_message(
        &s.id,
        msg(
            "assistant",
            vec![
                ContentBlock::text("I'll load the Recur triage skill and Linear tools."),
                ContentBlock::tool_use("t1", "tu-1", "Skill", ""),
                ContentBlock::text("No matching issues in ACME Triage."),
                ContentBlock::text(""),
            ],
        ),
    )
    .unwrap();
    assert_eq!(
        st.last_assistant_text("WS-snippet"),
        "No matching issues in ACME Triage."
    );
    let ui = st
        .create_session(Session {
            created_at: 3,
            ..sess("WS-snippet", "coder", "")
        })
        .unwrap();
    st.append_message(
        &ui.id,
        msg(
            "assistant",
            vec![ContentBlock::text_display(
                "WIRE do not show this in the snippet",
                "All tests passed on the nightly run.",
            )],
        ),
    )
    .unwrap();
    assert_eq!(
        st.last_assistant_text("WS-snippet"),
        "All tests passed on the nightly run."
    );
}

/// The child shares message rows through the fork message and new appends
/// stay isolated.
#[test]
fn fork_session_copies_join_rows_up_to_fork_point() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let parent = st
        .create_session(Session {
            context_mode: CONTEXT_MODE_PROVIDER.into(),
            ..sess("WS-1", "maestro", r#"{"code":"PA","title":"orig"}"#)
        })
        .unwrap();
    let u1 = st
        .append_message(&parent.id, text("user", "first"))
        .unwrap();
    let a1 = st
        .append_message(&parent.id, text("assistant", "ok one"))
        .unwrap();
    st.append_message(&parent.id, text("user", "second"))
        .unwrap();
    st.append_message(&parent.id, text("assistant", "ok two"))
        .unwrap();
    assert!(!st.session_message_is_tip(&parent.id, &a1.id));

    let child = st
        .fork_session(
            &parent.id,
            &a1.id,
            "coder",
            r#"{"code":"PA2","title":"fork"}"#,
        )
        .unwrap();
    assert_eq!(child.parent_session_id, parent.id);
    assert_eq!(child.fork_message_id, a1.id);
    assert!(
        child.kind == "coder" && child.provider_session_ref.is_empty() && child.status == "idle"
    );
    assert!(child.workspace_id == "WS-1" && child.provider == PROVIDER_CLAUDE_CODE);
    assert!(st.session_message_is_tip(&child.id, &a1.id));

    let got = transcript(&st, &child.id);
    assert_eq!(ids_of(&got), [u1.id.clone(), a1.id.clone()]);
    assert_eq!(got[0].blocks()[0].r#type, BLOCK_TEXT);

    st.append_message(&child.id, text("user", "from fork"))
        .unwrap();
    st.append_message(&parent.id, text("user", "from parent"))
        .unwrap();
    let pm = transcript(&st, &parent.id);
    let cm = transcript(&st, &child.id);
    assert_eq!(pm.len(), 5);
    assert_eq!(cm.len(), 3);
    assert_eq!(last_text(&pm), "from parent");
    assert_eq!(last_text(&cm), "from fork");

    assert!(!st.session_contains_message(&parent.id, "no-such-msg"));
    assert!(st.session_contains_message(&parent.id, &a1.id));
    assert!(
        st.fork_session(&parent.id, "no-such-msg", "coder", "{}")
            .is_err()
    );
    assert!(
        st.fork_session("no-such-parent", &a1.id, "coder", "{}")
            .is_err()
    );
    assert!(st.fork_session("", &a1.id, "coder", "{}").is_err());

    let same_kind = st.fork_session(&parent.id, &a1.id, " ", "").unwrap();
    assert_eq!(same_kind.kind, "maestro");
    assert_eq!(same_kind.config_json, r#"{"code":"PA","title":"orig"}"#);

    let moved = st
        .fork_session_to_workspace(&parent.id, &a1.id, "WS-2", "coder", r#"{"code":"PA"}"#)
        .unwrap();
    assert_eq!(moved.workspace_id, "WS-2");
    assert_eq!(
        ids_of(&transcript(&st, &moved.id)),
        [u1.id.clone(), a1.id.clone()]
    );
    assert!(
        st.fork_session_to_workspace(&parent.id, &a1.id, "", "coder", "{}")
            .is_err()
    );
}

#[test]
fn delete_session_drops_orphans_and_keeps_forks() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let parent = st.create_session(coder(r#"{"code":"PA"}"#)).unwrap();
    let shared = st.append_message(&parent.id, text("user", "keep")).unwrap();
    let only_parent = st
        .append_message(&parent.id, text("assistant", "gone"))
        .unwrap();
    let child = st
        .fork_session(&parent.id, &shared.id, "coder", r#"{"code":"PA2"}"#)
        .unwrap();
    st.delete_session(&parent.id).unwrap();
    assert!(st.get_session(&parent.id).is_none());
    assert_eq!(ids_of(&transcript(&st, &child.id)), vec![shared.id.clone()]);
    assert_eq!(
        query_i64(
            &st,
            &format!(
                "SELECT COUNT(*) FROM message WHERE id = '{}'",
                only_parent.id
            )
        ),
        0
    );
    assert_eq!(
        query_i64(
            &st,
            &format!(
                "SELECT COUNT(*) FROM message_content WHERE message_id = '{}'",
                only_parent.id
            )
        ),
        0
    );
}

#[test]
fn delete_closed_chat_drops_owned_and_strips_order() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_workspace(Workspace {
        id: "WS-1".into(),
        goal: "delete-closed".into(),
        chat_tab_order: vec!["PA".into(), "PA2".into()],
        ..Default::default()
    })
    .unwrap();
    let keep = st
        .create_session(Session {
            workspace_id: "WS-1".into(),
            ..coder(r#"{"code":"PA"}"#)
        })
        .unwrap();
    let drop = st
        .create_session(Session {
            workspace_id: "WS-1".into(),
            ..coder(r#"{"code":"PA2"}"#)
        })
        .unwrap();
    let owned = st
        .create_session(Session {
            workspace_id: "WS-1".into(),
            ..coder(r#"{"code":"CO","ownerCode":"PA2"}"#)
        })
        .unwrap();
    st.append_message(&drop.id, text("user", "throw away"))
        .unwrap();
    st.upsert_agent(
        "WS-1",
        recur_domain::Agent {
            code: "PA".into(),
            ..Default::default()
        },
    );
    assert_eq!(
        st.delete_closed_chat("WS-1", "PA").unwrap_err().to_string(),
        "chat is still open"
    );
    assert!(st.delete_closed_chat("WS-9", "PA2").is_err());
    assert!(st.delete_closed_chat("WS-1", "").is_err());
    st.delete_closed_chat("WS-1", "PA2").unwrap();
    assert!(st.get_session(&drop.id).is_none());
    assert!(st.get_session(&owned.id).is_none());
    assert!(st.get_session(&keep.id).is_some());
    assert_eq!(st.workspace("WS-1").unwrap().chat_tab_order, ["PA"]);
    st.close().unwrap();
    assert_eq!(
        open_temp(dir.path())
            .workspace("WS-1")
            .unwrap()
            .chat_tab_order,
        ["PA"]
    );
}

#[test]
fn truncate_and_drop_session_messages() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st.create_session(coder("")).unwrap();
    let m1 = st.append_message(&s.id, text("user", "one")).unwrap();
    let m2 = st.append_message(&s.id, text("assistant", "two")).unwrap();
    let m3 = st.append_message(&s.id, text("user", "three")).unwrap();
    let fork = st.fork_session(&s.id, &m2.id, "", "").unwrap();
    let gone = st.truncate_session_after(&s.id, &m1.id).unwrap().unwrap();
    assert_eq!(gone, [m2.id.clone(), m3.id.clone()]);
    assert_eq!(ids_of(&transcript(&st, &s.id)), vec![m1.id.clone()]);
    assert_eq!(
        ids_of(&transcript(&st, &fork.id)),
        [m1.id.clone(), m2.id.clone()],
        "fork keeps shared m2"
    );
    assert_eq!(
        query_i64(
            &st,
            &format!("SELECT COUNT(*) FROM message WHERE id = '{}'", m3.id)
        ),
        0
    );
    assert!(st.truncate_session_after(&s.id, &m1.id).unwrap().is_none());

    st.drop_session_messages(&fork.id, &[m2.id.clone(), " ".into()])
        .unwrap();
    assert_eq!(ids_of(&transcript(&st, &fork.id)), vec![m1.id.clone()]);
    assert_eq!(
        query_i64(
            &st,
            &format!("SELECT COUNT(*) FROM message WHERE id = '{}'", m2.id)
        ),
        0
    );
    let m4 = st
        .append_message(&fork.id, text("user", "after gap"))
        .unwrap();
    assert_eq!(
        ids_of(&transcript(&st, &fork.id)),
        [m1.id.clone(), m4.id.clone()]
    );
}

#[test]
fn write_session_context_and_fork_copies_it() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let parent = st
        .create_session(Session {
            provider: PROVIDER_CURSOR.into(),
            ..sess("WS-1", "coder", r#"{"code":"PA"}"#)
        })
        .unwrap();
    let early = st
        .append_message(&parent.id, text("user", "ancient"))
        .unwrap();
    let compacted = st
        .append_message(&parent.id, text("assistant", "done with the old file"))
        .unwrap();
    let live = st
        .append_message(&parent.id, text("user", "keep going"))
        .unwrap();

    let ctx = st
        .write_session_context(&parent.id, "older turns folded", &compacted.id)
        .unwrap();
    let got = st.active_session_context(&parent.id).unwrap();
    assert_eq!(got.id, ctx.id);
    assert_eq!(got.summary, "older turns folded");
    assert_eq!(got.last_compacted_message_id, compacted.id);
    assert_eq!(
        st.get_session(&parent.id).unwrap().active_context_id,
        ctx.id
    );

    let child = st
        .fork_session(&parent.id, &live.id, "coder", r#"{"code":"PA2"}"#)
        .unwrap();
    assert!(!child.active_context_id.is_empty() && child.active_context_id != ctx.id);
    let cc = st.active_session_context(&child.id).unwrap();
    assert_eq!(cc.summary, "older turns folded");
    assert_eq!(cc.last_compacted_message_id, compacted.id);
    assert_eq!(cc.session_id, child.id);

    let before = st
        .fork_session(&parent.id, &early.id, "coder", r#"{"code":"PA3"}"#)
        .unwrap();
    assert_eq!(before.active_context_id, "");
    assert!(st.active_session_context(&before.id).is_none());
    assert!(
        st.write_session_context(&parent.id, "", &compacted.id)
            .is_err()
    );
}

#[test]
fn deleting_workspace_cascades_sessions() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let s = st
        .create_session(sess("LOOP-2229", "pair", r#"{"code":"PA"}"#))
        .unwrap();
    let m = st.append_message(&s.id, text("user", "hi")).unwrap();
    st.write_session_context(&s.id, "sum", &m.id).unwrap();
    st.delete_workspace("LOOP-2229").unwrap();
    assert!(st.get_session(&s.id).is_none());
    assert_eq!(query_i64(&st, "SELECT COUNT(*) FROM session_message"), 0);
    assert_eq!(query_i64(&st, "SELECT COUNT(*) FROM session_context"), 0);
}
