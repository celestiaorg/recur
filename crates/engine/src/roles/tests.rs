//! Role and role tool tests.

use std::collections::BTreeMap;

use recur_agent_tools::names::mcp;
use recur_bundles::{
    ROLE_MODE_AGENT, ROLE_MODE_ASK, ROLE_MODE_LOOP, ROLE_MODE_PLAN, ROLE_MODE_REVIEW, Role,
};
use recur_domain::{AgentRole, Finding, Settings, Severity};

use super::*;

fn has(tools: &[String], want: &str) -> bool {
    tools.iter().any(|t| t == want)
}

#[test]
fn coder_tools_include_explore_and_edit() {
    let tools = role_spec(&AgentRole::Coder).allowed_tools;
    for want in [
        "Read",
        "Glob",
        "Grep",
        "LSP",
        "WebFetch",
        "WebSearch",
        "Edit",
        "Write",
        "Bash",
        "NotebookEdit",
        mcp::DONE,
        mcp::LIST_FINDINGS,
        mcp::UPDATE_FINDING,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
    ] {
        assert!(has(&tools, want), "Coder missing {want}; {tools:?}");
    }
    assert!(
        !has(&tools, mcp::REPORT_FINDING),
        "Coder must NOT have report_finding"
    );
}

#[test]
fn reviewer_is_read_only() {
    let tools = role_spec(&AgentRole::Reviewer).allowed_tools;
    for want in [
        "Read",
        "Glob",
        "Grep",
        "LSP",
        "WebFetch",
        "WebSearch",
        "Bash",
        mcp::REPORT_FINDING,
        mcp::LIST_FINDINGS,
        mcp::UPDATE_FINDING,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
    ] {
        assert!(has(&tools, want), "Reviewer missing {want}");
    }
    for banned in ["Edit", "Write", "NotebookEdit"] {
        assert!(!has(&tools, banned), "Reviewer must NOT have {banned}");
    }
    for want in GIT_READ_TOOLS {
        assert!(has(&tools, want), "Reviewer missing scoped git Bash {want}");
    }
    let deny = recur_provider_claude::args::disallowed_edit_tools(Some(&tools));
    for want in recur_provider_claude::args::EDIT_BUILTINS {
        assert!(
            deny.contains(&want),
            "Reviewer allowlist should disallow {want}, got {deny:?}"
        );
    }
    assert!(tools.iter().any(|t| t.starts_with("Bash(")));
}

#[test]
fn tester_tools_include_explore_and_edit() {
    let tools = role_spec(&AgentRole::Tester).allowed_tools;
    for want in [
        "Read",
        "Glob",
        "Grep",
        "WebFetch",
        "WebSearch",
        "Edit",
        "Write",
        "Bash",
        mcp::REPORT_RESULT,
        mcp::LIST_FINDINGS,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
    ] {
        assert!(has(&tools, want), "Tester missing {want}");
    }
}

#[test]
fn coder_dispatch_display_labels() {
    let findings = vec![Finding {
        id: "F1".into(),
        severity: Severity::Medium,
        file: "a.go".into(),
        line: 1,
        message: "off-by-one".into(),
        ..Finding::default()
    }];
    assert_eq!(
        coder_dispatch_display(&coder_fix_prompt("pong", &findings)),
        CODER_FIX_DISPLAY
    );
    assert_eq!(
        coder_dispatch_display(&coder_open_findings_nudge_prompt("pong", &findings)),
        CODER_FIX_DISPLAY
    );
    assert_eq!(
        coder_dispatch_display(&coder_resume_prompt("pong", 2, true)),
        CODER_RESUME_DISPLAY
    );
    assert_eq!(
        coder_dispatch_display(&coder_resume_prompt("pong", 2, false)),
        CODER_RESUME_DISPLAY
    );
    assert_eq!(coder_dispatch_display("create a cli game of pong"), "");
}

#[test]
fn qa_can_read() {
    let tools = role_spec(&AgentRole::Qa).allowed_tools;
    for want in [
        "Read",
        mcp::ATTACH_EVIDENCE,
        MCP_PLAYWRIGHT_ALL,
        mcp::REPORT_RESULT,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
    ] {
        assert!(has(&tools, want), "QA missing {want}");
    }
    for banned in ["Edit", "Write", "NotebookEdit"] {
        assert!(!has(&tools, banned), "QA must NOT have {banned}");
    }
}

#[test]
fn unknown_role_is_the_zero_spec() {
    assert_eq!(role_spec(&AgentRole::Maestro), RoleSpec::default());
}

// role_tools_test.go

#[test]
fn mode_tools_include_skill_mcp() {
    for mode in [
        ROLE_MODE_ASK,
        ROLE_MODE_PLAN,
        ROLE_MODE_AGENT,
        ROLE_MODE_REVIEW,
        ROLE_MODE_LOOP,
        "",
    ] {
        let got = mode_tools(mode);
        for want in [mcp::LIST_SKILLS, mcp::ACTIVATE_SKILL] {
            assert!(has(&got, want), "mode_tools({mode:?}) missing {want}");
        }
    }
}

#[test]
fn role_allowlist_ask_plus_extras() {
    let got = role_allowlist(&Role {
        mode: ROLE_MODE_ASK.into(),
        tools: vec![
            "attach_evidence".into(),
            "playwright".into(),
            "report_result".into(),
        ],
        ..Role::default()
    });
    for want in [
        "Read",
        mcp::ATTACH_EVIDENCE,
        MCP_PLAYWRIGHT_ALL,
        mcp::REPORT_RESULT,
        mcp::DONE,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
    ] {
        assert!(has(&got, want), "missing {want}; {got:?}");
    }
    assert!(!has(&got, "Edit"));
    assert!(role_wants_playwright(&Role {
        tools: vec![" playwright ".into()],
        ..Role::default()
    }));
}

#[test]
fn role_allowlist_drops_disabled() {
    let got = role_allowlist(&Role {
        mode: ROLE_MODE_AGENT.into(),
        disabled_tools: vec!["WebSearch".into()],
        ..Role::default()
    });
    assert!(!has(&got, "WebSearch"));
}

#[test]
fn maestro_tools_drop_retired_hands() {
    let t = maestro_tools();
    for banned in [mcp::RERUN_REVIEW, mcp::RESUME_LOOP, mcp::EXTEND_ITERATIONS] {
        assert!(
            !has(&t, banned),
            "conductor allowlist still has retired {banned}"
        );
    }
    for want in [
        mcp::EXTEND_ROUNDS,
        mcp::REENTER_PHASE,
        mcp::RUN_PHASE,
        mcp::DISPATCH_SPECIALIST,
        mcp::RENAME_SESSION,
        mcp::RENAME_WORKSPACE,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
        mcp::MESSAGE_CHILD,
    ] {
        assert!(has(&t, want), "conductor allowlist missing {want}");
    }
}

#[test]
fn model_for_slot_defaults_from_mode() {
    let set = Settings {
        model_agent: "openai/gpt-4.1".into(),
        model_review: "openai/o3".into(),
        model_loop: "openai/gpt-5".into(),
        ..Settings::default()
    };
    let none = BTreeMap::new();
    let spec = |slot: &str, mode: &str| RoleSpec {
        model_slot: slot.into(),
        mode: mode.into(),
        ..RoleSpec::default()
    };
    let (got, _) = model_for_slot(&set, &spec("", ROLE_MODE_AGENT), "fallback", &none);
    assert_eq!(got, set.agent_model().to_string());
    let (got, _) = model_for_slot(&set, &spec("", ROLE_MODE_REVIEW), "fallback", &none);
    assert_eq!(got, set.review_model().to_string());
    let (got, _) = model_for_slot(
        &set,
        &spec(ROLE_MODE_LOOP, ROLE_MODE_AGENT),
        "fallback",
        &none,
    );
    assert_eq!(got, set.loop_model().to_string());
    let (got, flags) = model_for_slot(
        &set,
        &spec("", ""),
        "fallback",
        &BTreeMap::from([("k".into(), "v".into())]),
    );
    assert_eq!(got, "fallback");
    assert_eq!(flags.get("k").map(String::as_str), Some("v"));
    let (got, _) = model_for_slot(&set, &spec("weird", ""), "fallback", &none);
    assert_eq!(got, "fallback");
}

#[test]
fn slot_role_spec_prefers_phase_then_loop_roles() {
    use crate::loop_handle::RoleView;
    use std::sync::Arc;
    let catalog = Arc::new(vec![
        Role {
            id: "a.writer".into(),
            code: "WR".into(),
            name: "Writer".into(),
            mode: "agent".into(),
            model_slot: "plan".into(),
            ..Role::default()
        },
        Role {
            id: "a.editor".into(),
            mode: "review".into(),
            ..Role::default()
        },
    ]);
    let view = RoleView {
        loop_roles: BTreeMap::from([("implementer".into(), "a.writer".into())]),
        phase_roles: Some(BTreeMap::from([("reviewer".into(), " a.editor ".into())])),
        catalog_roles: catalog,
    };
    let coder = role_spec(&AgentRole::Coder);
    let impl_spec = slot_role_spec(&view, "implementer", coder.clone());
    assert_eq!(
        (impl_spec.code.as_str(), impl_spec.name.as_str()),
        ("WR", "Writer")
    );
    assert_eq!(impl_spec.model_slot, "plan");
    assert_eq!(
        impl_spec.allowed_tools,
        role_allowlist(&view.catalog_roles[0])
    );
    let rev = slot_role_spec(&view, "reviewer", role_spec(&AgentRole::Reviewer));
    assert_eq!(rev.code, "RV", "an empty catalog code keeps the fallback's");
    assert_eq!(rev.mode, "review");
    assert_eq!(slot_role_spec(&view, "planner", coder.clone()), coder);
}

#[test]
fn disputed_findings_section_scopes_to_previous_iteration() {
    let store = recur_store::Store::new_in_memory();
    let add = |id: &str, iter: i64, by: &str, reason: &str| {
        let f = store.add_finding(Finding {
            id: id.into(),
            workspace_id: "w".into(),
            iteration: iter,
            file: "a.go".into(),
            line: 2,
            severity: Severity::High,
            message: format!("m-{id}"),
            status: "open".into(),
            ..Finding::default()
        });
        let _ = store.update_finding_status("w", &f.id, "invalid", reason, by);
    };
    add("f1", 2, recur_domain::FINDING_ACTOR_CODER, "not a bug");
    add("f2", 1, recur_domain::FINDING_ACTOR_CODER, "old");
    add("f3", 2, recur_domain::FINDING_ACTOR_REVIEWER, "self");
    assert_eq!(coder_disputed_findings_section(&store, "w", 1), "");
    let s = coder_disputed_findings_section(&store, "w", 3);
    assert!(
        s.contains("m-f1") && s.contains("Coder reason: not a bug"),
        "{s}"
    );
    assert!(!s.contains("m-f2") && !s.contains("m-f3"), "{s}");
    let full = reviewer_prompt_for(&store, "w", "task", "", 3);
    assert!(full.starts_with(&reviewer_system_prompt("task", "")));
    assert!(full.ends_with(&s));
}
