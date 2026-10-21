//! Compatibility with the Go store, against fixtures from
//! `tests/fixtures/gen_store.go.txt`:
//!
//! - `go_store.db`, populated by the Go store, reads back here exactly as Go
//!   reads it (`go_dump.json`), its JSON columns re-encode byte for byte,
//!   and rewriting every row through recur-store leaves every table
//!   unchanged.
//! - `rust_written.db` (made by `generate_rust_written_db` below from a copy
//!   of `go_store.db`) passes the Go verifier, and Go reads it exactly as
//!   recur-store does (`go_reads_rust.json`).
//! - A fresh Rust database has the same schema and migration bookkeeping as
//!   a fresh Go one (`go_schema.json`).
//!
//! `dump` must stay in step with `rustFixtureDump` in the generator.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use recur_domain::json::to_string as marshal;
use recur_domain::*;
use rusqlite::Connection;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::*;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn copy_db(name: &str, dir: &Path) -> PathBuf {
    let dst = db_file(dir);
    std::fs::copy(fixtures().join(name), &dst).unwrap();
    dst
}

fn read_json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures().join(name)).unwrap()).unwrap()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WsDump {
    agents: Vec<Agent>,
    feed: Option<Vec<FeedItem>>,
    findings: Vec<Finding>,
    open_findings: Option<Vec<Finding>>,
    escalation: Option<Escalation>,
    has_open_escalation: bool,
    events: Option<Vec<StoredEvent>>,
    sessions: Option<Vec<Session>>,
    last_assistant_text: String,
    #[serde(rename = "prByWorkspace")]
    pr_by_workspace: Option<Pr>,
    #[serde(rename = "openPRByWorkspace")]
    open_pr_by_workspace: Option<Pr>,
}

#[derive(Serialize)]
struct RuleDump {
    require: bool,
    #[serde(rename = "atMs")]
    at_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Dump {
    project_groups: Option<Vec<ProjectGroup>>,
    projects: Option<Vec<Project>>,
    workspaces: Vec<Workspace>,
    by_workspace: BTreeMap<String, WsDump>,
    prs: Vec<Pr>,
    pr_by_id: BTreeMap<String, Pr>,
    inbox: Option<Vec<InboxItem>>,
    policy: Policy,
    settings: Settings,
    resource: Resource,
    attention: AttentionCounts,
    unread_count: usize,
    unseen_chat_count: usize,
    pr_number_seq: i64,
    github_branch_rules: BTreeMap<String, RuleDump>,
    latest_analysis: BTreeMap<String, AnalysisRun>,
    app_sessions: Option<Vec<Session>>,
    transcripts: BTreeMap<String, Option<Vec<Message>>>,
    active_contexts: BTreeMap<String, Option<SessionContext>>,
    first_user_prompts: BTreeMap<String, String>,
    session_headers: Option<BTreeMap<String, Vec<Session>>>,
    #[serde(rename = "latestEventTS")]
    latest_event_ts: Option<BTreeMap<String, i64>>,
    schedules: Vec<ScheduledPrompt>,
    schedule_runs: Vec<ScheduledPromptRun>,
    active_runs: Vec<ScheduledPromptRun>,
}

/// Everything the store reads back, encoded like Go's `json.Marshal`.
fn dump(s: &Store) -> Value {
    let mut ws_ids: BTreeSet<String> = s.workspaces().into_iter().map(|w| w.id).collect();
    let (seq, rules, project_ids) = {
        let st = s.state.read();
        ws_ids.extend(st.agents.keys().cloned());
        ws_ids.extend(st.feed.keys().cloned());
        ws_ids.extend(st.findings.keys().cloned());
        let rules = st
            .github_branch_rules
            .iter()
            .map(|(k, r)| {
                (
                    k.clone(),
                    RuleDump {
                        require: r.require,
                        at_ms: r.at.timestamp_millis(),
                    },
                )
            })
            .collect();
        let ids: Vec<String> = st.projects.iter().map(|p| p.id.clone()).collect();
        (st.pr_number_seq, rules, ids)
    };
    ws_ids.extend(s.session_headers_by_workspace().unwrap_or_default().into_keys());

    let mut session_ids = Vec::new();
    let mut by_workspace = BTreeMap::new();
    for id in &ws_ids {
        let sessions = s.sessions_for_loop(id);
        session_ids.extend(sessions.iter().flatten().map(|x| x.id.clone()));
        by_workspace.insert(
            id.clone(),
            WsDump {
                agents: s.agents(id),
                feed: s.feed(id),
                findings: s.findings(id),
                open_findings: s.open_findings(id),
                escalation: s.escalation(id),
                has_open_escalation: s.has_open_escalation(id),
                events: s.events_for_loop(id).unwrap(),
                sessions,
                last_assistant_text: s.last_assistant_text(id),
                pr_by_workspace: s.pr_by_workspace(id),
                open_pr_by_workspace: s.open_pr_by_workspace(id),
            },
        );
    }
    let app_sessions = s.sessions_for_app_kind(SESSION_KIND_APP);
    session_ids.extend(app_sessions.iter().flatten().map(|x| x.id.clone()));
    session_ids.sort();
    let d = Dump {
        project_groups: s.project_groups(),
        projects: s.projects(),
        workspaces: s.workspaces(),
        by_workspace,
        prs: s.prs(),
        pr_by_id: s.prs().into_iter().map(|p| (p.id.clone(), s.pr(&p.id).unwrap())).collect(),
        inbox: s.inbox(),
        policy: s.policy(),
        settings: s.settings(),
        resource: s.resource(),
        attention: s.attention_counts(),
        unread_count: s.unread_count(),
        unseen_chat_count: s.unseen_chat_count(),
        pr_number_seq: seq,
        github_branch_rules: rules,
        latest_analysis: project_ids
            .iter()
            .filter_map(|id| s.latest_analysis(id, "").map(|a| (id.clone(), a)))
            .collect(),
        app_sessions,
        transcripts: session_ids
            .iter()
            .map(|id| (id.clone(), s.session_transcript(id).unwrap()))
            .collect(),
        active_contexts: session_ids
            .iter()
            .map(|id| (id.clone(), s.active_session_context(id)))
            .collect(),
        first_user_prompts: session_ids
            .iter()
            .map(|id| (id.clone(), s.session_first_user_prompt(id)))
            .collect(),
        session_headers: s.session_headers_by_workspace(),
        latest_event_ts: s.latest_event_ts_by_workspace(),
        schedules: s.scheduled_prompts().unwrap(),
        schedule_runs: s.scheduled_prompt_runs().unwrap(),
        active_runs: s.active_scheduled_prompt_runs().unwrap(),
    };
    serde_json::from_str(&marshal(&d).unwrap()).unwrap()
}

/// Reports the first differing path, so a mismatch is readable.
fn assert_same(got: &Value, want: &Value, path: &str) {
    match (got, want) {
        (Value::Object(g), Value::Object(w)) => {
            let keys: BTreeSet<&String> = g.keys().chain(w.keys()).collect();
            for k in keys {
                match (g.get(k), w.get(k)) {
                    (Some(a), Some(b)) => assert_same(a, b, &format!("{path}.{k}")),
                    (a, b) => panic!("{path}.{k}: got {a:?}, want {b:?}"),
                }
            }
        }
        (Value::Array(g), Value::Array(w)) => {
            assert_eq!(g.len(), w.len(), "{path}: length");
            for (i, (a, b)) in g.iter().zip(w).enumerate() {
                assert_same(a, b, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(got, want, "{path}"),
    }
}

/// Every row of every table, as text, ordered.
fn table_dump(path: &Path) -> BTreeMap<String, Vec<Vec<String>>> {
    let c = Connection::open(path).unwrap();
    let tables: Vec<String> = {
        let mut stmt = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let mut out = BTreeMap::new();
    for t in tables {
        if t == "__migrations" {
            continue;
        }
        let mut stmt = c.prepare(&format!("SELECT * FROM {t}")).unwrap();
        let n = stmt.column_count();
        let mut rows: Vec<Vec<String>> = stmt
            .query_map([], |r| {
                (0..n)
                    .map(|i| {
                        Ok(match r.get_ref(i)? {
                            rusqlite::types::ValueRef::Null => "NULL".to_owned(),
                            rusqlite::types::ValueRef::Integer(v) => v.to_string(),
                            rusqlite::types::ValueRef::Real(v) => v.to_string(),
                            rusqlite::types::ValueRef::Text(v) | rusqlite::types::ValueRef::Blob(v) => {
                                String::from_utf8_lossy(v).into_owned()
                            }
                        })
                    })
                    .collect()
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows.sort();
        out.insert(t, rows);
    }
    out
}

fn schema_dump(path: &Path) -> Value {
    let c = Connection::open(path).unwrap();
    let mut stmt = c
        .prepare("SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")
        .unwrap();
    let objects: Vec<BTreeMap<&str, String>> = stmt
        .query_map([], |r| {
            Ok(BTreeMap::from([
                ("type", r.get(0)?),
                ("name", r.get(1)?),
                ("tbl_name", r.get(2)?),
                ("sql", r.get(3)?),
            ]))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut stmt = c.prepare("SELECT version, name FROM __migrations ORDER BY version").unwrap();
    let migrations: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(serde_json::json!({"version": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?}))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    serde_json::json!({"objects": objects, "migrations": migrations})
}

fn isolated_home(name: &str, body: impl FnOnce()) {
    let home = tmp();
    crate::testenv::run(module_path!(), name, &[("HOME", home.path())], body);
}

#[test]
fn go_db_reads_back_identically() {
    isolated_home("go_db_reads_back_identically", || {
        let dir = tmp();
        let path = copy_db("go_store.db", dir.path());
        let st = Store::open(&path).unwrap();
        assert_same(&dump(&st), &read_json("go_dump.json"), "go_store.db");
    });
}

fn reencodes<T: DeserializeOwned + Serialize>(table: &str, doc: &str) {
    let v: T = serde_json::from_str(doc).unwrap_or_else(|e| panic!("{table}: {e}\n{doc}"));
    assert_eq!(marshal(&v).unwrap(), doc, "{table} re-encode");
}

/// The JSON Go stored in each column decodes into the domain type and
/// re-encodes to the identical bytes, so recur-store writes what Go wrote.
#[test]
fn go_json_columns_reencode_byte_for_byte() {
    let c = Connection::open(fixtures().join("go_store.db")).unwrap();
    let docs = |q: &str| -> Vec<String> {
        let mut stmt = c.prepare(q).unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let mut n = 0;
    let mut each = |q: &str, f: &dyn Fn(&str)| {
        let rows = docs(q);
        assert!(!rows.is_empty(), "{q}");
        n += rows.len();
        rows.iter().for_each(|d| f(d));
    };
    each("SELECT doc_json FROM workspaces", &|d| reencodes::<Workspace>("workspaces", d));
    each("SELECT doc_json FROM prs", &|d| reencodes::<Pr>("prs", d));
    each("SELECT doc_json FROM inbox", &|d| reencodes::<InboxItem>("inbox", d));
    each("SELECT doc_json FROM agents", &|d| reencodes::<Agent>("agents", d));
    each("SELECT doc_json FROM feed", &|d| reencodes::<FeedItem>("feed", d));
    each("SELECT doc_json FROM escalations", &|d| reencodes::<Escalation>("escalations", d));
    each("SELECT doc_json FROM analysis", &|d| reencodes::<AnalysisRun>("analysis", d));
    each("SELECT doc_json FROM policy", &|d| reencodes::<Policy>("policy", d));
    each("SELECT doc_json FROM settings", &|d| reencodes::<Settings>("settings", d));
    each("SELECT doc_json FROM resource", &|d| reencodes::<Resource>("resource", d));
    each("SELECT extra_json FROM projects", &|d| reencodes::<Project>("projects.extra_json", d));
    let typed = [
        (BLOCK_TEXT, "text"),
        (BLOCK_THINKING, "thinking"),
        (BLOCK_TOOL_USE, "tool_use"),
        (BLOCK_TOOL_RESULT, "tool_result"),
        (BLOCK_IMAGE, "image"),
        (BLOCK_COMPACT, "compact"),
        (BLOCK_FALLBACK, "fallback"),
    ];
    for (typ, label) in typed {
        let q = format!("SELECT content_json FROM message_content WHERE type = '{typ}'");
        each(&q, &|d| match typ {
            BLOCK_TEXT => reencodes::<TextPayload>(label, d),
            BLOCK_THINKING => reencodes::<ThinkingPayload>(label, d),
            BLOCK_TOOL_USE => reencodes::<ToolUsePayload>(label, d),
            BLOCK_TOOL_RESULT => reencodes::<ToolResultPayload>(label, d),
            BLOCK_IMAGE => reencodes::<ImagePayload>(label, d),
            BLOCK_COMPACT => reencodes::<CompactPayload>(label, d),
            _ => reencodes::<FallbackPayload>(label, d),
        });
    }
    assert!(n > 40, "{n} documents");
}

/// Writing every loaded row back through recur-store leaves every table of
/// the Go database byte-identical.
#[test]
fn rust_rewrite_of_go_db_changes_nothing() {
    isolated_home("rust_rewrite_of_go_db_changes_nothing", || {
        let dir = tmp();
        let path = copy_db("go_store.db", dir.path());
        let before = table_dump(&path);
        let st = Store::open(&path).unwrap();
        let snapshot = {
            let s = st.state.read();
            (
                s.project_groups.clone(),
                s.projects.clone(),
                s.findings.clone(),
                s.github_branch_rules.clone(),
                s.pr_number_seq,
                s.analysis.clone(),
            )
        };
        let (groups, projects, findings, rules, seq, analysis) = snapshot;
        let d = db(&st);
        for (i, g) in groups.iter().enumerate() {
            d.upsert_project_group(g, i).unwrap();
        }
        for (i, p) in projects.iter().enumerate() {
            d.upsert_project(p, i).unwrap();
        }
        for list in findings.values() {
            for (i, f) in list.iter().enumerate() {
                d.upsert_finding(f, i).unwrap();
            }
        }
        for (k, r) in &rules {
            let mut parts = k.split('\x00');
            let (p, repo, branch) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
            d.upsert_github_branch_rule(p, repo, branch, *r).unwrap();
        }
        d.set_pr_number_seq(seq).unwrap();
        for a in analysis.iter().rev() {
            st.add_analysis(a.clone());
        }
        for w in st.workspaces() {
            st.update_workspace(&w.id, |_| {}).unwrap();
            for a in st.agents(&w.id) {
                st.upsert_agent(&w.id, a);
            }
        }
        for p in st.prs() {
            st.update_pr(&p.id, |_| {}).unwrap();
        }
        for it in st.inbox().unwrap_or_default() {
            st.add_inbox_item(it);
        }
        let escalations: Vec<Escalation> = st.state.read().escalations.values().cloned().collect();
        for e in escalations {
            st.add_escalation(e);
        }
        st.set_policy(st.policy());
        st.set_settings(st.settings());
        st.set_resource(st.resource());
        st.close().unwrap();
        let after = table_dump(&path);
        for (t, rows) in &before {
            pretty_assertions::assert_eq!(rows, &after[t], "table {t}");
        }
    });
}

#[test]
fn rust_written_db_reads_like_go() {
    isolated_home("rust_written_db_reads_like_go", || {
        let dir = tmp();
        let path = copy_db("rust_written.db", dir.path());
        let st = Store::open(&path).unwrap();
        assert_same(&dump(&st), &read_json("go_reads_rust.json"), "rust_written.db");
    });
}

/// A fresh Rust-created database has Go's schema and bookkeeping.
#[test]
fn fresh_schema_matches_go() {
    let dir = tmp();
    Store::open(&db_file(dir.path())).unwrap().close().unwrap();
    let got = schema_dump(&db_file(dir.path()));
    assert_same(&got, &read_json("go_schema.json"), "schema");
}

/// Writes `tests/fixtures/rust_written.db`: a copy of `go_store.db` after
/// recur-store mutations of every kind. Regenerate with
/// `RECUR_RUST_FIXTURE_DIR=$PWD/crates/store/tests/fixtures cargo test -p recur-store generate_rust_written_db -- --ignored`,
/// then run the Go verifier (see `gen_store.go.txt`).
#[test]
#[ignore]
fn generate_rust_written_db() {
    let out = std::env::var("RECUR_RUST_FIXTURE_DIR").expect("RECUR_RUST_FIXTURE_DIR");
    let dir = tmp();
    let path = copy_db("go_store.db", dir.path());
    let st = Store::open(&path).unwrap();
    populate_from_rust(&st);
    st.close().unwrap();
    drop(st);
    std::fs::copy(&path, Path::new(&out).join("rust_written.db")).unwrap();
}

fn populate_from_rust(st: &Store) {
    st.add_project_group(ProjectGroup {
        id: "rust-grp".into(),
        name: "Rust group".into(),
        open: true,
        ..Default::default()
    })
    .unwrap();
    st.add_project(Project {
        id: "rust-proj".into(),
        name: "rust".into(),
        repo_path: "/nonexistent/recur-fixture/rust".into(),
        base_branch: "main".into(),
        color: "#00aaff".into(),
        pr_on_github: true,
        merge_method: MergeMethod::Rebase,
        enabled_skills: vec!["rusty".into()],
        group_id: "rust-grp".into(),
        ..Default::default()
    })
    .unwrap();
    st.add_workspace(Workspace {
        id: "W-rust".into(),
        project_id: "rust-proj".into(),
        group: Group::Active,
        goal: "Rust <fixture> & co".into(),
        chat_tab_order: vec!["PA".into(), "PA2".into()],
        agent_codes: Some(vec!["PA".into()]),
        progress: 0.5,
        plan: Some(Plan {
            summary: "rust plan".into(),
            steps: vec![PlanStep {
                id: "r1".into(),
                title: "Rust step".into(),
                status: "doing".into(),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    st.delete_closed_chat("W-rust", "PA2").unwrap();
    st.delete_workspace("LOOP-2235").unwrap();
    st.update_workspace("LOOP-2231", |w| w.phase = "rust phase".into()).unwrap();
    st.upsert_agent(
        "W-rust",
        Agent {
            id: "W-rust-PA".into(),
            workspace_id: "W-rust".into(),
            code: "PA".into(),
            unseen: true,
            session_id: "rust-sess".into(),
            ..Default::default()
        },
    );
    for (i, id) in ["r1", "r2", "r3"].into_iter().enumerate() {
        st.prepend_feed(
            "W-rust",
            FeedItem {
                id: id.into(),
                text: format!("rust feed {id}"),
                tool_use_id: format!("tu-{id}"),
                ts: 500 + i as i64,
                ..Default::default()
            },
        );
    }
    st.patch_feed_by_tool_use_id("W-rust", "tu-r2", |it| it.tool_result = "rust patched".into())
        .unwrap();
    let f = st.add_finding(Finding {
        id: "F-rust-1".into(),
        workspace_id: "W-rust".into(),
        file: "lib.rs".into(),
        line: 1,
        severity: Severity::Critical,
        message: "unsafe <block>".into(),
        status: "open".into(),
        ..Default::default()
    });
    st.add_finding_note("W-rust", &f.id, "human note", "").unwrap();
    st.add_finding_note("W-rust", &f.short_id, "coder note", FINDING_ACTOR_CODER).unwrap();
    st.add_finding(Finding {
        id: "F-rust-2".into(),
        workspace_id: "W-rust".into(),
        file: "main.rs".into(),
        severity: Severity::Info,
        message: "style".into(),
        status: "open".into(),
        ..Default::default()
    });
    st.set_finding_status("W-rust", "F-rust-2", "ignored").unwrap();
    st.resolve_findings("W-go");
    st.add_escalation(Escalation {
        id: "esc-rust".into(),
        workspace_id: "W-rust".into(),
        status: "open".into(),
        question: "Rust?".into(),
        ..Default::default()
    });
    assert!(st.answer_escalation("W-rust", "", "rust answer").1);
    let n = st.next_pr_number();
    assert_eq!(n, 481);
    st.add_pr(Pr {
        id: "rust-pr".into(),
        workspace_id: "W-rust".into(),
        project_id: "rust-proj".into(),
        number: n,
        source: PrSource::Local,
        title: "Rust PR".into(),
        state: "In review".into(),
        ..Default::default()
    })
    .unwrap();
    st.add_inbox_item(InboxItem {
        id: "ib-rust".into(),
        kind: "pr".into(),
        unread: true,
        title: "<b>rust</b>".into(),
        target: "pr:rust-pr".into(),
        workspace_id: "W-rust".into(),
        ..Default::default()
    });
    let mut pol = st.policy();
    pol.autonomy = AutonomyLevel::Cautious;
    st.set_policy(pol);
    let mut set = st.settings();
    set.max_loops = 6;
    st.set_settings(set);
    st.set_resource(Resource {
        iters_today: 99,
        ..Default::default()
    });
    st.set_github_branch_require_pr("rust-proj", "acme/rust", "main", true);
    st.add_analysis(AnalysisRun {
        id: "an-rust".into(),
        project_id: "rust-proj".into(),
        root: "/r".into(),
        when: "now".into(),
        ts: 777,
        summary: "rust".into(),
        ..Default::default()
    });
    for (i, t) in ["workspace.event", "workspace.updated"].into_iter().enumerate() {
        st.append_event(&StoredEvent {
            workspace_id: "W-rust".into(),
            r#type: t.into(),
            payload_json: format!("{{\"n\":{i}}}"),
            ts: 2000 + i as i64,
            ..Default::default()
        })
        .unwrap();
    }

    let sess = st
        .create_session(Session {
            id: "rust-sess".into(),
            workspace_id: "W-rust".into(),
            kind: "pair".into(),
            provider: PROVIDER_CLAUDE_CODE.into(),
            config_json: r#"{"code":"PA"}"#.into(),
            ..Default::default()
        })
        .unwrap();
    let user = st
        .append_message(
            &sess.id,
            Message {
                role: "user".into(),
                content: Some(vec![ContentBlock::text("rust prompt")]),
                ..Default::default()
            },
        )
        .unwrap();
    let mid = st
        .create_message(
            &sess.id,
            Message {
                role: "assistant".into(),
                model_id: "claude-sonnet-5-5".into(),
                ..Default::default()
            },
        )
        .unwrap();
    st.append_content_block(&mid, ContentBlock::text("on it")).unwrap();
    st.append_content_block(&mid, ContentBlock::tool_use("tu-rust", "toolu_r", "Bash", r#"{ "cmd": "ls" }"#))
        .unwrap();
    st.complete_message(
        &mid,
        STREAM_COMPLETE,
        &Message {
            input_tokens: 7,
            output_tokens: 3,
            ..Default::default()
        },
    )
    .unwrap();
    st.write_session_context(&sess.id, "rust summary", &user.id).unwrap();
    st.update_session_ref(&sess.id, "rust-ref").unwrap();
    st.patch_session_config_title(&sess.id, "Rust tab").unwrap();
    st.fork_session(&sess.id, &mid, "coder", r#"{"code":"CO","ownerCode":"PA"}"#).unwrap();
    st.update_session_config_json("sess-go-app", r#"{"code":"APP"}"#).unwrap();
    st.create_session(Session {
        id: "rust-app".into(),
        kind: SESSION_KIND_APP.into(),
        provider: PROVIDER_CLAUDE_CODE.into(),
        config_json: r#"{"code":"APP2","unseen":true}"#.into(),
        ..Default::default()
    })
    .unwrap();

    let mut sched = ScheduledPrompt {
        id: "rust-sched".into(),
        project_id: "rust-proj".into(),
        name: "Rusty".into(),
        prompt: "go".into(),
        base_branch: "main".into(),
        mode: "loop".into(),
        model: "claude-code/claude-sonnet-5-5".into(),
        cadence: "daily".into(),
        time: "08:30".into(),
        time_zone: "Europe/Berlin".into(),
        next_run_at: 1_767_312_000_000,
        flags: Flags::from([("effort".to_owned(), "max".to_owned())]),
        ..Default::default()
    };
    st.save_scheduled_prompt(&sched).unwrap();
    sched.next_run_at = 1_767_398_400_000;
    let run = ScheduledPromptRun {
        id: "run-rust".into(),
        schedule_id: "rust-sched".into(),
        due_at: 1_767_312_000_000,
        status: "waiting".into(),
        ..Default::default()
    };
    assert!(st.claim_scheduled_prompt(&sched, &run).unwrap());
    st.link_scheduled_prompt_run("run-rust", "W-rust").unwrap();
}
