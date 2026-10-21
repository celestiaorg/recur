//! Port of the wrap-up cases of Go `pr_test.go`: the escaped wrap HTML, the
//! prompt over every landing, the idle Completed fact (never on a child),
//! the PR fact patch, and the elapsed label.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use recur_domain::{AgentRole, Pr, PrSource, Workspace, WorkspaceState};

use crate::loop_handle::{LoopHandle, LoopState};
use crate::loops::WrapUpInputs;
use crate::loops::test_support::*;
use crate::loops::wrap_up::{format_loop_elapsed, loop_wrap_up_html};
use crate::testing::TestEngine;
use crate::turn::test_support::maestro_fixture;

fn add_ws(t: &TestEngine, id: &str, goal: &str) {
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            project_id: "P1".into(),
            goal: goal.into(),
            ..Workspace::default()
        })
        .unwrap();
}

fn idle_handle(id: &str) -> Arc<LoopHandle> {
    LoopHandle::new(
        id,
        "P1",
        id,
        LoopState {
            exit_state: WorkspaceState::Paused,
            exit_phase: "idle".into(),
            ..LoopState::default()
        },
    )
}

fn feed(t: &TestEngine, id: &str) -> Vec<recur_domain::FeedItem> {
    t.store.feed(id).unwrap_or_default()
}

#[test]
fn loop_wrap_up_html_cases() {
    let got = loop_wrap_up_html(
        "Root cause: race in checkout.\nLanded retries in the parent and both children.",
    );
    assert!(got.contains(r#"class="loop-wrap""#), "{got}");
    assert!(got.contains("Root cause: race in checkout."));
    assert!(got.contains("<br>"));
    assert_eq!(loop_wrap_up_html("  "), "");
    assert!(!loop_wrap_up_html("<script>x</script>").contains("<script>"));
    let fenced = loop_wrap_up_html("```\nRoot cause: nil deref\n```");
    assert!(fenced.contains("Root cause: nil deref") && !fenced.contains("```"));
}

#[tokio::test]
async fn wrap_up_prompt_sees_every_landing() {
    let t = TestEngine::new(None);
    add_ws(&t, "L-fan", "Fix flaky checkout");
    let h = idle_handle("L-fan");
    h.note_done_summary("Add cart mutex", "");
    h.note_done_summary("Retry payment token", "");
    let got = t.engine.wrap_up_prompt(
        &h,
        &WrapUpInputs {
            id: "L-fan".into(),
            goal: "Fix flaky checkout".into(),
            review_handoff: "S1: mutex in cart\nS2: retry the token".into(),
            ..WrapUpInputs::default()
        },
    );
    for want in [
        "Fix flaky checkout",
        "S1: mutex in cart",
        "S2: retry the token",
        "Add cart mutex",
        "Retry payment token",
        "every turn, not one Coder",
    ] {
        assert!(got.contains(want), "missing {want}: {got}");
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emit_settled_wrap_up_idle() {
    let reply = "Root cause: race in checkout. Landed retries in the parent and both child steps.";
    let runner = CaptureScript::new(move |_| maestro_fixture("wrap-1", reply));
    let t = TestEngine::new(Some(runner.provider()));
    add_ws(&t, "L-wrap", "Fix flaky checkout");
    let h = idle_handle("L-wrap");
    t.engine
        .emit_settled_wrap_up(
            &h,
            &WrapUpInputs {
                id: "L-wrap".into(),
                goal: "Fix flaky checkout".into(),
                review_handoff: "S1: mutex\nS2: retry".into(),
                started_at: Some(Instant::now() - Duration::from_secs(12)),
                ..WrapUpInputs::default()
            },
        )
        .await;
    let saw = runner.spec_at(0).expect("a wrap-up turn");
    assert!(saw.prompt.contains("Fix flaky checkout") && saw.prompt.contains("S1: mutex"));
    assert_eq!(saw.role, AgentRole::default(), "not a chat turn");

    let done: Vec<_> = feed(&t, "L-wrap")
        .into_iter()
        .filter(|it| it.text.contains("Completed in"))
        .collect();
    assert!(!done.is_empty(), "idle completion in the feed");
    for it in &done {
        assert!(it.text.contains(r#"class="loop-wrap""#), "{}", it.text);
        assert!(it.text.contains("Landed retries in the parent"));
        assert!(!it.text.contains("data-pr="), "not a PR fact");
    }

    let child = idle_handle("L-child");
    let before = feed(&t, "L-wrap").len();
    t.engine
        .emit_settled_wrap_up(
            &child,
            &WrapUpInputs {
                id: "L-child".into(),
                goal: "S1".into(),
                parent_id: "L-wrap".into(),
                ..WrapUpInputs::default()
            },
        )
        .await;
    assert_eq!(
        feed(&t, "L-wrap").len(),
        before,
        "a child never wraps up on the parent feed"
    );
    t.close().await;
}

#[tokio::test]
async fn attach_loop_wrap_patches_pr_fact() {
    let t = TestEngine::new(None);
    add_ws(&t, "L-prw", "t");
    t.engine.emit_feed_pr_created(
        "L-prw",
        &Pr {
            id: "12".into(),
            number: 12,
            source: PrSource::Local,
            ..Pr::default()
        },
        Duration::from_secs(5),
    );
    t.engine.attach_loop_wrap(
        "L-prw",
        &loop_wrap_up_html("Root cause: nil deref. Guarded the lookup."),
    );
    let got = feed(&t, "L-prw")
        .into_iter()
        .find(|it| it.text.contains(r#"data-pr="12""#))
        .map(|it| it.text)
        .unwrap_or_default();
    assert!(
        got.contains("Completed in") && got.contains("Root cause: nil deref"),
        "{got}"
    );
    t.close().await;
}

#[test]
fn format_loop_elapsed_cases() {
    for (d, want) in [
        (Duration::from_secs(3), "3s"),
        (Duration::from_secs(90), "1m 30s"),
        (Duration::from_secs(5 * 60), "5m"),
        (Duration::from_secs(62 * 60), "1h 2m"),
        (Duration::from_secs(2 * 3600), "2h"),
    ] {
        assert_eq!(format_loop_elapsed(d), want, "{d:?}");
    }
}
