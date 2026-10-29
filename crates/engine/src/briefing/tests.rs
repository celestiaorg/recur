//! Briefing tests: reason precedence and targets, the chat copy, settled rows,
//! PR hosts, sorting, the activity watermark, and the snapshot and context
//! services.

use chrono::{Local, TimeZone, Utc};
use recur_domain::*;

use super::*;
use crate::testing::TestEngine;

fn frozen_ms() -> i64 {
    Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0)
        .unwrap()
        .timestamp_millis()
}

fn inputs(ws: Workspace) -> BriefInputs {
    BriefInputs {
        ws,
        now_ms: frozen_ms(),
        ..BriefInputs::default()
    }
}

fn auth() -> Workspace {
    Workspace {
        id: "W1".into(),
        short: "Auth".into(),
        ..Workspace::default()
    }
}

fn step(id: &str, title: &str, status: &str) -> PlanStep {
    PlanStep {
        id: id.into(),
        title: title.into(),
        status: status.into(),
        ..PlanStep::default()
    }
}

fn unapproved() -> Plan {
    Plan {
        summary: "Add auth".into(),
        body: "Touch login.go".into(),
        steps: vec![step("S1", "Add login", STEP_PENDING)],
        ..Plan::default()
    }
}

fn unfinished() -> Plan {
    Plan {
        approved: true,
        summary: "Auth".into(),
        body: "Go".into(),
        steps: vec![
            step("S1", "Add login", STEP_DONE),
            step("S2", "Add tests", STEP_PENDING),
        ],
        ..Plan::default()
    }
}

fn agent(code: &str, name: &str, status: &str, task: &str) -> Agent {
    Agent {
        code: code.into(),
        name: name.into(),
        status: status.into(),
        task: task.into(),
        ..Agent::default()
    }
}

fn question() -> UserQuestion {
    UserQuestion {
        id: "q-1".into(),
        workspace_id: "W1".into(),
        agent_code: "PA".into(),
        question: "Which auth library?".into(),
        ..UserQuestion::default()
    }
}

fn escalation() -> Escalation {
    Escalation {
        id: "e-1".into(),
        workspace_id: "W1".into(),
        question: "Ship with caveats?".into(),
        ..Escalation::default()
    }
}

fn reason0(b: &WorkspaceBrief) -> &str {
    b.reasons
        .as_deref()
        .and_then(<[String]>::first)
        .map_or("", String::as_str)
}

fn brief(inp: BriefInputs) -> WorkspaceBrief {
    build_workspace_brief(&inp).expect("brief")
}

#[test]
fn reason_precedence() {
    let base = Workspace {
        project_id: "p".into(),
        branch: "recur/auth".into(),
        state: WorkspaceState::Paused,
        last: "waiting".into(),
        last_activity_at: frozen_ms(),
        ..auth()
    };
    let perm = PermissionRequest {
        id: "perm-1".into(),
        workspace_id: "W1".into(),
        agent_code: "PA".into(),
        tool: "Bash".into(),
        ..PermissionRequest::default()
    };
    let cases = [
        (
            "permission beats question",
            BriefInputs {
                perm: Some(perm),
                question: Some(question()),
                ..inputs(base.clone())
            },
            BRIEF_REASON_PERMISSION,
            BRIEF_BUCKET_NEEDS,
        ),
        (
            "question beats escalation",
            BriefInputs {
                question: Some(question()),
                escalation: Some(escalation()),
                ..inputs(base.clone())
            },
            BRIEF_REASON_QUESTION,
            BRIEF_BUCKET_NEEDS,
        ),
        (
            "retryable failure",
            inputs(Workspace {
                state: WorkspaceState::Failed,
                retryable: true,
                last_error: "rate limit".into(),
                ..auth()
            }),
            BRIEF_REASON_RETRYABLE_FAILURE,
            BRIEF_BUCKET_NEEDS,
        ),
        (
            "plan approval",
            inputs(Workspace {
                plan: Some(unapproved()),
                ..auth()
            }),
            BRIEF_REASON_PLAN_APPROVAL,
            BRIEF_BUCKET_NEEDS,
        ),
        (
            "open findings",
            BriefInputs {
                open_findings: 2,
                ..inputs(auth())
            },
            BRIEF_REASON_FINDINGS,
            BRIEF_BUCKET_NEEDS,
        ),
        (
            "active agent beats leftover findings",
            BriefInputs {
                agents: vec![agent("CO", "Coder", "working", "editing login.go")],
                open_findings: 3,
                ..inputs(Workspace {
                    state: WorkspaceState::Coding,
                    phase: "editing login.go".into(),
                    ..auth()
                })
            },
            BRIEF_REASON_ACTIVE_AGENT,
            BRIEF_BUCKET_WORKING,
        ),
        (
            "unfinished plan is pickup",
            inputs(Workspace {
                plan: Some(unfinished()),
                ..auth()
            }),
            BRIEF_REASON_UNFINISHED_PLAN,
            BRIEF_BUCKET_PICKUP,
        ),
        (
            "dirty idle work",
            inputs(Workspace {
                diff: DiffStat { add: 4, del: 1 },
                ..auth()
            }),
            BRIEF_REASON_DIRTY_IDLE,
            BRIEF_BUCKET_PICKUP,
        ),
    ];
    for (name, inp, want, bucket) in cases {
        let got = brief(inp);
        assert_eq!(reason0(&got), want, "{name}");
        assert_eq!(got.bucket, bucket, "{name}");
        assert!(
            !got.now.trim().is_empty() && !got.next.trim().is_empty(),
            "{name}: {} / {}",
            got.now,
            got.next
        );
        assert!(!got.primary.target.is_empty(), "{name}");
    }
}

#[test]
fn the_chat_now_line_names_the_action() {
    let named = vec![Session {
        updated_at: 10,
        config_json: r#"{"code":"PA","title":"oauth weirdness"}"#.into(),
        ..Session::default()
    }];
    let got = brief(BriefInputs {
        headers: named.clone(),
        ..inputs(auth())
    });
    assert_eq!(reason0(&got), BRIEF_REASON_RECENT_CHAT);
    assert_eq!(got.now, "Continue the “oauth weirdness” chat");

    let got = brief(BriefInputs {
        headers: vec![Session {
            updated_at: 10,
            config_json: r#"{"code":"PA","title":"New chat"}"#.into(),
            ..Session::default()
        }],
        ..inputs(auth())
    });
    assert_eq!(got.now, "Continue the latest chat");

    let got = brief(BriefInputs {
        agents: vec![Agent {
            code: "PA".into(),
            unseen: true,
            name: "oauth weirdness".into(),
            ..Agent::default()
        }],
        headers: named.clone(),
        ..inputs(auth())
    });
    assert_eq!(reason0(&got), BRIEF_REASON_UNSEEN_CHAT);
    assert_eq!(got.now, "New reply in “oauth weirdness”");

    let got = brief(BriefInputs {
        question: Some(question()),
        headers: named,
        ..inputs(auth())
    });
    assert_eq!(got.now, "“oauth weirdness” is waiting for an answer");
}

#[test]
fn archived_and_settled_workspaces_are_skipped() {
    for ws in [
        Workspace {
            id: "gone".into(),
            archived: true,
            short: "x".into(),
            ..Workspace::default()
        },
        Workspace {
            id: "done".into(),
            state: WorkspaceState::Merged,
            short: "shipped".into(),
            ..Workspace::default()
        },
        Workspace {
            id: "main".into(),
            is_main: true,
            short: "repo".into(),
            ..Workspace::default()
        },
    ] {
        let id = ws.id.clone();
        assert!(build_workspace_brief(&inputs(ws)).is_none(), "{id}");
    }
}

#[test]
fn plan_findings_and_dirty_route_to_their_panels() {
    for (inp, want) in [
        (
            inputs(Workspace {
                plan: Some(unapproved()),
                ..auth()
            }),
            "workspace:W1:plan",
        ),
        (
            inputs(Workspace {
                plan: Some(unfinished()),
                ..auth()
            }),
            "workspace:W1:plan",
        ),
        (
            BriefInputs {
                open_findings: 2,
                ..inputs(auth())
            },
            "workspace:W1:findings",
        ),
        (
            inputs(Workspace {
                diff: DiffStat { add: 4, del: 1 },
                ..auth()
            }),
            "workspace:W1:review",
        ),
    ] {
        assert_eq!(brief(inp).primary.target, want);
    }
}

#[test]
fn loop_status_opens_the_chat() {
    let cases = [
        (
            inputs(Workspace {
                state: WorkspaceState::Failed,
                retryable: true,
                run_started: true,
                loop_chat_code: "PA".into(),
                ..auth()
            }),
            "chat:W1:PA",
        ),
        (
            inputs(Workspace {
                state: WorkspaceState::Failed,
                last_error: "boom".into(),
                run_started: true,
                ..auth()
            }),
            "chat:W1:MA",
        ),
        (
            BriefInputs {
                agents: vec![agent("CO", "Coder", "working", "editing login.go")],
                ..inputs(Workspace {
                    state: WorkspaceState::Coding,
                    run_started: true,
                    loop_chat_code: "PA".into(),
                    ..auth()
                })
            },
            "chat:W1:PA",
        ),
        (
            BriefInputs {
                agents: vec![agent("PA", "Chat", "working", "writing tests")],
                ..inputs(Workspace {
                    state: WorkspaceState::Coding,
                    ..auth()
                })
            },
            "chat:W1:PA",
        ),
        (
            BriefInputs {
                escalation: Some(escalation()),
                ..inputs(Workspace {
                    state: WorkspaceState::Escalated,
                    run_started: true,
                    loop_chat_code: "PA".into(),
                    ..auth()
                })
            },
            "chat:W1:PA",
        ),
    ];
    for (inp, want) in cases {
        assert_eq!(brief(inp).primary.target, want);
    }
    let got = brief(inputs(Workspace {
        state: WorkspaceState::Paused,
        diff: DiffStat { add: 2, del: 0 },
        ..auth()
    }));
    assert_eq!(reason0(&got), BRIEF_REASON_DIRTY_IDLE);
}

#[test]
fn a_pr_host_brief() {
    let got = brief(BriefInputs {
        pr: Some(Pr {
            id: "pr-1".into(),
            number: 12,
            title: "Fix checkout".into(),
            state: "In review".into(),
            review_requested: true,
            ..Pr::default()
        }),
        ..inputs(Workspace {
            id: "host".into(),
            host_pr_id: "pr-1".into(),
            short: "hidden".into(),
            ..Workspace::default()
        })
    });
    assert!(
        got.is_pr_host && got.pr_id == "pr-1" && got.pr_number == 12,
        "{got:?}"
    );
    assert_eq!(reason0(&got), BRIEF_REASON_PR_ATTENTION);
    assert_eq!(got.primary.target, "pr:pr-1");
}

fn b(id: &str, bucket: &str, reason: &str, activity: i64) -> WorkspaceBrief {
    WorkspaceBrief {
        workspace_id: id.into(),
        bucket: bucket.into(),
        reasons: (!reason.is_empty()).then(|| vec![reason.into()]),
        last_activity_at: activity,
        ..WorkspaceBrief::default()
    }
}

fn ids(briefs: &[WorkspaceBrief]) -> Vec<&str> {
    briefs.iter().map(|b| b.workspace_id.as_str()).collect()
}

#[test]
fn sort_briefs_is_stable() {
    let mut briefs = vec![
        WorkspaceBrief {
            last_viewed_at: 1,
            ..b("pickup-old", BRIEF_BUCKET_PICKUP, "", 10)
        },
        b(
            "needs-unseen",
            BRIEF_BUCKET_NEEDS,
            BRIEF_REASON_UNSEEN_CHAT,
            50,
        ),
        b(
            "needs-perm",
            BRIEF_BUCKET_NEEDS,
            BRIEF_REASON_PERMISSION,
            20,
        ),
        WorkspaceBrief {
            state: WorkspaceState::Coding.as_str().into(),
            ..b("work-active", BRIEF_BUCKET_WORKING, "", 40)
        },
        WorkspaceBrief {
            state: WorkspaceState::Queued.as_str().into(),
            ..b("work-queued", BRIEF_BUCKET_WORKING, "", 90)
        },
        WorkspaceBrief {
            changed_since_viewed: true,
            last_viewed_at: 2,
            ..b("pickup-new", BRIEF_BUCKET_PICKUP, "", 5)
        },
    ];
    sort_briefs(&mut briefs);
    assert_eq!(
        ids(&briefs),
        [
            "needs-unseen",
            "needs-perm",
            "work-active",
            "work-queued",
            "pickup-new",
            "pickup-old"
        ]
    );
}

#[test]
fn needs_sort_newest_first() {
    let mut briefs = vec![
        b("old-pr", BRIEF_BUCKET_NEEDS, BRIEF_REASON_PR_ATTENTION, 10),
        b("just-now", BRIEF_BUCKET_NEEDS, BRIEF_REASON_UNSEEN_CHAT, 90),
        b(
            "mid-findings",
            BRIEF_BUCKET_NEEDS,
            BRIEF_REASON_FINDINGS,
            50,
        ),
    ];
    sort_briefs(&mut briefs);
    assert_eq!(ids(&briefs), ["just-now", "mid-findings", "old-pr"]);
}

#[test]
fn effective_activity_legacy_fallback() {
    let mut ws = Workspace {
        id: "legacy".into(),
        ..Workspace::default()
    };
    let headers = vec![
        Session {
            updated_at: 40,
            ..Session::default()
        },
        Session {
            updated_at: 10,
            ..Session::default()
        },
    ];
    let pr = Pr {
        updated_at: "2026-08-31T12:00:00Z".into(),
        ..Pr::default()
    };
    assert!(effective_workspace_activity(&ws, &headers, 25, Some(&pr)) >= 40);
    ws.last_activity_at = 99;
    assert_eq!(
        effective_workspace_activity(&ws, &headers, 25, Some(&pr)),
        99
    );
}

#[tokio::test]
async fn mark_workspace_viewed_is_a_watermark() {
    let te = TestEngine::new(None);
    let frozen = Utc.with_ymd_and_hms(2026, 8, 31, 15, 0, 0).unwrap();
    te.engine.set_clock(move || frozen.with_timezone(&Local));
    te.store
        .add_workspace(Workspace {
            id: "W-view".into(),
            short: "v".into(),
            last_activity_at: 200,
            seen_activity_at: 50,
            ..Workspace::default()
        })
        .unwrap();
    te.engine.mark_workspace_viewed("W-view", 180);
    let got = te.store.workspace("W-view").unwrap();
    assert_eq!(got.seen_activity_at, 180);
    assert_eq!(got.last_viewed_at, frozen.timestamp_millis());
    te.engine.mark_workspace_viewed("W-view", 100);
    assert_eq!(
        te.store.workspace("W-view").unwrap().seen_activity_at,
        180,
        "no rewind"
    );
    assert!(te.events.has("briefing.updated"));
    te.close().await;
}

#[tokio::test]
async fn changed_since_viewed_needs_a_prior_view() {
    let te = TestEngine::new(None);
    te.store
        .add_project(Project {
            id: "p1".into(),
            name: "P".into(),
            ..Project::default()
        })
        .unwrap();
    te.store
        .add_workspace(Workspace {
            id: "W-fresh".into(),
            project_id: "p1".into(),
            short: "f".into(),
            state: WorkspaceState::Paused,
            last_activity_at: 100,
            last: "created".into(),
            diff: DiffStat { add: 1, del: 0 },
            ..Workspace::default()
        })
        .unwrap();
    te.store
        .add_workspace(Workspace {
            id: "W-back".into(),
            project_id: "p1".into(),
            short: "b".into(),
            state: WorkspaceState::Paused,
            last_activity_at: 200,
            seen_activity_at: 100,
            last: "Coder pushed a fix".into(),
            diff: DiffStat { add: 1, del: 0 },
            ..Workspace::default()
        })
        .unwrap();
    let snap = te.engine.briefing_snapshot();
    let all = snap.workspaces.unwrap_or_default();
    let by = |id: &str| all.iter().find(|b| b.workspace_id == id).cloned();
    assert!(by("W-fresh").is_some_and(|b| !b.changed_since_viewed));
    assert!(by("W-back").is_some_and(|b| b.changed_since_viewed));
    te.close().await;
}

#[tokio::test]
async fn touch_workspace_activity_is_monotonic() {
    let te = TestEngine::new(None);
    te.store
        .add_workspace(Workspace {
            id: "W-act".into(),
            short: "a".into(),
            last_activity_at: 50,
            last: "old".into(),
            ..Workspace::default()
        })
        .unwrap();
    let at = |ms: i64| Utc.timestamp_millis_opt(ms).unwrap().with_timezone(&Local);
    te.engine.touch_workspace_activity("W-act", "newer", at(40));
    assert_eq!(te.store.workspace("W-act").unwrap().last_activity_at, 50);
    te.engine.touch_workspace_activity("W-act", "newer", at(80));
    let got = te.store.workspace("W-act").unwrap();
    assert_eq!((got.last_activity_at, got.last.as_str()), (80, "newer"));
    assert!(te.events.has("briefing.updated"));
    te.close().await;
}

#[tokio::test]
async fn the_snapshot_and_context_services() {
    let te = TestEngine::new(None);
    let frozen = Utc.with_ymd_and_hms(2026, 8, 31, 16, 0, 0).unwrap();
    te.engine.set_clock(move || frozen.with_timezone(&Local));
    te.store
        .add_project(Project {
            id: "p1".into(),
            name: "Storefront".into(),
            ..Project::default()
        })
        .unwrap();
    let plan = Plan {
        summary: "Fix checkout".into(),
        body: "Touch Cart.svelte".into(),
        approved: true,
        steps: vec![
            step("S1", "Fix the flake", STEP_DONE),
            step("S2", "Add a retry", STEP_PENDING),
        ],
        ..Plan::default()
    };
    te.store
        .add_workspace(Workspace {
            id: "W-live".into(),
            project_id: "p1".into(),
            short: "Checkout flake".into(),
            state: WorkspaceState::Coding,
            phase: "editing tests".into(),
            plan: Some(plan.clone()),
            last_activity_at: frozen.timestamp_millis(),
            ..Workspace::default()
        })
        .unwrap();
    te.store.upsert_agent(
        "W-live",
        Agent {
            workspace_id: "W-live".into(),
            ..agent("CO", "Coder", "working", "editing tests")
        },
    );
    te.store
        .add_workspace(Workspace {
            id: "W-arch".into(),
            project_id: "p1".into(),
            archived: true,
            short: "gone".into(),
            plan: Some(plan),
            ..Workspace::default()
        })
        .unwrap();
    let snap = te.engine.briefing_snapshot();
    assert_eq!(snap.counts.working, 1, "{snap:?}");
    let all = snap.workspaces.unwrap_or_default();
    assert_eq!(ids(&all), ["W-live"]);
    assert_eq!(all[0].project_name, "Storefront");
    assert_eq!(reason0(&all[0]), BRIEF_REASON_ACTIVE_AGENT);
    let ctx = te.engine.workspace_context("W-live").expect("context");
    assert_eq!(ctx.plan_steps.len(), 2);
    assert!(
        ctx.agents.len() == 1 && ctx.agents[0].code == "CO",
        "{:?}",
        ctx.agents
    );
    assert!(te.engine.workspace_context("nope").is_err());
    te.close().await;
}
