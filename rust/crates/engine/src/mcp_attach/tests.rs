//! Generic cases of Go `user_mcp_test.go` (the Linear preset itself is
//! tested in `recur-issues`; here it is reached through the tracker).

use std::collections::BTreeMap;
use std::sync::Arc;

use recur_agent::TurnRequest;
use recur_domain::{
    AgentRole, CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHAT_MODE_REVIEW,
    LINEAR_MCP_READ_ONLY_URL, LINEAR_MCP_WRITE_URL, MCP_ID_LINEAR, McpServer, Project,
    SETTING_LINEAR_API_KEY, SETTING_LINEAR_TEAM_KEY, Settings,
};
use recur_issues::IssueTracker;
use recur_issues::linear::Linear;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::roles::{maestro_plan_tools, maestro_tools};

fn linear(env_key: &'static str) -> Vec<Arc<dyn IssueTracker>> {
    vec![Arc::new(Linear::new().with_env(move |k| {
        (k == "LINEAR_API_KEY" && !env_key.is_empty()).then(|| env_key.to_owned())
    }))]
}

fn server(id: &str, enabled: bool) -> McpServer {
    McpServer {
        id: id.into(),
        enabled,
        ..McpServer::default()
    }
}

fn http(id: &str, url: &str) -> McpServer {
    McpServer {
        transport: "http".into(),
        url: url.into(),
        ..server(id, true)
    }
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn has(req: &TurnRequest, tool: &str) -> bool {
    req.allowed_tools
        .as_ref()
        .is_some_and(|t| t.iter().any(|n| n == tool))
}

fn attach(
    mode: &str,
    set: &Settings,
    proj: Option<&Project>,
    trackers: &[Arc<dyn IssueTracker>],
) -> TurnRequest {
    let mut req = TurnRequest::default();
    attach_user_mcp(&mut req, mode, set, proj, trackers);
    req
}

#[test]
fn shared_allowlist_is_copied_per_turn() {
    let shared = vec!["Read".to_owned()];
    let set = Settings {
        mcp_servers: vec![
            McpServer {
                enabled: false,
                ..http("acme", "https://acme.example/mcp")
            },
            McpServer {
                enabled: false,
                ..http("other", "https://other.example/mcp")
            },
        ],
        ..Settings::default()
    };
    let a = Project {
        enabled_mcp: vec!["acme".into()],
        ..Project::default()
    };
    let b = Project {
        enabled_mcp: vec!["other".into()],
        ..Project::default()
    };
    let mut ra = TurnRequest {
        allowed_tools: Some(shared.clone()),
        ..TurnRequest::default()
    };
    attach_user_mcp(&mut ra, CHAT_MODE_ASK, &set, Some(&a), &[]);
    let mut rb = TurnRequest {
        allowed_tools: Some(shared.clone()),
        ..TurnRequest::default()
    };
    attach_user_mcp(&mut rb, CHAT_MODE_ASK, &set, Some(&b), &[]);
    assert_eq!(shared, ["Read"]);
    assert!(has(&ra, "mcp__acme__*") && !has(&ra, "mcp__other__*"));
    assert!(has(&rb, "mcp__other__*") && !has(&rb, "mcp__acme__*"));
}

#[test]
fn linear_off_by_default() {
    let set = Settings {
        bundle_values: values(&[(SETTING_LINEAR_API_KEY, "lin_api_x")]),
        ..Settings::default()
    };
    assert!(
        attach(CHAT_MODE_ASK, &set, None, &linear(""))
            .user_mcp
            .is_empty()
    );
}

#[test]
fn linear_readonly_vs_write() {
    let set = Settings {
        mcp_servers: vec![server("linear", true)],
        bundle_values: values(&[
            (SETTING_LINEAR_API_KEY, "lin_api_x"),
            (SETTING_LINEAR_TEAM_KEY, "ENG"),
        ]),
        ..Settings::default()
    };
    let tr = linear("");
    let ask = attach(CHAT_MODE_ASK, &set, None, &tr);
    assert_eq!(ask.user_mcp.len(), 1);
    assert_eq!(ask.user_mcp[0].name, "linear");
    assert_eq!(ask.user_mcp[0].url, LINEAR_MCP_READ_ONLY_URL);
    assert_eq!(
        ask.user_mcp[0]
            .headers
            .get("Authorization")
            .map(String::as_str),
        Some("Bearer lin_api_x")
    );
    assert_eq!(
        ask.append_system_prompt,
        "Linear MCP is connected. Default team key: ENG. Prefer that team unless the user names another."
    );
    assert!(has(&ask, "mcp__linear__*"));
    assert!(!ask.append_system_prompt.contains("lin_api_x"));
    for mode in [CHAT_MODE_PLAN, CHAT_MODE_REVIEW] {
        assert_eq!(
            attach(mode, &set, None, &tr).user_mcp[0].url,
            LINEAR_MCP_READ_ONLY_URL
        );
    }
    for mode in [CHAT_MODE_AGENT, CHAT_MODE_LOOP] {
        assert_eq!(
            attach(mode, &set, None, &tr).user_mcp[0].url,
            LINEAR_MCP_WRITE_URL
        );
    }
}

#[test]
fn linear_key_sources() {
    let set = Settings {
        mcp_servers: vec![server("linear", true)],
        ..Settings::default()
    };
    assert!(
        attach(CHAT_MODE_AGENT, &set, None, &linear(""))
            .user_mcp
            .is_empty()
    );
    let env = attach(CHAT_MODE_ASK, &set, None, &linear("lin_env"));
    assert_eq!(
        env.user_mcp[0]
            .headers
            .get("Authorization")
            .map(String::as_str),
        Some("Bearer lin_env")
    );
    assert!(env.append_system_prompt.is_empty(), "no team key: no hint");
}

#[test]
fn project_force_off_and_on() {
    let tr = linear("");
    let set = Settings {
        mcp_servers: vec![server("linear", true)],
        bundle_values: values(&[(SETTING_LINEAR_API_KEY, "lin_api_x")]),
        ..Settings::default()
    };
    let off = Project {
        disabled_mcp: vec![MCP_ID_LINEAR.into()],
        ..Project::default()
    };
    assert!(
        attach(CHAT_MODE_AGENT, &set, Some(&off), &tr)
            .user_mcp
            .is_empty()
    );
    let set = Settings {
        bundle_values: values(&[(SETTING_LINEAR_API_KEY, "lin_api_x")]),
        ..Settings::default()
    };
    let on = Project {
        enabled_mcp: vec![MCP_ID_LINEAR.into()],
        ..Project::default()
    };
    let got = attach(CHAT_MODE_ASK, &set, Some(&on), &tr);
    assert_eq!(got.user_mcp.len(), 1);
    assert_eq!(got.user_mcp[0].url, LINEAR_MCP_READ_ONLY_URL);
}

#[test]
fn custom_http_and_stdio() {
    let set = Settings {
        mcp_servers: vec![McpServer {
            name: "Acme".into(),
            headers: values(&[("Authorization", "Bearer z")]),
            ..http("acme", "https://example.com/mcp")
        }],
        ..Settings::default()
    };
    let got = attach(CHAT_MODE_ASK, &set, None, &[]);
    assert_eq!(got.user_mcp.len(), 1);
    assert_eq!(
        (got.user_mcp[0].name.as_str(), got.user_mcp[0].url.as_str()),
        ("acme", "https://example.com/mcp")
    );
    assert!(has(&got, "mcp__acme__*"));
    let set = Settings {
        mcp_servers: vec![McpServer {
            name: "Filesystem".into(),
            transport: "stdio".into(),
            command: "npx".into(),
            args: vec![
                "-y".into(),
                "@modelcontextprotocol/server-filesystem".into(),
            ],
            env: values(&[("FOO", "bar")]),
            ..server("fs", true)
        }],
        ..Settings::default()
    };
    let got = attach(CHAT_MODE_ASK, &set, None, &[]);
    assert_eq!(got.user_mcp.len(), 1);
    let m = &got.user_mcp[0];
    assert_eq!((m.name.as_str(), m.command.as_str()), ("fs", "npx"));
    assert!(m.url.is_empty());
    assert_eq!(m.env.get("FOO").map(String::as_str), Some("bar"));
    assert!(has(&got, "mcp__fs__*"));
    // A stdio server with no command and an http one with no URL are skipped.
    let set = Settings {
        mcp_servers: vec![
            McpServer {
                transport: "stdio".into(),
                ..server("blank", true)
            },
            http("nourl", ""),
        ],
        ..Settings::default()
    };
    assert!(attach(CHAT_MODE_ASK, &set, None, &[]).user_mcp.is_empty());
}

#[test]
fn tracker_modes() {
    let req = |role: AgentRole, tools: Vec<String>| TurnRequest {
        role,
        allowed_tools: Some(tools),
        ..TurnRequest::default()
    };
    assert_eq!(
        tracker_mode_for(&req(AgentRole::Maestro, maestro_tools())),
        CHAT_MODE_LOOP
    );
    assert_eq!(
        tracker_mode_for(&req(AgentRole::Maestro, maestro_plan_tools())),
        CHAT_MODE_ASK
    );
    assert_eq!(
        tracker_mode_for(&req(
            AgentRole::default(),
            vec!["Read".into(), "Edit".into(), "Write".into()]
        )),
        CHAT_MODE_AGENT
    );
    assert_eq!(
        tracker_mode_for(&req(
            AgentRole::Reviewer,
            vec!["Read".into(), "Bash".into()]
        )),
        CHAT_MODE_ASK
    );
    assert_eq!(tracker_mode_for(&TurnRequest::default()), CHAT_MODE_AGENT);
}

#[tokio::test]
async fn skip_thinking_turns_omit_user_mcp_and_real_turns_get_it() {
    let cap = crate::turn::test_support::Capture::empty();
    let te = crate::testing::TestEngine::with(crate::testing::TestOptions {
        provider: Some(cap.clone()),
        trackers: linear(""),
        ..Default::default()
    });
    crate::turn::test_support::add_solo_workspace(&te.engine, "ws");
    let mut set = te.store.settings();
    set.mcp_servers = vec![server(MCP_ID_LINEAR, true)];
    set.bundle_values
        .insert(SETTING_LINEAR_API_KEY.into(), "lin_api_x".into());
    te.store.set_settings(set);
    let tok = CancellationToken::new();
    for skip in [true, false] {
        te.engine
            .run_agent(
                &tok,
                crate::turn::AgentSpec::new(TurnRequest {
                    workspace_id: "ws".into(),
                    skip_thinking: skip,
                    allowed_tools: Some(vec!["Read".into(), "Edit".into()]),
                    ..TurnRequest::default()
                }),
            )
            .await
            .unwrap();
    }
    let reqs = cap.requests();
    assert!(reqs[0].user_mcp.is_empty(), "one-shots skip user MCP");
    assert_eq!(reqs[1].user_mcp.len(), 1);
    assert_eq!(reqs[1].user_mcp[0].url, LINEAR_MCP_WRITE_URL);
    assert_eq!(
        reqs[1].env_scrub,
        ["LINEAR_API_KEY"],
        "tracker secrets are scrubbed"
    );
    // App-scoped turns (no workspace) never get custom MCP.
    te.engine
        .run_agent(
            &tok,
            crate::turn::AgentSpec::new(TurnRequest {
                allowed_tools: Some(vec!["mcp__recur__list_workspace_briefs".into()]),
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap();
    assert!(cap.requests()[2].user_mcp.is_empty());
    te.close().await;
}
