//! Go-generated golden fixtures for slice E4 (R2): every chat mode's turn
//! shape, the switch_mode matrix with its resume prompt and display, the
//! surface marker peel and subject prompts, child-state text, the
//! create_workspace child rule, Ask Recur's prompt and ids, briefing age and
//! chat nouns, full briefs from named inputs, and the fork preamble
//! (`tests/golden/go_e4.json`, generator `tests/golden/gen_e4_golden.go.txt`).

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use pretty_assertions::assert_eq;
use recur_agent_tools::CreateWorkspaceRequest;
use recur_domain::*;
use serde_json::{Value, json};

use crate::app_chat::{
    APP_CHAT_APPEND_PROMPT, APP_CHAT_TOOLS, app_chat_agent_id, app_chat_tab_name,
};
use crate::briefing::{BriefInputs, build_workspace_brief, chat_noun, format_age};
use crate::chat::children::{PARENT_STEER_PREFIX, child_notice_display, describe_child_state};
use crate::chat::create_tool::want_child;
use crate::chat::fork::fork_history_preamble;
use crate::chat::modes::chat_mode_shape;
use crate::chat::surface::{ChatSurface, peel_chat_surface};
use crate::chat::switch_mode::*;
use crate::testing::{TestEngine, TestOptions};
use crate::turn::sessions::app_code;

const GOLDEN: &str = include_str!("../tests/golden/go_e4.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden json")
}

fn cases<'a>(g: &'a Value, key: &str) -> &'a Vec<Value> {
    g[key]
        .as_array()
        .unwrap_or_else(|| panic!("no golden section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

#[test]
fn mode_shapes_match_go() {
    let g = golden();
    for c in cases(&g, "mode_shapes") {
        let (mode, child) = (s(&c["in"]["mode"]), c["in"]["child"].as_bool().unwrap());
        let got = chat_mode_shape(mode, &Settings::default(), child);
        let out = &c["out"];
        assert_eq!(
            json!({
                "tools": got.tools,
                "permission": got.permission,
                "appendPrompt": got.append_prompt,
                "mcp": got.mcp,
            }),
            *out,
            "mode {mode:?} child {child}"
        );
    }
}

#[test]
fn switch_mode_matches_go() {
    let g = golden();
    for c in cases(&g, "switch") {
        let (from, to) = (s(&c["in"]["from"]), s(&c["in"]["to"]));
        let out = &c["out"];
        assert_eq!(
            switch_mode_allowed(from, to),
            out["allowed"].as_bool().unwrap(),
            "{from} -> {to}"
        );
        assert_eq!(
            format_mode_switch_resume_prompt(from, to),
            s(&out["prompt"])
        );
        assert_eq!(format_mode_switch_display(to), s(&out["display"]), "{to}");
    }
    for c in cases(&g, "dead_ends") {
        assert_eq!(
            is_mode_switch_resume_dead_end(s(&c["in"])),
            c["out"].as_bool().unwrap(),
            "{:?}",
            c["in"]
        );
    }
    assert_eq!(SWITCH_MODE_PARK_RESULT_SUFFIX, s(&g["park_suffix"]));
    assert_eq!(MAX_MODE_REMOUNTS, g["max_remounts"].as_i64().unwrap());
}

#[test]
fn surface_peel_matches_go() {
    let g = golden();
    for c in cases(&g, "peel") {
        let input = s(&c["in"]);
        let (rest, surf, ok) = match peel_chat_surface(input) {
            Some((rest, surf)) => (rest, surf, true),
            None => (input.to_owned(), ChatSurface::default(), false),
        };
        let out = &c["out"];
        assert_eq!(rest, s(&out["rest"]), "{input:?}");
        assert_eq!(ok, out["ok"].as_bool().unwrap(), "{input:?}");
        assert_eq!(
            serde_json::to_value(&surf).unwrap(),
            out["surface"],
            "{input:?}"
        );
    }
}

#[tokio::test]
async fn subject_prompts_match_go() {
    let g = golden();
    let te = TestEngine::with(TestOptions {
        forges: vec![Arc::new(recur_forge::github::GitHub::new())],
        ..TestOptions::default()
    });
    for c in cases(&g, "subjects") {
        let surf: ChatSurface = serde_json::from_value(c["in"].clone()).unwrap();
        assert_eq!(
            te.engine.subject_append_prompt(&surf),
            s(&c["out"]),
            "{}",
            c["in"]
        );
    }
    te.close().await;
}

#[test]
fn child_text_matches_go() {
    let g = golden();
    for c in cases(&g, "children") {
        let i = &c["in"];
        let w = Workspace {
            id: s(&i["id"]).into(),
            goal: s(&i["goal"]).into(),
            state: WorkspaceState::from(s(&i["state"])),
            fan_out: (!s(&i["outcome"]).is_empty()).then(|| ChildFanOut {
                outcome: s(&i["outcome"]).into(),
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        };
        assert_eq!(describe_child_state(&w), s(&c["out"]["state"]), "{i}");
        assert_eq!(child_notice_display(&w), s(&c["out"]["display"]), "{i}");
    }
    assert_eq!(PARENT_STEER_PREFIX, s(&g["parent_steer"]));
}

#[test]
fn want_child_matches_go() {
    let g = golden();
    let caller = |name: &str| {
        let base = Workspace {
            project_id: "p".into(),
            branch: "b".into(),
            worktree_path: "/wt".into(),
            ..Workspace::default()
        };
        match name {
            "scheduled" => Workspace {
                schedule_id: "sched".into(),
                ..base
            },
            "bare" => Workspace {
                project_id: "p".into(),
                ..Workspace::default()
            },
            "nested" => Workspace {
                parent_id: "p0".into(),
                ..base
            },
            _ => base,
        }
    };
    for c in cases(&g, "want_child") {
        let i = &c["in"];
        let w = caller(s(&i["caller"]));
        let req = CreateWorkspaceRequest {
            standalone: i["standalone"].as_bool(),
            base_branch: s(&i["baseBranch"]).into(),
            project_id: s(&i["projectId"]).into(),
            ..CreateWorkspaceRequest::default()
        };
        let pid = if req.project_id.is_empty() {
            w.project_id.clone()
        } else {
            req.project_id.clone()
        };
        let (child, reason) = match want_child(&w, &req, s(&i["mode"]), &pid) {
            Ok(child) => (child, String::new()),
            Err(r) => (false, r),
        };
        assert_eq!(child, c["out"]["child"].as_bool().unwrap(), "{i}");
        assert_eq!(reason, s(&c["out"]["reason"]), "{i}");
    }
}

#[test]
fn app_chat_matches_go() {
    let g = golden();
    let a = &g["app_chat"];
    assert_eq!(APP_CHAT_APPEND_PROMPT, s(&a["appendPrompt"]));
    assert_eq!(json!(APP_CHAT_TOOLS), a["tools"]);
    for (code, id) in a["agentIds"].as_object().unwrap() {
        assert_eq!(app_chat_agent_id(code), s(id), "{code:?}");
    }
    for (code, name) in a["tabNames"].as_object().unwrap() {
        assert_eq!(app_chat_tab_name(&app_code(code)), s(name), "{code:?}");
    }
}

#[test]
fn briefing_text_matches_go() {
    let g = golden();
    for c in cases(&g, "ages") {
        let h = c["in"].as_i64().unwrap();
        assert_eq!(format_age(h * 3_600_000), s(&c["out"]), "{h}h");
    }
    for c in cases(&g, "chat_nouns") {
        assert_eq!(chat_noun(s(&c["in"])), s(&c["out"]), "{:?}", c["in"]);
    }
}

fn brief_cases() -> Vec<(&'static str, BriefInputs)> {
    let frozen = Utc
        .with_ymd_and_hms(2026, 8, 31, 12, 0, 0)
        .unwrap()
        .timestamp_millis();
    let auth = || Workspace {
        id: "W1".into(),
        short: "Auth".into(),
        ..Workspace::default()
    };
    let step = |id: &str, title: &str, status: &str| PlanStep {
        id: id.into(),
        title: title.into(),
        status: status.into(),
        ..PlanStep::default()
    };
    let unapproved = Plan {
        summary: "Add auth".into(),
        body: "Touch login.go".into(),
        steps: vec![step("S1", "Add login", STEP_PENDING)],
        ..Plan::default()
    };
    let unfinished = Plan {
        approved: true,
        summary: "Auth".into(),
        body: "Go".into(),
        steps: vec![
            step("S1", "Add login", STEP_DONE),
            step("S2", "Add tests", STEP_PENDING),
        ],
        ..Plan::default()
    };
    let q = UserQuestion {
        id: "q-1".into(),
        workspace_id: "W1".into(),
        agent_code: "PA".into(),
        question: "Which auth library?".into(),
        ..UserQuestion::default()
    };
    let named = vec![Session {
        updated_at: 10,
        config_json: r#"{"code":"PA","title":"oauth weirdness"}"#.into(),
        ..Session::default()
    }];
    let inp = |ws: Workspace| BriefInputs {
        ws,
        now_ms: frozen,
        ..BriefInputs::default()
    };
    let agent = |code: &str, name: &str, status: &str, task: &str| Agent {
        code: code.into(),
        name: name.into(),
        status: status.into(),
        task: task.into(),
        ..Agent::default()
    };
    vec![
        (
            "permission",
            BriefInputs {
                perm: Some(PermissionRequest {
                    id: "perm-1".into(),
                    workspace_id: "W1".into(),
                    agent_code: "PA".into(),
                    tool: "Bash".into(),
                    ..PermissionRequest::default()
                }),
                question: Some(q.clone()),
                ..inp(Workspace {
                    state: WorkspaceState::Paused,
                    last: "waiting".into(),
                    last_activity_at: frozen,
                    ..auth()
                })
            },
        ),
        (
            "question",
            BriefInputs {
                question: Some(q),
                headers: named.clone(),
                ..inp(auth())
            },
        ),
        (
            "escalation",
            BriefInputs {
                escalation: Some(Escalation {
                    id: "e-1".into(),
                    workspace_id: "W1".into(),
                    question: "Ship with caveats?".into(),
                    ..Escalation::default()
                }),
                ..inp(Workspace {
                    state: WorkspaceState::Escalated,
                    run_started: true,
                    loop_chat_code: "PA".into(),
                    ..auth()
                })
            },
        ),
        (
            "retryable",
            inp(Workspace {
                state: WorkspaceState::Failed,
                retryable: true,
                last_error: "rate limit".into(),
                ..auth()
            }),
        ),
        (
            "failure",
            inp(Workspace {
                state: WorkspaceState::Failed,
                last_error: "boom".into(),
                run_started: true,
                ..auth()
            }),
        ),
        (
            "plan_approval",
            inp(Workspace {
                plan: Some(unapproved),
                ..auth()
            }),
        ),
        (
            "findings",
            BriefInputs {
                open_findings: 2,
                ..inp(auth())
            },
        ),
        (
            "active_agent",
            BriefInputs {
                agents: vec![agent("CO", "Coder", "working", "editing login.go")],
                open_findings: 3,
                ..inp(Workspace {
                    state: WorkspaceState::Coding,
                    phase: "editing login.go".into(),
                    ..auth()
                })
            },
        ),
        (
            "unfinished_plan",
            inp(Workspace {
                plan: Some(unfinished),
                ..auth()
            }),
        ),
        (
            "dirty_idle",
            inp(Workspace {
                diff: DiffStat { add: 4, del: 1 },
                ..auth()
            }),
        ),
        (
            "recent_chat",
            BriefInputs {
                headers: named.clone(),
                ..inp(auth())
            },
        ),
        (
            "unseen_chat",
            BriefInputs {
                agents: vec![Agent {
                    code: "PA".into(),
                    unseen: true,
                    name: "oauth weirdness".into(),
                    ..Agent::default()
                }],
                headers: named,
                ..inp(auth())
            },
        ),
        (
            "quiet",
            BriefInputs {
                project_name: "Storefront".into(),
                ..inp(Workspace {
                    diff: DiffStat { add: 1, del: 0 },
                    last_activity_at: frozen - 72 * 3_600_000,
                    ..auth()
                })
            },
        ),
        (
            "pr_host",
            BriefInputs {
                pr: Some(Pr {
                    id: "pr-1".into(),
                    number: 12,
                    title: "Fix checkout".into(),
                    state: "In review".into(),
                    review_requested: true,
                    ..Pr::default()
                }),
                ..inp(Workspace {
                    id: "host".into(),
                    host_pr_id: "pr-1".into(),
                    short: "hidden".into(),
                    ..Workspace::default()
                })
            },
        ),
    ]
}

#[test]
fn briefs_match_go() {
    let g = golden();
    let want = g["briefs"].as_object().unwrap();
    let cases = brief_cases();
    assert_eq!(cases.len(), want.len());
    for (name, inp) in cases {
        let got = build_workspace_brief(&inp).map(|b| serde_json::to_value(b).unwrap());
        assert_eq!(got.unwrap_or(Value::Null), want[name], "{name}");
    }
}

#[test]
fn fork_preamble_matches_go() {
    let g = golden();
    let msgs = vec![
        Message {
            role: "user".into(),
            content: Some(vec![ContentBlock::text("first question")]),
            ..Message::default()
        },
        Message {
            role: "assistant".into(),
            content: Some(vec![
                ContentBlock::text("answer one"),
                ContentBlock::tool_use("t1", "c1", "Bash", r#"{"command":"ls"}"#),
            ]),
            ..Message::default()
        },
        Message {
            role: "user".into(),
            content: Some(vec![ContentBlock::tool_result("t1", "a.txt\nb.txt", false)]),
            ..Message::default()
        },
        Message {
            role: "assistant".into(),
            content: Some(vec![ContentBlock::text("two files")]),
            ..Message::default()
        },
    ];
    assert_eq!(
        fork_history_preamble(&msgs, None, ""),
        s(&g["fork_preamble"])
    );
}
