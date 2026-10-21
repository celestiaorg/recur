//! Port of Go `findings_checklist_test.go`: the Coder's open-findings
//! checklist (done refusal, update_finding attribution, disputes the
//! Reviewer re-judges) and done-summary routing between sibling walks.

use std::collections::BTreeMap;
use std::sync::Arc;

use recur_agent_tools::Host;
use recur_bundles::{LoopPhase, ROLE_MODE_AGENT, Role};
use recur_domain::{
    Agent, CHAT_MODE_REVIEW, FINDING_ACTOR_CODER, FINDING_ACTOR_REVIEWER, Finding, FindingStatus,
    Severity, Workspace, WorkspaceState,
};
use tokio_util::sync::CancellationToken;

use crate::host::EngineHost;
use crate::loop_handle::{LoopHandle, LoopState, RoleView};
use crate::loop_registry::{LOOP_MAP_SEP, RunningLoop};
use crate::roles::reviewer_prompt_for;
use crate::testing::TestEngine;

fn host(t: &TestEngine) -> EngineHost {
    EngineHost::new(Arc::downgrade(&t.engine))
}

fn add(t: &TestEngine, id: &str, state: WorkspaceState) {
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            project_id: "P1".into(),
            goal: "t".into(),
            state,
            ..Workspace::default()
        })
        .unwrap();
}

fn open_finding(ws: &str, id: &str, file: &str, line: i64, sev: Severity, msg: &str) -> Finding {
    Finding {
        id: id.into(),
        workspace_id: ws.into(),
        iteration: 1,
        file: file.into(),
        line,
        severity: sev,
        message: msg.into(),
        status: FindingStatus::Open.as_str().into(),
        ..Finding::default()
    }
}

fn register(t: &TestEngine, ws: &str, key: &str, state: LoopState) -> Arc<LoopHandle> {
    let h = LoopHandle::new(ws, "P1", key, state);
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&h),
        cancel: CancellationToken::new(),
    }));
    h
}

#[tokio::test]
async fn coder_done_refuses_open_findings() {
    let t = TestEngine::new(None);
    add(&t, "L-fc", WorkspaceState::Coding);
    let rep = host(&t);
    let stored = t
        .store
        .add_finding(open_finding("L-fc", "F1", "a.go", 1, Severity::High, "bug"));
    let err = rep.done("L-fc", "CO", "all good", "").unwrap_err();
    assert!(err.0.contains(stored.public_id()), "{}", err.0);
    rep.update_finding("L-fc", "CO", stored.public_id(), "resolved", "")
        .expect("update_finding resolved");
    let got = &t.store.findings("L-fc")[0];
    assert_eq!(got.status, FindingStatus::Resolved.as_str());
    assert_eq!(got.updated_by, FINDING_ACTOR_CODER);
    rep.done("L-fc", "CO", "fixed", "")
        .expect("done after resolve");
}

#[tokio::test]
async fn coder_done_stores_commit_summary() {
    let t = TestEngine::new(None);
    add(&t, "L-cs", WorkspaceState::Coding);
    let lp = register(&t, "L-cs", "L-cs", LoopState::default());
    host(&t)
        .done("L-cs", "CO", "Add holdings precheck", "")
        .expect("done");
    assert_eq!(lp.take_done_summary(), "Add holdings precheck");
    assert_eq!(lp.take_done_summary(), "");
}

#[tokio::test]
async fn writer_done_stores_commit_summary() {
    let t = TestEngine::new(None);
    add(&t, "L-wr", WorkspaceState::Coding);
    let lp = register(
        &t,
        "L-wr",
        "L-wr",
        LoopState {
            current_phase: LoopPhase {
                id: "converge".into(),
                ..LoopPhase::default()
            },
            roles: RoleView {
                loop_roles: BTreeMap::new(),
                phase_roles: Some(BTreeMap::from([(
                    "implementer".to_owned(),
                    "recur.roles.writer".to_owned(),
                )])),
                catalog_roles: Arc::new(vec![Role {
                    id: "recur.roles.writer".into(),
                    code: "WR".into(),
                    name: "Writer".into(),
                    mode: ROLE_MODE_AGENT.into(),
                    ..Role::default()
                }]),
            },
            ..LoopState::default()
        },
    );
    t.store.add_finding(open_finding(
        "L-wr",
        "F-open",
        "a.go",
        1,
        Severity::High,
        "bug",
    ));
    host(&t)
        .done("L-wr", "WR", "Draft the README outline", "")
        .expect("a writer is not gated by the Coder's checklist");
    assert_eq!(lp.take_done_summary(), "Draft the README outline");
}

#[tokio::test]
async fn done_summary_routes_to_owning_loop() {
    let t = TestEngine::new(None);
    add(&t, "L-ml", WorkspaceState::Coding);
    let a = register(
        &t,
        "L-ml",
        "L-ml",
        LoopState {
            chat_code: "PA".into(),
            ..LoopState::default()
        },
    );
    let b = register(
        &t,
        "L-ml",
        &format!("L-ml{LOOP_MAP_SEP}PA2"),
        LoopState {
            chat_code: "PA2".into(),
            ..LoopState::default()
        },
    );
    let (_inbox, _bind) = b.bind_inject("CO");
    host(&t)
        .done("L-ml", "CO", "B's holdings precheck", "")
        .expect("done");
    assert_eq!(
        a.take_done_summary(),
        "",
        "the sibling must not get B's summary"
    );
    assert_eq!(b.take_done_summary(), "B's holdings precheck");
}

#[tokio::test]
async fn done_summary_skips_ambiguous_owner() {
    let t = TestEngine::new(None);
    add(&t, "L-amb", WorkspaceState::Coding);
    let a = register(
        &t,
        "L-amb",
        "L-amb",
        LoopState {
            chat_code: "PA".into(),
            ..LoopState::default()
        },
    );
    let b = register(
        &t,
        "L-amb",
        &format!("L-amb{LOOP_MAP_SEP}PA2"),
        LoopState {
            chat_code: "PA2".into(),
            ..LoopState::default()
        },
    );
    let (_ia, _ba) = a.bind_inject("CO");
    let (_ib, _bb) = b.bind_inject("CO");
    host(&t)
        .done("L-amb", "CO", "A's holdings precheck", "")
        .expect("done");
    assert_eq!(a.take_done_summary(), "");
    assert_eq!(b.take_done_summary(), "");
}

#[tokio::test]
async fn finding_update_actor() {
    let t = TestEngine::new(None);
    for (code, want) in [
        ("CO", FINDING_ACTOR_CODER),
        ("RV", FINDING_ACTOR_REVIEWER),
        ("SR", FINDING_ACTOR_REVIEWER),
        ("PA", FINDING_ACTOR_CODER),
        ("", FINDING_ACTOR_CODER),
    ] {
        assert_eq!(t.engine.finding_update_actor("L", code), want, "{code}");
    }
}

#[tokio::test]
async fn reviewer_update_finding_preserves_attribution() {
    let t = TestEngine::new(None);
    add(&t, "L-rv", WorkspaceState::default());
    let rep = host(&t);
    t.store.add_finding(open_finding(
        "L-rv",
        "F-ok",
        "a.go",
        1,
        Severity::High,
        "bug",
    ));
    t.store.add_finding(open_finding(
        "L-rv",
        "F-no",
        "b.go",
        2,
        Severity::Low,
        "nit",
    ));
    let got = rep
        .update_finding("L-rv", "RV", "F-ok", "resolved", "")
        .unwrap();
    assert_eq!(got.updated_by, FINDING_ACTOR_REVIEWER);
    let got = rep
        .update_finding("L-rv", "RV", "F-no", "invalid", "already addressed")
        .unwrap();
    assert_eq!(got.status, FindingStatus::Invalid.as_str());
    assert_eq!(got.updated_by, FINDING_ACTOR_REVIEWER);
    let prompt = reviewer_prompt_for(&t.store, "L-rv", "t", "", 2);
    assert!(
        !prompt.contains("Coder disputed"),
        "a reviewer invalidation is not a coder dispute:\n{prompt}"
    );

    t.store.upsert_agent(
        "L-rv",
        Agent {
            id: "L-rv-PA".into(),
            workspace_id: "L-rv".into(),
            code: "PA".into(),
            name: "Agent".into(),
            model: "sonnet".into(),
            ..Agent::default()
        },
    );
    t.engine
        .remember_workspace_chat_selection("L-rv", "PA", "sonnet", None, CHAT_MODE_REVIEW, None)
        .await;
    t.store.add_finding(open_finding(
        "L-rv",
        "F-rm",
        "c.go",
        3,
        Severity::Medium,
        "regress",
    ));
    let got = rep
        .update_finding("L-rv", "PA", "F-rm", "resolved", "")
        .unwrap();
    assert_eq!(
        got.updated_by, FINDING_ACTOR_REVIEWER,
        "a Review-mode chat reviews"
    );
}

#[tokio::test]
async fn coder_update_finding_invalid_requires_reason() {
    let t = TestEngine::new(None);
    add(&t, "L-inv", WorkspaceState::default());
    let rep = host(&t);
    t.store
        .add_finding(open_finding("L-inv", "F2", "b.go", 2, Severity::Low, "nit"));
    assert!(
        rep.update_finding("L-inv", "CO", "F2", "invalid", "")
            .is_err()
    );
    let f = rep
        .update_finding("L-inv", "CO", "F2", "invalid", "out of scope for this task")
        .unwrap();
    assert_eq!(f.status_reason, "out of scope for this task");
    assert_eq!(f.updated_by, FINDING_ACTOR_CODER);
}

#[tokio::test]
async fn coder_cannot_touch_human_ignored() {
    let t = TestEngine::new(None);
    add(&t, "L-ig", WorkspaceState::default());
    let rep = host(&t);
    t.store
        .add_finding(open_finding("L-ig", "F3", "c.go", 3, Severity::Medium, "x"));
    t.store
        .set_finding_status("L-ig", "F3", FindingStatus::Ignored.as_str())
        .unwrap();
    assert!(
        rep.update_finding("L-ig", "CO", "F3", "resolved", "")
            .is_err()
    );
}

#[tokio::test]
async fn reviewer_prompt_includes_coder_disputed_findings() {
    let t = TestEngine::new(None);
    add(&t, "L-dp", WorkspaceState::Reviewing);
    let disputed = |id: &str, iter: i64, file: &str, sev: Severity, msg: &str, why: &str| Finding {
        id: id.into(),
        workspace_id: "L-dp".into(),
        iteration: iter,
        file: file.into(),
        line: 4,
        severity: sev,
        message: msg.into(),
        status: FindingStatus::Invalid.as_str().into(),
        status_reason: why.into(),
        updated_by: FINDING_ACTOR_CODER.into(),
        ..Finding::default()
    };
    t.store.add_finding(disputed(
        "F4",
        1,
        "d.go",
        Severity::High,
        "missing nil check",
        "nil is impossible here",
    ));
    t.store.add_finding(disputed(
        "F5",
        3,
        "e.go",
        Severity::Medium,
        "later dispute",
        "from a future cycle",
    ));
    let prompt = reviewer_prompt_for(&t.store, "L-dp", "ship feature", "", 2);
    assert!(prompt.contains("Coder disputed"), "{prompt}");
    assert!(prompt.contains("nil is impossible here"));
    assert!(!prompt.contains("from a future cycle"));
    assert!(
        !reviewer_prompt_for(&t.store, "L-dp", "ship feature", "", 1).contains("Coder disputed")
    );
}

#[tokio::test]
async fn agent_done_allows_open_findings() {
    let t = TestEngine::new(None);
    add(&t, "L-pa", WorkspaceState::default());
    let rep = host(&t);
    t.store.add_finding(open_finding(
        "L-pa",
        "F-open",
        "a.go",
        1,
        Severity::High,
        "bug",
    ));
    rep.done("L-pa", "PA", "unrelated work done", "")
        .expect("an Agent chat is not blocked by leftover opens");
    assert!(rep.done("L-pa", "CO", "all good", "").is_err());
}
