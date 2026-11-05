//! The shell service and schedules.

use chrono::{Datelike, TimeZone, Timelike};
use recur_domain::{Project, ScheduledPrompt};

use super::{Fx, FxOpts, write};
use crate::api::{IssuesView, ShellState};
use crate::app::projects::preview_scheduled_prompt;
use crate::app::shell::{
    ISSUES_VIEW_FILE, SCHEDULE_VIEW_FILE, SHELL_SAVE_DEBOUNCE, SHELL_STATE_FILE, UI_VIEW_FILE,
    clamp_shell_state,
};

fn st(view: &str, ws: &str) -> ShellState {
    ShellState {
        view: view.into(),
        workspace_id: ws.into(),
        ..Default::default()
    }
}

/// Nothing cached, reads from disk.
async fn fresh_shell(fx: &Fx) -> std::sync::Arc<dyn crate::api::ShellService> {
    fx.reopen().await.services().shell
}

fn file_state(fx: &Fx) -> ShellState {
    serde_json::from_slice(&std::fs::read(fx.root.join(SHELL_STATE_FILE)).unwrap()).unwrap()
}

#[test]
fn clamp_shell_state_keeps_view_ids() {
    let got = clamp_shell_state(ShellState {
        view: "detail".into(),
        workspace_id: "WS-1".into(),
        pr_id: "pr-1".into(),
        project_id: "p1".into(),
    });
    assert_eq!(got, st("detail", "WS-1"), "detail keeps only the workspace");
    for view in ["inbox", "schedules", "issues", "briefing", "terminals"] {
        assert_eq!(
            clamp_shell_state(st(view, "WS-1")),
            st(view, ""),
            "{view} drops ids"
        );
    }
    assert_eq!(clamp_shell_state(st("nope", "WS-1")), ShellState::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_state_round_trip() {
    let fx = Fx::new().await;
    let sh = &fx.s.shell;
    assert_eq!(sh.get_shell_state().await.unwrap(), ShellState::default());
    let want = st("detail", "WS-9");
    sh.set_shell_state(&want).await.unwrap();
    assert_eq!(
        sh.get_shell_state().await.unwrap(),
        want,
        "memory before flush"
    );
    sh.flush().await.unwrap();
    assert_eq!(file_state(&fx), want);
    assert_eq!(
        fresh_shell(&fx).await.get_shell_state().await.unwrap(),
        want,
        "load"
    );
    sh.set_shell_state(&want).await.unwrap();
    sh.set_shell_state(&st("inbox", "WS-9")).await.unwrap();
    sh.flush().await.unwrap();
    assert_eq!(
        fresh_shell(&fx).await.get_shell_state().await.unwrap(),
        st("inbox", "")
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_shell_state_unknown_preserves_file() {
    let fx = Fx::new().await;
    let sh = &fx.s.shell;
    let want = st("detail", "WS-9");
    sh.set_shell_state(&want).await.unwrap();
    sh.flush().await.unwrap();
    sh.set_shell_state(&st("nope", "WS-9")).await.unwrap();
    sh.set_shell_state(&st("welcome", "")).await.unwrap();
    assert_eq!(sh.get_shell_state().await.unwrap(), want);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_shell_state_debounced_until_flush() {
    let fx = Fx::new().await;
    let sh = &fx.s.shell;
    let want = st("detail", "WS-1");
    sh.set_shell_state(&want).await.unwrap();
    assert!(
        !fx.root.join(SHELL_STATE_FILE).exists(),
        "written before flush"
    );
    assert_eq!(sh.get_shell_state().await.unwrap(), want);
    sh.flush().await.unwrap();
    assert_eq!(file_state(&fx), want);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn flush_retries_after_write_error() {
    let fx = Fx::new().await;
    let sh = &fx.s.shell;
    let want = st("settings", "");
    sh.set_shell_state(&want).await.unwrap();
    let block = fx.root.join(SHELL_STATE_FILE);
    std::fs::create_dir(&block).unwrap();
    sh.flush().await.unwrap();
    assert!(
        fx.svc.shell_fail_delay() >= SHELL_SAVE_DEBOUNCE * 2,
        "backoff not armed"
    );
    assert!(
        !fx.root.join(format!("{SHELL_STATE_FILE}.tmp")).exists(),
        "tmp left behind"
    );
    std::fs::remove_dir(&block).unwrap();
    sh.flush().await.unwrap();
    assert_eq!(file_state(&fx), want);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn load_shell_state_corrupt() {
    let fx = Fx::new().await;
    write(fx.root.join(SHELL_STATE_FILE), "{not json");
    assert_eq!(
        fx.s.shell.get_shell_state().await.unwrap(),
        ShellState::default()
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn expanded_schedules_round_trip() {
    let fx = Fx::new().await;
    let sh = &fx.s.shell;
    assert!(sh.get_expanded_schedules().await.unwrap().is_empty());
    let ids = ["a", "", "a", "b"].map(String::from);
    sh.set_expanded_schedules(&ids).await.unwrap();
    assert_eq!(sh.get_expanded_schedules().await.unwrap(), ["a", "b"]);
    let file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fx.root.join(SCHEDULE_VIEW_FILE)).unwrap()).unwrap();
    assert_eq!(file["expanded"], serde_json::json!(["a", "b"]));
    let reload = fresh_shell(&fx)
        .await
        .get_expanded_schedules()
        .await
        .unwrap();
    assert_eq!(reload, ["a", "b"]);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn expanded_schedules_corrupt() {
    let fx = Fx::new().await;
    write(fx.root.join(SCHEDULE_VIEW_FILE), "{not json");
    assert!(
        fx.s.shell
            .get_expanded_schedules()
            .await
            .unwrap()
            .is_empty()
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn issues_view_round_trip() {
    let fx = Fx::new().await;
    let sh = &fx.s.shell;
    assert_eq!(sh.get_issues_view().await.unwrap(), IssuesView::default());
    let want = IssuesView {
        layout: "board".into(),
        source: "linear".into(),
        owner: "mine".into(),
        groups: vec!["todo".into()],
        sort: "priority".into(),
        projects: vec!["p1".into()],
    };
    sh.set_issues_view(&want).await.unwrap();
    assert_eq!(
        fresh_shell(&fx).await.get_issues_view().await.unwrap(),
        want
    );
    write(fx.root.join(ISSUES_VIEW_FILE), "{not json");
    assert_eq!(sh.get_issues_view().await.unwrap().layout, "");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ui_zoom_round_trip_and_clamp() {
    let fx = Fx::new().await;
    let sh = &fx.s.shell;
    assert_eq!(sh.get_ui_zoom().await.unwrap(), 1.);
    sh.set_ui_zoom(1.25).await.unwrap();
    assert_eq!(fresh_shell(&fx).await.get_ui_zoom().await.unwrap(), 1.25);
    for (set, want) in [(0.1, 0.5), (9., 2.), (0., 1.)] {
        sh.set_ui_zoom(set).await.unwrap();
        assert_eq!(sh.get_ui_zoom().await.unwrap(), want);
    }
    write(fx.root.join(UI_VIEW_FILE), "{not json");
    assert_eq!(sh.get_ui_zoom().await.unwrap(), 1.);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_scheduled_prompts_across_projects() {
    let fx = Fx::with(FxOpts {
        file_store: true,
        ..FxOpts::default()
    })
    .await;
    for id in ["one", "two"] {
        fx.store()
            .add_project(Project {
                id: id.into(),
                name: id.into(),
                ..Default::default()
            })
            .unwrap();
        fx.store()
            .save_scheduled_prompt(&ScheduledPrompt {
                id: id.into(),
                project_id: id.into(),
                name: id.into(),
                prompt: "p".into(),
                base_branch: "main".into(),
                mode: "ask".into(),
                model: "claude-code/sonnet".into(),
                cadence: "cron".into(),
                cron: "0 9 * * *".into(),
                time_zone: "UTC".into(),
                ..Default::default()
            })
            .unwrap();
    }
    let p = &fx.s.projects;
    assert_eq!(p.list_scheduled_prompts("").await.unwrap().len(), 2);
    for id in ["one", "two"] {
        let got = p.list_scheduled_prompts(id).await.unwrap();
        assert_eq!(
            got.iter()
                .map(|s| s.project_id.as_str())
                .collect::<Vec<_>>(),
            [id]
        );
    }
    fx.close().await;
}

#[test]
fn preview_scheduled_prompt_next_three() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();
    let mut p = ScheduledPrompt {
        cadence: "cron".into(),
        cron: "0 9 1,15 * *".into(),
        time_zone: "UTC".into(),
        ..Default::default()
    };
    let times = preview_scheduled_prompt(&p, now).unwrap();
    assert_eq!(times.len(), 3);
    for (i, at) in times.iter().enumerate() {
        let d = chrono::DateTime::from_timestamp_millis(*at).unwrap();
        assert!(d.hour() == 9 && matches!(d.day(), 1 | 15), "{times:?}");
        assert!(i == 0 || *at > times[i - 1], "{times:?}");
    }
    p.cron = "bad cron".into();
    assert!(preview_scheduled_prompt(&p, now).is_err());
    p.cron = "@reboot".into();
    assert!(preview_scheduled_prompt(&p, now).unwrap().is_empty());
}
