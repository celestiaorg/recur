//! Port of Go `schedules_test.go`: occurrences, missed/skipped/interrupted
//! runs, Run now, the plan and loop modes, Keep last N, and the
//! create_workspace tool that shares `launch_prompt_workspace`.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone, Utc};
use recur_agent::error::ErrorKind;
use recur_agent::{Provider, ProviderError};
use recur_agent_tools::CreateWorkspaceRequest;
use recur_domain::{
    AgentRole, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHILD_WORKSPACE_MODEL_MODE,
    CHILD_WORKSPACE_MODEL_PARENT, Flags, PROVIDER_CLAUDE_CODE, Pr, PrSource, ScheduledPrompt,
    ScheduledPromptRun, UserQuestion, WorkspaceState,
};
use tokio_util::task::TaskTracker;

use crate::codes::{MAESTRO_AGENT_CODE, PAIR_AGENT_CODE};
use crate::lifecycle::PHASE_INTERRUPTED;
use crate::loops::test_support::{AsyncProvider, CaptureScript, ConvergeRunner, ctx_runner};
use crate::testing::{TempRepo, TestEngine, add_project, add_run_project, init_repo, wait_for};
use crate::turn::test_support::maestro_fixture;

async fn drain(t: &TaskTracker) {
    t.close();
    t.wait().await;
    t.reopen();
}

fn utc(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
}

fn at_ms(ms: i64) -> DateTime<Local> {
    Utc.timestamp_millis_opt(ms).unwrap().with_timezone(&Local)
}

fn pin_clock(eng: &crate::Engine, t: DateTime<Utc>) {
    eng.set_clock(move || t.with_timezone(&Local));
}

fn scripted(session: &'static str, reply: &'static str) -> Arc<CaptureScript> {
    CaptureScript::new(move |_| maestro_fixture(session, reply))
}

struct Sx {
    te: TestEngine,
    _repo: TempRepo,
    proj: String,
}

impl Sx {
    fn new(provider: Option<Arc<dyn Provider>>) -> Self {
        let te = TestEngine::new(provider);
        let repo = init_repo();
        let proj = add_project(&te.store, &repo.path);
        Self {
            te,
            _repo: repo,
            proj,
        }
    }

    fn eng(&self) -> &Arc<crate::Engine> {
        &self.te.engine
    }

    /// Go `saveTestSchedule`.
    async fn save(&self, project_id: &str) -> ScheduledPrompt {
        pin_clock(self.eng(), utc(2026, 9, 8, 8));
        self.eng()
            .save_scheduled_prompt(ScheduledPrompt {
                project_id: project_id.into(),
                name: "Check tests".into(),
                prompt: "Find missing tests".into(),
                base_branch: "main".into(),
                mode: "ask".into(),
                model: "claude-code/sonnet".into(),
                cadence: "daily".into(),
                time: "09:00".into(),
                time_zone: "UTC".into(),
                ..ScheduledPrompt::default()
            })
            .await
            .expect("save schedule")
    }

    fn runs(&self) -> Vec<ScheduledPromptRun> {
        self.te.store.scheduled_prompt_runs().expect("runs")
    }

    async fn tick(&self, now: DateTime<Local>, startup: bool) {
        self.eng()
            .tick_scheduled_prompts(now, startup)
            .await
            .expect("tick");
    }

    async fn tick_locked(&self, now: DateTime<Local>) {
        let _g = self.eng().schedule.lock().await;
        self.tick(now, false).await;
    }

    async fn drain_schedules(&self) {
        drain(&self.eng().tasks.schedules).await;
    }
}

fn previous(p: &ScheduledPrompt) -> ScheduledPromptRun {
    ScheduledPromptRun {
        id: "previous".into(),
        schedule_id: p.id.clone(),
        due_at: p.next_run_at - 86_400_000,
        status: "running".into(),
        ..ScheduledPromptRun::default()
    }
}

fn hold_loop_slots(eng: &crate::Engine) -> usize {
    let mut slots = 0;
    while eng.governor().try_acquire_loop() {
        slots += 1;
    }
    slots
}

fn release_loop_slots(eng: &crate::Engine, slots: usize) {
    for _ in 0..slots {
        eng.governor().release_loop();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduled_prompt_runs_once_in_a_fresh_workspace() {
    let c = scripted("scheduled-session", "Tests checked.");
    let sx = Sx::new(Some(c.provider()));
    let p = sx.save(&sx.proj).await;
    for _ in 0..2 {
        sx.tick_locked(at_ms(p.next_run_at)).await;
    }
    sx.drain_schedules().await;
    let runs = sx.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "finished", "{:?}", runs[0]);
    let ws = sx
        .te
        .store
        .workspace(&runs[0].workspace_id)
        .expect("workspace");
    assert!(
        !ws.worktree_path.is_empty() && !ws.scheduled_prompt_pending,
        "{ws:?}"
    );
    assert_eq!(ws.schedule_id, p.id);
    let spec = c.with_role(AgentRole::Coder).remove(0);
    assert_eq!(spec.worktree.to_string_lossy(), ws.worktree_path);
    for tool in spec.allowed_tools.unwrap_or_default() {
        assert!(
            tool != "Edit" && tool != "Write",
            "Ask scheduled run exposed {tool}"
        );
    }
    let id = sx.eng().run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    assert_ne!(id, ws.id, "Run now reused the workspace");
    sx.te.close().await;
}

#[tokio::test]
async fn a_scheduled_prompt_missed_paused_and_deleted() {
    let sx = Sx::new(None);
    let p = sx.save(&sx.proj).await;
    let due = p.next_run_at;
    sx.tick(at_ms(due), true).await;
    let runs = sx.runs();
    assert!(
        runs.len() == 1 && runs[0].status == "missed" && runs[0].workspace_id.is_empty(),
        "{runs:?}"
    );
    let day = 86_400_000;
    sx.tick(at_ms(due + day + 120_000), false).await;
    let runs = sx.runs();
    assert!(runs.len() == 2 && runs[1].status == "missed", "{runs:?}");
    sx.eng().pause_scheduled_prompt(&p.id, true).await.unwrap();
    sx.tick(at_ms(due + day), false).await;
    assert_eq!(sx.runs().len(), 2);
    sx.eng().delete_scheduled_prompt(&p.id).await.unwrap();
    sx.tick(at_ms(due + 2 * day), false).await;
    assert!(sx.runs().is_empty());
    sx.te.close().await;
}

#[tokio::test]
async fn a_scheduled_prompt_overlap_and_restart() {
    let sx = Sx::new(None);
    let p = sx.save(&sx.proj).await;
    let r = previous(&p);
    sx.te.store.claim_scheduled_prompt(&p, &r).unwrap();
    assert!(sx.eng().run_scheduled_prompt_now(&p.id).await.is_err());
    sx.tick(at_ms(p.next_run_at), false).await;
    let runs = sx.runs();
    assert!(runs.len() == 2 && runs[1].status == "skipped", "{runs:?}");
    sx.tick(at_ms(p.next_run_at), true).await;
    assert_eq!(sx.runs()[0].status, "interrupted");
    let claimed = sx.te.store.claim_scheduled_prompt(&p, &r);
    assert!(matches!(claimed, Ok(false)), "{claimed:?}");
    sx.te.close().await;
}

#[tokio::test]
async fn an_interrupted_run_blocks_overlap_until_idle() {
    let sx = Sx::new(None);
    let p = sx.save(&sx.proj).await;
    let ws = sx
        .eng()
        .create_workspace(&p.project_id, "Needs you", "main")
        .await
        .unwrap();
    let st = &sx.te.store;
    st.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.state = WorkspaceState::Escalated;
        w.phase = "asking a question".into();
    });
    let r = previous(&p);
    st.claim_scheduled_prompt(&p, &r).unwrap();
    st.link_scheduled_prompt_run(&r.id, &ws.id).unwrap();
    sx.tick(sx.eng().now(), true).await;
    let got = sx.runs();
    assert!(got.len() == 1 && got[0].status == "interrupted", "{got:?}");
    assert!(sx.eng().run_scheduled_prompt_now(&p.id).await.is_err());
    st.update_workspace(&ws.id, |w| {
        w.state = WorkspaceState::Coding;
        w.phase = "coding".into();
    });
    assert!(sx.eng().run_scheduled_prompt_now(&p.id).await.is_err());
    sx.tick(at_ms(p.next_run_at), false).await;
    let runs = sx.runs();
    assert!(runs.len() == 2 && runs[1].status == "skipped", "{runs:?}");
    st.update_workspace(&ws.id, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
    });
    sx.eng()
        .run_scheduled_prompt_now(&p.id)
        .await
        .expect("an idle interrupted workspace must not block Run now");
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_setup_failure_fails_the_run() {
    let sx = Sx::new(None);
    let repo = init_repo();
    let proj = add_run_project(
        &sx.te.store,
        recur_domain::Project {
            id: "schedule-fail".into(),
            name: "Fail".into(),
            repo_path: repo.path.clone(),
            base_branch: "main".into(),
            setup_cmd: "exit 1".into(),
            ..recur_domain::Project::default()
        },
    );
    let p = sx.save(&proj.id).await;
    let id = sx.eng().run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    let runs = sx.runs();
    assert!(
        runs.len() == 1
            && runs[0].status == "failed"
            && !runs[0].detail.is_empty()
            && runs[0].workspace_id == id,
        "{runs:?}"
    );
    sx.te.close().await;
}

#[tokio::test]
async fn recovery_does_not_resume_a_scheduled_chat() {
    let sx = Sx::new(None);
    let eng = sx.eng();
    let ws = eng
        .create_workspace(&sx.proj, "Interrupted scheduled run", "main")
        .await
        .unwrap();
    eng.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "sonnet");
    sx.te
        .store
        .update_agent(&ws.id, PAIR_AGENT_CODE, |a| a.status = "working".into());
    let sess = eng
        .ensure_agent_session(
            &ws.id,
            AgentRole::Coder.as_str(),
            PAIR_AGENT_CODE,
            "Agent",
            PROVIDER_CLAUDE_CODE,
        )
        .unwrap();
    sx.te
        .store
        .set_session_status(&sess.id, "streaming")
        .unwrap();
    sx.te
        .store
        .update_workspace(&ws.id, |w| w.scheduled_prompt_pending = true);
    eng.recover();
    assert!(eng.interrupted_chats.lock().is_empty());
    assert!(
        !sx.te
            .store
            .workspace(&ws.id)
            .unwrap()
            .scheduled_prompt_pending
    );
    sx.te.close().await;
}

#[tokio::test]
async fn scheduled_prompt_validation() {
    let sx = Sx::new(None);
    let p = sx.save(&sx.proj).await;
    let mutations: [fn(&mut ScheduledPrompt); 6] = [
        |p| p.prompt = " ".into(),
        |p| p.project_id = "missing".into(),
        |p| p.base_branch = "missing".into(),
        |p| p.mode = "invalid".into(),
        |p| p.model = "invalid/model".into(),
        |p| p.time_zone = "Local".into(),
    ];
    for mutate in mutations {
        let mut bad = p.clone();
        mutate(&mut bad);
        assert!(
            sx.eng().save_scheduled_prompt(bad.clone()).await.is_err(),
            "accepted {bad:?}"
        );
    }
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_failure_fails_the_run() {
    let provider = AsyncProvider::wrap(|_req, _cx| async {
        Err(ProviderError::new(ErrorKind::Failed, "provider exploded"))
    });
    let sx = Sx::new(Some(provider));
    let p = sx.save(&sx.proj).await;
    sx.eng().run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    let runs = sx.runs();
    assert!(
        runs.len() == 1 && runs[0].status == "failed" && !runs[0].detail.is_empty(),
        "{runs:?}"
    );
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_now_at_the_due_time_consumes_the_occurrence() {
    let sx = Sx::new(Some(scripted("scheduled", "Done").provider()));
    let p = sx.save(&sx.proj).await;
    let due = Utc.timestamp_millis_opt(p.next_run_at).unwrap();
    pin_clock(sx.eng(), due);
    sx.eng().run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    sx.tick(at_ms(p.next_run_at), false).await;
    let saved = sx.eng().scheduled_prompt(&p.id).unwrap();
    assert!(
        saved.next_run_at > p.next_run_at && sx.runs().len() == 1,
        "{saved:?}"
    );
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_joins_the_scheduled_worker() {
    for mode in [CHAT_MODE_ASK, CHAT_MODE_LOOP] {
        let sx = Sx::new(Some(ctx_runner()));
        let mut p = sx.save(&sx.proj).await;
        p.mode = mode.into();
        sx.eng().save_scheduled_prompt(p.clone()).await.unwrap();
        sx.eng().start_scheduled_prompts().await.unwrap();
        let id = sx.eng().run_scheduled_prompt_now(&p.id).await.unwrap();
        let eng = Arc::clone(sx.eng());
        let active = move || {
            eng.live_chat_turn(&id, PAIR_AGENT_CODE).is_some()
                || eng.running_loop_by_id(&id).is_some()
        };
        wait_for(
            Duration::from_secs(5),
            "scheduled execution starts",
            &active,
        )
        .await;
        sx.eng().shutdown().await;
        assert!(
            sx.eng().schedule.lock().await.ticker.is_none(),
            "{mode}: Shutdown left the scheduler running"
        );
        assert!(!active(), "{mode}: Shutdown left the execution running");
        assert!(sx.eng().run_scheduled_prompt_now(&p.id).await.is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_mode_remount_blocks_overlap() {
    let sx = Sx::new(Some(scripted("scheduled", "Done").provider()));
    let p = sx.save(&sx.proj).await;
    let id = sx.eng().run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    let generation = sx.eng().alloc_continue_gen(&id, PAIR_AGENT_CODE);
    let mut r = sx.runs().remove(0);
    r.status = "waiting".into();
    sx.te.store.update_scheduled_prompt_run(&r).unwrap();
    sx.te
        .store
        .update_workspace(&id, |w| w.scheduled_prompt_pending = true);
    assert!(sx.eng().run_scheduled_prompt_now(&p.id).await.is_err());
    sx.tick(sx.eng().now(), false).await;
    assert_eq!(sx.runs()[0].status, "waiting");
    sx.eng().drop_continue_gen(generation);
    sx.tick(sx.eng().now(), false).await;
    let ws = sx.te.store.workspace(&id).unwrap();
    assert_eq!(sx.runs()[0].status, "finished");
    assert!(!ws.scheduled_prompt_pending);
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduled_plan_and_loop_modes() {
    let c = scripted("scheduled", "Plan ready");
    let sx = Sx::new(Some(c.provider()));
    let eng = sx.eng();
    let mut p = sx.save(&sx.proj).await;
    p.mode = CHAT_MODE_PLAN.into();
    p.cadence = "cron".into();
    p.cron = "*/15 9-17 * * MON-FRI".into();
    p.flags = Flags::from([("effort".to_owned(), "low".to_owned())]);
    p.simplify = Some(false);
    let mut p = eng.save_scheduled_prompt(p).await.unwrap();
    eng.run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    let runs = sx.runs();
    assert!(runs.len() == 1 && runs[0].status == "finished", "{runs:?}");
    let spec = c.with_role(AgentRole::Coder).remove(0);
    assert_eq!(spec.flags.get("effort").map(String::as_str), Some("low"));
    assert!(!spec.append_system_prompt.contains("SIMPLIFY MODE"));
    assert_eq!(
        eng.chat_simplify(&runs[0].workspace_id, PAIR_AGENT_CODE),
        Some(false)
    );

    let slots = hold_loop_slots(eng);
    p.id.clear();
    p.mode = CHAT_MODE_LOOP.into();
    p.name = "Short name".into();
    p.prompt = "Do the full task\nwith extra instructions".into();
    let p = eng.save_scheduled_prompt(p).await.unwrap();
    assert!(!p.loop_id.is_empty() && !p.policy_id.is_empty());
    let id = eng.run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    let st = &sx.te.store;
    let ws = st.workspace(&id).unwrap();
    assert!(ws.has_run() && ws.state == WorkspaceState::Queued, "{ws:?}");
    assert_eq!(
        (ws.goal.as_str(), ws.short.as_str()),
        (p.name.as_str(), p.name.as_str())
    );
    assert_eq!(eng.chat_loop_goal(&id, PAIR_AGENT_CODE).0, p.prompt);
    let (model, flags, mode) = eng.workspace_chat_selection(&id, PAIR_AGENT_CODE);
    assert_eq!(
        (model.as_str(), mode.as_str()),
        (p.model.as_str(), CHAT_MODE_LOOP)
    );
    assert_eq!(flags.get("effort").map(String::as_str), Some("low"));
    assert_eq!(eng.chat_simplify(&id, MAESTRO_AGENT_CODE), Some(false));
    for state in [
        WorkspaceState::Queued,
        WorkspaceState::Paused,
        WorkspaceState::Escalated,
    ] {
        st.update_workspace(&id, |w| {
            w.state = state.clone();
            w.phase = "needs input".into();
        });
        assert!(
            eng.run_scheduled_prompt_now(&p.id).await.is_err(),
            "allowed overlap in {state:?}"
        );
    }
    st.update_workspace(&id, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "artifact".into();
    });
    assert!(
        !eng.scheduled_chat_active(&id),
        "an artifact pause kept the schedule busy"
    );
    st.update_workspace(&id, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
    });
    sx.tick(eng.now(), false).await;
    for run in sx.runs() {
        assert!(
            run.schedule_id != p.id || run.status == "finished",
            "{run:?}"
        );
    }
    release_loop_slots(eng, slots);
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_on_startup_cron_runs_only_at_start() {
    let sx = Sx::new(Some(scripted("scheduled", "Done").provider()));
    let mut p = sx.save(&sx.proj).await;
    p.cadence = "cron".into();
    p.cron = "@reboot".into();
    sx.eng().save_scheduled_prompt(p).await.unwrap();
    sx.tick(sx.eng().now(), false).await;
    assert!(
        sx.runs().is_empty(),
        "a startup schedule ran on an ordinary tick"
    );
    sx.eng().start_scheduled_prompts().await.unwrap();
    sx.drain_schedules().await;
    sx.eng().start_scheduled_prompts().await.unwrap();
    let runs = sx.runs();
    assert!(runs.len() == 1 && runs[0].status == "finished", "{runs:?}");
    sx.te.close().await;
}

#[tokio::test]
async fn a_scheduled_loop_is_not_replayed_on_recovery() {
    let sx = Sx::new(None);
    for state in [WorkspaceState::Queued, WorkspaceState::Coding] {
        let ws = sx
            .eng()
            .create_workspace(&sx.proj, "Interrupted loop", "main")
            .await
            .unwrap();
        sx.te.store.update_workspace(&ws.id, |w| {
            w.run_started = true;
            w.state = state.clone();
            w.scheduled_prompt_pending = true;
        });
        sx.eng().recover();
        let got = sx.te.store.workspace(&ws.id).unwrap();
        assert!(
            got.state == WorkspaceState::Paused
                && got.phase != PHASE_INTERRUPTED
                && !got.scheduled_prompt_pending,
            "eligible for automatic replay: {got:?}"
        );
    }
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduled_loop_completes_its_graph() {
    let runner = ConvergeRunner::new(&[0]);
    let sx = Sx::new(Some(runner.provider()));
    sx.te.install_fixture_pack();
    let mut p = sx.save(&sx.proj).await;
    p.mode = CHAT_MODE_LOOP.into();
    p.loop_id = "test.loops.converge".into();
    p.policy_id = "test.policies.fail".into();
    p.name = "Short name".into();
    p.prompt = "Do the full task\nwith extra instructions".into();
    let p = sx.eng().save_scheduled_prompt(p).await.unwrap();
    let id = sx.eng().run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    let (st, eng, wid) = (Arc::clone(&sx.te.store), Arc::clone(sx.eng()), id.clone());
    wait_for(
        Duration::from_secs(15),
        "scheduled loop completes graph",
        || {
            st.workspace(&wid).is_some_and(|w| w.phase == "idle")
                && eng.running_loop_by_id(&wid).is_none()
        },
    )
    .await;
    sx.tick(sx.eng().now(), false).await;
    let runs = sx.runs();
    assert!(runs.len() == 1 && runs[0].status == "finished", "{runs:?}");
    let ws = sx.te.store.workspace(&id).unwrap();
    assert!(
        !ws.scheduled_prompt_pending && ws.loop_id == p.loop_id,
        "{ws:?}"
    );
    assert_eq!(ws.goal, p.name);
    assert!(
        runner
            .state
            .lock()
            .specs
            .iter()
            .any(|s| s.prompt.contains("with extra instructions")),
        "planner/coder did not receive the full multiline prompt"
    );
    sx.te.close().await;
}

fn spawned(sx: &Sx, title: &str) -> recur_domain::Workspace {
    sx.te
        .store
        .workspaces()
        .into_iter()
        .rfind(|w| w.goal == title)
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_workspace_from_chat() {
    let c = scripted("spawned", "Triaged.");
    let sx = Sx::new(Some(c.provider()));
    let eng = sx.eng();
    let caller = eng.create_workspace(&sx.proj, "triage", "").await.unwrap();
    eng.ensure_pair_agent(&caller.id, PAIR_AGENT_CODE, "");
    eng.remember_workspace_chat_selection(
        &caller.id,
        PAIR_AGENT_CODE,
        "sonnet",
        None,
        "agent",
        Some(false),
    )
    .await;
    let req = CreateWorkspaceRequest {
        title: "ACME-12: Fix login redirect".into(),
        prompt: "Fix the redirect described in ACME-12.".into(),
        ..CreateWorkspaceRequest::default()
    };
    let reply = eng
        .create_workspace_from_chat(&caller.id, PAIR_AGENT_CODE, req.clone())
        .await;
    assert!(reply.starts_with("Created "), "{reply}");
    sx.drain_schedules().await;
    let ws = spawned(&sx, &req.title);
    assert!(
        !ws.id.is_empty()
            && ws.id != caller.id
            && ws.project_id == sx.proj
            && !ws.worktree_path.is_empty()
            && !ws.scheduled_prompt_pending,
        "{ws:?}"
    );
    assert!(ws.schedule_id.is_empty());
    let spec = c
        .with_role(AgentRole::Coder)
        .into_iter()
        .find(|s| s.worktree.to_string_lossy() == ws.worktree_path)
        .expect("spawned turn");
    assert_eq!(spec.prompt, req.prompt);
    assert_eq!(eng.chat_simplify(&ws.id, PAIR_AGENT_CODE), Some(false));
    let again = eng
        .create_workspace_from_chat(&caller.id, PAIR_AGENT_CODE, req.clone())
        .await;
    assert!(again.contains("already exists"), "{again}");
    assert_eq!(sx.te.store.workspaces().len(), 2);
    for bad in [
        CreateWorkspaceRequest {
            project_id: "missing".into(),
            ..CreateWorkspaceRequest::default()
        },
        CreateWorkspaceRequest {
            base_branch: "missing".into(),
            ..CreateWorkspaceRequest::default()
        },
        CreateWorkspaceRequest {
            mode: "loop".into(),
            loop_id: "no.such.loop".into(),
            ..CreateWorkspaceRequest::default()
        },
    ] {
        let bad = CreateWorkspaceRequest {
            title: "x".into(),
            prompt: "y".into(),
            ..bad
        };
        let r = eng
            .create_workspace_from_chat(&caller.id, PAIR_AGENT_CODE, bad.clone())
            .await;
        assert!(r.starts_with("create_workspace:"), "{bad:?} = {r}");
    }
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_workspace_from_chat_follows_the_child_model_setting() {
    for (setting, want, not) in [
        ("", "composer", ""),
        (CHILD_WORKSPACE_MODEL_PARENT, "composer", ""),
        (CHILD_WORKSPACE_MODEL_MODE, "sonnet", "composer"),
    ] {
        let c = scripted("spawned", "Triaged.");
        let sx = Sx::new(Some(c.provider()));
        let eng = sx.eng();
        let caller = eng.create_workspace(&sx.proj, "triage", "").await.unwrap();
        eng.ensure_pair_agent(&caller.id, PAIR_AGENT_CODE, "");
        eng.remember_workspace_chat_selection(
            &caller.id,
            PAIR_AGENT_CODE,
            "cursor/composer-2.5",
            None,
            "agent",
            None,
        )
        .await;
        let mut set = sx.te.store.settings();
        set.model_agent = "claude-code/sonnet".into();
        set.child_workspace_model = setting.into();
        sx.te.store.set_settings(set);
        let reply = eng
            .create_workspace_from_chat(
                &caller.id,
                PAIR_AGENT_CODE,
                CreateWorkspaceRequest {
                    title: "ACME-12: model pick".into(),
                    prompt: "Fix it.".into(),
                    ..CreateWorkspaceRequest::default()
                },
            )
            .await;
        assert!(reply.starts_with("Created "), "{reply}");
        sx.drain_schedules().await;
        let spec = c.with_role(AgentRole::Coder).remove(0);
        assert!(
            spec.model.contains(want),
            "{setting:?}: model = {}",
            spec.model
        );
        if !not.is_empty() {
            assert!(
                !spec.model.contains(not),
                "{setting:?}: model = {}",
                spec.model
            );
        }
        sx.te.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_once_schedule_stays_exhausted_after_run_now_and_resume() {
    let sx = Sx::new(Some(scripted("once", "Done").provider()));
    let eng = sx.eng();
    pin_clock(eng, utc(2026, 9, 8, 8));
    let p = eng
        .save_scheduled_prompt(ScheduledPrompt {
            project_id: sx.proj.clone(),
            name: "Once".into(),
            prompt: "ping".into(),
            base_branch: "main".into(),
            mode: "ask".into(),
            model: "claude-code/sonnet".into(),
            cadence: "once".into(),
            date: "2026-09-10".into(),
            time: "09:00".into(),
            time_zone: "UTC".into(),
            ..ScheduledPrompt::default()
        })
        .await
        .unwrap();
    pin_clock(eng, utc(2026, 9, 10, 8));
    eng.run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    eng.pause_scheduled_prompt(&p.id, true).await.unwrap();
    eng.pause_scheduled_prompt(&p.id, false).await.unwrap();
    assert_eq!(eng.scheduled_prompt(&p.id).unwrap().next_run_at, 0);
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_workspace_clears_pending_without_a_run_row() {
    let sx = Sx::new(Some(scripted("spawned", "Triaged.").provider()));
    let eng = sx.eng();
    let caller = eng.create_workspace(&sx.proj, "triage", "").await.unwrap();
    eng.ensure_pair_agent(&caller.id, PAIR_AGENT_CODE, "");
    let slots = hold_loop_slots(eng);
    sx.te.install_fixture_pack();
    let req = CreateWorkspaceRequest {
        title: "ACME-12: Fix login".into(),
        prompt: "Fix the redirect.\nInclude tests.".into(),
        mode: CHAT_MODE_LOOP.into(),
        loop_id: "test.loops.converge".into(),
        policy_id: "test.policies.fail".into(),
        ..CreateWorkspaceRequest::default()
    };
    let reply = eng
        .create_workspace_from_chat(&caller.id, PAIR_AGENT_CODE, req.clone())
        .await;
    assert!(reply.starts_with("Created "), "{reply}");
    sx.drain_schedules().await;
    let ws = spawned(&sx, &req.title);
    assert!(!ws.id.is_empty() && ws.scheduled_prompt_pending, "{ws:?}");
    assert!(sx.runs().is_empty());
    assert_eq!(eng.chat_loop_goal(&ws.id, PAIR_AGENT_CODE).0, req.prompt);
    sx.te.store.update_workspace(&ws.id, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
    });
    sx.tick(eng.now(), false).await;
    assert!(
        !sx.te
            .store
            .workspace(&ws.id)
            .unwrap()
            .scheduled_prompt_pending,
        "tick left the pending marker with no schedule-run row"
    );
    release_loop_slots(eng, slots);
    sx.te.close().await;
}

#[tokio::test]
async fn scheduled_workspace_origins_are_stamped() {
    let sx = Sx::new(None);
    let ws = sx
        .eng()
        .create_workspace(&sx.proj, "legacy run", "main")
        .await
        .unwrap();
    let p = sx.save(&sx.proj).await;
    let r = ScheduledPromptRun {
        id: "run".into(),
        schedule_id: p.id.clone(),
        due_at: 100,
        status: "finished".into(),
        ..ScheduledPromptRun::default()
    };
    sx.te.store.claim_scheduled_prompt(&p, &r).unwrap();
    sx.te
        .store
        .link_scheduled_prompt_run(&r.id, &ws.id)
        .unwrap();
    sx.eng().stamp_scheduled_workspace_origins();
    assert_eq!(sx.te.store.workspace(&ws.id).unwrap().schedule_id, p.id);
    sx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keep_last_n_prunes_the_oldest() {
    let sx = Sx::new(Some(
        scripted("scheduled-session", "Tests checked.").provider(),
    ));
    let p = sx.save(&sx.proj).await;
    let mut set = sx.te.store.settings();
    set.scheduled_workspace_keep = 1;
    sx.te.store.set_settings(set);
    sx.tick_locked(at_ms(p.next_run_at)).await;
    sx.drain_schedules().await;
    let first = sx.runs()[0].workspace_id.clone();
    sx.eng().run_scheduled_prompt_now(&p.id).await.unwrap();
    sx.drain_schedules().await;
    let runs = sx.runs();
    assert_eq!(runs.len(), 2);
    assert!(sx.te.store.workspace(&first).is_some_and(|w| w.archived));
    let newer = sx.te.store.workspace(&runs[1].workspace_id).unwrap();
    assert!(!newer.archived && newer.schedule_id == p.id, "{newer:?}");
    sx.te.close().await;
}

/// Go `scheduledKeepPair`.
async fn keep_pair(keep: i64) -> (Sx, String, recur_domain::Workspace, recur_domain::Workspace) {
    let sx = Sx::new(None);
    let p = sx.save(&sx.proj).await;
    let mut set = sx.te.store.settings();
    set.scheduled_workspace_keep = keep;
    sx.te.store.set_settings(set);
    let older = sx
        .eng()
        .create_workspace(&p.project_id, "older", "main")
        .await
        .unwrap();
    let newer = sx
        .eng()
        .create_workspace(&p.project_id, "newer", "main")
        .await
        .unwrap();
    let st = &sx.te.store;
    let older = st
        .update_workspace(&older.id, |w| w.schedule_id = p.id.clone())
        .unwrap();
    let newer = st
        .update_workspace(&newer.id, |w| w.schedule_id = p.id.clone())
        .unwrap();
    (sx, p.id, older, newer)
}

#[tokio::test]
async fn keep_skips_a_parked_solo_chat() {
    let (sx, id, older, _) = keep_pair(1).await;
    sx.eng().ask_user_broker().park(
        UserQuestion {
            workspace_id: older.id.clone(),
            agent_code: "PA2".into(),
            question: "color?".into(),
            ..UserQuestion::default()
        },
        None,
    );
    sx.eng().prune_scheduled_workspaces(&id).await;
    assert!(
        sx.te
            .store
            .workspace(&older.id)
            .is_some_and(|w| !w.archived)
    );
    assert_eq!(sx.eng().pending_ask_users().len(), 1);
    sx.te.close().await;
}

#[tokio::test]
async fn keep_skips_a_solo_open_pr() {
    let (sx, id, older, _) = keep_pair(1).await;
    sx.te
        .store
        .add_pr(Pr {
            id: "gh-42".into(),
            workspace_id: older.id.clone(),
            number: 42,
            source: PrSource::GitHub,
            draft: true,
            state: "In review".into(),
            ..Pr::default()
        })
        .unwrap();
    assert_ne!(older.state, WorkspaceState::PrOpen);
    sx.eng().prune_scheduled_workspaces(&id).await;
    assert!(
        sx.te
            .store
            .workspace(&older.id)
            .is_some_and(|w| !w.archived)
    );
    sx.te.close().await;
}

#[tokio::test]
async fn keep_skips_a_dirty_worktree() {
    let (sx, id, older, _) = keep_pair(1).await;
    assert!(!older.worktree_path.is_empty());
    let path = std::path::Path::new(&older.worktree_path).join("scratch.txt");
    std::fs::write(&path, "keep me\n").unwrap();
    sx.eng().prune_scheduled_workspaces(&id).await;
    assert!(
        sx.te
            .store
            .workspace(&older.id)
            .is_some_and(|w| !w.archived)
    );
    assert!(path.exists(), "the uncommitted file must stay");
    sx.te.close().await;
}

#[tokio::test]
async fn keep_prunes_an_artifact_completion() {
    let (sx, id, older, newer) = keep_pair(1).await;
    sx.te.store.update_workspace(&older.id, |w| {
        w.run_started = true;
        w.state = WorkspaceState::Paused;
        w.phase = "artifact".into();
    });
    sx.eng().prune_scheduled_workspaces(&id).await;
    assert!(sx.te.store.workspace(&older.id).is_some_and(|w| w.archived));
    assert!(
        sx.te
            .store
            .workspace(&newer.id)
            .is_some_and(|w| !w.archived)
    );
    sx.te.close().await;
}
