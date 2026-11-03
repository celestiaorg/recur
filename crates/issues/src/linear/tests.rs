//! Linear issues, issue claims and the Linear MCP server, plus GraphQL runs
//! against a local server.

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use recur_domain::{
    CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHAT_MODE_REVIEW, IssueRef,
    LINEAR_MCP_READ_ONLY_URL, LINEAR_MCP_WRITE_URL, Project, SETTING_LINEAR_API_KEY,
    SETTING_LINEAR_TEAM_KEY, Settings,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

fn no_env() -> Linear {
    Linear::new().with_env(|_| None)
}

fn keyed(api: &str, team: &str) -> Settings {
    let mut values = BTreeMap::new();
    if !api.is_empty() {
        values.insert(SETTING_LINEAR_API_KEY.to_owned(), api.to_owned());
    }
    if !team.is_empty() {
        values.insert(SETTING_LINEAR_TEAM_KEY.to_owned(), team.to_owned());
    }
    Settings {
        bundle_values: values,
        ..Default::default()
    }
}

#[test]
fn link_issue_projects() {
    let projects = [
        Project {
            id: "houston".into(),
            linear_projects: "Houston".into(),
            ..Default::default()
        },
        Project {
            id: "botfun".into(),
            linear_projects: " BOTFUN , bot.fun".into(),
            ..Default::default()
        },
    ];
    let issue = |project: &str, team: &str| AssignedIssue {
        source: "linear".into(),
        linear_project: project.into(),
        linear_team: team.into(),
        ..Default::default()
    };
    let mut issues = vec![
        issue("houston", "BOTFUN"),
        issue("Unmapped", "BOTFUN"),
        issue("", "ENG"),
    ];
    link_projects(&mut issues, &projects);
    let got: Vec<&str> = issues.iter().map(|i| i.project_id.as_str()).collect();
    assert_eq!(got, ["houston", "botfun", ""]);
}

#[test]
fn linear_app_urls() {
    assert_eq!(
        app_url("https://linear.app/botfun/issue/BOTFUN-152/sold-coins?x=1").as_deref(),
        Some("linear://botfun/issue/BOTFUN-152/sold-coins?x=1")
    );
    for bad in [
        "https://github.com/a/b/issues/1",
        "http://linear.app/x",
        "linear://x",
        "",
    ] {
        assert_eq!(app_url(bad), None, "{bad}");
    }
    assert_eq!(app_url("https://linear.app").as_deref(), Some("linear://"));
    assert_eq!(
        app_url("https://linear.app/a b#frag").as_deref(),
        Some("linear://a%20b")
    );
}

#[test]
fn groups() {
    assert_eq!(group("started"), "progress");
    assert_eq!(group("backlog"), "backlog");
    assert_eq!(group("triage"), "backlog");
    assert_eq!(group("unstarted"), "todo");
}

#[test]
fn mcp_readonly_vs_write_and_team_hint() {
    let linear = no_env();
    let set = keyed("lin_api_x", "ENG");
    assert_eq!(linear.mcp_preset(), Some("linear"));
    let ask = linear.agent_mcp(&set, CHAT_MODE_ASK).unwrap();
    assert_eq!(ask.url, LINEAR_MCP_READ_ONLY_URL);
    assert_eq!(ask.headers["Authorization"], "Bearer lin_api_x");
    let hint = linear.agent_hint(&set).unwrap();
    assert!(hint.contains("Default team key: ENG"));
    assert!(!hint.contains("lin_api_x"));
    for mode in [CHAT_MODE_PLAN, CHAT_MODE_REVIEW] {
        assert_eq!(
            linear.agent_mcp(&set, mode).unwrap().url,
            LINEAR_MCP_READ_ONLY_URL,
            "{mode}"
        );
    }
    assert_eq!(
        linear.agent_mcp(&set, CHAT_MODE_AGENT).unwrap().url,
        LINEAR_MCP_WRITE_URL
    );
    assert_eq!(
        linear.agent_mcp(&set, CHAT_MODE_LOOP).unwrap().url,
        LINEAR_MCP_WRITE_URL
    );
    assert_eq!(linear.agent_hint(&keyed("lin_api_x", "")), None);
    assert_eq!(linear.secret_env(), ["LINEAR_API_KEY"]);
}

#[test]
fn mcp_key_from_env_or_skipped() {
    assert_eq!(
        no_env().agent_mcp(&Settings::default(), CHAT_MODE_AGENT),
        None
    );
    assert!(!no_env().configured(&Settings::default()));
    let env = Linear::new().with_env(|k| (k == "LINEAR_API_KEY").then(|| " lin_env ".to_owned()));
    let got = env.agent_mcp(&Settings::default(), CHAT_MODE_ASK).unwrap();
    assert_eq!(got.headers["Authorization"], "Bearer lin_env");
    let team = Linear::new().with_env(|k| (k == "LINEAR_TEAM_KEY").then(|| "OPS".to_owned()));
    assert!(
        team.agent_hint(&Settings::default())
            .unwrap()
            .contains("Default team key: OPS")
    );
    assert_eq!(
        Linear::new()
            .with_env(|_| Some("env".into()))
            .api_key(&keyed("setting", "")),
        "setting"
    );
}

#[tokio::test]
async fn claim_without_key_is_a_no_op() {
    let issue = IssueRef {
        source: "linear".into(),
        id: "uuid-157".into(),
        key: "BOTFUN-157".into(),
    };
    no_env().claim(&issue, &Settings::default()).await.unwrap();
    assert!(
        no_env()
            .list_open(query(&Settings::default(), &[]))
            .await
            .is_empty()
    );
}

fn query<'a>(settings: &'a Settings, projects: &'a [Project]) -> IssueQuery<'a> {
    IssueQuery {
        settings,
        projects,
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(15),
    }
}

type Seen = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

/// A one-request-per-connection HTTP server answering GraphQL bodies with
/// `answer(query, variables)`.
async fn serve(
    answer: impl Fn(&str, &serde_json::Value) -> (u16, String) + Send + Sync + 'static,
) -> (String, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/graphql", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            let answer = Arc::clone(&answer);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let body = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    let Some(end) = text.find("\r\n\r\n") else {
                        continue;
                    };
                    let len: usize = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        let head = text[..end].to_owned();
                        let body =
                            String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
                        break (head, body);
                    }
                };
                let (head, body) = body;
                let auth = head
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("authorization: ")
                            .or_else(|| l.strip_prefix("Authorization: "))
                    })
                    .unwrap_or_default()
                    .to_owned();
                let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                let q = v["query"].as_str().unwrap_or_default().to_owned();
                log.lock().push((auth, v.clone()));
                let (status, out) = answer(&q, &v["variables"]);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                    out.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (url, seen)
}

fn node(id: &str, updated: &str, state: &str, project: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id, "identifier": format!("ENG-{id}"), "title": format!("Issue {id}"), "url": format!("https://linear.app/x/issue/ENG-{id}"),
        "createdAt": "2026-08-01T00:00:00Z", "updatedAt": updated, "priority": 2, "priorityLabel": "High",
        "assignee": if id == "1" { serde_json::json!({"name": "Josh D", "displayName": "josh", "avatarUrl": "https://a/x.png"}) } else { serde_json::Value::Null },
        "state": {"name": "In Progress", "type": state, "color": "#f2c94c", "position": 2.5},
        "labels": {"nodes": [{"name": "bug", "color": "#eb5757"}]},
        "project": project.map(|p| serde_json::json!({"name": p})),
        "team": {"key": "ENG", "name": "Engineering"},
    })
}

#[tokio::test]
async fn lists_assigned_and_paged_unassigned() {
    let pages = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&pages);
    let (url, seen) = serve(move |q, vars| {
        let body = if q.contains("assignedIssues") {
            serde_json::json!({"data": {"viewer": {"assignedIssues": {"nodes": [node("1", "2026-08-03T00:00:00Z", "started", Some("Houston"))]}}}})
        } else if q.contains("teams(first: 50)") {
            serde_json::json!({"data": {"viewer": {"teams": {"nodes": [{"id": "t1"}, {"id": "t2"}]}}}})
        } else {
            let mut n = counter.lock();
            *n += 1;
            assert_eq!(vars["teams"], serde_json::json!(["t1", "t2"]));
            if *n == 1 {
                assert!(vars.get("after").is_none());
                serde_json::json!({"data": {"issues": {"nodes": [node("2", "2026-08-02T00:00:00Z", "backlog", None)], "pageInfo": {"hasNextPage": true, "endCursor": "c1"}}}})
            } else {
                assert_eq!(vars["after"], "c1");
                serde_json::json!({"data": {"issues": {"nodes": [node("3", "2026-08-01T00:00:00Z", "unstarted", None)], "pageInfo": {"hasNextPage": false, "endCursor": ""}}}})
            }
        };
        (200, body.to_string())
    })
    .await;
    let linear = no_env().with_endpoint(url);
    let projects = [Project {
        id: "eng".into(),
        linear_projects: "ENG".into(),
        ..Default::default()
    }];
    let settings = keyed("lin_api_x", "");
    let mut got = linear.list_open(query(&settings, &projects)).await;
    got.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(got.len(), 3);
    assert_eq!(*pages.lock(), 2);
    let first = &got[0];
    assert_eq!(
        (
            first.source.as_str(),
            first.r#ref.as_str(),
            first.group.as_str(),
            first.project.as_str()
        ),
        ("linear", "ENG-1", "progress", "Houston")
    );
    assert_eq!(
        (first.linear_project.as_str(), first.linear_team.as_str()),
        ("Houston", "ENG")
    );
    assert_eq!(first.state_position, 2.5);
    assert_eq!(first.assignees.as_ref().unwrap()[0].name, "josh");
    assert_eq!(first.project_id, "eng", "team key fallback");
    assert_eq!(
        (got[1].group.as_str(), got[1].project.as_str()),
        ("backlog", "Engineering")
    );
    assert!(got[1].assignees.is_none());
    assert_eq!(got[2].group, "todo");
    assert!(seen.lock().iter().all(|(auth, _)| auth == "lin_api_x"));
}

#[tokio::test]
async fn claim_assigns_viewer_when_unassigned() {
    let (url, seen) = serve(|q, _| {
        let body = if q.contains("issueUpdate") {
            r#"{"data":{"issueUpdate":{"success":true}}}"#.to_owned()
        } else {
            r#"{"data":{"viewer":{"id":"me"},"issue":{"assignee":null}}}"#.to_owned()
        };
        (200, body)
    })
    .await;
    let linear = no_env().with_endpoint(url);
    let issue = IssueRef {
        source: "linear".into(),
        id: "uuid-1".into(),
        key: "ENG-1".into(),
    };
    linear.claim(&issue, &keyed("k", "")).await.unwrap();
    let calls = seen.lock().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1].1["variables"],
        serde_json::json!({"a": "me", "id": "uuid-1"})
    );
}

#[tokio::test]
async fn graphql_errors_surface() {
    let (url, _) = serve(|_, _| {
        (
            200,
            r#"{"errors":[{"message":"Authentication required"}]}"#.to_owned(),
        )
    })
    .await;
    let linear = no_env().with_endpoint(url);
    let issue = IssueRef {
        source: "linear".into(),
        id: "x".into(),
        key: "ENG-1".into(),
    };
    assert_eq!(
        linear.claim(&issue, &keyed("k", "")).await.unwrap_err().0,
        "Authentication required"
    );
    let (url, _) = serve(|_, _| (500, "{}".to_owned())).await;
    assert_eq!(
        no_env()
            .with_endpoint(url)
            .claim(&issue, &keyed("k", ""))
            .await
            .unwrap_err()
            .0,
        "HTTP 500"
    );
    let (url, _) = serve(|_, _| (502, "<html>".to_owned())).await;
    assert!(
        no_env()
            .with_endpoint(url)
            .claim(&issue, &keyed("k", ""))
            .await
            .unwrap_err()
            .0
            .starts_with("HTTP 502: ")
    );
}

#[test]
fn golden_matches_go() {
    let g: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/golden/linear.json")).unwrap();
    assert_eq!(assigned_query(), g["assignedQuery"].as_str().unwrap());
    assert_eq!(unassigned_query(), g["unassignedQuery"].as_str().unwrap());
    assert_eq!(TEAMS_QUERY, g["teamsQuery"].as_str().unwrap());
    let nodes: Vec<IssueNode> = recur_forge::json::decode(g["nodes"].as_str().unwrap()).unwrap();
    assert_eq!(
        recur_domain::json::canonical(serde_json::to_value(to_issues(nodes)).unwrap()),
        g["issues"]
    );
    for case in g["appURLs"].as_array().unwrap() {
        let raw = case[0].as_str().unwrap();
        let want = case[2]
            .as_bool()
            .unwrap()
            .then(|| case[1].as_str().unwrap().to_owned());
        assert_eq!(app_url(raw), want, "{raw:?}");
    }
}
