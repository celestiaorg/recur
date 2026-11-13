//! Snapshot fixtures: every engine event payload, the role prompts and tool
//! tables, the kitchen word lists and the synthetic guards (payloads as JSON
//! values, `tests/golden/go_e1.json`). Edit the fixture by hand when the
//! behavior changes on purpose.

use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use recur_domain::{
    AgentStep, Artifact, AttentionCounts, DiffStat, Escalation, FeedItem, Finding,
    PermissionRequest, Pr, PrComment, Resource, UserQuestion, UserQuestionAnswer, Workspace,
    WorkspaceState,
};
use serde_json::Value;

use crate::bus::*;
use crate::guard::{child_loop_def, synthetic_policy, synthetic_pr_loop, synthetic_step_loop};
use crate::roles::*;
use crate::testing::TestEngine;
use crate::workspace::ids::{KITCHEN_ADJECTIVES, KITCHEN_DISHES};

const GOLDEN: &str = include_str!("../tests/golden/go_e1.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden json")
}

fn input<T: serde::de::DeserializeOwned>(g: &Value, key: &str) -> T {
    serde_json::from_value(g["inputs"][key].clone()).unwrap_or_else(|e| panic!("input {key}: {e}"))
}

fn go_event(g: &Value, case: &str) -> (String, String) {
    let ev = g["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|e| e["case"] == case)
        .unwrap_or_else(|| panic!("no golden case {case}"));
    (
        ev["name"].as_str().unwrap().to_owned(),
        ev["payload"].as_str().unwrap().to_owned(),
    )
}

fn json(s: &str) -> Value {
    recur_domain::json::canonical(serde_json::from_str(s).unwrap())
}

fn check(g: &Value, case: &str, ev: &EngineEvent) {
    let (name, payload) = go_event(g, case);
    assert_eq!(ev.name(), name, "{case}: name");
    assert_eq!(json(&ev.payload_json()), json(&payload), "{case}: payload");
}

fn route(surface: &str, ws: &str, code: &str, owner: &str) -> SessionRoute {
    SessionRoute {
        surface: surface.into(),
        workspace_id: ws.into(),
        code: code.into(),
        owner_code: owner.into(),
    }
}

#[tokio::test]
async fn engine_published_events_match_go() {
    let g = golden();
    let t = TestEngine::new(None);
    let ws: Workspace = input(&g, "workspace");
    t.store.add_workspace(ws.clone()).unwrap();

    t.engine.publish_loop_updated(
        &ws.id,
        WorkspaceState::Reviewing,
        "reviewing iteration 2",
        2,
    );
    check(&g, "loop-updated", t.events.all().last().unwrap());
    t.engine
        .publish_loop_updated("missing.ws", WorkspaceState::Paused, "paused", 0);
    check(&g, "loop-updated-missing", t.events.all().last().unwrap());
    // Persisted exactly as published.
    let stored = t.store.events_for_loop(&ws.id).unwrap().unwrap_or_default();
    let (_, want) = go_event(&g, "loop-updated");
    assert!(
        stored
            .iter()
            .any(|e| e.r#type == "workspace.updated" && json(&e.payload_json) == json(&want))
    );

    let feed: FeedItem = input(&g, "feedItem");
    t.engine.emit_workspace_event_item(&ws.id, feed);
    check(&g, "workspace-event", t.events.all().last().unwrap());
    t.engine.emit_workspace_event_item(
        &ws.id,
        FeedItem {
            id: "ev-2".into(),
            agent_code: "MA".into(),
            name: "Maestro".into(),
            text: "Converged.".into(),
            time: "now".into(),
            ..FeedItem::default()
        },
    );
    check(&g, "workspace-event-ma", t.events.all().last().unwrap());
    t.engine
        .emit_deviation(&ws.id, "Skipped <i>x</i>", "rule", "why");
    let mut dev = t.events.all().last().unwrap().clone();
    if let EngineEvent::WorkspaceEvent(e) = &mut dev {
        e.event.id = "ev-dev".into();
    }
    check(&g, "deviation", &dev);

    t.store.upsert_agent(
        &ws.id,
        recur_domain::Agent {
            id: format!("{}-PA", ws.id),
            workspace_id: ws.id.clone(),
            code: "PA".into(),
            status: "working".into(),
            ..Default::default()
        },
    );
    t.engine.update_agent_status(&ws.id, "CO", "working");
    check(
        &g,
        "agent-updated",
        t.events.by_name("agent.updated").last().unwrap(),
    );
    t.engine.append_agent_step(
        &ws.id,
        "CO",
        AgentStep {
            kind: "edit".into(),
            text: "a.go".into(),
            diff: Some(DiffStat { add: 1, del: 0 }),
            state: "edit".into(),
        },
    );
    check(&g, "agent-step", t.events.all().last().unwrap());

    let pr: Pr = input(&g, "pr");
    t.store.add_pr(pr.clone()).unwrap();
    t.engine.emit_pr_updated(&pr.id, false);
    check(&g, "pr-updated", t.events.all().last().unwrap());
    t.engine.emit_pr_updated(&pr.id, true);
    check(&g, "pr-updated-pending", t.events.all().last().unwrap());
    let bare: Pr = input(&g, "prBare");
    t.store.add_pr(bare.clone()).unwrap();
    t.engine.emit_pr_updated(&bare.id, false);
    check(&g, "pr-updated-bare", t.events.all().last().unwrap());
}

#[test]
fn literal_payloads_match_go() {
    let g = golden();
    let ws: Workspace = input(&g, "workspace");
    let w = ws.id.as_str();
    let wr = || WorkspaceRef::new(w);

    check(
        &g,
        "hoisted",
        &EngineEvent::WorkspaceHoisted(WorkspaceHoisted {
            chats_changed: true,
            code_remap: BTreeMap::from([
                ("PA".to_owned(), "PA3".to_owned()),
                ("PA2".to_owned(), "PA4".to_owned()),
            ]),
            focus_chat_code: "PA3".into(),
            from_workspace_id: "proj.host".into(),
            moved_codes: Some(vec!["PA3".into(), "PA4".into()]),
            workspace_id: "proj.dest".into(),
        }),
    );
    check(
        &g,
        "hoisted-bare",
        &EngineEvent::WorkspaceHoisted(WorkspaceHoisted {
            chats_changed: true,
            from_workspace_id: "proj.host".into(),
            workspace_id: "proj.dest".into(),
            ..WorkspaceHoisted::default()
        }),
    );
    check(&g, "deleted", &EngineEvent::WorkspaceDeleted(wr()));
    check(&g, "removed", &EngineEvent::WorkspaceRemoved(wr()));
    check(
        &g,
        "watched",
        &EngineEvent::WorkspaceWatched(WorkspaceRef::new("")),
    );
    check(&g, "findings", &EngineEvent::FindingsUpdated(wr()));
    check(
        &g,
        "fs-paths",
        &EngineEvent::WorkspaceFsChanged(FsChanged {
            paths: vec!["a.go".into(), "dir/b.ts".into()],
            rescan: Some(false),
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "fs-rescan",
        &EngineEvent::WorkspaceFsChanged(FsChanged {
            paths: vec![],
            rescan: Some(true),
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "fs-base",
        &EngineEvent::WorkspaceFsChanged(FsChanged {
            workspace_id: w.into(),
            ..FsChanged::default()
        }),
    );
    check(
        &g,
        "chat-tab",
        &EngineEvent::WorkspaceChatTab(ChatTab {
            code: "PA2".into(),
            title: "Fix <it>".into(),
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "agent-updated-app",
        &EngineEvent::AgentUpdated(AgentUpdated {
            agent_id: "app-APP".into(),
            code: Some("APP".into()),
            status: "working".into(),
            surface: Some("app".into()),
            workspace_id: String::new(),
        }),
    );
    let res: Resource = input(&g, "resource");
    check(&g, "resource", &EngineEvent::ResourceUpdated(res));
    check(
        &g,
        "inbox",
        &EngineEvent::InboxUpdated(InboxUpdated { unread: 3 }),
    );
    let counts: AttentionCounts = input(&g, "attention");
    check(
        &g,
        "attention",
        &EngineEvent::AttentionUpdated(AttentionUpdated::new(counts, w, "PA2", true, false)),
    );
    check(
        &g,
        "attention-clear",
        &EngineEvent::AttentionUpdated(AttentionUpdated::new(
            AttentionCounts::default(),
            "",
            "",
            false,
            true,
        )),
    );
    check(
        &g,
        "briefing",
        &EngineEvent::BriefingUpdated(BriefingUpdated {
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "briefing-all",
        &EngineEvent::BriefingUpdated(BriefingUpdated::default()),
    );
    check(
        &g,
        "notify",
        &EngineEvent::Notify(Notify {
            body: "Which <db>?".into(),
            target: Some(format!("workspace:{w}")),
            title: "A workflow needs you".into(),
        }),
    );
    check(
        &g,
        "notify-bare",
        &EngineEvent::Notify(Notify {
            body: "Recur needs Git".into(),
            target: None,
            title: "Git is too old".into(),
        }),
    );
    let perm: PermissionRequest = input(&g, "permission");
    check(
        &g,
        "permission-requested",
        &EngineEvent::PermissionRequested(perm),
    );
    check(
        &g,
        "permission-resolved",
        &EngineEvent::PermissionResolved(PermissionResolved {
            allow: false,
            id: "perm-1".into(),
            reason: "denied".into(),
        }),
    );
    let q: UserQuestion = input(&g, "question");
    check(
        &g,
        "question-asked",
        &EngineEvent::QuestionAsked(QuestionAsked {
            question: q,
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "question-answered",
        &EngineEvent::QuestionAnswered(QuestionAnswered {
            answer: UserQuestionAnswer {
                choice: "A".into(),
                ..Default::default()
            },
            id: "q-1".into(),
        }),
    );
    check(
        &g,
        "question-cleared",
        &EngineEvent::QuestionAnswered(QuestionAnswered {
            answer: UserQuestionAnswer::default(),
            id: "q-1".into(),
        }),
    );
    let esc: Escalation = input(&g, "escalation");
    check(
        &g,
        "escalation-raised",
        &EngineEvent::EscalationRaised(EscalationRaised {
            escalation: esc,
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "escalation-resolved-live",
        &EngineEvent::EscalationResolved(EscalationResolved {
            agent_code: None,
            answer: "Yes".into(),
            open: None,
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "escalation-resolved",
        &EngineEvent::EscalationResolved(EscalationResolved {
            agent_code: Some("MA".into()),
            answer: "Yes".into(),
            open: Some(false),
            workspace_id: w.into(),
        }),
    );
    let art: Artifact = input(&g, "artifact");
    check(
        &g,
        "artifact-created",
        &EngineEvent::ArtifactCreated(ArtifactEvent {
            artifact: art.clone(),
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "artifact-presented",
        &EngineEvent::ArtifactPresented(ArtifactEvent {
            artifact: art,
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "mode-changed",
        &EngineEvent::ChatModeChanged(ChatModeChanged {
            agent_code: "PA".into(),
            from: "agent".into(),
            mode: "plan".into(),
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "chat-renamed",
        &EngineEvent::ChatRenamed(ChatRenamed {
            agent_code: "PA".into(),
            title: "New title".into(),
            workspace_id: w.into(),
        }),
    );
    check(
        &g,
        "app-chat-tab",
        &EngineEvent::AppChatTab(AppChatTab {
            code: "AP2".into(),
            title: "Ask".into(),
        }),
    );
    check(&g, "app-focused", &EngineEvent::AppFocused(AppFocused {}));

    let cm: PrComment = input(&g, "comment");
    check(
        &g,
        "pr-comment",
        &EngineEvent::PrComment(PrCommentEvent {
            comment: cm,
            pr_id: "gh-proj-7".into(),
        }),
    );
    check(
        &g,
        "pr-removed",
        &EngineEvent::PrRemoved(PrRef {
            pr_id: "gh-proj-7".into(),
        }),
    );
    let bare: Pr = input(&g, "prBare");
    check(
        &g,
        "prs-synced",
        &EngineEvent::PrsSynced(PrsSynced {
            project_id: "proj".into(),
            prs: Some(vec![bare]),
        }),
    );
    check(
        &g,
        "prs-synced-nil",
        &EngineEvent::PrsSynced(PrsSynced {
            project_id: "proj".into(),
            prs: None,
        }),
    );
    check(
        &g,
        "branch-rules",
        &EngineEvent::BranchRules(BranchRules {
            branch: "main".into(),
            project_id: "proj".into(),
            remote: "origin".into(),
            repo: "o/r".into(),
            require_pr: true,
        }),
    );
}

#[test]
fn session_payloads_match_go() {
    let g = golden();
    let ws: Workspace = input(&g, "workspace");
    let w = ws.id.as_str();
    let co = route("", w, "CO", "");

    check(&g, "s-user", &route("", w, "CO", "PA2").user("do <it>"));
    check(&g, "s-user-app", &route("app", "", "APP", "").user("hi"));
    let mut d = co.delta();
    d.text = "par".into();
    d.seq = Some(7);
    d.replace = true;
    check(&g, "s-delta-cum", &EngineEvent::SessionDelta(d));
    let mut d = co.delta();
    d.text = "partial".into();
    d.seq = Some(8);
    d.replace = true;
    d.done = true;
    d.uuid = "u-1".into();
    check(&g, "s-delta-final", &EngineEvent::SessionDelta(d));
    let mut d = co.delta();
    d.text = "whole".into();
    d.done = true;
    check(&g, "s-delta-done", &EngineEvent::SessionDelta(d));
    let mut d = co.delta();
    d.text = "sub".into();
    d.seq = Some(1);
    d.replace = true;
    d.parent_id = Some("toolu_1".into());
    check(&g, "s-delta-nested", &EngineEvent::SessionDelta(d));
    let mut d = co.delta();
    d.text = "sub".into();
    d.done = true;
    d.parent_id = Some("toolu_1".into());
    check(&g, "s-delta-nested-done", &EngineEvent::SessionDelta(d));

    let ma = route("", w, "MA", "");
    let mut th = ma.thinking();
    th.text = Some("hmm".into());
    th.seq = Some(2);
    th.replace = true;
    check(&g, "s-think-cum", &EngineEvent::SessionThinking(th));
    let mut th = ma.thinking();
    th.tokens = Some(120);
    check(&g, "s-think-tokens", &EngineEvent::SessionThinking(th));
    let mut th = ma.thinking();
    th.tokens = Some(12);
    th.parent_id = Some("toolu_2".into());
    check(
        &g,
        "s-think-tokens-nested",
        &EngineEvent::SessionThinking(th),
    );
    let mut th = ma.thinking();
    th.text = Some("ok".into());
    th.seq = Some(3);
    th.replace = true;
    th.done = true;
    th.uuid = "u-2".into();
    check(&g, "s-think-final", &EngineEvent::SessionThinking(th));

    let mut st = route("", w, "CO", "PA2").step();
    st.id = "tu-1".into();
    st.tool = "Edit".into();
    st.tool_input = "a.go".into();
    st.description = "Edit a.go".into();
    st.input = r#"{"file_path":"a.go"}"#.into();
    st.uuid = "u-3".into();
    check(&g, "s-step", &EngineEvent::SessionStep(st));
    let mut st = co.step();
    st.id = "tu-2".into();
    st.tool = "Read".into();
    st.tool_input = "b.go".into();
    st.description = "Read b.go".into();
    st.input = r#"{"file_path":"b.go"}"#.into();
    st.parent_id = "toolu_1".into();
    check(&g, "s-step-nested", &EngineEvent::SessionStep(st));

    let mut r = co.tool_result();
    r.id = "tu-1".into();
    r.result = "ok <done>".into();
    check(&g, "s-result", &EngineEvent::SessionToolResult(r));
    let mut r = co.tool_result();
    r.id = "tu-1".into();
    r.result = "bad".into();
    r.is_error = true;
    r.input = Some(r#"{"name":"x"}"#.into());
    r.tool_input = Some("x".into());
    check(&g, "s-result-input", &EngineEvent::SessionToolResult(r));
    check(
        &g,
        "s-tool-input",
        &EngineEvent::SessionToolInput(SessionToolInput {
            code: "CO".into(),
            id: "tu-1".into(),
            input: r#"{"a":1}"#.into(),
            tool_input: "a=1".into(),
            workspace_id: w.into(),
        }),
    );

    let pa = route("", w, "PA", "");
    let mut c = pa.compact();
    c.status = "done".into();
    c.trigger = "auto".into();
    c.summary = Some("sum".into());
    c.pre_tokens = Some(9000);
    check(&g, "s-compact", &EngineEvent::SessionCompact(c));
    let mut c = pa.compact();
    c.status = "start".into();
    c.trigger = "manual".into();
    check(&g, "s-compact-bare", &EngineEvent::SessionCompact(c));
    let mut f = pa.fallback();
    f.from = "opus".into();
    f.to = "sonnet".into();
    f.from_label = "Opus".into();
    f.to_label = "Sonnet".into();
    f.retracted = vec!["u-1".into(), "u-2".into()];
    check(&g, "s-fallback", &EngineEvent::SessionFallback(f));
    let mut f = pa.fallback();
    f.to = "sonnet".into();
    f.to_label = "Sonnet".into();
    check(&g, "s-fallback-bare", &EngineEvent::SessionFallback(f));
    let usage = ContextUsage {
        input_tokens: 100,
        cache_creation_tokens: 20,
        cache_read_tokens: 300,
        context_window: 200_000,
    };
    check(&g, "s-context", &pa.context_usage(usage));
    check(&g, "s-turn-end", &pa.turn_end(1500, Some(usage)));
    check(
        &g,
        "s-turn-end-bare",
        &route("app", "", "APP", "").turn_end(800, None),
    );
    check(
        &g,
        "s-notice",
        &route("", w, "MA", "PA").notice("Retrying…"),
    );
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

fn check_role(g: &Value, key: &str, r: &RoleSpec) {
    let want = &g["roles"][key];
    assert_eq!(r.code, want["code"].as_str().unwrap(), "{key} code");
    assert_eq!(r.name, want["name"].as_str().unwrap(), "{key} name");
    assert_eq!(r.allowed_tools, strs(&want["allowedTools"]), "{key} tools");
    assert_eq!(
        r.model_slot,
        want["modelSlot"].as_str().unwrap(),
        "{key} slot"
    );
    assert_eq!(r.mode, want["mode"].as_str().unwrap(), "{key} mode");
}

#[test]
fn roles_and_tool_tables_match_go() {
    use recur_domain::AgentRole;
    let g = golden();
    check_role(&g, "coder", &role_spec(&AgentRole::Coder));
    check_role(&g, "reviewer", &role_spec(&AgentRole::Reviewer));
    check_role(&g, "tester", &role_spec(&AgentRole::Tester));
    check_role(&g, "qa", &role_spec(&AgentRole::Qa));
    check_role(&g, "security", &security_reviewer_role());
    assert_eq!(maestro_tools(), strs(&g["roles"]["maestroTools"]));
    assert_eq!(maestro_plan_tools(), strs(&g["roles"]["maestroPlanTools"]));
    assert_eq!(loop_flow_tools(), strs(&g["roles"]["loopFlowTools"]));
    assert_eq!(GIT_READ_TOOLS.to_vec(), strs(&g["roles"]["gitReadTools"]));
    assert_eq!(
        READ_EXPLORE_TOOLS.to_vec(),
        strs(&g["roles"]["readExploreTools"])
    );
    assert_eq!(EDIT_TOOLS.to_vec(), strs(&g["roles"]["editTools"]));
    for (mode, want) in g["roles"]["modeTools"].as_object().unwrap() {
        assert_eq!(mode_tools(mode), strs(want), "mode_tools({mode:?})");
    }
    // The fixture writes these with JSON names; bundles::Role reads TOML names.
    let sample: Vec<recur_bundles::Role> = g["inputs"]["sampleRoles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| recur_bundles::Role {
            mode: r["mode"].as_str().unwrap_or_default().into(),
            tools: r.get("tools").map(strs).unwrap_or_default(),
            disabled_tools: r.get("disabledTools").map(strs).unwrap_or_default(),
            ..Default::default()
        })
        .collect();
    let want = g["roles"]["roleAllowlists"].as_array().unwrap();
    for (r, w) in sample.iter().zip(want) {
        assert_eq!(role_allowlist(r), strs(w), "role_allowlist({r:?})");
    }
    for (name, want) in g["roles"]["mapRoleTool"].as_object().unwrap() {
        assert_eq!(
            map_role_tool(name),
            want.as_str().unwrap(),
            "map_role_tool({name:?})"
        );
    }
}

#[test]
fn prompts_match_go() {
    let g = golden();
    let p = &g["prompts"];
    let want = |k: &str| {
        p[k].as_str()
            .unwrap_or_else(|| panic!("prompt {k}"))
            .to_owned()
    };
    let findings: Vec<Finding> = input(&g, "findings");
    let mc = &g["inputs"]["maestroContext"];
    let ctx = MaestroContext {
        goal: mc["goal"].as_str().unwrap().into(),
        mission: mc["mission"].as_str().unwrap().into(),
        state: mc["state"].as_str().unwrap().into(),
        live: mc["live"].as_bool().unwrap(),
        iteration: mc["iteration"].as_i64().unwrap(),
        recent: strs(&mc["recent"]),
        impact: mc["impact"].as_str().unwrap().into(),
        pr_thread: mc["prThread"].as_str().unwrap().into(),
        handoff: mc["handoff"].as_str().unwrap().into(),
    };
    assert_eq!(
        security_reviewer_system_prompt("Rotate tokens"),
        want("securityReviewer")
    );
    assert_eq!(
        qa_system_prompt("Login works", "http://127.0.0.1:4321", "/tmp/shots"),
        want("qa")
    );
    assert_eq!(
        qa_runtime_extra("http://127.0.0.1:4321", "/tmp/shots"),
        want("qaRuntimeExtra")
    );
    assert_eq!(plan_section("  The plan  "), want("planSection"));
    assert_eq!(plan_section("   "), want("planSectionEmpty"));
    assert_eq!(
        coder_system_prompt("Add a flag", "1. do it", false),
        want("coder")
    );
    assert_eq!(
        coder_system_prompt("Add a flag", "1. do it", true),
        want("coderStructured")
    );
    assert_eq!(
        coder_system_prompt("Add a flag", "", false),
        want("coderNoPlan")
    );
    assert_eq!(coder_fix_prompt("Add a flag", &findings), want("coderFix"));
    assert_eq!(
        coder_conflict_prompt("Add a flag", &["a.go".into(), "b/c.go".into()]),
        want("coderConflict")
    );
    assert_eq!(
        coder_open_findings_nudge_prompt("Add a flag", &findings),
        want("coderNudge")
    );
    assert_eq!(
        coder_resume_prompt("Add a flag", 2, true),
        want("coderResumeRestored")
    );
    assert_eq!(
        coder_resume_prompt("Add a flag", 2, false),
        want("coderResumeFresh")
    );
    assert_eq!(
        maestro_plan_system_prompt("Add a flag"),
        want("maestroPlan")
    );
    assert_eq!(maestro_system_prompt(&ctx), want("maestro"));
    assert_eq!(
        maestro_system_prompt(&MaestroContext {
            goal: "same".into(),
            mission: "same".into(),
            state: "paused".into(),
            ..MaestroContext::default()
        }),
        want("maestroMinimal")
    );
    assert_eq!(
        reviewer_system_prompt("Add a flag", "the plan"),
        want("reviewer")
    );
    assert_eq!(
        reviewer_system_prompt("Add a flag", ""),
        want("reviewerNoPlan")
    );
}

#[test]
fn kitchen_words_and_guards_match_go() {
    let g = golden();
    assert_eq!(
        KITCHEN_ADJECTIVES.to_vec(),
        strs(&g["kitchen"]["adjectives"])
    );
    assert_eq!(KITCHEN_DISHES.to_vec(), strs(&g["kitchen"]["dishes"]));

    let json = |v: &dyn erased::Ser| v.json();
    let gd = &g["guards"];
    use recur_domain::AutonomyLevel as A;
    assert_eq!(
        json(&synthetic_policy("recur.policies.cautious", &A::Cautious)),
        gd["cautious"]
    );
    assert_eq!(
        json(&synthetic_policy(
            "recur.policies.aggressive",
            &A::Aggressive
        )),
        gd["aggressive"]
    );
    assert_eq!(json(&synthetic_policy("", &A::Balanced)), gd["balanced"]);
    assert_eq!(json(&synthetic_pr_loop()), gd["prLoop"]);
    let ph = recur_bundles::LoopPhase {
        roles: BTreeMap::from([
            ("implementer".to_owned(), "recur.roles.coder".to_owned()),
            ("reviewer".to_owned(), "recur.roles.reviewer".to_owned()),
        ]),
        ..Default::default()
    };
    assert_eq!(json(&synthetic_step_loop(ph)), gd["stepLoop"]);
    let parent = recur_bundles::LoopDef {
        id: "custom".into(),
        name: "Custom".into(),
        roles: BTreeMap::from([("implementer".to_owned(), "x".to_owned())]),
        bundle_name: "B".into(),
        bundle_id: "b.id".into(),
        source_root: "/src".into(),
        bundle_dir: "/src/B".into(),
        ..Default::default()
    };
    let conv = recur_bundles::LoopPhase {
        id: "conv".into(),
        uses: "converge".into(),
        start: "review".into(),
        gate: "g".into(),
        dispatch: "on_demand".into(),
        next: "x".into(),
        max_rounds: Some(3),
        roles: BTreeMap::from([("implementer".to_owned(), "test.roles.writer".to_owned())]),
        optional: true,
        ..Default::default()
    };
    assert_eq!(json(&child_loop_def(&parent, &conv)), gd["childLoop"]);
}

mod erased {
    /// Serializes to a `Value` for structural comparison with the fixture JSON
    /// (key order does not matter for these).
    pub trait Ser {
        fn json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn json(&self) -> serde_json::Value {
            serde_json::to_value(self).expect("serialize")
        }
    }
}
