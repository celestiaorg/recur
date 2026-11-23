use chrono::{Local, TimeZone as _};
use recur_domain::insights::{LostKind, TimeLost};
use recur_domain::{
    BLOCK_COMPACT, ContentBlock, DiffStat, Message, PROVIDER_CLAUDE_CODE, Pr, STREAM_ERROR,
    Session, Workspace, WorkspaceState,
};

use super::*;

const M: i64 = 60_000;

fn session(st: &Store, kind: &str, cfg: &str) -> String {
    st.create_session(Session {
        workspace_id: "w1".into(),
        kind: kind.into(),
        provider: PROVIDER_CLAUDE_CODE.into(),
        config_json: cfg.into(),
        ..Default::default()
    })
    .unwrap()
    .id
}

/// Appends a message and pins its times, as the provider would have stamped them.
fn add(st: &Store, session: &str, m: Message, created: i64, completed: i64, duration: i64) {
    let id = st.append_message(session, m).unwrap().id;
    exec(
        st,
        &format!(
            "UPDATE message SET created_at = {created}, completed_at = NULLIF({completed}, 0),
			duration_ms = NULLIF({duration}, 0) WHERE id = '{id}'"
        ),
    )
    .unwrap();
}

fn prompt(text: &str) -> Message {
    Message {
        role: "user".into(),
        content: Some(vec![ContentBlock::text(text)]),
        ..Default::default()
    }
}

fn assistant(blocks: Vec<ContentBlock>) -> Message {
    Message {
        role: "assistant".into(),
        provider: PROVIDER_CLAUDE_CODE.into(),
        model_id: "claude-opus-5-5".into(),
        content: Some(blocks),
        ..Default::default()
    }
}

fn results(blocks: Vec<ContentBlock>) -> Message {
    Message {
        role: "user".into(),
        provider: PROVIDER_CLAUDE_CODE.into(),
        content: Some(blocks),
        ..Default::default()
    }
}

fn bash(id: &str, cmd: &str) -> ContentBlock {
    ContentBlock::tool_use(
        id,
        "",
        "Bash",
        &serde_json::json!({ "command": cmd }).to_string(),
    )
}

fn pin_call(st: &Store, id: &str, start: i64, end: i64) {
    exec(
        st,
        &format!(
            "UPDATE tool_call SET started_at = {start}, ended_at = {end} WHERE tool_use_id = '{id}'"
        ),
    )
    .unwrap();
}

/// A coder and a reviewer on one workspace, written through the real
/// transcript path, then aggregated over the week they ran in.
#[test]
fn insights_report() {
    let dir = tmp();
    let st = Store::open(&db_file(dir.path())).unwrap();
    st.add_workspace(Workspace {
        id: "w1".into(),
        project_id: "p1".into(),
        ..Default::default()
    })
    .unwrap();
    let start = Local
        .with_ymd_and_hms(2026, 11, 2, 0, 0, 0)
        .unwrap()
        .timestamp_millis();
    let end = start + 7 * 24 * 60 * M;
    let t0 = start + 10 * 60 * M;

    let a = session(&st, "coder", "");
    add(&st, &a, prompt("fix it"), t0, 0, 0);
    add(
        &st,
        &a,
        assistant(vec![
            bash("tu1", "cargo test -p x"),
            bash("tu2", "cd x && cargo build"),
            bash("tu3", "cargo build"),
            bash("tu4", "cargo check"),
            ContentBlock::tool_use("tu5", "", "Grep", r#"{"pattern":"nope"}"#),
        ]),
        t0 + M,
        0,
        0,
    );
    add(
        &st,
        &a,
        results(vec![
            ContentBlock::tool_result(
                "tu1",
                "   Blocking waiting for file lock on build directory\nok",
                false,
            ),
            ContentBlock::tool_result("tu2", "error[E0425]", true),
            ContentBlock::tool_result("tu3", "error[E0425]", true),
            ContentBlock::tool_result("tu4", "error[E0425]", true),
            ContentBlock::tool_result("tu5", "No matches found", false),
        ]),
        t0 + 21 * M,
        0,
        0,
    );
    let mut end_a = assistant(vec![
        ContentBlock::text("done"),
        ContentBlock {
            r#type: BLOCK_COMPACT.into(),
            content_json: "{}".into(),
            ..Default::default()
        },
    ]);
    end_a.input_tokens = 1_000;
    end_a.cache_read_tokens = 9_000;
    end_a.output_tokens = 50;
    end_a.total_input_tokens = 6;
    end_a.total_cache_read_tokens = 62_537;
    end_a.total_cache_creation_tokens = 17_317;
    end_a.total_output_tokens = 147;
    add(&st, &a, end_a, t0 + 58 * M, t0 + 60 * M, 60 * M);
    add(
        &st,
        &a,
        assistant(vec![ContentBlock::tool_use(
            "ask",
            "",
            "mcp__recur__ask_user",
            "{}",
        )]),
        t0 + 59 * M,
        0,
        0,
    );
    add(
        &st,
        &a,
        results(vec![ContentBlock::tool_result(
            "ask",
            "Question delivered",
            false,
        )]),
        t0 + 60 * M,
        0,
        0,
    );
    add(&st, &a, prompt("yes"), t0 + 90 * M, 0, 0);
    let stopped = Message {
        stream_status: STREAM_ERROR.into(),
        ..assistant(vec![ContentBlock::text("Stopped by user")])
    };
    add(&st, &a, stopped, t0 + 95 * M, 0, 0);
    pin_call(&st, "tu1", t0 + M, t0 + 11 * M);
    pin_call(&st, "tu2", t0 + 12 * M, t0 + 13 * M);
    pin_call(&st, "tu3", t0 + 14 * M, t0 + 15 * M);
    pin_call(&st, "tu4", t0 + 16 * M, t0 + 17 * M);
    pin_call(&st, "tu5", t0 + 20 * M, t0 + 21 * M);
    pin_call(&st, "ask", t0 + 60 * M, t0 + 60 * M);
    exec(
        &st,
        &format!(
            "UPDATE message_content SET created_at = {} WHERE type = 'compact';
			UPDATE message SET context_window = 10500 WHERE input_tokens = 1000",
            t0 + 58 * M
        ),
    )
    .unwrap();

    let b = session(&st, "reviewer", r#"{"ownerCode":"PA1"}"#);
    add(&st, &b, prompt("review"), t0 + 30 * M, 0, 0);
    add(
        &st,
        &b,
        assistant(vec![ContentBlock::text("lgtm")]),
        t0 + 88 * M,
        t0 + 90 * M,
        60 * M,
    );

    let iso = |ms: i64| {
        chrono::DateTime::from_timestamp_millis(ms)
            .unwrap()
            .to_rfc3339()
    };
    st.add_pr(Pr {
        id: "pr1".into(),
        workspace_id: "w1".into(),
        state: "Merged".into(),
        updated_at: iso(t0 + 100 * M),
        diff: DiffStat { add: 10, del: 2 },
        file_count: 3,
        ..Default::default()
    })
    .unwrap();
    st.add_pr(Pr {
        id: "pr2".into(),
        workspace_id: "w1".into(),
        state: "Open".into(),
        ..Default::default()
    })
    .unwrap();

    let r = st.insights(start - 7 * 24 * 60 * M, start, end).unwrap();
    assert_eq!(r.agent_ms, 150 * M, "two 60m turns and a 30m wait");
    assert_eq!(r.prev_agent_ms, 0);
    assert_eq!((r.peak_agents, r.peak_at_ms), (2, t0 + 30 * M));
    assert_eq!(
        (r.split.commands_ms, r.split.model_ms, r.split.waiting_ms),
        (14 * M, 106 * M, 30 * M)
    );
    assert_eq!(r.by_hour[10], 90 * M);
    assert_eq!(r.by_hour[11], 60 * M);
    assert_eq!(r.by_weekday[0], 150 * M, "one Monday in the week");
    assert_eq!(r.days.len(), 371);
    assert_eq!(r.days[371 - 7].agent_ms, 150 * M);

    let t = &r.tasks;
    assert_eq!(
        (t.total, t.finished, t.stopped, t.failed, t.running),
        (3, 2, 1, 0, 0)
    );
    assert_eq!(
        (t.from_you, t.from_agent, t.from_schedule, t.projects),
        (2, 1, 0, 1)
    );

    assert_eq!(r.asks.count, 1);
    assert_eq!(r.asks.typical_wait_ms, Some(30 * M));
    assert_eq!(r.asks.longest_wait_ms, Some(30 * M));

    assert_eq!(
        r.lost,
        vec![
            TimeLost {
                kind: LostKind::BuildLock,
                ms: 10 * M,
                count: 1
            },
            TimeLost {
                kind: LostKind::RepeatedFailure,
                ms: 2 * M,
                count: 1
            },
        ]
    );

    let cmd = |n: &str| r.commands.iter().find(|c| c.name == n).unwrap();
    assert_eq!(r.commands[0].name, "cargo test");
    assert_eq!(r.commands.len(), 3, "the ask is waiting, not a command");
    let build = cmd("cargo build / check");
    assert_eq!(
        (build.calls, build.total_ms, build.failed, build.p50_ms),
        (3, 3 * M, 3, M)
    );
    let search = cmd("Search (rg, grep)");
    assert_eq!((search.failed, search.no_match), (0, 1));

    assert_eq!(r.models.len(), 1);
    let m = &r.models[0];
    assert_eq!(
        (m.model.as_str(), m.provider.as_str()),
        ("opus 5.5", "claude-code")
    );
    assert_eq!(
        (m.agent_ms, m.runs, m.commands, m.failed_commands),
        (120 * M, 2, 5, 3)
    );
    assert_eq!(
        (m.typical_task_ms, m.ms_per_ask),
        (None, None),
        "under 10 tasks"
    );
    assert_eq!(
        m.tokens_in,
        Some(79_860),
        "billed totals, not the footprint"
    );

    assert_eq!(
        (
            r.context.compactions,
            r.context.turns_over_90,
            r.context.largest
        ),
        (1, 1, 10_000)
    );
    assert_eq!(
        (r.tokens.input, r.tokens.cached, r.tokens.output),
        (79_860, 62_537, 147),
        "the reviewer's turn has no totals and is not counted"
    );
    assert_eq!(
        (
            r.handoffs.to_reviewer,
            r.handoffs.review_loops,
            r.handoffs.review_settled
        ),
        (1, 1, 1)
    );
    assert_eq!(
        (
            r.merged.total,
            r.merged.by_pr,
            r.merged.direct,
            r.merged.open_prs
        ),
        (1, 1, 0, 1)
    );
    assert_eq!(
        (r.changes.added, r.changes.removed, r.changes.files),
        (10, 2, 3)
    );

    // A direct land is recorded when the workspace turns merged; a child
    // folding into its parent is not a landing.
    for (id, parent) in [("w2", ""), ("w3", "w2")] {
        st.add_workspace(Workspace {
            id: id.into(),
            parent_id: parent.into(),
            ..Default::default()
        })
        .unwrap();
        st.update_workspace(id, |w| w.state = WorkspaceState::Merged);
    }
    let now = chrono::Utc::now().timestamp_millis();
    let r = st
        .insights(now - 180 * M, now - 60 * M, now + 60 * M)
        .unwrap();
    assert_eq!((r.merged.by_pr, r.merged.direct), (0, 1));
    assert_eq!(r.agent_ms, 0);
}

/// A quit mid-turn: startup recovery marks the partial reply an error at
/// relaunch and answers the open call with a provider-less carrier. Neither is
/// activity. An ask the next prompt answers a day later was not answered.
#[test]
fn insights_skip_recovery_and_stale_asks() {
    let dir = tmp();
    let st = Store::open(&db_file(dir.path())).unwrap();
    let start = Local
        .with_ymd_and_hms(2026, 11, 2, 0, 0, 0)
        .unwrap()
        .timestamp_millis();
    let end = start + 7 * 24 * 60 * M;
    let t0 = start + 10 * 60 * M;
    let relaunch = t0 + 600 * M;

    let s = session(&st, "coder", "");
    add(&st, &s, prompt("go"), t0, 0, 0);
    let cut = Message {
        stream_status: STREAM_ERROR.into(),
        ..assistant(vec![
            ContentBlock::text("Running the te"),
            bash("tu1", "cargo test"),
        ])
    };
    add(&st, &s, cut, t0 + M, relaunch, 0);
    exec(
        &st,
        &format!(
            "UPDATE tool_call SET started_at = {} WHERE tool_use_id = 'tu1'",
            t0 + M
        ),
    )
    .unwrap();
    let carrier = Message {
        role: "user".into(),
        content: Some(vec![ContentBlock::tool_result("tu1", "Interrupted", true)]),
        ..Default::default()
    };
    add(&st, &s, carrier, relaunch, 0, 0);
    assert_eq!(
        query_i64(&st, "SELECT COUNT(*) FROM tool_call WHERE ended_at IS NULL"),
        1,
        "the carrier leaves the call open"
    );

    let ask = relaunch + M;
    add(
        &st,
        &s,
        assistant(vec![ContentBlock::tool_use(
            "ask",
            "",
            "mcp__recur__ask_user",
            "{}",
        )]),
        ask,
        0,
        0,
    );
    add(
        &st,
        &s,
        results(vec![ContentBlock::tool_result(
            "ask",
            "Question delivered",
            false,
        )]),
        ask,
        0,
        0,
    );
    pin_call(&st, "ask", ask, ask);
    add(&st, &s, prompt("something else"), ask + 20 * 60 * M, 0, 0);

    let r = st.insights(start - 7 * 24 * 60 * M, start, end).unwrap();
    let t = &r.tasks;
    assert_eq!(
        (t.total, t.stopped, t.failed, t.finished),
        (2, 1, 0, 1),
        "the carrier is no task and the cut-off turn is stopped"
    );
    let test = r.commands.iter().find(|c| c.name == "cargo test").unwrap();
    assert_eq!((test.total_ms, test.failed), (0, 0));
    assert_eq!(r.asks.count, 1);
    assert_eq!(r.asks.longest_wait_ms, None);
    assert_eq!(r.agent_ms, 0);
    assert!(r.lost.is_empty(), "{:?}", r.lost);

    let now = chrono::Utc::now().timestamp_millis();
    let r = st
        .insights(
            now - 31 * 24 * 60 * M,
            now - 24 * 60 * M,
            now + 30 * 24 * 60 * M,
        )
        .unwrap();
    assert_eq!(
        r.days.last().unwrap().date,
        Local::now().format("%Y-%m-%d").to_string(),
        "the in-progress period ends today"
    );
}

/// ask_user parks its turn: the provider is killed, so nothing stamps a
/// duration. The work before the ask still counts, and a period in progress
/// compares with the same stretch of the previous one.
#[test]
fn insights_parked_turns_and_to_date_comparison() {
    let dir = tmp();
    let st = Store::open(&db_file(dir.path())).unwrap();
    let start = Local
        .with_ymd_and_hms(2026, 11, 9, 0, 0, 0)
        .unwrap()
        .timestamp_millis();
    let prev_start = start - 7 * 24 * 60 * M;
    let t0 = start + 10 * 60 * M;
    let s = session(&st, "coder", "");

    // Last Monday's turn is in the to-date window; last Saturday's is not.
    add(&st, &s, prompt("before"), prev_start + 10 * 60 * M, 0, 0);
    let e = prev_start + 11 * 60 * M;
    add(
        &st,
        &s,
        assistant(vec![ContentBlock::text("ok")]),
        e,
        e,
        30 * M,
    );
    add(&st, &s, prompt("weekend"), start - 30 * 60 * M, 0, 0);
    let e = start - 29 * 60 * M;
    add(
        &st,
        &s,
        assistant(vec![ContentBlock::text("ok")]),
        e,
        e,
        60 * M,
    );

    add(&st, &s, prompt("fix it"), t0, 0, 0);
    let ask = t0 + 40 * M;
    add(
        &st,
        &s,
        assistant(vec![ContentBlock::tool_use(
            "ask",
            "",
            "mcp__recur__ask_user",
            "{}",
        )]),
        ask,
        0,
        0,
    );
    add(
        &st,
        &s,
        results(vec![ContentBlock::tool_result(
            "ask",
            "Question delivered",
            false,
        )]),
        ask,
        0,
        0,
    );
    pin_call(&st, "ask", ask, ask);
    add(&st, &s, prompt("yes"), t0 + 50 * M, 0, 0);
    let e = t0 + 60 * M;
    add(
        &st,
        &s,
        assistant(vec![ContentBlock::text("done")]),
        e,
        e,
        10 * M,
    );

    let r = st.insights(prev_start, start, t0 + 120 * M).unwrap();
    assert_eq!(r.agent_ms, 60 * M, "40m parked, a 10m wait, a 10m turn");
    assert_eq!(r.split.waiting_ms, 10 * M);
    assert_eq!(r.split.model_ms, 50 * M);
    assert_eq!(r.models[0].agent_ms, 50 * M);
    assert_eq!(r.models[0].runs, 1);
    assert_eq!((r.peak_agents, r.peak_at_ms), (1, t0));
    assert_eq!(r.prev_agent_ms, 30 * M, "Monday to Monday, not the weekend");
}

/// 0019 backfills merges and queues tool calls written before it; the job
/// fills them in windows, resumably, while the report says it is preparing.
#[test]
fn insights_migration_backfill() {
    let dir = tmp();
    let path = db_file(dir.path());
    Store::open(&path).unwrap().close().unwrap();
    let use_ = |id: &str, name: &str, input: &str| {
        format!(
            r#"{{"id":"{id}","provider_tool_use_id":"p-{id}","name":"{name}","input":{input}}}"#
        )
    };
    let result = |id: &str, text: &str, err: bool| {
        serde_json::json!({ "tool_use_id": id, "result": text, "is_error": err }).to_string()
    };
    let blocks = [
        (
            "c1",
            "tool_use",
            use_("t1", "Bash", r#"{"command":"cd x && cargo test"}"#),
            1_000,
        ),
        (
            "c2",
            "tool_use",
            use_("t2", "grep", r#"{"pattern":"x"}"#),
            2_000,
        ),
        (
            "c3",
            "tool_use",
            use_("t3", "shell", r#"{"command":"git push"}"#),
            3_000,
        ),
        (
            "c4",
            "tool_result",
            result("t1", "Blocking waiting for file lock", true),
            5_000,
        ),
        (
            "c5",
            "tool_result",
            result("t2", "No matches found", false),
            2_500,
        ),
    ];
    let mut stmts = vec![
        "DROP TABLE tool_call".to_owned(),
        "DROP TABLE tool_call_backfill".to_owned(),
        "DROP TABLE workspace_merge".to_owned(),
        "DROP INDEX idx_message_turn_end".to_owned(),
        "DROP INDEX idx_message_prompt".to_owned(),
        "DROP INDEX idx_content_compact".to_owned(),
        "DROP INDEX idx_events_type_ts".to_owned(),
        "DELETE FROM __migrations WHERE version = 20".to_owned(),
        "INSERT INTO session (id, kind, context_mode, provider, created_at, updated_at)
			VALUES ('s1', 'coder', 'provider', 'claude-code', 0, 0)"
            .to_owned(),
        "INSERT INTO message (id, origin_session_id, role, provider, created_at)
			VALUES ('m1', 's1', 'assistant', 'claude-code', 0)"
            .to_owned(),
        r#"INSERT INTO prs (id, doc_json) VALUES
			('pr1', '{"workspaceId":"w1","state":"Merged","updatedAt":"2026-11-02T10:00:00Z"}'),
			('pr2', '{"workspaceId":"w2","state":"Open","updatedAt":"2026-11-02T10:00:00Z"}')"#
            .to_owned(),
        r#"INSERT INTO workspaces (id, doc_json) VALUES ('w4', '{"id":"w4","parentId":"w3"}')"#
            .to_owned(),
        r#"INSERT INTO events (workspace_id, type, payload_json, ts) VALUES
			('w3', 'workspace.event', '{"event":{"kind":"outcome","text":"Squash and merge into <code>main</code> on disk: no PR."}}', 7),
			('w3', 'workspace.event', '{"event":{"kind":"outcome","text":"Squash and merge to <code>origin/main</code>."}}', 9),
			('w4', 'workspace.event', '{"event":{"kind":"outcome","text":"Squash and merge into <code>w3</code> on disk: no PR."}}', 8),
			('w5', 'workspace.event', '{"event":{"kind":"outcome","text":"Local merge failed: dirty"}}', 8)"#
            .to_owned(),
    ];
    for (i, (id, typ, json, at)) in blocks.iter().enumerate() {
        stmts.push(format!(
            "INSERT INTO message_content (id, message_id, idx, type, content_json, created_at)
			VALUES ('{id}', 'm1', {i}, '{typ}', '{json}', {at})"
        ));
    }
    raw(&path, &stmts.iter().map(String::as_str).collect::<Vec<_>>());

    let st = Store::open(&path).unwrap();
    assert_eq!(query_i64(&st, "SELECT COUNT(*) FROM tool_call"), 0);
    assert_eq!(st.insights(0, 0, 10_000).unwrap().preparing, Some(0.));
    crate::tool_calls::backfill_tool_calls(db(&st), 2).unwrap();
    assert_eq!(st.insights(0, 0, 10_000).unwrap().preparing, None);
    let rows = |q: &str| -> Vec<String> {
        db(&st)
            .with(|c| {
                let mut q = c.prepare(q).unwrap();
                let rows = q.query_map([], |r| r.get(0)).unwrap();
                Ok(rows.map(Result::unwrap).collect())
            })
            .unwrap()
    };
    assert_eq!(
        rows(
            "SELECT concat_ws(' ', tool_use_id, label, started_at, ended_at, is_error,
				no_match, build_lock) FROM tool_call ORDER BY started_at"
        ),
        [
            "t1 cargo test 1000 5000 1 0 1",
            "t2 Search (rg, grep) 2000 2500 0 1 0",
            "t3 git, gh 3000 0 0 0",
        ],
        "id label started ended is_error no_match build_lock"
    );
    let calls = rows("SELECT concat_ws(' ', id, tool_use_id, ended_at) FROM tool_call ORDER BY 1");
    exec(
        &st,
        "INSERT INTO tool_call_backfill (id, next_rowid, end_rowid)
			SELECT 1, 0, MAX(rowid) FROM message_content",
    )
    .unwrap();
    crate::tool_calls::backfill_tool_calls(db(&st), 3).unwrap();
    assert_eq!(
        rows("SELECT concat_ws(' ', id, tool_use_id, ended_at) FROM tool_call ORDER BY 1"),
        calls,
        "a rerun changes nothing"
    );
    assert_eq!(
        query_i64(
            &st,
            "SELECT COUNT(*) FROM tool_call WHERE session_id = 's1' AND message_id = 'm1'
				AND length(id) = 36 AND substr(id, 15, 1) = '7'"
        ),
        3,
        "UUIDv7 ids on the right session and message"
    );
    assert_eq!(
        rows(
            "SELECT concat_ws(' ', workspace_id, via, merged_at) FROM workspace_merge
				ORDER BY workspace_id"
        ),
        ["w1 pr 1793613600000", "w3 direct 7"]
    );
}
