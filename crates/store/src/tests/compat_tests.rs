//! Upgrading from the Go app: `go_store.db`, written by the Go store, reads
//! back here as `go_dump.json` records, and rewriting every row through
//! recur-store loses nothing (JSON cells compared as values). Both fixtures
//! are frozen; edit `go_dump.json` by hand if the read shape changes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use recur_domain::json::canonical;
use recur_domain::*;
use rusqlite::Connection;
use serde::Serialize;
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

/// Everything the store reads back, as JSON.
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
    ws_ids.extend(
        s.session_headers_by_workspace()
            .unwrap_or_default()
            .into_keys(),
    );

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
        pr_by_id: s
            .prs()
            .into_iter()
            .map(|p| (p.id.clone(), s.pr(&p.id).unwrap()))
            .collect(),
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
    serde_json::to_value(&d).unwrap()
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
        _ => assert_eq!(canonical(got.clone()), canonical(want.clone()), "{path}"),
    }
}

/// Every row of every table, as text, ordered. JSON object and array cells
/// are re-encoded canonically so equal values compare equal.
fn table_dump(path: &Path) -> BTreeMap<String, Vec<Vec<String>>> {
    let c = Connection::open(path).unwrap();
    let tables: Vec<String> = {
        let mut stmt = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
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
                            rusqlite::types::ValueRef::Text(v)
                            | rusqlite::types::ValueRef::Blob(v) => {
                                let text = String::from_utf8_lossy(v).into_owned();
                                match serde_json::from_str::<Value>(&text) {
                                    Ok(j) if j.is_object() || j.is_array() => {
                                        canonical(j).to_string()
                                    }
                                    _ => text,
                                }
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

/// Writing every loaded row back through recur-store leaves every table of
/// the Go-written database unchanged.
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
            let (p, repo, branch) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap(),
            );
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
            assert_eq!(rows, &after[t], "table {t}");
        }
    });
}
