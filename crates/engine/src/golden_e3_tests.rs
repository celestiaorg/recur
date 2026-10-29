//! Go-generated golden fixtures for slice E3 (R2): loop error strings and
//! prompts, the PR fallback text, handoff/replan/reviewer briefings, the
//! findings threshold line, wrap-up HTML and elapsed labels, title rules,
//! the child step plan, the PR thread, and — from real walks — the cap and
//! plan-approval escalations and the Maestro's feed lines
//! (`tests/golden/go_e3.json`, generator `tests/golden/gen_e3_golden.go.txt`).
//! Walk ids are written as `$ID`, commit hashes `$HASH`, durations `$DUR`.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use pretty_assertions::assert_eq;
use recur_agent_tools::{Host, PlanSubmission};
use recur_bundles::FindingsThreshold;
use recur_domain::{
    AutonomyLevel, Commit, Escalation, Plan, PlanStep, Pr, Workspace, WorkspaceState,
};
use regex::Regex;
use serde_json::{Value, json};

use crate::chat_shared::*;
use crate::codes::MAESTRO_AGENT_CODE;
use crate::host::EngineHost;
use crate::loops::board::BOARD_CAP_PHASE;
use crate::loops::fanout::child_step_plan;
use crate::loops::handoff::{handoff_section, maestro_replan_system_prompt, maestro_replan_tools};
use crate::loops::hands::{RERUN_REVIEW_INSTRUCTION, dispatch_reviewer_system_prompt};
use crate::loops::lifecycle::{ERR_ALREADY_DISPATCHED, ERR_LOOP_OWNER_GONE};
use crate::loops::maestro::strip_tags;
use crate::loops::open_pr::{format_pr_thread, pr_fallback_intro, pr_fallback_markdown, pr_title};
use crate::loops::prompts::format_findings_threshold;
use crate::loops::test_support::{CaptureScript, ConvergeRunner};
use crate::loops::walker::{ERR_PHASE_FAILED, ERR_VERIFY_REJECTED};
use crate::loops::wrap_up::*;
use crate::testing::{TestEngine, add_project, init_repo, wait_for};
use crate::turn::test_support::maestro_fixture;

const GOLDEN: &str = include_str!("../tests/golden/go_e3.json");

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

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|x| s(x).to_owned()).collect())
        .unwrap_or_default()
}

static HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-f]{7,40}\b").unwrap());
static DUR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Completed in [^.·<]+").unwrap());

fn scrub(text: &str, id: &str) -> String {
    let text = if id.is_empty() {
        text.to_owned()
    } else {
        text.replace(id, "$ID")
    };
    let text = HASH.replace_all(&text, "$$HASH");
    DUR.replace_all(&text, "Completed in $$DUR").into_owned()
}

fn esc_json(esc: &Escalation, id: &str) -> Value {
    let opts: Vec<Value> = esc
        .options
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|o| json!({"text": o.text, "tag": o.tag, "desc": o.desc}))
        .collect();
    json!({
        "question": scrub(&esc.question, id),
        "context": scrub(&esc.context, id),
        "options": opts,
    })
}

fn maestro_feed(t: &TestEngine, id: &str) -> Vec<String> {
    t.store
        .feed(id)
        .unwrap_or_default()
        .into_iter()
        .filter(|it| it.agent_code == MAESTRO_AGENT_CODE)
        .map(|it| format!("{}: {}", it.name, scrub(&it.text, id)))
        .collect()
}

#[test]
fn constants_match_go() {
    let g = golden();
    let c = &g["constants"];
    let want = [
        ("errAlreadyDispatched", ERR_ALREADY_DISPATCHED.to_owned()),
        ("errLoopOwnerGone", ERR_LOOP_OWNER_GONE.to_owned()),
        ("errVerifyRejected", ERR_VERIFY_REJECTED.to_owned()),
        ("errPhaseFailed", ERR_PHASE_FAILED.to_owned()),
        ("boardCapPhase", BOARD_CAP_PHASE.to_owned()),
        ("wrapUpSystemPrompt", WRAP_UP_SYSTEM_PROMPT.to_owned()),
        (
            "firstWorkspaceRenamePrompt",
            FIRST_WORKSPACE_RENAME_PROMPT.to_owned(),
        ),
        (
            "firstWorkspaceRenamePromptNoBranch",
            FIRST_WORKSPACE_RENAME_PROMPT_NO_BRANCH.to_owned(),
        ),
        ("firstChatRenamePrompt", FIRST_CHAT_RENAME_PROMPT.to_owned()),
        (
            "rerunReviewInstruction",
            RERUN_REVIEW_INSTRUCTION.to_owned(),
        ),
        ("maestroReplanTools", maestro_replan_tools().join(",")),
    ];
    for (k, v) in want {
        assert_eq!(s(&c[k]), v, "{k}");
    }
}

#[test]
fn pr_text_matches_go() {
    let g = golden();
    for c in cases(&g, "prTitle") {
        assert_eq!(pr_title(s(&c["in"])), s(&c["out"]));
    }
    for c in cases(&g, "prFallbackIntro") {
        let i = strs(&c["in"]);
        assert_eq!(pr_fallback_intro(&i[0], &i[1]), s(&c["out"]));
    }
    for c in cases(&g, "githubPRMarkdown") {
        let i = &c["in"];
        let commits: Vec<Commit> = serde_json::from_value(i["commits"].clone()).unwrap();
        let got = pr_fallback_markdown(
            s(&i["goal"]),
            &commits,
            i["fileCount"].as_i64().unwrap(),
            i["add"].as_i64().unwrap(),
            i["del"].as_i64().unwrap(),
            &strs(&i["notes"]),
        );
        assert_eq!(got, s(&c["out"]));
    }
    let c = &g["formatPRThread"];
    let pr: Pr = serde_json::from_value(c["in"].clone()).unwrap();
    assert_eq!(format_pr_thread(&pr), s(&c["out"]));
}

#[test]
fn briefings_match_go() {
    let g = golden();
    for c in cases(&g, "handoffSection") {
        assert_eq!(handoff_section(s(&c["in"])), s(&c["out"]));
    }
    let c = &g["maestroReplanSystemPrompt"];
    let i = strs(&c["in"]);
    assert_eq!(
        maestro_replan_system_prompt(&i[0], &i[1], &i[2]),
        s(&c["out"])
    );
    let c = &g["dispatchReviewerSystemPrompt"];
    assert_eq!(dispatch_reviewer_system_prompt(s(&c["in"])), s(&c["out"]));
    for c in cases(&g, "formatFindingsThreshold") {
        let th = FindingsThreshold {
            all_zero: c["in"]["allZero"].as_bool().unwrap_or(false),
            levels: c["in"]["levels"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_i64().unwrap()))
                        .collect()
                })
                .unwrap_or_default(),
        };
        assert_eq!(format_findings_threshold(&th), s(&c["out"]), "{}", c["in"]);
    }
}

#[test]
fn wrap_up_text_matches_go() {
    let g = golden();
    for c in cases(&g, "loopWrapUp") {
        let p = s(&c["in"]);
        assert_eq!(loop_wrap_up_html(p), s(&c["out"]["html"]), "{p:?}");
        assert_eq!(sanitize_wrap_prose(p), s(&c["out"]["sanitized"]), "{p:?}");
    }
    for c in cases(&g, "formatLoopElapsed") {
        let ms = c["in"].as_u64().unwrap();
        assert_eq!(
            format_loop_elapsed(Duration::from_millis(ms)),
            s(&c["out"]),
            "{ms}ms"
        );
    }
    for c in cases(&g, "isCompletionFact") {
        assert_eq!(
            is_completion_fact(s(&c["in"])),
            c["out"].as_bool().unwrap(),
            "{}",
            c["in"]
        );
    }
    for c in cases(&g, "stripTags") {
        assert_eq!(strip_tags(s(&c["in"])), s(&c["out"]));
    }
}

#[test]
fn title_rules_match_go() {
    let g = golden();
    for c in cases(&g, "titles") {
        let i = s(&c["in"]);
        assert_eq!(
            provisional_workspace_title(i),
            s(&c["out"]["provisional"]),
            "{i:?}"
        );
        assert_eq!(truncate_words(i, 3), s(&c["out"]["words3"]), "{i:?}");
    }
    let c = &g["standingPromptSHA"];
    assert_eq!(standing_prompt_sha(s(&c["in"])), s(&c["out"]));
}

#[test]
fn child_step_plan_matches_go() {
    let g = golden();
    let c = &g["childStepPlan"];
    let parent: Workspace = serde_json::from_value(c["in"]["parent"].clone()).unwrap();
    let steps: Vec<PlanStep> = serde_json::from_value(c["in"]["steps"].clone()).unwrap();
    let got: Plan = child_step_plan(&parent, &steps);
    let want: Plan = serde_json::from_value(c["out"].clone()).unwrap();
    assert_eq!(got, want);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cap_escalation_and_feed_match_go() {
    let g = golden();
    let runner = ConvergeRunner::new(&[2; 8]);
    let t = TestEngine::new(Some(runner.provider()));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(
            &proj,
            "never converges, must escalate",
            "",
            AutonomyLevel::default(),
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(15), "cap escalation", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Escalated)
    })
    .await;
    let esc = t.store.escalation(&id).unwrap();
    assert_eq!(esc_json(&esc, &id), g["capEscalation"]);
    assert!(t.engine.answer_escalation(&id, "Stop", "").await.is_some());
    wait_for(Duration::from_secs(10), "stopped", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Stopped)
            && t.engine.running_loop_by_id(&id).is_none()
    })
    .await;
    assert_eq!(maestro_feed(&t, &id), strs(&g["capFeed"]));
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan_escalation_matches_go() {
    let g = golden();
    let runner = CaptureScript::new(|_| maestro_fixture("gate", "OK."));
    let t = TestEngine::new(Some(runner.provider()));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t.engine.create_workspace(&proj, "gated", "").await.unwrap();
    EngineHost::new(Arc::downgrade(&t.engine))
        .submit_plan(
            &ws.id,
            "PA",
            PlanSubmission {
                summary: "Wire the toggle".into(),
                steps: vec!["Edit settings".into(), "Add persistence".into()],
                ..PlanSubmission::default()
            },
        )
        .await;
    t.engine
        .start_run_on_workspace(&ws.id, "ship the toggle")
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "plan escalation", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|l| l.state == WorkspaceState::Escalated)
    })
    .await;
    let esc = t.store.escalation(&ws.id).unwrap();
    assert_eq!(esc_json(&esc, &ws.id), g["planEscalation"]);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn converge_feed_matches_go() {
    let g = golden();
    let runner = ConvergeRunner::new(&[1, 0]);
    let t = TestEngine::new(Some(runner.provider()));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(
            &proj,
            "Add a converging feature",
            "",
            AutonomyLevel::default(),
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(15), "pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
            && t.engine.running_loop_by_id(&id).is_none()
    })
    .await;
    assert_eq!(maestro_feed(&t, &id), strs(&g["convergeFeed"]));
    t.close().await;
}
