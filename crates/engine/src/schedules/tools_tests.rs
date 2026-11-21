//! create_schedule, list_schedules and delete_schedule through the host:
//! defaults, validation surfaced before approval, the permission gate and
//! the project scope.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone, Utc};
use recur_agent_tools::{Host, ScheduleRequest};
use recur_domain::{PermissionDecision, ScheduledPrompt, Workspace};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::host::EngineHost;
use crate::testing::{TestEngine, add_project, init_repo, wait_for};

struct Tx {
    te: TestEngine,
    repo: crate::testing::TempRepo,
}

impl Tx {
    fn new(bypass: bool) -> Self {
        let te = TestEngine::new(None);
        let repo = init_repo();
        let proj = add_project(&te.store, &repo.path);
        te.store
            .add_workspace(Workspace {
                id: "ws".into(),
                project_id: proj,
                goal: "g".into(),
                ..Default::default()
            })
            .unwrap();
        let mut set = te.store.settings();
        set.bypass_permissions = bypass;
        te.store.set_settings(set);
        let t = Utc.with_ymd_and_hms(2026, 11, 19, 12, 0, 0).unwrap();
        te.engine.set_clock(move || t.with_timezone(&Local));
        Self { te, repo }
    }

    fn host(&self) -> EngineHost {
        EngineHost::new(Arc::downgrade(&self.te.engine))
    }

    async fn create(&self, req: ScheduleRequest) -> Result<Value, String> {
        self.host()
            .create_schedule(&CancellationToken::new(), "ws", "PA", req)
            .await
            .map(|r| serde_json::from_str(&r).unwrap())
            .map_err(|e| e.0)
    }

    async fn list(&self) -> Value {
        serde_json::from_str(&self.host().list_schedules("ws", "PA").await.unwrap()).unwrap()
    }
}

fn daily() -> ScheduleRequest {
    ScheduleRequest {
        name: "Nightly".into(),
        prompt: "Run the slow tests".into(),
        cadence: "cron".into(),
        cron: "0 9 * * MON-FRI".into(),
        time_zone: "UTC".into(),
        mode: "agent".into(),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_list_delete_with_defaults() {
    let t = Tx::new(true);
    let made = t.create(daily()).await.unwrap();
    assert_eq!(
        made["model"],
        recur_domain::providers::FALLBACK_CLAUDE_MODEL
    );
    assert_eq!(made["baseBranch"], "main");
    assert_eq!(made["nextRun"], "2026-11-20 09:00 UTC");
    let id = made["id"].as_str().unwrap().to_owned();

    let zone = t
        .create(ScheduleRequest {
            time_zone: String::new(),
            ..daily()
        })
        .await
        .unwrap();
    assert!(!zone["timeZone"].as_str().unwrap().is_empty());

    t.te.store
        .add_project(recur_domain::Project {
            id: "other".into(),
            name: "other".into(),
            repo_path: t.repo.path.clone(),
            ..Default::default()
        })
        .unwrap();
    t.te.store
        .save_scheduled_prompt(&ScheduledPrompt {
            id: "elsewhere".into(),
            project_id: "other".into(),
            name: "x".into(),
            prompt: "x".into(),
            mode: "ask".into(),
            model: "claude-code/sonnet".into(),
            cadence: "cron".into(),
            cron: "0 9 * * *".into(),
            time_zone: "UTC".into(),
            ..Default::default()
        })
        .unwrap();
    let listed = t.list().await;
    assert_eq!(listed["count"], 2, "this project only: {listed}");
    assert_eq!(listed["schedules"][0]["name"], "Nightly");

    let h = t.host();
    let cancel = CancellationToken::new();
    let err = h
        .delete_schedule(&cancel, "ws", "PA", "elsewhere")
        .await
        .unwrap_err();
    assert!(err.0.contains("not found in this project"), "{err}");
    let gone = h.delete_schedule(&cancel, "ws", "PA", &id).await.unwrap();
    assert!(gone.contains("\"deleted\":true"), "{gone}");
    assert_eq!(t.list().await["count"], 1);
    t.te.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_timing_is_reported_before_any_approval() {
    let t = Tx::new(false);
    for (req, want) in [
        (
            ScheduleRequest {
                cron: "61 * * * *".into(),
                ..daily()
            },
            "invalid cron expression",
        ),
        (
            ScheduleRequest {
                time_zone: "Mars/Base".into(),
                ..daily()
            },
            "choose a valid IANA time zone",
        ),
        (
            ScheduleRequest {
                cadence: "once".into(),
                date: "2026-11-20".into(),
                time: "9am".into(),
                ..daily()
            },
            "time must be HH:MM",
        ),
    ] {
        let err = t.create(req).await.unwrap_err();
        assert!(err.contains(want), "{err}");
    }
    assert!(t.te.engine.pending_permissions().is_empty());
    t.te.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ask_mode_routes_create_through_approval() {
    let t = Tx::new(false);
    let task = {
        let h = t.host();
        tokio::spawn(async move {
            h.create_schedule(&CancellationToken::new(), "ws", "PA", daily())
                .await
        })
    };
    let eng = t.te.engine.clone();
    wait_for(Duration::from_secs(5), "permission request", || {
        !eng.pending_permissions().is_empty()
    })
    .await;
    let req = eng.pending_permissions().remove(0);
    assert_eq!(req.tool, "mcp__recur__create_schedule");
    assert!(eng.answer_permission(
        &req.id,
        PermissionDecision {
            allow: false,
            reason: "not now".into(),
            ..Default::default()
        }
    ));
    let err = task.await.unwrap().unwrap_err();
    assert_eq!(err.0, "permission denied: not now");
    assert_eq!(t.list().await["count"], 0);
    t.te.close().await;
}
