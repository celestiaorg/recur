//! The HTTP contract the agent CLIs depend on, replayed from the Go server
//! (`golden/http_cases.json`, generator `golden/golden_gen_test.go.txt`):
//! status, headers, body bytes and the raw arguments the host sees. Includes
//! the byte-exact `tools/list` snapshot (R2).

mod common;

use std::sync::{Arc, Mutex};

use recur_agent_tools::{Finding, Host};
use recur_domain::Finding as StoredFinding;
use recur_mcp::{McpServer, append_permission_request, names_from_allowed_tools};
use serde::Deserialize;
use serde_json::{Value, json};

/// The generator's `goldenReporter`: no-op host plus three overrides.
#[derive(Default)]
struct GoldenHost {
    notes: Mutex<Vec<Value>>,
}

#[async_trait::async_trait]
impl Host for GoldenHost {
    fn note_tool_args(&self, ws: &str, agent: &str, tool: &str, args: &str) {
        self.notes
            .lock()
            .unwrap()
            .push(json!({"ws": ws, "agent": agent, "tool": tool, "args": args}));
    }

    fn report_finding(&self, _ws: &str, _agent: &str, f: Finding) -> StoredFinding {
        StoredFinding {
            id: "uuid-1".into(),
            short_id: "find_1".into(),
            file: f.file,
            line: f.line,
            severity: f.severity,
            message: f.message,
            status: "open".into(),
            ..Default::default()
        }
    }

    async fn rename_session(&self, ws: &str, agent: &str, owner: &str, title: &str) -> String {
        format!("Renamed {ws}/{agent} ({owner}) to {title}")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    method: String,
    path: String,
    body: String,
    status: u16,
    headers: std::collections::BTreeMap<String, String>,
    resp_body: String,
    notes: Vec<Value>,
}

#[tokio::test]
async fn http_contract_matches_go() {
    let host = Arc::new(GoldenHost::default());
    let s = McpServer::new(Some(host.clone()));
    s.start().await.unwrap();
    let (tok, _) = s.register_owned("W1", "PA", "MA", None);
    let allowed: Vec<String> = [
        "Read",
        "mcp__recur__list_skills",
        "mcp__recur__rename_session",
    ]
    .map(String::from)
    .to_vec();
    let restricted_names = append_permission_request(names_from_allowed_tools(&allowed));
    let (restricted, _) = s.register("W2", "PA", Some(&restricted_names));
    let (inactive, _) = s.register("W3", "PA", None);
    s.unregister(&inactive);

    let cases: Vec<Case> = serde_json::from_str(include_str!("golden/http_cases.json")).unwrap();
    let mut failures = Vec::new();
    for c in &cases {
        let path = c
            .path
            .replace("$TOKEN", &tok)
            .replace("$RESTRICTED", &restricted)
            .replace("$INACTIVE", &inactive);
        host.notes.lock().unwrap().clear();
        let r = common::request(s.port(), &c.method, &path, &c.body).await;
        let mut headers = std::collections::BTreeMap::new();
        for k in [
            "content-type",
            "allow",
            "x-content-type-options",
            "location",
        ] {
            if let Some(v) = r.header(k) {
                headers.insert(k.to_string(), v.replace(&tok, "$TOKEN"));
            }
        }
        let body = r.body.replace(&tok, "$TOKEN");
        let notes = host.notes.lock().unwrap().clone();
        if r.status != c.status || headers != c.headers || body != c.resp_body || notes != c.notes {
            failures.push(format!(
                "{}: {} {}\n  want {} {:?} {:?} notes={:?}\n  got  {} {:?} {:?} notes={:?}",
                c.name,
                c.method,
                c.path,
                c.status,
                c.headers,
                c.resp_body,
                c.notes,
                r.status,
                headers,
                body,
                notes
            ));
        }
    }
    s.close().await.unwrap();
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn tools_list_snapshot_matches_go() {
    let s = McpServer::new(None);
    s.start().await.unwrap();
    let (_, url) = s.register("W1", "PA", None);
    let r = common::post(&url, r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#).await;
    assert_eq!(r.body, include_str!("golden/tools_list.json"));
    s.close().await.unwrap();
}

#[test]
fn constants_and_tool_order_match_go() {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Constants {
        consts: std::collections::BTreeMap<String, String>,
        tool_order: Vec<String>,
    }
    let c: Constants = serde_json::from_str(include_str!("golden/constants.json")).unwrap();
    assert_eq!(c.consts["protocolVersion"], recur_mcp::PROTOCOL_VERSION);
    assert_eq!(c.consts["serverName"], recur_mcp::SERVER_NAME);
    assert_eq!(c.consts["serverVersion"], recur_mcp::SERVER_VERSION);
    assert_eq!(
        c.consts["recurMCPPrefix"],
        recur_agent_tools::RECUR_MCP_PREFIX
    );
    let order: Vec<&str> = recur_agent_tools::host_tools()
        .iter()
        .map(|t| t.name())
        .collect();
    assert_eq!(order, c.tool_order);
}
